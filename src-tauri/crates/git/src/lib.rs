//! Thin wrappers around the `git` CLI. M1 needs only repo validation;
//! worktrees and diffs arrive in M3.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
}

/// Resolve `path` to the top level of the git work tree that contains it.
/// `env` is the environment to run git with (the login-shell env).
pub fn repo_toplevel(path: &Path, env: &HashMap<String, String>) -> Result<PathBuf, Error> {
    if !path.is_dir() {
        return Err(Error::NoSuchFolder(path.display().to_string()));
    }
    let git = hl_core::shell_env::which("git", env).ok_or(Error::GitNotFound)?;
    let out = Command::new(git)
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Error::Git(e.to_string()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(if stderr.contains("not a git repository") {
            Error::NotARepo(path.display().to_string())
        } else {
            Error::Git(stderr.trim().to_string())
        });
    }
    let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(PathBuf::from(top))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> HashMap<String, String> {
        std::env::vars().collect()
    }

    /// Fresh, canonical temp dir (macOS /var is a symlink to /private/var).
    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hl-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn finds_toplevel_from_subdir() {
        let d = tmp("repo");
        let ok = Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(&d)
            .status()
            .unwrap();
        assert!(ok.success());
        let sub = d.join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(repo_toplevel(&sub, &env()).unwrap(), d);
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
