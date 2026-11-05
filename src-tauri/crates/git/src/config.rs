//! Worktree settings, read from the same files upstream's `setup-worktree`
//! skill uses: `.humanlayer/workspace.json` (shared) and
//! `.humanlayer/workspace.local.json` (personal overrides, field by field).
//! Only the primary repo entry (`localPath: "."`) is used.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, Result};

pub const SHARED: &str = ".humanlayer/workspace.json";
pub const LOCAL: &str = ".humanlayer/workspace.local.json";

const DEFAULT_PATH: &str = "~/.humanlayer/workspaces/{{ TASKSLUG }}/{{ REPOBASENAME }}";
const DEFAULT_BRANCH: &str = "{{ TASKSLUG }}";
const DEFAULT_COPY: &[&str] = &[
    ".env*",
    ".claude/settings.local.json",
    "CLAUDE.local.md",
    ".humanlayer/workspace.local.json",
];

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    disabled: Option<bool>,
    path_template: Option<String>,
    branch_template: Option<String>,
    repos: Option<Vec<RepoEntry>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoEntry {
    local_path: Option<String>,
    source_ref: Option<String>,
    setup_command: Option<String>,
    copy_globs: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceConfig {
    pub disabled: bool,
    pub path_template: String,
    pub branch_template: String,
    pub source_ref: String,
    /// Run with `sh -c` in the new worktree. Empty = none.
    pub setup_command: String,
    /// Untracked files to copy from the repo into the worktree.
    pub copy_globs: Vec<String>,
    /// Config files that were found, for display.
    pub sources: Vec<String>,
}

impl WorkspaceConfig {
    /// Read the config for `repo`. Missing files mean defaults; a file that
    /// is not valid JSON is an error (shown to the user).
    pub fn load(repo: &Path) -> Result<Self> {
        let mut cfg = Self {
            disabled: false,
            path_template: DEFAULT_PATH.into(),
            branch_template: DEFAULT_BRANCH.into(),
            source_ref: "HEAD".into(),
            setup_command: String::new(),
            copy_globs: DEFAULT_COPY.iter().map(|s| s.to_string()).collect(),
            sources: vec![],
        };
        for name in [SHARED, LOCAL] {
            let path = repo.join(name);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let f: File = serde_json::from_str(&text)
                .map_err(|e| Error::Invalid(format!("{name} is not valid: {e}")))?;
            cfg.apply(f);
            cfg.sources.push(name.into());
        }
        Ok(cfg)
    }

    fn apply(&mut self, f: File) {
        if let Some(v) = f.disabled {
            self.disabled = v;
        }
        if let Some(v) = f.path_template {
            self.path_template = v;
        }
        if let Some(v) = f.branch_template {
            self.branch_template = v;
        }
        let primary = f
            .repos
            .unwrap_or_default()
            .into_iter()
            .find(|r| matches!(r.local_path.as_deref(), None | Some(".") | Some("./")));
        if let Some(r) = primary {
            if let Some(v) = r.source_ref {
                self.source_ref = v;
            }
            if let Some(v) = r.setup_command {
                self.setup_command = v;
            }
            if let Some(v) = r.copy_globs {
                self.copy_globs = v;
            }
        }
    }

    pub fn worktree_path(&self, repo: &Path, slug: &str) -> PathBuf {
        let s = render(&self.path_template, slug, repo);
        let p = match s.strip_prefix("~/") {
            Some(rest) => home().join(rest),
            None => PathBuf::from(s),
        };
        if p.is_absolute() { p } else { repo.join(p) }
    }

    pub fn branch(&self, repo: &Path, slug: &str) -> String {
        render(&self.branch_template, slug, repo)
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Replace `{{ TASKSLUG }}` and `{{ REPOBASENAME }}` (spaces optional).
fn render(template: &str, slug: &str, repo: &Path) -> String {
    let base = repo
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start..].find("}}") else {
            break;
        };
        out.push_str(&rest[..start]);
        match rest[start + 2..start + len].trim() {
            "TASKSLUG" => out.push_str(slug),
            "REPOBASENAME" => out.push_str(&base),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    out
}

/// A branch- and folder-safe slug from a task title, at most 40 chars.
pub fn slugify(title: &str) -> String {
    let mut s = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
        if s.len() >= 40 {
            break;
        }
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "task".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::tmp;

    #[test]
    fn slugs() {
        assert_eq!(slugify("Fix: the Login bug!"), "fix-the-login-bug");
        assert_eq!(slugify("  ***  "), "task");
        assert_eq!(slugify("Ünïcode ok"), "n-code-ok");
        assert!(slugify(&"a b ".repeat(50)).len() <= 40);
    }

    #[test]
    fn defaults_and_overrides() {
        let d = tmp("cfg");
        let c = WorkspaceConfig::load(&d).unwrap();
        assert!(c.sources.is_empty() && !c.disabled);
        let name = d.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            c.worktree_path(&d, "x"),
            home().join(format!(".humanlayer/workspaces/x/{name}"))
        );
        assert_eq!(c.branch(&d, "x"), "x");

        std::fs::create_dir_all(d.join(".humanlayer")).unwrap();
        std::fs::write(
            d.join(SHARED),
            r#"{"pathTemplate":"../wt/{{TASKSLUG}}","branchTemplate":"hl/{{ TASKSLUG }}",
                "repos":[{"localPath":".","setupCommand":"make setup","copyGlobs":[".env"]}]}"#,
        )
        .unwrap();
        std::fs::write(
            d.join(LOCAL),
            r#"{"repos":[{"localPath":".","setupCommand":"npm i"}]}"#,
        )
        .unwrap();
        let c = WorkspaceConfig::load(&d).unwrap();
        assert_eq!(c.setup_command, "npm i");
        assert_eq!(c.copy_globs, vec![".env"]);
        assert_eq!(c.branch(&d, "x"), "hl/x");
        assert_eq!(c.worktree_path(&d, "x"), d.join("../wt/x"));
        assert_eq!(c.sources.len(), 2);

        std::fs::write(d.join(LOCAL), "{nope").unwrap();
        let e = WorkspaceConfig::load(&d).unwrap_err().to_string();
        assert!(e.contains("workspace.local.json is not valid"), "{e}");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
