use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    AppHandle, Manager, Runtime,
};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::{
    commands,
    shares::{Share, ShareStatus},
    store::Store,
    tunnel::TunnelManager,
};

const TRAY_ID: &str = "porta-tray";
const ADD_ID: &str = "tray:add";
const OPEN_ID: &str = "tray:open";
const QUIT_ID: &str = "tray:quit";
const COPY_PREFIX: &str = "tray:copy:";
const TOGGLE_PREFIX: &str = "tray:toggle:";
#[cfg(target_os = "windows")]
const TRAY_ERROR: &str = "Porta couldn't create its notification-area menu. Quit and reopen Porta.";
#[cfg(target_os = "macos")]
const TRAY_ERROR: &str = "Porta couldn't create its menu-bar menu. Quit and reopen Porta.";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const TRAY_ERROR: &str = "Porta couldn't create its system tray menu. Quit and reopen Porta.";

#[derive(Debug, PartialEq, Eq)]
struct ShareMenuRow {
    label: String,
    copy_enabled: bool,
    toggle_label: &'static str,
}

pub(crate) fn setup<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let shares = app.state::<Store>().read(|shares, _| shares.to_vec())?;
    let menu = build_menu(app, &shares).map_err(|_| TRAY_ERROR.to_owned())?;
    let icon = tray_icon(&shares)?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(tray_icon_is_template())
        .tooltip("Porta")
        .menu(&menu)
        .on_menu_event(handle_menu_event)
        .build(app)
        .map_err(|_| TRAY_ERROR.to_owned())?;
    Ok(())
}

pub(crate) fn refresh<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };
    let shares = app.state::<Store>().read(|shares, _| shares.to_vec())?;
    let menu = build_menu(app, &shares).map_err(|_| TRAY_ERROR.to_owned())?;
    tray.set_icon_with_as_template(Some(tray_icon(&shares)?), tray_icon_is_template())
        .map_err(|_| TRAY_ERROR.to_owned())?;
    tray.set_menu(Some(menu)).map_err(|_| TRAY_ERROR.to_owned())
}

fn tray_icon(shares: &[Share]) -> Result<Image<'static>, String> {
    #[cfg(target_os = "macos")]
    let bytes: &'static [u8] = if has_live_share(shares) {
        include_bytes!("../icons/trayActiveTemplate.png")
    } else {
        include_bytes!("../icons/trayTemplate.png")
    };

    #[cfg(not(target_os = "macos"))]
    let bytes: &'static [u8] = if has_live_share(shares) {
        include_bytes!("../icons/trayWindowsActive.png")
    } else {
        include_bytes!("../icons/trayWindows.png")
    };

    Image::from_bytes(bytes).map_err(|_| TRAY_ERROR.to_owned())
}

const fn tray_icon_is_template() -> bool {
    cfg!(target_os = "macos")
}

fn has_live_share(shares: &[Share]) -> bool {
    shares
        .iter()
        .any(|share| matches!(share.status, ShareStatus::Live))
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, shares: &[Share]) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;

    for share in shares {
        let row = share_menu_row(share);
        let submenu = Submenu::with_id(app, format!("tray:share:{}", share.id), row.label, true)?;
        let copy = MenuItem::with_id(
            app,
            format!("{COPY_PREFIX}{}", share.id),
            "Copy link",
            row.copy_enabled,
            None::<&str>,
        )?;
        let toggle = MenuItem::with_id(
            app,
            format!("{TOGGLE_PREFIX}{}", share.id),
            row.toggle_label,
            true,
            None::<&str>,
        )?;
        submenu.append_items(&[&copy, &toggle])?;
        menu.append(&submenu)?;
    }

    if !shares.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append(&MenuItem::with_id(
        app,
        ADD_ID,
        "Share a folder…",
        true,
        Some("CmdOrCtrl+O"),
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        OPEN_ID,
        "Open Porta",
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        QUIT_ID,
        "Quit Porta",
        true,
        Some("CmdOrCtrl+Q"),
    )?)?;
    Ok(menu)
}

