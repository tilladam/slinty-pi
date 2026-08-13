//! Message/segment -> [`RowSpec`] construction, and hydration of a
//! `get_messages` payload into a full row list.
//!
//! `hydrate_rowspecs` and the live streaming path (`pi_core::backend::
//! Transcript`) share every building block here (`spec_for_segment`,
//! `tool_summary`, `content_text`) so a resumed transcript and a
//! freshly-streamed one render identically.

use std::collections::HashMap;

use pi_rpc::content_text;

use crate::highlight;
use crate::segmenter::{self, segment_markdown, Segment};

pub const TOOL_DETAIL_LIMIT: usize = 4000;

// ---------------------------------------------------------------------------
// Row specs: plain, UI-toolkit-agnostic row descriptions. Each frontend
// converts these to its own native row/view type (Slint: `row_to_slint` in
// slinty-pi; Swift: `RowRecord` conversions in pi-core-ffi).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowSpec {
    pub kind: &'static str,
    /// Markdown for the styled field (prose only; code and tables render
    /// through `code_lines`/`table_rows` instead).
    pub markdown: Option<String>,
    pub text: String,
    pub lang: String,
    pub level: i32,
    pub detail: String,
    pub running: bool,
    pub elapsed: String,
    pub first: bool,
    /// The full original markdown of this row's enclosing message/text
    /// block, shared by every row segmented out of it — used for the
    /// per-message copy affordance, which copies the source text rather
    /// than any one rendered/segmented piece of it. Empty where a group
    /// copy isn't offered (thinking/tool/info rows).
    pub raw: String,
    /// "code" rows: per-line, per-span highlighted content.
    pub code_lines: Vec<highlight::CodeLine>,
    /// "table" rows: row-major cells (header row first when present).
    pub table_rows: Vec<Vec<segmenter::TableCell>>,
}

impl RowSpec {
    pub fn note(kind: &'static str, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            ..Self::default()
        }
    }
}

pub fn spec_for_segment(segment: &Segment, first: bool, dark: bool, raw: &str) -> RowSpec {
    let raw = raw.to_string();
    match segment {
        Segment::Prose(md) => RowSpec {
            kind: "prose",
            markdown: Some(md.clone()),
            first,
            raw,
            ..RowSpec::default()
        },
        Segment::Heading { level, text } => RowSpec {
            kind: "heading",
            text: text.clone(),
            level: *level as i32,
            first,
            raw,
            ..RowSpec::default()
        },
        Segment::Code { lang, code } => RowSpec {
            kind: "code",
            code_lines: highlight::highlight_lines(code, lang, dark),
            text: code.clone(),
            lang: lang.clone(),
            first,
            raw,
            ..RowSpec::default()
        },
        Segment::Quote(md) => RowSpec {
            kind: "quote",
            markdown: Some(md.clone()),
            first,
            raw,
            ..RowSpec::default()
        },
        Segment::Rule => RowSpec {
            kind: "rule",
            first,
            raw,
            ..RowSpec::default()
        },
        Segment::Table(rows) => RowSpec {
            kind: "table",
            table_rows: rows.clone(),
            first,
            raw,
            ..RowSpec::default()
        },
    }
}

pub fn tool_summary(tool_name: &str, args: &serde_json::Value) -> String {
    let detail = match tool_name {
        "bash" => args.get("command").and_then(|v| v.as_str()),
        "read" | "write" | "edit" => args
            .get("path")
            .or_else(|| args.get("file_path"))
            .and_then(|v| v.as_str()),
        _ => None,
    };
    match detail {
        Some(d) => format!("{tool_name}  {}", first_line(d)),
        None => tool_name.to_string(),
    }
}

pub fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

pub fn tail(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    let mut start = s.len() - limit;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &s[start..])
}

pub fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ---------------------------------------------------------------------------
// Hydration: turn a `get_messages` payload (`AgentMessage[]`, per
// docs/session-format.md) into RowSpecs. Same building blocks as the live
// streaming path (`spec_for_segment`, `tool_summary`, `content_text`),
// applied to complete historical messages instead of deltas — so a resumed
// transcript and a freshly-streamed one render identically.
// ---------------------------------------------------------------------------

