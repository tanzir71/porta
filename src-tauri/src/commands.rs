use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use url::Url;
use uuid::Uuid;

use crate::{
    credentials,
    settings::{Settings, SettingsPatch},
    shares::{AppEvent, CreateShareInput, Share, ShareKind, ShareStatus, UpdateShareInput},
    stats::ShareStats,
    store::Store,
    tunnel::{self, TunnelManager},
};

const MISSING_SHARE: &str = "That share no longer exists. Return to the main window and try again.";
const FOLDER_MISSING: &str = "This folder was moved or deleted. Pick it again to reshare.";

#[tauri::command]
pub(crate) fn list_shares(store: State<'_, Store>) -> Result<Vec<Share>, String> {
    store.read(|shares, _| shares.to_vec())
}

#[tauri::command]
pub(crate) async fn create_share(
    app: AppHandle,
    store: State<'_, Store>,
    tunnels: State<'_, TunnelManager>,
    input: CreateShareInput,
) -> Result<Share, String> {
    let start_now = input.start_now.unwrap_or(true);
    let saved = create_share_in(&app, &store, input)?;
    if start_now {
        let _ = start_share_in(&app, &tunnels, &saved.id).await;
        return store.read(|shares, _| {
            shares
                .iter()
                .find(|share| share.id == saved.id)
                .cloned()
                .unwrap_or(saved)
        });
    }
    Ok(saved)
}

fn create_share_in<R: Runtime>(
    app: &AppHandle<R>,
    store: &Store,
    input: CreateShareInput,
) -> Result<Share, String> {
    let share = build_share(&input)?;
    let password = input.password.as_deref();

    if let Some(password) = password {
        credentials::replace_password(&share.id, Some(password))?;
    }

    let save_result = store.update(|shares, _| {
        shares.insert(0, share.clone());
        Ok(share.clone())
    });

    if save_result.is_err() && password.is_some() {
        let _ = credentials::replace_password(&share.id, None);
    }

    let saved = save_result?;
    emit_app_event(
        app,
        AppEvent::ShareChanged {
            share: saved.clone(),
        },
    )?;
    Ok(saved)
}

#[tauri::command]
pub(crate) async fn start_share(
    app: AppHandle,
    tunnels: State<'_, TunnelManager>,
    id: String,
) -> Result<(), String> {
    start_share_in(&app, &tunnels, &id).await
}

#[tauri::command]
pub(crate) async fn stop_share(
    app: AppHandle,
    tunnels: State<'_, TunnelManager>,
    id: String,
) -> Result<(), String> {
    stop_share_in(&app, &tunnels, &id).await
}

pub(crate) async fn stop_share_in<R: Runtime>(
    app: &AppHandle<R>,
    tunnels: &TunnelManager,
    id: &str,
) -> Result<(), String> {
    let store = app.state::<Store>();
    store.read(|shares, _| {
        shares
            .iter()
            .any(|share| share.id == id)
            .then_some(())
            .ok_or_else(|| MISSING_SHARE.to_owned())
    })??;

    tunnels.stop(id).await?;
    tunnel::transition_share(app, id, ShareStatus::Stopped, None, None)?;
    Ok(())
}

pub(crate) async fn start_share_in<R: Runtime>(
    app: &AppHandle<R>,
    tunnels: &TunnelManager,
    id: &str,
) -> Result<(), String> {
    let store = app.state::<Store>();
    let current = store.read(|shares, _| {
        shares
            .iter()
            .find(|share| share.id == id)
            .cloned()
            .ok_or_else(|| MISSING_SHARE.to_owned())
    })??;
    if matches!(current.status, ShareStatus::Starting | ShareStatus::Live) {
        return Ok(());
    }

    let starting = tunnel::transition_share(app, id, ShareStatus::Starting, None, None)?;
    if let Err(error) = tunnels.start(app, starting).await {
        tunnel::transition_share(app, id, ShareStatus::Error, None, Some(error.clone()))?;
        return Err(error);
    }
    Ok(())
}

pub(crate) fn prepare_auto_start_shares(app: &AppHandle) -> Result<Vec<String>, String> {
    prepare_auto_start_share_ids(&app.state::<Store>())
}

