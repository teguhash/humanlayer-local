# HumanLayer Local

A local-only macOS desktop app for running Claude Code and Codex sessions on a task board. It follows the research → plan → implement → review flow, with per-task git worktrees, Linear tickets and GitHub pull requests. Everything runs on your machine; there is no server.

It ports ideas from [HumanLayer](https://github.com/humanlayer/humanlayer) (Apache 2.0). See `NOTICE` and `docs/` for what was ported and how each milestone was checked.

## Requirements

- macOS on Apple Silicon or Intel
- Required on your login shell's PATH:
  - `git`
  - `claude` (Claude Code, signed in)
- Optional on PATH:
  - `codex`, for Codex sessions
  - `gh`, signed in with `gh auth login`, for GitHub issues and PRs

The app checks for these at start, and **Settings → Tools** shows what it found.

## Install a release

Releases are **not notarized by Apple** (no paid Developer ID yet), so macOS blocks the first launch of a downloaded copy. To allow it once:

1. Download the `.dmg` from GitHub Releases and drag **HumanLayer Local** to Applications.
2. Open it. macOS says it can't verify the developer; click **Done**.
3. Open **System Settings → Privacy & Security**, scroll to Security, and click **Open Anyway** next to HumanLayer Local. Confirm with your password.

Or remove the download quarantine flag from a terminal instead:

```bash
xattr -dr com.apple.quarantine "/Applications/HumanLayer Local.app"
```

After that, the app updates itself from GitHub Releases (**Settings → Updates**). Each update is checked against the release signing key before it is installed.

## Build from source

Prerequisites: Node 20 or newer, and Rust from rustup.

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

## Where things live

| What | Where |
|---|---|
| Database | `~/Library/Application Support/dev.teguh.humanlayerlocal/humanlayer-local.db` |
| Task docs (research, plans) | `~/Library/Application Support/dev.teguh.humanlayerlocal/artifacts/<task id>/`, linked into session folders as `.humanlayer/tasks/<slug>` |
| Worktrees | `~/.humanlayer/workspaces/<task>/<repo>` (configurable in `.humanlayer/workspace.json`) |
| Logs | `~/Library/Logs/dev.teguh.humanlayerlocal/` |
| Linear key, GitHub token | macOS Keychain, service `dev.teguh.humanlayerlocal` |

**Settings → Diagnostics → Copy diagnostics** makes a report for bug reports. Keys, tokens, emails and your home folder path are removed from it.

## Making a release (maintainers)

1. Bump `version` in `src-tauri/tauri.conf.json` and `package.json`.
2. Run the release script. It needs the updater signing key at `~/.tauri/humanlayer-local.key` (or set `TAURI_SIGNING_PRIVATE_KEY_FILE`). Keep that key out of the repo, and keep a backup: without it, installed copies can't verify new updates.

   ```bash
   scripts/release.sh teguhash/humanlayer-local
   ```

3. Create a GitHub Release tagged `v<version>`, and attach the `.dmg`, the `.app.tar.gz` and its `.sig`, and `latest.json` from `src-tauri/target/release/github-release/`.

The update feed URL is set in `src-tauri/tauri.conf.json` (`plugins.updater.endpoints`). Change it if the repo lives somewhere else.
