import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ChevronDown,
  ChevronRight,
  FolderGit2,
  FolderPlus,
  ListTodo,
  Plus,
  TerminalSquare,
  ArchiveRestore,
  Pencil,
  Play,
} from "lucide-react";
import {
  api,
  STAGE_SKILL,
  AGENT_LABEL,
  skillCommand,
  type Agent,
  type MoveResult,
  type Project,
  type Session,
  type Stage,
  type Task,
} from "@/lib/api";
import { Textarea } from "@/components/ui/textarea";
import { Board } from "@/components/Board";
import { TaskDocs } from "@/components/TaskDocs";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { ConfirmDelete } from "@/components/ConfirmDelete";
import { SessionTranscript } from "@/components/SessionTranscript";
import { ArchiveTask, WorktreePanel } from "@/components/Worktree";
import { DiffView } from "@/components/DiffView";
import { listen } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";

type Selection =
  | { kind: "project"; id: number }
  | { kind: "task"; id: number }
  | { kind: "session"; id: number }
  | null;

export default function App() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [tasks, setTasks] = useState<Record<number, Task[]>>({});
  const [sessions, setSessions] = useState<Record<number, Session[]>>({});
  const [openProjects, setOpenProjects] = useState<Set<number>>(new Set());
  const [openTasks, setOpenTasks] = useState<Set<number>>(new Set());
  const [sel, setSel] = useState<Selection>(null);
  const [error, setError] = useState<string | null>(null);
  // Short message after a board move (session started, waiting, ...).
  const [notice, setNotice] = useState<{ text: string; sessionId?: number } | null>(null);
  // Bumped per task when its docs folder changes.
  const [docsKey, setDocsKey] = useState<Record<number, number>>({});

  const fail = (e: unknown) => setError(String(e));

  const loadProjects = useCallback(() => api.listProjects().then(setProjects).catch(fail), []);
  const loadTasks = useCallback(
    (projectId: number) =>
      api
        .listTasks(projectId)
        .then((t) => setTasks((m) => ({ ...m, [projectId]: t })))
        .catch(fail),
    [],
  );
  const loadSessions = useCallback(
    (taskId: number) =>
      api
        .listSessions(taskId)
        .then((s) => setSessions((m) => ({ ...m, [taskId]: s })))
        .catch(fail),
    [],
  );

  useEffect(() => {
    loadProjects();
  }, [loadProjects]);

  // Whatever is selected shows its children, so load them on selection
  // (not only when the sidebar row is expanded).
  useEffect(() => {
    if (sel?.kind === "project") loadTasks(sel.id);
    if (sel?.kind === "task") loadSessions(sel.id);
  }, [sel, loadTasks, loadSessions]);

  // Keep session rows (status, CLI session id) current as agents run.
  useEffect(() => {
    const un = listen<Session>("session-updated", ({ payload: s }) =>
      setSessions((m) =>
        m[s.task_id] ? { ...m, [s.task_id]: m[s.task_id].map((x) => (x.id === s.id ? s : x)) } : m,
      ),
    );
    return () => {
      un.then((f) => f());
    };
  }, []);

  // Sessions started by the backend (stage moves, Iterate).
  useEffect(() => {
    const un1 = listen<Session>("session-created", ({ payload: s }) =>
      setSessions((m) =>
        m[s.task_id] && !m[s.task_id].some((x) => x.id === s.id) ? { ...m, [s.task_id]: [...m[s.task_id], s] } : m,
      ),
    );
    const un2 = listen<number>("artifacts-updated", ({ payload: id }) =>
      setDocsKey((m) => ({ ...m, [id]: (m[id] ?? 0) + 1 })),
    );
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
    };
  }, []);

  // Worktree progress and archive changes.
  useEffect(() => {
    const un = listen<Task>("task-updated", ({ payload: t }) =>
      setTasks((m) =>
        m[t.project_id] ? { ...m, [t.project_id]: m[t.project_id].map((x) => (x.id === t.id ? t : x)) } : m,
      ),
    );
    return () => {
      un.then((f) => f());
    };
  }, []);

  const allTasks = Object.values(tasks).flat();
  const allSessions = Object.values(sessions).flat();
  const project = (id: number) => projects.find((p) => p.id === id);
  const task = (id: number) => allTasks.find((t) => t.id === id);
  const session = (id: number) => allSessions.find((s) => s.id === id);

  function toggle(set: Set<number>, id: number): Set<number> {
    const next = new Set(set);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    return next;
  }

  function expandProject(id: number, force = false) {
    if (!openProjects.has(id) || force) loadTasks(id);
    setOpenProjects((s) => (force ? new Set(s).add(id) : toggle(s, id)));
  }

  function expandTask(id: number, force = false) {
    if (!openTasks.has(id) || force) loadSessions(id);
    setOpenTasks((s) => (force ? new Set(s).add(id) : toggle(s, id)));
  }

  async function addProject() {
    setError(null);
    const picked = await open({ directory: true, multiple: false, title: "Choose a git repository" });
    if (typeof picked !== "string") return;
    try {
      const p = await api.createProject(picked);
      await loadProjects();
      expandProject(p.id, true);
      setSel({ kind: "project", id: p.id });
    } catch (e) {
      fail(e);
    }
  }

  async function addTask(projectId: number, title: string) {
    const t = await api.createTask(projectId, title);
    await loadTasks(projectId);
    expandProject(projectId, true);
    setSel({ kind: "task", id: t.id });
  }

  async function addSession(taskId: number, agent: Agent) {
    try {
      const s = await api.createSession(taskId, agent);
      await loadSessions(taskId);
      expandTask(taskId, true);
      setSel({ kind: "session", id: s.id });
    } catch (e) {
      fail(e);
    }
  }

  async function moveTask(t: Task, stage: Stage, index: number) {
    setNotice(null);
    // Show the move at once; the reload below brings the real order.
    setTasks((m) => ({
      ...m,
      [t.project_id]: (m[t.project_id] ?? []).map((x) => (x.id === t.id ? { ...x, stage, position: index - 0.5 } : x)),
    }));
    try {
      showMove(await api.moveTask(t.id, stage, index));
    } catch (e) {
      fail(e);
    }
    await loadTasks(t.project_id);
  }

  function showMove(r: MoveResult) {
    if (r.session) {
      loadSessions(r.task.id);
      setNotice({ text: `Started /rpi:${r.session.skill} on “${r.task.title}”.`, sessionId: r.session.id });
    } else if (r.note) {
      setNotice({ text: r.note });
    }
  }

  function openSession(s: Session) {
    loadSessions(s.task_id).then(() => {
      expandTask(s.task_id, true);
      setSel({ kind: "session", id: s.id });
    });
  }

  async function removeProject(p: Project) {
    await api.deleteProject(p.id);
    setTasks(({ [p.id]: _, ...rest }) => rest);
    if (sel) setSel(null);
    await loadProjects();
  }

  async function removeTask(t: Task) {
    await api.deleteTask(t.id);
    setSessions(({ [t.id]: _, ...rest }) => rest);
    setSel({ kind: "project", id: t.project_id });
    await loadTasks(t.project_id);
  }

  async function removeSession(s: Session) {
    await api.deleteSession(s.id);
    setSel({ kind: "task", id: s.task_id });
    await loadSessions(s.task_id);
  }

  const isSel = (kind: string, id: number) => sel?.kind === kind && sel.id === id;

  return (
    <div className="flex h-full">
      {/* ---- sidebar ---- */}
      <aside className="flex w-72 flex-none flex-col border-r bg-secondary/30">
        <div className="flex items-center justify-between border-b px-3 py-2">
          <span className="font-semibold tracking-wider uppercase">Projects</span>
          <Button size="xs" variant="ghost" onClick={addProject} title="Add a git repository">
            <FolderPlus /> Add
          </Button>
        </div>
        <nav className="flex-1 overflow-auto py-1">
          {projects.length === 0 && (
            <p className="px-3 py-2 text-muted-foreground">No projects yet.</p>
          )}
          {projects.map((p) => (
            <div key={p.id}>
              <TreeRow
                depth={0}
                open={openProjects.has(p.id)}
                onToggle={() => expandProject(p.id)}
                selected={isSel("project", p.id)}
                onSelect={() => setSel({ kind: "project", id: p.id })}
                icon={<FolderGit2 className="size-4" />}
                label={p.name}
              />
              {openProjects.has(p.id) &&
                (tasks[p.id] ?? [])
                  .filter((t) => !t.archived_at || isSel("task", t.id))
                  .map((t) => (
                  <div key={t.id}>
                    <TreeRow
                      depth={1}
                      open={openTasks.has(t.id)}
                      onToggle={() => expandTask(t.id)}
                      selected={isSel("task", t.id)}
                      onSelect={() => setSel({ kind: "task", id: t.id })}
                      icon={<ListTodo className="size-4" />}
                      label={t.title}
                      status={t.archived_at ? "archived" : wtLabel(t)}
                    />
                    {openTasks.has(t.id) &&
                      (sessions[t.id] ?? []).map((s) => (
                        <TreeRow
                          key={s.id}
                          depth={2}
                          selected={isSel("session", s.id)}
                          onSelect={() => setSel({ kind: "session", id: s.id })}
                          icon={<TerminalSquare className="size-4" />}
                          label={sessionLabel(s)}
                          status={s.status}
                        />
                      ))}
                  </div>
                ))}
            </div>
          ))}
        </nav>
      </aside>

      {/* ---- main ---- */}
      <main className="flex min-w-0 flex-1 flex-col gap-3 p-4">
        {error && (
          <div className="flex items-start justify-between gap-2 border border-destructive bg-destructive/10 p-2 text-destructive">
            <span className="whitespace-pre-wrap">{error}</span>
            <button className="cursor-pointer" onClick={() => setError(null)}>
              ×
            </button>
          </div>
        )}

        {notice && (
          <div className="flex items-center justify-between gap-2 border border-accent bg-accent/10 p-2">
            <span>{notice.text}</span>
            <span className="flex flex-none gap-2">
              {notice.sessionId && (
                <Button
                  size="xs"
                  variant="outline"
                  onClick={() => {
                    const s = allSessions.find((x) => x.id === notice.sessionId);
                    if (s) openSession(s);
                    setNotice(null);
                  }}
                >
                  Open session
                </Button>
              )}
              <button className="cursor-pointer" onClick={() => setNotice(null)}>
                ×
              </button>
            </span>
          </div>
        )}

        {sel === null && (
          <div className="m-auto max-w-sm text-center text-muted-foreground">
            <p className="mb-3">Add a git repository as a project to get started.</p>
            <Button onClick={addProject}>
              <FolderPlus /> Add project
            </Button>
          </div>
        )}

        {sel?.kind === "project" && project(sel.id) && (
          <ProjectView
            project={project(sel.id)!}
            tasks={tasks[sel.id] ?? []}
            onAddTask={(title) => addTask(sel.id, title)}
            onOpenTask={(id) => setSel({ kind: "task", id })}
            onMove={moveTask}
            onDelete={() => removeProject(project(sel.id)!)}
            onChanged={(p) => setProjects((ps) => ps.map((x) => (x.id === p.id ? p : x)))}
            onError={fail}
          />
        )}

        {sel?.kind === "task" && task(sel.id) && (
          <TaskView
            task={task(sel.id)!}
            project={project(task(sel.id)!.project_id)}
            sessions={sessions[sel.id] ?? []}
            onAddSession={(a) => addSession(sel.id, a)}
            onOpenSession={(id) => setSel({ kind: "session", id })}
            onDelete={() => removeTask(task(sel.id)!)}
            onArchived={() => setSel({ kind: "project", id: task(sel.id)!.project_id })}
            docsKey={docsKey[sel.id] ?? 0}
            onSessionStarted={openSession}
            onStageResult={showMove}
            onError={fail}
          />
        )}

        {sel?.kind === "session" &&
          session(sel.id) &&
          (() => {
            const s = session(sel.id)!;
            const t = task(s.task_id);
            const p = t && project(t.project_id);
            const cwd = s.cwd ?? t?.worktree_path ?? p?.repo_path;
            return (
              <SessionView key={s.id} session={s} task={t} cwd={cwd} onDelete={() => removeSession(s)} />
            );
          })()}
      </main>
    </div>
  );
}

