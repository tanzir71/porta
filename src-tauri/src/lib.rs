pub mod settings;
pub mod shares;
pub mod stats;
pub mod store;

use settings::Settings;
use shares::Share;
use store::Store;

#[tauri::command]
fn list_shares(store: tauri::State<'_, Store>) -> Result<Vec<Share>, String> {
    store.read(|shares, _| shares.to_vec())
}

#[tauri::command]
fn get_settings(store: tauri::State<'_, Store>) -> Result<Settings, String> {
    store.read(|_, settings| settings.clone())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let store = Store::load(app.handle()).map_err(std::io::Error::other)?;
            tauri::Manager::manage(app, store);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![list_shares, get_settings])
        .run(tauri::generate_context!())
        .expect("Porta could not start");
}
