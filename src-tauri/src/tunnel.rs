use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

#[cfg(unix)]
use std::io;

use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_notification::{NotificationExt, PermissionState};
use tauri_plugin_shell::{
    process::{CommandChild, CommandEvent},
    ShellExt,
};
use tokio::sync::Mutex;

use crate::{
    credentials,
    provider::{
        profile_by_id, ProviderLaunch, ProviderProgram, ProviderTestResult, ResolvedProvider,
        UrlDiscovery,
    },
    server::{FileServer, FileServerConfig},
    shares::{AppEvent, Share, ShareKind, ShareStatus},
    stats::{ShareStats, StatsReporter},
    store::Store,
};

const URL_TIMEOUT: Duration = Duration::from_secs(30);
const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_BACKOFF_SECONDS: u64 = 60;
const MAX_CONSECUTIVE_FAILURES: u8 = 3;
const ALREADY_RUNNING: &str = "This share is already starting. Wait a moment, then try again.";
const MISSING_SHARE: &str = "That share no longer exists. Return to the main window and try again.";
const MISSING_PORT: &str =
    "This port share has no port. Edit the share and enter one from 1 to 65535.";

#[cfg(target_os = "windows")]
const STOP_ERROR: &str =
    "Porta couldn't stop this share cleanly. Quit Porta, then check Task Manager for its tunnel helper.";
#[cfg(target_os = "macos")]
const STOP_ERROR: &str =
    "Porta couldn't stop this share cleanly. Quit Porta, then check Activity Monitor for its tunnel helper.";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const STOP_ERROR: &str =
    "Porta couldn't stop this share cleanly. Quit Porta, then check your system's process manager for its tunnel helper.";
const UNSTABLE_TUNNEL_ERROR: &str =
    "The tunnel keeps dropping. Porta will retry when you toggle it back on.";
const FOLDER_MISSING_ERROR: &str = "This folder was moved or deleted. Pick it again to reshare.";
const FIRST_VISITOR_BODY: &str = "Someone just opened your link.";
const WINDOW_REFRESH_ERROR: &str = "Porta saved the change but couldn't refresh its windows. Close and reopen the window to see it.";

#[derive(Default)]
struct TunnelState {
    pending: HashSet<String>,
    sessions: HashMap<String, TunnelSession>,
}

struct TunnelSession {
    generation: u64,
    server: Option<FileServer>,
    child: Option<CommandChild>,
}

struct SpawnedProvider {
    receiver: tauri::async_runtime::Receiver<CommandEvent>,
    child: CommandChild,
    discovery: UrlDiscovery,
    connection_error: String,
}

struct SupervisionContext {
    share_id: String,
    generation: u64,
    origin: SocketAddr,
    provider: ResolvedProvider,
    discovery: UrlDiscovery,
    connection_error: String,
}

#[derive(Default)]
struct RetryState {
    consecutive_failures: u8,
}

#[derive(Debug, PartialEq, Eq)]
enum SessionEnd {
    ProcessExited,
    FolderMissing,
}

impl RetryState {
    fn fail(&mut self) -> Option<Duration> {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        (self.consecutive_failures < MAX_CONSECUTIVE_FAILURES)
            .then(|| retry_delay(self.consecutive_failures))
    }

    fn succeed(&mut self) {
        self.consecutive_failures = 0;
    }
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
    pub async fn start<R: Runtime>(&self, app: &AppHandle<R>, share: Share) -> Result<(), String> {
        {
            let mut state = self.state.lock().await;
            if state.pending.contains(&share.id) || state.sessions.contains_key(&share.id) {
                return Err(ALREADY_RUNNING.to_owned());
            }
            state.pending.insert(share.id.clone());
        }

        let provider = match resolve_provider(app, &share) {
            Ok(provider) => provider,
            Err(error) => {
                self.release_pending(&share.id).await;
                return Err(error);
            }
        };
        let (origin, server) = match start_local_origin(app, &share, &provider).await {
            Ok(started) => started,
            Err(error) => {
                self.release_pending(&share.id).await;
                return Err(error);
            }
        };
        let spawned = match spawn_provider(app, &provider, origin) {
            Ok(process) => process,
            Err(error) => {
                self.release_pending(&share.id).await;
                if let Some(server) = server {
                    let _ = server.stop().await;
                }
                return Err(error);
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
                    child: Some(spawned.child),
                },
            );
        }

        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            supervise_tunnel(
                app,
                SupervisionContext {
                    share_id: share.id,
                    generation,
                    origin,
                    provider,
                    discovery: spawned.discovery,
                    connection_error: spawned.connection_error,
                },
                spawned.receiver,
            )
            .await;
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

    async fn take_child(&self, id: &str, generation: u64) -> Option<CommandChild> {
        let mut state = self.state.lock().await;
        state
            .sessions
            .get_mut(id)
            .filter(|session| session.generation == generation)
            .and_then(|session| session.child.take())
    }

    async fn install_child(
        &self,
        id: &str,
        generation: u64,
        child: CommandChild,
    ) -> Result<(), CommandChild> {
        let mut state = self.state.lock().await;
        let Some(session) = state
            .sessions
            .get_mut(id)
            .filter(|session| session.generation == generation && session.child.is_none())
        else {
            return Err(child);
        };
        session.child = Some(child);
        Ok(())
    }