pub fn hydrate_rowspecs(messages: &[serde_json::Value], dark: bool) -> Vec<RowSpec> {
    let mut specs: Vec<RowSpec> = Vec::new();
    let mut tool_rows: HashMap<String, usize> = HashMap::new();
    let mut tool_summaries: HashMap<String, String> = HashMap::new();
    let mut pending_first = false;

    for message in messages {
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        // Every top-level message starts a new visual group, same as a
        // `message_start` event on the live path — except `toolResult`,
        // which never produces its own row (it updates the matching
        // `toolCall` row in place).
        if role != "toolResult" {
            pending_first = true;
        }
        match role {
            "user" => {
                let content = message.get("content").unwrap_or(&serde_json::Value::Null);
                let (text, images) = user_content_text(content);
                let display = if images > 0 {
                    format!(
                        "{text}\n[{images} image{}]",
                        if images == 1 { "" } else { "s" }
                    )
                } else {
                    text
                };
                let mut spec = RowSpec::note("user", display.clone());
                spec.first = std::mem::take(&mut pending_first);
                spec.raw = display;
                specs.push(spec);
            }
            "assistant" => {
                let Some(blocks) = message.get("content").and_then(|v| v.as_array()) else {
                    continue;
                };
                // Reserve `pending_first` for the message's first *text* block when
                // one exists, regardless of whether a `thinking`/`toolCall` block
                // comes before it — otherwise a reasoning or tool-call step ahead of
                // the reply text silently steals the flag, and the text segment that
                // follows never gets `first = true`, which is also what gates the
                // per-message copy/speak affordance in both frontends. A
                // thinking/toolCall row only claims the message-start spacing bump
                // itself when the message has no text block at all to give it to.
                let has_text = blocks.iter().any(|block| {
                    block.get("type").and_then(|v| v.as_str()) == Some("text")
                        && block
                            .get("text")
                            .and_then(|v| v.as_str())
                            .is_some_and(|t| !t.is_empty())
                });
                for block in blocks {
                    match block.get("type").and_then(|v| v.as_str()) {
                        Some("thinking") => {
                            let thinking =
                                block.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                            let first = if has_text {
                                false
                            } else {
                                std::mem::take(&mut pending_first)
                            };
                            specs.push(RowSpec {
                                kind: "thinking",
                                text: thinking.to_string(),
                                running: false,
                                first,
                                ..RowSpec::default()
                            });
                        }
                        Some("text") => {
                            let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            if text.is_empty() {
                                continue;
                            }
                            for (i, segment) in segment_markdown(text).iter().enumerate() {
                                let first = i == 0 && std::mem::take(&mut pending_first);
                                specs.push(spec_for_segment(segment, first, dark, text));
                            }
                        }
                        Some("toolCall") => {
                            let id = block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let args = block.get("arguments").cloned().unwrap_or_default();
                            let summary = tool_summary(name, &args);
                            let args_pretty =
                                serde_json::to_string_pretty(&args).unwrap_or_default();
                            let index = specs.len();
                            let first = if has_text {
                                false
                            } else {
                                std::mem::take(&mut pending_first)
                            };
                            specs.push(RowSpec {
                                kind: "tool",
                                text: format!("⚙ {summary}"),
                                detail: args_pretty,
                                running: true,
                                first,
                                ..RowSpec::default()
                            });
                            if !id.is_empty() {
                                tool_rows.insert(id.clone(), index);
                                tool_summaries.insert(id, summary);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "toolResult" => {
                let id = message
                    .get("toolCallId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let Some(&index) = tool_rows.get(id) else {
                    continue;
                };
                let Some(spec) = specs.get_mut(index) else {
                    continue;
                };
                let is_error = message
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let output = tail(&content_text(message), TOOL_DETAIL_LIMIT);
                let mark = if is_error { "✗" } else { "✓" };
                let summary = tool_summaries.get(id).cloned().unwrap_or_default();
                spec.text = format!("{mark} {summary}");
                spec.running = false;
                if !output.is_empty() {
                    spec.detail = format!("{}\n───\n{output}", spec.detail);
                }
            }
            "bashExecution" => {
                let command = message
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let output = message.get("output").and_then(|v| v.as_str()).unwrap_or("");
                let mark = match message.get("exitCode").and_then(|v| v.as_i64()) {
                    Some(0) => "✓",
                    Some(_) => "✗",
                    None => "⚙",
                };
                specs.push(RowSpec {
                    kind: "tool",
                    text: format!("{mark} bash  {}", first_line(command)),
                    detail: tail(output, TOOL_DETAIL_LIMIT),
                    first: std::mem::take(&mut pending_first),
                    ..RowSpec::default()
                });
            }
            "compactionSummary" => {
                let tokens_before = message
                    .get("tokensBefore")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let mut spec = RowSpec::note(
                    "info",
                    format!(
                        "context compacted · {} tokens before",
                        format_tokens(tokens_before)
                    ),
                );
                spec.first = std::mem::take(&mut pending_first);
                specs.push(spec);
            }
            "branchSummary" => {
                let summary = message
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let mut spec = RowSpec::note("info", format!("branched · {summary}"));
                spec.first = std::mem::take(&mut pending_first);
                specs.push(spec);
            }
            "custom"
                if message
                    .get("display")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false) =>
            {
                let content = message.get("content").unwrap_or(&serde_json::Value::Null);
                let (text, _) = user_content_text(content);
                let mut spec = RowSpec::note("info", text);
                spec.first = std::mem::take(&mut pending_first);
                specs.push(spec);
            }
            _ => {}
        }
    }
    specs
}

/// Join `TextContent` blocks and count `ImageContent` blocks in a
/// `UserMessage`/`CustomMessage`-shaped `content` field (bare string or
/// `(TextContent | ImageContent)[]`).
pub fn user_content_text(content: &serde_json::Value) -> (String, usize) {
    match content {
        serde_json::Value::String(s) => (s.clone(), 0),
        serde_json::Value::Array(items) => {
            let mut text = Vec::new();
            let mut images = 0;
            for item in items {
                match item.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                            text.push(t.to_string());
                        }
                    }
                    Some("image") => images += 1,
                    _ => {}
                }
            }
            (text.join("\n"), images)
        }
        _ => (String::new(), 0),
    }
}

#[cfg(test)]
mod format_tokens_tests {
    use super::format_tokens;

    #[test]
    fn below_one_thousand_is_a_bare_integer() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn thousands_get_a_k_suffix() {
        assert_eq!(format_tokens(1_000), "1.0k");
        assert_eq!(format_tokens(48_000), "48.0k");
        assert_eq!(format_tokens(999_499), "999.5k");
    }

    #[test]
    fn millions_get_an_m_suffix() {
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(2_500_000), "2.5M");
    }
}

