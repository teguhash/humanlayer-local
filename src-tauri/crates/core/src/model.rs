use serde::{Deserialize, Serialize};

pub type Id = i64;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub repo_path: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Task {
    pub id: Id,
    pub project_id: Id,
    pub title: String,
    pub stage: String,
    pub position: i64,
    pub worktree_path: Option<String>,
    pub linear_issue_id: Option<String>,
    pub github_issue: Option<String>,
    pub created_at: String,
    pub updated_at: String,
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
}