fn prepare_auto_start_share_ids(store: &Store) -> Result<Vec<String>, String> {
    store.update(|shares, settings| {
        for share in shares.iter_mut() {
            if matches!(share.status, ShareStatus::Starting | ShareStatus::Live) {
                share.status = ShareStatus::Stopped;
                share.url = None;
                share.error = None;
            }
        }

        Ok(if settings.auto_start_shares {
            shares
                .iter()
                .filter(|share| share.auto_start)
                .map(|share| share.id.clone())
                .collect()
        } else {
            Vec::new()
        })
    })
}

pub(crate) fn spawn_auto_start_shares(app: &AppHandle, share_ids: Vec<String>) {
    if share_ids.is_empty() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        for share_id in share_ids {
            let tunnels = app.state::<TunnelManager>();
            let _ = start_share_in(&app, &tunnels, &share_id).await;
        }
    });
}

#[tauri::command]
pub(crate) async fn update_share(
    app: AppHandle,
    store: State<'_, Store>,
    tunnels: State<'_, TunnelManager>,
    id: String,
    patch: UpdateShareInput,
) -> Result<Share, String> {
    let restart_runtime = store.read(|shares, _| {
        shares
            .iter()
            .find(|share| share.id == id)
            .map(|share| patch_requires_runtime_restart(share, &patch))
            .ok_or_else(|| MISSING_SHARE.to_owned())
    })??;
    let password_change = patch.password.clone();
    let previous_password = if password_change.is_some() {
        credentials::get_password(&id)?
    } else {
        None
    };

    if let Some(password) = password_change.as_ref() {
        validate_password(password.as_deref())?;
        credentials::replace_password(&id, password.as_deref())?;
    }

    let save_result = store.update(|shares, _| {
        let share = shares
            .iter_mut()
            .find(|share| share.id == id)
            .ok_or_else(|| MISSING_SHARE.to_owned())?;
        apply_share_patch(share, &patch)?;
        Ok(share.clone())
    });

    if save_result.is_err() && password_change.is_some() {
        let _ = credentials::replace_password(&id, previous_password.as_deref());
    }

    let saved = save_result?;
    emit_app_event(
        &app,
        AppEvent::ShareChanged {
            share: saved.clone(),
        },
    )?;
    if !restart_runtime {
        return Ok(saved);
    }

    stop_share_in(&app, &tunnels, &id).await?;
    start_share_in(&app, &tunnels, &id).await?;
    store.read(|shares, _| {
        shares
            .iter()
            .find(|share| share.id == id)
            .cloned()
            .unwrap_or(saved)
    })
}

#[tauri::command]
pub(crate) async fn delete_share(
    app: AppHandle,
    store: State<'_, Store>,
    tunnels: State<'_, TunnelManager>,
    id: String,
) -> Result<(), String> {
    stop_share_in(&app, &tunnels, &id).await?;
    delete_share_in(&app, &store, id)
}

fn delete_share_in<R: Runtime>(
    app: &AppHandle<R>,
    store: &Store,
    id: String,
) -> Result<(), String> {
    let password_protected = store.read(|shares, _| {
        shares
            .iter()
            .find(|share| share.id == id)
            .map(|share| share.password_protected)
            .ok_or_else(|| MISSING_SHARE.to_owned())
    })??;
    let previous_password = if password_protected {
        let password = credentials::get_password(&id)?;
        credentials::replace_password(&id, None)?;
        password
    } else {
        None
    };

    let delete_result = store.update(|shares, _| {
        let index = shares
            .iter()
            .position(|share| share.id == id)
            .ok_or_else(|| MISSING_SHARE.to_owned())?;
        shares.remove(index);
        Ok(())
    });

    if delete_result.is_err() && password_protected {
        let _ = credentials::replace_password(&id, previous_password.as_deref());
    }

    delete_result?;
    emit_app_event(app, AppEvent::ShareRemoved { id })
}

#[tauri::command]
pub(crate) async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    pick_folder_in(&app)
}

pub(crate) fn pick_folder_in<R: Runtime>(app: &AppHandle<R>) -> Result<Option<String>, String> {
    let selected = app
        .dialog()
        .file()
        .set_title("Choose a folder to share")
        .blocking_pick_folder();

    selected
        .map(|path| {
            path.into_path()
                .map_err(|_| {
                    "Porta couldn't read that folder path. Pick the folder again.".to_owned()
                })
                .and_then(path_to_string)
        })
        .transpose()
}

