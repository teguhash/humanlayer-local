mod commands;

use std::sync::Arc;

use hl_agents::{AgentEvent, AgentManager};
use hl_core::Db;
use tauri::{Emitter, Manager};

pub const AGENT_EVENT: &str = "agent-event";
const DB_FILE: &str = "humanlayer-local.db";

pub struct AppState {
    pub db: Db,
    pub agents: Arc<AgentManager>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Warm the shell-env cache off the main thread so the first
            // command is fast. Errors surface later, on first use.
            std::thread::spawn(hl_core::shell_env::get);

            // ~/Library/Application Support/<identifier>/humanlayer-local.db
            let path = app.path().app_data_dir()?.join(DB_FILE);
            let db = Db::open(&path)?;
            let handle = app.handle().clone();
            let agents = AgentManager::new(move |ev: AgentEvent| {
                if let Err(e) = handle.emit(AGENT_EVENT, ev) {
                    eprintln!("emit failed: {e}");
                }
            });
            app.manage(AppState { db, agents });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_projects,
            commands::create_project,
            commands::delete_project,
            commands::list_tasks,
            commands::create_task,
            commands::delete_task,
            commands::list_sessions,
            commands::create_session,
            commands::delete_session,
            commands::start_run,
            commands::respond_approval,
            commands::stop_run,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
