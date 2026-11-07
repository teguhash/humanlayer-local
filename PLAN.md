# HumanLayer Local: plan and milestones

A local-only desktop app based on HumanLayer (Riptide), without the cloud and team features.

## Goals

Keep:

1. Desktop app (Tauri 2, Rust backend)
2. Skills, especially the research → plan → implement (RPI) flow
3. Grouping into projects, tasks and sessions
4. Workflow board
5. Claude CLI and Codex CLI integration
6. Linear and GitHub integration

Drop: cloud API, Electric sync, Postgres, orgs, teams, ACLs, billing, Slack, Jira, and multi-host daemons.

Estimate: 12–18 weeks solo full-time, or about 7–10 weeks with an agent writing most of the code.

## Architecture

- **Shell:** Tauri 2. Rust backend, React + TypeScript webview. Reuse the shadcn components from upstream `packages/ui`.
- **Storage:** SQLite (`sqlx` or `rusqlite`). One database file under `~/Library/Application Support/<app>/`.
- **Rust crates** (Cargo workspace under `src-tauri/crates/`, packages named `hl-*`):
  - `core` (`hl-core`): domain types, DB, migrations, login-shell env
  - `agents` (`hl-agents`): Claude and Codex adapters; emits through an `EventSink` trait, no Tauri dependency
  - `git` (`hl-git`): worktrees and diffs, shelling out to the `git` CLI
  - `integrations`: Linear (GraphQL over `reqwest`) and GitHub (`gh` CLI, or `octocrab` with a PAT)
- **Runtime:** agent processes are child processes of the Rust backend. Their events are written to SQLite and sent to the UI as Tauri events. No separate daemon.
- **Skills:** bundle the RPI skills from upstream `apps/riptide-rpi-claude-plugin` as a Claude plugin and as a Codex skills folder. They are Apache 2.0: keep the LICENSE and add a NOTICE.
- **Secrets:** Linear API key and GitHub PAT go in the macOS Keychain, never in SQLite.

### Data model

```
projects       (id, name, repo_path, created_at)
tasks          (id, project_id, title, stage, position, worktree_path,
                linear_issue_id, github_issue, created_at, updated_at)
sessions       (id, task_id, agent_kind [claude|codex], cli_session_id,
                skill, status, created_at, started_at, ended_at)
events         (id, session_id, seq, kind, payload_json, created_at)
artifacts      (id, task_id, kind [research|plan|outline|...], path, updated_at)
pull_requests  (id, task_id, repo, number, state, checks_state, review_state, updated_at)
```

### Board stages

Backlog → Research → Plan → Implement → Review → Done

Moving a task into a stage starts a session with that stage's skill:

| Stage | Skill |
|---|---|
| Research | `/create-research` |
| Plan | `/create-plan` |
| Implement | `/implement-plan` |
| Review | `/describe-pr`, then open the PR |

## Upstream reference code

Paths are in the upstream `humanlayer` repo:

| Area | Path |
|---|---|
| Claude adapter | `apps/riptide-daemon/src/agents/claudecode/` |
| Codex adapter | `apps/riptide-daemon/src/agents/codelayer/` |
| Shared session runner | `apps/riptide-daemon/src/agents/shared/run-session.ts` |
| GitHub via git/gh | `apps/riptide-daemon/src/lib/github/` |
| Linear client | `apps/riptide-api/src/lib/linear-service.ts`, `format-linear-ticket.ts` |
| Linear stage → status | `apps/riptide-api/src/lib/linear-status-progression.ts` |
| Board UI | `apps/riptide-ui/src/routes/_app/board.tsx` |
| Tauri host (PTY, shell env) | `apps/riptide-native/src/` |
| RPI skills | `apps/riptide-rpi-claude-plugin/skills/` |
| Linear CLI used by skills | `apps/linear-cli/` |

## Milestones

