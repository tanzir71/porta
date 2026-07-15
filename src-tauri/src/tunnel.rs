use std::{
    collections::{HashMap, HashSet},
    io,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        LazyLock,
    },
    time::Duration,
};

use regex::Regex;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_shell::{
    process::{CommandChild, CommandEvent},
    ShellExt,
};
use tokio::sync::Mutex;

use crate::{
    server::{FileServer, FileServerConfig},
    shares::{AppEvent, Share, ShareStatus},
    store::Store,
};

const URL_TIMEOUT: Duration = Duration::from_secs(30);
const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(3);
const ALREADY_RUNNING: &str = "This share is already starting. Wait a moment, then try again.";
const SIDECAR_MISSING: &str =
    "Porta couldn't find its tunnel helper. Reinstall Porta, then try again.";
const SIDECAR_START_ERROR: &str =
    "Porta couldn't start its tunnel helper. Quit and reopen Porta, then try again.";
const CLOUDFLARE_ERROR: &str =
    "Couldn't reach Cloudflare — check your internet connection and try again.";
const MISSING_SHARE: &str = "That share no longer exists. Return to the main window and try again.";
const STOP_ERROR: &str =
    "Porta couldn't stop this share cleanly. Quit Porta, then check Activity Monitor for cloudflared.";
const WINDOW_REFRESH_ERROR: &str = "Porta saved the change but couldn't refresh its windows. Close and reopen the window to see it.";

static TUNNEL_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://[a-z0-9-]+\.trycloudflare\.com")
        .expect("the Cloudflare tunnel URL regex should be valid")
});

#[derive(Default)]
struct TunnelState {
    pending: HashSet<String>,
    sessions: HashMap<String, TunnelSession>,
}

struct TunnelSession {
    generation: u64,
    server: FileServer,
    child: CommandChild,
}

pub struct TunnelManager {
    state: Mutex<TunnelState>,
    next_generation: AtomicU64,
}

impl Default for TunnelManager {
    fn default() -> Self {
        Self {
            state: Mutex::new(TunnelState::default()),
            next_generation: AtomicU64::new(1),
        }
    }
}

impl TunnelManager {
    pub async fn start(&self, app: &AppHandle, share: Share) -> Result<(), String> {
        {
            let mut state = self.state.lock().await;
            if state.pending.contains(&share.id) || state.sessions.contains_key(&share.id) {
                return Err(ALREADY_RUNNING.to_owned());
            }
            state.pending.insert(share.id.clone());
        }

        let config = match FileServerConfig::for_share(&share) {
            Ok(config) => config,
            Err(error) => {
                self.release_pending(&share.id).await;
                return Err(error);
            }
        };
        let server = match FileServer::start(config).await {
            Ok(server) => server,
            Err(error) => {
                self.release_pending(&share.id).await;
                return Err(error);
            }
        };
        let origin = server.address();
        let command = match app.shell().sidecar("cloudflared") {
            Ok(command) => command.args(cloudflared_args(origin)),
            Err(_) => {
                self.release_pending(&share.id).await;
                let _ = server.stop().await;
                return Err(SIDECAR_MISSING.to_owned());
            }
        };
        let (receiver, child) = match command.spawn() {
            Ok(process) => process,
            Err(_) => {
                self.release_pending(&share.id).await;
                let _ = server.stop().await;
                return Err(SIDECAR_START_ERROR.to_owned());
            }
        };

        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        {
            let mut state = self.state.lock().await;
            state.pending.remove(&share.id);
            state.sessions.insert(
                share.id.clone(),
                TunnelSession {
                    generation,
                    server,
                    child,
                },
            );
        }

        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            monitor_startup(app, share.id, generation, receiver).await;
        });
        Ok(())
    }

    async fn release_pending(&self, id: &str) {
        self.state.lock().await.pending.remove(id);
    }

    pub async fn stop(&self, id: &str) -> Result<(), String> {
        let session = {
            let mut state = self.state.lock().await;
            state.pending.remove(id);
            state.sessions.remove(id)
        };

        if let Some(session) = session {
            shutdown_session(session).await?;
        }
        Ok(())
    }

    async fn take_session(&self, id: &str, generation: u64) -> Option<TunnelSession> {
        let mut state = self.state.lock().await;
        let is_current = state
            .sessions
            .get(id)
            .is_some_and(|session| session.generation == generation);
        is_current.then(|| state.sessions.remove(id)).flatten()
    }
}

