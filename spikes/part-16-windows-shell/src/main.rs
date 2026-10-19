// Hide the console window in release builds on Windows; keep it in debug for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("failed to run the A-Eye spike-16 Tauri shell");
}
