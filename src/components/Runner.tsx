// The M0 spike runner, embedded in the session view. Events are kept in
// memory only; persisting them to the events table is M2.
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type Agent, type AgentEvent } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

const MAX_ROWS = 2000;
const APPROVAL_TYPE = "control_request/can_use_tool";

type Row = AgentEvent & { key: string };

function label(r: Row): string {
  if (r.kind === "event") return r.event_type ?? "(no type)";
  if (r.kind === "exit") {
    const p = r.payload;
    if (p.stopped) return "exit (stopped)";
    return p.success ? "exit 0" : `exit ${p.code ?? "signal"}`;
  }
  return r.kind;
}

function tone(r: Row): string {
  if (r.kind === "error" || r.kind === "invalid_json") return "bg-destructive/15 text-destructive";
  if (r.kind === "exit" && !r.payload.success && !r.payload.stopped)
    return "bg-destructive/15 text-destructive";
  if (r.kind === "stderr") return "bg-[var(--terminal-warning)]/15";
  if (r.event_type === APPROVAL_TYPE) return "bg-accent/15";
  return "";
}

function preview(r: Row): string {
  const p = r.payload;
  if (r.kind === "stderr" || r.kind === "invalid_json") return p.line;
  if (r.kind === "error") return p.message;
  if (r.kind === "started") return `${p.binary} ${p.args.join(" ")}`;
  if (r.event_type === APPROVAL_TYPE)
    return `${p.request.tool_name}: ${JSON.stringify(p.request.input)}`;
  const text =
    p?.message?.content?.find?.((c: any) => c.type === "text")?.text ??
    p?.item?.text ??
    p?.result;
  const s = typeof text === "string" ? text : JSON.stringify(p);
  return s.length > 200 ? s.slice(0, 200) + "…" : s;
}

export function Runner({ agent, cwd }: { agent: Agent; cwd: string }) {
  const [prompt, setPrompt] = useState("");
  const [rows, setRows] = useState<Row[]>([]);
  const [running, setRunning] = useState<string[]>([]);
  const [answered, setAnswered] = useState<Record<string, "allowed" | "denied">>({});
  const [error, setError] = useState<string | null>(null);
  const mine = useRef(new Set<string>());
  const listRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    const un = listen<AgentEvent>("agent-event", ({ payload: ev }) => {
      // Only runs started from this view. The started event can arrive
      // before invoke resolves, so accept it by matching the cwd too.
      if (!mine.current.has(ev.run_id)) {
        if (ev.kind === "started" && ev.payload.cwd === cwd && ev.payload.agent === agent)
          mine.current.add(ev.run_id);
        else return;
      }
      setRows((prev) => {
        const next = [...prev, { ...ev, key: `${ev.run_id}-${ev.seq}` }];
        return next.length > MAX_ROWS ? next.slice(-MAX_ROWS) : next;
      });
      if (ev.kind === "exit") setRunning((r) => r.filter((id) => id !== ev.run_id));
    });
    return () => {
      un.then((f) => f());
    };
  }, [agent, cwd]);

  useEffect(() => {
    const el = listRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [rows]);

  async function send() {
    setError(null);
    try {
      const id = await api.startRun(agent, cwd, prompt);
      mine.current.add(id);
      setRunning((r) => [...r, id]);
      setPrompt("");
    } catch (e) {
      setError(String(e));
    }
  }

  async function answer(r: Row, allow: boolean) {
    try {
      await api.respondApproval(r.run_id, r.payload.request_id, allow, r.payload.request.input);
      setAnswered((a) => ({ ...a, [r.key]: allow ? "allowed" : "denied" }));
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <form
        className="flex flex-col gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          send();
        }}
      >
        <Textarea
          placeholder={`Prompt for ${agent} in ${cwd}  (⌘↵ to send)`}
          value={prompt}
          rows={3}
          onChange={(e) => setPrompt(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              send();
            }
          }}
        />
        <div className="flex flex-wrap gap-2">
          <Button type="submit" size="sm" disabled={!prompt.trim()}>
            Send
          </Button>
          {running.map((id) => (
            <Button
              key={id}
              type="button"
              size="sm"
              variant="destructive"
              onClick={() => api.stopRun(id).catch((e) => setError(String(e)))}
            >
              Stop {id}
            </Button>
          ))}
          <Button
            type="button"
            size="sm"
            variant="ghost"
            className="ml-auto"
            onClick={() => setRows([])}
          >
            Clear
          </Button>
        </div>
        {error && (
          <div className="border border-destructive bg-destructive/10 p-2 text-destructive whitespace-pre-wrap">
            {error}
          </div>
        )}
      </form>

      <div
        ref={listRef}
        className="min-h-0 flex-1 overflow-auto border bg-secondary/40"
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        {rows.length === 0 && (
          <div className="p-4 text-muted-foreground">
            No events yet. Events are not saved yet (M2): they clear when you leave this session.
          </div>
        )}
        {rows.map((r) => (
          <div key={r.key} className={cn("border-b", tone(r))}>
            <details>
              <summary className="flex cursor-pointer gap-3 overflow-hidden px-2 py-1 whitespace-nowrap">
                <span className="w-16 flex-none text-muted-foreground tabular-nums">
                  {r.run_id}#{r.seq}
                </span>
                <span className="min-w-56 flex-none font-semibold">{label(r)}</span>
                <span className="overflow-hidden text-ellipsis text-muted-foreground">
                  {preview(r)}
                </span>
              </summary>
              <pre className="max-h-96 overflow-auto px-3 py-2 text-xs break-all whitespace-pre-wrap">
                {JSON.stringify(r.payload, null, 2)}
              </pre>
            </details>
            {r.event_type === APPROVAL_TYPE && (
              <div className="flex items-center gap-2 px-2 pb-2 pl-20">
                {answered[r.key] ? (
                  <em>{answered[r.key]}</em>
                ) : running.includes(r.run_id) ? (
                  <>
                    <span>Approve {r.payload.request.tool_name}?</span>
                    <Button size="xs" onClick={() => answer(r, true)}>
                      Allow
                    </Button>
                    <Button size="xs" variant="destructive" onClick={() => answer(r, false)}>
                      Deny
                    </Button>
                  </>
                ) : (
                  <em className="text-muted-foreground">run ended</em>
                )}
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
