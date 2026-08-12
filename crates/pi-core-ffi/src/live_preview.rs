//! Live visibility of "thinking" (reasoning) content and tool-call
//! execution while a turn is still streaming (SW7). Ports (not shares —
//! this crate doesn't depend on `pi-core`, matching its established
//! posture) `pi_core::backend::Transcript`'s `ThinkingRegion`/`ToolRun`
//! state machine, ~verbatim, minus the Slint-specific shadow-row
//! addressing (`push_row`/`ui.set`) — this crate pushes fully-built
//! `RowRecord`s through `ChatSink` instead. See the SW7 plan's Design
//! section for why this is push-based rather than mirroring `preview_rows`'
//! Swift-pulled shape: tool-call rows need `tool_summary`/`tail` formatting
//! that only makes sense running once, server-side, against state Rust
//! already tracks.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use pi_rpc::{AssistantMessageEvent, Event};

use crate::{ChatSink, RowRecord};

/// Same cadence as `pi_core::backend`'s `TEXT_FLUSH` — reused for thinking,
/// not just text, matching the reference implementation.
const THINKING_FLUSH: Duration = Duration::from_millis(33);
/// Same cadence as `pi_core::backend`'s `TOOL_FLUSH`.
const TOOL_FLUSH: Duration = Duration::from_millis(100);

/// At most one live at a time — a fresh `ThinkingStart` mid-turn replaces
/// this with a new region (and a new `id`), it never reuses a prior one.
pub struct ThinkingRegionState {
    id: String,
    buffer: String,
    last_flush: Instant,
}

/// One entry per in-flight tool call, keyed by `tool_call_id` in the
/// caller's map — supports concurrent tool calls, each its own row.
pub struct ToolRunState {
    summary: String,
    args_pretty: String,
    started: Instant,
    last_flush: Instant,
}

