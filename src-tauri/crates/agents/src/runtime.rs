//! Session runtime: the operations the app calls, with every event written
//! to SQLite before it is reported.
//!
//! - `send`: starts a process if none is live (resuming the CLI session
//!   when the DB has its id), then sends the prompt.
//! - `interrupt`: ends the current turn; Claude keeps its process.
//! - `stop`: kills the process; the next `send` resumes.
//! - `respond_approval`: answers a pending tool approval.
//!
//! Status rules: `running` while a turn is in progress; `idle` after the
//! turn ends; `interrupted` if the process was stopped mid-turn or the app
//! quit mid-turn (see `Runtime::new`); `failed` if the CLI exited with an
//! error.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use hl_core::Db;
use hl_core::model::{AgentKind, Event, Id, Session, status, worktree_status};
use hl_core::shell_env;
use serde::Serialize;
use serde_json::{Value, json};

use crate::process::{self, Proc, ProcEvent};
use crate::transcript::{self, Decision, Item, SessionUsage};
use crate::{event_type, protocol};

/// One stored event plus its parsed items, as sent to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct SessionEvent {
    pub session_id: Id,
    /// -1 if the event could not be stored.
    pub seq: i64,
    pub kind: String,
    pub event_type: Option<String>,
    pub items: Vec<Item>,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Transcript {
    pub session: Session,
    pub events: Vec<SessionEvent>,
    pub usage: SessionUsage,
    /// A CLI process is alive for this session.
    pub live: bool,
    /// Approval request ids still waiting for an answer.
    pub pending_approvals: Vec<String>,
}

pub trait Sink: Send + Sync + 'static {
    fn event(&self, event: &SessionEvent);
    /// The session row changed (status, CLI session id).
    fn session(&self, session: &Session);
}

pub type EnvFn = dyn Fn() -> (HashMap<String, String>, Option<String>) + Send + Sync;

/// Called before each CLI process starts, with the session and its folder.
/// Returns extra CLI arguments (plugins, system prompt). It may also prepare
/// the folder (the app links the task's docs folder into it). Runs on a
/// blocking thread.
pub type PrepareFn = dyn Fn(&Session, &str) -> Result<Vec<String>, String> + Send + Sync;

pub struct Runtime {
    db: Arc<Db>,
    sink: Arc<dyn Sink>,
    env: Arc<EnvFn>,
    prepare: Arc<PrepareFn>,
    live: Mutex<HashMap<Id, Arc<Live>>>,
    next_request: AtomicU64,
}

struct Live {
    agent: AgentKind,
    proc: OnceLock<Arc<Proc>>,
    st: Mutex<LiveState>,
}

#[derive(Default)]
struct LiveState {
    turn_running: bool,
    exited: bool,
    /// request_id -> the CLI's request (tool, input, suggestions)
    pending: HashMap<String, Value>,
}

impl Live {
    fn proc(&self) -> Result<&Arc<Proc>, String> {
        self.proc
            .get()
            .ok_or_else(|| "Process is still starting.".into())
    }
}

impl Runtime {
    /// Uses the login-shell environment to find and run CLIs. Marks
    /// sessions left `running` by a previous app run as `interrupted`.
    pub fn new(db: Arc<Db>, sink: impl Sink) -> Result<Arc<Self>, String> {
        Self::with_env(db, sink, Arc::new(shell_env::get))
    }

    pub fn with_env(db: Arc<Db>, sink: impl Sink, env: Arc<EnvFn>) -> Result<Arc<Self>, String> {
        Self::with_options(db, sink, env, Arc::new(|_, _| Ok(vec![])))
    }

