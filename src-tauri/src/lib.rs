mod commands;
mod credentials;
pub mod server;
pub mod settings;
pub mod shares;
pub mod stats;
pub mod store;
mod tunnel;

use store::Store;
use tunnel::TunnelManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(TunnelManager::default())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let store = Store::load(app.handle()).map_err(std::io::Error::other)?;
            tauri::Manager::manage(app, store);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_shares,
            commands::create_share,
            commands::start_share,
            commands::delete_share,
            commands::update_share,
            commands::pick_folder,
            commands::reveal_in_finder,
            commands::open_url,
            commands::get_settings,
            commands::update_settings,
        ])
        .run(tauri::generate_context!())
        .expect("Porta could not start");
}
