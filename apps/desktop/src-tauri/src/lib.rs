//! Tauri backend for the Collapse desktop app: the app wiring only. The
//! commands themselves live in [`commands`], the path predicates they rely on
//! in [`paths`] and the naming exchange the extract dialog needs in [`names`],
//! all public so `tests/` can drive them directly.

pub mod commands;
pub mod names;
pub mod paths;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Keep in lockstep with the `invoke('...')` calls in `src/App.vue`;
        // nothing type-checks that crossing, so `tests/ipc.rs` pins it.
        .invoke_handler(tauri::generate_handler![
            commands::is_directory,
            commands::compress_path,
            commands::unwritable_names,
            commands::extract_archive,
            commands::check_server
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
