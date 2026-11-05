//! Normalized transcript items and per-session usage, derived from stored
//! events. Parsing happens at read time, so parser fixes apply to old
//! sessions too.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use hl_core::model::{AgentKind, Event};

use crate::protocol;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    AllowOnce,
    AllowSession,
    Deny,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

impl TokenUsage {
    fn add(&mut self, o: &TokenUsage) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.cache_read_tokens += o.cache_read_tokens;
        self.cache_creation_tokens += o.cache_creation_tokens;
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Item {
    UserPrompt {
        text: String,
    },
    AssistantText {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    ApprovalRequest {
        request_id: String,
        tool_name: String,
        input: Value,
        description: Option<String>,
    },
    ApprovalResponse {
        request_id: String,
        decision: Decision,
    },
    TurnEnd {
        ok: bool,
        subtype: Option<String>,
        /// Claude: cumulative for the current process, not this turn.
        cost_usd: Option<f64>,
        usage: Option<TokenUsage>,
        duration_ms: Option<u64>,
    },
    SessionInit {
        cli_session_id: String,
        model: Option<String>,
    },
    ProcessStarted {
        resume: bool,
    },
    ProcessExit {
        code: Option<i32>,
        success: bool,
        stopped: bool,
    },
    Notice {
        level: Level,
        text: String,
    },
}

/// Items for one stored event.
pub fn items_for(agent: AgentKind, kind: &str, payload: &Value) -> Vec<Item> {
    let s = |k: &str| payload[k].as_str().unwrap_or_default().to_string();
    match kind {
        "user_prompt" => vec![Item::UserPrompt { text: s("text") }],
        "cli" => protocol(agent).parse(payload),
        "process_started" => {
            let resume = payload["args"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == "--resume" || x == "resume"));
            vec![Item::ProcessStarted { resume }]
        }
        "process_exit" => vec![Item::ProcessExit {
            code: payload["code"].as_i64().map(|c| c as i32),
            success: payload["success"].as_bool().unwrap_or(false),
            stopped: payload["stopped"].as_bool().unwrap_or(false),
        }],
        "approval_response" => {
            match serde_json::from_value::<Decision>(payload["decision"].clone()) {
                Ok(decision) => vec![Item::ApprovalResponse {
                    request_id: s("request_id"),
                    decision,
                }],
                Err(_) => vec![],
            }
        }
        "interrupt_requested" => vec![Item::Notice {
            level: Level::Info,
            text: "Interrupt requested".into(),
        }],
        "stderr" => vec![Item::Notice {
            level: Level::Warn,
            text: s("line"),
        }],
        "invalid_json" => vec![Item::Notice {
            level: Level::Warn,
            text: format!("Not JSON: {}", s("line")),
        }],
        "error" => vec![Item::Notice {
            level: Level::Error,
            text: s("message"),
        }],
        other => vec![Item::Notice {
            level: Level::Info,
            text: format!("unknown event kind {other}"),
        }],
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct SessionUsage {
    pub tokens: TokenUsage,
    /// Claude only. Sum over CLI processes of each one's last reported
    /// cumulative cost.
    pub cost_usd: f64,
    pub turns: u32,
}

/// Total usage for a session's events.
pub fn usage(agent: AgentKind, events: &[Event]) -> SessionUsage {
    let mut u = SessionUsage::default();
    let mut run_cost = 0.0;
    for ev in events {
        match ev.kind.as_str() {
            "process_started" => {
                u.cost_usd += run_cost;
                run_cost = 0.0;
            }
            "cli" => {
                for item in protocol(agent).parse(&ev.payload) {
                    if let Item::TurnEnd {
                        cost_usd, usage, ..
                    } = item
                    {
                        u.turns += 1;
                        if let Some(t) = usage {
                            u.tokens.add(&t);
                        }
                        if let Some(c) = cost_usd {
                            // Cumulative within a process; an interrupted
                            // turn reports 0, so never go backwards.
                            run_cost = f64::max(run_cost, c);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    u.cost_usd += run_cost;
    u
}

pub(crate) fn u(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}