/// `pi_core::backend::format_elapsed`, ported verbatim (no `pi_render`/
/// `pi_rpc` home for this — small enough to duplicate locally).
fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 10.0 {
        format!("{secs:.1}s")
    } else if secs < 60.0 {
        format!("{secs:.0}s")
    } else {
        format!("{}m {:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

fn thinking_row(buffer: &str, running: bool) -> RowRecord {
    RowRecord::from(pi_render::RowSpec {
        kind: "thinking",
        text: buffer.to_string(),
        running,
        raw: buffer.to_string(),
        ..pi_render::RowSpec::default()
    })
}

/// Mirrors `tool_start`/`tool_update`/`tool_end`'s exact `RowSpec` shape:
/// `mark` is `"⚙"` while running, `"✓"`/`"✗"` once finished; `output` is
/// the tail-truncated partial/final result text, if any.
fn tool_row(
    mark: &str,
    summary: &str,
    args_pretty: &str,
    output: Option<&str>,
    running: bool,
    elapsed: String,
) -> RowRecord {
    let detail = match output {
        Some(out) if !out.is_empty() => format!("{args_pretty}\n───\n{out}"),
        _ => args_pretty.to_string(),
    };
    RowRecord::from(pi_render::RowSpec {
        kind: "tool",
        text: format!("{mark} {summary}"),
        detail,
        running,
        elapsed,
        ..pi_render::RowSpec::default()
    })
}

/// Dispatches one `Event` against live thinking/tool-call state, pushing
/// `ChatSink::on_thinking_row_changed`/`on_tool_row_changed` as needed.
/// Called from `run()` alongside (not instead of) `apply()` — a separate
/// function so `apply()`'s existing signature/tests stay untouched.
pub fn apply_live_preview(
    event: &Event,
    sink: &dyn ChatSink,
    thinking: &mut Option<ThinkingRegionState>,
    tools: &mut HashMap<String, ToolRunState>,
    thinking_seq: &mut u64,
) {
    match event {
        Event::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::ThinkingStart,
            ..
        } => {
            *thinking_seq += 1;
            let id = format!("thinking-{thinking_seq}");
            sink.on_thinking_row_changed(id.clone(), thinking_row("", true));
            *thinking = Some(ThinkingRegionState {
                id,
                buffer: String::new(),
                last_flush: Instant::now(),
            });
        }
        Event::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::ThinkingDelta { delta },
            ..
        } => {
            if let Some(region) = thinking.as_mut() {
                region.buffer.push_str(delta);
                if region.last_flush.elapsed() >= THINKING_FLUSH {
                    region.last_flush = Instant::now();
                    sink.on_thinking_row_changed(
                        region.id.clone(),
                        thinking_row(&region.buffer, true),
                    );
                }
            }
        }
        Event::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::ThinkingEnd,
            ..
        } => {
            if let Some(region) = thinking.take() {
                sink.on_thinking_row_changed(region.id, thinking_row(&region.buffer, false));
            }
        }
        Event::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => {
            let summary = pi_render::tool_summary(tool_name, args);
            let args_pretty = serde_json::to_string_pretty(args).unwrap_or_default();
            sink.on_tool_row_changed(
                tool_call_id.clone(),
                tool_row("⚙", &summary, &args_pretty, None, true, String::new()),
            );
            tools.insert(
                tool_call_id.clone(),
                ToolRunState {
                    summary,
                    args_pretty,
                    started: Instant::now(),
                    last_flush: Instant::now(),
                },
            );
        }
        Event::ToolExecutionUpdate {
            tool_call_id,
            partial_result,
            ..
        } => {
            let Some(run) = tools.get_mut(tool_call_id) else {
                return;
            };
            if run.last_flush.elapsed() < TOOL_FLUSH {
                return;
            }
            run.last_flush = Instant::now();
            let output = pi_render::tail(
                &pi_rpc::content_text(partial_result),
                pi_render::TOOL_DETAIL_LIMIT,
            );
            sink.on_tool_row_changed(
                tool_call_id.clone(),
                tool_row(
                    "⚙",
                    &run.summary,
                    &run.args_pretty,
                    Some(&output),
                    true,
                    String::new(),
                ),
            );
        }
        Event::ToolExecutionEnd {
            tool_call_id,
            result,
            is_error,
            ..
        } => {
            let Some(run) = tools.remove(tool_call_id) else {
                return;
            };
            let output =
                pi_render::tail(&pi_rpc::content_text(result), pi_render::TOOL_DETAIL_LIMIT);
            let mark = if *is_error { "✗" } else { "✓" };
            sink.on_tool_row_changed(
                tool_call_id.clone(),
                tool_row(
                    mark,
                    &run.summary,
                    &run.args_pretty,
                    Some(&output),
                    false,
                    format_elapsed(run.started.elapsed()),
                ),
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RecordingSink;
    use serde_json::json;

    fn mk_thinking(event: AssistantMessageEvent) -> Event {
        Event::MessageUpdate {
            message: serde_json::Value::Null,
            assistant_message_event: event,
        }
    }

    #[test]
    fn format_elapsed_covers_all_three_branches() {
        assert_eq!(format_elapsed(Duration::from_millis(2500)), "2.5s");
        assert_eq!(format_elapsed(Duration::from_secs(45)), "45s");
        assert_eq!(format_elapsed(Duration::from_secs(125)), "2m 05s");
    }

    #[test]
    fn thinking_start_then_end_produces_running_then_finished_rows() {
        let sink = RecordingSink::default();
        let mut thinking = None;
        let mut tools = HashMap::new();
        let mut seq = 0;

        apply_live_preview(
            &mk_thinking(AssistantMessageEvent::ThinkingStart),
            &sink,
            &mut thinking,
            &mut tools,
            &mut seq,
        );
        assert!(thinking.is_some());
        apply_live_preview(
            &mk_thinking(AssistantMessageEvent::ThinkingEnd),
            &sink,
            &mut thinking,
            &mut tools,
            &mut seq,
        );
        assert!(thinking.is_none());

        let events = sink.events.lock().unwrap();
        assert_eq!(
            *events,
            vec![
                "thinking_row:thinking-1:running=true:text=".to_string(),
                "thinking_row:thinking-1:running=false:text=".to_string(),
            ]
        );
    }

    #[test]
    fn a_second_thinking_region_gets_a_fresh_id() {
        let sink = RecordingSink::default();
        let mut thinking = None;
        let mut tools = HashMap::new();
        let mut seq = 0;

        for _ in 0..2 {
            apply_live_preview(
                &mk_thinking(AssistantMessageEvent::ThinkingStart),
                &sink,
                &mut thinking,
                &mut tools,
                &mut seq,
            );
            apply_live_preview(
                &mk_thinking(AssistantMessageEvent::ThinkingEnd),
                &sink,
                &mut thinking,
                &mut tools,
                &mut seq,
            );
        }
        assert_eq!(seq, 2);
        let events = sink.events.lock().unwrap();
        assert!(events
            .iter()
            .any(|e| e.starts_with("thinking_row:thinking-1:")));
        assert!(events
            .iter()
            .any(|e| e.starts_with("thinking_row:thinking-2:")));
    }

    #[test]
    fn tool_lifecycle_produces_start_then_end_with_mark_and_elapsed() {
        let sink = RecordingSink::default();
        let mut thinking = None;
        let mut tools = HashMap::new();
        let mut seq = 0;

        apply_live_preview(
            &Event::ToolExecutionStart {
                tool_call_id: "call-1".to_string(),
                tool_name: "bash".to_string(),
                args: json!({"command": "echo hi"}),
            },
            &sink,
            &mut thinking,
            &mut tools,
            &mut seq,
        );
        assert!(tools.contains_key("call-1"));

        apply_live_preview(
            &Event::ToolExecutionEnd {
                tool_call_id: "call-1".to_string(),
                tool_name: "bash".to_string(),
                result: json!({"content": [{"type": "text", "text": "hi"}]}),
                is_error: false,
            },
            &sink,
            &mut thinking,
            &mut tools,
            &mut seq,
        );
        assert!(!tools.contains_key("call-1"));

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events[0].starts_with("tool_row:call-1:running=true:"));
        assert!(events[1].starts_with("tool_row:call-1:running=false:"));
    }

    #[test]
    fn tool_error_result_is_recorded() {
        let sink = RecordingSink::default();
        let mut thinking = None;
        let mut tools = HashMap::new();
        let mut seq = 0;

        apply_live_preview(
            &Event::ToolExecutionStart {
                tool_call_id: "call-1".to_string(),
                tool_name: "bash".to_string(),
                args: json!({"command": "false"}),
            },
            &sink,
            &mut thinking,
            &mut tools,
            &mut seq,
        );
        apply_live_preview(
            &Event::ToolExecutionEnd {
                tool_call_id: "call-1".to_string(),
                tool_name: "bash".to_string(),
                result: json!({"content": [{"type": "text", "text": "boom"}]}),
                is_error: true,
            },
            &sink,
            &mut thinking,
            &mut tools,
            &mut seq,
        );

        let events = sink.events.lock().unwrap();
        assert!(events[1].contains('✗'));
    }

    #[test]
    fn two_concurrent_tool_calls_do_not_clobber_each_other() {
        let sink = RecordingSink::default();
        let mut thinking = None;
        let mut tools = HashMap::new();
        let mut seq = 0;

        for id in ["call-a", "call-b"] {
            apply_live_preview(
                &Event::ToolExecutionStart {
                    tool_call_id: id.to_string(),
                    tool_name: "bash".to_string(),
                    args: json!({"command": id}),
                },
                &sink,
                &mut thinking,
                &mut tools,
                &mut seq,
            );
        }
        assert_eq!(tools.len(), 2);

        apply_live_preview(
            &Event::ToolExecutionEnd {
                tool_call_id: "call-a".to_string(),
                tool_name: "bash".to_string(),
                result: json!({"content": []}),
                is_error: false,
            },
            &sink,
            &mut thinking,
            &mut tools,
            &mut seq,
        );
        assert_eq!(tools.len(), 1);
        assert!(tools.contains_key("call-b"));
    }

    #[test]
    fn tool_update_before_the_flush_gate_elapses_is_dropped() {
        let sink = RecordingSink::default();
        let mut thinking = None;
        let mut tools = HashMap::new();
        let mut seq = 0;

        apply_live_preview(
            &Event::ToolExecutionStart {
                tool_call_id: "call-1".to_string(),
                tool_name: "bash".to_string(),
                args: json!({}),
            },
            &sink,
            &mut thinking,
            &mut tools,
            &mut seq,
        );
        apply_live_preview(
            &Event::ToolExecutionUpdate {
                tool_call_id: "call-1".to_string(),
                tool_name: "bash".to_string(),
                args: json!({}),
                partial_result: json!({"content": [{"type": "text", "text": "partial"}]}),
            },
            &sink,
            &mut thinking,
            &mut tools,
            &mut seq,
        );

        // Only the start event's push — the immediately-following update is
        // gated by TOOL_FLUSH (100ms), which hasn't elapsed yet.
        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1);
    }
}
