use serde::{Deserialize, Serialize};

pub type Id = i64;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub repo_path: String,
    pub created_at: String,
    /// Agent for stage sessions when a task doesn't pick one.
    pub default_agent: AgentKind,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Task {
    pub id: Id,
    pub project_id: Id,
    pub title: String,
    /// Free text; written to the docs folder as task.md.
    pub description: String,
    /// Names the docs folder (`.humanlayer/tasks/<slug>`) and the worktree
    /// branch. Unique per project, fixed at creation.
    pub slug: String,
    pub stage: String,
    pub position: i64,
    pub worktree_path: Option<String>,
    pub linear_issue_id: Option<String>,
    pub github_issue: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Branch the task's worktree uses (kept after the worktree is removed).
    pub branch: Option<String>,
    /// What the worktree branched from: a branch name, or a commit when HEAD was detached.
    pub base_ref: Option<String>,
    pub base_commit: Option<String>,
    /// See `worktree_status`. None = no worktree.
    pub worktree_status: Option<String>,
    /// Output of the last worktree creation and setup command.
    pub worktree_log: Option<String>,
    pub archived_at: Option<String>,
    /// Linear ticket link (`linear_issue_id` is the identifier, e.g. ENG-1).
    pub linear_url: Option<String>,
    /// Workflow state name and ticket updatedAt, as last synced.
    pub linear_state: Option<String>,
    pub linear_updated_at: Option<String>,
    /// Overrides the project's `default_agent`. None = use the default.
    pub agent: Option<AgentKind>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(AgentKind::Claude),
            "codex" => Some(AgentKind::Codex),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Session {
    pub id: Id,
    pub task_id: Id,
    pub agent_kind: AgentKind,
    pub cli_session_id: Option<String>,
    pub skill: Option<String>,
    pub status: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    /// Folder the CLI runs in, fixed at the first turn.
    pub cwd: Option<String>,
}

/// One stored event. `kind` is ours (user_prompt, cli, stderr, ...);
/// `payload` is the CLI's JSON line for `cli` events.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Event {
    pub id: Id,
    pub session_id: Id,
    pub seq: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_at: String,
}

pub mod status {
    /// Waiting for the user (a process may or may not be alive).
    pub const IDLE: &str = "idle";
    /// A turn is in progress.
    pub const RUNNING: &str = "running";
    pub const COMPLETED: &str = "completed";
    /// The CLI exited with an error during a turn.
    pub const FAILED: &str = "failed";
    /// A turn was cut off: stopped by the user, or the app quit mid-turn.
    pub const INTERRUPTED: &str = "interrupted";
}

pub mod worktree_status {
    pub const CREATING: &str = "creating";
    pub const SETTING_UP: &str = "setting_up";
    pub const READY: &str = "ready";
    /// The worktree exists but the setup command failed.
    pub const SETUP_FAILED: &str = "setup_failed";
    /// `git worktree add` failed; there is no worktree.
    pub const FAILED: &str = "failed";
}

/// Board columns, in order, and the RPI skill each one starts.
pub mod stage {
    pub const BACKLOG: &str = "backlog";
    pub const RESEARCH: &str = "research";
    pub const PLAN: &str = "plan";
    pub const IMPLEMENT: &str = "implement";
    pub const REVIEW: &str = "review";
    pub const DONE: &str = "done";
    pub const ALL: [&str; 6] = [BACKLOG, RESEARCH, PLAN, IMPLEMENT, REVIEW, DONE];

    /// The bundled plugin skill (without the `rpi:` prefix) that moving a
    /// task into `stage` starts.
    pub fn skill(stage: &str) -> Option<&'static str> {
        match stage {
            RESEARCH => Some("create-research"),
            PLAN => Some("create-plan"),
            IMPLEMENT => Some("implement-plan"),
            REVIEW => Some("describe-pr"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Artifact {
    pub id: Id,
    pub task_id: Id,
    pub kind: String,
    /// File name inside the task's docs folder.
    pub path: String,
    pub updated_at: String,
}

/// The ticket fields the app stores on a task.
#[derive(Debug, Clone, Default)]
pub struct LinearTicket {
    pub identifier: String,
    pub title: String,
    pub description: String,
    pub url: String,
    pub state: String,
    pub updated_at: String,
}
