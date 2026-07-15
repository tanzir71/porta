use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, FromRequest, Multipart, State},
    http::{
        header::{AUTHORIZATION, ETAG, IF_NONE_MATCH, WWW_AUTHENTICATE},
        HeaderValue, Method, Request, StatusCode,
    },
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, Local};
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use subtle::ConstantTimeEq;
use tokio::{io::AsyncWriteExt, sync::oneshot, task::JoinHandle};
use tower_http::services::ServeDir;

use crate::{credentials, shares::Share};

const INVALID_FOLDER: &str = "This folder was moved or deleted. Pick it again to reshare.";
const START_ERROR: &str =
    "Porta couldn't start a local server. Quit and reopen Porta, then try again.";
const STOP_ERROR: &str =
    "Porta couldn't stop the local server cleanly. Quit Porta before sharing it again.";
const MISSING_PASSWORD: &str = "Porta couldn't find this share's password in Keychain. Edit the share, set the password again, then try again.";
const MAX_UPLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const LISTING_TEMPLATE: &str = include_str!("../../server-templates/listing.html");
const LISTING_DISABLED_PAGE: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="robots" content="noindex"><title>Folder browsing is off</title></head><body><main><h1>Folder browsing is off</h1><p>Ask the person sharing this folder for a direct link to a file.</p></main></body></html>"#;
const FOLDER_SVG: &str = r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M3 19V6a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>"#;
const FILE_SVG: &str = r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M6 2h8l4 4v16H6V2Z" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/><path d="M14 2v5h4" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>"#;

/// Settings needed to start one folder server.
pub struct FileServerConfig {
    root: PathBuf,
    share_name: String,
    show_listing: bool,
    allow_uploads: bool,
    password: Option<String>,
}

impl FileServerConfig {
    /// Creates a listing-enabled configuration with uploads hidden.
    pub fn new(root: impl Into<PathBuf>, share_name: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            share_name: share_name.into(),
            show_listing: true,
            allow_uploads: false,
            password: None,
        }
    }

    /// Builds a configuration for a persisted share, reading its password from Keychain.
    pub fn for_share(share: &Share) -> Result<Self, String> {
        let root = share.path.clone().ok_or_else(|| {
            "This folder share has no folder. Edit the share and pick it again.".to_owned()
        })?;
        let password = if share.password_protected {
            Some(credentials::get_password(&share.id)?.ok_or_else(|| MISSING_PASSWORD.to_owned())?)
        } else {
            None
        };

        Ok(Self::new(root, share.name.clone())
            .show_listing(share.show_listing)
            .allow_uploads(share.allow_uploads)
            .password(password))
    }

    /// Controls whether visitors may browse directory contents.
    pub fn show_listing(mut self, show_listing: bool) -> Self {
        self.show_listing = show_listing;
        self
    }

    /// Controls whether the listing shows its upload widget.
    pub fn allow_uploads(mut self, allow_uploads: bool) -> Self {
        self.allow_uploads = allow_uploads;
        self
    }

    /// Supplies an in-memory password; persisted configurations should use [`Self::for_share`].
    pub fn password(mut self, password: Option<String>) -> Self {
        self.password = password;
        self
    }
}

#[derive(Clone)]
struct ListingState {
    root: PathBuf,
    share_name: String,
    show_listing: bool,
    allow_uploads: bool,
}

#[derive(Clone)]
struct AuthState {
    password: Option<Arc<str>>,
}

struct DirectoryEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<std::time::SystemTime>,
}

