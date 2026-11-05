//! What a task changed: the worktree (committed and uncommitted, plus
//! untracked files) compared with where its branch left the base branch.

use std::path::Path;

use serde::Serialize;

use crate::{Env, Result, git, git_raw};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FileChange {
    pub path: String,
    /// Set for renames.
    pub old_path: Option<String>,
    /// added, modified, deleted, renamed, copied, type_changed, untracked
    pub status: String,
    /// None for binary files.
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DiffSummary {
    /// What we compared against, for display ("main").
    pub base: String,
    /// The merge base actually used.
    pub base_commit: String,
    pub files: Vec<FileChange>,
}

/// Where the task's branch left `base_ref`. Falls back to `base_commit`
/// when the base branch is gone or unrelated.
pub fn merge_base(wt: &Path, base_ref: &str, base_commit: &str, env: &Env) -> String {
    if !base_ref.is_empty()
        && let Ok(mb) = git(wt, &["merge-base", base_ref, "HEAD"], env)
        && !mb.is_empty()
    {
        return mb;
    }
    base_commit.to_string()
}

pub fn summary(wt: &Path, base_ref: &str, base_commit: &str, env: &Env) -> Result<DiffSummary> {
    let mb = merge_base(wt, base_ref, base_commit, env);
    let mut files = tracked(wt, &mb, env)?;
    for path in untracked(wt, env)? {
        let additions = std::fs::read(wt.join(&path))
            .ok()
            .filter(|b| !b.contains(&0))
            .map(|b| count_lines(&b));
        files.push(FileChange {
            path,
            old_path: None,
            status: "untracked".into(),
            additions,
            deletions: additions.map(|_| 0),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(DiffSummary {
        base: if base_ref.is_empty() {
            mb[..mb.len().min(8)].to_string()
        } else {
            base_ref.to_string()
        },
        base_commit: mb,
        files,
    })
}

fn count_lines(b: &[u8]) -> u64 {
    let n = b.iter().filter(|&&c| c == b'\n').count() as u64;
    if b.last().is_some_and(|&c| c != b'\n') { n + 1 } else { n }
}

fn tracked(wt: &Path, mb: &str, env: &Env) -> Result<Vec<FileChange>> {
    // --numstat -z: "add\tdel\tpath\0", or for renames "add\tdel\t\0old\0new\0".
    // --name-status -z: "M\0path\0", or "R100\0old\0new\0".
    let names = git_raw(wt, &["diff", "-M", "--name-status", "-z", mb], env, &[])?;
    let nums = git_raw(wt, &["diff", "-M", "--numstat", "-z", mb], env, &[])?;
    let names: Vec<String> = split0(&names);
    let nums: Vec<String> = split0(&nums);

    let mut out = vec![];
    let mut i = 0;
    let mut j = 0;
    while i < names.len() {
        let code = names[i].clone();
        let kind = code.chars().next().unwrap_or('M');
        let two_paths = matches!(kind, 'R' | 'C');
        let (old_path, path) = if two_paths {
            (Some(names[i + 1].clone()), names.get(i + 2).cloned().unwrap_or_default())
        } else {
            (None, names.get(i + 1).cloned().unwrap_or_default())
        };
        i += if two_paths { 3 } else { 2 };

        let (mut additions, mut deletions) = (None, None);
        if let Some(rec) = nums.get(j) {
            let mut parts = rec.splitn(3, '\t');
            additions = parts.next().and_then(|s| s.parse().ok());
            deletions = parts.next().and_then(|s| s.parse().ok());
            let path_part = parts.next().unwrap_or("");
            j += if path_part.is_empty() { 3 } else { 1 };
        }
        out.push(FileChange {
            path,
            old_path,
            status: match kind {
                'A' => "added",
                'D' => "deleted",
                'R' => "renamed",
                'C' => "copied",
                'T' => "type_changed",
                _ => "modified",
            }
            .into(),
            additions,
            deletions,
        });
    }
    Ok(out)
}

fn split0(b: &[u8]) -> Vec<String> {
    b.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

fn untracked(wt: &Path, env: &Env) -> Result<Vec<String>> {
    Ok(split0(&git_raw(
        wt,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        env,
        &[],
    )?))
}

const PATCH_LIMIT: usize = 1024 * 1024;

/// Unified diff for one file. `old_path` is set for renames.
pub fn file_patch(
    wt: &Path,
    base_ref: &str,
    base_commit: &str,
    path: &str,
    old_path: Option<&str>,
    env: &Env,
) -> Result<String> {
    let mb = merge_base(wt, base_ref, base_commit, env);
    let is_untracked = untracked(wt, env)?.iter().any(|p| p == path);
    let raw = if is_untracked {
        // Exit code 1 means "files differ", which is the normal case here.
        git_raw(wt, &["diff", "--no-index", "--", "/dev/null", path], env, &[1])?
    } else {
        let mut args = vec!["diff", "-M", mb.as_str(), "--"];
        if let Some(o) = old_path {
            args.push(o);
        }
        args.push(path);
        git_raw(wt, &args, env, &[])?
    };
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    if text.len() > PATCH_LIMIT {
        let mut cut = PATCH_LIMIT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n[diff cut at 1 MB]\n");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn summary_and_patches() {
        let repo = repo("diff");
        let env = env();
        sh(&repo, "printf 'keep\\n' > b.txt && printf 'x\\n' > gone.txt && git add . && git commit -qm more");
        sh(&repo, "git checkout -q -b task");
        // Committed change, rename, delete, uncommitted edit, untracked, binary.
        sh(&repo, "printf 'one\\nTWO\\nthree\\n' > a.txt && git commit -qam edit");
        sh(&repo, "git mv b.txt c.txt && git rm -q gone.txt && git commit -qm mv");
        // Base branch moves on; that must not show up.
        sh(&repo, "git checkout -q main && printf 'z\\n' > main-only.txt && git add . && git commit -qm main && git checkout -q task");
        sh(&repo, "printf 'keep\\nmore\\n' > c.txt");
        sh(&repo, "printf 'n1\\nn2' > new.txt && printf '\\000\\001' > bin.dat");

        let base = git(&repo, &["rev-parse", "main~1"], &env).unwrap();
        let s = summary(&repo, "main", &base, &env).unwrap();
        assert_eq!(s.base, "main");
        let got: Vec<(&str, &str, Option<u64>, Option<u64>)> = s
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.status.as_str(), f.additions, f.deletions))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a.txt", "modified", Some(2), Some(1)),
                ("bin.dat", "untracked", None, None),
                ("c.txt", "renamed", Some(1), Some(0)),
                ("gone.txt", "deleted", Some(0), Some(1)),
                ("new.txt", "untracked", Some(2), Some(0)),
            ]
        );
        assert_eq!(s.files[2].old_path.as_deref(), Some("b.txt"));

        let p = file_patch(&repo, "main", &base, "a.txt", None, &env).unwrap();
        assert!(p.contains("-two") && p.contains("+TWO") && p.contains("+three"), "{p}");
        let p = file_patch(&repo, "main", &base, "c.txt", Some("b.txt"), &env).unwrap();
        assert!(p.contains("rename from b.txt") && p.contains("+more"), "{p}");
        let p = file_patch(&repo, "main", &base, "new.txt", None, &env).unwrap();
        assert!(p.contains("+n1") && p.contains("+n2"), "{p}");

        // Base branch deleted: falls back to the stored commit.
        sh(&repo, "git branch -q -D main");
        let s2 = summary(&repo, "main", &base, &env).unwrap();
        assert_eq!(s2.base_commit, base);
        std::fs::remove_dir_all(&repo).unwrap();
    }
}
