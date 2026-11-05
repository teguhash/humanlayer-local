// Session transcript: loads stored events, then follows live ones. Every
// event is in SQLite, so this survives leaving the view and app restarts.
import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  api,
  type Decision,
  type Item,
  type Session,
  type SessionEvent,
  type SessionUsage,
} from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

type Entry = { key: string; seq: number; item: Item };

export function SessionTranscript({ session }: { session: Session }) {
  const id = session.id;
  const [events, setEvents] = useState<Map<number, SessionEvent>>(new Map());
  const [live, setLive] = useState(false);
  const [pending, setPending] = useState<Set<string>>(new Set());
  const [usage, setUsage] = useState<SessionUsage | null>(null);
  const [prompt, setPrompt] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [raw, setRaw] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  const fail = (e: unknown) => setError(String(e));
  const running = session.status === "running";

  // Subscribe first, then load, so nothing between the two is lost;
  // events are keyed by seq, so duplicates collapse.
  useEffect(() => {
    let alive = true;
    const apply = (ev: SessionEvent) => {
      for (const item of ev.items) {
        if (item.type === "process_started") setLive(true);
        if (item.type === "process_exit") {
          setLive(false);
          setPending(new Set());
        }
        if (item.type === "approval_request")
          setPending((p) => new Set(p).add(item.request_id));
        if (item.type === "approval_response")
          setPending((p) => {
            const n = new Set(p);
            n.delete(item.request_id);
            return n;
          });
        if (item.type === "turn_end") {
          setPending(new Set());
          api.getUsage(id).then((u) => alive && setUsage(u), fail);
        }
      }
    };
    const un = listen<SessionEvent>("session-event", ({ payload: ev }) => {
      if (ev.session_id !== id) return;
      setEvents((m) => new Map(m).set(ev.seq < 0 ? -Date.now() : ev.seq, ev));
      apply(ev);
    });
    api.getTranscript(id).then((t) => {
      if (!alive) return;
      setEvents((m) => {
        const n = new Map(m);
        for (const e of t.events) n.set(e.seq, e);
        return n;
      });
      setLive(t.live);
      setPending(new Set(t.pending_approvals));
      setUsage(t.usage);
      setLoaded(true);
    }, fail);
    return () => {
      alive = false;
      un.then((f) => f());
    };
  }, [id]);

  const sorted = useMemo(() => [...events.values()].sort((a, b) => a.seq - b.seq), [events]);
  const entries: Entry[] = useMemo(
    () =>
      sorted.flatMap((e) =>
        e.items.map((item, i) => ({ key: `${e.seq}-${i}`, seq: e.seq, item })),
      ),
    [sorted],
  );
  const results = useMemo(() => {
    const m = new Map<string, Extract<Item, { type: "tool_result" }>>();
    for (const { item } of entries) if (item.type === "tool_result") m.set(item.tool_use_id, item);
    return m;
  }, [entries]);
  const toolIds = useMemo(
    () => new Set(entries.flatMap(({ item }) => (item.type === "tool_use" ? [item.id] : []))),
    [entries],
  );
  const decisions = useMemo(() => {
    const m = new Map<string, Decision>();
    for (const { item } of entries)
      if (item.type === "approval_response") m.set(item.request_id, item.decision);
    return m;
  }, [entries]);

  useEffect(() => {
    const el = listRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [events, raw]);

  async function send() {
    const text = prompt.trim();
    if (!text) return;
    setError(null);
    try {
      await api.sendMessage(id, text);
      setPrompt("");
    } catch (e) {
      fail(e);
    }
  }

  function render({ key, item }: Entry) {
    switch (item.type) {
      case "user_prompt":
        return (
          <div key={key} className="border-l-2 border-accent bg-accent/10 px-3 py-2 whitespace-pre-wrap">
            {item.text}
          </div>
        );
      case "assistant_text":
        return (
          <div key={key} className="px-3 py-1 whitespace-pre-wrap">
            {item.text}
          </div>
        );
      case "thinking":
        return (
          <details key={key} className="px-3 text-muted-foreground">
            <summary className="cursor-pointer">Thinking</summary>
            <div className="whitespace-pre-wrap">{item.text}</div>
          </details>
        );
      case "tool_use":
        return <ToolCall key={key} name={item.name} input={item.input} result={results.get(item.id)} />;
      case "tool_result":
        // Shown inside its tool call; orphans get their own row.
        return toolIds.has(item.tool_use_id) ? null : (
          <ToolCall key={key} name="(tool result)" input={null} result={item} />
        );
      case "approval_request": {
        const decided = decisions.get(item.request_id);
        const open = pending.has(item.request_id) && !decided;
        return (
          <div key={key} className="mx-3 border border-accent bg-accent/10 p-2">
            <div className="mb-1">
              <span className="font-semibold">{item.tool_name}</span> wants to run
              {item.description ? `: ${item.description}` : ""}
            </div>
            <pre className="mb-2 max-h-40 overflow-auto text-xs whitespace-pre-wrap">
              {JSON.stringify(item.input, null, 2)}
            </pre>
            {open ? (
              <div className="flex flex-wrap gap-2">
                <Button size="xs" onClick={() => answer(item.request_id, "allow_once")}>
                  Allow once
                </Button>
                <Button size="xs" variant="outline" onClick={() => answer(item.request_id, "allow_session")}>
                  Allow for session
                </Button>
                <Button size="xs" variant="destructive" onClick={() => answer(item.request_id, "deny")}>
                  Deny
                </Button>
              </div>
            ) : (
              <em className="text-muted-foreground">
                {decided ? DECISION_LABEL[decided] : "Expired (the process ended before an answer)"}
              </em>
            )}
          </div>
        );
      }
      case "approval_response":
        return null;
      case "turn_end":
        return (
          <div
            key={key}
            className={cn("px-3 text-xs", item.ok ? "text-muted-foreground" : "text-destructive")}
          >
            {item.ok ? "Turn finished" : `Turn ended: ${item.subtype ?? "error"}`}
            {item.duration_ms != null && ` · ${(item.duration_ms / 1000).toFixed(1)}s`}
          </div>
        );
      case "session_init":
        return null;
      case "process_started":
        return (
          <div key={key} className="flex items-center gap-2 px-3 text-xs text-muted-foreground">
            <span className="h-px flex-1 bg-border" />
            {item.resume ? "Resumed CLI session" : "Started CLI"}
            <span className="h-px flex-1 bg-border" />
          </div>
        );
      case "process_exit":
        if (item.success && !item.stopped) return null;
        return (
          <div key={key} className={cn("px-3 text-xs", item.stopped ? "text-muted-foreground" : "text-destructive")}>
            {item.stopped ? "Process stopped" : `Process exited with code ${item.code ?? "signal"}`}
          </div>
        );
      case "notice":
        return (
          <div
            key={key}
            className={cn(
              "px-3 text-xs whitespace-pre-wrap",
              item.level === "error" && "text-destructive",
              item.level === "warn" && "text-[var(--terminal-warning)]",
              item.level === "info" && "text-muted-foreground",
            )}
          >
            {item.text}
          </div>
        );
    }
  }

  async function answer(requestId: string, decision: Decision) {
    try {
      await api.respondApproval(id, requestId, decision);
    } catch (e) {
      fail(e);
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div
        ref={listRef}
        className="min-h-0 flex-1 overflow-auto border bg-secondary/30 py-2"
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        {loaded && sorted.length === 0 && (
          <p className="px-3 text-muted-foreground">No messages yet. Send a prompt to start.</p>
        )}
        {raw ? (
          sorted.map((e) => <RawRow key={e.seq} e={e} />)
        ) : (
          <div className="flex flex-col gap-2">{entries.map(render)}</div>
        )}
        {running && !raw && <div className="px-3 text-muted-foreground animate-pulse">Working…</div>}
      </div>

      {error && (
        <div className="flex justify-between gap-2 border border-destructive bg-destructive/10 p-2 text-destructive">
          <span className="whitespace-pre-wrap">{error}</span>
          <button className="cursor-pointer" onClick={() => setError(null)}>
            ×
          </button>
        </div>
      )}

      <form
        className="flex flex-col gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          send();
        }}
      >
        <Textarea
          placeholder={running ? "Wait for the turn to finish, or interrupt it" : "Message (⌘↵ to send)"}
          value={prompt}
          rows={3}
          onChange={(e) => setPrompt(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              if (!running) send();
            }
          }}
        />
        <div className="flex flex-wrap items-center gap-2">
          <Button type="submit" size="sm" disabled={running || !prompt.trim()}>
            Send
          </Button>
          {running && (
            <Button type="button" size="sm" variant="outline" onClick={() => api.interrupt(id).catch(fail)}>
              Interrupt
            </Button>
          )}
          {live && (
            <Button type="button" size="sm" variant="destructive" onClick={() => api.stop(id).catch(fail)}>
              Stop process
            </Button>
          )}
          <Badge variant="outline">{session.status}</Badge>
          {usage && <UsageLine usage={usage} />}
          <label className="ml-auto flex cursor-pointer items-center gap-1 text-muted-foreground">
            <input type="checkbox" checked={raw} onChange={(e) => setRaw(e.target.checked)} />
            Raw events
          </label>
        </div>
      </form>
    </div>
  );
}

