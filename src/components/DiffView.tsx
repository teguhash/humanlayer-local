import { useCallback, useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { api, type DiffSummary, type FileChange } from "@/lib/api";
import { cn } from "@/lib/utils";

const LETTER: Record<FileChange["status"], string> = {
  added: "A",
  modified: "M",
  deleted: "D",
  renamed: "R",
  copied: "C",
  type_changed: "T",
  untracked: "U",
};

/** Changed files in the task's worktree vs. its base branch, and one file's diff. */
export function DiffView(props: { taskId: number; refreshKey?: unknown }) {
  const [sum, setSum] = useState<DiffSummary | null>(null);
  const [selected, setSelected] = useState<FileChange | null>(null);
  const [patch, setPatch] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(() => {
    setLoading(true);
    setErr(null);
    api
      .getDiff(props.taskId)
      .then((s) => {
        setSum(s);
        // Keep the selected file if it is still changed.
        setSelected((cur) => s.files.find((f) => f.path === cur?.path) ?? s.files[0] ?? null);
      })
      .catch((e) => setErr(String(e)))
      .finally(() => setLoading(false));
  }, [props.taskId]);

  useEffect(load, [load, props.refreshKey]);

  useEffect(() => {
    setPatch(null);
    if (!selected) return;
    api
      .getFileDiff(props.taskId, selected.path, selected.old_path)
      .then(setPatch)
      .catch((e) => setPatch(`Could not load diff: ${e}`));
  }, [props.taskId, selected, sum]);

  if (err) return <p className="whitespace-pre-wrap text-destructive">{err}</p>;
  if (!sum) return <p className="text-muted-foreground">Loading changes…</p>;

  const total = sum.files.reduce(
    (a, f) => [a[0] + (f.additions ?? 0), a[1] + (f.deletions ?? 0)],
    [0, 0],
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex items-center justify-between gap-2">
        <span className="text-muted-foreground">
          {sum.files.length} {sum.files.length === 1 ? "file" : "files"} changed vs <code>{sum.base}</code>{" "}
          <span className="text-[var(--terminal-success)]">+{total[0]}</span>{" "}
          <span className="text-destructive">−{total[1]}</span>
        </span>
        <Button size="sm" variant="ghost" onClick={load} disabled={loading}>
          <RefreshCw className={cn(loading && "animate-spin")} /> Refresh
        </Button>
      </div>
      {sum.files.length === 0 ? (
        <p className="text-muted-foreground">No changes yet.</p>
      ) : (
        <div className="flex min-h-0 flex-1 border">
          <ul className="w-64 flex-none overflow-auto border-r">
            {sum.files.map((f) => (
              <li
                key={f.path}
                title={f.old_path ? `${f.old_path} → ${f.path}` : f.path}
                className={cn(
                  "flex cursor-pointer items-center gap-2 px-2 py-1 hover:bg-accent/10",
                  selected?.path === f.path && "bg-accent/20",
                )}
                onClick={() => setSelected(f)}
              >
                <span
                  className={cn(
                    "w-3 flex-none text-xs font-semibold",
                    (f.status === "added" || f.status === "untracked") && "text-[var(--terminal-success)]",
                    f.status === "deleted" && "text-destructive",
                    f.status === "modified" && "text-[var(--terminal-warning)]",
                  )}
                >
                  {LETTER[f.status]}
                </span>
                <span className="min-w-0 flex-1 truncate" style={{ direction: "rtl", textAlign: "left" }}>
                  {f.path}
                </span>
                <span className="flex-none text-xs text-muted-foreground">
                  {f.additions === null ? "bin" : `+${f.additions} −${f.deletions ?? 0}`}
                </span>
              </li>
            ))}
          </ul>
          <div className="min-w-0 flex-1 overflow-auto">
            {patch === null ? (
              <p className="p-2 text-muted-foreground">Loading…</p>
            ) : (
              <Patch text={patch} />
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function Patch(props: { text: string }) {
  const lines = props.text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  if (lines.length === 0) return <p className="p-2 text-muted-foreground">No textual changes.</p>;
  return (
    <pre className="min-w-max p-2 text-xs leading-5">
      {lines.map((l, i) => (
        <div
          key={i}
          className={cn(
            l.startsWith("+") && !l.startsWith("+++") && "bg-[var(--terminal-success)]/15",
            l.startsWith("-") && !l.startsWith("---") && "bg-destructive/15",
            l.startsWith("@@") && "text-accent",
            /^(diff |index |--- |\+\+\+ |new file|deleted file|similarity|rename |old mode|new mode)/.test(l) &&
              "text-muted-foreground",
          )}
        >
          {l || " "}
        </div>
      ))}
    </pre>
  );
}