function TreeRow(props: {
  depth: number;
  open?: boolean;
  onToggle?: () => void;
  selected: boolean;
  onSelect: () => void;
  icon: React.ReactNode;
  label: string;
  status?: string;
}) {
  return (
    <div
      className={cn(
        "flex cursor-pointer items-center gap-1 py-1 pr-2 hover:bg-accent/10",
        props.selected && "bg-accent/20 text-accent",
      )}
      style={{ paddingLeft: 8 + props.depth * 16 }}
      onClick={props.onSelect}
    >
      {props.onToggle ? (
        <button
          className="flex size-4 cursor-pointer items-center justify-center text-muted-foreground"
          aria-label={props.open ? "Collapse" : "Expand"}
          onClick={(e) => {
            e.stopPropagation();
            props.onToggle!();
          }}
        >
          {props.open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
        </button>
      ) : (
        <span className="size-4" />
      )}
      {props.icon}
      <span className="truncate">{props.label}</span>
      {props.status && props.status !== "idle" && (
        <span
          className={cn(
            "ml-auto flex-none text-xs",
            props.status === "running" && "text-[var(--terminal-success)] animate-pulse",
            props.status === "failed" && "text-destructive",
            props.status === "interrupted" && "text-[var(--terminal-warning)]",
            (props.status === "setting up" || props.status === "creating") && "animate-pulse",
            props.status === "archived" && "text-muted-foreground",
            props.status === "setup failed" && "text-destructive",
          )}
        >
          {props.status}
        </span>
      )}
    </div>
  );
}

function Header(props: { title: string; sub?: React.ReactNode; actions?: React.ReactNode }) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-3 border-b pb-3">
      <div className="min-w-48 flex-1">
        <h1 className="truncate text-lg font-semibold">{props.title}</h1>
        {props.sub && <div className="truncate text-muted-foreground">{props.sub}</div>}
      </div>
      <div className="flex flex-wrap gap-2">{props.actions}</div>
    </div>
  );
}

