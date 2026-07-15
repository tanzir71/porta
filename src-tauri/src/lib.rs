use serde_json::{json, Value};

#[tauri::command]
fn list_shares() -> Result<Vec<Value>, String> {
    Ok(Vec::new())
}

#[tauri::command]
fn get_settings() -> Result<Value, String> {
    Ok(json!({
        "launchAtLogin": false,
        "autoStartShares": true,
        "showDockIcon": true,
        "notifyOnFirstVisitor": true,
        "copyUrlOnStart": true,
        "theme": "system"
    }))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![list_shares, get_settings])
        .run(tauri::generate_context!())
        .expect("Porta could not start");
}