impl Drop for TunnelManager {
    fn drop(&mut self) {
        let sessions = self
            .state
            .get_mut()
            .sessions
            .drain()
            .map(|(_, session)| session)
            .collect();
        terminate_sessions_blocking(sessions);
    }
}

fn cloudflared_args(origin: SocketAddr) -> Vec<String> {
    vec![
        "tunnel".to_owned(),
        "--url".to_owned(),
        format!("http://{origin}"),
        "--no-autoupdate".to_owned(),
    ]
}

async fn monitor_startup(
    app: AppHandle,
    share_id: String,
    generation: u64,
    receiver: tauri::async_runtime::Receiver<CommandEvent>,
) {
    match wait_for_tunnel_url(receiver, URL_TIMEOUT).await {
        Ok(url) => {
            if transition_share(&app, &share_id, ShareStatus::Live, Some(url), None).is_err() {
                let _ = cleanup_session(&app, &share_id, generation).await;
            }
        }
        Err(error) => {
            if cleanup_session(&app, &share_id, generation).await {
                let _ = transition_share(&app, &share_id, ShareStatus::Error, None, Some(error));
            }
        }
    }
}

async fn cleanup_session(app: &AppHandle, share_id: &str, generation: u64) -> bool {
    let manager = app.state::<TunnelManager>();
    if let Some(session) = manager.take_session(share_id, generation).await {
        let _ = shutdown_session(session).await;
        true
    } else {
        false
    }
}

async fn shutdown_session(session: TunnelSession) -> Result<(), String> {
    let TunnelSession { server, child, .. } = session;
    let (process_stopped, server_stopped) = tokio::join!(
        terminate_child(child),
        tokio::time::timeout(GRACEFUL_STOP_TIMEOUT, server.stop())
    );

    if process_stopped && matches!(server_stopped, Ok(Ok(()))) {
        Ok(())
    } else {
        Err(STOP_ERROR.to_owned())
    }
}

async fn terminate_child(child: CommandChild) -> bool {
    if send_signal(child.pid(), libc::SIGTERM).is_err() {
        return child.kill().is_ok();
    }

    if wait_for_process_exit(child.pid(), GRACEFUL_STOP_TIMEOUT).await {
        true
    } else {
        child.kill().is_ok()
    }
}

async fn wait_for_process_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while process_is_running(pid) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    !process_is_running(pid)
}

fn terminate_sessions_blocking(sessions: Vec<TunnelSession>) {
    for session in &sessions {
        let _ = send_signal(session.child.pid(), libc::SIGTERM);
    }

    let deadline = std::time::Instant::now() + GRACEFUL_STOP_TIMEOUT;
    while sessions
        .iter()
        .any(|session| process_is_running(session.child.pid()))
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(25));
    }

    for session in sessions {
        if process_is_running(session.child.pid()) {
            let _ = session.child.kill();
        }
    }
}

