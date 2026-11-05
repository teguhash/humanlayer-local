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
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, Runtime};
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

struct SessionEmitter<R: Runtime> {
    app: AppHandle<R>,
    session_id: String,
    /// Held while emitting so seq order equals delivery order across the
    /// stdout, stderr and supervisor tasks.
    seq: Arc<Mutex<u64>>,
}

// Manual impl: derive(Clone) would wrongly require `R: Clone`.
impl<R: Runtime> Clone for SessionEmitter<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            session_id: self.session_id.clone(),
            seq: self.seq.clone(),
        }
    }
}

impl<R: Runtime> SessionEmitter<R> {
    fn emit(&self, kind: &'static str, event_type: Option<String>, payload: Value) {
        let mut seq = self.seq.lock().unwrap();
        let ev = AgentEvent {
            session_id: self.session_id.clone(),
            seq: *seq,
            kind,
            event_type,
            payload,
        };
        *seq += 1;
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
pub async fn start_session<R: Runtime>(
    app: AppHandle<R>,
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
    let stdin_lines = match agent {
        AgentKind::Claude => vec![
            StdinMsg::Line(
                json!({"type":"control_request","request_id":"init","request":{"subtype":"initialize"}})
                    .to_string(),
            ),
            StdinMsg::Line(
                json!({"type":"user","message":{"role":"user","content":prompt}}).to_string(),
            ),
        ],
        // codex exec reads the prompt to EOF.
        AgentKind::Codex => vec![StdinMsg::Line(prompt), StdinMsg::Close],
    };
    spawn_process(
        app,
        agent.binary(),
        &bin,
        args,
        &env,
        &cwd,
        stdin_lines,
        agent == AgentKind::Claude,
    )
}

/// Spawn a CLI and stream its output as events. Returns once the child is
/// running; all I/O happens on background tasks. `close_on_result` closes
/// stdin after a Claude `result` event so the CLI exits after one turn.
#[allow(clippy::too_many_arguments)]
fn spawn_process<R: Runtime>(
    app: AppHandle<R>,
    agent_name: &str,
    bin: &Path,
    args: Vec<String>,
    env: &HashMap<String, String>,
    cwd: &str,
    stdin_lines: Vec<StdinMsg>,
    close_on_result: bool,
) -> Result<String, String> {
    let mut child = Command::new(bin)
        .args(&args)
        .env_clear()
        .envs(env)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // Own process group, so Stop also reaches tools the CLI started
        // (shells, MCP servers) that would otherwise hold our pipes open.
        .process_group(0)
        .spawn()
        .map_err(|e| format!("Failed to start {}: {e}", bin.display()))?;

    let session_id = format!("s{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let em = SessionEmitter {
        app: app.clone(),
        session_id: session_id.clone(),
        seq: Arc::new(Mutex::new(0)),
    };
    em.emit(
        "started",
        None,
        json!({ "agent": agent_name, "binary": bin, "args": args, "cwd": cwd }),
    );

    // stdin writer
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let (stdin_tx, stdin_rx) = mpsc::unbounded_channel();
    tauri::async_runtime::spawn(stdin_writer(stdin, stdin_rx, em.clone()));
    for msg in stdin_lines {
        let _ = stdin_tx.send(msg);
    }

    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let out_task = tauri::async_runtime::spawn(read_stdout(
        stdout,
        em.clone(),
        close_on_result.then(|| stdin_tx.clone()),
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
    let pgid = child.id().map(|p| p as i32);
    tauri::async_runtime::spawn(async move {
        let killed;
        let status = tokio::select! {
            s = child.wait() => { killed = false; s }
            _ = kill_rx => {
                killed = true;
                signal_group(pgid, libc::SIGTERM);
                match tokio::time::timeout(Duration::from_secs(3), child.wait()).await {
                    Ok(s) => s,
                    Err(_) => {
                        signal_group(pgid, libc::SIGKILL);
                        child.wait().await
                    }
                }
            }
        };
        if killed {
            // Leftover grandchildren would keep stdout open forever.
            signal_group(pgid, libc::SIGKILL);
        }
        // Drain output before reporting exit so the order is right, but do
        // not hang if a detached grandchild still holds the pipe.
        let drain = async {
            let _ = out_task.await;
            let _ = err_task.await;
        };
        if tokio::time::timeout(Duration::from_secs(5), drain)
            .await
            .is_err()
        {
            em.emit(
                "error",
                None,
                json!({ "message": "output pipes still open 5s after exit; a child process may still be running" }),
            );
        }
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
            Err(e) => em.emit(
                "error",
                None,
                json!({ "message": format!("wait failed: {e}") }),
            ),
        }
    });

    Ok(session_id)
}

fn signal_group(pgid: Option<i32>, sig: i32) {
    if let Some(pgid) = pgid {
        // SAFETY: plain syscall; a stale pgid just returns ESRCH.
        unsafe {
            libc::killpg(pgid, sig);
        }
    }
}

async fn stdin_writer<R: Runtime>(
    mut stdin: ChildStdin,
    mut rx: mpsc::UnboundedReceiver<StdinMsg>,
    em: SessionEmitter<R>,
) {
    while let Some(msg) = rx.recv().await {
        match msg {
            StdinMsg::Line(mut line) => {
                line.push('\n');
                if let Err(e) = stdin.write_all(line.as_bytes()).await {
                    em.emit(
                        "error",
                        None,
                        json!({ "message": format!("stdin write failed: {e}") }),
                    );
                    return;
                }
                let _ = stdin.flush().await;
            }
            StdinMsg::Close => return, // dropping stdin sends EOF
        }
    }
}

async fn read_stdout<R: Runtime>(
    stdout: impl AsyncRead + Unpin,
    em: SessionEmitter<R>,
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
                em.emit(
                    "error",
                    None,
                    json!({ "message": format!("stdout read failed: {e}") }),
                );
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

async fn read_stderr<R: Runtime>(stderr: impl AsyncRead + Unpin, em: SessionEmitter<R>) {
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
    send_approval(&sessions, &session_id, &request_id, allow, input)
}

fn send_approval(
    sessions: &Sessions,
    session_id: &str,
    request_id: &str,
    allow: bool,
    input: Value,
) -> Result<(), String> {
    let map = sessions.0.lock().unwrap();
    let tx = map
        .get(session_id)
        .and_then(|s| s.stdin.as_ref())
        .ok_or("Session is not running.")?;
    tx.send(StdinMsg::Line(
        approval_response(request_id, allow, input).to_string(),
    ))
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
pub fn stop_session(
    sessions: tauri::State<'_, Sessions>,
    session_id: String,
) -> Result<(), String> {
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
        assert_eq!(
            event_type(&json!({"type":"turn.started"})).unwrap(),
            "turn.started"
        );
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

    // ---- Runtime tests against Tauri's mock runtime ----

    use std::sync::mpsc as std_mpsc;
    use std::time::Instant;
    use tauri::Listener;
    use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};

    fn mock_app() -> (tauri::App<MockRuntime>, std_mpsc::Receiver<Value>) {
        let app = mock_builder()
            .manage(Sessions::default())
            .build(mock_context(noop_assets()))
            .unwrap();
        let (tx, rx) = std_mpsc::channel();
        app.listen_any(EVENT_NAME, move |e| {
            let _ = tx.send(serde_json::from_str(e.payload()).unwrap());
        });
        (app, rx)
    }

    /// Collect events for one session until its exit event. On each event,
    /// `on_event` may react (e.g. answer an approval).
    fn collect(
        rx: &std_mpsc::Receiver<Value>,
        session_id: &str,
        timeout: Duration,
        mut on_event: impl FnMut(&Value),
    ) -> Vec<Value> {
        let deadline = Instant::now() + timeout;
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = rx.recv_timeout(left).expect("timed out waiting for exit");
            if ev["session_id"] != session_id {
                continue;
            }
            on_event(&ev);
            let done = ev["kind"] == "exit";
            out.push(ev);
            if done {
                return out;
            }
        }
    }

    fn kinds(evs: &[Value]) -> Vec<String> {
        evs.iter()
            .map(|e| match e["event_type"].as_str() {
                Some(t) => format!("{}:{t}", e["kind"].as_str().unwrap()),
                None => e["kind"].as_str().unwrap().to_string(),
            })
            .collect()
    }

    /// spawn_process must run inside the tokio runtime, as it does when
    /// called from the async start_session command.
    #[allow(clippy::too_many_arguments)]
    fn sh_spawn(
        app: AppHandle<MockRuntime>,
        name: &str,
        bin: &Path,
        args: Vec<String>,
        env: &HashMap<String, String>,
        cwd: &str,
        stdin_lines: Vec<StdinMsg>,
        close_on_result: bool,
    ) -> String {
        tauri::async_runtime::block_on(async {
            spawn_process(app, name, bin, args, env, cwd, stdin_lines, close_on_result)
        })
        .unwrap()
    }

    #[test]
    fn invalid_json_stderr_and_nonzero_exit_become_events() {
        let (app, rx) = mock_app();
        let script = r#"read x; echo 'not json'; printf '{"type":"hi","got":"%s"}\n' "$x"; echo oops >&2; exit 3"#;
        let id = sh_spawn(
            app.handle().clone(),
            "sh",
            Path::new("/bin/sh"),
            vec!["-c".into(), script.into()],
            &std::env::vars().collect(),
            "/tmp",
            vec![StdinMsg::Line("ping".into()), StdinMsg::Close],
            false,
        );
        let evs = collect(&rx, &id, Duration::from_secs(10), |_| {});
        let k = kinds(&evs);
        assert_eq!(k.first().unwrap(), "started");
        assert!(k.contains(&"invalid_json".to_string()), "{k:?}");
        assert!(k.contains(&"stderr".to_string()), "{k:?}");
        let hi = evs.iter().find(|e| e["event_type"] == "hi").unwrap();
        assert_eq!(hi["payload"]["got"], "ping");
        let exit = evs.last().unwrap();
        assert_eq!(exit["payload"]["code"], 3);
        assert_eq!(exit["payload"]["success"], false);
        // seq is strictly increasing within a session
        let seqs: Vec<u64> = evs.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
    }

    #[test]
    fn bad_folder_and_empty_prompt_are_errors() {
        let (app, _rx) = mock_app();
        let h = app.handle().clone();
        let r = tauri::async_runtime::block_on(start_session(
            h.clone(),
            AgentKind::Codex,
            "/definitely/not/here".into(),
            "hi".into(),
        ));
        assert!(r.unwrap_err().contains("Folder does not exist"));
        let r = tauri::async_runtime::block_on(start_session(
            h,
            AgentKind::Codex,
            "/tmp".into(),
            " ".into(),
        ));
        assert!(r.unwrap_err().contains("empty"));
    }

    #[test]
    fn stop_kills_a_running_process() {
        let (app, rx) = mock_app();
        let id = sh_spawn(
            app.handle().clone(),
            "sh",
            Path::new("/bin/sh"),
            vec!["-c".into(), "echo '{\"type\":\"up\"}'; sleep 30".into()],
            &std::env::vars().collect(),
            "/tmp",
            vec![],
            false,
        );
        let sessions = app.state::<Sessions>();
        let mut stopped = false;
        let evs = collect(&rx, &id, Duration::from_secs(10), |e| {
            if e["event_type"] == "up" && !stopped {
                stopped = true;
                let kill = sessions
                    .0
                    .lock()
                    .unwrap()
                    .get_mut(&id)
                    .unwrap()
                    .kill
                    .take()
                    .unwrap();
                kill.send(()).unwrap();
            }
        });
        assert_eq!(evs.last().unwrap()["payload"]["stopped"], true);
        assert!(sessions.0.lock().unwrap().is_empty());
    }

    fn scratch_dir(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("hl-local-e2e-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    /// Real `claude` CLI: needs it installed and logged in. Costs tokens.
    /// cargo test -- --ignored --nocapture e2e_claude
    #[test]
    #[ignore]
    fn e2e_claude_with_approval() {
        let (app, rx) = mock_app();
        let dir = scratch_dir("claude");
        let t0 = Instant::now();
        let id = tauri::async_runtime::block_on(start_session(
            app.handle().clone(),
            AgentKind::Claude,
            dir.clone(),
            "Use the Write tool to create e2e.txt containing exactly: ok. Then stop.".into(),
        ))
        .unwrap();
        println!("start_session returned in {:?}", t0.elapsed());
        let sessions = app.state::<Sessions>();
        let mut asked = 0;
        let evs = collect(&rx, &id, Duration::from_secs(180), |e| {
            if e["event_type"] == "control_request/can_use_tool" {
                asked += 1;
                let p = &e["payload"];
                send_approval(
                    &sessions,
                    &id,
                    p["request_id"].as_str().unwrap(),
                    true,
                    p["request"]["input"].clone(),
                )
                .unwrap();
            }
        });
        println!("{:#?}", kinds(&evs));
        println!("first event after {:?}", t0.elapsed());
        assert!(asked >= 1, "no approval request seen");
        assert!(kinds(&evs).iter().any(|k| k == "event:result/success"));
        assert_eq!(evs.last().unwrap()["payload"]["success"], true);
        let body = std::fs::read_to_string(format!("{dir}/e2e.txt")).unwrap();
        assert_eq!(body.trim(), "ok");
    }

    /// Real `codex` CLI: needs it installed and logged in. Costs tokens.
    #[test]
    #[ignore]
    fn e2e_codex() {
        let (app, rx) = mock_app();
        let dir = scratch_dir("codex");
        let id = tauri::async_runtime::block_on(start_session(
            app.handle().clone(),
            AgentKind::Codex,
            dir,
            "Reply with exactly: pong".into(),
        ))
        .unwrap();
        let evs = collect(&rx, &id, Duration::from_secs(180), |_| {});
        println!("{:#?}", kinds(&evs));
        let msg = evs
            .iter()
            .find(|e| e["event_type"] == "item.completed/agent_message")
            .expect("no agent_message");
        assert!(
            msg["payload"]["item"]["text"]
                .as_str()
                .unwrap()
                .contains("pong")
        );
        assert_eq!(evs.last().unwrap()["payload"]["success"], true);
    }
}