const DECISION_LABEL: Record<Decision, string> = {
  allow_once: "Allowed once",
  allow_session: "Allowed for this session",
  deny: "Denied",
};

function summarize(name: string, input: any): string {
  if (!input || typeof input !== "object") return "";
  const v = input.command ?? input.file_path ?? input.path ?? input.pattern ?? input.url ?? input.description;
  if (typeof v === "string") return v;
  const s = JSON.stringify(input);
  return name && s.length > 120 ? s.slice(0, 120) + "…" : s;
}

function ToolCall(props: {
  name: string;
  input: any;
  result?: { content: string; is_error: boolean };
}) {
  const r = props.result;
  return (
    <details className={cn("mx-3 border", r?.is_error && "border-destructive")}>
      <summary className="flex cursor-pointer gap-2 overflow-hidden px-2 py-1 whitespace-nowrap">
        <span className="font-semibold">{props.name}</span>
        <span className="truncate text-muted-foreground">{summarize(props.name, props.input)}</span>
        <span className={cn("ml-auto flex-none text-xs", r?.is_error ? "text-destructive" : "text-muted-foreground")}>
          {r ? (r.is_error ? "error" : "done") : "…"}
        </span>
      </summary>
      {props.input != null && (
        <pre className="max-h-60 overflow-auto border-t px-2 py-1 text-xs whitespace-pre-wrap">
          {JSON.stringify(props.input, null, 2)}
        </pre>
      )}
      {r && (
        <pre className="max-h-80 overflow-auto border-t px-2 py-1 text-xs break-all whitespace-pre-wrap">
          {r.content || "(no output)"}
        </pre>
      )}
    </details>
  );
}

function UsageLine({ usage }: { usage: SessionUsage }) {
  const t = usage.tokens;
  const k = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n));
  return (
    <span
      className="text-xs text-muted-foreground tabular-nums"
      title={`input ${t.input_tokens}, output ${t.output_tokens}, cache read ${t.cache_read_tokens}, cache write ${t.cache_creation_tokens}`}
    >
      {usage.turns} turns · in {k(t.input_tokens + t.cache_read_tokens + t.cache_creation_tokens)} · out{" "}
      {k(t.output_tokens)}
      {usage.cost_usd > 0 && ` · $${usage.cost_usd.toFixed(4)}`}
    </span>
  );
}

function RawRow({ e }: { e: SessionEvent }) {
  return (
    <details className="border-b">
      <summary className="flex cursor-pointer gap-3 px-2 py-1 whitespace-nowrap">
        <span className="w-10 flex-none text-muted-foreground tabular-nums">#{e.seq}</span>
        <span className="font-semibold">{e.event_type ?? e.kind}</span>
      </summary>
      <pre className="max-h-96 overflow-auto px-3 py-2 text-xs break-all whitespace-pre-wrap">
        {JSON.stringify(e.payload, null, 2)}
      </pre>
    </details>
  );
}