| # | Milestone | Weeks | Done when |
|---|---|---|---|
| M0 | Spike | 1 | One prompt round-trips through both `claude` and `codex` in the app |
| M1 | Data + shell | 2–3 | Pick a repo folder, then create, view and delete projects, tasks and sessions |
| M2 | Claude adapter | 4–6 | Streamed transcript, tool calls, approvals, interrupt and resume all work, and survive an app restart |
| M3 | Worktrees + diffs | 6–7 | Each task gets its own worktree; diff view shows its changes |
| M4 | Board + RPI flow | 8–10 | Drag a task to Research and a research session starts; its doc shows on the task |
| M5 | Codex adapter | 10–11 | Same session features as Claude, chosen per task |
| M6 | Linear | 12–13 | Import a ticket as a task; ticket status follows the task's stage |
| M7 | GitHub | 14–16 | Import an issue; open a PR from a task; PR checks and reviews show on the board |
| M8 | Packaging | 17–18 | Signed, notarized macOS build with auto-update |

### M0: Spike (week 1)

Answers the risky questions first.

- [x] Install prerequisites: Rust (rustup), Xcode command line tools (already present)
- [x] `npm run tauri dev` opens the scaffold window (spike screen confirmed by hand)
- [x] Rust command spawns `claude -p --output-format stream-json --input-format stream-json --verbose` and forwards each JSON line to the UI as a Tauri event (plus `--permission-mode default --permission-prompt-tool stdio`)
- [x] Same for `codex exec --json`
- [x] Decide the approval mechanism: Claude `--permission-prompt-tool` (an MCP tool served by the app) or hooks. Check current CLI flags first; they change between releases — decided: stdio control protocol (`--permission-prompt-tool stdio`), see docs/m0-findings.md
- [x] Write down the CLI versions tested (claude 2.1.314, codex 0.166.0, in docs/m0-findings.md)

### M1: Data + shell (weeks 2–3)

