# HumanLayer Local

> **Early stage.** This is a personal project that works end to end on the author's Mac, but it is young: expect rough edges, schema changes between versions, and features that only cover the common path. See [What works today](#what-works-today) and [Known gaps](#known-gaps) before relying on it.

HumanLayer Local is a macOS desktop app for running AI coding agents (Claude Code and Codex) through a **research → plan → implement → review** flow. Each task sits on a board, gets its own git worktree, and keeps its research and plan docs next to it. It can link a task to a Linear ticket or a GitHub issue and open the pull request when the work is done.

It ports ideas from [HumanLayer](https://github.com/humanlayer/humanlayer) (Apache 2.0) into a single app that needs no HumanLayer account and no backend.

## Local means local

There is no server and no HumanLayer account. Everything the app does happens on your machine:

- **Agents run as your own CLIs.** The app starts `claude` and `codex` as child processes in your repo or worktree, with your existing sign-ins and your login shell's environment. Prompts and code go only where those CLIs already send them.
- **Your data stays on disk.** Projects, tasks, sessions and every agent event are stored in a SQLite file in `~/Library/Application Support/`. Research docs and plans are plain Markdown files in a folder per task.
- **Secrets go in the macOS Keychain.** That covers the Linear API key and the optional GitHub token. They are never written to the database, logs or diagnostics.
- **Integrations are direct.** Linear is called with your personal API key, and GitHub through your `gh` login. Nothing passes through a third-party service.
- **Network use you opt into:**
  - the agent CLIs talking to their model providers
  - Linear and GitHub, if you connect them
  - the update check against this repo's GitHub Releases, which you start from Settings

## What it does

1. **Board.** Add a git repo as a project. Its tasks sit in columns: Backlog, Research, Plan, Implement, Review, Done.
2. **Stage skills.** Dragging a task into a stage starts a session with that stage's skill: `create-research`, `create-plan`, `implement-plan` or `describe-pr`. The skills are bundled from HumanLayer's RPI set.
3. **Worktrees.** Implement creates a git worktree and branch for the task, configurable through `.humanlayer/workspace.json`, and runs your setup command there. The Changes tab shows the diff against the base branch.
4. **Docs.** Agents write research, plans and outlines into the task's docs folder. The app lists them, renders the Markdown, and has an **Iterate** button that starts a session to revise a doc from your feedback.
5. **Sessions.**
   - You see a live transcript with tool calls.
   - You approve or deny Claude's tool use: once, for the session, or not at all.
   - You can interrupt, resume, and see token usage.
   - Each task picks Claude or Codex; each project has a default.
6. **Linear.**
   - Import a ticket, with its images, as a task.
   - Linked tickets are re-checked every 60 seconds.
   - When a task changes stage, its ticket moves forward, never back.
   - A button posts a doc to the ticket as a comment.
7. **GitHub.**
   - Import an issue as a task.
   - Push the task's branch and open a PR.
   - See checks, reviews and conflicts on the board card.
   - Merge from the app. A merged PR moves its task to Done.

## What works today

Each milestone in [PLAN.md](PLAN.md) was built, tested, and confirmed by hand in the real app. Notes on how each was checked are in [`docs/`](docs).

| Area | Status |
|---|---|
| Projects, tasks, board with drag and drop | Working |
| Claude Code sessions: streaming, approvals, interrupt, resume, usage | Working |
| Codex sessions (`codex exec`, resume) | Working, sandbox only (see below) |
| Per-task worktrees, setup command, diff view, archive/restore | Working |
| Bundled RPI skills, task docs folder, Markdown viewer, Iterate | Working |
| Linear: Keychain key, import, 60 s sync, stage → status, post doc | Working |
| GitHub: `gh` login or token, issue import, open PR, PR status, merge | Working |
| First-run tool check, log files, Copy diagnostics | Working |
| Release build (`.dmg`, signed update archive) | Builds; updater not yet tested across two real releases |

## Known gaps

- **Not notarized.** There is no paid Apple Developer ID yet, so downloaded builds are ad-hoc signed and macOS blocks the first launch (workaround below). For the same reason, macOS may ask again for Keychain access after an update.
- **macOS only.** Only tested on Apple Silicon. Intel should build but is untested. There are no Windows or Linux builds.
- **Codex has no per-call approvals.** `codex exec` doesn't support them, so Codex runs in the `workspace-write` sandbox: it can edit the session folder and the task's docs, and it has no network.
- **Linear:**
  - It needs a personal API key; there is no OAuth.
  - The stage → status mapping is fixed. It tries HumanLayer's RPI column names first, then Linear's defaults.
  - Docs reach the ticket only as comments you post with a button, because the docs live only on your machine.
- **GitHub:**
  - It needs the `gh` CLI, and it works with github.com only (no GitHub Enterprise).
  - Only PRs opened or linked through the app are tracked.
  - Branches are pushed to `origin`.
  - Merging keeps the branch.
- **One machine, one user.** Nothing syncs between Macs, and there is no team sharing.
- **Agents can see your keys.** Sessions get `LINEAR_API_KEY` and, if you saved one, `GH_TOKEN`, so the `linear` and `gh` CLIs work inside them.
- **Tested in parts.** Real Linear and GitHub write actions (status changes, comments, push, PR create and merge) were tested by hand, not by automated tests.

## Requirements

- macOS
- Required on your login shell's PATH:
  - `git`
  - [`claude`](https://docs.anthropic.com/en/docs/claude-code) (Claude Code, signed in)
- Optional on PATH:
  - `codex`, for Codex sessions
  - [`gh`](https://cli.github.com), signed in with `gh auth login`, for GitHub

The app checks these at start, and **Settings → Tools** shows what it found.

## Install a release

Releases are **not notarized**, so macOS blocks the first launch of a downloaded copy. To allow it once:

1. Download the `.dmg` from [Releases](https://github.com/teguhash/humanlayer-local/releases) and drag **HumanLayer Local** to Applications.
2. Open it. macOS says it can't verify the developer; click **Done**.
3. Open **System Settings → Privacy & Security**, scroll to Security, and click **Open Anyway** next to HumanLayer Local.

Or remove the download quarantine flag from a terminal instead:

```bash
xattr -dr com.apple.quarantine "/Applications/HumanLayer Local.app"
```

After that, **Settings → Updates** installs new versions from GitHub Releases. Each update is checked against the release signing key before it is installed.

## Build from source

Prerequisites: Node 20 or newer, and Rust from [rustup](https://rustup.rs).

```bash
npm install
```

```bash
npm run tauri dev
```

To build a local `.app` and `.dmg` (ad-hoc signed, which is enough to run on your own Mac):

```bash
npm run tauri build
```

The Rust tests:

```bash
cd src-tauri && cargo test --workspace
```

## Where things live

| What | Where |
|---|---|
| Database | `~/Library/Application Support/dev.teguh.humanlayerlocal/humanlayer-local.db` |
| Task docs (research, plans) | `~/Library/Application Support/dev.teguh.humanlayerlocal/artifacts/<task id>/`, linked into session folders as `.humanlayer/tasks/<slug>` (git-ignored through `.git/info/exclude`) |
| Worktrees | `~/.humanlayer/workspaces/<task>/<repo>` (configurable in `.humanlayer/workspace.json`) |
| Logs | `~/Library/Logs/dev.teguh.humanlayerlocal/` |
| Linear key, GitHub token | macOS Keychain, service `dev.teguh.humanlayerlocal` |

**Settings → Diagnostics → Copy diagnostics** makes a report for bug reports. Keys, tokens, emails and your home folder path are removed from it.

## Project layout

- `src/`: React and TypeScript UI.
- `src-tauri/src/`: Tauri commands, plus the RPI flow, Linear, GitHub, secrets and diagnostics.
- `src-tauri/crates/`:
  - `core`: SQLite, models and the docs store.
  - `agents`: the Claude and Codex adapters and the session runtime.
  - `git`: git and `gh` helpers.
  - `linear`: the GraphQL client.
- `src-tauri/resources/rpi/`: the bundled skills and agents.
- `PLAN.md` holds the milestones. `docs/m*-findings.md` records what each milestone found and how it was checked.

## Making a release (maintainers)

1. Bump `version` in `src-tauri/tauri.conf.json` and `package.json`.
2. Run the release script. It needs the updater signing key at `~/.tauri/humanlayer-local.key` (or set `TAURI_SIGNING_PRIVATE_KEY_FILE`). Keep that key out of the repo, and keep a backup.

   ```bash
   scripts/release.sh teguhash/humanlayer-local
   ```

3. Create a GitHub Release tagged `v<version>`, and attach the files from `src-tauri/target/release/github-release/`.

## Credits and license

Parts of this project are derived from [HumanLayer](https://github.com/humanlayer/humanlayer) under the Apache License 2.0: the RPI skills and agents, UI components, and several ported ideas. See [NOTICE](NOTICE) and [licenses/humanlayer-LICENSE](licenses/humanlayer-LICENSE). This project is not affiliated with HumanLayer.

HumanLayer Local is licensed under the [Apache License 2.0](LICENSE).
