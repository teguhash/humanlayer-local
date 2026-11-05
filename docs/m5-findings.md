# M5 findings: Codex adapter

Date: 2026-11-05. codex-cli 0.166.0. Flags were checked against `codex --help`, `codex exec --help` and `codex exec resume --help`.

## Approvals: sandbox only

- `codex exec` has no human approval path (see m0-findings). The app chose option (a) from M0: run `--sandbox workspace-write` with no approvals, and let the sandbox be the guard.
- `workspace-write` allows edits in the session folder (repo or worktree) and in any added writable roots. It also allows temp folders. Network is off.
- `codex app-server` (option b) is still marked experimental. It stays the path if per-call approvals are needed later.
- `exec resume` takes no `--sandbox`, `--cd` or `--add-dir`. The app passes the same settings as `-c` overrides on every turn:
  - `-c sandbox_mode="workspace-write"`
  - `-c sandbox_workspace_write.writable_roots=[...]`
  - `-c developer_instructions="..."`
- Options go before the positionals (`<thread_id>` and `-` for stdin). `Agent::args` now takes the prepare hook's options, so each CLI decides where they go.

## Skills

- Codex reads project skills from `.agents/skills/` in the working folder and follows symlinks, including a link to a folder of skill folders.
- Before each Codex process starts, the app links `<session folder>/.agents/skills/humanlayer-rpi` to the bundled `resources/rpi/skills`. Nothing is written to `~/.codex`.
- `/.agents/skills/humanlayer-rpi` goes into `.git/info/exclude`. Only the link is excluded, so a repo's own committed `.agents/skills` are untouched.
- A skill is invoked with a `$name` mention. A stage session therefore sends `$create-research <title>`; Claude gets `/rpi:create-research <title>`.
- Verified with a probe skill: `$hl-probe` ran its instructions with no shell commands. This worked as a direct link and as a nested folder link.
- The bundled skills are identical to the upstream copies some users already have in `~/.codex/skills`, so a name clash loads the same text.
- The skills mention Claude sub-agents. Codex has none of the bundled agents, so it does that research itself. Upstream ships the same skill text to Codex.

## Task docs folder and prompt

- The docs folder link (`.humanlayer/tasks/<slug>`) works as it does for Claude.
- Its target lives in app data, which is outside the sandbox. It is passed as a writable root. Probe in `~/Library/Application Support/…`: without the root nothing was written; with it, the write succeeded.
- `developer_instructions` carries the same task and docs-folder note as Claude's `--append-system-prompt`. Claude-only tool names are left out. Verified: the model repeated a word given only there.
- Values are passed as TOML strings. JSON string escaping is valid TOML basic-string escaping.

## Events

| Codex item | Transcript |
|---|---|
| `agent_message` | Assistant text |
| `reasoning` | Thinking |
| `command_execution` | `shell` tool call and its output; non-zero exit is an error |
| `file_change` | `apply_patch` tool call; the result lists `update a.txt`, `add b.txt` |
| `mcp_tool_call` | `mcp__<server>__<tool>` tool call and its result or error |
| `web_search` | `web_search` tool call |
| `todo_list` | "Plan:" checklist notice |
| `error` item | Warning notice |
| `turn.failed`, `error` | Error notice, turn ends failed |

- Seen in real runs: `agent_message`, `command_execution` and `file_change`. A run that was asked for a todo plan still sent no `todo_list`.
- The remaining shapes follow the CLI's event types and are covered by unit tests only.
- Any other item type still shows as an info notice with its JSON.

## Agent choice

- Migration 0004 adds `projects.default_agent` (default `claude`) and `tasks.agent` (NULL = use the project default).
- Stage moves, the stage button and Iterate all use the task's agent. The extra "Claude session" and "Codex session" buttons still pick an agent explicitly, and Codex sessions get the skills and docs link too.