    async fn is_current(&self, id: &str, generation: u64) -> bool {
        self.state
            .lock()
            .await
            .sessions
            .get(id)
            .is_some_and(|session| session.generation == generation)
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

async fn start_local_origin<R: Runtime>(
    app: &AppHandle<R>,
    share: &Share,
    provider: &ResolvedProvider,
) -> Result<(SocketAddr, Option<FileServer>), String> {
    match share.kind {
        ShareKind::Folder => {
            let reporter = stats_reporter(app, share.id.clone());
            let server = FileServer::start(
                FileServerConfig::for_share(share)?
                    .stats(reporter)
                    .bind_port(provider.preferred_local_port())
                    .visitor_headers(provider.visitor_headers()),
            )
            .await?;
            Ok((server.address(), Some(server)))
        }
        ShareKind::Port => {
            let port = share.port.ok_or_else(|| MISSING_PORT.to_owned())?;
            if let Some(expected) = provider.preferred_local_port() {
                if expected != port {
                    return Err(format!(
                        "This provider profile routes to local port {expected}, but this share uses port {port}. Edit the profile or share so the ports match."
                    ));
                }
            }
            Ok((SocketAddr::from((Ipv4Addr::LOCALHOST, port)), None))
        }
    }
}

fn stats_reporter<R: Runtime>(app: &AppHandle<R>, share_id: String) -> Arc<StatsReporter> {
    let stats_app = app.clone();
    let stats_share_id = share_id.clone();
    let notification_app = app.clone();
    StatsReporter::with_first_visitor(
        move |stats| {
            let store = stats_app.state::<Store>();
            let saved = store.update(|shares, _| {
                let share = shares
                    .iter_mut()
                    .find(|share| share.id == stats_share_id)
                    .ok_or_else(|| MISSING_SHARE.to_owned())?;
                share.stats = stats;
                Ok(())
            });
            if saved.is_ok() {
                let _ = stats_app.emit(
                    "app_event",
                    AppEvent::StatsUpdated {
                        id: stats_share_id.clone(),
                        stats,
                    },
                );
            }
        },
        move || {
            show_first_visitor_notification(&notification_app, &share_id);
        },
    )
}

fn show_first_visitor_notification<R: Runtime>(app: &AppHandle<R>, share_id: &str) {
    let store = app.state::<Store>();
    let title = store
        .read(|shares, settings| {
            first_visitor_notification_title(shares, settings.notify_on_first_visitor, share_id)
        })
        .ok()
        .flatten();
    let Some(title) = title else {
        return;
    };

    let notification = app.notification();
    let granted = matches!(
        notification.permission_state(),
        Ok(PermissionState::Granted)
    ) || matches!(
        notification.request_permission(),
        Ok(PermissionState::Granted)
    );
    if granted {
        let _ = notification
            .builder()
            .title(title)
            .body(FIRST_VISITOR_BODY)
            .show();
    }
}

fn first_visitor_notification_title(
    shares: &[Share],
    enabled: bool,
    share_id: &str,
) -> Option<String> {
    enabled
        .then(|| {
            shares
                .iter()
                .find(|share| share.id == share_id)
                .map(|share| share.name.clone())
        })
        .flatten()
}

fn resolve_provider<R: Runtime>(
    app: &AppHandle<R>,
    share: &Share,
) -> Result<ResolvedProvider, String> {
    let store = app.state::<Store>();
    let profile = store.read_with_providers(|_, settings, profiles| {
        let id = share
            .provider_id
            .as_deref()
            .unwrap_or(&settings.default_provider_id);
        profile_by_id(profiles, id)
    })?;
    let profile = profile.ok_or_else(|| {
        "This share's tunnel provider no longer exists. Edit the share and choose another provider."
            .to_owned()
    })?;
    let credential = if profile
        .kind
        .requires_credential(profile.credential_env.as_deref())
    {
        credentials::get_provider_secret(&profile.id)?
    } else {
        None
    };
    ResolvedProvider::new(profile, credential)
}

fn spawn_provider<R: Runtime>(
    app: &AppHandle<R>,
    provider: &ResolvedProvider,
    origin: SocketAddr,
) -> Result<SpawnedProvider, String> {
    let ProviderLaunch {
        program,
        arguments,
        environment,
        discovery,
        start_error,
        connection_error,
    } = provider.launch(origin)?;
    let command = match program {
        ProviderProgram::BundledCloudflared => {
            app.shell().sidecar("cloudflared").map_err(|_| {
                "Porta couldn't find Cloudflare's tunnel helper. Reinstall Porta, then try again."
                    .to_owned()
            })?
        }
        ProviderProgram::External(executable) => app.shell().command(executable),
    }
    .args(arguments)
    .envs(environment);
    let (receiver, child) = command.spawn().map_err(|_| start_error)?;
    Ok(SpawnedProvider {
        receiver,
        child,
        discovery,
        connection_error,
    })
}

#[cfg(test)]
const CLOUDFLARE_ERROR: &str =
    "Couldn't reach Cloudflare — check your internet connection and try again.";

#[cfg(test)]
fn quick_provider() -> ResolvedProvider {
    ResolvedProvider::new(crate::provider::cloudflare_quick_profile(), None)
        .expect("built-in provider should resolve")
}

#[cfg(test)]
fn cloudflared_args(origin: SocketAddr) -> Vec<String> {
    quick_provider()
        .launch(origin)
        .expect("built-in provider launch should build")
        .arguments
}

#[cfg(test)]
async fn wait_for_tunnel_url(
    receiver: &mut tauri::async_runtime::Receiver<CommandEvent>,
    timeout: Duration,
) -> Result<String, String> {
    let launch = quick_provider()
        .launch("127.0.0.1:43123".parse().expect("origin should parse"))
        .expect("built-in provider launch should build");
    wait_for_provider_url(
        receiver,
        &launch.discovery,
        timeout,
        &launch.connection_error,
    )
    .await
}

fn retry_delay(consecutive_failures: u8) -> Duration {
    let exponent = consecutive_failures.saturating_sub(1).min(6);
    Duration::from_secs((1_u64 << exponent).min(MAX_BACKOFF_SECONDS))
}

async fn supervise_tunnel<R: Runtime>(
    app: AppHandle<R>,
    context: SupervisionContext,
    mut receiver: tauri::async_runtime::Receiver<CommandEvent>,
) {
    let SupervisionContext {
        share_id,
        generation,
        origin,
        provider,
        discovery,
        connection_error,
    } = context;
    let manager = app.state::<TunnelManager>();
    match wait_for_provider_url(&mut receiver, &discovery, URL_TIMEOUT, &connection_error).await {
        Ok(url) => {
            if !manager.is_current(&share_id, generation).await {
                return;
            }
            if transition_share(&app, &share_id, ShareStatus::Live, Some(url), None).is_err() {
                let _ = cleanup_session(&app, &share_id, generation).await;
                return;
            }
        }
        Err(error) => {
            if cleanup_session(&app, &share_id, generation).await {
                let _ = transition_share(&app, &share_id, ShareStatus::Error, None, Some(error));
            }
            return;
        }
    }

    let mut retries = RetryState::default();
    loop {
        if wait_for_session_end(&app, &share_id, &mut receiver).await == SessionEnd::FolderMissing {
            if cleanup_session(&app, &share_id, generation).await {
                let _ = transition_share(
                    &app,
                    &share_id,
                    ShareStatus::Error,
                    None,
                    Some(FOLDER_MISSING_ERROR.to_owned()),
                );
            }
            return;
        }
        if !stop_managed_child(&manager, &share_id, generation).await {
            return;
        }

        let Some(mut delay) = retries.fail() else {
            mark_unstable(&app, &share_id, generation).await;
            return;
        };

        loop {
            tokio::time::sleep(delay).await;
            if !manager.is_current(&share_id, generation).await {
                return;
            }

            let spawned = match spawn_provider(&app, &provider, origin) {
                Ok(process) => process,
                Err(_) => {
                    let Some(next_delay) = retries.fail() else {
                        mark_unstable(&app, &share_id, generation).await;
                        return;
                    };
                    delay = next_delay;
                    continue;
                }
            };
            if let Err(child) = manager
                .install_child(&share_id, generation, spawned.child)
                .await
            {
                let _ = terminate_child(child).await;
                return;
            }

            let mut next_receiver = spawned.receiver;
            match wait_for_provider_url(
                &mut next_receiver,
                &spawned.discovery,
                URL_TIMEOUT,
                &spawned.connection_error,
            )
            .await
            {
                Ok(url) => {
                    if !manager.is_current(&share_id, generation).await {
                        return;
                    }
                    retries.succeed();
                    if transition_share(&app, &share_id, ShareStatus::Live, Some(url), None)
                        .is_err()
                    {
                        let _ = cleanup_session(&app, &share_id, generation).await;
                        return;
                    }
                    receiver = next_receiver;
                    break;
                }
                Err(_) => {
                    if !stop_managed_child(&manager, &share_id, generation).await {
                        return;
                    }
                    let Some(next_delay) = retries.fail() else {
                        mark_unstable(&app, &share_id, generation).await;
                        return;
                    };
                    delay = next_delay;
                }
            }
        }
    }
}

async fn stop_managed_child(manager: &TunnelManager, share_id: &str, generation: u64) -> bool {
    let Some(child) = manager.take_child(share_id, generation).await else {
        return false;
    };
    let _ = terminate_child(child).await;
    true
}

async fn mark_unstable<R: Runtime>(app: &AppHandle<R>, share_id: &str, generation: u64) {
    if cleanup_session(app, share_id, generation).await {
        let _ = transition_share(
            app,
            share_id,
            ShareStatus::Error,
            None,
            Some(UNSTABLE_TUNNEL_ERROR.to_owned()),
        );
    }
}

async fn cleanup_session<R: Runtime>(app: &AppHandle<R>, share_id: &str, generation: u64) -> bool {
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
        async move {
            match child {
                Some(child) => terminate_child(child).await,
                None => true,
            }
        },
        async move {
            match server {
                Some(server) => {
                    match tokio::time::timeout(GRACEFUL_STOP_TIMEOUT, server.stop()).await {
                        Ok(result) => result.is_ok(),
                        // Cancelling FileServer::stop drops the server and aborts its task,
                        // which intentionally disconnects a visitor who is still downloading.
                        Err(_) => true,
                    }
                }
                None => true,
            }
        }
    );

    if process_stopped && server_stopped {
        Ok(())
    } else {
        Err(STOP_ERROR.to_owned())
    }
}

async fn terminate_child(child: CommandChild) -> bool {
    #[cfg(unix)]
    {
        if send_signal(child.pid(), libc::SIGTERM).is_err() {
            return child.kill().is_ok();
        }

        if wait_for_process_exit(child.pid(), GRACEFUL_STOP_TIMEOUT).await {
            true
        } else {
            child.kill().is_ok()
        }
    }

    #[cfg(windows)]
    {
        let pid = child.pid();
        if child.kill().is_err() {
            return false;
        }
        wait_for_process_exit(pid, GRACEFUL_STOP_TIMEOUT).await
    }

    #[cfg(not(any(unix, windows)))]
    {
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

#[cfg(unix)]
fn terminate_sessions_blocking(sessions: Vec<TunnelSession>) {
    for session in &sessions {
        if let Some(child) = &session.child {
            let _ = send_signal(child.pid(), libc::SIGTERM);
        }
    }

    let deadline = std::time::Instant::now() + GRACEFUL_STOP_TIMEOUT;
    while sessions
        .iter()
        .filter_map(|session| session.child.as_ref())
        .any(|child| process_is_running(child.pid()))
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(25));
    }

    for session in sessions {
        if let Some(child) = session.child {
            if process_is_running(child.pid()) {
                let _ = child.kill();
            }
        }
    }
}

#[cfg(windows)]
fn terminate_sessions_blocking(sessions: Vec<TunnelSession>) {
    let mut pids = Vec::new();
    for session in sessions {
        if let Some(child) = session.child {
            pids.push(child.pid());
            let _ = child.kill();
        }
    }

    let deadline = std::time::Instant::now() + GRACEFUL_STOP_TIMEOUT;
    while pids.iter().any(|pid| process_is_running(*pid)) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(not(any(unix, windows)))]
fn terminate_sessions_blocking(sessions: Vec<TunnelSession>) {
    for session in sessions {
        if let Some(child) = session.child {
            let _ = child.kill();
        }
    }
}

#[cfg(unix)]
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

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    // SAFETY: signal 0 does not alter the process; it only checks whether the PID exists.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }

    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, STILL_ACTIVE},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return false;
    }

    let mut exit_code = 0;
    let queried = unsafe { GetExitCodeProcess(process, &mut exit_code) } != 0;
    unsafe {
        CloseHandle(process);
    }
    queried && exit_code == STILL_ACTIVE as u32
}

