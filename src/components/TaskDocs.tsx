import { useCallback, useEffect, useState } from "react";
import { MessageSquarePlus, RefreshCw, Send } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Markdown } from "@/components/Markdown";
import { api, ITERATE_SKILL, type Artifact, type Session } from "@/lib/api";
import { cn } from "@/lib/utils";

const IMAGE = /\.(png|jpe?g|gif|webp|svg)$/i;

const KIND_LABEL: Record<string, string> = {
  task: "task",
  research_questions: "questions",
  research: "research",
  design: "design",
  outline: "outline",
  plan: "plan",
  prd: "PRD",
  tdd: "TDD",
  pr_description: "PR",
  other: "file",
};

/** The task's docs folder: file list, rendered Markdown, and Iterate. */
export function TaskDocs(props: {
  taskId: number;
  slug: string;
  /** Changes when the backend says the folder changed. */
  refreshKey: unknown;
  onSessionStarted: (s: Session) => void;
  /** Linear ticket identifier when the task is linked. */
  linearId?: string | null;
}) {
  const [files, setFiles] = useState<Artifact[] | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const [text, setText] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [posted, setPosted] = useState<string | null>(null);

  const load = useCallback(() => {
    api
      .listArtifacts(props.taskId)
      .then((f) => {
        setFiles(f);
        // Default to the newest non-task doc.
        setSel((cur) =>
          cur && f.some((a) => a.path === cur) ? cur : (f.find((a) => a.kind !== "task") ?? f[0])?.path ?? null,
        );
      })
      .catch((e) => setErr(String(e)));
  }, [props.taskId]);

  useEffect(load, [load, props.refreshKey]);

  const current = files?.find((f) => f.path === sel);
  const isImage = !!sel && IMAGE.test(sel);
  useEffect(() => {
    setText(null);
    setPosted(null);
    if (!sel) return;
    if (IMAGE.test(sel)) {
      api
        .readArtifactDataUrl(props.taskId, sel)
        .then(setText)
        .catch((e) => setText(`Could not read ${sel}: ${e}`));
      return;
    }
    api
      .readArtifact(props.taskId, sel)
      .then(setText)
      .catch((e) => setText(`*Could not read ${sel}: ${e}*`));
    // Re-read when the file changes on disk.
  }, [props.taskId, sel, current?.updated_at]);

  if (err) return <p className="whitespace-pre-wrap text-destructive">{err}</p>;
  if (!files) return <p className="text-muted-foreground">Loading docs…</p>;

  const iterSkill = current && ITERATE_SKILL[current.kind];
  // Image links in a doc are relative to the doc's folder.
  const resolveImage = (src: string) => {
    const dir = sel && sel.includes("/") ? sel.slice(0, sel.lastIndexOf("/") + 1) : "";
    return api.readArtifactDataUrl(props.taskId, dir + src.replace(/^\.\//, ""));
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex items-center justify-between gap-2 text-muted-foreground">
        <span>
          Docs folder: <code>.humanlayer/tasks/{props.slug}</code> (linked into each session's folder)
        </span>
        <Button size="sm" variant="ghost" onClick={load}>
          <RefreshCw /> Refresh
        </Button>
      </div>
      {files.length <= 1 && (
        <p className="text-muted-foreground">
          No docs yet. Move the task to Research on the board, or start a session with a skill such as{" "}
          <code>/rpi:create-research</code>.
        </p>
      )}
      {files.length > 0 && (
        <div className="flex min-h-0 flex-1 border">
          <ul className="w-56 flex-none overflow-auto border-r">
            {files.map((f) => (
              <li
                key={f.path}
                title={f.path}
                className={cn(
                  "flex cursor-pointer items-center gap-2 px-2 py-1 hover:bg-accent/10",
                  sel === f.path && "bg-accent/20",
                )}
                onClick={() => {
                  setSel(f.path);
                  setFeedback(null);
                }}
              >
                <span className="w-16 flex-none text-xs text-muted-foreground">{KIND_LABEL[f.kind] ?? f.kind}</span>
                <span className="min-w-0 flex-1 truncate">{f.path}</span>
              </li>
            ))}
          </ul>
          <div className="flex min-w-0 flex-1 flex-col">
            {current && (
              <div className="flex items-center justify-between gap-2 border-b px-3 py-1.5">
                <span className="truncate text-muted-foreground">
                  {current.path} · {new Date(Number(current.updated_at) * 1000).toLocaleString()}
                </span>
                <span className="flex flex-none items-center gap-2">
                  {posted && <span className="text-muted-foreground">{posted}</span>}
                  {props.linearId && current.path.endsWith(".md") && current.path !== "ticket.md" && (
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={busy}
                      title={`Post this doc to ${props.linearId} as a comment; posting again updates that comment`}
                      onClick={() => {
                        setBusy(true);
                        setErr(null);
                        setPosted(null);
                        api
                          .postDocToLinear(props.taskId, current.path)
                          .then((r) => setPosted(`${r === "updated" ? "Updated" : "Posted"} on ${props.linearId}`))
                          .catch((x) => setPosted(`Could not post: ${x}`))
                          .finally(() => setBusy(false));
                      }}
                    >
                      <Send /> Post to Linear
                    </Button>
                  )}
                  {iterSkill && feedback === null && (
                    <Button size="sm" variant="outline" onClick={() => setFeedback("")}>
                      <MessageSquarePlus /> Iterate
                    </Button>
                  )}
                </span>
              </div>
            )}
            {current && feedback !== null && (
              <form
                className="flex flex-col gap-2 border-b p-3"
                onSubmit={(e) => {
                  e.preventDefault();
                  setBusy(true);
                  setErr(null);
                  api
                    .iterateArtifact(props.taskId, current.path, feedback)
                    .then((s) => {
                      setFeedback(null);
                      props.onSessionStarted(s);
                    })
                    .catch((x) => setErr(String(x)))
                    .finally(() => setBusy(false));
                }}
              >
                <label className="text-muted-foreground">
                  Feedback for <code>/rpi:{iterSkill}</code> on {current.path}
                </label>
                <Textarea
                  autoFocus
                  rows={4}
                  value={feedback}
                  onChange={(e) => setFeedback(e.target.value)}
                  placeholder="What should change?"
                />
                <div className="flex justify-end gap-2">
                  <Button type="button" size="sm" variant="ghost" onClick={() => setFeedback(null)}>
                    Cancel
                  </Button>
                  <Button type="submit" size="sm" disabled={!feedback.trim() || busy}>
                    {busy ? "Starting…" : "Start iterate session"}
                  </Button>
                </div>
              </form>
            )}
            <div className="min-h-0 flex-1 overflow-auto p-4">
              {text === null ? (
                <p className="text-muted-foreground">Loading…</p>
              ) : isImage ? (
                text.startsWith("data:") ? (
                  <img src={text} alt={sel ?? ""} className="max-w-full" />
                ) : (
                  <p className="text-destructive">{text}</p>
                )
              ) : sel?.endsWith(".md") ? (
                <Markdown text={text} resolveImage={resolveImage} />
              ) : (
                <pre className="text-xs whitespace-pre-wrap">{text}</pre>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
