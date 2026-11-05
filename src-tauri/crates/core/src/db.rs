//! SQLite storage. One connection behind a mutex: the app is single-user
//! and writes are small, so this is simpler than a pool and fast enough.
//!
//! Migrations are plain SQL files applied in order; `PRAGMA user_version`
//! records how many have run.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::model::{AgentKind, Event, Id, Project, Session, Task, status, worktree_status};

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_worktrees.sql"),
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
        let title = non_empty(title, "Task title")?;
        self.with(|c| {
            // New tasks go to the bottom of the backlog column.
            c.query_row(
                "INSERT INTO tasks (project_id, title, position)
                 VALUES (?1, ?2, (SELECT coalesce(max(position), -1) + 1
                                  FROM tasks WHERE project_id = ?1 AND stage = 'backlog'))
                 RETURNING *",
                params![project_id, title],
                task_row,
            )
            .map_err(|e| fk_to_not_found(e, "Project"))
        })
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

    // ---- sessions ----

    pub fn create_session(&self, task_id: Id, agent: AgentKind) -> Result<Session> {
        self.with(|c| {
            c.query_row(
                "INSERT INTO sessions (task_id, agent_kind) VALUES (?1, ?2) RETURNING *",
                params![task_id, agent.as_str()],
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
    })
}

fn task_row(r: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get("id")?,
        project_id: r.get("project_id")?,
        title: r.get("title")?,
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
