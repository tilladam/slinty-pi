//! Session file entry types, per `docs/session-format.md` of pi-coding-agent
//! (version 3). Deserialization is tolerant: unrecognized entry types map to
//! `EntryKind::Unknown` and message content is kept as raw JSON — pi-sessions
//! only needs a handful of fields out of the `AgentMessage` union, and
//! shouldn't fork out of sync with pi's own (larger) type definitions.

use serde::Deserialize;
use serde_json::Value;

/// First line of a session file. Not part of the entry tree (no `id`/`parentId`).
#[derive(Debug, Clone, Deserialize)]
pub struct SessionHeader {
    #[serde(default = "default_version")]
    pub version: u32,
    pub id: String,
    pub timestamp: String,
    pub cwd: String,
    #[serde(default, rename = "parentSession")]
    pub parent_session: Option<String>,
}

fn default_version() -> u32 {
    1
}

/// One line of the session tree (everything after the header).
#[derive(Debug, Clone, Deserialize)]
pub struct SessionEntry {
    pub id: String,
    #[serde(default, rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(flatten)]
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntryKind {
    /// `message.role` is one of "user" | "assistant" | "toolResult" |
    /// "bashExecution" | "custom" | "branchSummary" | "compactionSummary".
    /// Kept as raw JSON; see `message_role`/`message_text_preview` helpers.
    Message { message: Value },
    #[serde(rename_all = "camelCase")]
    ModelChange { provider: String, model_id: String },
    #[serde(rename_all = "camelCase")]
    ThinkingLevelChange { thinking_level: String },
    #[serde(rename_all = "camelCase")]
    Compaction {
        summary: String,
        #[serde(default)]
        tokens_before: u64,
        #[serde(default)]
        usage: Option<Value>,
    },
    #[serde(rename_all = "camelCase")]
    BranchSummary { from_id: String, summary: String },
    #[serde(rename_all = "camelCase")]
    Custom {
        custom_type: String,
        #[serde(default)]
        data: Option<Value>,
    },
    #[serde(rename_all = "camelCase")]
    CustomMessage {
        custom_type: String,
        content: Value,
        #[serde(default)]
        display: bool,
    },
    Label {
        #[serde(rename = "targetId")]
        target_id: String,
        #[serde(default)]
        label: Option<String>,
    },
    SessionInfo { name: String },
    #[serde(other)]
    Unknown,
}

/// Best-effort role of a `message` entry's payload ("user", "assistant", …),
/// or `None` for non-message entries.
pub fn message_role(kind: &EntryKind) -> Option<&str> {
    match kind {
        EntryKind::Message { message } => message.get("role").and_then(Value::as_str),
        _ => None,
    }
}

/// Plain-text preview of a message's `content` field, which is either a bare
/// string or `(TextContent | ImageContent | ...)[]`. Used for session-list
/// titles and full-text search; not a faithful content reconstruction.
pub fn content_preview(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// Total cost (USD) reported on a message entry, if any (`usage.cost.total`
/// on assistant/toolResult messages, or on compaction/branch_summary usage).
pub fn entry_cost(kind: &EntryKind) -> f64 {
    let usage = match kind {
        EntryKind::Message { message } => message.get("usage"),
        EntryKind::Compaction { usage, .. } => usage.as_ref(),
        _ => None,
    };
    usage
        .and_then(|u| u.pointer("/cost/total"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

/// Total tokens reported on a message entry, if any (`usage.totalTokens`).
pub fn entry_tokens(kind: &EntryKind) -> u64 {
    let usage = match kind {
        EntryKind::Message { message } => message.get("usage"),
        EntryKind::Compaction { usage, .. } => usage.as_ref(),
        _ => None,
    };
    usage
        .and_then(|u| u.get("totalTokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header() {
        let line = r#"{"type":"session","version":3,"id":"abc","timestamp":"2026-01-01T00:00:00.000Z","cwd":"/tmp/x"}"#;
        let h: SessionHeader = serde_json::from_str(line).unwrap();
        assert_eq!(h.version, 3);
        assert_eq!(h.cwd, "/tmp/x");
        assert!(h.parent_session.is_none());
    }

    #[test]
    fn parses_message_entry() {
        let line = r#"{"type":"message","id":"a1","parentId":"a0","timestamp":"t","message":{"role":"user","content":"hi"}}"#;
        let e: SessionEntry = serde_json::from_str(line).unwrap();
        assert_eq!(e.id, "a1");
        assert_eq!(e.parent_id.as_deref(), Some("a0"));
        assert_eq!(message_role(&e.kind), Some("user"));
    }

    #[test]
    fn unknown_entry_kind_is_tolerated() {
        let line = r#"{"type":"totally_new","id":"x","parentId":null,"timestamp":"t"}"#;
        let e: SessionEntry = serde_json::from_str(line).unwrap();
        assert!(matches!(e.kind, EntryKind::Unknown));
    }

    #[test]
    fn content_preview_handles_string_and_array() {
        assert_eq!(content_preview(&Value::String("hi".into())), "hi");
        let arr = serde_json::json!([{"type":"text","text":"a"},{"type":"image","data":".."},{"type":"text","text":"b"}]);
        assert_eq!(content_preview(&arr), "a b");
    }
}
