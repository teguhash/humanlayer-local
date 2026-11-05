import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type Agent = "claude" | "codex";

type AgentEvent = {
  session_id: string;
  seq: number;
  kind: "started" | "event" | "invalid_json" | "stderr" | "exit" | "error";
  event_type: string | null;
  payload: any;
};

type Row = AgentEvent & { key: string; agent: Agent };

const MAX_ROWS = 2000;
const APPROVAL_TYPE = "control_request/can_use_tool";

function rowLabel(r: Row): string {
  if (r.kind === "event") return r.event_type ?? "(no type)";
  if (r.kind === "exit") {
    const p = r.payload;
    if (p.stopped) return "exit (stopped)";
    return p.success ? "exit 0" : `exit ${p.code ?? "signal"}`;
  }
  return r.kind;
}

function rowClass(r: Row): string {
  if (r.kind === "error" || r.kind === "invalid_json") return "row bad";
  if (r.kind === "exit" && !r.payload.success && !r.payload.stopped) return "row bad";
  if (r.kind === "stderr") return "row warn";
  if (r.event_type === APPROVAL_TYPE) return "row ask";
  return "row";
}

function preview(r: Row): string {
  const p = r.payload;
  if (r.kind === "stderr" || r.kind === "invalid_json") return p.line;
  if (r.kind === "error") return p.message;
  if (r.kind === "started") return `${p.binary} ${p.args.join(" ")}`;
  if (r.event_type === APPROVAL_TYPE)
    return `${p.request.tool_name}: ${JSON.stringify(p.request.input)}`;
  // Claude assistant text / Codex agent_message text, when present.
  const text =
    p?.message?.content?.find?.((c: any) => c.type === "text")?.text ??
    p?.item?.text ??
    p?.result;
  const s = typeof text === "string" ? text : JSON.stringify(p);
  return s.length > 200 ? s.slice(0, 200) + "…" : s;
}

export default function App() {
  const [agent, setAgent] = useState<Agent>("claude");
  const [cwd, setCwd] = useState("");
  const [prompt, setPrompt] = useState("");
  const [rows, setRows] = useState<Row[]>([]);
  const [running, setRunning] = useState<Record<string, Agent>>({});
  const [answered, setAnswered] = useState<Record<string, "allowed" | "denied">>({});
  const [startError, setStartError] = useState<string | null>(null);
  const [clock, setClock] = useState(() => new Date());
  const agentOf = useRef<Record<string, Agent>>({});
  const listRef = useRef<HTMLDivElement>(null);
  const stickToBottom = useRef(true);

  // Ticking clock: a visible check that the UI thread stays responsive.
  useEffect(() => {
    const t = setInterval(() => setClock(new Date()), 250);
    return () => clearInterval(t);
  }, []);

  useEffect(() => {
    const un = listen<AgentEvent>("agent-event", ({ payload: ev }) => {
      const a = agentOf.current[ev.session_id] ?? (ev.payload?.agent as Agent) ?? "claude";
      agentOf.current[ev.session_id] = a;
      setRows((prev) => {
        const next = [...prev, { ...ev, agent: a, key: `${ev.session_id}-${ev.seq}` }];
        return next.length > MAX_ROWS ? next.slice(-MAX_ROWS) : next;
      });
      if (ev.kind === "exit")
        setRunning(({ [ev.session_id]: _, ...rest }) => rest);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    const el = listRef.current;
    if (el && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [rows]);

  async function send() {
    setStartError(null);
    try {
      const id = await invoke<string>("start_session", { agent, cwd, prompt });
      agentOf.current[id] = agent;
      setRunning((r) => ({ ...r, [id]: agent }));
    } catch (e) {
      setStartError(String(e));
    }
  }

  async function answer(r: Row, allow: boolean) {
    try {
      await invoke("respond_approval", {
        sessionId: r.session_id,
        requestId: r.payload.request_id,
        allow,
        input: r.payload.request.input,
      });
      setAnswered((a) => ({ ...a, [r.key]: allow ? "allowed" : "denied" }));
    } catch (e) {
      setStartError(String(e));
    }
  }

  async function stop(id: string) {
    try {
      await invoke("stop_session", { sessionId: id });
    } catch (e) {
      setStartError(String(e));
    }
  }

  return (
    <main className="spike">
      <header>
        <strong>HumanLayer Local · M0 spike</strong>
        <span className="clock" title="Updates 4×/s; if it freezes, the UI is blocked">
          {clock.toLocaleTimeString()}.{String(clock.getMilliseconds()).padStart(3, "0").slice(0, 1)}
        </span>
      </header>

      <form
        className="controls"
        onSubmit={(e) => {
          e.preventDefault();
          send();
        }}
      >
        <select value={agent} onChange={(e) => setAgent(e.target.value as Agent)}>
          <option value="claude">claude</option>
          <option value="codex">codex</option>
        </select>
        <input
          className="cwd"
          placeholder="/path/to/working/folder"
          value={cwd}
          onChange={(e) => setCwd(e.target.value)}
        />
        <textarea
          placeholder="Prompt"
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
        <div className="actions">
          <button type="submit" disabled={!cwd || !prompt.trim()}>
            Send (⌘↵)
          </button>
          {Object.entries(running).map(([id, a]) => (
            <button type="button" key={id} className="stop" onClick={() => stop(id)}>
              Stop {a} {id}
            </button>
          ))}
          <button type="button" className="clear" onClick={() => setRows([])}>
            Clear
          </button>
        </div>
        {startError && <div className="start-error">{startError}</div>}
      </form>

      <div
        className="events"
        ref={listRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        {rows.length === 0 && <div className="empty">No events yet.</div>}
        {rows.map((r) => (
          <div key={r.key} className={rowClass(r)}>
            <details>
              <summary>
                <span className="sid">
                  {r.agent} {r.session_id}#{r.seq}
                </span>
                <span className="type">{rowLabel(r)}</span>
                <span className="preview">{preview(r)}</span>
              </summary>
              <pre>{JSON.stringify(r.payload, null, 2)}</pre>
            </details>
            {r.event_type === APPROVAL_TYPE && (
              <div className="approval">
                {answered[r.key] ? (
                  <em>{answered[r.key]}</em>
                ) : running[r.session_id] ? (
                  <>
                    <span>Approve {r.payload.request.tool_name}?</span>
                    <button onClick={() => answer(r, true)}>Allow</button>
                    <button onClick={() => answer(r, false)}>Deny</button>
                  </>
                ) : (
                  <em>session ended</em>
                )}
              </div>
            )}
          </div>
        ))}
      </div>
    </main>
  );
}
