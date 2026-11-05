//! The RPI flow: the bundled skills plugin, the per-session prompt that
//! points the agent at the task's docs folder, stage moves that start
//! sessions, and the docs-folder watcher.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hl_core::Db;
use hl_core::artifacts::{ArtifactStore, LINK_DIR};
use hl_core::model::{AgentKind, Id, Session, Task, stage};
use tauri::{AppHandle, Emitter, Manager};

use crate::{ARTIFACTS_UPDATED, AppState, SESSION_CREATED};

/// The bundled plugin folder (skills + agents). Packaged builds read it from
/// the app's resources; dev builds fall back to the source tree.
pub fn plugin_dir(app: &AppHandle) -> Option<PathBuf> {
    let candidates = [
        app.path().resource_dir().ok().map(|d| d.join("rpi")),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/rpi")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|d| d.join(".claude-plugin/plugin.json").is_file())
}

/// Where Codex finds the bundled skills: a link in the session folder
/// (Codex reads project skills from `.agents/skills`, following links).
pub const CODEX_SKILLS_LINK: &str = ".agents/skills/humanlayer-rpi";

/// Builds the extra CLI arguments for each session start: load the skills,
/// link the task's docs folder into the session folder, and say where it is.
pub fn prepare_fn(
    db: Arc<Db>,
    store: Arc<ArtifactStore>,
    plugin: Option<PathBuf>,
) -> Arc<hl_agents::PrepareFn> {
    Arc::new(move |session: &Session, cwd: &str| {
        let task = db.get_task(session.task_id).map_err(|e| e.to_string())?;
        let cwd = Path::new(cwd);
        let link = store.link_into(&task, cwd)?;
        let (env, _) = hl_core::shell_env::get();
        let ignore = |rel: &str| {
            let top = rel.split('/').next().unwrap_or(rel);
            // Ignore the whole docs folder, but only our link under .agents
            // (a repo may commit its own skills there).
            let pattern = if top == LINK_DIR {
                format!("/{LINK_DIR}/")
            } else {
                format!("/{rel}")
            };
            if let Err(e) = hl_git::ensure_ignored(cwd, rel, &pattern, &env) {
                eprintln!("could not add git ignore rule in {}: {e}", cwd.display());
            }
        };
        ignore(&format!("{LINK_DIR}/{}", task.slug));
        let docs_dir = store.dir(task.id).to_string_lossy().into_owned();
        let prompt = system_prompt(&task, &link, session.agent_kind);
        let mut args = vec![];
        match session.agent_kind {
            AgentKind::Claude => {
                if let Some(p) = &plugin {
                    let p = p.to_string_lossy().into_owned();
                    args.push("--plugin-dir".into());
                    args.push(p.clone());
                    // Skills read their own reference templates; don't prompt for that.
                    args.push("--add-dir".into());
                    args.push(p);
                }
                // Writes go through the link into app data; allow that folder.
                args.push("--add-dir".into());
                args.push(docs_dir);
                args.push("--append-system-prompt".into());
                args.push(prompt);
            }
            AgentKind::Codex => {
                if let Some(p) = &plugin {
                    hl_core::artifacts::ensure_link(
                        &p.join("skills"),
                        &cwd.join(CODEX_SKILLS_LINK),
                    )?;
                    ignore(CODEX_SKILLS_LINK);
                }
                // The sandbox allows writes in the session folder only; the
                // docs folder lives in app data, behind the link.
                args.extend(hl_agents::codex::config_strings(
                    "sandbox_workspace_write.writable_roots",
                    &[docs_dir],
                ));
                args.extend(hl_agents::codex::config_string(
                    "developer_instructions",
                    &prompt,
                ));
            }
        }
        Ok(args)
    })
}

fn system_prompt(task: &Task, link: &Path, agent: AgentKind) -> String {
    let dir = link.display();
    let (skill_word, list_tools) = match agent {
        AgentKind::Claude => ("an rpi: skill", "not with List, Glob or Grep, and "),
        AgentKind::Codex => ("a skill such as $create-plan", ""),
    };
    format!(
        "<task_information>
You are working on the task \"{title}\" (slug: {slug}) in HumanLayer Local.
</task_information>

<artifacts_directory_information>
Your task artifacts directory is: {dir}

If the user requests {skill_word} and doesn't mention a file, check here first.
If the user asks you to continue work on a task, design discussion, plan or other task document and doesn't mention a file, check here first.

This directory is a symlink. List it with `ls -La`, {list_tools}not with `ls -la` (that shows the link target, which is irrelevant).
Read and write files in it as normal files, through the path above. Do not write to the link's target directly.
The app lists the Markdown files in this directory on the task and renders them, so save task documents (research, plans, outlines) here.
When writing Markdown documents, do not hard-wrap lines; the viewer wraps them.
</artifacts_directory_information>",
        title = task.title,
        slug = task.slug,
    )
}

/// How to invoke a bundled skill: a plugin slash command for Claude, a
/// `$name` mention for Codex.
pub fn skill_prompt(agent: AgentKind, skill: &str, args: &str) -> String {
    let head = match agent {
        AgentKind::Claude => format!("/rpi:{skill}"),
        AgentKind::Codex => format!("${skill}"),
    };
    if args.is_empty() {
        head
    } else {
        format!("{head} {args}")
    }
}

/// The skill arguments of a stage session's first message.
pub fn stage_args(task: &Task, skill: &str) -> String {
    if skill == "create-research" {
        let mut p = task.title.clone();
        if !task.description.trim().is_empty() {
            p.push_str(&format!("\n\n{}", task.description.trim()));
        }
        if let Some(id) = &task.linear_issue_id {
            p.push_str(&format!(
                "\n\nLinear ticket {id}: read ticket.md in the task artifacts directory (its images are in images/ there)."
            ));
        }
        p
    } else {
        String::new()
    }
}

/// Create a session for `skill` on the task, with the task's agent, and
/// send the skill invocation with `args`.
pub async fn start_skill_session(
    app: &AppHandle,
    task_id: Id,
    skill: &str,
    args: &str,
) -> Result<Session, String> {
    let state = app.state::<AppState>();
    let task = state.db.get_task(task_id).map_err(|e| e.to_string())?;
    let agent = state.db.agent_for(&task).map_err(|e| e.to_string())?;
    let s = state
        .db
        .create_session_with(task_id, agent, Some(skill))
        .map_err(|e| e.to_string())?;
    let _ = app.emit(SESSION_CREATED, &s);
    state
        .rt
        .send(s.id, &skill_prompt(agent, skill, args))
        .await?;
    Ok(s)
}

/// What a stage move did, for the UI.
#[derive(serde::Serialize)]
pub struct MoveResult {
    pub task: Task,
    /// The session that was started, if any.
    pub session: Option<Session>,
    /// Shown to the user (e.g. "waiting for the worktree").
    pub note: Option<String>,
}

/// Start the stage's skill after a move. Implement runs in a worktree: when
/// the task has none, one is created first and the session starts when its
/// setup finishes (see `on_worktree_done`).
pub async fn on_stage_entered(app: &AppHandle, task: Task) -> Result<MoveResult, String> {
    let Some(skill) = stage::skill(&task.stage) else {
        return Ok(MoveResult {
            task,
            session: None,
            note: None,
        });
    };
    if task.archived_at.is_some() {
        return Ok(MoveResult {
            task,
            session: None,
            note: None,
        });
    }
    if task.stage == stage::IMPLEMENT && task.worktree_path.is_none() {
        let state = app.state::<AppState>();
        match crate::commands::start_worktree(app, &state, task.id).await {
            Ok(t) => {
                state
                    .pending_stage
                    .lock()
                    .unwrap()
                    .insert(task.id, task.stage.clone());
                return Ok(MoveResult {
                    task: t,
                    session: None,
                    note: Some(
                        "Creating a worktree first; the implement session starts when it is ready."
                            .into(),
                    ),
                });
            }
            Err(e) if e.contains("turned off") => {} // run in the repo
            Err(e) => return Err(e),
        }
    }
    let s = start_skill_session(app, task.id, skill, &stage_args(&task, skill)).await?;
    Ok(MoveResult {
        task,
        session: Some(s),
        note: None,
    })
}

/// Called when a worktree job ends. Starts a stage session that was waiting
/// for it, unless creation failed.
pub fn on_worktree_done(app: &AppHandle, task_id: Id, ok: bool) {
    let state = app.state::<AppState>();
    let Some(st) = state.pending_stage.lock().unwrap().remove(&task_id) else {
        return;
    };
    if !ok {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let Ok(task) = state.db.get_task(task_id) else {
            return;
        };
        if task.stage != st || task.archived_at.is_some() {
            return; // moved on meanwhile
        }
        if let Some(skill) = stage::skill(&st)
            && let Err(e) =
                start_skill_session(&app, task_id, skill, &stage_args(&task, skill)).await
        {
            eprintln!("task {task_id}: could not start {skill}: {e}");
        }
    });
}

