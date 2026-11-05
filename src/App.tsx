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

} from "lucide-react";
import { api, type Agent, type Project, type Session, type Task } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { ConfirmDelete } from "@/components/ConfirmDelete";
import { Runner } from "@/components/Runner";
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
                (tasks[p.id] ?? []).map((t) => (
                  <div key={t.id}>
                    <TreeRow
                      depth={1}
                      open={openTasks.has(t.id)}
                      onToggle={() => expandTask(t.id)}
                      selected={isSel("task", t.id)}
                      onSelect={() => setSel({ kind: "task", id: t.id })}
                      icon={<ListTodo className="size-4" />}
                      label={t.title}
                    />
                    {openTasks.has(t.id) &&
                      (sessions[t.id] ?? []).map((s) => (
                        <TreeRow
                          key={s.id}
                          depth={2}
                          selected={isSel("session", s.id)}
                          onSelect={() => setSel({ kind: "session", id: s.id })}
                          icon={<TerminalSquare className="size-4" />}
                          label={`${s.agent_kind} #${s.id}`}
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
            onDelete={() => removeProject(project(sel.id)!)}
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
          />
        )}

        {sel?.kind === "session" &&
          session(sel.id) &&
          (() => {
            const s = session(sel.id)!;
            const t = task(s.task_id);
            const p = t && project(t.project_id);
            const cwd = t?.worktree_path ?? p?.repo_path;
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
    </div>
  );
}

function Header(props: { title: string; sub?: React.ReactNode; actions?: React.ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-3 border-b pb-3">
      <div className="min-w-0">
        <h1 className="truncate text-lg font-semibold">{props.title}</h1>
        {props.sub && <div className="truncate text-muted-foreground">{props.sub}</div>}
      </div>
      <div className="flex flex-none gap-2">{props.actions}</div>
    </div>
  );
}

function ProjectView(props: {
  project: Project;
  tasks: Task[];
  onAddTask: (title: string) => Promise<void>;
  onOpenTask: (id: number) => void;
  onDelete: () => Promise<void>;
}) {
  const [title, setTitle] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const p = props.project;
  return (
    <>
      <Header
        title={p.name}
        sub={p.repo_path}
        actions={
          <ConfirmDelete
            what={`project “${p.name}”`}
            detail="This deletes its tasks and sessions from HumanLayer Local. The repository on disk is not touched."
            onConfirm={props.onDelete}
          />
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
      <List
        empty="No tasks yet."
        items={props.tasks.map((t) => ({
          id: t.id,
          label: t.title,
          meta: <Badge variant="outline">{t.stage}</Badge>,
        }))}
        onOpen={props.onOpenTask}
      />
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
}) {
  const t = props.task;
  return (
    <>
      <Header
        title={t.title}
        sub={
          <>
            {props.project?.name} · <Badge variant="outline">{t.stage}</Badge>
          </>
        }
        actions={
          <>
            <Button size="sm" onClick={() => props.onAddSession("claude")}>
              <Plus /> Claude session
            </Button>
            <Button size="sm" variant="outline" onClick={() => props.onAddSession("codex")}>
              <Plus /> Codex session
            </Button>
            <ConfirmDelete
              what={`task “${t.title}”`}
              detail="This deletes its sessions too."
              onConfirm={props.onDelete}
            />
          </>
        }
      />
      <List
        empty="No sessions yet."
        items={props.sessions.map((s) => ({
          id: s.id,
          label: `${s.agent_kind} #${s.id}`,
          meta: (
            <span className="text-muted-foreground">
              {s.status} · {new Date(s.created_at).toLocaleString()}
            </span>
          ),
        }))}
        onOpen={props.onOpenSession}
      />
    </>
  );
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
        <Runner agent={s.agent_kind} cwd={props.cwd} />
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
