//! Load the user's login-shell environment.
//!
//! Apps launched from Finder or the Dock do not inherit the PATH a user sees
//! in their terminal, so `claude` and `codex` (often in ~/.local/bin or
//! Homebrew) are not found. We ask the user's shell for its environment once
//! and reuse it for every CLI we spawn. Idea ported from upstream
//! `apps/riptide-native/src/shell_env.rs`, with two changes:
//! - `-i -l` so PATH edits in `.zshrc` are picked up (not only `.zprofile`);
//! - `env -0` with a marker, so multi-line values and shell banner noise do
//!   not corrupt parsing. A timeout guards against a hanging rc file.

use std::collections::HashMap;
use std::env;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const MARKER: &str = "__HL_LOCAL_ENV_START__";
const TIMEOUT: Duration = Duration::from_secs(5);

static SHELL_ENV: OnceLock<Result<HashMap<String, String>, String>> = OnceLock::new();

/// The shell environment, or the process environment if loading failed.
/// The second value is a load error to show the user, if any.
pub fn get() -> (HashMap<String, String>, Option<String>) {
    match SHELL_ENV.get_or_init(load) {
        Ok(map) => (map.clone(), None),
        Err(e) => (env::vars().collect(), Some(e.clone())),
    }
}

fn load() -> Result<HashMap<String, String>, String> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    // Try interactive login first (matches a terminal), then plain login.
    run_shell(&shell, "-ilc").or_else(|first| {
        run_shell(&shell, "-lc").map_err(|second| format!("{first}; fallback: {second}"))
    })
}

fn run_shell(shell: &str, flags: &str) -> Result<HashMap<String, String>, String> {
    let script = format!("printf '{MARKER}'; /usr/bin/env -0");
    let mut child = Command::new(shell)
        .arg(flags)
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start {shell}: {e}"))?;

    // Read stdout on a thread so a full pipe cannot deadlock the wait loop.
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{shell} {flags} timed out after {TIMEOUT:?}"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("waiting for {shell}: {e}")),
        }
    }
    let buf = reader.join().map_err(|_| "reader thread panicked")?;
    parse(&String::from_utf8_lossy(&buf))
}

fn parse(out: &str) -> Result<HashMap<String, String>, String> {
    let (_, body) = out
        .split_once(MARKER)
        .ok_or("shell output had no environment marker")?;
    let map: HashMap<String, String> = body
        .split('\0')
        .filter_map(|entry| entry.split_once('='))
        .filter(|(k, _)| !k.is_empty() && !k.contains(char::is_whitespace))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    if map.contains_key("PATH") {
        Ok(map)
    } else {
        Err("shell environment had no PATH".into())
    }
}

/// Find an executable by name on the given PATH.
pub fn which(name: &str, env: &HashMap<String, String>) -> Option<PathBuf> {
    let path = env.get("PATH")?;
    env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiline_and_noise() {
        let out = format!("banner text\n{MARKER}PATH=/a:/b\0MULTI=x\ny\0=junk\0");
        let map = parse(&out).unwrap();
        assert_eq!(map["PATH"], "/a:/b");
        assert_eq!(map["MULTI"], "x\ny");
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn missing_marker_is_error() {
        assert!(parse("PATH=/a\0").is_err());
    }
}
