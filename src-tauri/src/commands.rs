//! Tauri commands. All are `async` so Tauri runs them off the main thread;
//! the slow ones (git, shell env) also move to a blocking thread.
//! Errors are plain strings shown to the user as-is.

use std::path::PathBuf;

use std::sync::Arc;
use std::time::Duration;

use hl_agents::{Decision, SessionUsage, Transcript};
use hl_core::Db;
use hl_core::model::{AgentKind, Id, Project, Session, Task, worktree_status as ws};
use hl_git::config::WorkspaceConfig;
use hl_git::diff::DiffSummary;
use hl_git::worktree::{self, Branch};
use tauri::{AppHandle, Emitter, State};

use crate::rpi::{self, MoveResult};
use crate::{AppState, TASK_UPDATED};
use hl_core::model::Artifact;

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
    state.db.delete_project(id).map_err(err)?;
    // Stop agent processes whose session row is gone.
    state.rt.prune();
    Ok(())
}

// ---- tasks ----

#[tauri::command]
pub async fn list_tasks(state: State<'_, AppState>, project_id: Id) -> Res<Vec<Task>> {
    state.db.list_tasks(project_id).map_err(err)
}

#[tauri::command]
pub async fn create_task(
    state: State<'_, AppState>,
    project_id: Id,
    title: String,
    description: Option<String>,
) -> Res<Task> {
    state
        .db
        .create_task_with(project_id, &title, description.as_deref().unwrap_or(""))
        .map_err(err)
}

#[tauri::command]
pub async fn update_task(
    app: AppHandle,
    state: State<'_, AppState>,
    id: Id,
    title: String,
    description: String,
) -> Res<Task> {
    let t = state
        .db
        .update_task_text(id, &title, &description)
        .map_err(err)?;
    emit_task(&app, &t);
    Ok(t)
}

/// Pick the agent for the task's stage sessions (None = project default).
#[tauri::command]
pub async fn set_task_agent(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: Id,
    agent: Option<AgentKind>,
) -> Res<Task> {
    let t = state.db.set_task_agent(task_id, agent).map_err(err)?;
    emit_task(&app, &t);
    Ok(t)
}

#[tauri::command]
pub async fn set_project_agent(
    state: State<'_, AppState>,
    project_id: Id,
    agent: AgentKind,
) -> Res<Project> {
    state.db.set_project_agent(project_id, agent).map_err(err)
}

/// Move a task on the board (`index` 0 = top of the column). Entering a
/// new stage starts that stage's skill session.
#[tauri::command]
pub async fn move_task(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: Id,
    stage: String,
    index: usize,
) -> Res<MoveResult> {
    let (task, from) = state.db.move_task(task_id, &stage, index).map_err(err)?;
    emit_task(&app, &task);
    if from == task.stage {
        return Ok(MoveResult {
            task,
            session: None,
            note: None,
        });
    }
    rpi::on_stage_entered(&app, task).await
}

/// Start the current stage's skill session again (after a failure, say).
#[tauri::command]
pub async fn start_stage_session(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: Id,
) -> Res<MoveResult> {
    let task = state.db.get_task(task_id).map_err(err)?;
    rpi::on_stage_entered(&app, task).await
}

// ---- task docs ----

#[tauri::command]
pub async fn list_artifacts(state: State<'_, AppState>, task_id: Id) -> Res<Vec<Artifact>> {
    rpi::refresh_artifacts(&state.db, &state.artifacts, task_id)?;
    state.db.list_artifacts(task_id).map_err(err)
}

#[tauri::command]
pub async fn read_artifact(state: State<'_, AppState>, task_id: Id, path: String) -> Res<String> {
    state.artifacts.read(task_id, &path)
}