#[cfg(not(any(unix, windows)))]
fn process_is_running(_pid: u32) -> bool {
    false
}

async fn wait_for_provider_url(
    receiver: &mut tauri::async_runtime::Receiver<CommandEvent>,
    discovery: &UrlDiscovery,
    timeout: Duration,
    connection_error: &str,
) -> Result<String, String> {
    let started = tokio::time::Instant::now();
    let deadline = started + timeout;
    let fixed_deadline = discovery.fixed_delay().map(|delay| started + delay);
    let mut output = String::new();

    loop {
        let wake_at = fixed_deadline.map_or(deadline, |fixed| fixed.min(deadline));
        match tokio::time::timeout_at(wake_at, receiver.recv()).await {
            Ok(Some(CommandEvent::Stdout(bytes) | CommandEvent::Stderr(bytes))) => {
                output.push_str(&String::from_utf8_lossy(&bytes));
                if let Some(url) = discovery.inspect(&output) {
                    return Ok(url);
                }
                if output.len() > 64 * 1024 {
                    let mut keep_from = output.len().saturating_sub(32 * 1024);
                    while keep_from < output.len() && !output.is_char_boundary(keep_from) {
                        keep_from += 1;
                    }
                    output.drain(..keep_from);
                }
            }
            Ok(Some(CommandEvent::Error(_) | CommandEvent::Terminated(_))) | Ok(None) => {
                return Err(connection_error.to_owned());
            }
            Ok(Some(_)) => {}
            Err(_) => {
                if fixed_deadline.is_some_and(|fixed| fixed <= tokio::time::Instant::now()) {
                    return discovery
                        .fixed_url()
                        .ok_or_else(|| connection_error.to_owned());
                }
                return Err(connection_error.to_owned());
            }
        }
    }
}

