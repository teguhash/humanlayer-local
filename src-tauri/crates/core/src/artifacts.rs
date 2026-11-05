//! Each task's docs folder (research, plans, ...). The real folder lives in
//! app data (`<root>/<task id>/`), so it survives worktrees coming and
//! going. Before a session starts it is linked into the session's folder as
//! `.humanlayer/tasks/<slug>`, the path the RPI skills expect.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::model::{Id, Task};

pub const LINK_DIR: &str = ".humanlayer/tasks";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArtifactFile {
    /// File name inside the task's folder (subfolders as `a/b.md`).
    pub name: String,
    /// research, research_questions, plan, outline, design, prd, tdd,
    /// pr_description, task, other
    pub kind: String,
    /// Seconds since the Unix epoch.
    pub modified: u64,
    pub size: u64,
}

pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir(&self, task_id: Id) -> PathBuf {
        self.root.join(task_id.to_string())
    }

    /// Create the folder and (re)write `task.md` from the task's title and
    /// description. `task.md` belongs to the app; edit the task instead.
    pub fn ensure(&self, task: &Task) -> std::io::Result<PathBuf> {
        let dir = self.dir(task.id);
        std::fs::create_dir_all(&dir)?;
        let mut body = format!("# {}\n", task.title);
        if !task.description.trim().is_empty() {
            body.push_str(&format!("\n{}\n", task.description.trim()));
        }
        let path = dir.join("task.md");
        if std::fs::read_to_string(&path).ok().as_deref() != Some(body.as_str()) {
            std::fs::write(path, body)?;
        }
        Ok(dir)
    }

    /// Link the task's folder into `cwd` as `.humanlayer/tasks/<slug>`.
    /// Returns the link path. A wrong or stale link is replaced; a real
    /// folder at that path is left alone and reported as an error.
    pub fn link_into(&self, task: &Task, cwd: &Path) -> Result<PathBuf, String> {
        let target = self.ensure(task).map_err(|e| e.to_string())?;
        let link = cwd.join(LINK_DIR).join(&task.slug);
        ensure_link(&target, &link)?;
        Ok(link)
    }

    /// Files in the task's folder, newest first. Hidden files are skipped.
    pub fn scan(&self, task_id: Id) -> Vec<ArtifactFile> {
        let dir = self.dir(task_id);
        let mut out = vec![];
        walk(&dir, &dir, &mut out, 0);
        out.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.name.cmp(&b.name)));
        out
    }

    /// Read one file as text. `name` must stay inside the task's folder.
    pub fn read(&self, task_id: Id, name: &str) -> Result<String, String> {
        if name
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err("Invalid file name.".into());
        }
        let path = self.dir(task_id).join(name);
        let bytes = std::fs::read(&path).map_err(|e| format!("{name}: {e}"))?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(format!("{name} is larger than 4 MB."));
        }
        String::from_utf8(bytes).map_err(|_| format!("{name} is not a text file."))
    }

    /// Delete the task's folder (task deleted).
    pub fn remove(&self, task_id: Id) {
        let _ = std::fs::remove_dir_all(self.dir(task_id));
    }
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<ArtifactFile>, depth: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            if depth < 3 {
                walk(base, &path, out, depth + 1);
            }
            continue;
        }
        let rel = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        out.push(ArtifactFile {
            kind: kind_of(&rel).into(),
            name: rel,
            modified: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs()),
            size: meta.len(),
        });
    }
}

/// Classify by the RPI skills' file names, e.g. `03-research-auth.md`.
pub fn kind_of(name: &str) -> &'static str {
    let n = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
    if !n.ends_with(".md") {
        return "other";
    }
    if n == "task.md" || n == "ticket.md" {
        return "task";
    }
    // Order matters: "research-questions" before "research".
    for (needle, kind) in [
        ("research-questions", "research_questions"),
        ("research", "research"),
        ("structure-outline", "outline"),
        ("outline", "outline"),
        ("design-discussion", "design"),
        ("plan", "plan"),
        ("prd", "prd"),
        ("tdd", "tdd"),
        ("pr-description", "pr_description"),
        ("description", "pr_description"),
    ] {
        if n.contains(needle) {
            return kind;
        }
    }
    "other"
}

/// Make `link` a symlink to `target`, replacing a stale link. Refuses to
/// touch a real file or folder at `link`.
pub fn ensure_link(target: &Path, link: &Path) -> Result<(), String> {
    std::fs::create_dir_all(link.parent().unwrap()).map_err(|e| e.to_string())?;
    match std::fs::symlink_metadata(link) {
        Ok(m) if m.file_type().is_symlink() => {
            if std::fs::read_link(link).ok().as_deref() == Some(target) {
                return Ok(());
            }
            std::fs::remove_file(link).map_err(|e| e.to_string())?;
        }
        Ok(_) => {
            return Err(format!(
                "{} already exists and is not managed by HumanLayer Local. Move it away to use it here.",
                link.display()
            ));
        }
        Err(_) => {}
    }
    std::os::unix::fs::symlink(target, link).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: Id, slug: &str) -> Task {
        Task {
            id,
            project_id: 1,
            title: "Fix it".into(),
            description: "Details".into(),
            slug: slug.into(),
            stage: "backlog".into(),
            position: 0,
            worktree_path: None,
            linear_issue_id: None,
            github_issue: None,
            created_at: String::new(),
            updated_at: String::new(),
            branch: None,
            base_ref: None,
            base_commit: None,
            worktree_status: None,
            worktree_log: None,
            archived_at: None,
            agent: None,
        }
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of("01-research-questions-x.md"), "research_questions");
        assert_eq!(kind_of("02-research-auth.md"), "research");
        assert_eq!(kind_of("05-structure-outline-x.md"), "outline");
        assert_eq!(kind_of("06-plan-add-billing.md"), "plan");
        assert_eq!(kind_of("pr-description.md"), "pr_description");
        assert_eq!(kind_of("task.md"), "task");
        assert_eq!(kind_of("diagram.png"), "other");
    }

    #[test]
    fn link_scan_read() {
        let base = std::env::temp_dir().join(format!("hl-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let store = ArtifactStore::new(base.join("store"));
        let cwd = base.join("repo");
        std::fs::create_dir_all(&cwd).unwrap();
        let t = task(7, "fix-it");

        let link = store.link_into(&t, &cwd).unwrap();
        assert_eq!(link, cwd.join(".humanlayer/tasks/fix-it"));
        // Writing through the link lands in the store.
        std::fs::write(link.join("02-research-x.md"), "# R").unwrap();
        let files = store.scan(7);
        let names: Vec<_> = files
            .iter()
            .map(|f| (f.name.as_str(), f.kind.as_str()))
            .collect();
        assert!(
            names.contains(&("02-research-x.md", "research")),
            "{names:?}"
        );
        assert!(names.contains(&("task.md", "task")));
        assert_eq!(store.read(7, "task.md").unwrap(), "# Fix it\n\nDetails\n");
        assert!(store.read(7, "../x").is_err());

        // Idempotent; a stale link is replaced; a real folder is refused.
        store.link_into(&t, &cwd).unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("/nonexistent", &link).unwrap();
        store.link_into(&t, &cwd).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), store.dir(7));
        let other = task(8, "real");
        std::fs::create_dir_all(cwd.join(".humanlayer/tasks/real")).unwrap();
        assert!(
            store
                .link_into(&other, &cwd)
                .unwrap_err()
                .contains("not managed")
        );

        store.remove(7);
        assert!(store.scan(7).is_empty());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
