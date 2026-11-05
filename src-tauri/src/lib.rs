mod commands;
mod rpi;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hl_agents::{Runtime, SessionEvent, Sink};
use hl_core::Db;
use hl_core::artifacts::ArtifactStore;
use hl_core::model::{Id, Session};
use tauri::{AppHandle, Emitter, Manager};

/// Webview events. Payloads: `SessionEvent` and `Session`.
pub const SESSION_EVENT: &str = "session-event";
pub const SESSION_UPDATED: &str = "session-updated";
/// Payload: `Task` (worktree progress, archive).
pub const TASK_UPDATED: &str = "task-updated";
/// Payload: `Session` (started by a stage move or Iterate).
pub const SESSION_CREATED: &str = "session-created";
/// Payload: task id whose docs list changed.
pub const ARTIFACTS_UPDATED: &str = "artifacts-updated";
const DB_FILE: &str = "humanlayer-local.db";

pub struct AppState {
    pub db: Arc<Db>,
    pub rt: Arc<Runtime>,
    pub artifacts: Arc<ArtifactStore>,
    /// Task id -> stage whose session waits for the worktree.
    pub pending_stage: Mutex<HashMap<Id, String>>,
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
            let data = app.path().app_data_dir()?;
            let db = Arc::new(Db::open(&data.join(DB_FILE))?);
            // No worktree setup survives an app restart.
            db.fail_unfinished_worktrees()?;
            let artifacts = Arc::new(ArtifactStore::new(data.join("artifacts")));
            let plugin = rpi::plugin_dir(app.handle());
            if plugin.is_none() {
                eprintln!("RPI plugin folder not found; skills will be unavailable");
            }
            // Also marks sessions left running by the last app run as interrupted.
            let rt = Runtime::with_options(
                db.clone(),
                TauriSink(app.handle().clone()),
                Arc::new(hl_core::shell_env::get),
                rpi::prepare_fn(db.clone(), artifacts.clone(), plugin),
            )?;
            if let Err(e) =
                rpi::watch_artifacts(app.handle().clone(), db.clone(), artifacts.clone())
            {
                eprintln!("docs watcher failed to start: {e}");
            }
            app.manage(AppState {
                db,
                rt,
                artifacts,
                pending_stage: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_projects,
            commands::create_project,
            commands::delete_project,
            commands::list_tasks,
            commands::create_task,
            commands::delete_task,
            commands::update_task,
            commands::move_task,
            commands::set_task_agent,
            commands::set_project_agent,
            commands::start_stage_session,
            commands::list_artifacts,
            commands::read_artifact,
            commands::iterate_artifact,
            commands::create_worktree,
            commands::retry_setup,
            commands::worktree_dirty_files,
            commands::get_diff,
            commands::get_file_diff,
            commands::archive_task,
            commands::unarchive_task,
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
