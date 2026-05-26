mod commands;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            commands::detect_languages,
            commands::start_indexing,
            commands::cancel_indexing,
            commands::get_indexer_status,
            commands::install_indexer,
            commands::uninstall_indexer,
            commands::update_indexer,
            commands::get_config,
            commands::save_config,
            commands::clean_cache,
            commands::validate_index,
            commands::check_updates,
            commands::reveal_in_explorer,
        ])
        .setup(|_app| {
            #[cfg(debug_assertions)]
            {
                use tauri::Manager;

                let window = _app.get_webview_window("main").unwrap();
                window.open_devtools();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
