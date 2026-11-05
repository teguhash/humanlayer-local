//! Codex CLI (`codex exec --json`). One process per turn; follow-ups use
//! `codex exec resume <thread_id>`. `exec` has no per-call approvals, so the
//! sandbox is the guard: `workspace-write` (edits inside the session folder
//! and any writable roots the app adds; no network). Verified against 0.166.0.

use serde_json::{Value, json};

use crate::Agent;
use crate::transcript::{Decision, Item, Level, TokenUsage, u};

pub struct Codex;

pub const SANDBOX: &str = "workspace-write";

/// A `-c key=value` override with a TOML string value.
pub fn config_string(key: &str, value: &str) -> [String; 2] {
    // JSON string escapes are valid TOML basic-string escapes.
    ["-c".into(), format!("{key}={}", Value::from(value))]
}

/// A `-c key=[...]` override with a TOML array of strings.
pub fn config_strings(key: &str, values: &[String]) -> [String; 2] {
    ["-c".into(), format!("{key}={}", json!(values))]
}

impl Agent for Codex {
    fn binary(&self) -> &'static str {
        "codex"
    }

    fn args(&self, cwd: &str, resume: Option<&str>, extra: Vec<String>) -> Vec<String> {
        let mut v: Vec<String> = vec!["exec".into()];
        if resume.is_some() {
            v.push("resume".into());
        }
        v.extend(["--json", "--skip-git-repo-check"].map(String::from));
        match resume {
            // `exec resume` takes no --sandbox/--cd; set the sandbox via
            // config, and the working dir is the process cwd.
            Some(_) => v.extend(config_string("sandbox_mode", SANDBOX)),
            None => v.extend(["--sandbox", SANDBOX, "--cd", cwd].map(String::from)),
        }
        v.extend(extra);
        if let Some(id) = resume {
            v.push(id.into());
        }
        // Prompt from stdin, so it can start with "-" and has no length limit.
        v.push("-".into());
        v
    }

    fn persistent(&self) -> bool {
        false
    }

    fn user_turn(&self, text: &str) -> String {
        text.to_string()
    }

    fn approval(&self, _: &str, _: Decision, _: &Value) -> Option<String> {
        None
    }

    fn cli_session_id(&self, raw: &Value) -> Option<String> {
        (raw["type"] == "thread.started")
            .then(|| raw["thread_id"].as_str().map(String::from))
            .flatten()
    }

    fn is_turn_end(&self, raw: &Value) -> bool {
        matches!(raw["type"].as_str(), Some("turn.completed" | "turn.failed"))
    }

    fn parse(&self, raw: &Value) -> Vec<Item> {
        let t = raw["type"].as_str().unwrap_or_default();
        let item = &raw["item"];
        let id = item["id"].as_str().unwrap_or_default().to_string();
        match (t, item["type"].as_str().unwrap_or_default()) {
            ("thread.started", _) => vec![Item::SessionInit {
                cli_session_id: raw["thread_id"].as_str().unwrap_or_default().into(),
                model: None,
            }],
            ("item.completed", "agent_message") => vec![Item::AssistantText {
                text: item["text"].as_str().unwrap_or_default().into(),
            }],
            ("item.completed", "reasoning") => vec![Item::Thinking {
                text: item["text"].as_str().unwrap_or_default().into(),
            }],
            ("item.started", "command_execution") => vec![Item::ToolUse {
                id,
                name: "shell".into(),
                input: serde_json::json!({ "command": item["command"] }),
            }],
            ("item.completed", "command_execution") => vec![Item::ToolResult {
                tool_use_id: id,
                content: item["aggregated_output"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
                is_error: item["exit_code"].as_i64().is_some_and(|c| c != 0),
            }],
            ("item.started", "file_change") => vec![Item::ToolUse {
                id,
                name: "apply_patch".into(),
                input: json!({ "changes": item["changes"] }),
            }],
            ("item.completed", "file_change") => vec![Item::ToolResult {
                tool_use_id: id,
                content: change_summary(item),
                is_error: item["status"] == "failed",
            }],
            ("item.started", "mcp_tool_call") => vec![Item::ToolUse {
                id,
                name: format!(
                    "mcp__{}__{}",
                    item["server"].as_str().unwrap_or("?"),
                    item["tool"].as_str().unwrap_or("?")
                ),
                input: item["arguments"].clone(),
            }],
            ("item.completed", "mcp_tool_call") => {
                let failed = item["status"] == "failed" || !item["error"].is_null();
                let content = if failed {
                    item["error"]["message"]
                        .as_str()
                        .map(String::from)
                        .unwrap_or_else(|| item["error"].to_string())
                } else {
                    item["result"].to_string()
                };
                vec![Item::ToolResult {
                    tool_use_id: id,
                    content,
                    is_error: failed,
                }]
            }
            ("item.started", "web_search") => vec![Item::ToolUse {
                id,
                name: "web_search".into(),
                input: json!({ "query": item["query"] }),
            }],
            ("item.completed", "web_search") => vec![Item::ToolResult {
                tool_use_id: id,
                content: String::new(),
                is_error: false,
            }],
            ("item.started" | "item.updated" | "item.completed", "todo_list") => {
                vec![Item::Notice {
                    level: Level::Info,
                    text: todo_text(item),
                }]
            }
            ("item.completed", "error") => vec![Item::Notice {
                level: Level::Warn,
                text: item["message"].as_str().unwrap_or("Codex error").into(),
            }],
            ("item.completed", other) if !other.is_empty() => vec![Item::Notice {
                level: Level::Info,
                text: format!("{other}: {}", item),
            }],
            ("turn.completed", _) => {
                let us = &raw["usage"];
                let cached = u(us, "cached_input_tokens");
                vec![Item::TurnEnd {
                    ok: true,
                    subtype: None,
                    cost_usd: None,
                    usage: Some(TokenUsage {
                        // Codex counts cached tokens inside input_tokens.
                        input_tokens: u(us, "input_tokens").saturating_sub(cached),
                        output_tokens: u(us, "output_tokens"),
                        cache_read_tokens: cached,
                        cache_creation_tokens: u(us, "cache_write_input_tokens"),
                    }),
                    duration_ms: None,
                }]
            }
            ("turn.failed", _) | ("error", _) => {
                let mut v = vec![Item::Notice {
                    level: Level::Error,
                    text: raw["error"]["message"]
                        .as_str()
                        .or(raw["message"].as_str())
                        .unwrap_or("Codex reported an error")
                        .into(),
                }];
                if t == "turn.failed" {
                    v.push(Item::TurnEnd {
                        ok: false,
                        subtype: Some("failed".into()),
                        cost_usd: None,
                        usage: None,
                        duration_ms: None,
                    });
                }
                v
            }
            _ => vec![],
        }
    }
}

/// "update a.txt" lines for a file_change item.
fn change_summary(item: &Value) -> String {
    item["changes"]
        .as_array()
        .map(|cs| {
            cs.iter()
                .map(|c| {
                    format!(
                        "{} {}",
                        c["kind"].as_str().unwrap_or("change"),
                        c["path"].as_str().unwrap_or("?")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// A plan update as a checklist.
fn todo_text(item: &Value) -> String {
    let lines: Vec<String> = item["items"]
        .as_array()
        .map(|xs| {
            xs.iter()
                .map(|x| {
                    let done = x["completed"].as_bool().unwrap_or(false);
                    format!(
                        "{} {}",
                        if done { "[x]" } else { "[ ]" },
                        x["text"].as_str().unwrap_or_default()
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    format!("Plan:\n{}", lines.join("\n"))
}