- [x] Cargo workspace with `core` crate; SQLite migrations for the data model above (crates are `hl-core`, `hl-agents`, `hl-git`: `core` clashes with Rust's built-in crate)
- [x] Tauri commands: project, task and session CRUD (create, list, delete; edit comes with the board in M4)
- [x] Sidebar: projects → tasks → sessions
- [x] Folder picker for a project's repo path; validate that it is a git repo (native picker confirmed by hand)
- [x] Copy shadcn components and Tailwind setup from upstream `packages/ui`

### M2: Claude adapter (weeks 4–6)

The hardest milestone.

- [x] `agents` crate: a `Agent` trait (start, send, interrupt, resume, stop) with a Claude implementation — the trait describes each CLI's protocol; `Runtime` provides send/interrupt/stop/approvals, and resume happens automatically on send
- [x] Parse stream events: assistant text, tool use, tool result, usage, result
- [x] Store every event in `events`; the transcript rebuilds from the DB after a restart
- [x] Approvals UI: allow once, allow for session, deny (session scope forced, never written to settings files)
- [x] Interrupt (stop the current turn) and resume using `cli_session_id`
- [x] Token usage and cost per session
- [x] On app start, mark sessions left running as interrupted

### M3: Worktrees + diffs (weeks 6–7)

- [x] `git` crate: create a worktree per task (`git worktree add`), with a branch named from the task
- [x] Run setup scripts in new worktrees (see upstream `setup-worktree` skill). Reads the same `.humanlayer/workspace.json` / `workspace.local.json` (primary repo entry only): pathTemplate, branchTemplate, sourceRef, copyGlobs, setupCommand, disabled
- [x] Diff view: changed files and per-file diff against the base branch (merge base, so later commits on the base branch don't show; includes uncommitted and untracked files)
- [x] Remove the worktree when a task is archived, after confirming (lists uncommitted files and needs an explicit "discard"; the branch is kept and Restore checks it out again)

Note: worktrees are created on request ("Create worktree" on the task), not on task creation, so research/plan sessions can run in the repo. A session's folder is fixed at its first turn because Claude keys `--resume` by folder.

### M4: Board + RPI flow (weeks 8–10)

- [x] Board view with drag-and-drop between stages and ordering within a column (native HTML5 drag and drop, no library; needs `dragDropEnabled: false` in tauri.conf.json; M4 flow confirmed by hand)
- [x] Moving into a stage starts a session with that stage's skill (`/rpi:<skill>`; moving into Implement creates the worktree first and starts the session when it is ready)
- [x] Bundle the RPI skills; install them as a Claude plugin at app start — loaded per session with `--plugin-dir` instead, so nothing is written to the user's Claude settings (docs/m4-findings.md)
- [x] Watch the task's artifact folder; list research, plan and outline docs on the task (folder lives in app data, linked into each session folder as `.humanlayer/tasks/<slug>`)
- [x] Markdown viewer for artifacts
- [x] Iterate buttons run `/iterate-research` and `/iterate-plan` with the user's feedback (also outline, research questions, design discussion)

### M5: Codex adapter (weeks 10–11)

- [x] Codex implementation of the `Agent` trait (`codex exec --json`, resume) (`--sandbox workspace-write`, no per-call approvals; docs/m5-findings.md)
- [x] Install skills into the Codex skills folder — linked per session into `.agents/skills/humanlayer-rpi` instead, so nothing is written to `~/.codex`
- [x] Per-task choice of agent; default set per project (Codex research run confirmed by hand)
- [x] Map Codex events into the same transcript UI (`file_change`, `mcp_tool_call`, `web_search`, `todo_list` added)

### M6: Linear (weeks 12–13)

- [x] Settings: Linear API key, stored in the Keychain (checked with Linear before saving; docs/m6-findings.md)
- [x] Import a ticket by ID or URL as a task (title, description, images) (ticket.md and images/ in the task's docs folder)
- [x] Poll linked tickets every 60 seconds while the app is open (title/description follow the ticket unless edited here)
- [x] Port the stage → status mapping; update the ticket when a task changes stage (forward only; M6 flow confirmed by hand)
- [x] Post links to research and plan docs on the ticket — docs are local, so a Post to Linear button posts the doc as a collapsible comment instead (reposting updates it)
- [x] Skills keep using the `linear` CLI with the same API key (sessions get `LINEAR_API_KEY`; the bundled skills don't call it today)

### M7: GitHub (weeks 14–16)

- [x] Auth: reuse the `gh` login; fall back to a PAT in the Keychain (the PAT reaches gh and sessions as `GH_TOKEN`; docs/m7-findings.md)
- [x] Import an issue as a task (`#12`, `owner/repo#12` or URL; `github-issue.md` in the docs folder)
- [x] Open a PR from the task's worktree (`gh pr create`) and link it to the task (pushes the branch first; refuses with uncommitted files)
- [x] Poll open PRs every 30–60 seconds: checks, reviews, mergeable state (every 45 s; a merged PR moves the task to Done)
- [x] Show PR state on the board card; merge button calls `gh pr merge` (squash, merge or rebase; the branch is kept; M7 flow confirmed by hand)

### M8: Packaging (weeks 17–18)

- [ ] Developer ID signing and notarization — deferred: no paid Apple Developer account yet. Releases are ad-hoc signed; README explains "Open Anyway". Adding it later is config and CI secrets only.
- [x] Tauri updater with a static update feed (for example GitHub Releases) (`latest.json` on GitHub Releases, made by `scripts/release.sh`; docs/m8-findings.md)
- [x] Log files and a "copy diagnostics" action (`~/Library/Logs/dev.teguh.humanlayerlocal/`; report scrubs keys, tokens, emails and the home path)
- [x] First-run checks: `claude`, `codex`, `gh` and `git` found on PATH (banner at start; Settings → Tools; release build confirmed by hand)

## Risks

1. **CLI output formats change.** Pin tested CLI versions; show a warning on unknown events instead of crashing.
2. **Approval UX.** If the M0 spike shows per-call approvals are hard to wire, ship with permission modes only and add approvals later.
3. **Polling limits.** Linear and GitHub rate-limit requests. Poll only active tasks and back off when the app is idle.
4. **Scope creep from upstream.** The upstream app has many more features. Build only what this plan lists before M8.

If short on time, cut in this order: Codex adapter (M5), GitHub issue import (keep PRs), auto-update.
