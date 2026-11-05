# M0 spike findings

Date: 2026-11-05. Machine: macOS (Darwin 27.0.0), Apple Silicon.

## Versions tested

| Tool | `--version` output |
|---|---|
| Claude Code | `2.1.314 (Claude Code)` |
| Codex CLI | `codex-cli 0.166.0` |
| Tauri (crate) | 2.12.5 |

Both CLIs were found at `~/.local/bin`, which is on PATH only via the login shell.

## Exact flags used

Flags were checked against `claude --help`, `codex --help` and `codex exec --help` for the versions above.

### Claude

```
claude -p \
  --input-format stream-json \
  --output-format stream-json \
  --verbose \
  --permission-mode default \
  --permission-prompt-tool stdio
```

- The prompt goes in on stdin as a stream-json user message, after an `initialize` control request:
  ```json
  {"type":"control_request","request_id":"init","request":{"subtype":"initialize"}}
  {"type":"user","message":{"role":"user","content":"<prompt>"}}
  ```
- With `--input-format stream-json`, the CLI keeps running while stdin is open. The spike closes stdin after the first `result` event, so one prompt makes one turn and the process exits.
- `--verbose` is required with `-p --output-format stream-json`.
- `--permission-prompt-tool` is **not listed** in `claude --help` as an option of its own. It appears only inside the help text of `--permission-prompts` ("the SDK host or --permission-prompt-tool"). The value `stdio` is what the Agent SDK passes. It works in 2.1.314, but treat it as an undocumented contract and pin the version.
- `--permission-mode` choices in this version: `acceptEdits`, `auto`, `bypassPermissions`, `manual`, `dontAsk`, `plan`, and `default` is accepted as well. (`manual` is new compared with what PLAN.md assumed.)

### Codex

```
codex exec --json --skip-git-repo-check --sandbox read-only --cd <folder> -
```

- `-` reads the prompt from stdin. The app writes the prompt and then closes stdin. If stdin stays open, `codex exec` waits for it: with a prompt argument and stdin left open, it prints `Reading additional input from stdin...` and hangs until EOF. **Always close or null stdin.**
- `--sandbox read-only` was chosen for spike safety. Other values: `workspace-write`, `danger-full-access`.
- `codex exec` has no `--ask-for-approval` flag. That flag exists only on the top-level interactive `codex`. `exec` takes `--approve-for-me` (automatic review) or `--dangerously-bypass-approvals-and-sandbox`, so there is **no per-call human approval in `codex exec --json`**.

### Environment

Both CLIs are spawned with `env_clear()` plus the environment from the user's login shell. The shell env is read by running `$SHELL -ilc "printf MARKER; /usr/bin/env -0"`, with a 5 s timeout and a fallback to `-lc` (see `src-tauri/src/shell_env.rs`). This differs from upstream in three ways:
- `-i` is added, so PATH edits in `.zshrc` count (most users put them there).
- `env -0` with a marker is used, so multi-line values and shell banners do not break parsing.
- A timeout is added, so a hanging rc file cannot block the app.

Each CLI runs in its own process group (`process_group(0)`). Stop sends SIGTERM to the group, then SIGKILL after 3 s.

## Event types observed

The app labels each event `type`, plus `subtype` (Claude), `request.subtype` (control requests) or `item.type` (Codex). Samples below are trimmed (`…`), and ids and paths are shortened.

### Claude (2.1.314)

| Event | When |
|---|---|
| `system/hook_started`, `system/hook_response` | User's SessionStart hooks ran (only when the user has hooks) |
| `control_response` | Reply to our `initialize` request |
| `system/init` | Session start: tools, MCP servers, model, permissionMode |
| `system/thinking_tokens` | Thinking progress estimate |
| `assistant` | Assistant message (text and/or tool_use) |
| `rate_limit_event` | Plan usage windows |
| `control_request/can_use_tool` | **Approval request** (only with `--permission-prompt-tool stdio`) |
| `system/permission_denied` | Tool auto-denied (seen without the stdio flag) |
| `user` | Tool result fed back to the model |
| `system/task_summary` | Short status line ("Writing hello.txt") |
| `system/post_turn_summary` | One-line summary after the turn |
| `result/success` | End of turn: cost, usage, session_id, num_turns |

Samples:

