mod commands;
mod diagnostics;
mod github;
mod linear;
mod rpi;
mod secrets;

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
/// Payload: `PullRequest` (opened, polled, merged).
pub const PR_UPDATED: &str = "pr-updated";
/// Payload: `linear::Notice` for the notice bar.
pub const NOTICE: &str = "notice";
const DB_FILE: &str = "humanlayer-local.db";

pub struct AppState {
    pub db: Arc<Db>,
    pub rt: Arc<Runtime>,
    pub artifacts: Arc<ArtifactStore>,
    /// Task id -> stage whose session waits for the worktree.
    pub pending_stage: Mutex<HashMap<Id, String>>,
    pub secrets: Arc<secrets::Secrets>,
    pub linear: linear::LinearState,
}

struct TauriSink(AppHandle);

impl Sink for TauriSink {
    fn event(&self, event: &SessionEvent) {
        if let Err(e) = self.0.emit(SESSION_EVENT, event) {
            log::warn!("emit failed: {e}");
        }
    }
    fn session(&self, session: &Session) {
        if let Err(e) = self.0.emit(SESSION_UPDATED, session) {
            log::warn!("emit failed: {e}");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri_plugin_log::{RotationStrategy, Target, TargetKind};
    let app = tauri::Builder::default()
        // ~/Library/Logs/<identifier>/humanlayer-local.log, rotated at 5 MB.
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::LogDir {
                        file_name: Some("humanlayer-local".into()),
                    }),
                ])
                .level(log::LevelFilter::Info)
                .max_file_size(5_000_000)
                .rotation_strategy(RotationStrategy::KeepSome(3))
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Warm the shell-env cache off the main thread so the first
            // command is fast. Errors surface later, on first use.
            std::thread::spawn(hl_core::shell_env::get);

            log::info!("HumanLayer Local {} starting", app.package_info().version);
            // ~/Library/Application Support/<identifier>/humanlayer-local.db
            let data = app.path().app_data_dir()?;
            let db = Arc::new(Db::open(&data.join(DB_FILE))?);
            // No worktree setup survives an app restart.
            db.fail_unfinished_worktrees()?;
            let artifacts = Arc::new(ArtifactStore::new(data.join("artifacts")));
            let plugin = rpi::plugin_dir(app.handle());
            if plugin.is_none() {
                log::warn!("RPI plugin folder not found; skills will be unavailable");
            }
            // Also marks sessions left running by the last app run as interrupted.
            let secrets = Arc::new(secrets::Secrets::new());
            // Sessions get the shell env plus saved keys (LINEAR_API_KEY,
            // GH_TOKEN), so the `linear` and `gh` CLIs work inside them.
            let env_secrets = secrets.clone();
            let env = move || {
                let (mut env, err) = hl_core::shell_env::get();
                for (k, v) in env_secrets.env_vars() {
                    env.insert(k.into(), v);
                }
                (env, err)
            };
            let rt = Runtime::with_options(
                db.clone(),
                TauriSink(app.handle().clone()),
                Arc::new(env),
                rpi::prepare_fn(db.clone(), artifacts.clone(), plugin),
            )?;
            if let Err(e) =
                rpi::watch_artifacts(app.handle().clone(), db.clone(), artifacts.clone())
            {
                log::warn!("docs watcher failed to start: {e}");
            }
            app.manage(AppState {
                db,
                rt,
                artifacts,
                pending_stage: Mutex::new(HashMap::new()),
                secrets,
                linear: linear::LinearState::default(),
            });
            linear::start_polling(app.handle().clone());
            github::start_polling(app.handle().clone());
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
            commands::linear_status,
            commands::set_linear_key,
            commands::clear_linear_key,
            commands::import_linear_ticket,
            commands::sync_linear_now,
            commands::post_doc_to_linear,
            commands::read_artifact_data_url,
            commands::github_status,
            commands::set_github_token,
            commands::clear_github_token,
            commands::import_github_issue,
            commands::open_pr,
            commands::list_project_prs,
            commands::refresh_pr,
            commands::merge_pr,
            commands::check_tools,
            commands::diagnostics,
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
