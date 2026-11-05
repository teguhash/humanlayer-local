// Typed wrappers for the Tauri commands in src-tauri/src/commands.rs.
import { invoke } from "@tauri-apps/api/core";

export type Agent = "claude" | "codex";

export type Project = {
  id: number;
  name: string;
  repo_path: string;
  created_at: string;
  /** Agent for stage sessions when a task doesn't pick one. */
  default_agent: Agent;
};

export const AGENT_LABEL: Record<Agent, string> = { claude: "Claude", codex: "Codex" };

/** How a skill is invoked (mirrors rpi::skill_prompt). */
export const skillCommand = (agent: Agent, skill: string) =>
  agent === "codex" ? "$" + skill : "/rpi:" + skill;

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
  /** GitHub issue "owner/repo#12" when imported from one. */
  github_issue: string | null;
  github_issue_url: string | null;
  /** Linear ticket identifier (e.g. ENG-123) when linked. */
  linear_issue_id: string | null;
  created_at: string;
  updated_at: string;
  branch: string | null;
  base_ref: string | null;
  base_commit: string | null;
  worktree_status: WorktreeStatus | null;
  worktree_log: string | null;
  archived_at: string | null;
  linear_url: string | null;
  /** Workflow state name as last synced. */
  linear_state: string | null;
  /** Overrides the project's default_agent; null = use the default. */
  agent: Agent | null;
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

export type LinearStatus = {
  has_key: boolean;
  account: string | null;
  /** Seconds since the Unix epoch, as text. */
  last_poll: string | null;
  last_error: string | null;
};
export type GithubStatus = {
  gh_installed: boolean;
  login: string | null;
  /** Where gh got the credential: "keyring", "GH_TOKEN", ... */
  source: string | null;
  has_token: boolean;
  error: string | null;
};

// Mirrors hl_core::model::PullRequest.
export type PullRequest = {
  id: number;
  task_id: number;
  repo: string;
  number: number;
  url: string;
  title: string;
  state: "OPEN" | "CLOSED" | "MERGED" | "";
  is_draft: boolean;
  mergeable: string;
  merge_state: string;
  review_state: string;
  checks_state: "passing" | "failing" | "pending" | "none" | "";
  head_ref: string;
  base_ref: string;
  updated_at: string;
};

/** True when an import reference is GitHub's (#12, owner/repo#12, github.com URL). */
export const isGithubRef = (s: string) =>
  /github\.com\//.test(s) || /^#?\d+$/.test(s.trim()) || /^[\w.-]+\/[\w.-]+#\d+$/.test(s.trim());

export type Tool = {
  name: string;
  purpose: string;
  required: boolean;
  path: string | null;
  version: string | null;
  error: string | null;
  install: string;
};
export type ToolCheck = { tools: Tool[]; env_error: string | null };

export type Notice = { level: "info" | "error"; text: string };

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
  setTaskAgent: (taskId: number, agent: Agent | null) => invoke<Task>("set_task_agent", { taskId, agent }),
  setProjectAgent: (projectId: number, agent: Agent) =>
    invoke<Project>("set_project_agent", { projectId, agent }),
  startStageSession: (taskId: number) => invoke<MoveResult>("start_stage_session", { taskId }),
  listArtifacts: (taskId: number) => invoke<Artifact[]>("list_artifacts", { taskId }),
  readArtifact: (taskId: number, path: string) => invoke<string>("read_artifact", { taskId, path }),
  iterateArtifact: (taskId: number, path: string, feedback: string) =>
    invoke<Session>("iterate_artifact", { taskId, path, feedback }),
  readArtifactDataUrl: (taskId: number, path: string) =>
    invoke<string>("read_artifact_data_url", { taskId, path }),
  postDocToLinear: (taskId: number, path: string) => invoke<string>("post_doc_to_linear", { taskId, path }),

  linearStatus: () => invoke<LinearStatus>("linear_status"),
  setLinearKey: (key: string) => invoke<LinearStatus>("set_linear_key", { key }),
  clearLinearKey: () => invoke<void>("clear_linear_key"),
  syncLinearNow: () => invoke<LinearStatus>("sync_linear_now"),
  importLinearTicket: (projectId: number, reference: string) =>
    invoke<{ task: Task; note: string | null }>("import_linear_ticket", { projectId, reference }),

  githubStatus: () => invoke<GithubStatus>("github_status"),
  setGithubToken: (token: string) => invoke<GithubStatus>("set_github_token", { token }),
  clearGithubToken: () => invoke<GithubStatus>("clear_github_token"),
  importGithubIssue: (projectId: number, reference: string) =>
    invoke<Task>("import_github_issue", { projectId, reference }),
  openPr: (taskId: number, draft: boolean) => invoke<PullRequest>("open_pr", { taskId, draft }),
  listProjectPrs: (projectId: number) => invoke<PullRequest[]>("list_project_prs", { projectId }),
  refreshPr: (id: number) => invoke<PullRequest>("refresh_pr", { id }),
  mergePr: (id: number, method: string) => invoke<PullRequest>("merge_pr", { id, method }),

  checkTools: () => invoke<ToolCheck>("check_tools"),
  diagnostics: () => invoke<string>("diagnostics"),

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