```json
{"type":"system","subtype":"init","cwd":"/…/play","session_id":"4a773067-…","tools":["Task","…"],"mcp_servers":[{"name":"…","status":"pending","source":"claudeai"}],"model":"claude-haiku-5-5","permissionMode":"default","apiKeySource":"none","claude_code_version":"2.1.314","output_style":"default"}
{"type":"assistant","message":{"model":"claude-haiku-5-5","id":"msg_…","role":"assistant","content":[{"type":"text","text":"pong"}],"stop_reason":null,"usage":{"input_tokens":2,"cache_read_input_tokens":21771,"output_tokens":4}},"parent_tool_use_id":null,"session_id":"4a773067-…"}
{"type":"control_request","request_id":"d216c2d4-…","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"/…/play/hello.txt","content":"hi"},"description":"hello.txt","permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"toolu_01KH…"}}
{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_01KH…","type":"tool_result","content":"File created successfully at: /…/hello.txt"}]},"tool_use_result":{"type":"create","filePath":"/…/hello.txt"},"tool_result_meta":[{"id":"toolu_01KH…","permission_decision":{"decision":"accept","source":"user_temporary"}}]}
{"type":"system","subtype":"task_summary","detail":"Writing hello.txt","session_id":"f24d8def-…"}
{"type":"system","subtype":"post_turn_summary","status_category":"completed","status_detail":"replied with exactly 'pong' as requested","needs_action":""}
{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","rateLimitType":"five_hour","unifiedWindows":{"five_hour":{"utilization":0.05,"status":"allowed"}}}}
{"type":"system","subtype":"thinking_tokens","estimated_tokens":50,"estimated_tokens_delta":50}
{"type":"system","subtype":"hook_started","hook_name":"SessionStart:startup","hook_event":"SessionStart"}
{"type":"result","subtype":"success","is_error":false,"num_turns":1,"duration_api_ms":2711,"stop_reason":"end_turn","session_id":"4a773067-…","total_cost_usd":0.00304,"usage":{"input_tokens":2,"output_tokens":4},"permission_denials":[],"terminal_reason":"completed"}
```

**Privacy note:** the `initialize` `control_response` includes the signed-in account (email, organization, subscription type). `hook_response` includes the full output of the user's hooks. The M2 events table stores raw payloads, so redact or drop these fields before writing them to logs or a "copy diagnostics" bundle.

### Codex (0.166.0)

| Event | When |
|---|---|
| `thread.started` | Start; carries `thread_id` (use it to resume) |
| `turn.started` | Turn begins |
| `item.started/command_execution` | Shell command starting |
| `item.completed/command_execution` | Shell command done, with output and exit code |
| `item.completed/agent_message` | Assistant text |
| `turn.completed` | End of turn, with token usage |

Samples:

```json
{"type":"thread.started","thread_id":"01a1a966-93f1-7380-baf5-e0144baca8ad"}
{"type":"turn.started"}
{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc 'ls -la'","aggregated_output":"","exit_code":null,"status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc 'ls -la'","aggregated_output":"total 0\n…","exit_code":0,"status":"completed"}}
{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"pong"}}
{"type":"turn.completed","usage":{"input_tokens":20128,"cached_input_tokens":13440,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0}}
```

Item types such as `reasoning`, `file_change` and `mcp_tool_call` were not seen in these short runs. M5 needs a longer workspace-write run to catalogue them.

## Approvals: recommendation

**Use neither the MCP permission tool nor hooks. Use the stdio control protocol: `--permission-prompt-tool stdio` with `--input-format stream-json`.** This is the same mechanism the Agent SDK's `canUseTool` uses, and upstream relies on it through the SDK (`riptide-daemon/src/effects/services/claude-sessions/prepare-claude-session.ts`).

How it works (proven in the spike):
1. Claude writes `{"type":"control_request","request_id":…,"request":{"subtype":"can_use_tool","tool_name":…,"input":…,"permission_suggestions":[…],"tool_use_id":…}}` to stdout and waits.
2. The app replies on stdin:
   - Allow: `{"type":"control_response","response":{"subtype":"success","request_id":…,"response":{"behavior":"allow","updatedInput":<input>}}}`
   - Deny: same, with `{"behavior":"deny","message":"…"}`
3. Verified both ways. Allow: the file was written, and `tool_result_meta` shows `"source":"user_temporary"`. Deny: no file, and the model saw the deny message and replied.
4. Without `--permission-prompt-tool stdio`, no request is sent. The call is denied at once, and the CLI emits `system/permission_denied`.

