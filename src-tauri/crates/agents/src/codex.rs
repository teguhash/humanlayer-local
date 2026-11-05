//! Codex CLI (`codex exec --json`). One process per turn; follow-ups use
//! `codex exec resume <thread_id>`. No per-call approvals in exec mode, so
//! it runs read-only until M5 decides. Verified against 0.166.0.

use serde_json::Value;

use crate::Agent;
use crate::transcript::{Decision, Item, Level, TokenUsage, u};

pub struct Codex;

impl Agent for Codex {
    fn binary(&self) -> &'static str {
        "codex"
    }

    fn args(&self, cwd: &str, resume: Option<&str>) -> Vec<String> {
        let mut v: Vec<String> = vec!["exec".into()];
        match resume {
            // `exec resume` takes no --sandbox/--cd; set the sandbox via
            // config, and the working dir is the process cwd.
            Some(id) => v.extend(
                [
                    "resume",
                    "--json",
                    "--skip-git-repo-check",
                    "-c",
                    "sandbox_mode=\"read-only\"",
                    id,
                ]
                .map(String::from),
            ),
            None => v.extend(
                [
                    "--json",
                    "--skip-git-repo-check",
                    "--sandbox",
                    "read-only",
                    "--cd",
                    cwd,
                ]
                .map(String::from),
            ),
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