function ProjectView(props: {
  project: Project;
  tasks: Task[];
  onAddTask: (title: string) => Promise<void>;
  onOpenTask: (id: number) => void;
  onMove: (t: Task, stage: Stage, index: number) => void;
  onDelete: () => Promise<void>;
  onChanged: (p: Project) => void;
  onError: (e: unknown) => void;
}) {
  const [title, setTitle] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [showArchived, setShowArchived] = useState(false);
  const p = props.project;
  const active = props.tasks.filter((t) => !t.archived_at);
  const archived = props.tasks.filter((t) => t.archived_at);
  return (
    <>
      <Header
        title={p.name}
        sub={p.repo_path}
        actions={
          <>
            <AgentSelect
              label="Default agent"
              title="Runs stage sessions for tasks that don't pick an agent"
              value={p.default_agent}
              options={[
                ["claude", "Claude"],
                ["codex", "Codex"],
              ]}
              onChange={(v) => api.setProjectAgent(p.id, v as Agent).then(props.onChanged).catch(props.onError)}
            />
            <ConfirmDelete
              what={`project “${p.name}”`}
              detail="This deletes its tasks and sessions from HumanLayer Local. The repository on disk is not touched."
              onConfirm={props.onDelete}
            />
          </>
        }
      />
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          setErr(null);
          props
            .onAddTask(title)
            .then(() => setTitle(""))
            .catch((e) => setErr(String(e)));
        }}
      >
        <Input placeholder="New task title" value={title} onChange={(e) => setTitle(e.target.value)} />
        <Button type="submit" disabled={!title.trim()}>
          <Plus /> Task
        </Button>
      </form>
      {err && <p className="text-destructive">{err}</p>}
      <Board
        tasks={active}
        onMove={props.onMove}
        onOpen={props.onOpenTask}
        label={(t) =>
          [t.agent && t.agent !== p.default_agent ? AGENT_LABEL[t.agent] : "", wtLabel(t)]
            .filter(Boolean)
            .join(" · ")
        }
      />
      {archived.length > 0 && (
        <>
          <button
            className="cursor-pointer self-start text-muted-foreground hover:text-foreground"
            onClick={() => setShowArchived((v) => !v)}
          >
            {showArchived ? "Hide" : "Show"} archived ({archived.length})
          </button>
          {showArchived && (
            <List
              empty=""
              items={archived.map((t) => ({
                id: t.id,
                label: t.title,
                meta: (
                  <span className="text-muted-foreground">
                    archived {new Date(t.archived_at!).toLocaleDateString()}
                  </span>
                ),
              }))}
              onOpen={props.onOpenTask}
            />
          )}
        </>
      )}
    </>
  );
}

