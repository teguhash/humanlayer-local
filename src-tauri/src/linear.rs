//! Linear in the app: import a ticket as a task, keep linked tasks in sync
//! (polled every 60 s), move tickets forward when tasks change stage, and
//! post task docs to the ticket as comments.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use hl_core::model::{Id, LinearTicket, Task};
use hl_linear::{Client, Issue, status, ticket};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::{ARTIFACTS_UPDATED, AppState, NOTICE, TASK_UPDATED, rpi};

pub const POLL_EVERY: Duration = Duration::from_secs(60);
/// Issues per GraphQL request when polling.
const BATCH: usize = 25;
pub const IMAGES_DIR: &str = "images";

/// What Settings shows about the connection. Never includes the key.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LinearStatus {
    pub has_key: bool,
    /// "Name (Organization)" once the key was checked.
    pub account: Option<String>,
    pub last_poll: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Default)]
pub struct LinearState {
    pub status: Mutex<LinearStatus>,
}

/// A message for the app's notice bar.
#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    pub level: &'static str,
    pub text: String,
}

pub fn notice(app: &AppHandle, level: &'static str, text: String) {
    let _ = app.emit(NOTICE, Notice { level, text });
}

pub fn client(state: &AppState) -> Result<Client, String> {
    match state.secrets.linear_key()? {
        Some(k) => Ok(Client::new(&k)),
        None => Err("Add a Linear API key in Settings first.".into()),
    }
}

fn ticket_of(i: &Issue) -> LinearTicket {
    LinearTicket {
        identifier: i.identifier.clone(),
        title: i.title.clone(),
        description: i.description.clone(),
        url: i.url.clone(),
        state: i.state.name.clone(),
        updated_at: i.updated_at.clone(),
    }
}

fn now() -> String {
    // Seconds since the epoch; the UI formats it.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

/// Write `ticket.md` and the ticket's images into the task's docs folder.
/// An image that fails to download keeps its Linear link. Returns the
/// number of images that failed.
async fn write_ticket(
    state: &AppState,
    c: &Client,
    task: &Task,
    issue: &Issue,
) -> Result<usize, String> {
    let dir = state.artifacts.ensure(task).map_err(|e| e.to_string())?;
    let mut local = HashMap::new();
    let mut failed = 0;
    for (n, url) in ticket::issue_images(issue).iter().enumerate() {
        match c.download(url).await {
            Ok((bytes, ctype)) => {
                let name = ticket::image_file_name(&issue.identifier, n + 1, url, ctype.as_deref());
                let rel = format!("{IMAGES_DIR}/{name}");
                let path = dir.join(&rel);
                std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
                std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
                local.insert(url.clone(), rel);
            }
            Err(e) => {
                eprintln!("{}: image {} failed: {e}", issue.identifier, n + 1);
                failed += 1;
            }
        }
    }
    std::fs::write(dir.join("ticket.md"), ticket::ticket_md(issue, &local))
        .map_err(|e| e.to_string())?;
    Ok(failed)
}

fn emit_task(app: &AppHandle, t: &Task) {
    let _ = app.emit(TASK_UPDATED, t);
}

fn refresh_docs(app: &AppHandle, state: &AppState, task_id: Id) {
    if rpi::refresh_artifacts(&state.db, &state.artifacts, task_id).unwrap_or(false) {
        let _ = app.emit(ARTIFACTS_UPDATED, task_id);
    }
}

/// Import a ticket (ID or URL) into a project's backlog.
pub async fn import(
    app: &AppHandle,
    project_id: Id,
    reference: &str,
) -> Result<(Task, Option<String>), String> {
    let state = app.state::<AppState>();
    let id = ticket::parse_ref(reference).map_err(|e| e.to_string())?;
    let c = client(&state)?;
    let issue = c.issue(&id).await.map_err(|e| e.to_string())?;
    let task = state
        .db
        .create_linear_task(project_id, &ticket_of(&issue))
        .map_err(|e| e.to_string())?;
    let note = match write_ticket(&state, &c, &task, &issue).await {
        Ok(0) => None,
        Ok(n) => Some(format!(
            "{n} image(s) from {} could not be downloaded; ticket.md keeps their links.",
            issue.identifier
        )),
        Err(e) => Some(format!("Imported, but ticket.md could not be written: {e}")),
    };
    refresh_docs(app, &state, task.id);
    Ok((task, note))
}

/// Check every linked task once. Errors are recorded for Settings.
pub async fn poll_once(app: &AppHandle) {
    let state = app.state::<AppState>();
    let c = match state.secrets.linear_key() {
        Ok(Some(k)) => Client::new(&k),
        Ok(None) => return,
        Err(e) => return record(&state, Some(e)),
    };
    let tasks = match state.db.linked_tasks() {
        Ok(t) => t,
        Err(e) => return record(&state, Some(e.to_string())),
    };
    let mut missing = vec![];
    for chunk in tasks.chunks(BATCH) {
        let ids: Vec<String> = chunk
            .iter()
            .filter_map(|t| t.linear_issue_id.clone())
            .collect();
        let issues = match c.issues(&ids).await {
            Ok(i) => i,
            Err(e) => return record(&state, Some(e.to_string())),
        };
        for (task, issue) in chunk.iter().zip(issues) {
            let Some(issue) = issue else {
                missing.push(task.linear_issue_id.clone().unwrap_or_default());
                continue;
            };
            if task.linear_updated_at.as_deref() == Some(issue.updated_at.as_str()) {
                continue;
            }
            match state.db.sync_linear_task(task.id, &ticket_of(&issue)) {
                Ok(t) => {
                    if let Err(e) = write_ticket(&state, &c, &t, &issue).await {
                        eprintln!("{}: ticket.md: {e}", issue.identifier);
                    }
                    emit_task(app, &t);
                    refresh_docs(app, &state, t.id);
                }
                Err(e) => eprintln!("{}: sync failed: {e}", issue.identifier),
            }
        }
    }
    record(
        &state,
        (!missing.is_empty()).then(|| format!("Not found in Linear: {}", missing.join(", "))),
    );
}

fn record(state: &AppState, error: Option<String>) {
    let mut s = state.linear.status.lock().unwrap();
    s.last_poll = Some(now());
    s.last_error = error;
}

/// Poll forever. The first check runs shortly after start.
pub fn start_polling(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        loop {
            poll_once(&app).await;
            tokio::time::sleep(POLL_EVERY).await;
        }
    });
}

