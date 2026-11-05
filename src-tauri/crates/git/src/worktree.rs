//! Create, set up and remove per-task worktrees.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::WorkspaceConfig;
use crate::{Env, Error, Result, git};

#[derive(Debug, Clone, PartialEq)]
pub struct Created {
    pub path: PathBuf,
    pub branch: String,
    /// Branch name the worktree started from, or a commit if HEAD was detached.
    pub base_ref: String,
    pub base_commit: String,
    /// Human-readable record of what was done, for the task's log.
    pub log: String,
}

/// What to check out.
pub enum Branch<'a> {
    /// A new branch named from `slug` via the config's template. If the
    /// branch or folder is taken, `-2`, `-3`, ... is appended to the slug.
    New { slug: &'a str },
    /// An existing branch (restoring an archived task).
    Existing { branch: &'a str, slug: &'a str },
}

pub fn create(repo: &Path, cfg: &WorkspaceConfig, which: Branch, env: &Env) -> Result<Created> {
    if cfg.disabled {
        return Err(Error::Invalid(
            "Worktrees are turned off for this project (\"disabled\": true in .humanlayer/workspace.json).".into(),
        ));
    }
    let mut log = String::new();
    if !cfg.sources.is_empty() {
        log.push_str(&format!("Config: {}\n", cfg.sources.join(", ")));
    }

    let (path, branch, base_ref, base_commit) = match which {
        Branch::New { slug } => {
            let base_commit = git(
                repo,
                &[
                    "rev-parse",
                    "--verify",
                    &format!("{}^{{commit}}", cfg.source_ref),
                ],
                env,
            )
            .map_err(|_| Error::Invalid(format!("Base ref `{}` not found.", cfg.source_ref)))?;
            let base_ref = if cfg.source_ref == "HEAD" {
                git(repo, &["symbolic-ref", "--short", "-q", "HEAD"], env)
                    .unwrap_or_else(|_| base_commit.clone())
            } else {
                cfg.source_ref.clone()
            };
            let (path, branch) = free_name(repo, cfg, slug, env)?;
            mkdir_parent(&path)?;
            git(
                repo,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    &path.to_string_lossy(),
                    &base_commit,
                ],
                env,
            )?;
            log.push_str(&format!(
                "Created branch {branch} from {base_ref} ({})\n",
                short(&base_commit)
            ));
            (path, branch, base_ref, base_commit)
        }
        Branch::Existing { branch, slug } => {
            if !branch_exists(repo, branch, env)? {
                return Err(Error::Invalid(format!("Branch {branch} no longer exists.")));
            }
            let mut path = cfg.worktree_path(repo, slug);
            let mut n = 2;
            while path.exists() {
                path = cfg.worktree_path(repo, &format!("{slug}-{n}"));
                n += 1;
            }
            mkdir_parent(&path)?;
            git(
                repo,
                &["worktree", "add", &path.to_string_lossy(), branch],
                env,
            )?;
            log.push_str(&format!("Checked out existing branch {branch}\n"));
            let head = git(&path, &["rev-parse", "HEAD"], env)?;
            (path, branch.to_string(), String::new(), head)
        }
    };
    log.push_str(&format!("Worktree: {}\n", path.display()));

    let copied = copy_globs(repo, &path, &cfg.copy_globs);
    if !copied.is_empty() {
        log.push_str(&format!("Copied: {}\n", copied.join(", ")));
    }
    Ok(Created {
        path,
        branch,
        base_ref,
        base_commit,
        log,
    })
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

fn mkdir_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Invalid(format!("Cannot create {}: {e}", parent.display())))?;
    }
    Ok(())
}