pub(crate) async fn test_provider_connection<R: Runtime>(
    app: &AppHandle<R>,
    provider: ResolvedProvider,
) -> Result<ProviderTestResult, String> {
    use tokio::io::AsyncWriteExt;

    let port = provider.preferred_local_port().unwrap_or(0);
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|_| match provider.preferred_local_port() {
            Some(port) => format!(
                "Porta couldn't use local port {port} for the provider test. Stop the app using it, then try again."
            ),
            None => "Porta couldn't start the provider test server. Quit and reopen Porta, then try again."
                .to_owned(),
        })?;
    let origin = listener.local_addr().map_err(|_| {
        "Porta couldn't read the provider test address. Quit and reopen Porta, then try again."
            .to_owned()
    })?;
    let origin_task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 19\r\nConnection: close\r\n\r\nPorta provider test",
                )
                .await;
        }
    });

    let mut spawned = match spawn_provider(app, &provider, origin) {
        Ok(spawned) => spawned,
        Err(error) => {
            origin_task.abort();
            return Err(error);
        }
    };
    let result = wait_for_provider_url(
        &mut spawned.receiver,
        &spawned.discovery,
        URL_TIMEOUT,
        &spawned.connection_error,
    )
    .await;
    let stopped = terminate_child(spawned.child).await;
    origin_task.abort();
    if !stopped {
        return Err(STOP_ERROR.to_owned());
    }
    let url = result?;
    Ok(ProviderTestResult {
        message: format!("{} connected successfully.", provider.profile.name),
        url,
    })
}

#[cfg(test)]
async fn wait_for_process_end(receiver: &mut tauri::async_runtime::Receiver<CommandEvent>) {
    while let Some(event) = receiver.recv().await {
        if matches!(event, CommandEvent::Error(_) | CommandEvent::Terminated(_)) {
            return;
        }
    }
}

async fn wait_for_session_end<R: Runtime>(
    app: &AppHandle<R>,
    share_id: &str,
    receiver: &mut tauri::async_runtime::Receiver<CommandEvent>,
) -> SessionEnd {
    let mut folder_check = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            event = receiver.recv() => {
                if event.is_none_or(|event| {
                    matches!(event, CommandEvent::Error(_) | CommandEvent::Terminated(_))
                }) {
                    return SessionEnd::ProcessExited;
                }
            }
            _ = folder_check.tick() => {
                if !share_source_available(app, share_id).await {
                    return SessionEnd::FolderMissing;
                }
            }
        }
    }
}