#[tauri::command]
pub(crate) fn reveal_in_finder(app: AppHandle, path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err(FOLDER_MISSING.to_owned());
    }

    app.opener().reveal_item_in_dir(path).map_err(|_| {
        "Porta couldn't open Finder. Open the folder from Finder and try again.".to_owned()
    })
}

#[tauri::command]
pub(crate) fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    let parsed = Url::parse(&url)
        .map_err(|_| "This link is invalid. Restart the share to get a new link.".to_owned())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(
            "This link can't be opened safely. Restart the share to get a new link.".to_owned(),
        );
    }

    app.opener().open_url(url, None::<&str>).map_err(|_| {
        "Porta couldn't open your browser. Copy the link and open it manually.".to_owned()
    })
}

#[tauri::command]
pub(crate) fn get_settings(store: State<'_, Store>) -> Result<Settings, String> {
    store.read(|_, settings| settings.clone())
}

#[tauri::command]
pub(crate) fn update_settings(
    app: AppHandle,
    store: State<'_, Store>,
    patch: SettingsPatch,
) -> Result<Settings, String> {
    let previous = store.read(|_, settings| settings.clone())?;
    let next = apply_settings_patch(&previous, &patch);

    if let Err(error) = apply_setting_side_effects(&app, &previous, &next) {
        let _ = apply_setting_side_effects(&app, &next, &previous);
        return Err(error);
    }
    let save_result = store.update(|_, settings| {
        *settings = next.clone();
        Ok(next.clone())
    });

    if save_result.is_err() {
        let _ = apply_setting_side_effects(&app, &next, &previous);
    }

    save_result
}

fn build_share(input: &CreateShareInput) -> Result<Share, String> {
    let (path, port, default_name) = match input.kind {
        ShareKind::Folder => {
            let requested_path = input
                .path
                .as_deref()
                .ok_or_else(|| "Choose a folder to share, then try again.".to_owned())?;
            let canonical_path = Path::new(requested_path)
                .canonicalize()
                .map_err(|_| FOLDER_MISSING.to_owned())?;
            if !canonical_path.is_dir() {
                return Err("Choose a folder instead of a file, then try again.".to_owned());
            }
            let default_name = canonical_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Shared Folder")
                .to_owned();
            (Some(path_to_string(canonical_path)?), None, default_name)
        }
        ShareKind::Port => {
            let port = input
                .port
                .filter(|port| *port > 0)
                .ok_or_else(|| "Enter a port from 1 to 65535, then try again.".to_owned())?;
            (None, Some(port), format!("Port {port}"))
        }
    };

    let name = normalized_name(input.name.as_deref(), &default_name)?;
    validate_password(input.password.as_deref())?;

    Ok(Share {
        id: Uuid::new_v4().to_string(),
        kind: input.kind,
        name,
        path,
        port,
        url: None,
        status: ShareStatus::Stopped,
        error: None,
        password_protected: input.password.is_some(),
        show_listing: input.show_listing.unwrap_or(true),
        allow_uploads: input.allow_uploads.unwrap_or(false),
        auto_start: input.auto_start.unwrap_or(false),
        stats: ShareStats::default(),
        created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    })
}

fn apply_share_patch(share: &mut Share, patch: &UpdateShareInput) -> Result<(), String> {
    if let Some(name) = patch.name.as_deref() {
        share.name = normalized_name(Some(name), &share.name)?;
    }
    if let Some(password) = patch.password.as_ref() {
        share.password_protected = password.is_some();
    }
    if let Some(show_listing) = patch.show_listing {
        share.show_listing = show_listing;
    }
    if let Some(allow_uploads) = patch.allow_uploads {
        share.allow_uploads = allow_uploads;
    }
    if let Some(auto_start) = patch.auto_start {
        share.auto_start = auto_start;
    }
    Ok(())
}

fn patch_requires_runtime_restart(share: &Share, patch: &UpdateShareInput) -> bool {
    matches!(share.status, ShareStatus::Starting | ShareStatus::Live)
        && share.kind == ShareKind::Folder
        && (patch.password.is_some()
            || patch.show_listing.is_some()
            || patch.allow_uploads.is_some())
}

fn normalized_name(requested: Option<&str>, fallback: &str) -> Result<String, String> {
    let name = requested.unwrap_or(fallback).trim();
    if name.is_empty() {
        return Err("Give this share a name, then try again.".to_owned());
    }
    Ok(name.to_owned())
}