/// Start a session that revises a document with the user's feedback
/// (`/rpi:iterate-research`, `/rpi:iterate-plan`, ...).
#[tauri::command]
pub async fn iterate_artifact(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: Id,
    path: String,
    feedback: String,
) -> Res<Session> {
    if feedback.trim().is_empty() {
        return Err("Write some feedback first.".into());
    }
    let task = state.db.get_task(task_id).map_err(err)?;
    let kind = hl_core::artifacts::kind_of(&path);
    let skill = rpi::iterate_skill(kind).ok_or_else(|| format!("No iterate skill for {path}."))?;
    let args = format!(
        "{}/{}/{path}\n\nFeedback:\n{}",
        hl_core::artifacts::LINK_DIR,
        task.slug,
        feedback.trim()
    );
    rpi::start_skill_session(&app, task_id, skill, &args).await
}

/// Delete a task. Its worktree folder is removed first (the branch is
/// kept); `force` discards uncommitted changes in it.
#[tauri::command]
pub async fn delete_task(state: State<'_, AppState>, id: Id, force: Option<bool>) -> Res<()> {
    let task = state.db.get_task(id).map_err(err)?;
    stop_task_sessions(&state, id).await;
    remove_worktree(&state.db, &task, force.unwrap_or(false)).await?;
    state.db.delete_task(id).map_err(err)?;
    state.artifacts.remove(id);
    // Stop agent processes whose session row is gone.
    state.rt.prune();
    Ok(())
}

// ---- worktrees ----

fn emit_task(app: &AppHandle, t: &Task) {
    if let Err(e) = app.emit(TASK_UPDATED, t) {
        eprintln!("emit failed: {e}");
    }
}

fn busy(t: &Task) -> bool {
    matches!(
        t.worktree_status.as_deref(),
        Some(ws::CREATING | ws::SETTING_UP)
    )
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)
}

/// Create the task's worktree, then run the setup command, in the
/// background. Returns at once with status `creating`; progress arrives as
/// `task-updated` events. A task that had a worktree before (restored from
/// the archive) gets its old branch back.
#[tauri::command]
pub async fn create_worktree(app: AppHandle, state: State<'_, AppState>, task_id: Id) -> Res<Task> {
    start_worktree(&app, &state, task_id).await
}

pub(crate) async fn start_worktree(app: &AppHandle, state: &AppState, task_id: Id) -> Res<Task> {
    let app = app.clone();
    let task = state.db.get_task(task_id).map_err(err)?;
    if task.worktree_path.is_some() || busy(&task) {
        return Err("This task already has a worktree.".into());
    }
    let repo = PathBuf::from(
        state
            .db
            .get_project(task.project_id)
            .map_err(err)?
            .repo_path,
    );
    let cfg = {
        let repo = repo.clone();
        blocking(move || WorkspaceConfig::load(&repo))
            .await?
            .map_err(err)?
    };
    if cfg.disabled {
        return Err(
            "Worktrees are turned off for this project (\"disabled\": true in .humanlayer/workspace.json)."
                .into(),
        );
    }
    let t = state
        .db
        .set_worktree_status(task_id, ws::CREATING, Some(""))
        .map_err(err)?;
    emit_task(&app, &t);
    let db = state.db.clone();
    std::thread::spawn(move || worktree_job(&app, &db, &task, &repo, &cfg));
    Ok(t)
}

fn worktree_job(
    app: &AppHandle,
    db: &Db,
    task: &Task,
    repo: &std::path::Path,
    cfg: &WorkspaceConfig,
) {
    let (env, _) = hl_core::shell_env::get();
    let slug = task.slug.clone();
    let which = match &task.branch {
        Some(b) => Branch::Existing {
            branch: b,
            slug: &slug,
        },
        None => Branch::New { slug: &slug },
    };
    let save = |status: &str, log: &str| match db.set_worktree_status(task.id, status, Some(log)) {
        Ok(t) => emit_task(app, &t),
        Err(e) => eprintln!("task {}: {e}", task.id),
    };
    let c = match worktree::create(repo, cfg, which, &env) {
        Ok(c) => c,
        Err(e) => {
            save(ws::FAILED, &e.to_string());
            return rpi::on_worktree_done(app, task.id, false);
        }
    };
    // A restored branch keeps its original base.
    let base_ref = task
        .base_ref
        .clone()
        .filter(|_| c.base_ref.is_empty())
        .unwrap_or(c.base_ref);
    let base_commit = task.base_commit.clone().unwrap_or(c.base_commit);
    if let Err(e) = db.set_worktree(
        task.id,
        &c.path.to_string_lossy(),
        &c.branch,
        &base_ref,
        &base_commit,
    ) {
        save(ws::FAILED, &format!("{}\n{e}", c.log));
        return rpi::on_worktree_done(app, task.id, false);
    }
    run_setup_step(app, db, task.id, &c.path, &cfg.setup_command, c.log, &env);
    // A failed setup still leaves a usable worktree; the session can start.
    rpi::on_worktree_done(app, task.id, true);
}

