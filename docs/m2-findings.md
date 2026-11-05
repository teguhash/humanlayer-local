# M2 findings: Claude session protocol

Date: 2026-11-05. Versions: Claude Code 2.1.314 and codex-cli 0.166.0, the same as M0.

These were probed by driving `claude -p --input-format stream-json --output-format stream-json --verbose --permission-mode default --permission-prompt-tool stdio` from a script. The fake CLI in `src-tauri/crates/agents/tests/fixtures/fake-claude` mirrors them.

## Multi-turn on one process

- While stdin is open, each `{"type":"user",…}` line starts a new turn, and each turn ends with one `result` event. The process stays alive between turns and remembers the conversation.
- A `system/init` event is emitted at the start of **every** turn, not just once per process.
- Closing stdin makes the CLI exit after the current turn.

## Usage and cost

| Field | Scope |
|---|---|
| `result.usage` | **This turn only** |
| `result.modelUsage[*]` | Cumulative for the process |
| `result.total_cost_usd` | **Cumulative for the process** |

- Example: turn 1 cost 0.00304 and turn 2 reported 0.00638. Cache reads were 23321 in turn 1 and 26584 in turn 2, and modelUsage showed 49905, their sum.
- A resumed process (`--resume`) starts counting from 0 again.
- An interrupted turn reports `total_cost_usd: 0` and an empty `modelUsage`. Its tokens are not reported anywhere.

So session cost = the sum, over processes, of each process's highest `total_cost_usd`. Session tokens = the sum of the per-turn `usage` values. See `transcript::usage`.

## Interrupt

- Request: `{"type":"control_request","request_id":"<id>","request":{"subtype":"interrupt"}}`.
- The reply is `control_response` with `{"still_queued":[]}`.
- Claude then emits a `user` message with text `[Request interrupted by user]`, then `result` with subtype `error_during_execution`.
- **The process stays alive**, and the next user message runs normally.

## Resume

- `--resume <session_id>` works with stream-json. The resumed session keeps **the same `session_id`**, unless you pass `--fork-session`.
- The model remembered earlier turns, both in the probe and in the e2e test after stopping the process.

## Allow for this session

- `can_use_tool` requests carry `permission_suggestions`. For Write it was `[{"type":"setMode","mode":"acceptEdits","destination":"session"}]`.
- Answering `{"behavior":"allow","updatedInput":…,"updatedPermissions":<suggestions>}` applied them. Later edits came back with `permission_decision.source: "config"` and `reason_type: "mode"`, with no new prompt.
- **Safety:** suggestions can name `localSettings` or `userSettings` as the destination, which would write to the user's settings files. The app rewrites every destination to `session`. When there are no suggestions, it sends `addRules` for the tool name, also scoped to the session.
- Gotcha for tests: Claude auto-allows writes inside its own scratchpad and read-only Bash commands such as `echo`, so those never prompt. Use explicit paths in the working folder.

## Privacy

The `initialize` reply includes `account` (email, organization, plan). `Claude::sanitize` drops it before anything is stored, and a test checks that it is gone.

## Codex follow-ups

- `codex exec resume [OPTIONS] <SESSION_ID> -` accepts `--json` and `--skip-git-repo-check`, but not `--sandbox` or `--cd`.
- The app sets the sandbox with `-c sandbox_mode="read-only"` and the working folder with the process cwd.
- Verified: a second turn remembered a word from the first.
