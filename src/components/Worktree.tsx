import { useEffect, useState } from "react";
import { Archive, GitBranch, Loader2, RotateCcw } from "lucide-react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button, buttonVariants } from "@/components/ui/button";
import { api, type Task } from "@/lib/api";
import { cn } from "@/lib/utils";

const busy = (t: Task) => t.worktree_status === "creating" || t.worktree_status === "setting_up";

/** Worktree status for a task, with create / retry actions and the setup log. */
export function WorktreePanel(props: { task: Task; repoPath?: string }) {
  const t = props.task;
  const [err, setErr] = useState<string | null>(null);
  const [showLog, setShowLog] = useState(false);
  const st = t.worktree_status;
  const failed = st === "failed" || st === "setup_failed";

  // Open the log automatically when something goes wrong.
  useEffect(() => {
    if (failed) setShowLog(true);
  }, [failed]);

  const run = (p: Promise<unknown>) => {
    setErr(null);
    p.catch((e) => setErr(String(e)));
  };

  let line: React.ReactNode;
  let action: React.ReactNode = null;
  if (st === null || st === "failed") {
    line = (
      <>
        {st === "failed" && <span className="text-destructive">Creating the worktree failed. </span>}
        No worktree: new sessions run in the repo folder <code>{props.repoPath}</code>.
      </>
    );
    if (!t.archived_at)
      action = (
        <Button size="sm" variant="outline" onClick={() => run(api.createWorktree(t.id))}>
          <GitBranch /> {t.branch ? `Check out ${t.branch} in a worktree` : "Create worktree"}
        </Button>
      );
  } else if (busy(t)) {
    line = (
      <span className="flex items-center gap-2">
        <Loader2 className="size-4 animate-spin" />
        {st === "creating" ? "Creating worktree…" : "Running setup command…"}
      </span>
    );
  } else {
    line = (
      <>
        <span className={cn(st === "setup_failed" && "text-destructive")}>
          {st === "setup_failed" ? "Setup command failed" : "Worktree ready"}
        </span>
        {" · "}
        <code>{t.branch}</code> from <code>{t.base_ref || t.base_commit?.slice(0, 8)}</code>
        {" · "}
        <code className="break-all">{t.worktree_path}</code>
      </>
    );
    action = (
      <Button size="sm" variant="outline" onClick={() => run(api.retrySetup(t.id))}>
        <RotateCcw /> {st === "setup_failed" ? "Retry setup" : "Rerun setup"}
      </Button>
    );
  }

  return (
    <div className="flex flex-col gap-2 border p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">{line}</div>
        <div className="flex flex-none gap-2">
          {t.worktree_log && (
            <Button size="sm" variant="ghost" onClick={() => setShowLog((v) => !v)}>
              {showLog ? "Hide log" : "Log"}
            </Button>
          )}
          {action}
        </div>
      </div>
      {err && <p className="whitespace-pre-wrap text-destructive">{err}</p>}
      {showLog && t.worktree_log && (
        <pre className="max-h-60 overflow-auto bg-secondary/40 p-2 text-xs whitespace-pre-wrap">
          {t.worktree_log}
        </pre>
      )}
    </div>
  );
}

/**
 * Archive button. Asks first; lists uncommitted files in the worktree and
 * makes the user tick a box before they are discarded.
 */
export function ArchiveTask(props: { task: Task; onDone: () => void }) {
  const t = props.task;
  const [open, setOpen] = useState(false);
  const [dirty, setDirty] = useState<string[] | null>(null);
  const [discard, setDiscard] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [working, setWorking] = useState(false);

  useEffect(() => {
    if (!open) return;
    setDirty(null);
    setDiscard(false);
    setErr(null);
    api
      .worktreeDirtyFiles(t.id)
      .then(setDirty)
      .catch((e) => setErr(String(e)));
  }, [open, t.id]);

  const hasWt = !!t.worktree_path;
  const blocked = busy(t) || dirty === null || (dirty.length > 0 && !discard);

  return (
    <AlertDialog open={open} onOpenChange={setOpen}>
      <AlertDialogTrigger asChild>
        <Button size="sm" variant="outline" title="Archive this task">
          <Archive /> Archive
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Archive “{t.title}”?</AlertDialogTitle>
          <AlertDialogDescription>
            Running sessions are stopped.{" "}
            {hasWt ? (
              <>
                The worktree folder <code className="break-all">{t.worktree_path}</code> is removed. Branch{" "}
                <code>{t.branch}</code> is kept, so you can restore the task later.
              </>
            ) : (
              "This task has no worktree."
            )}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {busy(t) && <p className="text-destructive">Wait for the worktree setup to finish.</p>}
        {hasWt && dirty === null && !err && <p className="text-muted-foreground">Checking for uncommitted changes…</p>}
        {dirty && dirty.length > 0 && (
          <div className="flex flex-col gap-2">
            <p className="text-destructive">
              {dirty.length} uncommitted {dirty.length === 1 ? "file" : "files"} in the worktree will be lost:
            </p>
            <pre className="max-h-40 overflow-auto bg-secondary/40 p-2 text-xs">{dirty.join("\n")}</pre>
            <label className="flex items-center gap-2">
              <input type="checkbox" checked={discard} onChange={(e) => setDiscard(e.target.checked)} />
              Discard these changes
            </label>
          </div>
        )}
        {err && <p className="whitespace-pre-wrap text-destructive">{err}</p>}
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            className={buttonVariants({ variant: dirty?.length ? "destructive" : "default" })}
            disabled={blocked || working}
            onClick={(e) => {
              e.preventDefault();
              setWorking(true);
              setErr(null);
              api
                .archiveTask(t.id, discard)
                .then(() => {
                  setOpen(false);
                  props.onDone();
                })
                .catch((x) => setErr(String(x)))
                .finally(() => setWorking(false));
            }}
          >
            {working ? "Archiving…" : "Archive"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