fn branch_exists(repo: &Path, branch: &str, env: &Env) -> Result<bool> {
    let r = format!("refs/heads/{branch}");
    match git(repo, &["show-ref", "--verify", "--quiet", &r], env) {
        Ok(_) => Ok(true),
        Err(Error::Git(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// First slug, slug-2, slug-3, ... whose branch and folder are both free.
fn free_name(
    repo: &Path,
    cfg: &WorkspaceConfig,
    slug: &str,
    env: &Env,
) -> Result<(PathBuf, String)> {
    for n in 1..100 {
        let s = if n == 1 {
            slug.to_string()
        } else {
            format!("{slug}-{n}")
        };
        let branch = cfg.branch(repo, &s);
        git(repo, &["check-ref-format", "--branch", &branch], env)
            .map_err(|_| Error::Invalid(format!("`{branch}` is not a valid branch name.")))?;
        let path = cfg.worktree_path(repo, &s);
        if !path.exists() && !branch_exists(repo, &branch, env)? {
            return Ok((path, branch));
        }
    }
    Err(Error::Invalid(format!("No free branch name for {slug}.")))
}

/// Copy files matching `globs` (relative to the repo) into the worktree,
/// keeping their paths. Files that already exist there (tracked ones) are
/// left alone. Returns the copied relative paths.
pub fn copy_globs(repo: &Path, wt: &Path, globs: &[String]) -> Vec<String> {
    let mut copied = vec![];
    for g in globs {
        let pattern = repo.join(g).to_string_lossy().into_owned();
        let Ok(paths) = glob::glob(&pattern) else {
            continue;
        };
        for src in paths.flatten() {
            let Ok(rel) = src.strip_prefix(repo) else {
                continue;
            };
            let dst = wt.join(rel);
            if !src.is_file() || dst.exists() {
                continue;
            }
            if let Some(p) = dst.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            if std::fs::copy(&src, &dst).is_ok() {
                copied.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    copied
}

const LOG_LIMIT: usize = 64 * 1024;

/// Run the setup command with `sh -c` in the worktree. Returns success and
/// the combined output (last 64 KB). Blocks until it exits.
pub fn run_setup(wt: &Path, command: &str, env: &Env) -> (bool, String) {
    let res = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("{{\n{command}\n}} 2>&1"))
        .current_dir(wt)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .output();
    match res {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            if text.len() > LOG_LIMIT {
                let mut cut = text.len() - LOG_LIMIT;
                while !text.is_char_boundary(cut) {
                    cut += 1;
                }
                text = format!("[... earlier output cut ...]\n{}", &text[cut..]);
            }
            if !out.status.success() {
                text.push_str(&format!(
                    "\n[setup command exited with {}]",
                    out.status
                        .code()
                        .map_or("a signal".to_string(), |c| format!("code {c}"))
                ));
            }
            (out.status.success(), text)
        }
        Err(e) => (false, format!("Could not run setup command: {e}")),
    }
}

/// Files with uncommitted changes (including untracked) in the worktree.
pub fn dirty_files(wt: &Path, env: &Env) -> Result<Vec<String>> {
    let out = git(wt, &["status", "--porcelain"], env)?;
    Ok(out
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| l[3..].to_string())
        .collect())
}

/// Remove the worktree folder. `force` discards uncommitted changes. The
/// branch is kept. A folder that is already gone is pruned from git's list.
pub fn remove(repo: &Path, wt: &Path, force: bool, env: &Env) -> Result<()> {
    if !wt.exists() {
        git(repo, &["worktree", "prune"], env)?;
        return Ok(());
    }
    let p = wt.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&p);
    git(repo, &args, env).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkspaceConfig;
    use crate::testutil::*;

    fn cfg(repo: &Path) -> WorkspaceConfig {
        let mut c = WorkspaceConfig::load(repo).unwrap();
        c.path_template = format!("{}-wt/{{{{ TASKSLUG }}}}", repo.display());
        c
    }

    #[test]
    fn create_copy_setup_remove_restore() {
        let repo = repo("wt");
        let wt_root = PathBuf::from(format!("{}-wt", repo.display()));
        let _ = std::fs::remove_dir_all(&wt_root);
        std::fs::write(repo.join(".env"), "SECRET=1").unwrap();
        std::fs::write(repo.join(".gitignore"), ".env\n").unwrap();
        sh(&repo, "git add .gitignore && git commit -qm ignore");
        let mut c = cfg(&repo);
        let env = env();

        let a = create(&repo, &c, Branch::New { slug: "fix-it" }, &env).unwrap();
        assert_eq!(a.branch, "fix-it");
        assert_eq!(a.base_ref, "main");
        assert_eq!(a.path, wt_root.join("fix-it"));
        assert_eq!(
            std::fs::read_to_string(a.path.join(".env")).unwrap(),
            "SECRET=1"
        );
        assert!(a.log.contains("Copied: .env"), "{}", a.log);

        // Same slug again gets a suffix.
        let b = create(&repo, &c, Branch::New { slug: "fix-it" }, &env).unwrap();
        assert_eq!(b.branch, "fix-it-2");

        let (ok, out) = run_setup(&a.path, "echo hi; echo err >&2; pwd", &env);
        assert!(ok);
        assert!(
            out.contains("hi") && out.contains("err") && out.contains("fix-it"),
            "{out}"
        );
        let (ok, out) = run_setup(&a.path, "exit 3", &env);
        assert!(!ok && out.contains("code 3"), "{out}");

        assert!(dirty_files(&a.path, &env).unwrap().is_empty());
        std::fs::write(a.path.join("new.txt"), "x").unwrap();
        assert_eq!(dirty_files(&a.path, &env).unwrap(), vec!["new.txt"]);
        assert!(remove(&repo, &a.path, false, &env).is_err());
        remove(&repo, &a.path, true, &env).unwrap();
        assert!(!a.path.exists());

        // Restore on the kept branch.
        let r = create(
            &repo,
            &c,
            Branch::Existing {
                branch: "fix-it",
                slug: "fix-it",
            },
            &env,
        )
        .unwrap();
        assert_eq!(r.path, a.path);
        assert!(
            create(
                &repo,
                &c,
                Branch::Existing {
                    branch: "nope",
                    slug: "x"
                },
                &env
            )
            .is_err()
        );

        // Already-deleted folder: remove just prunes.
        std::fs::remove_dir_all(&b.path).unwrap();
        remove(&repo, &b.path, false, &env).unwrap();

        c.disabled = true;
        assert!(create(&repo, &c, Branch::New { slug: "z" }, &env).is_err());
        c.disabled = false;
        c.source_ref = "no-such-ref".into();
        assert!(create(&repo, &c, Branch::New { slug: "z" }, &env).is_err());

        std::fs::remove_dir_all(&repo).unwrap();
        std::fs::remove_dir_all(&wt_root).unwrap();
    }
}