/// One loopback HTTP server for one shared folder.
pub struct FileServer {
    address: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl FileServer {
    /// Starts serving the configured folder from an OS-assigned loopback port.
    pub async fn start(config: FileServerConfig) -> Result<Self, String> {
        let root = tokio::fs::canonicalize(&config.root)
            .await
            .map_err(|_| INVALID_FOLDER.to_owned())?;
        let metadata = tokio::fs::metadata(&root)
            .await
            .map_err(|_| INVALID_FOLDER.to_owned())?;
        if !metadata.is_dir() {
            return Err("Choose a folder instead of a file, then try again.".to_owned());
        }

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| START_ERROR.to_owned())?;
        let address = listener.local_addr().map_err(|_| START_ERROR.to_owned())?;
        let listing_state = ListingState {
            root: root.clone(),
            share_name: config.share_name,
            show_listing: config.show_listing,
            allow_uploads: config.allow_uploads,
        };
        let auth_state = AuthState {
            password: config.password.map(Arc::from),
        };
        let app = Router::new()
            .fallback_service(ServeDir::new(&root))
            .layer(middleware::from_fn_with_state(
                listing_state,
                serve_directory_listing,
            ))
            .layer(middleware::from_fn_with_state(root.clone(), add_etag))
            .layer(middleware::from_fn_with_state(
                auth_state,
                require_basic_auth,
            ))
            .layer(middleware::from_fn_with_state(root, enforce_shared_root))
            .layer(DefaultBodyLimit::disable());
        let (shutdown, shutdown_signal) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_signal.await;
                })
                .await
        });

        Ok(Self {
            address,
            shutdown: Some(shutdown),
            task: Some(task),
        })
    }

    /// Returns the loopback address cloudflared should connect to.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Stops accepting requests and waits for active requests to finish.
    pub async fn stop(mut self) -> Result<(), String> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let Some(task) = self.task.take() else {
            return Ok(());
        };

        match task.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) | Err(_) => Err(STOP_ERROR.to_owned()),
        }
    }
}

impl Drop for FileServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn enforce_shared_root(
    State(root): State<PathBuf>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if resolve_request_path(&root, request.uri().path())
        .await
        .is_none()
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(request).await
}

async fn require_basic_auth(
    State(state): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let Some(expected) = state.password.as_deref() else {
        return next.run(request).await;
    };
    let authorized = request
        .headers()
        .get(AUTHORIZATION)
        .is_some_and(|header| valid_basic_credentials(header, expected));
    if authorized {
        return next.run(request).await;
    }

    let mut response = (
        StatusCode::UNAUTHORIZED,
        "Enter this share's password to continue.",
    )
        .into_response();
    response.headers_mut().insert(
        WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"Porta\""),
    );
    response
}

fn valid_basic_credentials(header: &HeaderValue, expected: &str) -> bool {
    let Some((scheme, encoded)) = header.to_str().ok().and_then(|value| value.split_once(' '))
    else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("basic") {
        return false;
    }
    let Ok(decoded) = BASE64.decode(encoded.trim()) else {
        return false;
    };
    let Ok(credentials) = std::str::from_utf8(&decoded) else {
        return false;
    };
    let Some((_, supplied)) = credentials.split_once(':') else {
        return false;
    };

    bool::from(supplied.as_bytes().ct_eq(expected.as_bytes()))
}

