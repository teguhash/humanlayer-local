//! SQLite storage. One connection behind a mutex: the app is single-user
//! and writes are small, so this is simpler than a pool and fast enough.
//!
//! Migrations are plain SQL files applied in order; `PRAGMA user_version`
//! records how many have run.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::artifacts::ArtifactFile;
use crate::model::{
    AgentKind, Artifact, Event, Id, LinearTicket, Project, Session, Task, stage, status,
    worktree_status,
};
use crate::slug::slugify;

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_worktrees.sql"),
    include_str!("../migrations/0003_board.sql"),
    include_str!("../migrations/0004_agents.sql"),
    include_str!("../migrations/0005_linear.sql"),
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::Invalid(format!("cannot create {}: {e}", dir.display())))?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        backfill_slugs(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        f(&conn)
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.with(|c| Ok(c.pragma_query_value(None, "user_version", |r| r.get(0))?))
    }

    // ---- projects ----

    pub fn create_project(&self, name: &str, repo_path: &str) -> Result<Project> {
        let name = non_empty(name, "Project name")?;
        self.with(|c| {
            let res = c.query_row(
                "INSERT INTO projects (name, repo_path) VALUES (?1, ?2) RETURNING *",
                params![name, repo_path],
                project_row,
            );
            match res {
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    Err(Error::Invalid(format!(
                        "A project for {repo_path} already exists."
                    )))
                }
                r => Ok(r?),
            }
        })
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT * FROM projects ORDER BY name COLLATE NOCASE, id")?;
            let rows = st.query_map([], project_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn get_project(&self, id: Id) -> Result<Project> {
        self.with(|c| {
            c.query_row("SELECT * FROM projects WHERE id = ?1", [id], project_row)
                .optional()?
                .ok_or(Error::NotFound("Project"))
        })
    }

    pub fn delete_project(&self, id: Id) -> Result<()> {
        self.delete("projects", "Project", id)
    }

    // ---- tasks ----

    pub fn create_task(&self, project_id: Id, title: &str) -> Result<Task> {
        self.create_task_with(project_id, title, "")
    }

    pub fn create_task_with(&self, project_id: Id, title: &str, description: &str) -> Result<Task> {
        let title = non_empty(title, "Task title")?;
        self.with(|c| {
            let slug = free_slug(c, project_id, &slugify(title))?;
            // New tasks go to the bottom of the backlog column.
            c.query_row(
                "INSERT INTO tasks (project_id, title, description, slug, position)
                 VALUES (?1, ?2, ?3, ?4, (SELECT coalesce(max(position), -1) + 1
                                  FROM tasks WHERE project_id = ?1 AND stage = 'backlog'))
                 RETURNING *",
                params![project_id, title, description.trim(), slug],
                task_row,
            )
            .map_err(|e| fk_to_not_found(e, "Project"))
        })
    }

    /// A task for a Linear ticket: slug from the identifier and title, and
    /// the imported text remembered for later syncs.
    pub fn create_linear_task(&self, project_id: Id, t: &LinearTicket) -> Result<Task> {
        let title = non_empty(&t.title, "Task title")?;
        let description = t.description.trim();
        self.with(|c| {
            let slug = free_slug(
                c,
                project_id,
                &slugify(&format!("{} {title}", t.identifier)),
            )?;
            let res = c.query_row(
                "INSERT INTO tasks (project_id, title, description, slug, position,
                   linear_issue_id, linear_url, linear_state, linear_updated_at,
                   linear_synced_title, linear_synced_description)
                 VALUES (?1, ?2, ?3, ?4, (SELECT coalesce(max(position), -1) + 1
                                  FROM tasks WHERE project_id = ?1 AND stage = 'backlog'),
                   ?5, ?6, ?7, ?8, ?2, ?3)
                 RETURNING *",
                params![
                    project_id,
                    title,
                    description,
                    slug,
                    t.identifier,
                    t.url,
                    t.state,
                    t.updated_at
                ],
                task_row,
            );
            match res {
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::ConstraintViolation
                        && e.extended_code != rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY =>
                {
                    Err(Error::Invalid(format!(
                        "{} is already a task in this project.",
                        t.identifier
                    )))
                }
                r => r.map_err(|e| fk_to_not_found(e, "Project")),
            }
        })
    }

    /// Apply a newer copy of the ticket. Title and description follow the
    /// ticket unless they were edited here since the last sync.
    pub fn sync_linear_task(&self, id: Id, t: &LinearTicket) -> Result<Task> {
        let title = non_empty(&t.title, "Task title")?;
        self.update_task(
            "UPDATE tasks SET
               title = CASE WHEN title IS linear_synced_title THEN ?2 ELSE title END,
               description = CASE WHEN description IS linear_synced_description
                                  THEN ?3 ELSE description END,
               linear_synced_title = ?2, linear_synced_description = ?3,
               linear_url = ?4, linear_state = ?5, linear_updated_at = ?6,
               updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 RETURNING *",
            params![
                id,
                title,
                t.description.trim(),
                t.url,
                t.state,
                t.updated_at
            ],
        )
    }

    pub fn set_linear_state(&self, id: Id, state: &str) -> Result<Task> {
        self.update_task(
            "UPDATE tasks SET linear_state = ?2 WHERE id = ?1 RETURNING *",
            params![id, state],
        )
    }

    /// Linked tasks that are not archived, across all projects.
    pub fn linked_tasks(&self) -> Result<Vec<Task>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT * FROM tasks WHERE linear_issue_id IS NOT NULL AND archived_at IS NULL
                 ORDER BY id",
            )?;
            let rows = st.query_map([], task_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn linear_post(&self, task_id: Id, path: &str) -> Result<Option<String>> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT comment_id FROM linear_posts WHERE task_id = ?1 AND path = ?2",
                params![task_id, path],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    pub fn set_linear_post(&self, task_id: Id, path: &str, comment_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO linear_posts (task_id, path, comment_id) VALUES (?1, ?2, ?3)
                 ON CONFLICT (task_id, path) DO UPDATE SET comment_id = ?3,
                   posted_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                params![task_id, path, comment_id],
            )?;
            Ok(())
        })
    }

    /// Edit title and description. The slug stays, so the docs folder and
    /// branch keep their names.
    pub fn update_task_text(&self, id: Id, title: &str, description: &str) -> Result<Task> {
        let title = non_empty(title, "Task title")?;
        self.update_task(
            "UPDATE tasks SET title = ?2, description = ?3,
               updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 RETURNING *",
            params![id, title, description.trim()],
        )
    }

    /// Override the project's default agent for this task (None = default).
    pub fn set_task_agent(&self, id: Id, agent: Option<AgentKind>) -> Result<Task> {
        self.update_task(
            "UPDATE tasks SET agent = ?2,
               updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 RETURNING *",
            params![id, agent.map(AgentKind::as_str)],
        )
    }

    pub fn set_project_agent(&self, id: Id, agent: AgentKind) -> Result<Project> {
        self.with(|c| {
            c.query_row(
                "UPDATE projects SET default_agent = ?2 WHERE id = ?1 RETURNING *",
                params![id, agent.as_str()],
                project_row,
            )
            .optional()?
            .ok_or(Error::NotFound("Project"))
        })
    }

    /// The agent that runs the task's stage sessions.
    pub fn agent_for(&self, task: &Task) -> Result<AgentKind> {
        Ok(match task.agent {
            Some(a) => a,
            None => self.get_project(task.project_id)?.default_agent,
        })
    }

    /// Put the task in `new_stage` at `index` (0 = top), renumbering that
    /// column. Returns the task and the stage it came from.
    pub fn move_task(&self, id: Id, new_stage: &str, index: usize) -> Result<(Task, String)> {
        if !stage::ALL.contains(&new_stage) {
            return Err(Error::Invalid(format!("Unknown stage {new_stage}.")));
        }
        let mut conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let tx = conn.transaction()?;
        let (project_id, old_stage): (Id, String) = tx
            .query_row(
                "SELECT project_id, stage FROM tasks WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or(Error::NotFound("Task"))?;
        let mut ids: Vec<Id> = {
            let mut st = tx.prepare(
                "SELECT id FROM tasks WHERE project_id = ?1 AND stage = ?2 AND id != ?3
                 ORDER BY position, id",
            )?;
            st.query_map(params![project_id, new_stage, id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        ids.insert(index.min(ids.len()), id);
        for (pos, tid) in (0_i64..).zip(&ids) {
            tx.execute(
                "UPDATE tasks SET position = ?2, stage = ?3,
                   updated_at = CASE WHEN id = ?4 THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE updated_at END
                 WHERE id = ?1",
                params![tid, pos, new_stage, id],
            )?;
        }
        let task = tx.query_row("SELECT * FROM tasks WHERE id = ?1", [id], task_row)?;
        tx.commit()?;
        Ok((task, old_stage))
    }

    pub fn list_tasks(&self, project_id: Id) -> Result<Vec<Task>> {
        self.with(|c| {
            let mut st =
                c.prepare("SELECT * FROM tasks WHERE project_id = ?1 ORDER BY position, id")?;
            let rows = st.query_map([project_id], task_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn get_task(&self, id: Id) -> Result<Task> {
        self.with(|c| {
            c.query_row("SELECT * FROM tasks WHERE id = ?1", [id], task_row)
                .optional()?
                .ok_or(Error::NotFound("Task"))
        })
    }

    pub fn delete_task(&self, id: Id) -> Result<()> {
        self.delete("tasks", "Task", id)
    }

    /// Run an `UPDATE tasks ... WHERE id = ?1 RETURNING *`.
    fn update_task(&self, sql: &str, args: impl rusqlite::Params) -> Result<Task> {
        self.with(|c| {
            c.query_row(sql, args, task_row)
                .optional()?
                .ok_or(Error::NotFound("Task"))
        })
    }

    /// Record a newly created worktree (status `setting_up`).
    pub fn set_worktree(
        &self,
        id: Id,
        path: &str,
        branch: &str,
        base_ref: &str,
        base_commit: &str,
    ) -> Result<Task> {
        self.update_task(
            "UPDATE tasks SET worktree_path = ?2, branch = ?3, base_ref = ?4, base_commit = ?5,
               worktree_status = ?6, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 RETURNING *",
            params![
                id,
                path,
                branch,
                base_ref,
                base_commit,
                worktree_status::SETTING_UP
            ],
        )
    }

    /// Set the worktree status; `log` replaces the stored log when given.
    pub fn set_worktree_status(&self, id: Id, new_status: &str, log: Option<&str>) -> Result<Task> {
        self.update_task(
            "UPDATE tasks SET worktree_status = ?2, worktree_log = coalesce(?3, worktree_log),
               updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 RETURNING *",
            params![id, new_status, log],
        )
    }

    /// The worktree is gone. Branch and base stay so it can be recreated.
    pub fn clear_worktree(&self, id: Id) -> Result<Task> {
        self.update_task(
            "UPDATE tasks SET worktree_path = NULL, worktree_status = NULL,
               updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 RETURNING *",
            [id],
        )
    }

    pub fn set_task_archived(&self, id: Id, archived: bool) -> Result<Task> {
        self.update_task(
            "UPDATE tasks SET
               archived_at = CASE WHEN ?2 THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') END,
               updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 RETURNING *",
            params![id, archived],
        )
    }

    /// At app start no setup is still running; mark cut-off ones failed.
    pub fn fail_unfinished_worktrees(&self) -> Result<usize> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE tasks SET
                   worktree_status = CASE WHEN worktree_path IS NULL THEN ?1 ELSE ?2 END,
                   worktree_log = coalesce(worktree_log, '') || '\n[The app quit before this finished.]'
                 WHERE worktree_status IN (?3, ?4)",
                params![
                    worktree_status::FAILED,
                    worktree_status::SETUP_FAILED,
                    worktree_status::CREATING,
                    worktree_status::SETTING_UP
                ],
            )?)
        })
    }

    // ---- artifacts ----

    /// Replace the stored list for a task with a fresh scan. Returns true
    /// when anything changed.
    pub fn replace_artifacts(&self, task_id: Id, files: &[ArtifactFile]) -> Result<bool> {
        let before = self.list_artifacts(task_id)?;
        let same = before.len() == files.len()
            && files.iter().all(|f| {
                before
                    .iter()
                    .any(|a| a.path == f.name && a.updated_at == f.modified.to_string())
            });
        if same {
            return Ok(false);
        }
        let mut conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM artifacts WHERE task_id = ?1", [task_id])?;
        for f in files {
            tx.execute(
                "INSERT INTO artifacts (task_id, kind, path, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params![task_id, f.kind, f.name, f.modified.to_string()],
            )
            .map_err(|e| fk_to_not_found(e, "Task"))?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// Newest first. `updated_at` is seconds since the Unix epoch.
    pub fn list_artifacts(&self, task_id: Id) -> Result<Vec<Artifact>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT * FROM artifacts WHERE task_id = ?1
                 ORDER BY CAST(updated_at AS INTEGER) DESC, path",
            )?;
            let rows = st.query_map([task_id], |r| {
                Ok(Artifact {
                    id: r.get("id")?,
                    task_id: r.get("task_id")?,
                    kind: r.get("kind")?,
                    path: r.get("path")?,
                    updated_at: r.get("updated_at")?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    // ---- sessions ----

    pub fn create_session(&self, task_id: Id, agent: AgentKind) -> Result<Session> {
        self.create_session_with(task_id, agent, None)
    }

    /// `skill` is recorded on the session (e.g. "create-research").
    pub fn create_session_with(
        &self,
        task_id: Id,
        agent: AgentKind,
        skill: Option<&str>,
    ) -> Result<Session> {
        self.with(|c| {
            c.query_row(
                "INSERT INTO sessions (task_id, agent_kind, skill) VALUES (?1, ?2, ?3) RETURNING *",
                params![task_id, agent.as_str(), skill],
                session_row,
            )
            .map_err(|e| fk_to_not_found(e, "Task"))
        })
    }

    pub fn list_sessions(&self, task_id: Id) -> Result<Vec<Session>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT * FROM sessions WHERE task_id = ?1 ORDER BY id")?;
            let rows = st.query_map([task_id], session_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn get_session(&self, id: Id) -> Result<Session> {
        self.with(|c| {
            c.query_row("SELECT * FROM sessions WHERE id = ?1", [id], session_row)
                .optional()?
                .ok_or(Error::NotFound("Session"))
        })
    }

    pub fn delete_session(&self, id: Id) -> Result<()> {
        self.delete("sessions", "Session", id)
    }

    /// Set status. `running` also sets `started_at` the first time; leaving
    /// `running` sets `ended_at`.
    pub fn set_session_status(&self, id: Id, new_status: &str) -> Result<Session> {
        self.with(|c| {
            c.query_row(
                "UPDATE sessions SET
                   status = ?2,
                   started_at = CASE WHEN ?2 = 'running' THEN coalesce(started_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')) ELSE started_at END,
                   ended_at = CASE WHEN ?2 = 'running' THEN NULL ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now') END
                 WHERE id = ?1 RETURNING *",
                params![id, new_status],
                session_row,
            )
            .optional()?
            .ok_or(Error::NotFound("Session"))
        })
    }

    pub fn set_session_cwd(&self, id: Id, cwd: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE sessions SET cwd = ?2 WHERE id = ?1",
                params![id, cwd],
            )?;
            Ok(())
        })
    }

    pub fn set_cli_session_id(&self, id: Id, cli_session_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE sessions SET cli_session_id = ?2 WHERE id = ?1",
                params![id, cli_session_id],
            )?;
            Ok(())
        })
    }

    /// At app start no CLI process survives, so a session still marked
    /// running was cut off. Returns how many were changed.
    pub fn mark_running_sessions_interrupted(&self) -> Result<usize> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE sessions SET status = ?1, ended_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE status = ?2",
                params![status::INTERRUPTED, status::RUNNING],
            )?)
        })
    }

    // ---- events ----

    /// Append an event; `seq` is the next number for this session.
    pub fn append_event(
        &self,
        session_id: Id,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<Event> {
        let json = serde_json::to_string(payload)
            .map_err(|e| Error::Invalid(format!("event payload: {e}")))?;
        self.with(|c| {
            c.query_row(
                "INSERT INTO events (session_id, seq, kind, payload_json)
                 VALUES (?1, (SELECT coalesce(max(seq), -1) + 1 FROM events WHERE session_id = ?1), ?2, ?3)
                 RETURNING *",
                params![session_id, kind, json],
                event_row,
            )
            .map_err(|e| fk_to_not_found(e, "Session"))
        })
    }

    /// Events with `seq > after` (all when `after` is None), in order.
    pub fn list_events(&self, session_id: Id, after: Option<i64>) -> Result<Vec<Event>> {
        self.with(|c| {
            let mut st =
                c.prepare("SELECT * FROM events WHERE session_id = ?1 AND seq > ?2 ORDER BY seq")?;
            let rows = st.query_map(params![session_id, after.unwrap_or(-1)], event_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    fn delete(&self, table: &'static str, what: &'static str, id: Id) -> Result<()> {
        self.with(|c| {
            let n = c.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [id])?;
            if n == 0 {
                Err(Error::NotFound(what))
            } else {
                Ok(())
            }
        })
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let done: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let total = MIGRATIONS.len() as i64;
    if done > total {
        return Err(Error::Invalid(format!(
            "Database schema version {done} is newer than this app ({total}). Update the app."
        )));
    }
    for (i, sql) in (0..).zip(MIGRATIONS).skip(done as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i + 1_i64)?;
        tx.commit()?;
    }
    Ok(())
}

/// `base`, or `base-2`, `base-3`, ... whichever is free in the project.
fn free_slug(c: &Connection, project_id: Id, base: &str) -> Result<String> {
    let mut st = c.prepare("SELECT 1 FROM tasks WHERE project_id = ?1 AND slug = ?2")?;
    for n in 1.. {
        let s = if n == 1 {
            base.to_string()
        } else {
            format!("{base}-{n}")
        };
        if !st.exists(params![project_id, s])? {
            return Ok(s);
        }
    }
    unreachable!()
}

/// Tasks created before migration 0003 have no slug.
fn backfill_slugs(conn: &mut Connection) -> Result<()> {
    let rows: Vec<(Id, Id, String)> = {
        let mut st =
            conn.prepare("SELECT id, project_id, title FROM tasks WHERE slug IS NULL ORDER BY id")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let tx = conn.transaction()?;
    for (id, project_id, title) in rows {
        let slug = free_slug(&tx, project_id, &slugify(&title))?;
        tx.execute(
            "UPDATE tasks SET slug = ?2 WHERE id = ?1",
            params![id, slug],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn non_empty<'a>(s: &'a str, what: &str) -> Result<&'a str> {
    let t = s.trim();
    if t.is_empty() {
        Err(Error::Invalid(format!("{what} is empty.")))
    } else {
        Ok(t)
    }
}

fn fk_to_not_found(e: rusqlite::Error, parent: &'static str) -> Error {
    match &e {
        rusqlite::Error::SqliteFailure(f, _)
            if f.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY =>
        {
            Error::NotFound(parent)
        }
        _ => Error::Sqlite(e),
    }
}

fn project_row(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get("id")?,
        name: r.get("name")?,
        repo_path: r.get("repo_path")?,
        created_at: r.get("created_at")?,
        default_agent: agent_col(r.get("default_agent")?)?,
    })
}

fn task_row(r: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get("id")?,
        project_id: r.get("project_id")?,
        title: r.get("title")?,
        description: r.get("description")?,
        slug: r.get::<_, Option<String>>("slug")?.unwrap_or_default(),
        stage: r.get("stage")?,
        position: r.get("position")?,
        worktree_path: r.get("worktree_path")?,
        linear_issue_id: r.get("linear_issue_id")?,
        github_issue: r.get("github_issue")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        branch: r.get("branch")?,
        base_ref: r.get("base_ref")?,
        base_commit: r.get("base_commit")?,
        worktree_status: r.get("worktree_status")?,
        worktree_log: r.get("worktree_log")?,
        archived_at: r.get("archived_at")?,
        linear_url: r.get("linear_url")?,
        linear_state: r.get("linear_state")?,
        linear_updated_at: r.get("linear_updated_at")?,
        agent: r
            .get::<_, Option<String>>("agent")?
            .map(agent_col)
            .transpose()?,
    })
}

fn agent_col(s: String) -> rusqlite::Result<AgentKind> {
    AgentKind::parse(&s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown agent {s}").into(),
        )
    })
}

