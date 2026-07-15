use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use porta_lib::server::{FileServer, FileServerConfig};
use regex::Regex;
use tempfile::tempdir;

const FIXTURE_NAME: &str = "porta-live-smoke.txt";
const FIXTURE_BYTES: &[u8] = b"Porta live tunnel smoke test\n";

struct CloudflaredGuard {
    child: Child,
}

impl CloudflaredGuard {
    fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for CloudflaredGuard {
    fn drop(&mut self) {
        // SAFETY: the guard owns this child PID and SIGTERM is a valid POSIX signal.
        let _ = unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn bundled_cloudflared() -> PathBuf {
    let binary = if cfg!(target_arch = "aarch64") {
        "cloudflared-aarch64-apple-darwin"
    } else {
        "cloudflared-x86_64-apple-darwin"
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(binary)
}

/// Requires working internet access and intentionally reaches Cloudflare's Quick Tunnel service.
#[tokio::test]
#[ignore = "requires internet access and a bundled cloudflared binary"]
async fn real_quick_tunnel_downloads_a_porta_served_file() {
    let root = tempdir().expect("temporary share should be created");
    std::fs::write(root.path().join(FIXTURE_NAME), FIXTURE_BYTES)
        .expect("smoke fixture should be written");
    let server = FileServer::start(FileServerConfig::new(root.path(), "Live smoke test"))
        .await
        .expect("Porta file server should start");

    let mut child = Command::new(bundled_cloudflared())
        .args([
            "tunnel",
            "--url",
            &format!("http://{}", server.address()),
            "--no-autoupdate",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bundled cloudflared should start");
    let stderr = child
        .stderr
        .take()
        .expect("cloudflared stderr should exist");
    let guard = CloudflaredGuard { child };
    let pid = guard.pid();
    let (line_sender, line_receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = line_sender.send(line);
        }
    });

    let url_pattern = Regex::new(r"https://[a-z0-9-]+\.trycloudflare\.com")
        .expect("tunnel URL regex should compile");
    let deadline = Instant::now() + Duration::from_secs(30);
    let public_url = loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("cloudflared should print a URL within 30 seconds");
        let line = line_receiver
            .recv_timeout(remaining)
            .expect("cloudflared should keep running until it prints a URL");
        if let Some(found) = url_pattern.find(&line) {
            break found.as_str().to_owned();
        }
    };
    let download_url = format!("{public_url}/{FIXTURE_NAME}");
    println!("PORTA_SMOKE_URL={download_url}");

    // The edge URL is printed before its public DNS record has necessarily propagated.
    let download_deadline = Instant::now() + Duration::from_secs(90);
    let downloaded = loop {
        let output = Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--max-time",
                "5",
                &download_url,
            ])
            .output()
            .expect("curl should run");
        if output.status.success() {
            break output.stdout;
        }
        assert!(
            Instant::now() < download_deadline,
            "public tunnel did not serve the fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        thread::sleep(Duration::from_millis(500));
    };
    assert_eq!(downloaded, FIXTURE_BYTES);

    if let Ok(seconds) = std::env::var("PORTA_SMOKE_HOLD_SECONDS") {
        thread::sleep(Duration::from_secs(
            seconds.parse().expect("hold duration should be seconds"),
        ));
    }

    server.stop().await.expect("Porta server should stop");
    drop(guard);
    // SAFETY: signal 0 only checks whether the just-reaped PID still exists.
    assert_ne!(unsafe { libc::kill(pid as libc::pid_t, 0) }, 0);
}