async fn serve_directory_listing(
    State(state): State<ListingState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request.method() == Method::POST {
        return receive_uploads(&state, request).await;
    }
    if request.method() != Method::GET {
        return next.run(request).await;
    }

    let request_path = request.uri().path().to_owned();
    let Some(directory) = resolve_request_path(&state.root, &request_path).await else {
        return next.run(request).await;
    };
    if !directory.is_dir() {
        return next.run(request).await;
    }

    if !state.show_listing {
        let has_safe_root_index = if request_path == "/" {
            match resolve_request_path(&state.root, "/index.html").await {
                Some(index) => tokio::fs::metadata(index)
                    .await
                    .is_ok_and(|metadata| metadata.is_file()),
                None => false,
            }
        } else {
            false
        };
        if has_safe_root_index {
            return next.run(request).await;
        }
        return (StatusCode::FORBIDDEN, Html(LISTING_DISABLED_PAGE)).into_response();
    }

    if !request_path.ends_with('/') {
        let mut location = format!("{request_path}/");
        if let Some(query) = request.uri().query() {
            location.push('?');
            location.push_str(query);
        }
        return Redirect::permanent(&location).into_response();
    }

    match render_listing(&state, &directory, &request_path).await {
        Some(listing) => Html(listing).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn receive_uploads(state: &ListingState, request: Request<Body>) -> Response {
    if !state.allow_uploads {
        return (
            StatusCode::FORBIDDEN,
            "Uploads are turned off for this folder. Ask the host to enable uploads, then try again.",
        )
            .into_response();
    }

    let request_path = request.uri().path().to_owned();
    let Some(directory) = resolve_request_path(&state.root, &request_path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !tokio::fs::metadata(&directory)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
    {
        return StatusCode::NOT_FOUND.into_response();
    }

    let mut multipart = match Multipart::from_request(request, &()).await {
        Ok(multipart) => multipart,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "Porta couldn't read that upload. Choose the files and try again.",
            )
                .into_response();
        }
    };
    let mut uploaded = false;

    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "The upload was interrupted. Choose the files and try again.",
                )
                    .into_response();
            }
        };
        if field.name() != Some("files") {
            continue;
        }
        let Some(name) = field.file_name().and_then(safe_upload_name) else {
            return (
                StatusCode::BAD_REQUEST,
                "One file has an invalid name. Rename it, then try again.",
            )
                .into_response();
        };
        let (mut file, path) = match reserve_upload_file(&directory, &name).await {
            Ok(reserved) => reserved,
            Err(_) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Porta couldn't prepare that upload. Ask the host to check free disk space, then try again.",
                )
                    .into_response();
            }
        };
        let mut written = 0_u64;

        loop {
            let chunk = match field.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(_) => {
                    drop(file);
                    let _ = tokio::fs::remove_file(&path).await;
                    return (
                        StatusCode::BAD_REQUEST,
                        "The upload was interrupted. Choose the files and try again.",
                    )
                        .into_response();
                }
            };
            written = match checked_upload_size(written, chunk.len()) {
                Some(total) => total,
                _ => {
                    drop(file);
                    let _ = tokio::fs::remove_file(&path).await;
                    return (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "This file is larger than 2 GB. Choose a smaller file and try again.",
                    )
                        .into_response();
                }
            };
            if file.write_all(&chunk).await.is_err() {
                drop(file);
                let _ = tokio::fs::remove_file(&path).await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Porta couldn't save that file. Ask the host to check free disk space, then try again.",
                )
                    .into_response();
            }
        }

        if file.flush().await.is_err() {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Porta couldn't finish saving that file. Ask the host to check free disk space, then try again.",
            )
                .into_response();
        }
        uploaded = true;
    }

    if !uploaded {
        return (
            StatusCode::BAD_REQUEST,
            "Choose at least one file, then try again.",
        )
            .into_response();
    }
    Redirect::to(&request_path).into_response()
}

fn checked_upload_size(written: u64, chunk_size: usize) -> Option<u64> {
    written
        .checked_add(chunk_size as u64)
        .filter(|total| *total <= MAX_UPLOAD_BYTES)
}

fn safe_upload_name(name: &str) -> Option<String> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return None;
    }
    Some(name.to_owned())
}

async fn reserve_upload_file(
    directory: &Path,
    requested_name: &str,
) -> std::io::Result<(tokio::fs::File, PathBuf)> {
    let mut copy = 1_u64;
    loop {
        let name = collision_name(requested_name, copy);
        let path = directory.join(name);
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                copy = copy.checked_add(1).ok_or_else(|| {
                    std::io::Error::other("upload copy number exceeded its supported range")
                })?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn collision_name(requested_name: &str, copy: u64) -> String {
    if copy == 1 {
        return requested_name.to_owned();
    }
    let path = Path::new(requested_name);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(requested_name);
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if !extension.is_empty() => {
            format!("{stem} ({copy}).{extension}")
        }
        _ => format!("{stem} ({copy})"),
    }
}

async fn render_listing(
    state: &ListingState,
    directory: &Path,
    request_path: &str,
) -> Option<String> {
    let mut reader = tokio::fs::read_dir(directory).await.ok()?;
    let mut entries = Vec::new();
    while let Ok(Some(entry)) = reader.next_entry().await {
        let canonical = match tokio::fs::canonicalize(entry.path()).await {
            Ok(path) if path.starts_with(&state.root) => path,
            _ => continue,
        };
        let metadata = match tokio::fs::metadata(canonical).await {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        entries.push(DirectoryEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: metadata.is_dir(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        });
    }
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });

    let rows = if entries.is_empty() {
        r#"<div class="empty-msg">This folder is empty</div>"#.to_owned()
    } else {
        entries
            .iter()
            .map(|entry| render_row(entry, request_path))
            .collect::<String>()
    };
    let uploads = if state.allow_uploads { "block" } else { "none" };

    Some(
        LISTING_TEMPLATE
            .replace("{{SHARE_NAME}}", &escape_html(&state.share_name))
            .replace("{{BREADCRUMB}}", &render_breadcrumb(request_path))
            .replace("{{ROWS}}", &rows)
            .replace("{{UPLOADS}}", uploads),
    )
}

