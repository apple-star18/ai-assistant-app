mod commands;
mod config;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![commands::health::get_app_health])
        .run(tauri::generate_context!())
        .expect("failed to run Tauri application");
}
