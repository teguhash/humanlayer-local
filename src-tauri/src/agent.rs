//! M0 spike: spawn `claude` or `codex` in JSON streaming mode and forward
//! every stdout line to the webview as an `agent-event` Tauri event.
//!
//! Everything runs on Tauri's async (tokio) runtime; commands return as soon
//! as the child is spawned. Failures (CLI missing, non-zero exit, bad JSON
//! lines, stderr output) become events too, so the UI can show them.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

use crate::shell_env;

pub const EVENT_NAME: &str = "agent-event";

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
}

impl AgentKind {
    fn binary(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
        }
    }

    /// Flags verified against `claude --help` 2.1.314 and
    /// `codex exec --help` 0.166.0. See docs/m0-findings.md.
    fn args(self, cwd: &str) -> Vec<String> {
        let v: &[&str] = match self {
            AgentKind::Claude => &[
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "default",
                // Hidden flag: routes permission prompts to us as
                // `control_request`/`can_use_tool` on stdout. Without it,
                // anything that would prompt is denied automatically.
                "--permission-prompt-tool",
                "stdio",
            ],
            // Prompt is read from stdin ("-"), so it can start with "-" and
            // has no argv length limit. read-only sandbox keeps the spike safe.
            AgentKind::Codex => &[
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                "--cd",
                cwd,
                "-",
            ],
        };
        v.iter().map(|s| s.to_string()).collect()
    }
}

/// One message to the UI. `kind` is ours; `event_type` is the CLI's.
#[derive(Debug, Clone, Serialize)]
pub struct AgentEvent {
    pub session_id: String,
    pub seq: u64,
    /// started | event | invalid_json | stderr | exit | error
    pub kind: &'static str,
    pub event_type: Option<String>,
    pub payload: Value,
}

enum StdinMsg {
    Line(String),
    Close,
}

struct Session {
    stdin: Option<mpsc::UnboundedSender<StdinMsg>>,
    kill: Option<oneshot::Sender<()>>,
}

#[derive(Default)]
pub struct Sessions(Mutex<HashMap<String, Session>>);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct SessionEmitter {
    app: AppHandle,
    session_id: String,
    seq: Arc<AtomicU64>,
}

impl SessionEmitter {
    fn emit(&self, kind: &'static str, event_type: Option<String>, payload: Value) {
        let ev = AgentEvent {
            session_id: self.session_id.clone(),
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            kind,
            event_type,
            payload,
        };
        if let Err(e) = self.app.emit(EVENT_NAME, ev) {
            eprintln!("emit failed: {e}");
        }
    }
}

/// `type` plus a useful sub-key: `subtype` / `request.subtype` for Claude,
/// `item.type` for Codex. E.g. `system/init`, `item.completed/agent_message`.
pub fn event_type(v: &Value) -> Option<String> {
    let t = v.get("type")?.as_str()?;
    let sub = v
        .get("subtype")
        .or_else(|| v.pointer("/request/subtype"))
        .or_else(|| v.pointer("/item/type"))
        .and_then(Value::as_str);
    Some(match sub {
        Some(s) => format!("{t}/{s}"),
        None => t.to_string(),
    })
}

#[tauri::command]
pub async fn start_session(
    app: AppHandle,
    agent: AgentKind,
    cwd: String,
    prompt: String,
) -> Result<String, String> {
    if prompt.trim().is_empty() {
        return Err("Prompt is empty.".into());
    }
    if !Path::new(&cwd).is_dir() {
        return Err(format!("Folder does not exist: {cwd}"));
    }

    // Loading the shell env can take a second; keep it off the async workers.
    let (env, env_err) = tauri::async_runtime::spawn_blocking(shell_env::get)
        .await
        .map_err(|e| e.to_string())?;
    let bin = shell_env::which(agent.binary(), &env).ok_or_else(|| {
        format!(
            "`{}` not found on PATH. Install it, or check that your login shell puts it on PATH.{}",
            agent.binary(),
            env_err
                .map(|e| format!(" (Shell env failed to load: {e})"))
                .unwrap_or_default()
        )
    })?;
    let args = agent.args(&cwd);

    let mut child = Command::new(&bin)
        .args(&args)
        .env_clear()
        .envs(&env)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Failed to start {}: {e}", bin.display()))?;

    let session_id = format!("s{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let em = SessionEmitter {
        app: app.clone(),
        session_id: session_id.clone(),
        seq: Arc::new(AtomicU64::new(0)),
    };
    em.emit(
        "started",
        None,
        json!({ "agent": agent.binary(), "binary": bin, "args": args, "cwd": cwd }),
    );

    // stdin writer
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let (stdin_tx, stdin_rx) = mpsc::unbounded_channel();
    tauri::async_runtime::spawn(stdin_writer(stdin, stdin_rx, em.clone()));
    match agent {
        AgentKind::Claude => {
            let init = json!({"type":"control_request","request_id":"init","request":{"subtype":"initialize"}});
            let user = json!({"type":"user","message":{"role":"user","content":prompt}});
            let _ = stdin_tx.send(StdinMsg::Line(init.to_string()));
            let _ = stdin_tx.send(StdinMsg::Line(user.to_string()));
        }
        AgentKind::Codex => {
            // codex exec reads the prompt to EOF.
            let _ = stdin_tx.send(StdinMsg::Line(prompt));
            let _ = stdin_tx.send(StdinMsg::Close);
        }
    }

    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let out_task = tauri::async_runtime::spawn(read_stdout(
        stdout,
        em.clone(),
        (agent == AgentKind::Claude).then(|| stdin_tx.clone()),
    ));
    let err_task = tauri::async_runtime::spawn(read_stderr(stderr, em.clone()));

    let (kill_tx, kill_rx) = oneshot::channel();
    app.state::<Sessions>().0.lock().unwrap().insert(
        session_id.clone(),
        Session {
            stdin: Some(stdin_tx),
            kill: Some(kill_tx),
        },
    );

    // Supervisor: wait for exit or a stop request, then report.
    tauri::async_runtime::spawn(async move {
        let killed;
        let status = tokio::select! {
            s = child.wait() => { killed = false; s }
            _ = kill_rx => {
                killed = true;
                let _ = child.start_kill();
                child.wait().await
            }
        };
        // Drain output before reporting exit so the order is right.
        let _ = out_task.await;
        let _ = err_task.await;
        em.app
            .state::<Sessions>()
            .0
            .lock()
            .unwrap()
            .remove(&em.session_id);
        match status {
            Ok(s) => em.emit(
                "exit",
                None,
                json!({ "code": s.code(), "success": s.success(), "stopped": killed }),
            ),
            Err(e) => em.emit("error", None, json!({ "message": format!("wait failed: {e}") })),
        }
    });

    Ok(session_id)
}