fn render_row(entry: &DirectoryEntry, request_path: &str) -> String {
    let encoded_name = utf8_percent_encode(&entry.name, NON_ALPHANUMERIC);
    let trailing_slash = if entry.is_dir { "/" } else { "" };
    let href = format!("{request_path}{encoded_name}{trailing_slash}");
    let icon = if entry.is_dir { FOLDER_SVG } else { FILE_SVG };
    let size = if entry.is_dir {
        "—".to_owned()
    } else {
        human_size(entry.size)
    };
    let modified = entry
        .modified
        .map(format_date)
        .unwrap_or_else(|| "—".to_owned());

    format!(
        r#"<a class="row" href="{}"><span class="ic">{icon}</span><span class="nm">{}</span><span class="sz">{size}</span><span class="dt">{modified}</span></a>"#,
        escape_html(&href),
        escape_html(&entry.name),
    )
}

fn render_breadcrumb(request_path: &str) -> String {
    let decoded = percent_decode_str(request_path.trim_matches('/')).decode_utf8_lossy();
    let mut breadcrumb = r#"<a href="/">Home</a>"#.to_owned();
    let mut href = String::from("/");
    for segment in decoded.split('/').filter(|segment| !segment.is_empty()) {
        href.push_str(&utf8_percent_encode(segment, NON_ALPHANUMERIC).to_string());
        href.push('/');
        breadcrumb.push_str(r#" <span>/</span> <a href=""#);
        breadcrumb.push_str(&escape_html(&href));
        breadcrumb.push_str(r#"">"#);
        breadcrumb.push_str(&escape_html(segment));
        breadcrumb.push_str("</a>");
    }
    breadcrumb
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let formatted = format!("{value:.1}");
    format!("{} {}", formatted.trim_end_matches(".0"), UNITS[unit])
}

fn format_date(time: std::time::SystemTime) -> String {
    let date: DateTime<Local> = time.into();
    date.format("%b %-d, %Y").to_string()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn add_etag(State(root): State<PathBuf>, request: Request<Body>, next: Next) -> Response {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return next.run(request).await;
    }

    let etag = etag_for_request(&root, request.uri().path()).await;
    if let Some(etag) = etag.as_ref() {
        let matches = request
            .headers()
            .get(IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|values| {
                values
                    .split(',')
                    .any(|value| matches!(value.trim(), "*") || value.trim() == etag)
            });
        if matches {
            let mut response = StatusCode::NOT_MODIFIED.into_response();
            insert_etag(&mut response, etag);
            return response;
        }
    }

    let mut response = next.run(request).await;
    if matches!(
        response.status(),
        StatusCode::OK | StatusCode::PARTIAL_CONTENT
    ) {
        if let Some(etag) = etag.as_ref() {
            insert_etag(&mut response, etag);
        }
    }
    response
}

fn insert_etag(response: &mut Response, etag: &str) {
    if let Ok(value) = HeaderValue::from_str(etag) {
        response.headers_mut().insert(ETAG, value);
    }
}

async fn etag_for_request(root: &Path, request_path: &str) -> Option<String> {
    let path = resolve_request_path(root, request_path).await?;
    let metadata = tokio::fs::metadata(path).await.ok()?;
    if !metadata.is_file() {
        return None;
    }
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();

    Some(format!("W/\"{:x}-{modified:x}\"", metadata.len()))
}

async fn resolve_request_path(root: &Path, request_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode_str(request_path.trim_start_matches('/'))
        .decode_utf8()
        .ok()?;
    if decoded
        .split('/')
        .any(|segment| matches!(segment, ".." | ".") || segment.contains('\\'))
    {
        return None;
    }

    let path = tokio::fs::canonicalize(root.join(decoded.as_ref()))
        .await
        .ok()?;
    if !path.starts_with(root) {
        return None;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, net::SocketAddr};

    use base64::Engine as _;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{checked_upload_size, FileServer, FileServerConfig, MAX_UPLOAD_BYTES};

    struct HttpResponse {
        status: u16,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    async fn request(address: SocketAddr, path: &str, headers: &[(&str, &str)]) -> HttpResponse {
        request_with_body(address, "GET", path, headers, &[]).await
    }

    async fn request_with_body(
        address: SocketAddr,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> HttpResponse {
        let mut stream = tokio::net::TcpStream::connect(address)
            .await
            .expect("test server should accept a connection");
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            request.push_str(name);
            request.push_str(": ");
            request.push_str(value);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        stream
            .write_all(request.as_bytes())
            .await
            .expect("test request should write");
        stream
            .write_all(body)
            .await
            .expect("test request body should write");

        let mut bytes = Vec::new();
        stream
            .read_to_end(&mut bytes)
            .await
            .expect("test response should read");
        let header_end = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("response should contain headers");
        let raw_headers =
            std::str::from_utf8(&bytes[..header_end]).expect("response headers should be UTF-8");
        let mut lines = raw_headers.lines();
        let status = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|status| status.parse().ok())
            .expect("response should contain a numeric status");
        let headers = lines
            .filter_map(|line| line.split_once(": "))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
            .collect();

        HttpResponse {
            status,
            headers,
            body: bytes[header_end + 4..].to_vec(),
        }
    }

    fn multipart_file(boundary: &str, filename: &str, contents: &[u8]) -> Vec<u8> {
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(contents);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        body
    }

    #[tokio::test]
    async fn serves_each_folder_with_mime_etag_and_byte_ranges() {
        let first_root = tempdir().expect("first temporary folder should be created");
        let second_root = tempdir().expect("second temporary folder should be created");
        tokio::fs::write(first_root.path().join("clip.mp4"), b"abcdefghij")
            .await
            .expect("first media file should be written");
        tokio::fs::write(second_root.path().join("clip.mp4"), b"second folder")
            .await
            .expect("second media file should be written");

        let first = FileServer::start(FileServerConfig::new(first_root.path(), "First"))
            .await
            .expect("first file server should start");
        let second = FileServer::start(FileServerConfig::new(second_root.path(), "Second"))
            .await
            .expect("second file server should start");
        assert_ne!(first.address(), second.address());

        let full = request(first.address(), "/clip.mp4", &[]).await;
        assert_eq!(full.status, 200);
        assert_eq!(full.body, b"abcdefghij");
        assert_eq!(
            full.headers.get("content-type").map(String::as_str),
            Some("video/mp4")
        );
        let etag = full
            .headers
            .get("etag")
            .expect("file response should contain an ETag")
            .clone();

        let range = request(first.address(), "/clip.mp4", &[("Range", "bytes=2-5")]).await;
        assert_eq!(range.status, 206);
        assert_eq!(range.body, b"cdef");
        assert_eq!(
            range.headers.get("content-range").map(String::as_str),
            Some("bytes 2-5/10")
        );
        assert_eq!(range.headers.get("etag"), Some(&etag));

        let unchanged = request(
            first.address(),
            "/clip.mp4",
            &[("If-None-Match", etag.as_str())],
        )
        .await;
        assert_eq!(unchanged.status, 304);
        assert!(unchanged.body.is_empty());

        let isolated = request(second.address(), "/clip.mp4", &[]).await;
        assert_eq!(isolated.status, 200);
        assert_eq!(isolated.body, b"second folder");

        first.stop().await.expect("first server should stop");
        second.stop().await.expect("second server should stop");
    }

    #[tokio::test]
    async fn renders_sorted_nested_directory_listings_from_the_template() {
        let root = tempdir().expect("temporary folder should be created");
        tokio::fs::create_dir_all(root.path().join("Alpha/Deep Folder"))
            .await
            .expect("nested directory should be created");
        tokio::fs::create_dir(root.path().join("zeta"))
            .await
            .expect("second directory should be created");
        tokio::fs::write(root.path().join("Zoo.bin"), b"zoo")
            .await
            .expect("second file should be written");
        tokio::fs::write(root.path().join("beta.txt"), vec![b'b'; 1536])
            .await
            .expect("sized file should be written");
        tokio::fs::write(
            root.path().join("Alpha/Deep Folder/doc & notes.txt"),
            b"nested",
        )
        .await
        .expect("nested file should be written");

        let server = FileServer::start(
            FileServerConfig::new(root.path(), "<Team & Files>").allow_uploads(true),
        )
        .await
        .expect("listing server should start");

        let root_listing = request(server.address(), "/", &[]).await;
        assert_eq!(root_listing.status, 200);
        assert_eq!(
            root_listing.headers.get("content-type").map(String::as_str),
            Some("text/html; charset=utf-8")
        );
        let root_html = String::from_utf8(root_listing.body)
            .expect("directory listing should contain UTF-8 HTML");
        assert!(root_html.contains("&lt;Team &amp; Files&gt;"));
        assert!(root_html.contains("display: block"));
        assert!(root_html.contains("1.5 KB"));
        assert!(!root_html.contains("{{"));

        let alpha = root_html.find(">Alpha</span>").expect("Alpha row");
        let zeta = root_html.find(">zeta</span>").expect("zeta row");
        let beta = root_html.find(">beta.txt</span>").expect("beta row");
        let zoo = root_html.find(">Zoo.bin</span>").expect("Zoo row");
        assert!(alpha < zeta && zeta < beta && beta < zoo);

        let nested = request(server.address(), "/Alpha/Deep%20Folder/", &[]).await;
        assert_eq!(nested.status, 200);
        let nested_html = String::from_utf8(nested.body)
            .expect("nested directory listing should contain UTF-8 HTML");
        assert!(nested_html.contains(r#"<a href="/Alpha/">Alpha</a>"#));
        assert!(nested_html.contains(r#"<a href="/Alpha/Deep%20Folder/">Deep Folder</a>"#));
        assert!(nested_html.contains(r#"href="/Alpha/Deep%20Folder/doc%20%26%20notes%2Etxt""#));
        assert!(nested_html.contains(r#"<span class="dt">"#));

        server.stop().await.expect("listing server should stop");
    }

    #[tokio::test]
    async fn disabled_listing_serves_only_a_root_index_or_direct_files() {
        let site_root = tempdir().expect("site folder should be created");
        tokio::fs::write(
            site_root.path().join("index.html"),
            b"<!doctype html><h1>Portfolio</h1>",
        )
        .await
        .expect("site index should be written");
        tokio::fs::write(site_root.path().join("resume.pdf"), b"resume")
            .await
            .expect("direct file should be written");
        tokio::fs::create_dir(site_root.path().join("private"))
            .await
            .expect("nested directory should be created");

        let site = FileServer::start(
            FileServerConfig::new(site_root.path(), "Portfolio").show_listing(false),
        )
        .await
        .expect("site server should start");
        let landing = request(site.address(), "/", &[]).await;
        assert_eq!(landing.status, 200);
        assert_eq!(landing.body, b"<!doctype html><h1>Portfolio</h1>");
        let direct_file = request(site.address(), "/resume.pdf", &[]).await;
        assert_eq!(direct_file.status, 200);
        assert_eq!(direct_file.body, b"resume");
        let nested = request(site.address(), "/private/", &[]).await;
        assert_eq!(nested.status, 403);

        let files_root = tempdir().expect("file folder should be created");
        tokio::fs::write(files_root.path().join("known.txt"), b"known")
            .await
            .expect("known file should be written");
        let files = FileServer::start(
            FileServerConfig::new(files_root.path(), "Files").show_listing(false),
        )
        .await
        .expect("file server should start");
        let forbidden = request(files.address(), "/", &[]).await;
        assert_eq!(forbidden.status, 403);
        assert_eq!(
            forbidden.headers.get("content-type").map(String::as_str),
            Some("text/html; charset=utf-8")
        );
        let forbidden_html =
            String::from_utf8(forbidden.body).expect("403 page should be UTF-8 HTML");
        assert!(forbidden_html.contains("Folder browsing is off"));
        assert!(forbidden_html.contains("Ask the person sharing this folder"));
        assert!(!forbidden_html.contains("known.txt"));

        site.stop().await.expect("site server should stop");
        files.stop().await.expect("file server should stop");
    }

    #[tokio::test]
    async fn blocks_raw_encoded_and_symlink_path_escapes() {
        let root = tempdir().expect("shared folder should be created");
        let outside = tempdir().expect("outside folder should be created");
        tokio::fs::create_dir(root.path().join("a"))
            .await
            .expect("nested folder should be created");
        tokio::fs::write(root.path().join("safe.txt"), b"safe")
            .await
            .expect("safe file should be written");
        let secret = outside.path().join("secret.txt");
        tokio::fs::write(&secret, b"must not escape")
            .await
            .expect("outside file should be written");
        std::os::unix::fs::symlink(&secret, root.path().join("escape.txt"))
            .expect("escape symlink should be created");

        let server = FileServer::start(FileServerConfig::new(root.path(), "Safe files"))
            .await
            .expect("safe server should start");
        let safe = request(server.address(), "/safe.txt", &[]).await;
        assert_eq!(safe.status, 200);
        assert_eq!(safe.body, b"safe");

        let raw = request(server.address(), "/a/../../etc/passwd", &[]).await;
        assert_eq!(raw.status, 404);
        let encoded = request(server.address(), "/a/%2e%2e/%2e%2e/etc/passwd", &[]).await;
        assert_eq!(encoded.status, 404);
        let symlink = request(server.address(), "/escape.txt", &[]).await;
        assert_eq!(symlink.status, 404);
        assert_ne!(symlink.body, b"must not escape");

        let curl_url = format!("http://{}/a/../../etc/passwd", server.address());
        let curl = tokio::task::spawn_blocking(move || {
            std::process::Command::new("curl")
                .args([
                    "--silent",
                    "--output",
                    "/dev/null",
                    "--write-out",
                    "%{http_code}",
                    "--path-as-is",
                    "--max-time",
                    "5",
                    "--noproxy",
                    "*",
                    &curl_url,
                ])
                .output()
        })
        .await
        .expect("curl verification task should finish")
        .expect("curl should be available on macOS");
        assert!(curl.status.success());
        assert_eq!(curl.stdout, b"404");

        server.stop().await.expect("safe server should stop");
    }

    #[tokio::test]
    async fn requires_the_keychain_password_with_porta_basic_auth_realm() {
        let root = tempdir().expect("protected folder should be created");
        tokio::fs::write(root.path().join("private.txt"), b"private")
            .await
            .expect("protected file should be written");
        let server = FileServer::start(
            FileServerConfig::new(root.path(), "Protected")
                .password(Some("correct horse 🔒".to_owned())),
        )
        .await
        .expect("protected server should start");

        let missing = request(server.address(), "/private.txt", &[]).await;
        assert_eq!(missing.status, 401);
        assert_eq!(
            missing.headers.get("www-authenticate").map(String::as_str),
            Some(r#"Basic realm="Porta""#)
        );
        assert_ne!(missing.body, b"private");

        let wrong = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("porta:wrong")
        );
        let rejected = request(
            server.address(),
            "/private.txt",
            &[("Authorization", &wrong)],
        )
        .await;
        assert_eq!(rejected.status, 401);

        let correct = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("any username:correct horse 🔒")
        );
        let allowed = request(
            server.address(),
            "/private.txt",
            &[("Authorization", &correct)],
        )
        .await;
        assert_eq!(allowed.status, 200);
        assert_eq!(allowed.body, b"private");

        server.stop().await.expect("protected server should stop");
    }

    #[tokio::test]
    async fn uploads_to_the_current_folder_without_overwriting() {
        assert_eq!(
            checked_upload_size(MAX_UPLOAD_BYTES - 1, 1),
            Some(MAX_UPLOAD_BYTES)
        );
        assert_eq!(checked_upload_size(MAX_UPLOAD_BYTES, 1), None);

        let root = tempdir().expect("upload folder should be created");
        let nested = root.path().join("nested");
        tokio::fs::create_dir(&nested)
            .await
            .expect("upload target should be created");
        tokio::fs::write(nested.join("report.txt"), b"original")
            .await
            .expect("original file should be written");
        let server =
            FileServer::start(FileServerConfig::new(root.path(), "Uploads").allow_uploads(true))
                .await
                .expect("upload server should start");
        let boundary = "porta-upload-boundary";
        let uploaded_bytes = b"new\0binary\xffcontents";
        let body = multipart_file(boundary, "report.txt", uploaded_bytes);
        let content_type = format!("multipart/form-data; boundary={boundary}");

        let response = request_with_body(
            server.address(),
            "POST",
            "/nested/",
            &[("Content-Type", &content_type)],
            &body,
        )
        .await;
        assert_eq!(response.status, 303);
        assert_eq!(
            response.headers.get("location").map(String::as_str),
            Some("/nested/")
        );
        assert_eq!(
            tokio::fs::read(nested.join("report.txt"))
                .await
                .expect("original file should remain readable"),
            b"original"
        );
        assert_eq!(
            tokio::fs::read(nested.join("report (2).txt"))
                .await
                .expect("collision-safe upload should be readable"),
            uploaded_bytes
        );

        let unsafe_body = multipart_file(boundary, "../../escape.txt", b"escape");
        let unsafe_response = request_with_body(
            server.address(),
            "POST",
            "/nested/",
            &[("Content-Type", &content_type)],
            &unsafe_body,
        )
        .await;
        assert_eq!(unsafe_response.status, 400);
        assert!(!root.path().join("escape.txt").exists());

        let disabled_root = tempdir().expect("disabled upload folder should be created");
        let disabled = FileServer::start(FileServerConfig::new(disabled_root.path(), "Disabled"))
            .await
            .expect("disabled upload server should start");
        let disabled_response = request_with_body(
            disabled.address(),
            "POST",
            "/",
            &[("Content-Type", &content_type)],
            &body,
        )
        .await;
        assert_eq!(disabled_response.status, 403);
        assert!(!disabled_root.path().join("report.txt").exists());

        server.stop().await.expect("upload server should stop");
        disabled
            .stop()
            .await
            .expect("disabled upload server should stop");
    }

    #[tokio::test]
    async fn m2_temp_share_integration_covers_listing_download_traversal_and_auth() {
        let root = tempdir().expect("M2 integration folder should be created");
        let expected: Vec<u8> = (0_u8..=255).cycle().take(4096).collect();
        tokio::fs::write(root.path().join("payload.bin"), &expected)
            .await
            .expect("integration payload should be written");
        let server = FileServer::start(
            FileServerConfig::new(root.path(), "M2 integration")
                .password(Some("integration-secret".to_owned())),
        )
        .await
        .expect("M2 integration server should start");
        let correct = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("porta:integration-secret")
        );
        let wrong = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("porta:not-the-password")
        );

        let listing = request(server.address(), "/", &[("Authorization", &correct)]).await;
        assert_eq!(listing.status, 200);
        assert!(String::from_utf8(listing.body)
            .expect("listing should be UTF-8 HTML")
            .contains("payload.bin"));

        let download = request(
            server.address(),
            "/payload.bin",
            &[("Authorization", &correct)],
        )
        .await;
        assert_eq!(download.status, 200);
        assert_eq!(download.body, expected);

        let traversal = request(
            server.address(),
            "/nested/../../etc/passwd",
            &[("Authorization", &correct)],
        )
        .await;
        assert_eq!(traversal.status, 404);

        let rejected = request(
            server.address(),
            "/payload.bin",
            &[("Authorization", &wrong)],
        )
        .await;
        assert_eq!(rejected.status, 401);

        server
            .stop()
            .await
            .expect("M2 integration server should stop");
    }
}
