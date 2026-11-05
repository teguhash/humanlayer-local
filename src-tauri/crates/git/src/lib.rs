//! Thin wrappers around the `git` CLI: repo validation, per-task worktrees
//! (`worktree`), their setup config (`config`), and diffs (`diff`).

pub mod config;
pub mod diff;
pub mod worktree;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub type Env = HashMap<String, String>;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error("Folder does not exist: {0}")]
    NoSuchFolder(String),
    #[error("`git` not found on PATH.")]
    GitNotFound,
    #[error("{0} is not inside a git repository.")]
    NotARepo(String),
    #[error("git failed: {0}")]
    Git(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Run git in `dir`. Exit codes in `ok_codes` (besides 0) count as success.
pub(crate) fn git_raw(dir: &Path, args: &[&str], env: &Env, ok_codes: &[i32]) -> Result<Vec<u8>> {
    let git = hl_core::shell_env::which("git", env).ok_or(Error::GitNotFound)?;
    let out = Command::new(git)
        .args(args)
        .current_dir(dir)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Error::Git(e.to_string()))?;
    let code = out.status.code().unwrap_or(-1);
    if out.status.success() || ok_codes.contains(&code) {
        Ok(out.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(Error::Git(if stderr.is_empty() {
            format!("`git {}` exited with {code}", args.join(" "))
        } else {
            stderr
        }))
    }
}

/// `git_raw` with trimmed UTF-8 output.
pub(crate) fn git(dir: &Path, args: &[&str], env: &Env) -> Result<String> {
    Ok(String::from_utf8_lossy(&git_raw(dir, args, env, &[])?)
        .trim()
        .to_string())
}

/// Resolve `path` to the top level of the git work tree that contains it.
/// `env` is the environment to run git with (the login-shell env).
pub fn repo_toplevel(path: &Path, env: &Env) -> Result<PathBuf> {
    if !path.is_dir() {
        return Err(Error::NoSuchFolder(path.display().to_string()));
    }
    match git(path, &["rev-parse", "--show-toplevel"], env) {
        Ok(top) => Ok(PathBuf::from(top)),
        Err(Error::Git(e)) if e.contains("not a git repository") => {
            Err(Error::NotARepo(path.display().to_string()))
        }
        Err(e) => Err(e),
    }
}

/// Make git ignore `rel` (a path inside `cwd`'s work tree) by adding
/// `pattern` to the repo's `info/exclude`, which is local and never
/// committed (shared by all worktrees). Does nothing if already ignored.
pub fn ensure_ignored(cwd: &Path, rel: &str, pattern: &str, env: &Env) -> Result<()> {
    // Exit 0: ignored. Exit 1: not ignored.
    if git(cwd, &["check-ignore", "-q", rel], env).is_ok() {
        return Ok(());
    }
    let common = git(cwd, &["rev-parse", "--git-common-dir"], env)?;
    let info = cwd.join(common).join("info");
    std::fs::create_dir_all(&info).map_err(|e| Error::Git(e.to_string()))?;
    let path = info.join("exclude");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if text.lines().any(|l| l.trim() == pattern) {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("# HumanLayer Local (linked folders)\n{pattern}\n"));
    std::fs::write(&path, text).map_err(|e| Error::Git(e.to_string()))
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    pub fn env() -> Env {
        std::env::vars().collect()
    }

    /// Fresh, canonical temp dir (macOS /var is a symlink to /private/var).
    pub fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hl-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    pub fn sh(dir: &Path, cmd: &str) {
        let ok = Command::new("sh")
            .args(["-c", cmd])
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .status()
            .unwrap();
        assert!(ok.success(), "{cmd}");
    }

    /// A repo on branch `main` with one commit.
    pub fn repo(name: &str) -> PathBuf {
        let d = tmp(name);
        sh(
            &d,
            "git init -q -b main && printf 'one\\ntwo\\n' > a.txt && git add . && git commit -qm init",
        );
        d
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn finds_toplevel_from_subdir() {
        let d = tmp("repo");
        sh(&d, "git init -q");
        let sub = d.join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(repo_toplevel(&sub, &env()).unwrap(), d);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn ignore_rule_goes_to_info_exclude_once() {
        let d = repo("ign");
        let env = env();
        std::fs::create_dir_all(d.join(".humanlayer/tasks/x")).unwrap();
        std::fs::write(d.join(".humanlayer/tasks/x/a.md"), "a").unwrap();
        ensure_ignored(&d, ".humanlayer/tasks/x", "/.humanlayer/tasks/", &env).unwrap();
        ensure_ignored(&d, ".humanlayer/tasks/x", "/.humanlayer/tasks/", &env).unwrap();
        let ex = std::fs::read_to_string(d.join(".git/info/exclude")).unwrap();
        assert_eq!(ex.matches("/.humanlayer/tasks/").count(), 1, "{ex}");
        assert_eq!(git(&d, &["status", "--porcelain"], &env).unwrap(), "");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn rejects_non_repo_and_missing() {
        let d = tmp("plain");
        assert_eq!(
            repo_toplevel(&d, &env()),
            Err(Error::NotARepo(d.display().to_string()))
        );
        let missing = d.join("nope");
        assert!(matches!(
            repo_toplevel(&missing, &env()),
            Err(Error::NoSuchFolder(_))
        ));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
