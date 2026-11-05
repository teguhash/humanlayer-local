//! Claude and Codex CLI adapters.
//!
//! - [`Agent`]: what differs per CLI (flags, stdin messages, parsing).
//! - [`process`]: spawning and streaming, shared by both.
//! - [`runtime::Runtime`]: the session operations the app calls (send,
//!   interrupt, stop, approvals; resume is automatic on send), persisting
//!   every event to SQLite and reporting it to an [`runtime::Sink`].
//! - [`transcript`]: normalized items and usage, rebuilt from stored events.

pub mod claude;
pub mod codex;
pub mod process;
pub mod runtime;
pub mod transcript;

use hl_core::model::AgentKind;
use serde_json::Value;

pub use runtime::{EnvFn, PrepareFn, Runtime, SessionEvent, Sink, Transcript};
pub use transcript::{Decision, Item, SessionUsage};

/// The protocol of one agent CLI. Implementations are stateless; process
/// and session state live in [`Runtime`].
pub trait Agent: Send + Sync {
    fn binary(&self) -> &'static str;
    /// Arguments for a new process; `resume` is the CLI's own session id.
    fn args(&self, cwd: &str, resume: Option<&str>) -> Vec<String>;
    /// One process serves many turns (Claude) or one turn (Codex).
    fn persistent(&self) -> bool;
    /// Lines written right after spawn.
    fn handshake(&self) -> Vec<String> {
        vec![]
    }
    /// The stdin line(s) for a user turn.
    fn user_turn(&self, text: &str) -> String;
    /// A stdin line that ends the current turn but keeps the process, if
    /// the CLI supports it. Otherwise interrupt stops the process.
    fn interrupt(&self, _request_id: &str) -> Option<String> {
        None
    }
    /// The stdin line answering an approval request.
    fn approval(&self, request_id: &str, decision: Decision, request: &Value) -> Option<String>;
    /// `(request_id, request)` if this line asks for approval.
    fn approval_request(&self, _raw: &Value) -> Option<(String, Value)> {
        None
    }
    fn cli_session_id(&self, raw: &Value) -> Option<String>;
    fn is_turn_end(&self, raw: &Value) -> bool;
    /// Drop fields that must not be stored.
    fn sanitize(&self, raw: Value) -> Value {
        raw
    }
    fn parse(&self, raw: &Value) -> Vec<Item>;
}

pub fn protocol(kind: AgentKind) -> &'static dyn Agent {
    match kind {
        AgentKind::Claude => &claude::Claude,
        AgentKind::Codex => &codex::Codex,
    }
}

/// `type` plus a useful sub-key: `subtype` / `request.subtype` for Claude,
/// `item.type` for Codex. E.g. `system/init`, `item.completed/agent_message`.
pub fn event_type(v: &Value) -> Option<String> {
    let t = v.get("type")?.as_str()?;
    let sub = v
        .get("subtype")
        .or_else(|| v.pointer("/request/subtype"))
        .or_else(|| v.pointer("/item/type"))
        .and_then(Value::as_str);
    Some(match sub {
        Some(s) => format!("{t}/{s}"),
        None => t.to_string(),
    })
}

#[cfg(test)]
mod tests;