fn validate_password(password: Option<&str>) -> Result<(), String> {
    if password.is_some_and(str::is_empty) {
        return Err(
            "Choose a password or turn password protection off, then try again.".to_owned(),
        );
    }
    Ok(())
}

fn path_to_string(path: PathBuf) -> Result<String, String> {
    path.into_os_string().into_string().map_err(|_| {
        "Porta can't use this folder name. Rename the folder in Finder, then try again.".to_owned()
    })
}

fn emit_app_event<R: Runtime>(app: &AppHandle<R>, event: AppEvent) -> Result<(), String> {
    app.emit("app_event", event).map_err(|_| {
        "Porta saved the change but couldn't refresh its windows. Close and reopen the window to see it."
            .to_owned()
    })?;
    let _ = crate::tray::refresh(app);
    Ok(())
}

fn apply_settings_patch(settings: &Settings, patch: &SettingsPatch) -> Settings {
    Settings {
        launch_at_login: patch.launch_at_login.unwrap_or(settings.launch_at_login),
        auto_start_shares: patch
            .auto_start_shares
            .unwrap_or(settings.auto_start_shares),
        show_dock_icon: patch.show_dock_icon.unwrap_or(settings.show_dock_icon),
        notify_on_first_visitor: patch
            .notify_on_first_visitor
            .unwrap_or(settings.notify_on_first_visitor),
        copy_url_on_start: patch
            .copy_url_on_start
            .unwrap_or(settings.copy_url_on_start),
        theme: patch.theme.unwrap_or(settings.theme),
    }
}

fn apply_setting_side_effects(
    app: &AppHandle,
    previous: &Settings,
    next: &Settings,
) -> Result<(), String> {
    if previous.launch_at_login != next.launch_at_login {
        apply_autolaunch(app, next.launch_at_login)?;
    }

    #[cfg(target_os = "macos")]
    if previous.show_dock_icon != next.show_dock_icon {
        apply_dock_policy(app, next.show_dock_icon)?;
    }

    Ok(())
}

pub(crate) fn apply_initial_app_settings(app: &AppHandle) -> Result<(), String> {
    let (launch_at_login, show_dock_icon) = app
        .state::<Store>()
        .read(|_, settings| (settings.launch_at_login, settings.show_dock_icon))?;
    apply_autolaunch(app, launch_at_login)?;

    #[cfg(target_os = "macos")]
    apply_dock_policy(app, show_dock_icon)?;

    Ok(())
}