function TaskView(props: {
  task: Task;
  project?: Project;
  sessions: Session[];
  onAddSession: (agent: Agent) => void;
  onOpenSession: (id: number) => void;
  onDelete: () => Promise<void>;
  onArchived: () => void;
  docsKey: number;
  onSessionStarted: (s: Session) => void;
  onStageResult: (r: MoveResult) => void;
  onError: (e: unknown) => void;
}) {
  const t = props.task;
  const [tab, setTab] = useState<"sessions" | "docs" | "changes">("sessions");
  const [editing, setEditing] = useState(false);
  const skill = STAGE_SKILL[t.stage];
  const defaultAgent = props.project?.default_agent ?? "claude";
  const agent = t.agent ?? defaultAgent;
  const [err, setErr] = useState<string | null>(null);
  const hasWt = !!t.worktree_path && t.worktree_status !== "creating";
  // Refresh the diff whenever a session of this task changes status.
  const sessionKey = props.sessions.map((s) => s.status).join();
  return (
    <>
      <Header
        title={t.title}
        sub={
          <>
            {props.project?.name} · <Badge variant="outline">{t.stage}</Badge>
            {t.archived_at && <> · archived {new Date(t.archived_at).toLocaleString()}</>}
          </>
        }
        actions={
          t.archived_at ? (
            <>
              <Button
                size="sm"
                variant="outline"
                onClick={() => api.unarchiveTask(t.id).catch((e) => setErr(String(e)))}
              >
                <ArchiveRestore /> Restore
              </Button>
              <ConfirmDelete
                what={`task “${t.title}”`}
                detail="This deletes its sessions too. Its branch is kept."
                onConfirm={props.onDelete}
              />
            </>
          ) : (
            <>
              <AgentSelect
                label="Agent"
                title="Runs this task's stage sessions"
                value={t.agent ?? ""}
                options={[
                  ["", `Project default (${AGENT_LABEL[defaultAgent]})`],
                  ["claude", "Claude"],
                  ["codex", "Codex"],
                ]}
                onChange={(v) => api.setTaskAgent(t.id, (v || null) as Agent | null).catch(props.onError)}
              />
              {skill && (
                <Button
                  size="sm"
                  title={`Start a ${AGENT_LABEL[agent]} session with ${skillCommand(agent, skill)}`}
                  onClick={() => api.startStageSession(t.id).then(props.onStageResult).catch(props.onError)}
                >
                  <Play /> {skill}
                </Button>
              )}
              <Button size="sm" variant="outline" onClick={() => setEditing(true)}>
                <Pencil /> Edit
              </Button>
              <Button size="sm" variant={skill ? "outline" : "default"} onClick={() => props.onAddSession("claude")}>
                <Plus /> Claude session
              </Button>
              <Button size="sm" variant="outline" onClick={() => props.onAddSession("codex")}>
                <Plus /> Codex session
              </Button>
              <ArchiveTask task={t} onDone={props.onArchived} />
              <ConfirmDelete
                what={`task “${t.title}”`}
                detail={
                  t.worktree_path
                    ? `This deletes its sessions and removes the worktree folder ${t.worktree_path}. Branch ${t.branch} is kept. Deleting fails if the worktree has uncommitted changes; archive instead to review them.`
                    : "This deletes its sessions too."
                }
                onConfirm={props.onDelete}
              />
            </>
          )
        }
      />
      {err && <p className="text-destructive">{err}</p>}
      {editing ? (
        <EditTask task={t} onDone={() => setEditing(false)} />
      ) : (
        t.description && <p className="whitespace-pre-wrap text-muted-foreground">{t.description}</p>
      )}
      <WorktreePanel task={t} repoPath={props.project?.repo_path} />
      <div className="flex gap-1 border-b">
        {(["sessions", "docs", "changes"] as const).map((k) => (
          <button
            key={k}
            disabled={k === "changes" && !hasWt}
            title={k === "changes" && !hasWt ? "Create a worktree to track this task's changes" : undefined}
            className={cn(
              "-mb-px cursor-pointer border-b-2 border-transparent px-3 py-1 capitalize disabled:cursor-not-allowed disabled:opacity-50",
              tab === k && "border-accent text-accent",
            )}
            onClick={() => setTab(k)}
          >
            {k}
          </button>
        ))}
      </div>
      {tab === "docs" ? (
        <TaskDocs taskId={t.id} slug={t.slug} refreshKey={props.docsKey} onSessionStarted={props.onSessionStarted} />
      ) : tab === "changes" && hasWt ? (
        <DiffView taskId={t.id} refreshKey={sessionKey} />
      ) : (
        <List
          empty="No sessions yet."
          items={props.sessions.map((s) => ({
            id: s.id,
            label: sessionLabel(s),
            meta: (
              <span className="text-muted-foreground">
                {s.status} · {new Date(s.created_at).toLocaleString()}
              </span>
            ),
          }))}
          onOpen={props.onOpenSession}
        />
      )}
    </>
  );
}