/// Watch the docs root; rescan a task's folder after changes settle and
/// emit `artifacts-updated` with the task id when its list changed.
pub fn watch_artifacts(
    app: AppHandle,
    db: Arc<Db>,
    store: Arc<ArtifactStore>,
) -> notify::Result<()> {
    use notify::{RecursiveMode, Watcher};
    let root = store.root().to_path_buf();
    std::fs::create_dir_all(&root).map_err(notify::Error::io)?;
    let root = root.canonicalize().map_err(notify::Error::io)?;
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(tx)?;
    watcher.watch(&root, RecursiveMode::Recursive)?;
    std::thread::spawn(move || {
        let _watcher = watcher; // keep alive
        let task_of = |p: &Path| -> Option<Id> {
            p.strip_prefix(&root)
                .ok()?
                .components()
                .next()?
                .as_os_str()
                .to_str()?
                .parse()
                .ok()
        };
        while let Ok(first) = rx.recv() {
            let mut ids: HashMap<Id, ()> = HashMap::new();
            let mut add = |ev: notify::Result<notify::Event>| {
                if let Ok(ev) = ev {
                    for p in &ev.paths {
                        if let Some(id) = task_of(p) {
                            ids.insert(id, ());
                        }
                    }
                }
            };
            add(first);
            // Debounce: collect until 300 ms pass without events.
            while let Ok(ev) = rx.recv_timeout(Duration::from_millis(300)) {
                add(ev);
            }
            for id in ids.into_keys() {
                if refresh_artifacts(&db, &store, id).unwrap_or(false) {
                    let _ = app.emit(ARTIFACTS_UPDATED, id);
                }
            }
        }
    });
    Ok(())
}

