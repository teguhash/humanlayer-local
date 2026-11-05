//! First-run tool checks and the "Copy diagnostics" report. The report is
//! meant to be pasted into a bug report, so secrets are scrubbed from it.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    pub name: &'static str,
    /// What the app uses it for.
    pub purpose: &'static str,
    /// The app can't work without it.
    pub required: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    /// Found but `--version` failed.
    pub error: Option<String>,
    pub install: &'static str,
}

const TOOLS: &[(&str, &str, bool, &str)] = &[
    (
        "git",
        "projects, worktrees and diffs",
        true,
        "xcode-select --install",
    ),
    (
        "claude",
        "Claude Code sessions",
        true,
        "npm install -g @anthropic-ai/claude-code",
    ),
    (
        "codex",
        "Codex sessions",
        false,
        "npm install -g @openai/codex",
    ),
    (
        "gh",
        "GitHub issues and pull requests",
        false,
        "brew install gh, then gh auth login",
    ),
];

/// First line of `<bin> --version`, with a timeout so a hung CLI can't
/// stall the check.
fn version(bin: &Path, env: &std::collections::HashMap<String, String>) -> Result<String, String> {
    let mut child = Command::new(bin)
        .arg("--version")
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            let out = child.wait_with_output().map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(if out.stdout.is_empty() {
                &out.stderr
            } else {
                &out.stdout
            })
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
            return if status.success() {
                Ok(text)
            } else {
                Err(format!("`--version` exited with {status}: {text}"))
            };
        }
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            return Err("`--version` did not finish within 10 seconds".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Look for each tool on the login shell's PATH (the same env sessions get).
pub fn check_tools() -> (Vec<Tool>, Option<String>) {
    let (env, env_err) = hl_core::shell_env::get();
    let tools = TOOLS
        .iter()
        .map(|&(name, purpose, required, install)| {
            let path = hl_core::shell_env::which(name, &env);
            let (version, error) = match &path {
                Some(p) => match version(p, &env) {
                    Ok(v) => (Some(v), None),
                    Err(e) => (None, Some(e)),
                },
                None => (None, None),
            };
            Tool {
                name,
                purpose,
                required,
                path: path.map(|p| p.display().to_string()),
                version,
                error,
                install,
            }
        })
        .collect();
    (
        tools,
        env_err.map(|e| format!("Your login shell's environment could not be loaded: {e}")),
    )
}

/// Remove things that must never be pasted into a bug report: API keys and
/// tokens, email addresses and the home folder path.
pub fn redact(text: &str, home: Option<&str>) -> String {
    let mut out = String::with_capacity(text.len());
    for word in split_keep(text) {
        out.push_str(&redact_word(word));
    }
    match home {
        Some(h) if h.len() > 1 => out.replace(h, "~"),
        _ => out,
    }
}

/// Split into runs of token characters and everything else.
fn split_keep(text: &str) -> Vec<&str> {
    let is_tok = |c: char| c.is_ascii_alphanumeric() || "_-.@+".contains(c);
    let mut parts = vec![];
    let mut start = 0;
    let mut cur: Option<bool> = None;
    for (i, c) in text.char_indices() {
        let t = is_tok(c);
        if cur.is_some_and(|k| k != t) {
            parts.push(&text[start..i]);
            start = i;
        }
        cur = Some(t);
    }
    if start < text.len() {
        parts.push(&text[start..]);
    }
    parts
}

fn redact_word(w: &str) -> String {
    const PREFIXES: &[&str] = &[
        "lin_api_",
        "lin_oauth_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "sk-ant-",
        "sk-",
    ];
    if PREFIXES
        .iter()
        .any(|p| w.starts_with(p) && w.len() >= p.len() + 8)
    {
        return "[redacted token]".into();
    }
    if let Some((user, domain)) = w.split_once('@')
        && !user.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
    {
        return "[redacted email]".into();
    }
    // Long opaque strings (hex or base64-ish) are likely secrets or ids.
    let long_opaque = w.len() >= 40
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && w.chars().any(|c| c.is_ascii_digit())
        && w.chars().any(|c| c.is_ascii_alphabetic());
    if long_opaque {
        return "[redacted]".into();
    }
    w.to_string()
}

/// The last `n` lines of the newest log file in `dir`.
pub fn log_tail(dir: &Path, n: usize) -> Option<(String, String)> {
    let newest = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?;
    let text = std::fs::read_to_string(newest.path()).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(n)..].join("\n");
    Some((newest.path().display().to_string(), tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction() {
        let s = "key lin_api_abcdefghij123 and ghp_ABCDEFGH12345678 for ann@example.com in /Users/ann/x, \
                 sha 0123456789abcdef0123456789abcdef01234567 keep ENG-123 and 2026-11-05";
        let r = redact(s, Some("/Users/ann"));
        assert!(
            !r.contains("lin_api_abc") && !r.contains("ghp_") && !r.contains("ann@"),
            "{r}"
        );
        assert!(r.contains("~/x"), "{r}");
        assert!(r.contains("[redacted]"), "{r}");
        assert!(r.contains("ENG-123") && r.contains("2026-11-05"), "{r}");
        // Short prefixed words are left (e.g. "sk-" in prose).
        assert_eq!(redact("sk-1", None), "sk-1");
    }

    #[test]
    fn tools_listed() {
        let (tools, _) = check_tools();
        assert_eq!(
            tools.iter().map(|t| t.name).collect::<Vec<_>>(),
            ["git", "claude", "codex", "gh"]
        );
        let git = &tools[0];
        assert!(git.path.is_some() && git.version.as_deref().unwrap_or("").contains("git"));
    }
}