async fn stdin_writer(
    mut stdin: ChildStdin,
    mut rx: mpsc::UnboundedReceiver<StdinMsg>,
    em: SessionEmitter,
) {
    while let Some(msg) = rx.recv().await {
        match msg {
            StdinMsg::Line(mut line) => {
                line.push('\n');
                if let Err(e) = stdin.write_all(line.as_bytes()).await {
                    em.emit("error", None, json!({ "message": format!("stdin write failed: {e}") }));
                    return;
                }
                let _ = stdin.flush().await;
            }
            StdinMsg::Close => return, // dropping stdin sends EOF
        }
    }
}

async fn read_stdout(
    stdout: impl AsyncRead + Unpin,
    em: SessionEmitter,
    claude_stdin: Option<mpsc::UnboundedSender<StdinMsg>>,
) {
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) => {
                em.emit("error", None, json!({ "message": format!("stdout read failed: {e}") }));
                return;
            }
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) => {
                let et = event_type(&v);
                // Claude keeps running while stdin is open. One prompt per
                // session in the spike: close stdin after the turn result.
                if let (Some(tx), Some(t)) = (&claude_stdin, et.as_deref())
                    && t.starts_with("result")
                {
                    let _ = tx.send(StdinMsg::Close);
                }
                em.emit("event", et, v);
            }
            Err(e) => em.emit(
                "invalid_json",
                None,
                json!({ "line": line, "error": e.to_string() }),
            ),
        }
    }
}

async fn read_stderr(stderr: impl AsyncRead + Unpin, em: SessionEmitter) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if !line.trim().is_empty() {
            em.emit("stderr", None, json!({ "line": line }));
        }
    }
}

/// Answer a Claude `can_use_tool` control request.
#[tauri::command]
pub fn respond_approval(
    sessions: tauri::State<'_, Sessions>,
    session_id: String,
    request_id: String,
    allow: bool,
    input: Value,
) -> Result<(), String> {
    let map = sessions.0.lock().unwrap();
    let tx = map
        .get(&session_id)
        .and_then(|s| s.stdin.as_ref())
        .ok_or("Session is not running.")?;
    tx.send(StdinMsg::Line(approval_response(&request_id, allow, input).to_string()))
        .map_err(|_| "Session stdin is closed.".to_string())
}

pub fn approval_response(request_id: &str, allow: bool, input: Value) -> Value {
    let response = if allow {
        json!({ "behavior": "allow", "updatedInput": input })
    } else {
        json!({ "behavior": "deny", "message": "Denied by user in HumanLayer Local." })
    };
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": response }
    })
}

#[tauri::command]
pub fn stop_session(sessions: tauri::State<'_, Sessions>, session_id: String) -> Result<(), String> {
    let mut map = sessions.0.lock().unwrap();
    let kill = map
        .get_mut(&session_id)
        .and_then(|s| s.kill.take())
        .ok_or("Session is not running.")?;
    let _ = kill.send(());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_types() {
        let c = json!({"type":"system","subtype":"init"});
        assert_eq!(event_type(&c).unwrap(), "system/init");
        let r = json!({"type":"control_request","request":{"subtype":"can_use_tool"}});
        assert_eq!(event_type(&r).unwrap(), "control_request/can_use_tool");
        let x = json!({"type":"item.completed","item":{"type":"agent_message"}});
        assert_eq!(event_type(&x).unwrap(), "item.completed/agent_message");
        assert_eq!(event_type(&json!({"type":"turn.started"})).unwrap(), "turn.started");
        assert!(event_type(&json!([1])).is_none());
    }

    #[test]
    fn approval_shapes() {
        let a = approval_response("r1", true, json!({"x":1}));
        assert_eq!(a.pointer("/response/response/behavior").unwrap(), "allow");
        assert_eq!(a.pointer("/response/request_id").unwrap(), "r1");
        let d = approval_response("r1", false, Value::Null);
        assert_eq!(d.pointer("/response/response/behavior").unwrap(), "deny");
    }
}