/// Rescan one task's folder into the DB. Ok(true) if the list changed.
pub fn refresh_artifacts(db: &Db, store: &ArtifactStore, task_id: Id) -> Result<bool, String> {
    if db.get_task(task_id).is_err() {
        return Ok(false);
    }
    db.replace_artifacts(task_id, &store.scan(task_id))
        .map_err(|e| e.to_string())
}

/// The iterate skill for a document kind.
pub fn iterate_skill(kind: &str) -> Option<&'static str> {
    match kind {
        "research" => Some("iterate-research"),
        "plan" => Some("iterate-plan"),
        "outline" => Some("iterate-structure-outline"),
        "research_questions" => Some("iterate-research-questions"),
        "design" => Some("iterate-design-discussion"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts() {
        let mut t = Task {
            id: 1,
            project_id: 1,
            title: "Add login".into(),
            description: String::new(),
            slug: "add-login".into(),
            stage: "research".into(),
            position: 0,
            worktree_path: None,
            linear_issue_id: None,
            github_issue: None,
            created_at: String::new(),
            updated_at: String::new(),
            branch: None,
            base_ref: None,
            base_commit: None,
            worktree_status: None,
            worktree_log: None,
            archived_at: None,
            agent: None,
            linear_url: None,
            linear_state: None,
            linear_updated_at: None,
            github_issue_url: None,
        };
        let claude = |skill| skill_prompt(AgentKind::Claude, skill, &stage_args(&t, skill));
        assert_eq!(claude("create-research"), "/rpi:create-research Add login");
        assert_eq!(claude("create-plan"), "/rpi:create-plan");
        t.description = " via magic links ".into();
        assert_eq!(
            skill_prompt(
                AgentKind::Codex,
                "create-research",
                &stage_args(&t, "create-research")
            ),
            "$create-research Add login\n\nvia magic links"
        );
        let link = Path::new("/r/.humanlayer/tasks/add-login");
        let sp = system_prompt(&t, link, AgentKind::Claude);
        assert!(sp.contains("Your task artifacts directory is: /r/.humanlayer/tasks/add-login"));
        assert!(sp.contains("an rpi: skill"));
        let sp = system_prompt(&t, link, AgentKind::Codex);
        assert!(sp.contains("$create-plan") && !sp.contains("Glob"));
        assert_eq!(iterate_skill("research"), Some("iterate-research"));
        assert_eq!(iterate_skill("task"), None);
    }
}
