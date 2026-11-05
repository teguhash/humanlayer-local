//! Claude and Codex CLI adapters.
//!
//! [`AgentManager`] spawns `claude` or `codex` in JSON streaming mode and
//! hands every stdout line to an [`EventSink`] (the Tauri app forwards them
//! to the webview; M2 will also write them to SQLite). All I/O runs on tokio
//! tasks; [`AgentManager::start`] returns once the child is running.
//! Failures (CLI missing, non-zero exit, bad JSON lines, stderr output)
//! become events too, so the UI can show them.
//!
//! Still the M0 shape: one prompt per run, runs are not linked to DB
//! sessions yet. The `Agent` trait (start/send/interrupt/resume/stop) is M2.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hl_core::model::AgentKind;
use hl_core::shell_env;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

/// Flags verified against `claude --help` 2.1.314 and `codex exec --help`
/// 0.166.0. See docs/m0-findings.md.
fn cli_args(agent: AgentKind, cwd: &str) -> Vec<String> {
    let v: &[&str] = match agent {
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
        // Prompt is read from stdin ("-"), so it can start with "-" and has
        // no argv length limit. read-only sandbox until M5 decides approvals.
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

/// One message to the UI. `kind` is ours; `event_type` is the CLI's.
#[derive(Debug, Clone, Serialize)]
pub struct AgentEvent {
    pub run_id: String,
    pub seq: u64,
    /// started | event | invalid_json | stderr | exit | error
    pub kind: &'static str,
    pub event_type: Option<String>,
    pub payload: Value,
}

/// Where events go. Called from tokio worker threads, in seq order per run.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: AgentEvent);
}

impl<F: Fn(AgentEvent) + Send + Sync + 'static> EventSink for F {
    fn emit(&self, event: AgentEvent) {
        self(event)
    }
}

enum StdinMsg {
    Line(String),
    Close,
}

struct Run {
    stdin: mpsc::UnboundedSender<StdinMsg>,
    kill: Option<oneshot::Sender<()>>,
}

pub struct AgentManager {
    sink: Arc<dyn EventSink>,
    runs: Mutex<HashMap<String, Run>>,
    next_id: AtomicU64,
}

#[derive(Clone)]
struct RunEmitter {
    sink: Arc<dyn EventSink>,
    run_id: String,
    /// Held while emitting so seq order equals delivery order across the
    /// stdout, stderr and supervisor tasks.
    seq: Arc<Mutex<u64>>,
}

