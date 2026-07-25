//! Wire types for pi's RPC mode, per `docs/rpc.md` of pi-coding-agent.
//!
//! Deserialization is deliberately tolerant: unknown event kinds map to
//! `Event::Unknown` / `AssistantMessageEvent::Unknown` and unknown fields are
//! ignored, so newer pi versions don't break the client.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamingBehavior {
    Steer,
    FollowUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageContent {
    #[serde(rename = "type")]
    pub kind: String, // always "image"
    pub data: String, // base64
    pub mime_type: String,
}

/// Commands sent to pi on stdin. The request `id` is attached by the client.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    #[serde(rename_all = "camelCase")]
    Prompt {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        images: Option<Vec<ImageContent>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        streaming_behavior: Option<StreamingBehavior>,
    },
    Steer {
        message: String,
    },
    FollowUp {
        message: String,
    },
    Abort,
    #[serde(rename_all = "camelCase")]
    NewSession {
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_session: Option<String>,
    },
    GetState,
    GetMessages,
    #[serde(rename_all = "camelCase")]
    SetModel {
        provider: String,
        model_id: String,
    },
    CycleModel,
    GetAvailableModels,
    SetThinkingLevel {
        level: ThinkingLevel,
    },
    GetAvailableThinkingLevels,
    #[serde(rename_all = "camelCase")]
    Compact {
        #[serde(skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
    },
    SetAutoCompaction {
        enabled: bool,
    },
    SetAutoRetry {
        enabled: bool,
    },
    AbortRetry,
    Bash {
        command: String,
    },
    AbortBash,
    GetSessionStats,
    #[serde(rename_all = "camelCase")]
    ExportHtml {
        #[serde(skip_serializing_if = "Option::is_none")]
        output_path: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    SwitchSession {
        session_path: String,
    },
    #[serde(rename_all = "camelCase")]
    Fork {
        entry_id: String,
    },
    Clone,
    GetForkMessages,
    GetEntries {
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<String>,
    },
    GetTree,
    GetLastAssistantText,
    SetSessionName {
        name: String,
    },
    GetCommands,
}

/// `{"type":"response", ...}` line from pi.
#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub id: Option<String>,
    pub command: String,
    pub success: bool,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Streaming delta inside a `message_update` event.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantMessageEvent {
    Start,
    #[serde(rename_all = "camelCase")]
    TextStart {
        #[serde(default)]
        content_index: u64,
    },
    #[serde(rename_all = "camelCase")]
    TextDelta {
        #[serde(default)]
        content_index: u64,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename_all = "camelCase")]
    TextEnd {
        #[serde(default)]
        content_index: u64,
        #[serde(default)]
        content: String,
    },
    ThinkingStart,
    ThinkingDelta {
        #[serde(default)]
        delta: String,
    },
    ThinkingEnd,
    ToolcallStart,
    ToolcallDelta,
    #[serde(rename_all = "camelCase")]
    ToolcallEnd {
        #[serde(default)]
        tool_call: Value,
    },
    Done {
        #[serde(default)]
        reason: String,
    },
    Error {
        #[serde(default)]
        reason: String,
    },
    #[serde(other)]
    Unknown,
}

/// A dialog or notification request from a pi extension (`ctx.ui.*`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionUiRequest {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub prefill: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, Value>,
}

