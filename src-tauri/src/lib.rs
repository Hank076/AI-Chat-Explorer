mod project_browser;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            project_browser::list_projects,
            project_browser::list_project_entries,
            project_browser::read_memory,
            project_browser::read_session_timeline,
            project_browser::search_sessions,
            project_browser::delete_session,
            project_browser::delete_project,
            project_browser::get_project_delete_impact,
            project_browser::get_codex_project_discovery_mode,
            project_browser::list_codex_projects,
            project_browser::list_codex_project_entries,
            project_browser::read_codex_session_timeline,
            project_browser::delete_codex_session,
            project_browser::list_vscode_copilot_projects,
            project_browser::list_vscode_copilot_project_entries,
            project_browser::read_vscode_copilot_session_timeline
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
