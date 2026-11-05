//! GitHub in the app, all through `gh`: sign-in status, issue import,
//! opening a PR from the task's worktree, polling open PRs, merging.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hl_core::artifacts::ArtifactStore;
use hl_core::model::{Id, PullRequest, Task, stage};
use hl_git::{Env, github};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::{AppState, PR_UPDATED, TASK_UPDATED, linear, rpi, secrets};

pub const POLL_EVERY: Duration = Duration::from_secs(45);

/// The shell env plus saved credentials (GH_TOKEN when a token is saved).
pub fn env(state: &AppState) -> Env {
    let (mut env, _) = hl_core::shell_env::get();
    env.extend(
        state
            .secrets
            .env_vars()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v)),
    );
    env
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

fn err(e: hl_git::Error) -> String {
    e.to_string()
}

/// What Settings shows. Never includes a token.
#[derive(Debug, Clone, Serialize)]
pub struct GithubStatus {
    pub gh_installed: bool,
    /// Signed-in login, and where gh got the credential ("keyring", "GH_TOKEN", ...).
    pub login: Option<String>,
    pub source: Option<String>,
    /// A token is saved in the app (used instead of gh's own login).
    pub has_token: bool,
    pub error: Option<String>,
}

pub async fn status(state: &AppState) -> GithubStatus {
    let has_token = matches!(state.secrets.get(secrets::GITHUB_TOKEN), Ok(Some(_)));
    let env = env(state);
    let gh_installed = hl_core::shell_env::which("gh", &env).is_some();
    let res = if gh_installed {
        blocking(move || github::auth_status(&env).map_err(err)).await
    } else {
        Ok(None)
    };
    let (login, source, error) = match res {
        Ok(Some((l, s))) => (Some(l), Some(s), None),
        Ok(None) => (None, None, None),
        Err(e) => (None, None, Some(e)),
    };
    GithubStatus {
        gh_installed,
        login,
        source,
        has_token,
        error,
    }
}

pub async fn set_token(state: &AppState, token: &str) -> Result<String, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("Paste a GitHub token first.".into());
    }
    let (env, _) = hl_core::shell_env::get();
    let t = token.clone();
    let login = blocking(move || github::token_login(&t, &env).map_err(err)).await?;
    state.secrets.set(secrets::GITHUB_TOKEN, &token)?;
    Ok(login)
}

fn project_dir(state: &AppState, task: &Task) -> Result<PathBuf, String> {
    let p = state
        .db
        .get_project(task.project_id)
        .map_err(|e| e.to_string())?;
    Ok(PathBuf::from(p.repo_path))
}

/// Import an issue (`12`, `#12`, `owner/repo#12` or URL) into the backlog.
pub async fn import_issue(
    app: &AppHandle,
    project_id: Id,
    reference: &str,
) -> Result<Task, String> {
    let state = app.state::<AppState>();
    let (repo, number) = github::parse_issue_ref(reference).map_err(err)?;
    let project = state
        .db
        .get_project(project_id)
        .map_err(|e| e.to_string())?;
    let env = env(&state);
    let dir = PathBuf::from(&project.repo_path);
    let issue = blocking(move || {
        let repo = match repo {
            Some(r) => r,
            None => github::repo_name(&dir, &env).map_err(err)?,
        };
        github::issue(&dir, &repo, number, &env).map_err(err)
    })
    .await?;
    let r = format!("{}#{}", issue.repo, issue.number);
    let task = state
        .db
        .create_github_task(
            project_id,
            &r,
            issue.number,
            &issue.title,
            &issue.body,
            &issue.url,
        )
        .map_err(|e| e.to_string())?;
    let dir = state.artifacts.ensure(&task).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("github-issue.md"), github::issue_md(&issue))
        .map_err(|e| e.to_string())?;
    let _ = rpi::refresh_artifacts(&state.db, &state.artifacts, task.id);
    Ok(task)
}

fn to_row(p: &github::Pr) -> PullRequest {
    PullRequest {
        repo: p.repo.clone(),
        number: p.number as i64,
        url: p.url.clone(),
        title: p.title.clone(),
        state: p.state.clone(),
        is_draft: p.is_draft,
        mergeable: p.mergeable.clone(),
        merge_state: p.merge_state.clone(),
        review_state: p.review.clone(),
        checks_state: p.checks.clone(),
        head_ref: p.head.clone(),
        base_ref: p.base.clone(),
        ..Default::default()
    }
}

/// The PR body: the newest PR-description doc if there is one, else the
/// task description, plus links to the issue and ticket.
fn pr_body(store: &ArtifactStore, task: &Task, repo: &str) -> String {
    let files = store.scan(task.id);
    let doc = files
        .iter()
        .find(|f| f.kind == "pr_description")
        .and_then(|f| store.read(task.id, &f.name).ok());
    let mut body = doc
        .unwrap_or_else(|| task.description.clone())
        .trim()
        .to_string();
    let mut links = vec![];
    if let Some(i) = &task.github_issue {
        // Same repo: "Closes #12" links and closes the issue on merge.
        let short = i.strip_prefix(&format!("{repo}#")).map(|n| format!("#{n}"));
        links.push(format!("Closes {}", short.as_deref().unwrap_or(i)));
    }
    if let (Some(id), Some(url)) = (&task.linear_issue_id, &task.linear_url) {
        links.push(format!("Linear: [{id}]({url})"));
    }
    if !links.is_empty() {
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str(&links.join("\n"));
    }
    body
}

