// Typed wrappers for the Tauri commands in src-tauri/src/commands.rs.
import { invoke } from "@tauri-apps/api/core";

export type Agent = "claude" | "codex";

export type Project = { id: number; name: string; repo_path: string; created_at: string };

export type Task = {
  id: number;
  project_id: number;
  title: string;
  stage: string;
  position: number;
  worktree_path: string | null;
  linear_issue_id: string | null;
  github_issue: string | null;
  created_at: string;
  updated_at: string;
};

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
  createTask: (projectId: number, title: string) =>
    invoke<Task>("create_task", { projectId, title }),
  deleteTask: (id: number) => invoke<void>("delete_task", { id }),

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
