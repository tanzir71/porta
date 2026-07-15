mod commands;
mod credentials;
mod drag_drop;
#[cfg(test)]
mod error_audit;
mod provider;
pub mod server;
pub mod settings;
pub mod shares;
pub mod stats;
pub mod store;
mod tray;
mod tunnel;

use store::Store;
use tauri::WindowEvent;
use tunnel::TunnelManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if has_quit_arg(&args) {
                app.exit(0);
            } else {
                let _ = tray::show_main_window(app);
            }
        }))
        .manage(TunnelManager::default())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let store = Store::load(app.handle()).map_err(std::io::Error::other)?;
            tauri::Manager::manage(app, store);
            commands::apply_initial_app_settings(app.handle()).map_err(std::io::Error::other)?;
            let auto_start_ids =
                commands::prepare_auto_start_shares(app.handle()).map_err(std::io::Error::other)?;
            tray::setup(app.handle()).map_err(std::io::Error::other)?;
            commands::spawn_auto_start_shares(app.handle(), auto_start_ids);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .on_webview_event(drag_drop::handle)
        .invoke_handler(tauri::generate_handler![
            commands::list_shares,
            commands::create_share,
            commands::start_share,
            commands::stop_share,
            commands::delete_share,
            commands::update_share,
            commands::pick_folder,
            commands::reveal_in_finder,
            commands::open_url,
            commands::get_settings,
            commands::update_settings,
            commands::list_provider_profiles,
            commands::save_provider_profile,
            commands::delete_provider_profile,
            commands::test_provider,
            commands::pick_provider_executable,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| {
            eprintln!("Porta startup detail: {error}");
            eprintln!(
                "Porta couldn't start. Quit and reopen Porta. If it still won't open, reinstall Porta."
            );
            std::process::exit(1);
        });
}

fn has_quit_arg(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--quit")
}

#[cfg(test)]
mod tests {
    use super::has_quit_arg;

    #[test]
    fn recognizes_explicit_quit_argument() {
        assert!(has_quit_arg(&["porta.exe".to_owned(), "--quit".to_owned()]));
        assert!(!has_quit_arg(&["porta.exe".to_owned()]));
    }
}