/// Push the task's branch and open a PR (or link the one that exists).
pub async fn open_pr(app: &AppHandle, task_id: Id, draft: bool) -> Result<PullRequest, String> {
    let state = app.state::<AppState>();
    let task = state.db.get_task(task_id).map_err(|e| e.to_string())?;
    let (Some(wt), Some(branch)) = (task.worktree_path.clone(), task.branch.clone()) else {
        return Err("This task has no worktree branch yet. Move it to Implement, or create a worktree in the Changes tab.".into());
    };
    if task.worktree_status.as_deref() != Some(hl_core::model::worktree_status::READY) {
        return Err("The worktree is not ready yet.".into());
    }
    let repo_dir = project_dir(&state, &task)?;
    let env = env(&state);
    let base_ref = task.base_ref.clone();
    let title = task.title.clone();
    let store_task = task.clone();
    let body_fn = {
        let store = state.artifacts.clone();
        move |repo: &str| pr_body(&store, &store_task, repo)
    };
    let pr = blocking(move || {
        let wt = Path::new(&wt);
        let repo = github::repo_name(&repo_dir, &env).map_err(err)?;
        // A base that is a commit (detached HEAD) can't be a PR base.
        let base = match base_ref {
            Some(b) if !(b.len() == 40 && b.chars().all(|c| c.is_ascii_hexdigit())) => b,
            _ => github::default_branch(&repo_dir, &repo, &env).map_err(err)?,
        };
        let dirty = hl_git::worktree::dirty_files(wt, &env).map_err(err)?;
        if !dirty.is_empty() {
            return Err(format!(
                "{} file(s) in the worktree are not committed (e.g. {}). Commit them first; a PR only has commits.",
                dirty.len(),
                dirty[0]
            ));
        }
        if github::commits_ahead(wt, &base, &env).map_err(err)? == 0 {
            return Err(format!("{branch} has no commits that {base} doesn't have yet."));
        }
        github::push(wt, "origin", &branch, &env).map_err(err)?;
        let number = match github::find_pr(wt, &repo, &branch, &env).map_err(err)? {
            Some(n) => n,
            None => {
                let body = body_fn(&repo);
                let new = github::NewPr {
                    repo: &repo,
                    head: &branch,
                    base: &base,
                    title: &title,
                    body: &body,
                    draft,
                };
                github::create_pr(wt, &new, &env).map_err(err)?
            },
        };
        github::pr(wt, &repo, number, &env).map_err(err)
    })
    .await?;
    let row = state
        .db
        .upsert_pr(task_id, &to_row(&pr))
        .map_err(|e| e.to_string())?;
    let _ = app.emit(PR_UPDATED, &row);
    Ok(row)
}

/// Refresh one PR. A newly merged PR moves its task to Done.
pub async fn refresh(app: &AppHandle, pr: &PullRequest) -> Result<PullRequest, String> {
    let state = app.state::<AppState>();
    let task = state.db.get_task(pr.task_id).map_err(|e| e.to_string())?;
    let dir = project_dir(&state, &task)?;
    let env = env(&state);
    let (repo, number) = (pr.repo.clone(), pr.number as u64);
    let fresh = blocking(move || github::pr(&dir, &repo, number, &env).map_err(err)).await?;
    let row = state
        .db
        .upsert_pr(pr.task_id, &to_row(&fresh))
        .map_err(|e| e.to_string())?;
    let changed = PullRequest {
        updated_at: String::new(),
        ..row.clone()
    } != PullRequest {
        updated_at: String::new(),
        ..pr.clone()
    };
    if changed {
        let _ = app.emit(PR_UPDATED, &row);
    }
    if row.state == "MERGED" && pr.state != "MERGED" && task.stage != stage::DONE {
        let done = state
            .db
            .list_tasks(task.project_id)
            .map(|ts| ts.iter().filter(|t| t.stage == stage::DONE).count());
        if let Ok((t, _)) = state.db.move_task(task.id, stage::DONE, done.unwrap_or(0)) {
            let _ = app.emit(TASK_UPDATED, &t);
            linear::on_stage_changed(app, &t);
            linear::notice(
                app,
                "info",
                format!("PR #{} merged; “{}” moved to Done.", row.number, t.title),
            );
        }
    }
    Ok(row)
}

pub async fn poll_once(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(prs) = state.db.open_prs() else { return };
    for pr in prs {
        if let Err(e) = refresh(app, &pr).await {
            log::warn!("PR {}#{}: {e}", pr.repo, pr.number);
        }
    }
}

pub fn start_polling(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(8)).await;
        loop {
            poll_once(&app).await;
            tokio::time::sleep(POLL_EVERY).await;
        }
    });
}

pub async fn merge(app: &AppHandle, pr_id: Id, method: &str) -> Result<PullRequest, String> {
    let state = app.state::<AppState>();
    let pr = state.db.get_pr(pr_id).map_err(|e| e.to_string())?;
    let task = state.db.get_task(pr.task_id).map_err(|e| e.to_string())?;
    let dir = project_dir(&state, &task)?;
    let env = env(&state);
    let (repo, number, method) = (pr.repo.clone(), pr.number as u64, method.to_string());
    blocking(move || github::merge_pr(&dir, &repo, number, &method, &env).map_err(err)).await?;
    refresh(app, &pr).await
}