    pub fn with_options(
        db: Arc<Db>,
        sink: impl Sink,
        env: Arc<EnvFn>,
        prepare: Arc<PrepareFn>,
    ) -> Result<Arc<Self>, String> {
        db.mark_running_sessions_interrupted()
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(Self {
            db,
            sink: Arc::new(sink),
            env,
            prepare,
            live: Mutex::new(HashMap::new()),
            next_request: AtomicU64::new(1),
        }))
    }

    fn get_live(&self, id: Id) -> Option<Arc<Live>> {
        self.live
            .lock()
            .unwrap()
            .get(&id)
            .filter(|l| !l.st.lock().unwrap().exited)
            .cloned()
    }

    /// Send a prompt. Must be called inside a tokio runtime.
    pub async fn send(self: &Arc<Self>, session_id: Id, text: &str) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Prompt is empty.".into());
        }
        let session = self.db.get_session(session_id).map_err(|e| e.to_string())?;
        let cwd = self.session_cwd(&session)?;
        let proto = protocol(session.agent_kind);

        let existing = self.get_live(session_id);
        if let Some(l) = &existing
            && l.st.lock().unwrap().turn_running
        {
            return Err("A turn is already running. Interrupt it or wait for it to finish.".into());
        }

        self.record(
            session_id,
            session.agent_kind,
            "user_prompt",
            json!({ "text": text }),
        );
        let live = match existing.filter(|_| proto.persistent()) {
            Some(l) => l,
            None => match self
                .spawn_live(&session, &cwd, session.cli_session_id.as_deref())
                .await
            {
                Ok(l) => l,
                Err(e) => {
                    self.record(
                        session_id,
                        session.agent_kind,
                        "error",
                        json!({ "message": e }),
                    );
                    self.set_status(session_id, status::FAILED);
                    return Err(e);
                }
            },
        };

        live.st.lock().unwrap().turn_running = true;
        // Status before the prompt goes out, so a fast reply cannot be
        // overwritten by a late `running`.
        self.set_status(session_id, status::RUNNING);
        let proc = live.proc()?;
        if let Err(e) = proc.send_line(proto.user_turn(text)) {
            live.st.lock().unwrap().turn_running = false;
            self.set_status(session_id, status::FAILED);
            return Err(e);
        }
        if !proto.persistent() {
            proc.close_stdin();
        }
        Ok(())
    }

    /// The folder the CLI runs in. Fixed at the first turn (stored on the
    /// session): Claude keys resumable sessions by folder. New sessions use
    /// the task's worktree when it has one, else the repo.
    fn session_cwd(&self, session: &Session) -> Result<String, String> {
        let cwd = match &session.cwd {
            Some(c) => c.clone(),
            None => {
                let task = self
                    .db
                    .get_task(session.task_id)
                    .map_err(|e| e.to_string())?;
                if matches!(
                    task.worktree_status.as_deref(),
                    Some(worktree_status::CREATING | worktree_status::SETTING_UP)
                ) {
                    return Err("The task's worktree is still being set up.".into());
                }
                let cwd = match task.worktree_path {
                    Some(w) => w,
                    None => {
                        self.db
                            .get_project(task.project_id)
                            .map_err(|e| e.to_string())?
                            .repo_path
                    }
                };
                if Path::new(&cwd).is_dir() {
                    self.db
                        .set_session_cwd(session.id, &cwd)
                        .map_err(|e| e.to_string())?;
                }
                cwd
            }
        };
        if Path::new(&cwd).is_dir() {
            Ok(cwd)
        } else {
            Err(format!("Working folder does not exist: {cwd}"))
        }
    }

    async fn spawn_live(
        self: &Arc<Self>,
        session: &Session,
        cwd: &str,
        resume: Option<&str>,
    ) -> Result<Arc<Live>, String> {
        let proto = protocol(session.agent_kind);
        let env_fn = self.env.clone();
        // Loading the shell env can take a second; keep it off the workers.
        let (env, env_err) = tokio::task::spawn_blocking(move || env_fn())
            .await
            .map_err(|e| e.to_string())?;
        let (prepare, s, c) = (self.prepare.clone(), session.clone(), cwd.to_string());
        let extra = tokio::task::spawn_blocking(move || prepare(&s, &c))
            .await
            .map_err(|e| e.to_string())??;
        let args = proto.args(cwd, resume, extra);
        let bin = shell_env::which(proto.binary(), &env).ok_or_else(|| {
            format!(
                "`{}` not found on PATH. Install it, or check that your login shell puts it on PATH.{}",
                proto.binary(),
                env_err
                    .map(|e| format!(" (Shell env failed to load: {e})"))
                    .unwrap_or_default()
            )
        })?;

        let live = Arc::new(Live {
            agent: session.agent_kind,
            proc: OnceLock::new(),
            st: Mutex::new(LiveState::default()),
        });
        let weak: Weak<Self> = Arc::downgrade(self);
        let (sid, cb_live) = (session.id, live.clone());
        let proc = process::spawn(&bin, &args, &env, cwd, move |ev| {
            if let Some(rt) = weak.upgrade() {
                rt.on_proc_event(sid, &cb_live, ev);
            }
        })?;
        let _ = live.proc.set(proc.clone());
        for line in proto.handshake() {
            proc.send_line(line)?;
        }
        self.live.lock().unwrap().insert(session.id, live.clone());
        // It may have died already (e.g. bad flags); don't keep it listed.
        if live.st.lock().unwrap().exited {
            self.remove_live(session.id, &live);
        }
        Ok(live)
    }

    fn remove_live(&self, id: Id, live: &Arc<Live>) {
        let mut map = self.live.lock().unwrap();
        if map.get(&id).is_some_and(|l| Arc::ptr_eq(l, live)) {
            map.remove(&id);
        }
    }

    fn on_proc_event(&self, id: Id, live: &Arc<Live>, ev: ProcEvent) {
        let proto = protocol(live.agent);
        match ev {
            ProcEvent::Line(raw) => {
                let raw = proto.sanitize(raw);
                if let Some(cid) = proto.cli_session_id(&raw) {
                    self.update_cli_session_id(id, &cid);
                }
                if let Some((rid, req)) = proto.approval_request(&raw) {
                    live.st.lock().unwrap().pending.insert(rid, req);
                }
                let turn_end = proto.is_turn_end(&raw);
                self.record(id, live.agent, "cli", raw);
                if turn_end {
                    {
                        let mut st = live.st.lock().unwrap();
                        st.turn_running = false;
                        st.pending.clear();
                    }
                    self.set_status(id, status::IDLE);
                }
            }
            ProcEvent::Exit {
                success, stopped, ..
            } => {
                let (kind, payload) = ev.to_record();
                self.record(id, live.agent, kind, payload);
                let was_running = {
                    let mut st = live.st.lock().unwrap();
                    st.exited = true;
                    st.pending.clear();
                    std::mem::take(&mut st.turn_running)
                };
                self.remove_live(id, live);
                let next = match (was_running, stopped, success) {
                    (true, true, _) => Some(status::INTERRUPTED),
                    (true, false, true) => Some(status::IDLE),
                    (_, false, false) => Some(status::FAILED),
                    _ => None,
                };
                if let Some(s) = next {
                    self.set_status(id, s);
                }
            }
            other => {
                let (kind, payload) = other.to_record();
                self.record(id, live.agent, kind, payload);
            }
        }
    }

    fn update_cli_session_id(&self, id: Id, cid: &str) {
        match self.db.get_session(id) {
            Ok(s) if s.cli_session_id.as_deref() != Some(cid) => {
                if self.db.set_cli_session_id(id, cid).is_ok()
                    && let Ok(s) = self.db.get_session(id)
                {
                    self.sink.session(&s);
                }
            }
            _ => {}
        }
    }

    fn set_status(&self, id: Id, s: &str) {
        match self.db.set_session_status(id, s) {
            Ok(session) => self.sink.session(&session),
            Err(e) => eprintln!("session {id}: set status {s}: {e}"),
        }
    }

    /// Store, then report. If storing fails (e.g. the session was deleted)
    /// the event is still reported, with seq -1.
    fn record(&self, id: Id, agent: AgentKind, kind: &str, payload: Value) {
        let ev = match self.db.append_event(id, kind, &payload) {
            Ok(e) => to_session_event(agent, e),
            Err(e) => {
                eprintln!("session {id}: store {kind} event: {e}");
                SessionEvent {
                    session_id: id,
                    seq: -1,
                    kind: kind.into(),
                    event_type: event_type(&payload),
                    items: transcript::items_for(agent, kind, &payload),
                    payload,
                    created_at: String::new(),
                }
            }
        };
        self.sink.event(&ev);
    }

    pub fn respond_approval(
        &self,
        session_id: Id,
        request_id: &str,
        decision: Decision,
    ) -> Result<(), String> {
        let live = self
            .get_live(session_id)
            .ok_or("The session is not running, so this request has expired.")?;
        let request = live
            .st
            .lock()
            .unwrap()
            .pending
            .remove(request_id)
            .ok_or("This request was already answered or has expired.")?;
        let line = protocol(live.agent)
            .approval(request_id, decision, &request)
            .ok_or("This agent does not support approvals.")?;
        self.record(
            session_id,
            live.agent,
            "approval_response",
            json!({ "request_id": request_id, "decision": decision }),
        );
        live.proc()?.send_line(line)
    }

    pub fn interrupt(&self, session_id: Id) -> Result<(), String> {
        let live = self.get_live(session_id).ok_or("Nothing is running.")?;
        if !live.st.lock().unwrap().turn_running {
            return Err("No turn is running.".into());
        }
        let rid = format!(
            "interrupt-{}",
            self.next_request.fetch_add(1, Ordering::Relaxed)
        );
        self.record(session_id, live.agent, "interrupt_requested", json!({}));
        match protocol(live.agent).interrupt(&rid) {
            Some(line) => live.proc()?.send_line(line),
            None => {
                live.proc()?.stop();
                Ok(())
            }
        }
    }

    pub fn stop(&self, session_id: Id) -> Result<(), String> {
        let live = self.get_live(session_id).ok_or("Nothing is running.")?;
        live.proc()?.stop();
        Ok(())
    }

    /// Stop processes whose session no longer exists (after a delete).
    pub fn prune(&self) {
        let ids: Vec<Id> = self.live.lock().unwrap().keys().copied().collect();
        for id in ids {
            if self.db.get_session(id).is_err() {
                let _ = self.stop(id);
            }
        }
    }

    /// Stop every process (app quit).
    pub fn shutdown(&self) {
        let all: Vec<Arc<Live>> = self.live.lock().unwrap().values().cloned().collect();
        for l in all {
            if let Ok(p) = l.proc() {
                p.stop();
            }
        }
    }

    pub fn is_live(&self, session_id: Id) -> bool {
        self.get_live(session_id).is_some()
    }

    pub fn transcript(&self, session_id: Id) -> Result<Transcript, String> {
        let session = self.db.get_session(session_id).map_err(|e| e.to_string())?;
        let events = self
            .db
            .list_events(session_id, None)
            .map_err(|e| e.to_string())?;
        let usage = transcript::usage(session.agent_kind, &events);
        let live = self.get_live(session_id);
        let pending_approvals = live
            .as_ref()
            .map(|l| l.st.lock().unwrap().pending.keys().cloned().collect())
            .unwrap_or_default();
        Ok(Transcript {
            live: live.is_some(),
            pending_approvals,
            usage,
            events: events
                .into_iter()
                .map(|e| to_session_event(session.agent_kind, e))
                .collect(),
            session,
        })
    }

    pub fn usage(&self, session_id: Id) -> Result<SessionUsage, String> {
        let session = self.db.get_session(session_id).map_err(|e| e.to_string())?;
        let events = self
            .db
            .list_events(session_id, None)
            .map_err(|e| e.to_string())?;
        Ok(transcript::usage(session.agent_kind, &events))
    }
}

fn to_session_event(agent: AgentKind, e: Event) -> SessionEvent {
    SessionEvent {
        session_id: e.session_id,
        seq: e.seq,
        event_type: (e.kind == "cli").then(|| event_type(&e.payload)).flatten(),
        items: transcript::items_for(agent, &e.kind, &e.payload),
        kind: e.kind,
        payload: e.payload,
        created_at: e.created_at,
    }
}