fn run_setup_step(
    app: &AppHandle,
    db: &Db,
    task_id: Id,
    wt: &std::path::Path,
    command: &str,
    mut log: String,
    env: &hl_git::Env,
) {
    let save = |status: &str, log: &str| match db.set_worktree_status(task_id, status, Some(log)) {
        Ok(t) => emit_task(app, &t),
        Err(e) => eprintln!("task {task_id}: {e}"),
    };
    if command.trim().is_empty() {
        log.push_str(
            "No setup command (set repos[0].setupCommand in .humanlayer/workspace.json).\n",
        );
        return save(ws::READY, &log);
    }
    log.push_str(&format!("$ {command}\n"));
    save(ws::SETTING_UP, &log);
    let (ok, out) = worktree::run_setup(wt, command, env);
    log.push_str(&out);
    save(if ok { ws::READY } else { ws::SETUP_FAILED }, &log);
}

/// Run the setup command again (after it failed, or to refresh).
#[tauri::command]
pub async fn retry_setup(app: AppHandle, state: State<'_, AppState>, task_id: Id) -> Res<Task> {
    let task = state.db.get_task(task_id).map_err(err)?;
    let Some(path) = task.worktree_path.clone().filter(|_| !busy(&task)) else {
        return Err("No finished worktree to set up.".into());
    };
    let repo = PathBuf::from(
        state
            .db
            .get_project(task.project_id)
            .map_err(err)?
            .repo_path,
    );
    let cfg = blocking(move || WorkspaceConfig::load(&repo))
        .await?
        .map_err(err)?;
    let t = state
        .db
        .set_worktree_status(task_id, ws::SETTING_UP, None)
        .map_err(err)?;
    emit_task(&app, &t);
    let db = state.db.clone();
    std::thread::spawn(move || {
        let (env, _) = hl_core::shell_env::get();
        run_setup_step(
            &app,
            &db,
            task_id,
            &PathBuf::from(path),
            &cfg.setup_command,
            String::new(),
            &env,
        );
    });
    Ok(t)
}

/// Uncommitted files in the task's worktree (empty when there is none).
#[tauri::command]
pub async fn worktree_dirty_files(state: State<'_, AppState>, task_id: Id) -> Res<Vec<String>> {
    let task = state.db.get_task(task_id).map_err(err)?;
    let Some(path) = task.worktree_path.map(PathBuf::from).filter(|p| p.is_dir()) else {
        return Ok(vec![]);
    };
    blocking(move || worktree::dirty_files(&path, &hl_core::shell_env::get().0))
        .await?
        .map_err(err)
}

#[tauri::command]
pub async fn get_diff(state: State<'_, AppState>, task_id: Id) -> Res<DiffSummary> {
    let (wt, base_ref, base_commit) = diff_target(&state.db, task_id)?;
    blocking(move || {
        hl_git::diff::summary(&wt, &base_ref, &base_commit, &hl_core::shell_env::get().0)
    })
    .await?
    .map_err(err)
}