fn share_menu_row(share: &Share) -> ShareMenuRow {
    let active = matches!(share.status, ShareStatus::Starting | ShareStatus::Live);
    let toggle_label = if active { "Turn off" } else { "Turn on" };
    ShareMenuRow {
        label: format!(
            "{} {} — Copy link / {toggle_label}",
            if active { "●" } else { "○" },
            share.name
        ),
        copy_enabled: share.url.is_some(),
        toggle_label,
    }
}

fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();
    match id {
        ADD_ID => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                open_folder_sheet(&app);
            });
        }
        OPEN_ID => {
            let _ = show_main_window(app);
        }
        QUIT_ID => app.exit(0),
        _ => {
            if let Some(share_id) = id.strip_prefix(COPY_PREFIX) {
                copy_share_url(app, share_id);
            } else if let Some(share_id) = id.strip_prefix(TOGGLE_PREFIX) {
                let app = app.clone();
                let share_id = share_id.to_owned();
                tauri::async_runtime::spawn(async move {
                    toggle_share(&app, &share_id).await;
                });
            }
        }
    }
}

fn copy_share_url<R: Runtime>(app: &AppHandle<R>, share_id: &str) {
    let url = app.state::<Store>().read(|shares, _| {
        shares
            .iter()
            .find(|share| share.id == share_id)
            .and_then(|share| share.url.clone())
    });
    if let Ok(Some(url)) = url {
        let _ = app.clipboard().write_text(url);
    }
}

async fn toggle_share<R: Runtime>(app: &AppHandle<R>, share_id: &str) {
    let status = app.state::<Store>().read(|shares, _| {
        shares
            .iter()
            .find(|share| share.id == share_id)
            .map(|share| share.status)
    });
    let tunnels = app.state::<TunnelManager>();
    match status {
        Ok(Some(ShareStatus::Starting | ShareStatus::Live)) => {
            let _ = commands::stop_share_in(app, &tunnels, share_id).await;
        }
        Ok(Some(_)) => {
            let _ = commands::start_share_in(app, &tunnels, share_id).await;
        }
        _ => {}
    }
}

fn open_folder_sheet<R: Runtime>(app: &AppHandle<R>) {
    let Ok(Some(path)) = commands::pick_folder_in(app) else {
        return;
    };
    let _ = show_main_window(app);
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Ok(path) = serde_json::to_string(&path) else {
        return;
    };
    let _ = window.eval(format!(
        "window.dispatchEvent(new CustomEvent('porta:folder-dropped', {{ detail: {path} }}));"
    ));
}

pub(crate) fn show_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Porta couldn't find its main window. Quit and reopen Porta.".to_owned())?;
    window.show().map_err(|_| TRAY_ERROR.to_owned())?;
    window.unminimize().map_err(|_| TRAY_ERROR.to_owned())?;
    window.set_focus().map_err(|_| TRAY_ERROR.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{has_live_share, share_menu_row};
    use crate::shares::{Share, ShareStatus};

    fn fixture() -> Share {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_contract.json"))
                .expect("fixture should contain JSON");
        serde_json::from_value(fixture["share"].clone()).expect("fixture share should deserialize")
    }

    #[test]
    fn live_and_stopped_share_rows_offer_the_expected_quick_actions() {
        let mut share = fixture();
        share.name = "Client Mockups".to_owned();
        share.status = ShareStatus::Live;
        share.url = Some("https://quiet-harbor.trycloudflare.com".to_owned());
        let live = share_menu_row(&share);
        assert_eq!(live.label, "● Client Mockups — Copy link / Turn off");
        assert!(live.copy_enabled);
        assert_eq!(live.toggle_label, "Turn off");

        share.status = ShareStatus::Stopped;
        share.url = None;
        let stopped = share_menu_row(&share);
        assert_eq!(stopped.label, "○ Client Mockups — Copy link / Turn on");
        assert!(!stopped.copy_enabled);
        assert_eq!(stopped.toggle_label, "Turn on");
    }

    #[test]
    fn tray_badge_appears_only_when_a_share_is_live() {
        let mut share = fixture();
        share.status = ShareStatus::Starting;
        assert!(!has_live_share(&[share.clone()]));

        share.status = ShareStatus::Live;
        assert!(has_live_share(&[share.clone()]));

        share.status = ShareStatus::Error;
        assert!(!has_live_share(&[share]));
    }
}
