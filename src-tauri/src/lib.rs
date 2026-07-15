pub mod settings;
pub mod shares;
pub mod stats;

use settings::Settings;
use shares::Share;

#[tauri::command]
fn list_shares() -> Result<Vec<Share>, String> {
    Ok(Vec::new())
}

#[tauri::command]
fn get_settings() -> Result<Settings, String> {
    Ok(Settings::default())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![list_shares, get_settings])
        .run(tauri::generate_context!())
        .expect("Porta could not start");
}