#[tauri::command]
pub async fn get_file_diff(
    state: State<'_, AppState>,
    task_id: Id,
    path: String,
    old_path: Option<String>,
) -> Res<String> {
    let (wt, base_ref, base_commit) = diff_target(&state.db, task_id)?;
    blocking(move || {
        hl_git::diff::file_patch(
            &wt,
            &base_ref,
            &base_commit,
            &path,
            old_path.as_deref(),
            &hl_core::shell_env::get().0,
        )
    })
    .await?
    .map_err(err)
}

fn diff_target(db: &Db, task_id: Id) -> Res<(PathBuf, String, String)> {
    let task = db.get_task(task_id).map_err(err)?;
    let wt = task
        .worktree_path
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .ok_or("This task has no worktree.")?;
    Ok((
        wt,
        task.base_ref.unwrap_or_default(),
        task.base_commit.unwrap_or_default(),
    ))
}

/// Stop the task's agent processes and wait (up to 5 s) for them to exit,
/// so nothing writes into a worktree that is being removed.
async fn stop_task_sessions(state: &AppState, task_id: Id) {
    let ids: Vec<Id> = state
        .db
        .list_sessions(task_id)
        .map(|v| v.into_iter().map(|s| s.id).collect())
        .unwrap_or_default();
    for &id in &ids {
        let _ = state.rt.stop(id);
    }
    for _ in 0..250 {
        if !ids.iter().any(|&id| state.rt.is_live(id)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn remove_worktree(db: &Arc<Db>, task: &Task, force: bool) -> Res<()> {
    if busy(task) {
        return Err("Wait for the worktree setup to finish first.".into());
    }
    let Some(path) = task.worktree_path.clone() else {
        return Ok(());
    };
    let repo = PathBuf::from(db.get_project(task.project_id).map_err(err)?.repo_path);
    blocking(move || {
        worktree::remove(
            &repo,
            &PathBuf::from(path),
            force,
            &hl_core::shell_env::get().0,
        )
    })
    .await?
    .map_err(err)?;
    db.clear_worktree(task.id).map_err(err)?;
    Ok(())
}

/// Archive a task: stop its sessions and remove its worktree folder (the
/// branch is kept). `force` discards uncommitted changes in the worktree.
#[tauri::command]
pub async fn archive_task(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: Id,
    force: bool,
) -> Res<Task> {
    let task = state.db.get_task(task_id).map_err(err)?;
    stop_task_sessions(&state, task_id).await;
    remove_worktree(&state.db, &task, force).await?;
    let t = state.db.set_task_archived(task_id, true).map_err(err)?;
    emit_task(&app, &t);
    Ok(t)
}

#[tauri::command]
pub async fn unarchive_task(app: AppHandle, state: State<'_, AppState>, task_id: Id) -> Res<Task> {
    let t = state.db.set_task_archived(task_id, false).map_err(err)?;
    emit_task(&app, &t);
    Ok(t)
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
    state.db.delete_session(id).map_err(err)?;
    // Stop agent processes whose session row is gone.
    state.rt.prune();
    Ok(())
}

// ---- agent sessions ----

#[tauri::command]
pub async fn get_transcript(state: State<'_, AppState>, session_id: Id) -> Res<Transcript> {
    state.rt.transcript(session_id)
}

#[tauri::command]
pub async fn get_usage(state: State<'_, AppState>, session_id: Id) -> Res<SessionUsage> {
    state.rt.usage(session_id)
}

#[tauri::command]
pub async fn send_message(state: State<'_, AppState>, session_id: Id, text: String) -> Res<()> {
    state.rt.send(session_id, &text).await
}

#[tauri::command]
pub async fn interrupt_session(state: State<'_, AppState>, session_id: Id) -> Res<()> {
    state.rt.interrupt(session_id)
}

#[tauri::command]
pub async fn stop_session(state: State<'_, AppState>, session_id: Id) -> Res<()> {
    state.rt.stop(session_id)
}

#[tauri::command]
pub async fn respond_approval(
    state: State<'_, AppState>,
    session_id: Id,
    request_id: String,
    decision: Decision,
) -> Res<()> {
    state.rt.respond_approval(session_id, &request_id, decision)
}