fn event_row(r: &Row) -> rusqlite::Result<Event> {
    let json: String = r.get("payload_json")?;
    Ok(Event {
        id: r.get("id")?,
        session_id: r.get("session_id")?,
        seq: r.get("seq")?,
        kind: r.get("kind")?,
        payload: serde_json::from_str(&json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        created_at: r.get("created_at")?,
    })
}

fn session_row(r: &Row) -> rusqlite::Result<Session> {
    let kind: String = r.get("agent_kind")?;
    Ok(Session {
        id: r.get("id")?,
        task_id: r.get("task_id")?,
        agent_kind: AgentKind::parse(&kind).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!("unknown agent_kind {kind}").into(),
            )
        })?,
        cli_session_id: r.get("cli_session_id")?,
        skill: r.get("skill")?,
        status: r.get("status")?,
        created_at: r.get("created_at")?,
        started_at: r.get("started_at")?,
        ended_at: r.get("ended_at")?,
        cwd: r.get("cwd")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    #[test]
    fn migrations_run_once_and_are_idempotent() {
        let dir = std::env::temp_dir().join(format!("hl-core-mig-{}", std::process::id()));
        let path = dir.join("t.db");
        let _ = std::fs::remove_dir_all(&dir);
        let a = Db::open(&path).unwrap();
        assert_eq!(a.schema_version().unwrap(), MIGRATIONS.len() as i64);
        a.create_project("p", "/r").unwrap();
        drop(a);
        let b = Db::open(&path).unwrap();
        assert_eq!(b.list_projects().unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn task_agent_falls_back_to_project_default() {
        let db = db();
        let p = db.create_project("Demo", "/repo").unwrap();
        assert_eq!(p.default_agent, AgentKind::Claude);
        let t = db.create_task(p.id, "First").unwrap();
        assert_eq!(
            (t.agent, db.agent_for(&t).unwrap()),
            (None, AgentKind::Claude)
        );
        db.set_project_agent(p.id, AgentKind::Codex).unwrap();
        assert_eq!(db.agent_for(&t).unwrap(), AgentKind::Codex);
        let t = db.set_task_agent(t.id, Some(AgentKind::Claude)).unwrap();
        assert_eq!(db.agent_for(&t).unwrap(), AgentKind::Claude);
        let t = db.set_task_agent(t.id, None).unwrap();
        assert_eq!(db.agent_for(&t).unwrap(), AgentKind::Codex);
        assert!(db.set_project_agent(999, AgentKind::Codex).is_err());
    }

    #[test]
    fn linear_import_sync_and_posts() {
        let db = db();
        let p = db.create_project("Demo", "/repo").unwrap();
        let mut tk = LinearTicket {
            identifier: "ENG-7".into(),
            title: "Fix login".into(),
            description: " It breaks ".into(),
            url: "https://linear.app/x/issue/ENG-7".into(),
            state: "Todo".into(),
            updated_at: "1".into(),
        };
        let t = db.create_linear_task(p.id, &tk).unwrap();
        assert_eq!(t.slug, "eng-7-fix-login");
        assert_eq!(
            (t.description.as_str(), t.linear_state.as_deref()),
            ("It breaks", Some("Todo"))
        );
        let dup = db.create_linear_task(p.id, &tk).unwrap_err();
        assert!(dup.to_string().contains("already a task"), "{dup}");
        assert!(db.create_linear_task(999, &tk).is_err());

        // Untouched fields follow the ticket.
        tk.title = "Fix login v2".into();
        tk.state = "In Progress".into();
        let t = db.sync_linear_task(t.id, &tk).unwrap();
        assert_eq!(
            (t.title.as_str(), t.linear_state.as_deref()),
            ("Fix login v2", Some("In Progress"))
        );
        // A local edit sticks.
        db.update_task_text(t.id, "My title", "It breaks").unwrap();
        tk.title = "Fix login v3".into();
        tk.description = "Now with steps".into();
        let t = db.sync_linear_task(t.id, &tk).unwrap();
        assert_eq!(
            (t.title.as_str(), t.description.as_str()),
            ("My title", "Now with steps")
        );
        assert_eq!(db.linked_tasks().unwrap().len(), 1);

        assert_eq!(db.linear_post(t.id, "a.md").unwrap(), None);
        db.set_linear_post(t.id, "a.md", "c1").unwrap();
        db.set_linear_post(t.id, "a.md", "c2").unwrap();
        assert_eq!(db.linear_post(t.id, "a.md").unwrap().as_deref(), Some("c2"));
    }

    #[test]
    fn newer_schema_is_rejected() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        let err = Db::init(conn).err().unwrap();
        assert!(err.to_string().contains("newer"), "{err}");
    }

    #[test]
    fn project_task_session_crud_and_cascade() {
        let db = db();
        let p = db.create_project("  Demo ", "/repo").unwrap();
        assert_eq!(p.name, "Demo");
        assert_eq!(db.get_project(p.id).unwrap(), p);

        let t1 = db.create_task(p.id, "First").unwrap();
        let t2 = db.create_task(p.id, "Second").unwrap();
        assert_eq!(
            (t1.stage.as_str(), t1.position, t2.position),
            ("backlog", 0, 1)
        );
        assert_eq!(db.list_tasks(p.id).unwrap(), vec![t1.clone(), t2.clone()]);

        let s = db.create_session(t1.id, AgentKind::Codex).unwrap();
        assert_eq!(
            (s.agent_kind, s.status.as_str()),
            (AgentKind::Codex, "idle")
        );
        assert_eq!(db.list_sessions(t1.id).unwrap(), vec![s.clone()]);

        db.delete_session(s.id).unwrap();
        assert!(matches!(
            db.get_session(s.id),
            Err(Error::NotFound("Session"))
        ));

        db.create_session(t2.id, AgentKind::Claude).unwrap();
        db.delete_project(p.id).unwrap();
        assert!(db.list_tasks(p.id).unwrap().is_empty());
        assert!(db.list_sessions(t2.id).unwrap().is_empty());
    }

    fn session(db: &Db) -> Session {
        let p = db.create_project("p", "/r").unwrap();
        let t = db.create_task(p.id, "t").unwrap();
        db.create_session(t.id, AgentKind::Claude).unwrap()
    }

    #[test]
    fn events_get_sequential_seq_per_session() {
        let db = db();
        let s = session(&db);
        let a = db
            .append_event(s.id, "user_prompt", &serde_json::json!({"text":"hi"}))
            .unwrap();
        let b = db
            .append_event(s.id, "cli", &serde_json::json!({"type":"x"}))
            .unwrap();
        assert_eq!((a.seq, b.seq), (0, 1));
        assert_eq!(
            db.list_events(s.id, None).unwrap(),
            vec![a.clone(), b.clone()]
        );
        assert_eq!(db.list_events(s.id, Some(0)).unwrap(), vec![b]);
        assert_eq!(a.payload["text"], "hi");
        assert!(matches!(
            db.append_event(999, "x", &serde_json::json!({})),
            Err(Error::NotFound("Session"))
        ));
    }

    #[test]
    fn status_timestamps_and_restart_recovery() {
        let db = db();
        let s = session(&db);
        let r = db.set_session_status(s.id, status::RUNNING).unwrap();
        assert!(r.started_at.is_some() && r.ended_at.is_none());
        db.set_cli_session_id(s.id, "abc").unwrap();
        assert_eq!(db.mark_running_sessions_interrupted().unwrap(), 1);
        let after = db.get_session(s.id).unwrap();
        assert_eq!(after.status, status::INTERRUPTED);
        assert_eq!(after.cli_session_id.as_deref(), Some("abc"));
        assert_eq!(after.started_at, r.started_at);
        assert!(after.ended_at.is_some());
        assert_eq!(db.mark_running_sessions_interrupted().unwrap(), 0);
    }

    #[test]
    fn worktree_lifecycle_and_archive() {
        let db = db();
        let p = db.create_project("p", "/r").unwrap();
        let t = db.create_task(p.id, "t").unwrap();
        assert_eq!(t.worktree_status, None);
        db.set_worktree_status(t.id, worktree_status::CREATING, None)
            .unwrap();
        let t = db.set_worktree(t.id, "/w", "b", "main", "abc").unwrap();
        assert_eq!(
            (
                t.worktree_path.as_deref(),
                t.branch.as_deref(),
                t.worktree_status.as_deref()
            ),
            (Some("/w"), Some("b"), Some(worktree_status::SETTING_UP))
        );
        assert_eq!(db.fail_unfinished_worktrees().unwrap(), 1);
        let t = db.get_task(t.id).unwrap();
        assert_eq!(
            t.worktree_status.as_deref(),
            Some(worktree_status::SETUP_FAILED)
        );
        assert!(t.worktree_log.unwrap().contains("app quit"));
        let t = db
            .set_worktree_status(t.id, worktree_status::READY, Some("ok"))
            .unwrap();
        assert_eq!(t.worktree_log.as_deref(), Some("ok"));
        let t = db.clear_worktree(t.id).unwrap();
        assert_eq!((t.worktree_path, t.worktree_status), (None, None));
        assert_eq!(t.branch.as_deref(), Some("b"));
        assert!(
            db.set_task_archived(t.id, true)
                .unwrap()
                .archived_at
                .is_some()
        );
        assert!(
            db.set_task_archived(t.id, false)
                .unwrap()
                .archived_at
                .is_none()
        );
        assert!(matches!(
            db.clear_worktree(999),
            Err(Error::NotFound("Task"))
        ));

        let s = db.create_session(t.id, AgentKind::Claude).unwrap();
        db.set_session_cwd(s.id, "/w").unwrap();
        assert_eq!(db.get_session(s.id).unwrap().cwd.as_deref(), Some("/w"));
    }

    #[test]
    fn slugs_text_and_board_moves() {
        let db = db();
        let p = db.create_project("p", "/r").unwrap();
        let a = db.create_task(p.id, "Fix it!").unwrap();
        let b = db.create_task_with(p.id, "fix it", " why ").unwrap();
        let c = db.create_task(p.id, "Other").unwrap();
        assert_eq!((a.slug.as_str(), b.slug.as_str()), ("fix-it", "fix-it-2"));
        assert_eq!(b.description, "why");
        let a2 = db.update_task_text(a.id, "Renamed", "d").unwrap();
        assert_eq!((a2.title.as_str(), a2.slug.as_str()), ("Renamed", "fix-it"));

        // Move c to the top of research, then a below it, then b between.
        let (c2, from) = db.move_task(c.id, "research", 0).unwrap();
        assert_eq!((c2.stage.as_str(), from.as_str()), ("research", "backlog"));
        db.move_task(a.id, "research", 9).unwrap();
        db.move_task(b.id, "research", 1).unwrap();
        let order: Vec<_> = db
            .list_tasks(p.id)
            .unwrap()
            .into_iter()
            .filter(|t| t.stage == "research")
            .map(|t| t.id)
            .collect();
        assert_eq!(order, vec![c.id, b.id, a.id]);
        // Reorder within the column.
        db.move_task(a.id, "research", 0).unwrap();
        let first = db
            .list_tasks(p.id)
            .unwrap()
            .into_iter()
            .find(|t| t.stage == "research")
            .unwrap();
        assert_eq!(first.id, a.id);
        assert!(matches!(
            db.move_task(a.id, "nope", 0),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            db.move_task(999, "plan", 0),
            Err(Error::NotFound("Task"))
        ));
    }

    #[test]
    fn artifacts_replace_and_list() {
        let db = db();
        let p = db.create_project("p", "/r").unwrap();
        let t = db.create_task(p.id, "t").unwrap();
        let f = |name: &str, kind: &str, m: u64| ArtifactFile {
            name: name.into(),
            kind: kind.into(),
            modified: m,
            size: 1,
        };
        assert!(
            db.replace_artifacts(t.id, &[f("a.md", "research", 1), f("b.md", "plan", 2)])
                .unwrap()
        );
        assert!(
            !db.replace_artifacts(t.id, &[f("b.md", "plan", 2), f("a.md", "research", 1)])
                .unwrap()
        );
        let l = db.list_artifacts(t.id).unwrap();
        assert_eq!(
            l.iter().map(|a| a.path.as_str()).collect::<Vec<_>>(),
            vec!["b.md", "a.md"]
        );
        assert!(
            db.replace_artifacts(t.id, &[f("a.md", "research", 3)])
                .unwrap()
        );
        assert_eq!(db.list_artifacts(t.id).unwrap().len(), 1);
        let s = db
            .create_session_with(t.id, AgentKind::Claude, Some("create-research"))
            .unwrap();
        assert_eq!(s.skill.as_deref(), Some("create-research"));
    }

    #[test]
    fn backfill_gives_old_tasks_unique_slugs() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute_batch(
            "INSERT INTO projects (id, name, repo_path) VALUES (1, 'p', '/r');
             INSERT INTO tasks (project_id, title, position) VALUES (1, 'Same', 0), (1, 'Same', 1);",
        )
        .unwrap();
        let db = Db::init(conn).unwrap();
        let slugs: Vec<_> = db
            .list_tasks(1)
            .unwrap()
            .into_iter()
            .map(|t| t.slug)
            .collect();
        assert_eq!(slugs, vec!["same", "same-2"]);
    }

    #[test]
    fn validation_and_not_found_errors() {
        let db = db();
        assert!(matches!(
            db.create_project(" ", "/r"),
            Err(Error::Invalid(_))
        ));
        db.create_project("a", "/r").unwrap();
        let dup = db.create_project("b", "/r").unwrap_err().to_string();
        assert!(dup.contains("already exists"), "{dup}");
        assert!(matches!(
            db.create_task(999, "x"),
            Err(Error::NotFound("Project"))
        ));
        assert!(matches!(
            db.create_session(999, AgentKind::Claude),
            Err(Error::NotFound("Task"))
        ));
        assert!(matches!(db.delete_task(999), Err(Error::NotFound("Task"))));
        assert!(matches!(
            db.get_project(999),
            Err(Error::NotFound("Project"))
        ));
    }
}