async fn share_source_available<R: Runtime>(app: &AppHandle<R>, share_id: &str) -> bool {
    let source = app
        .state::<Store>()
        .read(|shares, _| {
            shares
                .iter()
                .find(|share| share.id == share_id)
                .map(|share| (share.kind, share.path.clone()))
        })
        .ok()
        .flatten();
    match source {
        Some((ShareKind::Port, _)) => true,
        Some((ShareKind::Folder, Some(path))) => tokio::fs::metadata(path)
            .await
            .is_ok_and(|metadata| metadata.is_dir()),
        Some((ShareKind::Folder, None)) | None => false,
    }
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
        if status == ShareStatus::Starting {
            share.stats = ShareStats::default();
        }
        Ok(share.clone())
    })?;
    let clipboard_url = store
        .read(|_, settings| url_to_copy(&share, settings.copy_url_on_start).map(str::to_owned))?;
    app.emit(
        "app_event",
        AppEvent::ShareChanged {
            share: share.clone(),
        },
    )
    .map_err(|_| WINDOW_REFRESH_ERROR.to_owned())?;
    let _ = crate::tray::refresh(app);
    if let Some(url) = clipboard_url {
        let _ = app.clipboard().write_text(url);
    }
    Ok(share)
}

fn url_to_copy(share: &Share, copy_url_on_start: bool) -> Option<&str> {
    (copy_url_on_start && share.status == ShareStatus::Live)
        .then_some(share.url.as_deref())
        .flatten()
}

#[cfg(test)]
mod tests {
    use std::{fs, process::Command as StdCommand, sync::mpsc, time::Duration};

    #[cfg(unix)]
    use std::process::Command;

    use regex::Regex;
    use tauri::{Listener, Manager};
    use tauri_plugin_shell::{process::CommandEvent, ShellExt};
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{
        cloudflared_args, first_visitor_notification_title, quick_provider, retry_delay,
        start_local_origin, stats_reporter, supervise_tunnel, transition_share, url_to_copy,
        wait_for_process_end, wait_for_provider_url, wait_for_tunnel_url, RetryState, UrlDiscovery,
        CLOUDFLARE_ERROR, FIRST_VISITOR_BODY, FOLDER_MISSING_ERROR, UNSTABLE_TUNNEL_ERROR,
        URL_TIMEOUT,
    };
    #[cfg(unix)]
    use super::{send_signal, wait_for_process_exit};
    use crate::{
        provider::{build_profile, ProviderKind, ResolvedProvider, SaveProviderProfileInput},
        server::{FileServer, FileServerConfig},
        shares::ShareStatus,
        store::Store,
    };

