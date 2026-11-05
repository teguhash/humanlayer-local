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

export type AgentEvent = {
  run_id: string;
  seq: number;
  kind: "started" | "event" | "invalid_json" | "stderr" | "exit" | "error";
  event_type: string | null;
  payload: any;
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

  startRun: (agent: Agent, cwd: string, prompt: string) =>
    invoke<string>("start_run", { agent, cwd, prompt }),
  respondApproval: (runId: string, requestId: string, allow: boolean, input: unknown) =>
    invoke<void>("respond_approval", { runId, requestId, allow, input }),
  stopRun: (runId: string) => invoke<void>("stop_run", { runId }),
};