fn apply_autolaunch(app: &AppHandle, enabled: bool) -> Result<(), String> {
    const ERROR: &str = "Porta couldn't update Login Items. Open System Settings, then try again.";
    let autolaunch = app.autolaunch();
    if enabled {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    }
    .map_err(|_| ERROR.to_owned())?;

    let actual = autolaunch.is_enabled().map_err(|_| ERROR.to_owned())?;
    if actual != enabled {
        return Err(ERROR.to_owned());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn apply_dock_policy(app: &AppHandle, show_dock_icon: bool) -> Result<(), String> {
    let policy = match dock_policy(show_dock_icon) {
        DockPolicy::Regular => tauri::ActivationPolicy::Regular,
        DockPolicy::Accessory => tauri::ActivationPolicy::Accessory,
    };
    app.set_activation_policy(policy).map_err(|_| {
        "Porta couldn't update the Dock icon. Quit and reopen Porta, then try again.".to_owned()
    })
}

#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
enum DockPolicy {
    Regular,
    Accessory,
}

#[cfg(target_os = "macos")]
fn dock_policy(show_dock_icon: bool) -> DockPolicy {
    if show_dock_icon {
        DockPolicy::Regular
    } else {
        DockPolicy::Accessory
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::mpsc, time::Duration};

    use serde_json::Value;
    use tauri::{Listener, WebviewWindowBuilder};
    use tempfile::tempdir;

    use super::{
        apply_settings_patch, apply_share_patch, build_share, create_share_in, delete_share_in,
        emit_app_event, patch_requires_runtime_restart, prepare_auto_start_share_ids,
    };
    use crate::{
        settings::{Settings, SettingsPatch, Theme},
        shares::{AppEvent, CreateShareInput, Share, ShareKind, ShareStatus, UpdateShareInput},
        store::Store,
    };

    fn fixture_share() -> Share {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_contract.json"))
                .expect("fixture should contain JSON");
        serde_json::from_value(fixture["share"].clone()).expect("fixture share should deserialize")
    }

    #[test]
    fn builds_and_patches_contract_shares_while_stopped() {
        let folder = tempdir().expect("temporary folder should be created");
        let input = CreateShareInput {
            kind: ShareKind::Folder,
            path: Some(folder.path().display().to_string()),
            port: None,
            name: Some("  Demo files  ".to_owned()),
            password: Some("secret".to_owned()),
            show_listing: None,
            allow_uploads: None,
            auto_start: Some(true),
            start_now: Some(true),
        };
        let mut share = build_share(&input).expect("valid folder share should be built");

        assert_eq!(share.status, ShareStatus::Stopped);
        assert_eq!(share.name, "Demo files");
        assert_eq!(
            share.path.as_deref(),
            folder
                .path()
                .canonicalize()
                .ok()
                .and_then(|path| path.to_str().map(str::to_owned))
                .as_deref()
        );
        assert!(share.password_protected);
        assert!(share.show_listing);
        assert!(share.auto_start);

        apply_share_patch(
            &mut share,
            &UpdateShareInput {
                name: Some("Updated".to_owned()),
                password: Some(None),
                show_listing: Some(false),
                allow_uploads: Some(true),
                auto_start: Some(false),
            },
        )
        .expect("valid patch should apply");

        assert_eq!(share.name, "Updated");
        assert!(!share.password_protected);
        assert!(!share.show_listing);
        assert!(share.allow_uploads);
        assert!(!share.auto_start);
    }

    #[test]
    fn live_runtime_settings_restart_but_a_rename_preserves_the_public_url() {
        let mut share = fixture_share();
        share.kind = ShareKind::Folder;
        share.status = ShareStatus::Live;
        share.url = Some("https://same-link.trycloudflare.com".to_owned());

        let rename = UpdateShareInput {
            name: Some("Renamed while live".to_owned()),
            ..UpdateShareInput::default()
        };
        assert!(!patch_requires_runtime_restart(&share, &rename));
        apply_share_patch(&mut share, &rename).expect("rename should apply");
        assert_eq!(share.name, "Renamed while live");
        assert_eq!(share.status, ShareStatus::Live);
        assert_eq!(
            share.url.as_deref(),
            Some("https://same-link.trycloudflare.com")
        );

        for patch in [
            UpdateShareInput {
                password: Some(None),
                ..UpdateShareInput::default()
            },
            UpdateShareInput {
                show_listing: Some(false),
                ..UpdateShareInput::default()
            },
            UpdateShareInput {
                allow_uploads: Some(true),
                ..UpdateShareInput::default()
            },
        ] {
            assert!(patch_requires_runtime_restart(&share, &patch));
        }

        share.status = ShareStatus::Stopped;
        assert!(!patch_requires_runtime_restart(
            &share,
            &UpdateShareInput {
                password: Some(Some("new password".to_owned())),
                ..UpdateShareInput::default()
            }
        ));
    }

    #[test]
    fn settings_patch_changes_only_present_fields() {
        let settings = Settings::default();
        let next = apply_settings_patch(
            &settings,
            &SettingsPatch {
                theme: Some(Theme::Dark),
                notify_on_first_visitor: Some(false),
                ..SettingsPatch::default()
            },
        );

        assert_eq!(next.theme, Theme::Dark);
        assert!(!next.notify_on_first_visitor);
        assert_eq!(next.launch_at_login, settings.launch_at_login);
        assert_eq!(next.auto_start_shares, settings.auto_start_shares);
        assert_eq!(next.show_dock_icon, settings.show_dock_icon);
        assert_eq!(next.copy_url_on_start, settings.copy_url_on_start);
    }

    #[test]
    fn auto_start_selection_honors_both_switches_and_resets_stale_runtime_state() {
        let data_dir = tempdir().expect("temporary data directory should be created");
        let store = Store::load_from_dir(data_dir.path()).expect("test store should load");
        let mut opted_in = fixture_share();
        opted_in.id = "opted-in".to_owned();
        opted_in.auto_start = true;
        opted_in.status = ShareStatus::Live;
        opted_in.url = Some("https://stale.trycloudflare.com".to_owned());

        let mut opted_out = fixture_share();
        opted_out.id = "opted-out".to_owned();
        opted_out.auto_start = false;
        opted_out.status = ShareStatus::Starting;

        store
            .update(|shares, settings| {
                *shares = vec![opted_in, opted_out];
                settings.auto_start_shares = true;
                Ok(())
            })
            .expect("fixture state should save");

        assert_eq!(
            prepare_auto_start_share_ids(&store).expect("selection should succeed"),
            vec!["opted-in"]
        );
        store
            .read(|shares, _| {
                assert!(shares
                    .iter()
                    .all(|share| share.status == ShareStatus::Stopped));
                assert!(shares.iter().all(|share| share.url.is_none()));
            })
            .expect("normalized shares should be readable");

        store
            .update(|_, settings| {
                settings.auto_start_shares = false;
                Ok(())
            })
            .expect("master switch should save");
        assert!(prepare_auto_start_share_ids(&store)
            .expect("disabled selection should succeed")
            .is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dock_visibility_maps_to_the_expected_live_activation_policy() {
        assert_eq!(super::dock_policy(true), super::DockPolicy::Regular);
        assert_eq!(super::dock_policy(false), super::DockPolicy::Accessory);
    }

    #[test]
    fn broadcasts_share_changes_to_every_open_window() {
        let app = tauri::test::mock_app();
        let first = WebviewWindowBuilder::new(&app, "first", Default::default())
            .build()
            .expect("first mock window should open");
        let second = WebviewWindowBuilder::new(&app, "second", Default::default())
            .build()
            .expect("second mock window should open");
        let (sender, receiver) = mpsc::channel();
        let first_sender = sender.clone();

        first.listen("app_event", move |event| {
            first_sender
                .send(("first", event.payload().to_owned()))
                .expect("first listener should report the event");
        });
        second.listen("app_event", move |event| {
            sender
                .send(("second", event.payload().to_owned()))
                .expect("second listener should report the event");
        });

        let share = build_share(&CreateShareInput {
            kind: ShareKind::Port,
            path: None,
            port: Some(4173),
            name: Some("Preview".to_owned()),
            password: None,
            show_listing: None,
            allow_uploads: None,
            auto_start: None,
            start_now: None,
        })
        .expect("port share should be valid");
        let event = AppEvent::ShareChanged { share };
        let expected = serde_json::to_value(&event).expect("event should serialize");

        emit_app_event(app.handle(), event).expect("event should broadcast");

        let mut labels = HashSet::new();
        for _ in 0..2 {
            let (label, payload) = receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("both windows should receive the event");
            labels.insert(label);
            assert_eq!(
                serde_json::from_str::<Value>(&payload).expect("payload should be JSON"),
                expected
            );
        }
        assert_eq!(labels, HashSet::from(["first", "second"]));
    }

    #[test]
    fn create_survives_relaunch_and_delete_is_permanent() {
        let data_dir = tempdir().expect("temporary data directory should be created");

        let created_id = {
            let app = tauri::test::mock_app();
            let store = Store::load_from_dir(data_dir.path()).expect("test store should load");
            let created = create_share_in(
                app.handle(),
                &store,
                CreateShareInput {
                    kind: ShareKind::Port,
                    path: None,
                    port: Some(4173),
                    name: Some("Persistent preview".to_owned()),
                    password: None,
                    show_listing: None,
                    allow_uploads: None,
                    auto_start: None,
                    start_now: Some(false),
                },
            )
            .expect("create command implementation should succeed");

            assert_eq!(created.name, "Persistent preview");
            assert_eq!(created.status, ShareStatus::Stopped);
            created.id
        };

        {
            let relaunched = tauri::test::mock_app();
            let store = Store::load_from_dir(data_dir.path()).expect("saved store should reload");
            let shares: Vec<Share> = store
                .read(|shares, _| shares.to_vec())
                .expect("reloaded shares should be readable");
            assert_eq!(shares.len(), 1);
            assert_eq!(shares[0].id, created_id);

            delete_share_in(relaunched.handle(), &store, created_id)
                .expect("delete command implementation should succeed");
        }

        let store = Store::load_from_dir(data_dir.path()).expect("deleted store should reload");
        let shares: Vec<Share> = store
            .read(|shares, _| shares.to_vec())
            .expect("shares should be readable after deletion relaunch");
        assert!(shares.is_empty());
    }
}
