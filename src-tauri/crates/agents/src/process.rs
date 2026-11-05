//! Spawn a CLI and stream its output. Agent-agnostic: every stdout line is
//! parsed as JSON and reported through one callback, called in order.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone)]
pub enum ProcEvent {
    Started {
        binary: String,
        args: Vec<String>,
        cwd: String,
    },
    Line(Value),
    InvalidJson {
        line: String,
        error: String,
    },
    Stderr(String),
    Error(String),
    Exit {
        code: Option<i32>,
        success: bool,
        stopped: bool,
    },
}

impl ProcEvent {
    /// Stored event kind and payload for everything except `Line`.
    pub fn to_record(&self) -> (&'static str, Value) {
        match self {
            ProcEvent::Started { binary, args, cwd } => (
                "process_started",
                json!({ "binary": binary, "args": args, "cwd": cwd }),
            ),
            ProcEvent::Line(v) => ("cli", v.clone()),
            ProcEvent::InvalidJson { line, error } => {
                ("invalid_json", json!({ "line": line, "error": error }))
            }
            ProcEvent::Stderr(line) => ("stderr", json!({ "line": line })),
            ProcEvent::Error(message) => ("error", json!({ "message": message })),
            ProcEvent::Exit {
                code,
                success,
                stopped,
            } => (
                "process_exit",
                json!({ "code": code, "success": success, "stopped": stopped }),
            ),
        }
    }
}

enum StdinMsg {
    Line(String),
    Close,
}

/// Handle to a running process.
pub struct Proc {
    stdin: mpsc::UnboundedSender<StdinMsg>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
}

impl Proc {
    pub fn send_line(&self, line: String) -> Result<(), String> {
        self.stdin
            .send(StdinMsg::Line(line))
            .map_err(|_| "The agent process is no longer reading input.".to_string())
    }

    /// EOF on stdin. Claude exits after its current turn; Codex starts.
    pub fn close_stdin(&self) {
        let _ = self.stdin.send(StdinMsg::Close);
    }

    /// SIGTERM the process group, SIGKILL after 3 s.
    pub fn stop(&self) {
        if let Some(k) = self.kill.lock().unwrap().take() {
            let _ = k.send(());
        }
    }
}

type Callback = Arc<Mutex<Box<dyn FnMut(ProcEvent) + Send>>>;

/// Spawn `bin`. Must be called inside a tokio runtime. `on_event` gets
/// `Started` before this returns and `Exit` last; calls never overlap.
pub fn spawn(
    bin: &Path,
    args: &[String],
    env: &HashMap<String, String>,
    cwd: &str,
    on_event: impl FnMut(ProcEvent) + Send + 'static,
) -> Result<Arc<Proc>, String> {
    let mut child = Command::new(bin)
        .args(args)
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

    let cb: Callback = Arc::new(Mutex::new(Box::new(on_event)));
    let emit = {
        let cb = cb.clone();
        move |ev: ProcEvent| (cb.lock().unwrap_or_else(|p| p.into_inner()))(ev)
    };
    emit(ProcEvent::Started {
        binary: bin.display().to_string(),
        args: args.to_vec(),
        cwd: cwd.to_string(),
    });

    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let (stdin_tx, stdin_rx) = mpsc::unbounded_channel();
    let (kill_tx, kill_rx) = oneshot::channel();

    let emit = Arc::new(emit);
    tokio::spawn(stdin_writer(stdin, stdin_rx, emit.clone()));
    let out_task = tokio::spawn(read_stdout(stdout, emit.clone()));
    let err_task = tokio::spawn(read_stderr(stderr, emit.clone()));

    let pgid = child.id().map(|p| p as i32);
    tokio::spawn(async move {
        let stopped;
        let status = tokio::select! {
            s = child.wait() => { stopped = false; s }
            _ = kill_rx => {
                stopped = true;
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
        if stopped {
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
            emit(ProcEvent::Error(
                "output pipes still open 5s after exit; a child process may still be running"
                    .into(),
            ));
        }
        emit(match status {
            Ok(s) => ProcEvent::Exit {
                code: s.code(),
                success: s.success(),
                stopped,
            },
            Err(e) => ProcEvent::Error(format!("wait failed: {e}")),
        });
    });

    Ok(Arc::new(Proc {
        stdin: stdin_tx,
        kill: Mutex::new(Some(kill_tx)),
    }))
}

fn signal_group(pgid: Option<i32>, sig: i32) {
    if let Some(pgid) = pgid {
        // SAFETY: plain syscall; a stale pgid just returns ESRCH.
        unsafe {
            libc::killpg(pgid, sig);
        }
    }
}

type Emit = Arc<dyn Fn(ProcEvent) + Send + Sync>;

async fn stdin_writer(
    mut stdin: ChildStdin,
    mut rx: mpsc::UnboundedReceiver<StdinMsg>,
    emit: Emit,
) {
    while let Some(msg) = rx.recv().await {
        match msg {
            StdinMsg::Line(mut line) => {
                line.push('\n');
                if let Err(e) = stdin.write_all(line.as_bytes()).await {
                    emit(ProcEvent::Error(format!("stdin write failed: {e}")));
                    return;
                }
                let _ = stdin.flush().await;
            }
            StdinMsg::Close => return, // dropping stdin sends EOF
        }
    }
}

async fn read_stdout(stdout: impl AsyncRead + Unpin, emit: Emit) {
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) => {
                emit(ProcEvent::Error(format!("stdout read failed: {e}")));
                return;
            }
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        emit(match serde_json::from_str::<Value>(line) {
            Ok(v) => ProcEvent::Line(v),
            Err(e) => ProcEvent::InvalidJson {
                line: line.to_string(),
                error: e.to_string(),
            },
        });
    }
}

async fn read_stderr(stderr: impl AsyncRead + Unpin, emit: Emit) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if !line.trim().is_empty() {
            emit(ProcEvent::Stderr(line));
        }
    }
}