function sessionLabel(s: Session): string {
  return `${s.agent_kind} #${s.id}${s.skill ? ` · ${s.skill}` : ""}`;
}

function AgentSelect(props: {
  label: string;
  title: string;
  value: string;
  options: [string, string][];
  onChange: (v: string) => void;
}) {
  return (
    <label className="flex items-center gap-1 text-muted-foreground" title={props.title}>
      {props.label}
      <select
        className="h-8 rounded-md border bg-background px-2 text-foreground"
        value={props.value}
        onChange={(e) => props.onChange(e.target.value)}
      >
        {props.options.map(([v, l]) => (
          <option key={v} value={v}>
            {l}
          </option>
        ))}
      </select>
    </label>
  );
}

function EditTask(props: { task: Task; onDone: () => void }) {
  const [title, setTitle] = useState(props.task.title);
  const [desc, setDesc] = useState(props.task.description);
  const [err, setErr] = useState<string | null>(null);
  return (
    <form
      className="flex flex-col gap-2 border p-3"
      onSubmit={(e) => {
        e.preventDefault();
        api
          .updateTask(props.task.id, title, desc)
          .then(props.onDone)
          .catch((x) => setErr(String(x)));
      }}
    >
      <Input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Title" />
      <Textarea
        rows={5}
        value={desc}
        onChange={(e) => setDesc(e.target.value)}
        placeholder="Description: what should be done and why. Research starts from the title and this text; it is also saved as task.md in the docs folder."
      />
      {err && <p className="text-destructive">{err}</p>}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" size="sm" onClick={props.onDone}>
          Cancel
        </Button>
        <Button type="submit" size="sm" disabled={!title.trim()}>
          Save
        </Button>
      </div>
    </form>
  );
}