fn send_signal(pid: u32, signal: libc::c_int) -> io::Result<()> {
    // SAFETY: kill only receives a process identifier and a valid POSIX signal number.
    if unsafe { libc::kill(pid as libc::pid_t, signal) } == 0 {
        return Ok(());
    }

    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

fn process_is_running(pid: u32) -> bool {
    // SAFETY: signal 0 does not alter the process; it only checks whether the PID exists.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }

    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

async fn wait_for_tunnel_url(
    mut receiver: tauri::async_runtime::Receiver<CommandEvent>,
    timeout: Duration,
) -> Result<String, String> {
    tokio::time::timeout(timeout, async move {
        let mut stderr = String::new();
        while let Some(event) = receiver.recv().await {
            match event {
                CommandEvent::Stderr(bytes) => {
                    stderr.push_str(&String::from_utf8_lossy(&bytes));
                    if let Some(found) = TUNNEL_URL.find(&stderr) {
                        return Ok(found.as_str().to_owned());
                    }
                    if stderr.len() > 64 * 1024 {
                        stderr.clear();
                    }
                }
                CommandEvent::Error(_) | CommandEvent::Terminated(_) => {
                    return Err(CLOUDFLARE_ERROR.to_owned());
                }
                _ => {}
            }
        }
        Err(CLOUDFLARE_ERROR.to_owned())
    })
    .await
    .unwrap_or_else(|_| Err(CLOUDFLARE_ERROR.to_owned()))
}

pub(crate) fn transition_share<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    status: ShareStatus,
    url: Option<String>,
    error: Option<String>,
) -> Result<Share, String> {
    let store = app.state::<Store>();
    let share = store.update(|shares, _| {
        let share = shares
            .iter_mut()
            .find(|share| share.id == id)
            .ok_or_else(|| MISSING_SHARE.to_owned())?;
        share.status = status;
        share.url = url;
        share.error = error;
        Ok(share.clone())
    })?;
    app.emit(
        "app_event",
        AppEvent::ShareChanged {
            share: share.clone(),
        },
    )
    .map_err(|_| WINDOW_REFRESH_ERROR.to_owned())?;
    Ok(share)
}

#[cfg(test)]
mod tests {
    use std::{process::Command, sync::mpsc, time::Duration};

    use tauri::{Listener, Manager};
    use tauri_plugin_shell::{process::CommandEvent, ShellExt};
    use tempfile::tempdir;

    use super::{
        cloudflared_args, send_signal, transition_share, wait_for_process_exit,
        wait_for_tunnel_url, URL_TIMEOUT,
    };
    use crate::{
        server::{FileServer, FileServerConfig},
        shares::ShareStatus,
        store::Store,
    };

    #[tokio::test]
    async fn uses_exact_args_and_discovers_a_split_stderr_url() {
        assert_eq!(URL_TIMEOUT, Duration::from_secs(30));
        assert_eq!(
            cloudflared_args("127.0.0.1:43123".parse().expect("address should parse")),
            [
                "tunnel",
                "--url",
                "http://127.0.0.1:43123",
                "--no-autoupdate"
            ]
        );

        let (sender, receiver) = tauri::async_runtime::channel(4);
        sender
            .send(CommandEvent::Stderr(
                b"INF requesting https://quiet-".to_vec(),
            ))
            .await
            .expect("first stderr event should send");
        sender
            .send(CommandEvent::Stderr(
                b"harbor.trycloudflare.com from edge".to_vec(),
            ))
            .await
            .expect("second stderr event should send");
        drop(sender);

        assert_eq!(
            wait_for_tunnel_url(receiver, Duration::from_millis(100))
                .await
                .expect("URL should be discovered"),
            "https://quiet-harbor.trycloudflare.com"
        );
    }

    #[tokio::test]
    async fn sigterm_stops_a_child_within_the_grace_period() {
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep process should start");
        let pid = child.id();
        let waiter = std::thread::spawn(move || child.wait());

        send_signal(pid, libc::SIGTERM).expect("SIGTERM should be delivered");
        let exited = wait_for_process_exit(pid, Duration::from_secs(1)).await;
        if !exited {
            let _ = send_signal(pid, libc::SIGKILL);
        }
        let status = waiter
            .join()
            .expect("wait thread should finish")
            .expect("child should be reaped");

        assert!(exited, "SIGTERM should stop the child before force-kill");
        assert!(
            !status.success(),
            "signal termination should not be success"
        );
    }

    #[tokio::test]
    async fn manager_stop_terminates_its_tauri_child_and_local_server() {
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_shell::init())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app should build");
        let (_receiver, child) = app
            .shell()
            .command("sleep")
            .arg("30")
            .spawn()
            .expect("managed child should start");
        let pid = child.pid();
        let root = tempdir().expect("temporary share should be created");
        let server = FileServer::start(FileServerConfig::new(root.path(), "Teardown test"))
            .await
            .expect("local server should start");
        let manager = super::TunnelManager::default();
        manager.state.lock().await.sessions.insert(
            "teardown".to_owned(),
            super::TunnelSession {
                generation: 1,
                server,
                child,
            },
        );

        let started = tokio::time::Instant::now();
        manager
            .stop("teardown")
            .await
            .expect("manager should stop the session cleanly");

        assert!(started.elapsed() < super::GRACEFUL_STOP_TIMEOUT);
        assert!(!super::process_is_running(pid));
        assert!(manager.state.lock().await.sessions.is_empty());

        let (_quit_receiver, quit_child) = app
            .shell()
            .command("sleep")
            .arg("30")
            .spawn()
            .expect("quit child should start");
        let quit_pid = quit_child.pid();
        let quit_root = tempdir().expect("temporary quit share should be created");
        let quit_server = FileServer::start(FileServerConfig::new(quit_root.path(), "Quit test"))
            .await
            .expect("quit server should start");
        manager.state.lock().await.sessions.insert(
            "quit".to_owned(),
            super::TunnelSession {
                generation: 2,
                server: quit_server,
                child: quit_child,
            },
        );

        let quit_started = tokio::time::Instant::now();
        drop(manager);

        assert!(quit_started.elapsed() < super::GRACEFUL_STOP_TIMEOUT);
        assert!(!super::process_is_running(quit_pid));
    }

    #[test]
    fn persists_starting_and_live_transitions_and_emits_them() {
        let data_dir = tempdir().expect("temporary store should be created");
        let store = Store::load_from_dir(data_dir.path()).expect("test store should load");
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_contract.json"))
                .expect("fixture should contain JSON");
        let share: crate::shares::Share = serde_json::from_value(fixture["share"].clone())
            .expect("fixture share should deserialize");
        let id = share.id.clone();
        store
            .update(|shares, _| {
                shares.push(share);
                Ok(())
            })
            .expect("fixture share should persist");
        let app = tauri::test::mock_builder()
            .manage(store)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app should build");
        let (sender, receiver) = mpsc::channel();
        app.listen("app_event", move |event| {
            sender
                .send(event.payload().to_owned())
                .expect("event should be captured");
        });

        transition_share(app.handle(), &id, ShareStatus::Starting, None, None)
            .expect("starting transition should persist");
        transition_share(
            app.handle(),
            &id,
            ShareStatus::Live,
            Some("https://quiet-harbor.trycloudflare.com".to_owned()),
            None,
        )
        .expect("live transition should persist");

        let starting: crate::shares::AppEvent = serde_json::from_str(
            &receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("starting event should arrive"),
        )
        .expect("starting event should deserialize");
        let live: crate::shares::AppEvent = serde_json::from_str(
            &receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("live event should arrive"),
        )
        .expect("live event should deserialize");
        assert!(matches!(
            starting,
            crate::shares::AppEvent::ShareChanged { share }
                if share.status == ShareStatus::Starting && share.url.is_none()
        ));
        assert!(matches!(
            live,
            crate::shares::AppEvent::ShareChanged { share }
                if share.status == ShareStatus::Live
                    && share.url.as_deref()
                        == Some("https://quiet-harbor.trycloudflare.com")
        ));

        let stored = app
            .state::<Store>()
            .read(|shares, _| shares[0].clone())
            .expect("live share should remain readable");
        assert_eq!(stored.status, ShareStatus::Live);
        assert_eq!(
            stored.url.as_deref(),
            Some("https://quiet-harbor.trycloudflare.com")
        );
    }
}
