//! Claude Code CLI (`claude -p` stream-json). Verified against 2.1.314;
//! see docs/m0-findings.md and docs/m2-findings.md.

use serde_json::{Value, json};

use crate::Agent;
use crate::transcript::{Decision, Item, Level, TokenUsage, u};

pub struct Claude;

impl Agent for Claude {
    fn binary(&self) -> &'static str {
        "claude"
    }

    fn args(&self, _cwd: &str, resume: Option<&str>) -> Vec<String> {
        let mut v: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "default",
            // Hidden flag: permission prompts come to us as
            // control_request/can_use_tool. Without it they auto-deny.
            "--permission-prompt-tool",
            "stdio",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(id) = resume {
            v.push("--resume".into());
            v.push(id.into());
        }
        v
    }

    fn persistent(&self) -> bool {
        true
    }

    fn handshake(&self) -> Vec<String> {
        vec![
            json!({"type":"control_request","request_id":"init","request":{"subtype":"initialize"}})
                .to_string(),
        ]
    }

    fn user_turn(&self, text: &str) -> String {
        json!({"type":"user","message":{"role":"user","content":text}}).to_string()
    }

    fn interrupt(&self, request_id: &str) -> Option<String> {
        Some(
            json!({"type":"control_request","request_id":request_id,"request":{"subtype":"interrupt"}})
                .to_string(),
        )
    }

    fn approval(&self, request_id: &str, decision: Decision, request: &Value) -> Option<String> {
        let input = request["input"].clone();
        let response = match decision {
            Decision::AllowOnce => json!({ "behavior": "allow", "updatedInput": input }),
            Decision::AllowSession => json!({
                "behavior": "allow",
                "updatedInput": input,
                "updatedPermissions": session_permissions(request),
            }),
            Decision::Deny => {
                json!({ "behavior": "deny", "message": "Denied by user in HumanLayer Local." })
            }
        };
        Some(
            json!({
                "type": "control_response",
                "response": { "subtype": "success", "request_id": request_id, "response": response }
            })
            .to_string(),
        )
    }

    fn cli_session_id(&self, raw: &Value) -> Option<String> {
        match raw["type"].as_str()? {
            "system" if raw["subtype"] == "init" => raw["session_id"].as_str().map(String::from),
            "result" => raw["session_id"].as_str().map(String::from),
            _ => None,
        }
    }

    fn is_turn_end(&self, raw: &Value) -> bool {
        raw["type"] == "result"
    }

    fn approval_request(&self, raw: &Value) -> Option<(String, Value)> {
        if raw["type"] == "control_request" && raw["request"]["subtype"] == "can_use_tool" {
            Some((
                raw["request_id"].as_str()?.to_string(),
                raw["request"].clone(),
            ))
        } else {
            None
        }
    }

    /// The `initialize` reply carries the signed-in account (email, org).
    /// It is not needed, so it is never stored.
    fn sanitize(&self, mut raw: Value) -> Value {
        if raw["type"] == "control_response"
            && let Some(r) = raw
                .pointer_mut("/response/response")
                .and_then(Value::as_object_mut)
        {
            r.remove("account");
        }
        raw
    }

    fn parse(&self, raw: &Value) -> Vec<Item> {
        let t = raw["type"].as_str().unwrap_or_default();
        let sub = raw["subtype"].as_str().unwrap_or_default();
        match (t, sub) {
            ("system", "init") => vec![Item::SessionInit {
                cli_session_id: raw["session_id"].as_str().unwrap_or_default().into(),
                model: raw["model"].as_str().map(String::from),
            }],
            ("assistant", _) => blocks(raw)
                .iter()
                .filter_map(|b| match b["type"].as_str()? {
                    "text" => Some(Item::AssistantText {
                        text: b["text"].as_str()?.into(),
                    }),
                    "thinking" => Some(Item::Thinking {
                        text: b["thinking"].as_str()?.into(),
                    })
                    .filter(|i| !matches!(i, Item::Thinking { text } if text.is_empty())),
                    "tool_use" => Some(Item::ToolUse {
                        id: b["id"].as_str()?.into(),
                        name: b["name"].as_str()?.into(),
                        input: b["input"].clone(),
                    }),
                    _ => None,
                })
                .collect(),
            ("user", _) => blocks(raw)
                .iter()
                .filter_map(|b| match b["type"].as_str()? {
                    "tool_result" => Some(Item::ToolResult {
                        tool_use_id: b["tool_use_id"].as_str()?.into(),
                        content: content_text(&b["content"]),
                        is_error: b["is_error"].as_bool().unwrap_or(false),
                    }),
                    // Claude writes this marker after an interrupt.
                    "text" => b["text"]
                        .as_str()
                        .filter(|t| t.starts_with("[Request interrupted"))
                        .map(|t| Item::Notice {
                            level: Level::Info,
                            text: t.into(),
                        }),
                    _ => None,
                })
                .collect(),
            ("control_request", _) if raw["request"]["subtype"] == "can_use_tool" => {
                let r = &raw["request"];
                vec![Item::ApprovalRequest {
                    request_id: raw["request_id"].as_str().unwrap_or_default().into(),
                    tool_name: r["tool_name"].as_str().unwrap_or_default().into(),
                    input: r["input"].clone(),
                    description: r["description"].as_str().map(String::from),
                }]
            }
            ("result", _) => {
                let us = &raw["usage"];
                vec![Item::TurnEnd {
                    ok: !raw["is_error"].as_bool().unwrap_or(false) && sub == "success",
                    subtype: Some(sub.to_string()),
                    cost_usd: raw["total_cost_usd"].as_f64(),
                    usage: Some(TokenUsage {
                        input_tokens: u(us, "input_tokens"),
                        output_tokens: u(us, "output_tokens"),
                        cache_read_tokens: u(us, "cache_read_input_tokens"),
                        cache_creation_tokens: u(us, "cache_creation_input_tokens"),
                    }),
                    duration_ms: raw["duration_ms"].as_u64(),
                }]
            }
            _ => vec![],
        }
    }
}

fn blocks(raw: &Value) -> Vec<Value> {
    match &raw["message"]["content"] {
        Value::Array(a) => a.clone(),
        Value::String(s) => vec![json!({"type":"text","text":s})],
        _ => vec![],
    }
}

fn content_text(c: &Value) -> String {
    match c {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// "Allow for this session": the CLI's own suggestions, but always scoped
/// to the session so nothing is written to the user's settings files. If
/// there are none, allow this tool for the session.
pub fn session_permissions(request: &Value) -> Value {
    let mut sugg: Vec<Value> = request["permission_suggestions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for s in &mut sugg {
        s["destination"] = json!("session");
    }
    if sugg.is_empty() {
        sugg.push(json!({
            "type": "addRules",
            "rules": [{ "toolName": request["tool_name"] }],
            "behavior": "allow",
            "destination": "session",
        }));
    }
    Value::Array(sugg)
}