#[cfg(test)]
mod hydrate_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hydrates_user_and_assistant_text() {
        let messages = vec![
            json!({"role": "user", "content": [{"type": "text", "text": "hello"}]}),
            json!({
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "pondering"},
                    {"type": "text", "text": "hi there"}
                ]
            }),
        ];
        let specs = hydrate_rowspecs(&messages, false);
        let kinds: Vec<&str> = specs.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec!["user", "thinking", "prose"]);
        assert!(specs[0].first, "user row starts a new group");
        assert!(
            !specs[1].first,
            "thinking isn't the copy-eligible row when the message also has text"
        );
        assert!(
            specs[2].first,
            "the reply's text is what should carry the group-copy/speak affordance, \
             not the thinking block ahead of it"
        );
        assert_eq!(specs[0].text, "hello");
        assert_eq!(specs[1].text, "pondering");
        assert!(
            !specs[1].running,
            "hydrated thinking is never still-running"
        );
    }

    #[test]
    fn thinking_or_tool_call_ahead_of_text_does_not_steal_the_first_flag() {
        // Regression test: a message shaped `thinking -> text` (or
        // `toolCall -> text`) used to have its `thinking`/`tool` row
        // consume the message's one `first` flag before the reply text
        // ever got a chance at it, silently disabling the group-copy/speak
        // affordance (gated on `first`) for every such reply — exactly the
        // shape of a turn that reasons or calls a tool before answering.
        let messages = vec![json!({
            "role": "assistant",
            "content": [
                {"type": "toolCall", "id": "call_1", "name": "bash", "arguments": {"command": "ls"}},
                {"type": "text", "text": "done"}
            ]
        })];
        let specs = hydrate_rowspecs(&messages, false);
        let kinds: Vec<&str> = specs.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec!["tool", "prose"]);
        assert!(!specs[0].first, "the tool row shouldn't claim the flag");
        assert!(specs[1].first, "the text segment should get it instead");
    }

    #[test]
    fn a_lone_thinking_block_with_no_text_still_gets_first_for_spacing() {
        // When a message has nothing else to give the flag to, a leading
        // thinking/tool row should still mark the start of a new message
        // group (used for spacing in both frontends) — only messages that
        // *also* have a text block redirect `first` to that text.
        let messages = vec![json!({
            "role": "assistant",
            "content": [{"type": "thinking", "thinking": "still going"}]
        })];
        let specs = hydrate_rowspecs(&messages, false);
        assert!(specs[0].first);
    }

    #[test]
    fn raw_is_the_original_text_block_shared_by_every_segment() {
        let text = "intro prose\n\n```rust\nfn f() {}\n```\n\noutro prose";
        let messages = vec![json!({
            "role": "assistant",
            "content": [{"type": "text", "text": text}]
        })];
        let specs = hydrate_rowspecs(&messages, false);
        assert_eq!(specs.len(), 3, "prose, code, prose");
        assert!(
            specs.iter().all(|s| s.raw == text),
            "every segment shares the full block"
        );
        assert!(specs[0].first);
        assert!(!specs[1].first);
        assert!(!specs[2].first);
    }

    #[test]
    fn raw_is_empty_for_thinking_and_tool_rows() {
        let messages = vec![json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "hmm"},
                {"type": "toolCall", "id": "call_1", "name": "bash", "arguments": {"command": "ls"}}
            ]
        })];
        let specs = hydrate_rowspecs(&messages, false);
        assert!(specs.iter().all(|s| s.raw.is_empty()));
    }

    #[test]
    fn matches_tool_call_to_its_result() {
        let messages = vec![
            json!({"role": "user", "content": "run tests"}),
            json!({
                "role": "assistant",
                "content": [{"type": "toolCall", "id": "call_1", "name": "bash", "arguments": {"command": "cargo test"}}]
            }),
            json!({
                "role": "toolResult",
                "toolCallId": "call_1",
                "toolName": "bash",
                "content": [{"type": "text", "text": "test result: ok"}],
                "isError": false
            }),
        ];
        let specs = hydrate_rowspecs(&messages, false);
        let tool = specs.iter().find(|s| s.kind == "tool").expect("tool row");
        assert!(!tool.running);
        assert!(tool.text.starts_with('✓'), "text was {:?}", tool.text);
        assert!(tool.detail.contains("test result: ok"));
    }

    #[test]
    fn tool_error_result_marks_the_row_failed() {
        let messages = vec![
            json!({
                "role": "assistant",
                "content": [{"type": "toolCall", "id": "call_e", "name": "bash", "arguments": {"command": "false"}}]
            }),
            json!({
                "role": "toolResult",
                "toolCallId": "call_e",
                "toolName": "bash",
                "content": [{"type": "text", "text": "exit 1"}],
                "isError": true
            }),
        ];
        let specs = hydrate_rowspecs(&messages, false);
        assert!(specs[0].text.starts_with('✗'));
    }

    #[test]
    fn unmatched_tool_call_stays_running() {
        // An interrupted session: the call was made but pi never got a result.
        let messages = vec![json!({
            "role": "assistant",
            "content": [{"type": "toolCall", "id": "call_2", "name": "bash", "arguments": {"command": "sleep 100"}}]
        })];
        let specs = hydrate_rowspecs(&messages, false);
        assert!(specs[0].running);
    }

    #[test]
    fn maps_bash_execution_and_summaries() {
        let messages = vec![
            json!({"role": "bashExecution", "command": "ls", "output": "a.txt", "exitCode": 0, "cancelled": false, "truncated": false}),
            json!({"role": "compactionSummary", "summary": "…", "tokensBefore": 48000}),
            json!({"role": "branchSummary", "summary": "explored X first", "fromId": "abc"}),
        ];
        let specs = hydrate_rowspecs(&messages, false);
        assert_eq!(specs[0].kind, "tool");
        assert!(specs[0].text.starts_with('✓'));
        assert_eq!(specs[1].kind, "info");
        assert!(specs[1].text.contains("compacted"));
        assert_eq!(specs[2].kind, "info");
        assert!(specs[2].text.contains("explored X first"));
    }

    #[test]
    fn skips_non_displayed_custom_messages() {
        let messages = vec![
            json!({"role": "custom", "customType": "x", "content": "hidden", "display": false}),
            json!({"role": "custom", "customType": "x", "content": "shown", "display": true}),
        ];
        let specs = hydrate_rowspecs(&messages, false);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].text, "shown");
    }

    #[test]
    fn counts_images_in_user_content() {
        let messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "look"},
                {"type": "image", "data": "..", "mimeType": "image/png"}
            ]
        })];
        let specs = hydrate_rowspecs(&messages, false);
        assert!(
            specs[0].text.contains("1 image"),
            "text was {:?}",
            specs[0].text
        );
    }

    #[test]
    fn multi_turn_session_round_trips_in_order() {
        // Mirrors a real session on disk: user -> assistant(thinking+toolCall)
        // -> toolResult -> assistant(text).
        let messages = vec![
            json!({"role": "user", "content": [{"type": "text", "text": "do you have access to my mcp?"}]}),
            json!({
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "let's check"},
                    {"type": "toolCall", "id": "call_x", "name": "mcp", "arguments": {"server": "obsidian"}}
                ]
            }),
            json!({
                "role": "toolResult",
                "toolCallId": "call_x",
                "toolName": "mcp",
                "content": [{"type": "text", "text": "obsidian (17 tools)"}],
                "isError": false
            }),
            json!({"role": "assistant", "content": [{"type": "text", "text": "Yes, I can interact with it."}]}),
        ];
        let specs = hydrate_rowspecs(&messages, false);
        let kinds: Vec<&str> = specs.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec!["user", "thinking", "tool", "prose"]);
        assert!(!specs[2].running);
        assert!(
            specs[3].first,
            "second assistant message starts a new group"
        );
    }
}