    macro_rules! long_running_command {
        ($app:expr) => {{
            #[cfg(unix)]
            {
                $app.shell().command("sleep").arg("30")
            }
            #[cfg(windows)]
            {
                $app.shell().command("powershell.exe").args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Start-Sleep -Seconds 30",
                ])
            }
            #[cfg(not(any(unix, windows)))]
            {
                $app.shell().command("sleep").arg("30")
            }
        }};
    }

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

        let (sender, mut receiver) = tauri::async_runtime::channel(4);
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
        sender
            .send(CommandEvent::Error("simulated crash".to_owned()))
            .await
            .expect("termination event should send");
        drop(sender);

        assert_eq!(
            wait_for_tunnel_url(&mut receiver, Duration::from_millis(100))
                .await
                .expect("URL should be discovered"),
            "https://quiet-harbor.trycloudflare.com"
        );
        tokio::time::timeout(
            Duration::from_millis(100),
            wait_for_process_end(&mut receiver),
        )
        .await
        .expect("supervisor should continue through the later crash event");
    }

    #[tokio::test]
    async fn provider_output_trimming_preserves_unicode_boundaries() {
        let (sender, mut receiver) = tauri::async_runtime::channel(2);
        sender
            .send(CommandEvent::Stdout("€".repeat(22_000).into_bytes()))
            .await
            .expect("large Unicode output should send");
        sender
            .send(CommandEvent::Stdout(
                b"https://unicode-safe.example.com".to_vec(),
            ))
            .await
            .expect("public URL should send");
        drop(sender);
        let discovery = UrlDiscovery::Output {
            pattern: Regex::new(r"(?P<url>https://[A-Za-z0-9.-]+\.example\.com)")
                .expect("test pattern should compile"),
            fixed_url: None,
        };

        assert_eq!(
            wait_for_provider_url(
                &mut receiver,
                &discovery,
                Duration::from_millis(100),
                "provider failed",
            )
            .await
            .expect("URL after large Unicode output should be discovered"),
            "https://unicode-safe.example.com"
        );
    }

    #[tokio::test]
    async fn custom_provider_test_launches_directly_and_leaves_no_process() {
        let directory = tempdir().expect("temporary provider directory should be created");
        let source = directory.path().join("fake_provider.rs");
        let executable = directory.path().join(if cfg!(windows) {
            "fake-provider.exe"
        } else {
            "fake-provider"
        });
        let pid_file = directory.path().join("provider.pid");
        fs::write(
            &source,
            r#"use std::{env, fs, process, thread, time::Duration};
fn main() {
    let pid_file = env::args().nth(1).expect("pid file argument");
    fs::write(pid_file, process::id().to_string()).expect("write pid");
    println!("https://porta-provider-test.example.com");
    thread::sleep(Duration::from_secs(60));
}
"#,
        )
        .expect("fake provider source should be written");
        let compiled = StdCommand::new("rustc")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .status()
            .expect("rustc should compile the fake provider");
        assert!(compiled.success(), "fake provider should compile");

        let profile = build_profile(
            SaveProviderProfileInput {
                id: None,
                name: "Lifecycle test provider".to_owned(),
                kind: ProviderKind::Custom,
                executable: Some(executable.to_string_lossy().into_owned()),
                arguments: vec![pid_file.to_string_lossy().into_owned()],
                public_url: None,
                url_pattern: Some(r"(?P<url>https://[A-Za-z0-9.-]+\.example\.com)".to_owned()),
                credential_env: None,
                forwarded_ip_header: None,
                local_port: None,
                credential: None,
                clear_credential: false,
            },
            None,
        )
        .expect("fake provider profile should validate");
        let provider =
            ResolvedProvider::new(profile, None).expect("credential-free provider should resolve");
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_shell::init())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app should build");

        let result = super::test_provider_connection(app.handle(), provider)
            .await
            .expect("fake provider test should connect");
        assert_eq!(result.url, "https://porta-provider-test.example.com");
        let pid: u32 = fs::read_to_string(&pid_file)
            .expect("fake provider should record its pid")
            .parse()
            .expect("fake provider pid should be numeric");
        assert!(
            !super::process_is_running(pid),
            "provider test must stop its child process"
        );
    }

    #[tokio::test]
    async fn port_shares_use_the_users_loopback_port_without_an_axum_server() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app should build");
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_contract.json"))
                .expect("fixture should contain JSON");
        let mut share: crate::shares::Share = serde_json::from_value(fixture["share"].clone())
            .expect("fixture share should deserialize");
        share.kind = crate::shares::ShareKind::Port;
        share.path = None;
        share.port = Some(4173);

        let provider = quick_provider();
        let (origin, server) = start_local_origin(app.handle(), &share, &provider)
            .await
            .expect("port origin should be created");

        assert_eq!(origin, "127.0.0.1:4173".parse().expect("address is valid"));
        assert!(server.is_none());
        assert_eq!(
            cloudflared_args(origin),
            [
                "tunnel",
                "--url",
                "http://127.0.0.1:4173",
                "--no-autoupdate"
            ]
        );

        share.port = None;
        assert_eq!(
            start_local_origin(app.handle(), &share, &provider)
                .await
                .err()
                .as_deref(),
            Some("This port share has no port. Edit the share and enter one from 1 to 65535.")
        );
    }

    #[test]
    fn retry_policy_backs_off_and_stops_after_three_consecutive_failures() {
        let delays: Vec<_> = (1..=8).map(retry_delay).collect();
        assert_eq!(
            delays,
            [
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(32),
                Duration::from_secs(60),
                Duration::from_secs(60),
            ]
        );

        let mut retries = RetryState::default();
        assert_eq!(retries.fail(), Some(Duration::from_secs(1)));
        assert_eq!(retries.fail(), Some(Duration::from_secs(2)));
        assert_eq!(retries.fail(), None);
        retries.succeed();
        assert_eq!(retries.fail(), Some(Duration::from_secs(1)));
        assert_eq!(
            UNSTABLE_TUNNEL_ERROR,
            "The tunnel keeps dropping. Porta will retry when you toggle it back on."
        );
    }

    #[test]
    fn copy_url_setting_selects_only_live_public_urls() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_contract.json"))
                .expect("fixture should contain JSON");
        let mut share: crate::shares::Share = serde_json::from_value(fixture["share"].clone())
            .expect("fixture share should deserialize");
        share.status = ShareStatus::Live;
        share.url = Some("https://quiet-harbor.trycloudflare.com".to_owned());

        assert_eq!(
            url_to_copy(&share, true),
            Some("https://quiet-harbor.trycloudflare.com")
        );
        assert_eq!(url_to_copy(&share, false), None);

        share.status = ShareStatus::Starting;
        assert_eq!(url_to_copy(&share, true), None);
        share.status = ShareStatus::Live;
        share.url = None;
        assert_eq!(url_to_copy(&share, true), None);
    }

    #[test]
    fn first_visitor_notification_uses_the_share_name_and_exact_body() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_contract.json"))
                .expect("fixture should contain JSON");
        let share: crate::shares::Share = serde_json::from_value(fixture["share"].clone())
            .expect("fixture share should deserialize");

        assert_eq!(
            first_visitor_notification_title(std::slice::from_ref(&share), true, &share.id),
            Some(share.name.clone())
        );
        assert_eq!(
            first_visitor_notification_title(std::slice::from_ref(&share), false, &share.id),
            None
        );
        assert_eq!(
            first_visitor_notification_title(std::slice::from_ref(&share), true, "missing"),
            None
        );
        assert_eq!(FIRST_VISITOR_BODY, "Someone just opened your link.");
    }

    #[tokio::test]
    async fn coalesces_stats_ticks_and_persists_the_latest_snapshot() {
        let data_dir = tempdir().expect("temporary store should be created");
        let store = Store::load_from_dir(data_dir.path()).expect("test store should load");
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_contract.json"))
                .expect("fixture should contain JSON");
        let share: crate::shares::Share = serde_json::from_value(fixture["share"].clone())
            .expect("fixture share should deserialize");
        let id = share.id.clone();
        store
            .update(|shares, settings| {
                shares.push(share);
                settings.notify_on_first_visitor = false;
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
        let reporter = stats_reporter(app.handle(), id.clone());

        reporter.record_request(Some("203.0.113.1"));
        reporter.record_request(Some("203.0.113.1"));
        reporter.record_request(Some("2001:db8::2"));
        reporter.record_bytes(4096);
        reporter.record_bytes(512);
        tokio::time::sleep(Duration::from_millis(1100)).await;

        let event: crate::shares::AppEvent = serde_json::from_str(
            &receiver
                .try_recv()
                .expect("one coalesced stats event should arrive"),
        )
        .expect("stats event should deserialize");
        assert!(matches!(
            event,
            crate::shares::AppEvent::StatsUpdated { id: event_id, stats }
                if event_id == id
                    && stats.visitors == 2
                    && stats.requests == 3
                    && stats.bytes_served == 4608
        ));
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        let stored = app
            .state::<Store>()
            .read(|shares, _| shares[0].stats)
            .expect("stats should remain readable");
        assert_eq!(stored.visitors, 2);
        assert_eq!(stored.requests, 3);
        assert_eq!(stored.bytes_served, 4608);
        reporter.deactivate();
    }

    #[tokio::test]
    async fn unreachable_tunnel_persists_the_exact_actionable_error_and_cleans_up() {
        let (idle_sender, mut idle_receiver) = tauri::async_runtime::channel(1);
        assert_eq!(
            wait_for_tunnel_url(&mut idle_receiver, Duration::from_millis(10)).await,
            Err(
                "Couldn't reach Cloudflare — check your internet connection and try again."
                    .to_owned()
            )
        );
        drop(idle_sender);
        assert_eq!(
            CLOUDFLARE_ERROR,
            "Couldn't reach Cloudflare — check your internet connection and try again."
        );

        let data_dir = tempdir().expect("temporary store should be created");
        let root = tempdir().expect("temporary share should be created");
        let store = Store::load_from_dir(data_dir.path()).expect("test store should load");
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_contract.json"))
                .expect("fixture should contain JSON");
        let mut share: crate::shares::Share = serde_json::from_value(fixture["share"].clone())
            .expect("fixture share should deserialize");
        share.path = Some(root.path().to_string_lossy().into_owned());
        share.status = ShareStatus::Starting;
        share.url = None;
        share.error = None;
        share.password_protected = false;
        let share_id = share.id.clone();
        store
            .update(|shares, settings| {
                shares.push(share);
                settings.copy_url_on_start = false;
                Ok(())
            })
            .expect("starting share should persist");

        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_shell::init())
            .manage(store)
            .manage(super::TunnelManager::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app should build");
        let (_child_receiver, child) = long_running_command!(app)
            .spawn()
            .expect("managed child should start");
        let pid = child.pid();
        let server = FileServer::start(FileServerConfig::new(root.path(), "Offline test"))
            .await
            .expect("local server should start");
        let origin = server.address();
        app.state::<super::TunnelManager>()
            .state
            .lock()
            .await
            .sessions
            .insert(
                share_id.clone(),
                super::TunnelSession {
                    generation: 1,
                    server: Some(server),
                    child: Some(child),
                },
            );
        let (event_sender, event_receiver) = tauri::async_runtime::channel(1);
        event_sender
            .send(CommandEvent::Error("network unavailable".to_owned()))
            .await
            .expect("failure event should send");
        drop(event_sender);

        let provider = quick_provider();
        let launch = provider
            .launch(origin)
            .expect("built-in provider launch should build");
        supervise_tunnel(
            app.handle().clone(),
            super::SupervisionContext {
                share_id: share_id.clone(),
                generation: 1,
                origin,
                provider,
                discovery: launch.discovery,
                connection_error: launch.connection_error,
            },
            event_receiver,
        )
        .await;

        let stored = app
            .state::<Store>()
            .read(|shares, _| shares[0].clone())
            .expect("failed share should remain readable");
        assert_eq!(stored.status, ShareStatus::Error);
        assert_eq!(stored.url, None);
        assert_eq!(stored.error.as_deref(), Some(CLOUDFLARE_ERROR));
        assert!(!super::process_is_running(pid));
        assert!(app
            .state::<super::TunnelManager>()
            .state
            .lock()
            .await
            .sessions
            .is_empty());
    }

    #[tokio::test]
    async fn deleting_a_live_shared_folder_marks_it_error_and_stops_its_session() {
        let data_dir = tempdir().expect("temporary store should be created");
        let root = tempdir().expect("temporary share should be created");
        let root_path = root.path().to_path_buf();
        let store = Store::load_from_dir(data_dir.path()).expect("test store should load");
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_contract.json"))
                .expect("fixture should contain JSON");
        let mut share: crate::shares::Share = serde_json::from_value(fixture["share"].clone())
            .expect("fixture share should deserialize");
        share.path = Some(root_path.to_string_lossy().into_owned());
        share.status = ShareStatus::Starting;
        share.url = None;
        share.error = None;
        share.password_protected = false;
        let share_id = share.id.clone();
        store
            .update(|shares, settings| {
                shares.push(share);
                settings.copy_url_on_start = false;
                Ok(())
            })
            .expect("starting share should persist");

        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_shell::init())
            .manage(store)
            .manage(super::TunnelManager::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app should build");
        let (_child_receiver, child) = long_running_command!(app)
            .spawn()
            .expect("managed child should start");
        let pid = child.pid();
        let server = FileServer::start(FileServerConfig::new(&root_path, "Missing folder test"))
            .await
            .expect("local server should start");
        let origin = server.address();
        app.state::<super::TunnelManager>()
            .state
            .lock()
            .await
            .sessions
            .insert(
                share_id.clone(),
                super::TunnelSession {
                    generation: 1,
                    server: Some(server),
                    child: Some(child),
                },
            );
        let (event_sender, event_receiver) = tauri::async_runtime::channel(2);
        event_sender
            .send(CommandEvent::Stderr(
                b"https://missing-folder.trycloudflare.com".to_vec(),
            ))
            .await
            .expect("live URL event should send");
        root.close().expect("shared folder should be removed");

        let provider = quick_provider();
        let launch = provider
            .launch(origin)
            .expect("built-in provider launch should build");
        tokio::time::timeout(
            Duration::from_secs(2),
            supervise_tunnel(
                app.handle().clone(),
                super::SupervisionContext {
                    share_id: share_id.clone(),
                    generation: 1,
                    origin,
                    provider,
                    discovery: launch.discovery,
                    connection_error: launch.connection_error,
                },
                event_receiver,
            ),
        )
        .await
        .expect("missing-folder watcher should react promptly");
        drop(event_sender);

        let stored = app
            .state::<Store>()
            .read(|shares, _| shares[0].clone())
            .expect("failed share should remain readable");
        assert_eq!(stored.status, ShareStatus::Error);
        assert_eq!(stored.url, None);
        assert_eq!(stored.error.as_deref(), Some(FOLDER_MISSING_ERROR));
        assert!(!super::process_is_running(pid));
        assert!(app
            .state::<super::TunnelManager>()
            .state
            .lock()
            .await
            .sessions
            .is_empty());
    }

    #[cfg(unix)]
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
        let (_receiver, child) = long_running_command!(app)
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
                server: Some(server),
                child: Some(child),
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

        let (_quit_receiver, quit_child) = long_running_command!(app)
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
                server: Some(quit_server),
                child: Some(quit_child),
            },
        );

        let quit_started = tokio::time::Instant::now();
        drop(manager);

        assert!(quit_started.elapsed() < super::GRACEFUL_STOP_TIMEOUT);
        assert!(!super::process_is_running(quit_pid));
    }

    #[tokio::test]
    async fn stopping_during_a_download_drops_the_connection_without_an_orphan() {
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_shell::init())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app should build");
        let (_receiver, child) = long_running_command!(app)
            .spawn()
            .expect("managed child should start");
        let pid = child.pid();
        let root = tempdir().expect("temporary share should be created");
        let payload = std::fs::File::create(root.path().join("large.bin"))
            .expect("large sparse download should be created");
        payload
            .set_len(64 * 1024 * 1024)
            .expect("large sparse download should be sized");
        drop(payload);
        let server = FileServer::start(FileServerConfig::new(root.path(), "Download stop test"))
            .await
            .expect("local server should start");
        let address = server.address();
        let manager = super::TunnelManager::default();
        manager.state.lock().await.sessions.insert(
            "active-download".to_owned(),
            super::TunnelSession {
                generation: 1,
                server: Some(server),
                child: Some(child),
            },
        );

        let mut visitor = tokio::net::TcpStream::connect(address)
            .await
            .expect("visitor should connect");
        visitor
            .write_all(
                format!("GET /large.bin HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .expect("visitor request should write");
        tokio::time::sleep(Duration::from_millis(100)).await;

        let started = tokio::time::Instant::now();
        manager
            .stop("active-download")
            .await
            .expect("forced download stop should still be clean");
        assert!(started.elapsed() <= super::GRACEFUL_STOP_TIMEOUT + Duration::from_millis(500));

        let mut remaining = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), visitor.read_to_end(&mut remaining))
            .await
            .expect("visitor connection should close after the stop")
            .ok();
        assert!(!super::process_is_running(pid));
        assert!(manager.state.lock().await.sessions.is_empty());
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
    }

    #[tokio::test]
    #[ignore = "spawns the bundled cloudflared sidecar for release QA"]
    async fn bundled_cloudflared_has_zero_orphans_after_ten_start_stop_cycles() {
        let data_dir = tempdir().expect("temporary store should be created");
        let root = tempdir().expect("temporary share should be created");
        let store = Store::load_from_dir(data_dir.path()).expect("test store should load");
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_contract.json"))
                .expect("fixture should contain JSON");
        let mut share: crate::shares::Share = serde_json::from_value(fixture["share"].clone())
            .expect("fixture share should deserialize");
        share.id = "ten-cycle-release-qa".to_owned();
        share.path = Some(root.path().to_string_lossy().into_owned());
        share.status = ShareStatus::Stopped;
        share.url = None;
        share.error = None;
        share.password_protected = false;
        store
            .update(|shares, settings| {
                shares.push(share.clone());
                settings.copy_url_on_start = false;
                Ok(())
            })
            .expect("QA share should persist");
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_shell::init())
            .manage(store)
            .manage(super::TunnelManager::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app should build");

        for cycle in 1..=10 {
            let manager = app.state::<super::TunnelManager>();
            manager
                .start(app.handle(), share.clone())
                .await
                .unwrap_or_else(|error| panic!("cycle {cycle} should start: {error}"));
            let pid = manager
                .state
                .lock()
                .await
                .sessions
                .get(&share.id)
                .and_then(|session| session.child.as_ref())
                .map(tauri_plugin_shell::process::CommandChild::pid)
                .expect("started cycle should own a cloudflared child");
            assert!(super::process_is_running(pid));

            manager
                .stop(&share.id)
                .await
                .unwrap_or_else(|error| panic!("cycle {cycle} should stop: {error}"));
            assert!(
                !super::process_is_running(pid),
                "cycle {cycle} left cloudflared PID {pid} running"
            );
            assert!(manager.state.lock().await.sessions.is_empty());
            println!("release QA cycle {cycle}/10 stopped PID {pid}");
        }
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
            .update(|shares, settings| {
                shares.push(share);
                settings.copy_url_on_start = false;
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
                if share.status == ShareStatus::Starting
                    && share.url.is_none()
                    && share.stats == crate::stats::ShareStats::default()
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
        assert_eq!(stored.stats, crate::stats::ShareStats::default());
        assert_eq!(
            stored.url.as_deref(),
            Some("https://quiet-harbor.trycloudflare.com")
        );
    }
}
