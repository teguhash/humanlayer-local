# M4 findings: RPI skills in app sessions

Date: 2026-11-05. Claude Code 2.1.314.

## Loading the skills

- `claude --plugin-dir <folder>` loads a plugin for that process only. Nothing is installed into `~/.claude`, and the user's own plugins still load.
- The app bundles upstream `apps/riptide-rpi-claude-plugin` as `src-tauri/resources/rpi` (plugin name `rpi`). The upstream daemon passed its research agents in code; here they are converted to plugin agent files in `agents/`.
- The `system/init` event then lists 22 `rpi:*` skills (also listed as slash commands) and 7 `rpi:*` agents.
- A stream-json user message `/rpi:create-research <text>` runs the skill with `<text>` as its argument. With no argument, the skill gives its fixed "I'm ready to research the codebase…" reply.
- Upstream skills had three symlinks (`references/show-me.md`) that point outside the plugin folder. They are replaced with copies, because Tauri resource bundling fails on broken links.

## Task docs folder

- The skills expect the task's docs in `.humanlayer/tasks/<slug>` in the session's folder, plus a system-prompt note naming that folder (upstream `execute-codelayer.ts`).
- The app keeps the real folder in app data (`artifacts/<task id>/`). Before each session process starts, it links that folder into the session folder (repo or worktree) and passes `--append-system-prompt` with the note. The docs therefore stay put when worktrees are created or removed.
- `/.humanlayer/tasks/` is added to the repo's `.git/info/exclude`, which is local and never committed, so the link never shows up in `git status` or in the Changes tab.
- `--add-dir <real docs folder>` and `--add-dir <plugin folder>` stop the CLI from asking before it reads skill templates. Writes still ask under the `default` permission mode.

## Real run

`/rpi:create-research` on a two-file scratch repo:
- It read the skill's reference templates.
- It wrote `01-research-greeting-flow.md` through the link into the docs folder.
- It cost $0.34.
