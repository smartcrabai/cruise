use tauri::Manager;

pub mod commands;
pub mod state;

/// Tauri application entry point.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;
            let data_dir = cruise::paths::data_dir()?;
            let manager = cruise::session::SessionManager::new(data_dir);
            app.manage(state::AppState::new(
                cruise::application::CruiseApplication::new(manager),
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_sessions,
            commands::get_session,
            commands::get_session_dag,
            commands::get_session_plan,
            commands::get_session_log,
            commands::run_session,
            commands::cancel_session,
            commands::cancel_run_all,
            commands::respond_to_option,
            commands::respond_to_ask,
            commands::pending_prompts,
            commands::clean_sessions,
            commands::list_configs,
            commands::create_session,
            commands::use_input_as_plan,
            commands::approve_session,
            commands::publish_plan_issue,
            commands::delete_session,
            commands::discard_session,
            commands::reset_session,
            commands::update_session,
            commands::edit_current_step,
            commands::regenerate_session_plan,
            commands::generate_plan_for_draft,
            commands::fix_session,
            commands::ask_session,
            commands::list_directory,
            commands::list_github_repos,
            commands::run_all_sessions,
            commands::get_update_readiness,
            commands::get_new_session_history_summary,
            commands::get_new_session_config_defaults,
            commands::get_new_session_draft,
            commands::save_new_session_draft,
            commands::clear_new_session_draft,
            commands::get_app_config,
            commands::update_app_config,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("Tauri error: {e}");
            std::process::exit(1);
        });
}
