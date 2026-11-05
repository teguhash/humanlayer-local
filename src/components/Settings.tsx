import { useCallback, useEffect, useState } from "react";
import { ClipboardCopy, Download, KeyRound, RefreshCw, Trash2 } from "lucide-react";
import { getVersion } from "@tauri-apps/api/app";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api, type GithubStatus, type LinearStatus, type ToolCheck } from "@/lib/api";

export function ToolTable(props: { tools: ToolCheck | null }) {
  if (!props.tools) return <p className="text-muted-foreground">Checking…</p>;
  return (
    <table className="w-full text-left">
      <tbody>
        {props.tools.tools.map((t) => (
          <tr key={t.name} className="border-t align-top">
            <td className="py-1 pr-2 font-mono">{t.name}</td>
            <td className="py-1 pr-2">
              {t.version ? (
                <span title={t.path ?? ""}>{t.version}</span>
              ) : t.path ? (
                <span className="text-destructive">{t.error}</span>
              ) : (
                <span className={t.required ? "text-destructive" : "text-muted-foreground"}>
                  Not found{t.required ? " (required)" : ""}. Install: <code>{t.install}</code>
                </span>
              )}
            </td>
            <td className="py-1 text-muted-foreground">{t.purpose}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** App settings: the Linear API key and GitHub sign-in (secrets live in the macOS Keychain). */
export function Settings() {
  const [st, setSt] = useState<LinearStatus | null>(null);
  const [gh, setGh] = useState<GithubStatus | null>(null);
  const [token, setToken] = useState("");
  const [tools, setTools] = useState<ToolCheck | null>(null);
  const [version, setVersion] = useState("");
  const [update, setUpdate] = useState<Update | null>(null);
  const [updMsg, setUpdMsg] = useState<string | null>(null);
  const [diag, setDiag] = useState<string | null>(null);
  const [diagMsg, setDiagMsg] = useState<string | null>(null);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const load = useCallback(() => {
    api
      .linearStatus()
      .then(setSt)
      .catch((e) => setErr(String(e)));
    api
      .githubStatus()
      .then(setGh)
      .catch((e) => setErr(String(e)));
    api
      .checkTools()
      .then(setTools)
      .catch((e) => setErr(String(e)));
    getVersion()
      .then(setVersion)
      .catch(() => {});
  }, []);

  const checkUpdate = () => {
    setUpdMsg("Checking…");
    setUpdate(null);
    check()
      .then((u) => {
        setUpdate(u);
        setUpdMsg(u ? `Version ${u.version} is available.` : "You have the latest version.");
      })
      .catch((e) => setUpdMsg(`Could not check for updates: ${e}`));
  };

  const install = () => {
    if (!update) return;
    setUpdMsg("Downloading…");
    update
      .downloadAndInstall()
      .then(() => relaunch())
      .catch((e) => setUpdMsg(`Update failed: ${e}`));
  };

  const copyDiagnostics = () => {
    setDiagMsg(null);
    api
      .diagnostics()
      .then(async (text) => {
        setDiag(text);
        try {
          await navigator.clipboard.writeText(text);
          setDiagMsg("Copied. Keys, tokens, emails and your home folder are removed; check before sharing.");
        } catch {
          setDiagMsg("Could not copy automatically; select the text below and copy it.");
        }
      })
      .catch((e) => setDiagMsg(String(e)));
  };
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
        <div className="flex items-center justify-between">
          <h2 className="font-semibold">Tools</h2>
          <Button size="sm" variant="ghost" onClick={load}>
            <RefreshCw /> Check again
          </Button>
        </div>
        <p className="text-muted-foreground">Looked up on your login shell's PATH, the same one sessions use.</p>
        {tools?.env_error && <p className="text-destructive">{tools.env_error}</p>}
        <ToolTable tools={tools} />
      </section>
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
      </section>
      <section className="flex flex-col gap-2 border p-3">
        <h2 className="font-semibold">GitHub</h2>
        <p className="text-muted-foreground">
          The app uses the <code>gh</code> CLI and its login (<code>gh auth login</code>). If you can't use that, save a
          token here (fine-grained or classic, with access to your repos' contents, issues and pull requests). It is
          stored in your macOS Keychain and passed to <code>gh</code> and sessions as <code>GH_TOKEN</code>.
        </p>
        {gh && !gh.gh_installed && (
          <p className="text-destructive">
            <code>gh</code> is not installed. Install it with <code>brew install gh</code>.
          </p>
        )}
        {gh?.login ? (
          <p>
            Signed in as {gh.login} ({gh.source === "GH_TOKEN" ? "saved token" : `gh login, ${gh.source}`}).
          </p>
        ) : (
          gh?.gh_installed && <p className="text-muted-foreground">gh is not signed in.</p>
        )}
        {gh?.error && <p className="whitespace-pre-wrap text-destructive">{gh.error}</p>}
        {gh?.has_token ? (
          <div>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => run(api.clearGithubToken())}>
              <Trash2 /> Remove token (use gh's login)
            </Button>
          </div>
        ) : (
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              run(api.setGithubToken(token).then(() => setToken("")));
            }}
          >
            <Input
              type="password"
              autoComplete="off"
              placeholder="Optional token: github_pat_… or ghp_…"
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
            <Button type="submit" variant="outline" disabled={!token.trim() || busy}>
              <KeyRound /> Save
            </Button>
          </form>
        )}
      </section>
      <section className="flex flex-col gap-2 border p-3">
        <h2 className="font-semibold">Updates</h2>
        <p className="text-muted-foreground">
          Version {version || "?"}. Updates come from the project's GitHub Releases and are verified with the
          release signing key before installing.
        </p>
        <div className="flex items-center gap-2">
          <Button size="sm" variant="outline" onClick={checkUpdate}>
            <RefreshCw /> Check for updates
          </Button>
          {update && (
            <Button size="sm" onClick={install}>
              <Download /> Install {update.version} and restart
            </Button>
          )}
        </div>
        {updMsg && <p className="whitespace-pre-wrap text-muted-foreground">{updMsg}</p>}
      </section>
      <section className="flex flex-col gap-2 border p-3">
        <h2 className="font-semibold">Diagnostics</h2>
        <p className="text-muted-foreground">
          Logs are in <code>~/Library/Logs/dev.teguh.humanlayerlocal/</code>. Copy a report for a bug report: app and
          tool versions, sign-in state (no account names), and the last 200 log lines.
        </p>
        <div>
          <Button size="sm" variant="outline" onClick={copyDiagnostics}>
            <ClipboardCopy /> Copy diagnostics
          </Button>
        </div>
        {diagMsg && <p className="text-muted-foreground">{diagMsg}</p>}
        {diag && (
          <textarea
            readOnly
            className="h-48 w-full border bg-background p-2 font-mono text-xs"
            value={diag}
            onFocus={(e) => e.currentTarget.select()}
          />
        )}
      </section>
      {err && <p className="whitespace-pre-wrap text-destructive">{err}</p>}
    </div>
  );
}
