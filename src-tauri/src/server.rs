use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use axum::{
    body::Body,
    extract::State,
    http::{
        header::{ETAG, IF_NONE_MATCH},
        HeaderValue, Method, Request, StatusCode,
    },
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    Router,
};
use chrono::{DateTime, Local};
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use tokio::{sync::oneshot, task::JoinHandle};
use tower_http::services::ServeDir;

const INVALID_FOLDER: &str = "This folder was moved or deleted. Pick it again to reshare.";
const START_ERROR: &str =
    "Porta couldn't start a local server. Quit and reopen Porta, then try again.";
const STOP_ERROR: &str =
    "Porta couldn't stop the local server cleanly. Quit Porta before sharing it again.";
const LISTING_TEMPLATE: &str = include_str!("../../server-templates/listing.html");
const FOLDER_SVG: &str = r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M3 19V6a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>"#;
const FILE_SVG: &str = r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M6 2h8l4 4v16H6V2Z" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/><path d="M14 2v5h4" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>"#;

/// Settings needed to start one folder server.
pub struct FileServerConfig {
    root: PathBuf,
    share_name: String,
    allow_uploads: bool,
}

impl FileServerConfig {
    /// Creates a listing-enabled configuration with uploads hidden.
    pub fn new(root: impl Into<PathBuf>, share_name: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            share_name: share_name.into(),
            allow_uploads: false,
        }
    }

    /// Controls whether the listing shows its upload widget.
    pub fn allow_uploads(mut self, allow_uploads: bool) -> Self {
        self.allow_uploads = allow_uploads;
        self
    }
}

#[derive(Clone)]
struct ListingState {
    root: PathBuf,
    share_name: String,
    allow_uploads: bool,
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
            allow_uploads: config.allow_uploads,
        };
        let app = Router::new()
            .fallback_service(ServeDir::new(&root))
            .layer(middleware::from_fn_with_state(
                listing_state,
                serve_directory_listing,
            ))
            .layer(middleware::from_fn_with_state(root, add_etag));
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

async fn serve_directory_listing(
    State(state): State<ListingState>,
    request: Request<Body>,
    next: Next,
) -> Response {
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

    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{FileServer, FileServerConfig};

    struct HttpResponse {
        status: u16,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    async fn request(address: SocketAddr, path: &str, headers: &[(&str, &str)]) -> HttpResponse {
        let mut stream = tokio::net::TcpStream::connect(address)
            .await
            .expect("test server should accept a connection");
        let mut request =
            format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
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
}