/// After a stage move: move the ticket to the stage's state, forward only.
/// Runs in the background; problems show in the notice bar.
pub fn on_stage_changed(app: &AppHandle, task: &Task) {
    let Some(id) = task.linear_issue_id.clone() else {
        return;
    };
    let (app, task_id, stage) = (app.clone(), task.id, task.stage.clone());
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let c = match client(&state) {
            Ok(c) => c,
            Err(_) => return, // no key: nothing to sync
        };
        let res = async {
            let issue = c.issue(&id).await.map_err(|e| e.to_string())?;
            let Some(target) = status::advance(&stage, &issue.state, &issue.states) else {
                return Ok(None);
            };
            c.set_state(&issue.id, &target.id)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<_, String>(Some(target.name.clone()))
        }
        .await;
        match res {
            Ok(Some(name)) => {
                if let Ok(t) = state.db.set_linear_state(task_id, &name) {
                    emit_task(&app, &t);
                }
                notice(&app, "info", format!("Moved {id} to “{name}” in Linear."));
            }
            Ok(None) => {}
            Err(e) => notice(
                &app,
                "error",
                format!("Could not update {id} in Linear: {e}"),
            ),
        }
    });
}

/// The comment body for a posted doc.
pub fn doc_comment(path: &str, content: &str) -> String {
    format!(
        "**{path}** (from HumanLayer Local)\n\n+++ [{path}]\n\n{}\n\n+++",
        content.trim()
    )
}

/// Post a doc to the task's ticket as a comment; posting again updates it.
/// Returns "posted" or "updated".
pub async fn post_doc(state: &AppState, task_id: Id, path: &str) -> Result<&'static str, String> {
    let task = state.db.get_task(task_id).map_err(|e| e.to_string())?;
    let id = task
        .linear_issue_id
        .clone()
        .ok_or("This task is not linked to a Linear ticket.")?;
    let content = state.artifacts.read(task_id, path)?;
    let body = doc_comment(path, &content);
    let c = client(state)?;
    if let Some(cid) = state
        .db
        .linear_post(task_id, path)
        .map_err(|e| e.to_string())?
        && c.update_comment(&cid, &body)
            .await
            .map_err(|e| e.to_string())?
    {
        return Ok("updated");
    }
    let issue = c.issue(&id).await.map_err(|e| e.to_string())?;
    let cid = c
        .create_comment(&issue.id, &body)
        .await
        .map_err(|e| e.to_string())?;
    state
        .db
        .set_linear_post(task_id, path, &cid)
        .map_err(|e| e.to_string())?;
    Ok("posted")
}

#[cfg(test)]
mod tests {
    #[test]
    fn comment_body() {
        let b = super::doc_comment("01-research-x.md", "\n# R\n");
        assert_eq!(
            b,
            "**01-research-x.md** (from HumanLayer Local)\n\n+++ [01-research-x.md]\n\n# R\n\n+++"
        );
    }
}
