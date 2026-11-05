mod commands;

use std::sync::Arc;

use hl_agents::{Runtime, SessionEvent, Sink};
use hl_core::Db;
use hl_core::model::Session;
use tauri::{AppHandle, Emitter, Manager};

/// Webview events. Payloads: `SessionEvent` and `Session`.
pub const SESSION_EVENT: &str = "session-event";
pub const SESSION_UPDATED: &str = "session-updated";
const DB_FILE: &str = "humanlayer-local.db";

pub struct AppState {
    pub db: Arc<Db>,
    pub rt: Arc<Runtime>,
}

struct TauriSink(AppHandle);

impl Sink for TauriSink {
    fn event(&self, event: &SessionEvent) {
        if let Err(e) = self.0.emit(SESSION_EVENT, event) {
            eprintln!("emit failed: {e}");
        }
    }
    fn session(&self, session: &Session) {
        if let Err(e) = self.0.emit(SESSION_UPDATED, session) {
            eprintln!("emit failed: {e}");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Warm the shell-env cache off the main thread so the first
            // command is fast. Errors surface later, on first use.
            std::thread::spawn(hl_core::shell_env::get);

            // ~/Library/Application Support/<identifier>/humanlayer-local.db
            let path = app.path().app_data_dir()?.join(DB_FILE);
            let db = Arc::new(Db::open(&path)?);
            // Also marks sessions left running by the last app run as interrupted.
            let rt = Runtime::new(db.clone(), TauriSink(app.handle().clone()))?;
            app.manage(AppState { db, rt });
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
            commands::get_transcript,
            commands::get_usage,
            commands::send_message,
            commands::interrupt_session,
            commands::stop_session,
            commands::respond_approval,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event
            && let Some(state) = handle.try_state::<AppState>()
        {
            // Don't leave CLI processes behind.
            state.rt.shutdown();
        }
    });
}
