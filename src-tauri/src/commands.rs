//! Tauri commands. All are `async` so Tauri runs them off the main thread;
//! the slow ones (git, shell env) also move to a blocking thread.
//! Errors are plain strings shown to the user as-is.

use std::path::PathBuf;

use hl_core::model::{AgentKind, Id, Project, Session, Task};
use serde_json::Value;
use tauri::State;

use crate::AppState;

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ---- projects ----

#[tauri::command]
pub async fn list_projects(state: State<'_, AppState>) -> Res<Vec<Project>> {
    state.db.list_projects().map_err(err)
}

/// Create a project for the git repo that contains `path`. The stored path
/// is the repo's top level. `name` defaults to the folder name.
#[tauri::command]
pub async fn create_project(
    state: State<'_, AppState>,
    path: String,
    name: Option<String>,
) -> Res<Project> {
    let top: PathBuf = tauri::async_runtime::spawn_blocking(move || {
        let (env, _) = hl_core::shell_env::get();
        hl_git::repo_toplevel(&PathBuf::from(path), &env)
    })
    .await
    .map_err(err)?
    .map_err(err)?;
    let name = name
        .filter(|n| !n.trim().is_empty())
        .or_else(|| top.file_name().map(|f| f.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "project".into());
    state
        .db
        .create_project(&name, &top.to_string_lossy())
        .map_err(err)
}

#[tauri::command]
pub async fn delete_project(state: State<'_, AppState>, id: Id) -> Res<()> {
    state.db.delete_project(id).map_err(err)
}

// ---- tasks ----

#[tauri::command]
pub async fn list_tasks(state: State<'_, AppState>, project_id: Id) -> Res<Vec<Task>> {
    state.db.list_tasks(project_id).map_err(err)
}

#[tauri::command]
pub async fn create_task(state: State<'_, AppState>, project_id: Id, title: String) -> Res<Task> {
    state.db.create_task(project_id, &title).map_err(err)
}

#[tauri::command]
pub async fn delete_task(state: State<'_, AppState>, id: Id) -> Res<()> {
    state.db.delete_task(id).map_err(err)
}

// ---- sessions ----

#[tauri::command]
pub async fn list_sessions(state: State<'_, AppState>, task_id: Id) -> Res<Vec<Session>> {
    state.db.list_sessions(task_id).map_err(err)
}

#[tauri::command]
pub async fn create_session(
    state: State<'_, AppState>,
    task_id: Id,
    agent: AgentKind,
) -> Res<Session> {
    state.db.create_session(task_id, agent).map_err(err)
}

#[tauri::command]
pub async fn delete_session(state: State<'_, AppState>, id: Id) -> Res<()> {
    state.db.delete_session(id).map_err(err)
}

// ---- agent runs (M0 spike runner; not persisted until M2) ----

#[tauri::command]
pub async fn start_run(
    state: State<'_, AppState>,
    agent: AgentKind,
    cwd: String,
    prompt: String,
) -> Res<String> {
    state.agents.start(agent, &cwd, &prompt).await
}

#[tauri::command]
pub async fn respond_approval(
    state: State<'_, AppState>,
    run_id: String,
    request_id: String,
    allow: bool,
    input: Value,
) -> Res<()> {
    state
        .agents
        .respond_approval(&run_id, &request_id, allow, input)
}

#[tauri::command]
pub async fn stop_run(state: State<'_, AppState>, run_id: String) -> Res<()> {
    state.agents.stop(&run_id)
}