/// Events streamed by pi on stdout (everything that is not a `response`).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    AgentStart,
    #[serde(rename_all = "camelCase")]
    AgentEnd {
        #[serde(default)]
        will_retry: bool,
        #[serde(default)]
        messages: Value,
    },
    AgentSettled,
    TurnStart,
    #[serde(rename_all = "camelCase")]
    TurnEnd {
        #[serde(default)]
        message: Value,
        #[serde(default)]
        tool_results: Value,
    },
    MessageStart {
        #[serde(default)]
        message: Value,
    },
    #[serde(rename_all = "camelCase")]
    MessageUpdate {
        #[serde(default)]
        message: Value,
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        #[serde(default)]
        message: Value,
    },
    #[serde(rename_all = "camelCase")]
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        #[serde(default)]
        args: Value,
    },
    #[serde(rename_all = "camelCase")]
    ToolExecutionUpdate {
        tool_call_id: String,
        #[serde(default)]
        tool_name: String,
        #[serde(default)]
        args: Value,
        #[serde(default)]
        partial_result: Value,
    },
    #[serde(rename_all = "camelCase")]
    ToolExecutionEnd {
        tool_call_id: String,
        #[serde(default)]
        tool_name: String,
        #[serde(default)]
        result: Value,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(rename_all = "camelCase")]
    QueueUpdate {
        #[serde(default)]
        steering: Vec<String>,
        #[serde(default)]
        follow_up: Vec<String>,
    },
    CompactionStart {
        #[serde(default)]
        reason: String,
    },
    #[serde(rename_all = "camelCase")]
    CompactionEnd {
        #[serde(default)]
        reason: String,
        #[serde(default)]
        result: Value,
        #[serde(default)]
        aborted: bool,
        #[serde(default)]
        will_retry: bool,
        #[serde(default)]
        error_message: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    AutoRetryStart {
        #[serde(default)]
        attempt: u32,
        #[serde(default)]
        max_attempts: u32,
        #[serde(default)]
        delay_ms: u64,
        #[serde(default)]
        error_message: String,
    },
    #[serde(rename_all = "camelCase")]
    AutoRetryEnd {
        #[serde(default)]
        success: bool,
        #[serde(default)]
        attempt: u32,
        #[serde(default)]
        final_error: Option<String>,
    },
    ExtensionUiRequest(ExtensionUiRequest),
    #[serde(rename_all = "camelCase")]
    ExtensionError {
        #[serde(default)]
        extension_path: String,
        #[serde(default)]
        event: String,
        #[serde(default)]
        error: String,
    },
    /// Any event kind this client doesn't know about (forward compatibility).
    #[serde(other)]
    Unknown,
}

/// Reply to a dialog-type [`ExtensionUiRequest`].
#[derive(Debug, Clone)]
pub enum ExtensionUiReply {
    Value(String),
    Confirmed(bool),
    Cancelled,
}

/// Extract the concatenated text content from a tool result / partial result
/// (`{"content": [{"type": "text", "text": ...}, ...]}`).
pub fn content_text(result: &Value) -> String {
    let Some(items) = result.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    items
        .iter()
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_serializes_with_snake_case_tag() {
        let cmd = Command::Prompt {
            message: "hi".into(),
            images: None,
            streaming_behavior: Some(StreamingBehavior::Steer),
        };
        let v = serde_json::to_value(&cmd).unwrap();
        assert_eq!(v["type"], "prompt");
        assert_eq!(v["message"], "hi");
        assert_eq!(v["streamingBehavior"], "steer");
        assert!(v.get("images").is_none());

        let v = serde_json::to_value(Command::GetAvailableModels).unwrap();
        assert_eq!(v["type"], "get_available_models");

        let v = serde_json::to_value(Command::FollowUp {
            message: "m".into(),
        })
        .unwrap();
        assert_eq!(v["type"], "follow_up");
    }

    #[test]
    fn text_delta_event_parses() {
        let line = r#"{"type":"message_update","message":{},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hello ","partial":{}}}"#;
        let ev: Event = serde_json::from_str(line).unwrap();
        match ev {
            Event::MessageUpdate {
                assistant_message_event: AssistantMessageEvent::TextDelta { delta, .. },
                ..
            } => assert_eq!(delta, "Hello "),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn unknown_event_kind_is_tolerated() {
        let ev: Event = serde_json::from_str(r#"{"type":"totally_new_event","x":1}"#).unwrap();
        assert!(matches!(ev, Event::Unknown));
    }

    #[test]
    fn extension_ui_request_parses() {
        let line = r#"{"type":"extension_ui_request","id":"u1","method":"confirm","title":"Clear?","message":"sure?","timeout":5000}"#;
        let ev: Event = serde_json::from_str(line).unwrap();
        match ev {
            Event::ExtensionUiRequest(r) => {
                assert_eq!(r.method, "confirm");
                assert_eq!(r.timeout, Some(5000));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn content_text_joins_text_blocks() {
        let v: Value = serde_json::json!({
            "content": [
                {"type": "text", "text": "a"},
                {"type": "image", "data": "..."},
                {"type": "text", "text": "b"}
            ]
        });
        assert_eq!(content_text(&v), "a\nb");
    }
}
