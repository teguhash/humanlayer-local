// Typed wrappers for the Tauri commands in src-tauri/src/commands.rs.
import { invoke } from "@tauri-apps/api/core";

export type Agent = "claude" | "codex";

export type Project = { id: number; name: string; repo_path: string; created_at: string };

export type Task = {
  id: number;
  project_id: number;
  title: string;
  description: string;
  /** Names the docs folder (.humanlayer/tasks/<slug>) and the branch. */
  slug: string;
  stage: Stage;
  position: number;
  worktree_path: string | null;
  linear_issue_id: string | null;
  github_issue: string | null;
  created_at: string;
  updated_at: string;
  branch: string | null;
  base_ref: string | null;
  base_commit: string | null;
  worktree_status: WorktreeStatus | null;
  worktree_log: string | null;
  archived_at: string | null;
};

export const STAGES = ["backlog", "research", "plan", "implement", "review", "done"] as const;
export type Stage = (typeof STAGES)[number];
/** Skill each stage starts (mirrors hl_core::model::stage::skill). */
export const STAGE_SKILL: Partial<Record<Stage, string>> = {
  research: "create-research",
  plan: "create-plan",
  implement: "implement-plan",
  review: "describe-pr",
};

export type Artifact = {
  id: number;
  task_id: number;
  kind: string;
  path: string;
  /** Seconds since the Unix epoch, as text. */
  updated_at: string;
};
/** Mirrors rpi::iterate_skill. */
export const ITERATE_SKILL: Record<string, string> = {
  research: "iterate-research",
  plan: "iterate-plan",
  outline: "iterate-structure-outline",
  research_questions: "iterate-research-questions",
  design: "iterate-design-discussion",
};

export type MoveResult = { task: Task; session: Session | null; note: string | null };

export type WorktreeStatus = "creating" | "setting_up" | "ready" | "setup_failed" | "failed";

// Mirrors hl_git::diff.
export type FileChange = {
  path: string;
  old_path: string | null;
  status: "added" | "modified" | "deleted" | "renamed" | "copied" | "type_changed" | "untracked";
  additions: number | null;
  deletions: number | null;
};
export type DiffSummary = { base: string; base_commit: string; files: FileChange[] };

export type Session = {
  id: number;
  task_id: number;
  agent_kind: Agent;
  cli_session_id: string | null;
  skill: string | null;
  status: string;
  created_at: string;
  started_at: string | null;
  ended_at: string | null;
  /** Folder the CLI runs in; set at the first turn. */
  cwd: string | null;
};

export type TokenUsage = {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
};

export type Decision = "allow_once" | "allow_session" | "deny";

// Mirrors hl_agents::transcript::Item.
export type Item =
  | { type: "user_prompt"; text: string }
  | { type: "assistant_text"; text: string }
  | { type: "thinking"; text: string }
  | { type: "tool_use"; id: string; name: string; input: any }
  | { type: "tool_result"; tool_use_id: string; content: string; is_error: boolean }
  | { type: "approval_request"; request_id: string; tool_name: string; input: any; description: string | null }
  | { type: "approval_response"; request_id: string; decision: Decision }
  | {
      type: "turn_end";
      ok: boolean;
      subtype: string | null;
      cost_usd: number | null;
      usage: TokenUsage | null;
      duration_ms: number | null;
    }
  | { type: "session_init"; cli_session_id: string; model: string | null }
  | { type: "process_started"; resume: boolean }
  | { type: "process_exit"; code: number | null; success: boolean; stopped: boolean }
  | { type: "notice"; level: "info" | "warn" | "error"; text: string };

export type SessionEvent = {
  session_id: number;
  seq: number;
  kind: string;
  event_type: string | null;
  items: Item[];
  payload: any;
  created_at: string;
};

export type SessionUsage = { tokens: TokenUsage; cost_usd: number; turns: number };

export type Transcript = {
  session: Session;
  events: SessionEvent[];
  usage: SessionUsage;
  live: boolean;
  pending_approvals: string[];
};

export const api = {
  listProjects: () => invoke<Project[]>("list_projects"),
  createProject: (path: string, name?: string) =>
    invoke<Project>("create_project", { path, name: name ?? null }),
  deleteProject: (id: number) => invoke<void>("delete_project", { id }),

  listTasks: (projectId: number) => invoke<Task[]>("list_tasks", { projectId }),
  createTask: (projectId: number, title: string, description = "") =>
    invoke<Task>("create_task", { projectId, title, description }),
  updateTask: (id: number, title: string, description: string) =>
    invoke<Task>("update_task", { id, title, description }),
  moveTask: (taskId: number, stage: Stage, index: number) =>
    invoke<MoveResult>("move_task", { taskId, stage, index }),
  startStageSession: (taskId: number) => invoke<MoveResult>("start_stage_session", { taskId }),
  listArtifacts: (taskId: number) => invoke<Artifact[]>("list_artifacts", { taskId }),
  readArtifact: (taskId: number, path: string) => invoke<string>("read_artifact", { taskId, path }),
  iterateArtifact: (taskId: number, path: string, feedback: string) =>
    invoke<Session>("iterate_artifact", { taskId, path, feedback }),
  deleteTask: (id: number, force = false) => invoke<void>("delete_task", { id, force }),

  createWorktree: (taskId: number) => invoke<Task>("create_worktree", { taskId }),
  retrySetup: (taskId: number) => invoke<Task>("retry_setup", { taskId }),
  worktreeDirtyFiles: (taskId: number) => invoke<string[]>("worktree_dirty_files", { taskId }),
  getDiff: (taskId: number) => invoke<DiffSummary>("get_diff", { taskId }),
  getFileDiff: (taskId: number, path: string, oldPath: string | null) =>
    invoke<string>("get_file_diff", { taskId, path, oldPath }),
  archiveTask: (taskId: number, force: boolean) => invoke<Task>("archive_task", { taskId, force }),
  unarchiveTask: (taskId: number) => invoke<Task>("unarchive_task", { taskId }),

  listSessions: (taskId: number) => invoke<Session[]>("list_sessions", { taskId }),
  createSession: (taskId: number, agent: Agent) =>
    invoke<Session>("create_session", { taskId, agent }),
  deleteSession: (id: number) => invoke<void>("delete_session", { id }),

  getTranscript: (sessionId: number) => invoke<Transcript>("get_transcript", { sessionId }),
  getUsage: (sessionId: number) => invoke<SessionUsage>("get_usage", { sessionId }),
  sendMessage: (sessionId: number, text: string) =>
    invoke<void>("send_message", { sessionId, text }),
  interrupt: (sessionId: number) => invoke<void>("interrupt_session", { sessionId }),
  stop: (sessionId: number) => invoke<void>("stop_session", { sessionId }),
  respondApproval: (sessionId: number, requestId: string, decision: Decision) =>
    invoke<void>("respond_approval", { sessionId, requestId, decision }),
};