/** Short worktree state for list rows; empty when there is nothing to say. */
function wtLabel(t: Task): string {
  switch (t.worktree_status) {
    case "creating":
      return "creating";
    case "setting_up":
      return "setting up";
    case "setup_failed":
      return "setup failed";
    case "failed":
      return "worktree failed";
    default:
      return "";
  }
}

function SessionView(props: {
  session: Session;
  task?: Task;
  cwd?: string;
  onDelete: () => Promise<void>;
}) {
  const s = props.session;
  return (
    <>
      <Header
        title={`${s.agent_kind} session #${s.id}`}
        sub={
          <>
            {props.task?.title} · {s.status} · {props.cwd}
          </>
        }
        actions={<ConfirmDelete what={`session #${s.id}`} onConfirm={props.onDelete} />}
      />
      {props.cwd ? (
        <SessionTranscript session={s} />
      ) : (
        <p className="text-muted-foreground">No working folder for this session.</p>
      )}
    </>
  );
}

function List(props: {
  empty: string;
  items: { id: number; label: string; meta?: React.ReactNode }[];
  onOpen: (id: number) => void;
}) {
  if (props.items.length === 0) return <p className="text-muted-foreground">{props.empty}</p>;
  return (
    <ul className="divide-y border">
      {props.items.map((i) => (
        <li
          key={i.id}
          className="flex cursor-pointer items-center justify-between gap-3 px-3 py-2 hover:bg-accent/10"
          onClick={() => props.onOpen(i.id)}
        >
          <span className="truncate">{i.label}</span>
          {i.meta}
        </li>
      ))}
    </ul>
  );
}