Comparison:

| | stdio control protocol | `--permission-prompt-tool <mcp tool>` | Hooks (`PreToolUse`) |
|---|---|---|---|
| Extra moving parts | None: same pipes we already read | App must serve an MCP server (stdio or HTTP) and pass `--mcp-config` | A hook command (separate process per call) that must reach back into the app over a socket or HTTP, plus `--settings` injection |
| Gets the call in context | Yes: request id, `tool_use_id`, suggestions | Yes, as tool arguments | Yes, as hook stdin JSON |
| "Allow for session" | Return `updatedPermissions` built from `permission_suggestions` (not tested yet) | Same idea; less clear | Hook has to keep its own rule store |
| Blocking/timeout risk | No timeout seen; the CLI waited for our answer | MCP tool-call timeouts apply | Hook timeouts apply, so a slow human answer can fail the call |
| Interacts with user's own hooks/MCP | No | Adds a server to the user's MCP list for the session | Merges with the user's hooks; ordering is unclear |
| Risk | Flag is undocumented in `--help` | Documented, but more code | Documented, but the most plumbing |

Spike proof:
- `src-tauri/src/agent.rs`: `respond_approval` command and `approval_response()`.
- UI Allow/Deny buttons on `control_request/can_use_tool` rows.
- Test `e2e_claude_with_approval` (real CLI, auto-allows and checks the written file).

The fallback, if the flag ever disappears, is the MCP permission tool. Hooks are the least attractive option.

**Codex:** `codex exec --json` has no human approval path. Options for M5:
- (a) Run `exec` with a sandbox and no approvals: workspace-write, using the sandbox as the guard.
- (b) Move to `codex app-server` (experimental JSON-RPC over stdio, has a `generate-json-schema` subcommand), which is what interactive clients use for approvals.

## Verification done

| Check | How | Result |
|---|---|---|
| `npm run tauri dev` builds and opens | Ran it; Vite served on :1420, `target/debug/humanlayer-local` process running | Pass: window opens; spike screen checked by hand by the user |
| Claude round-trip through the Rust command | `cargo test -- --ignored e2e_claude_with_approval` (Tauri mock runtime, real CLI) | Pass: events streamed, approval answered, file written, exit 0 |
| Codex round-trip through the Rust command | `cargo test -- --ignored e2e_codex` | Pass: `agent_message` "pong", exit 0 |
| Non-JSON line, stderr, non-zero exit become events | `invalid_json_stderr_and_nonzero_exit_become_events` | Pass |
| Stop kills the CLI and its children | `stop_kills_a_running_process` (`sh` with a `sleep` grandchild) | Pass. Without the process group it hung: fixed |
| Bad folder / empty prompt return errors | `bad_folder_and_empty_prompt_are_errors` | Pass |
| Non-blocking | `start_session` is an async command and returns once spawned (1.8 s on the first call, which includes loading the shell env in `spawn_blocking`). Events stream while the turn runs (about 10 s) | Pass in tests. UI has a ticking clock for a manual check |
| `cargo clippy --all-targets -- -D warnings` | src-tauri | Clean |
| `npm run build` | `tsc && vite build` | Pass |

The user clicked through the spike screen by hand and it worked. Not verified: launching the bundled `.app` from Finder (the main reason for the shell-env code).

## Changes suggested for PLAN.md

1. **M0 flag line:** add `--permission-mode default --permission-prompt-tool stdio`, and note that the flag is hidden from `--help`.
2. **Approvals decision:** stdio control protocol (see above). Risk 2 can be downgraded for Claude. It still stands for Codex.
3. **M5 Codex:** `codex exec --json` cannot do approvals. Either accept sandbox-only for Codex or plan for `codex app-server`. Budget extra time if app-server is chosen; it is marked experimental.
4. **M2 events table:** redact account info (`initialize` response) and hook output before storing or exporting.
5. **M2 `Agent` trait:** Claude stays alive while stdin is open. "send" for follow-up turns can write another user message to the same process instead of respawning with `--resume`, and interrupt can use a control request. Worth testing early in M2 (subtype `interrupt`, untested here).
6. **Runtime:** spawn every agent in its own process group, and always close or null stdin for Codex.
7. **Data model:** `sessions.cli_session_id` holds Claude's `session_id` (in `system/init` and `result`) or Codex's `thread_id` (in `thread.started`).
