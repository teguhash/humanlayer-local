import { useCallback, useEffect, useState } from "react";
import { KeyRound, RefreshCw, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api, type LinearStatus } from "@/lib/api";

/** App settings. For now: the Linear API key (kept in the macOS Keychain). */
export function Settings() {
  const [st, setSt] = useState<LinearStatus | null>(null);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const load = useCallback(() => {
    api
      .linearStatus()
      .then(setSt)
      .catch((e) => setErr(String(e)));
  }, []);
  useEffect(load, [load]);

  const run = (p: Promise<unknown>) => {
    setBusy(true);
    setErr(null);
    p.catch((e) => setErr(String(e))).finally(() => {
      setBusy(false);
      load();
    });
  };

  return (
    <div className="flex max-w-xl flex-col gap-4">
      <h1 className="text-lg font-semibold">Settings</h1>
      <section className="flex flex-col gap-2 border p-3">
        <h2 className="font-semibold">Linear</h2>
        <p className="text-muted-foreground">
          A personal API key from Linear (Settings → Security &amp; access → Personal API keys). It is checked with
          Linear, then stored in your macOS Keychain. Sessions get it as <code>LINEAR_API_KEY</code> so the{" "}
          <code>linear</code> CLI works inside them.
        </p>
        {st?.has_key ? (
          <>
            <p>
              Key saved{st.account ? <> · connected as {st.account}</> : null}.
            </p>
            <p className="text-muted-foreground">
              Linked tickets are checked every 60 seconds
              {st.last_poll ? `; last check ${new Date(Number(st.last_poll) * 1000).toLocaleTimeString()}` : ""}.
            </p>
            {st.last_error && <p className="whitespace-pre-wrap text-destructive">{st.last_error}</p>}
            <div className="flex gap-2">
              <Button size="sm" variant="outline" disabled={busy} onClick={() => run(api.syncLinearNow())}>
                <RefreshCw /> Check now
              </Button>
              <Button size="sm" variant="outline" disabled={busy} onClick={() => run(api.clearLinearKey())}>
                <Trash2 /> Remove key
              </Button>
            </div>
          </>
        ) : (
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              run(api.setLinearKey(key).then(() => setKey("")));
            }}
          >
            <Input
              type="password"
              autoComplete="off"
              placeholder="lin_api_…"
              value={key}
              onChange={(e) => setKey(e.target.value)}
            />
            <Button type="submit" disabled={!key.trim() || busy}>
              <KeyRound /> {busy ? "Checking…" : "Save"}
            </Button>
          </form>
        )}
        {err && <p className="whitespace-pre-wrap text-destructive">{err}</p>}
      </section>
    </div>
  );
}
