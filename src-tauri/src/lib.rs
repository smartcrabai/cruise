pub mod commands;
pub mod events;
pub mod gui_option_handler;
pub mod state;

/// Tauri application entry point.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(state::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::list_sessions,
            commands::get_session,
            commands::get_session_plan,
            commands::get_session_log,
            commands::run_session,
            commands::cancel_session,
            commands::respond_to_option,
            commands::clean_sessions,
            commands::list_configs,
            commands::create_session,
            commands::approve_session,
            commands::discard_session,
            commands::reset_session,
            commands::fix_session,
            commands::list_directory,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("Tauri error: {e}");
            std::process::exit(1);
        });
}