impl RunEmitter {
    fn emit(&self, kind: &'static str, event_type: Option<String>, payload: Value) {
        let mut seq = self.seq.lock().unwrap_or_else(|p| p.into_inner());
        self.sink.emit(AgentEvent {
            run_id: self.run_id.clone(),
            seq: *seq,
            kind,
            event_type,
            payload,
        });
        *seq += 1;
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

impl AgentManager {
    pub fn new(sink: impl EventSink) -> Arc<Self> {
        Arc::new(Self {
            sink: Arc::new(sink),
            runs: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    /// Start one prompt in `cwd`. Must be called inside a tokio runtime.
    pub async fn start(
        self: &Arc<Self>,
        agent: AgentKind,
        cwd: &str,
        prompt: &str,
    ) -> Result<String, String> {
        if prompt.trim().is_empty() {
            return Err("Prompt is empty.".into());
        }
        if !Path::new(cwd).is_dir() {
            return Err(format!("Folder does not exist: {cwd}"));
        }
        // Loading the shell env can take a second; keep it off the workers.
        let (env, env_err) = tokio::task::spawn_blocking(shell_env::get)
            .await
            .map_err(|e| e.to_string())?;
        let bin = shell_env::which(agent.as_str(), &env).ok_or_else(|| {
            format!(
                "`{}` not found on PATH. Install it, or check that your login shell puts it on PATH.{}",
                agent.as_str(),
                env_err
                    .map(|e| format!(" (Shell env failed to load: {e})"))
                    .unwrap_or_default()
            )
        })?;
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
            AgentKind::Codex => vec![StdinMsg::Line(prompt.to_string()), StdinMsg::Close],
        };
        self.spawn(Spawn {
            name: agent.as_str(),
            bin: &bin,
            args: cli_args(agent, cwd),
            env: &env,
            cwd,
            stdin_lines,
            close_on_result: agent == AgentKind::Claude,
        })
    }

    /// Answer a Claude `can_use_tool` control request.
    pub fn respond_approval(
        &self,
        run_id: &str,
        request_id: &str,
        allow: bool,
        input: Value,
    ) -> Result<(), String> {
        let runs = self.runs.lock().unwrap();
        let run = runs.get(run_id).ok_or("Run is not active.")?;
        run.stdin
            .send(StdinMsg::Line(
                approval_response(request_id, allow, input).to_string(),
            ))
            .map_err(|_| "Run stdin is closed.".to_string())
    }

    pub fn stop(&self, run_id: &str) -> Result<(), String> {
        let mut runs = self.runs.lock().unwrap();
        let kill = runs
            .get_mut(run_id)
            .and_then(|r| r.kill.take())
            .ok_or("Run is not active.")?;
        let _ = kill.send(());
        Ok(())
    }

    pub fn active_runs(&self) -> usize {
        self.runs.lock().unwrap().len()
    }

    /// Spawn a CLI and stream its output as events. Returns once the child
    /// is running; all I/O happens on background tasks.
    fn spawn(self: &Arc<Self>, s: Spawn<'_>) -> Result<String, String> {
        let mut child = Command::new(s.bin)
            .args(&s.args)
            .env_clear()
            .envs(s.env)
            .current_dir(s.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            // Own process group, so Stop also reaches tools the CLI started
            // (shells, MCP servers) that would otherwise hold our pipes open.
            .process_group(0)
            .spawn()
            .map_err(|e| format!("Failed to start {}: {e}", s.bin.display()))?;

        let run_id = format!("r{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let em = RunEmitter {
            sink: self.sink.clone(),
            run_id: run_id.clone(),
            seq: Arc::new(Mutex::new(0)),
        };
        em.emit(
            "started",
            None,
            json!({ "agent": s.name, "binary": s.bin, "args": s.args, "cwd": s.cwd }),
        );

        let stdin = child.stdin.take().ok_or("no stdin")?;
        let (stdin_tx, stdin_rx) = mpsc::unbounded_channel();
        tokio::spawn(stdin_writer(stdin, stdin_rx, em.clone()));
        for msg in s.stdin_lines {
            let _ = stdin_tx.send(msg);
        }

        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;
        let out_task = tokio::spawn(read_stdout(
            stdout,
            em.clone(),
            s.close_on_result.then(|| stdin_tx.clone()),
        ));
        let err_task = tokio::spawn(read_stderr(stderr, em.clone()));

        let (kill_tx, kill_rx) = oneshot::channel();
        self.runs.lock().unwrap().insert(
            run_id.clone(),
            Run {
                stdin: stdin_tx,
                kill: Some(kill_tx),
            },
        );

        // Supervisor: wait for exit or a stop request, then report.
        let pgid = child.id().map(|p| p as i32);
        let me = self.clone();
        tokio::spawn(async move {
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
            // Drain output before reporting exit so the order is right, but
            // do not hang if a detached grandchild still holds the pipe.
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
            me.runs.lock().unwrap().remove(&em.run_id);
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

        Ok(run_id)
    }
}

struct Spawn<'a> {
    name: &'a str,
    bin: &'a Path,
    args: Vec<String>,
    env: &'a HashMap<String, String>,
    cwd: &'a str,
    stdin_lines: Vec<StdinMsg>,
    /// Close stdin after a Claude `result` event so it exits after one turn.
    close_on_result: bool,
}

fn signal_group(pgid: Option<i32>, sig: i32) {
    if let Some(pgid) = pgid {
        // SAFETY: plain syscall; a stale pgid just returns ESRCH.
        unsafe {
            libc::killpg(pgid, sig);
        }
    }
}

async fn stdin_writer(
    mut stdin: ChildStdin,
    mut rx: mpsc::UnboundedReceiver<StdinMsg>,
    em: RunEmitter,
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

async fn read_stdout(
    stdout: impl AsyncRead + Unpin,
    em: RunEmitter,
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
                // run for now: close stdin after the turn result.
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

async fn read_stderr(stderr: impl AsyncRead + Unpin, em: RunEmitter) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if !line.trim().is_empty() {
            em.emit("stderr", None, json!({ "line": line }));
        }
    }
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

#[cfg(test)]
mod tests;
