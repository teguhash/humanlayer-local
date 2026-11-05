import { useState } from "react";
import { ExternalLink, GitMerge, GitPullRequest, RefreshCw } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { api, type PullRequest, type Task } from "@/lib/api";

/** One short line for board cards, e.g. "PR #5 · checks passing · approved". */
export function prLabel(pr: PullRequest): string {
  if (pr.state === "MERGED") return `PR #${pr.number} merged`;
  if (pr.state === "CLOSED") return `PR #${pr.number} closed`;
  const parts = [`PR #${pr.number}${pr.is_draft ? " draft" : ""}`];
  if (pr.checks_state && pr.checks_state !== "none") parts.push(`checks ${pr.checks_state}`);
  if (pr.review_state === "APPROVED") parts.push("approved");
  if (pr.review_state === "CHANGES_REQUESTED") parts.push("changes requested");
  if (pr.mergeable === "CONFLICTING") parts.push("conflicts");
  return parts.join(" · ");
}

const REVIEW: Record<string, string> = {
  APPROVED: "approved",
  CHANGES_REQUESTED: "changes requested",
  REVIEW_REQUIRED: "review required",
};

/** Why GitHub won't merge yet, from mergeStateStatus. */
const BLOCKED: Record<string, string> = {
  BLOCKED: "blocked by branch protection (reviews or required checks)",
  BEHIND: "branch is behind its base",
  DIRTY: "has merge conflicts",
  UNSTABLE: "some checks are failing",
  DRAFT: "is a draft",
};

/** The task's PR: open one from the worktree branch, or show its state and merge it. */
export function PullRequestPanel(props: { task: Task; pr?: PullRequest; onError: (e: unknown) => void }) {
  const t = props.task;
  const pr = props.pr;
  const [busy, setBusy] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [draft, setDraft] = useState(false);
  const [method, setMethod] = useState("squash");

  const run = (what: string, p: Promise<unknown>) => {
    setBusy(what);
    setErr(null);
    p.catch((e) => setErr(String(e))).finally(() => setBusy(null));
  };

  if (!pr) {
    if (!t.branch || t.worktree_status !== "ready" || t.archived_at) return null;
    return (
      <div className="flex flex-col gap-1 border p-2">
        <div className="flex items-center justify-between gap-2">
          <span className="text-muted-foreground">
            No pull request for <code>{t.branch}</code> yet. Opening one pushes the branch to <code>origin</code>.
          </span>
          <span className="flex flex-none items-center gap-2">
            <label className="flex items-center gap-1 text-muted-foreground">
              <input type="checkbox" checked={draft} onChange={(e) => setDraft(e.target.checked)} /> Draft
            </label>
            <Button size="sm" variant="outline" disabled={!!busy} onClick={() => run("open", api.openPr(t.id, draft))}>
              <GitPullRequest /> {busy === "open" ? "Opening…" : "Open PR"}
            </Button>
          </span>
        </div>
        {err && <p className="whitespace-pre-wrap text-destructive">{err}</p>}
      </div>
    );
  }

  const open = pr.state === "OPEN";
  const why = open ? BLOCKED[pr.merge_state] : undefined;
  return (
    <div className="flex flex-col gap-1 border p-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="flex flex-wrap items-center gap-2">
          <button
            className="inline-flex cursor-pointer items-center gap-1 font-medium underline-offset-2 hover:underline"
            onClick={() => openUrl(pr.url).catch(props.onError)}
            title="Open on GitHub"
          >
            <GitPullRequest className="size-4" /> {pr.repo}#{pr.number} <ExternalLink className="size-3" />
          </button>
          <Badge variant={pr.state === "MERGED" ? "default" : "outline"}>
            {pr.is_draft && open ? "draft" : pr.state.toLowerCase()}
          </Badge>
          {open && pr.checks_state !== "none" && (
            <Badge variant={pr.checks_state === "failing" ? "destructive" : "secondary"}>
              checks {pr.checks_state}
            </Badge>
          )}
          {open && REVIEW[pr.review_state] && <Badge variant="secondary">{REVIEW[pr.review_state]}</Badge>}
          {open && pr.mergeable === "CONFLICTING" && <Badge variant="destructive">conflicts</Badge>}
        </span>
        <span className="flex flex-none items-center gap-2">
          <Button size="sm" variant="ghost" disabled={!!busy} onClick={() => run("refresh", api.refreshPr(pr.id))}>
            <RefreshCw /> {busy === "refresh" ? "Checking…" : "Refresh"}
          </Button>
          {open && (
            <>
              <select
                className="h-8 rounded-md border bg-background px-2"
                value={method}
                onChange={(e) => setMethod(e.target.value)}
                title="Merge method"
              >
                <option value="squash">Squash</option>
                <option value="merge">Merge commit</option>
                <option value="rebase">Rebase</option>
              </select>
              <Button
                size="sm"
                disabled={!!busy || pr.is_draft}
                title={why ? `GitHub says this PR ${why}` : "Merge on GitHub (gh pr merge); the branch is kept"}
                onClick={() => run("merge", api.mergePr(pr.id, method))}
              >
                <GitMerge /> {busy === "merge" ? "Merging…" : "Merge"}
              </Button>
            </>
          )}
        </span>
      </div>
      {why && <p className="text-muted-foreground">Not ready: this PR {why}.</p>}
      {err && <p className="whitespace-pre-wrap text-destructive">{err}</p>}
    </div>
  );
}
