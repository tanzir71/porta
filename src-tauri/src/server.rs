use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use axum::{
    body::Body,
    http::{
        header::{ETAG, IF_NONE_MATCH},
        HeaderValue, Method, Request, StatusCode,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};
use percent_encoding::percent_decode_str;
use tokio::{sync::oneshot, task::JoinHandle};
use tower_http::services::ServeDir;

const INVALID_FOLDER: &str = "This folder was moved or deleted. Pick it again to reshare.";
const START_ERROR: &str =
    "Porta couldn't start a local server. Quit and reopen Porta, then try again.";
const STOP_ERROR: &str =
    "Porta couldn't stop the local server cleanly. Quit Porta before sharing it again.";

/// One loopback HTTP server for one shared folder.
pub struct FileServer {
    address: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl FileServer {
    /// Starts serving `root` from an OS-assigned loopback port.
    pub async fn start(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = tokio::fs::canonicalize(root.as_ref())
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
        let app = Router::new()
            .fallback_service(ServeDir::new(&root))
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

async fn add_etag(
    axum::extract::State(root): axum::extract::State<PathBuf>,
    request: Request<Body>,
    next: Next,
) -> Response {
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

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, net::SocketAddr};

    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::FileServer;

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

        let first = FileServer::start(first_root.path())
            .await
            .expect("first file server should start");
        let second = FileServer::start(second_root.path())
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
}
