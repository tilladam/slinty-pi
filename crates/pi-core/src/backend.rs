//! Backend: owns the pi child process (or the demo synthesizer) and projects
//! agent events onto the transcript model.
//!
//! Threading contract: this module runs on tokio worker threads; every UI
//! touch goes through a [`UiSink`] implementation, which is responsible for
//! its own thread-hop to wherever UI state actually lives (the Slint event
//! loop, a Swift main actor, ...). Closures on the UI side are expected to
//! run in submission order, so the backend tracks a shadow row count to
//! address rows it appended without reading UI state back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use pi_rpc::{
    content_text, AssistantMessageEvent, Command, Event, ExtensionUiReply, ImageContent, PiClient,
    PiError, PiOptions, ThinkingLevel,
};

use crate::attach;
use crate::demo_sessions;
use crate::highlight;
use crate::local;
use crate::palette;
use crate::segmenter::{self, segment_markdown, Segment};

const TEXT_FLUSH: Duration = Duration::from_millis(33);
const TOOL_FLUSH: Duration = Duration::from_millis(100);
const TOOL_DETAIL_LIMIT: usize = 4000;

/// Commands from UI callbacks to the backend.
#[derive(Debug)]
pub enum UiCmd {
    Send(String),
    Abort,
    SetModel(usize),
    SetThinking(usize),
    /// Load a different session file within the *same* project (no respawn).
    SwitchSession(String),
    /// Change the working directory: the current child is aborted and
    /// killed, a new `pi --mode rpc` is spawned in the new cwd.
    SwitchProject(PathBuf),
    /// Start a fresh session in the current project (same child, no respawn).
    NewSession,
    /// Move a session file to the OS trash and refresh the sidebar. If it's
    /// the currently-open session, starts a new one so the child keeps
    /// working against a file that still exists.
    DeleteSession(String),
    /// Set the active session's display name (`set_session_name` has no
    /// path argument — it always applies to the session the single live
    /// child currently has open).
    RenameSession(String),
    /// Sidebar search box changed; re-filter the session list.
    SidebarSearch(String),
    /// Fetch and display the active session's branch tree.
    OpenTree,
    /// Fork the active session from a prior user message (by entry id).
    ForkFrom(String),
    /// Duplicate the active session's active branch into a new session file
    /// at the current point, and switch the live child to it.
    CloneSession,
    /// Build the full palette entry list (sessions + pi commands; app
    /// actions are static and added client-side) and rank it against an
    /// empty query. Execution is dispatched directly in `main.rs`, not
    /// through this channel — most palette actions are UI-only or already
    /// map onto an existing `UiCmd`.
    OpenPalette,
    /// Palette query box changed; re-rank the already-built entry list.
    PaletteQuery(String),
    /// Attach button (or, in principle, a future drop handler) picked a
    /// path. Images are read, base64-encoded, and queued for the next
    /// `Send`; everything else is appended to the composer as `@path`.
    AttachPath(PathBuf),
    /// Remove a queued image attachment by its chip index.
    RemoveAttachment(usize),
    /// Refresh and open the models panel (rapid-mlx detection state, running
    /// servers, cached-model catalog with fit labels).
    OpenModels,
    /// "serve" clicked on a cached rapid-mlx alias: (re)spawn a managed
    /// `rapid-mlx serve` child on the default port, then `set_model` once
    /// it's ready.
    ServeRapidMlxModel(String),
    /// "load" clicked on a router model id: `POST /models/load`, then poll
    /// `/models` until it settles (loaded/failed/timeout).
    LoadRouterModel(String),
    /// "unload" clicked on a router model id: `POST /models/unload`, then
    /// refresh.
    UnloadRouterModel(String),
    /// Hugging Face search box submitted (Enter): `GET
    /// https://huggingface.co/api/models?search=...&filter=gguf`.
    SearchHfModels(String),
    /// A quant chip clicked on an HF search result: `owner/repo:quant`,
    /// `POST /models` to start the download, then poll like
    /// `LoadRouterModel`.
    DownloadRouterModel(String),
    /// "add to pi" clicked on the Ollama section: writes every
    /// currently-detected Ollama model into `~/.pi/agent/models.json` under
    /// the canonical `ollama` provider preset.
    AddOllamaToPi,
    /// The status-bar server dot was clicked: restart a dead managed
    /// rapid-mlx server, or open the models panel when healthy.
    ServerDotClicked,
    /// "Save" clicked in the models panel's API-key section.
    SaveApiKey {
        provider: String,
        key: Secret,
    },
}

/// A credential in transit. `Debug` prints a redaction so `UiCmd`'s derive
/// (or any future `?cmd` log line) can never leak key material.
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(«redacted»)")
    }
}

// ---------------------------------------------------------------------------
// Row specs: plain, UI-toolkit-agnostic row descriptions pushed through a
// `UiSink`. Each frontend converts these to its own native row/view type
// (Slint: `RowSpec::to_row`-equivalent in slinty-pi; other toolkits do the
// same locally).
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
    fn note(kind: &'static str, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            ..Self::default()
        }
    }
}

// ---------------------------------------------------------------------------
// UiSink: the one trait a frontend implements to receive backend output.
// Every method is `&self` and fire-and-forget, mirroring the non-blocking
// `Weak::upgrade_in_event_loop` semantics slinty-pi's Slint implementation
// uses — an implementor queues the real UI mutation onto wherever its UI
// state actually lives and returns immediately. `Send + Sync` because the
// backend calls these from tokio worker threads.
// ---------------------------------------------------------------------------

pub trait UiSink: Send + Sync {
    fn push(&self, spec: RowSpec);
    /// Replace a row, preserving its user-toggled expansion state (a
    /// frontend concern the backend doesn't know about).
    fn set(&self, index: usize, spec: RowSpec);
    /// Append many rows in as few UI-thread hops as the implementor likes,
    /// so hydrating a large session stays responsive.
    fn push_all(&self, specs: Vec<RowSpec>);
    fn clear(&self);
    fn truncate(&self, len: usize);

    fn set_streaming(&self, streaming: bool);
    fn set_status(&self, status: String);
    fn set_context_percent(&self, percent: f32);
    fn set_queue(&self, items: Vec<(&'static str, String)>);
    fn set_models(&self, labels: Vec<String>, index: i32);
    fn set_server_dot(&self, state: i32);
    fn set_thinking(&self, labels: Vec<String>, index: i32);
    /// `labels`/`paths` are parallel arrays (projects other than the current
    /// one).
    fn set_projects(&self, labels: Vec<String>, paths: Vec<String>, current_name: String);
    /// `rows` are `(path, title, relative_time, active, cost)`.
    fn set_sidebar_sessions(&self, rows: Vec<(String, String, String, bool, String)>);
    /// `rows` are `(id, depth, summary, label, can_fork, active)`.
    fn set_tree(&self, rows: Vec<(String, i32, String, String, bool, bool)>);
    /// Rapid-mlx section only. Deliberately separate from
    /// [`Self::set_router_panel`]: collecting rapid-mlx state spawns the CLI
    /// a handful of times, which the router's load/unload poll loop must not
    /// repeat on every tick.
    fn set_rapid_mlx_panel(&self, data: RapidMlxPanelData);
    /// llama.cpp router section only — see [`Self::set_rapid_mlx_panel`]'s
    /// doc comment for why the two are separate setters.
    fn set_router_panel(&self, data: RouterPanelData);
    fn show_models_panel(&self);
    /// `results` rows are `(id, gated, downloads, quants)`.
    fn set_hf_search_results(&self, results: Vec<(String, bool, i32, Vec<String>)>);
    /// A pre-formatted "N model(s): a, b, c" string (or empty when not
    /// detected) — built in Rust rather than exposing a raw row list, since
    /// this section is a one-click "add all" affordance, not a browsable
    /// list like the router/rapid-mlx sections.
    fn set_auth_entries(&self, labels: Vec<String>);
    fn set_ollama_panel(&self, detected: bool, summary: String, model_count: i32);
    fn set_palette_entries(&self, entries: Vec<palette::PaletteEntry>);
    /// Prefill the composer (used after a fork, which hands back the text of
    /// the message it forked from instead of keeping it in context).
    fn set_composer_text(&self, text: String);
    /// Append `@path` to the composer (non-image attachment); the
    /// implementor owns the spacing, since it has the current text and this
    /// doesn't.
    fn append_composer_text(&self, path: &Path);
    /// Chip labels (file names) for queued image attachments.
    fn set_pending_attachments(&self, names: Vec<String>);
}

/// UI-ready rapid-mlx panel data. `cached` rows are `(alias, hf_repo,
/// human_size, fit_label)`.
#[derive(Debug, Clone, PartialEq)]
pub struct RapidMlxPanelData {
    pub version: Option<String>,
    pub running_summary: Option<String>,
    pub cached: Vec<(String, String, String, String)>,
    pub catalog_count: usize,
}

/// UI-ready router panel data. `models` rows are `(id, status_label, loaded,
/// busy)`.
#[derive(Debug, Clone, PartialEq)]
pub struct RouterPanelData {
    pub status_label: String,
    pub base_url: String,
    pub models: Vec<(String, String, bool, bool)>,
}

// ---------------------------------------------------------------------------
// Transcript projection
// ---------------------------------------------------------------------------

struct StreamRegion {
    start: usize,
    buffer: String,
    prev: Vec<Segment>,
    last_flush: Instant,
    first: bool,
}

struct ThinkingRegion {
    row: usize,
    buffer: String,
    last_flush: Instant,
    first: bool,
}

struct ToolRun {
    row: usize,
    name: String,
    summary: String,
    args_pretty: String,
    started: Instant,
    last_flush: Instant,
}

pub struct Transcript {
    ui: Box<dyn UiSink>,
    dark: Arc<AtomicBool>,
    /// Shadow count of rows pushed so far, mirroring what the UI side will
    /// end up with once its (asynchronous, submission-ordered) queue drains
    /// — lets the backend address a row it just pushed without reading UI
    /// state back. Owned here rather than by the `UiSink` implementor: it's
    /// plain backend bookkeeping, not a UI-toolkit concern.
    rows: usize,
    stream: Option<StreamRegion>,
    thinking: Option<ThinkingRegion>,
    tools: HashMap<String, ToolRun>,
    /// The next content row starts a new visual message group.
    pending_first: bool,
}

impl Transcript {
    pub fn new(ui: Box<dyn UiSink>, dark: Arc<AtomicBool>) -> Self {
        Self {
            ui,
            dark,
            rows: 0,
            stream: None,
            thinking: None,
            tools: HashMap::new(),
            pending_first: false,
        }
    }

    /// Assign the next shadow row index, push the row, and hand the index
    /// back — the index is precomputed rather than read from the `UiSink`
    /// call because the whole point of the shadow counter is to avoid
    /// needing a synchronous readback from (possibly async) UI state.
    fn push_row(&mut self, spec: RowSpec) -> usize {
        let index = self.rows;
        self.rows += 1;
        self.ui.push(spec);
        index
    }

    fn push_all_rows(&mut self, specs: Vec<RowSpec>) {
        self.rows += specs.len();
        self.ui.push_all(specs);
    }

    /// Drop all live-stream/tool state and clear the transcript, in
    /// preparation for hydrating a different session.
    pub fn reset(&mut self) {
        self.stream = None;
        self.thinking = None;
        self.tools.clear();
        self.pending_first = false;
        self.rows = 0;
        self.ui.clear();
    }

    /// Render a session's full message history (as returned by `get_messages`
    /// after a `switch_session`) without replaying events. Appends to
    /// whatever is already in the transcript — call [`Transcript::reset`]
    /// first when switching sessions.
    pub fn hydrate(&mut self, messages: &[serde_json::Value]) {
        let dark = self.dark.load(Ordering::Relaxed);
        let specs = hydrate_rowspecs(messages, dark);
        self.push_all_rows(specs);
    }

    pub fn user_prompt(&mut self, text: &str, steering: bool) {
        let display = if steering {
            format!("↪ {text}")
        } else {
            text.to_string()
        };
        let mut spec = RowSpec::note("user", display.clone());
        spec.first = true;
        spec.raw = display;
        self.push_row(spec);
        self.pending_first = true;
    }

    pub fn note(&mut self, kind: &'static str, text: impl Into<String>) {
        self.push_row(RowSpec::note(kind, text));
    }

    fn flush_stream(&mut self, force: bool) {
        let Some(region) = self.stream.take() else {
            return;
        };
        let mut region = region;
        if !force && region.last_flush.elapsed() < TEXT_FLUSH {
            self.stream = Some(region);
            return;
        }
        let dark = self.dark.load(Ordering::Relaxed);
        let segments = segment_markdown(&region.buffer);
        for (i, segment) in segments.iter().enumerate() {
            if region.prev.get(i) == Some(segment) {
                continue;
            }
            let spec = spec_for_segment(segment, region.first && i == 0, dark, &region.buffer);
            let index = region.start + i;
            if index < self.rows {
                self.ui.set(index, spec);
            } else {
                self.push_row(spec);
            }
        }
        if segments.len() < region.prev.len() {
            self.rows = region.start + segments.len();
            self.ui.truncate(self.rows);
        }
        region.prev = segments;
        region.last_flush = Instant::now();
        self.stream = Some(region);
    }

    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::AgentStart => self.ui.set_streaming(true),
            Event::AgentSettled => {
                self.finish_stream();
                self.finish_thinking();
                self.ui.set_streaming(false);
            }
            Event::MessageStart { message } => {
                if message.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                    self.pending_first = true;
                }
            }
            Event::MessageUpdate {
                assistant_message_event,
                ..
            } => self.apply_delta(assistant_message_event),
            Event::MessageEnd { .. } => {
                self.finish_stream();
                self.finish_thinking();
            }
            Event::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => self.tool_start(tool_call_id, tool_name, args),
            Event::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                ..
            } => self.tool_update(tool_call_id, partial_result),
            Event::ToolExecutionEnd {
                tool_call_id,
                result,
                is_error,
                ..
            } => self.tool_end(tool_call_id, result, *is_error),
            Event::QueueUpdate {
                steering,
                follow_up,
            } => {
                let mut items: Vec<(&'static str, String)> = Vec::new();
                items.extend(steering.iter().map(|s| ("steer", s.clone())));
                items.extend(follow_up.iter().map(|s| ("follow-up", s.clone())));
                self.ui.set_queue(items);
            }
            Event::CompactionStart { .. } => self.ui.set_status("compacting context…".into()),
            Event::CompactionEnd { .. } => self.ui.set_status(String::new()),
            Event::AutoRetryStart {
                attempt,
                max_attempts,
                ..
            } => self
                .ui
                .set_status(format!("retrying ({attempt}/{max_attempts})…")),
            Event::ExtensionError { error, .. } => {
                self.note("error", format!("extension error: {error}"));
            }
            _ => {}
        }
    }

    fn apply_delta(&mut self, delta: &AssistantMessageEvent) {
        match delta {
            AssistantMessageEvent::TextStart { .. } => {
                self.finish_thinking();
                self.stream = Some(StreamRegion {
                    start: self.rows,
                    buffer: String::new(),
                    prev: Vec::new(),
                    last_flush: Instant::now() - TEXT_FLUSH,
                    first: std::mem::take(&mut self.pending_first),
                });
            }
            AssistantMessageEvent::TextDelta { delta, .. } => {
                if let Some(region) = self.stream.as_mut() {
                    region.buffer.push_str(delta);
                    self.flush_stream(false);
                }
            }
            AssistantMessageEvent::TextEnd { content, .. } => {
                if let Some(region) = self.stream.as_mut() {
                    if !content.is_empty() {
                        region.buffer = content.clone();
                    }
                }
                self.finish_stream();
            }
            AssistantMessageEvent::ThinkingStart => {
                let first = std::mem::take(&mut self.pending_first);
                let row = self.push_row(RowSpec {
                    kind: "thinking",
                    running: true,
                    first,
                    ..RowSpec::default()
                });
                self.thinking = Some(ThinkingRegion {
                    row,
                    buffer: String::new(),
                    last_flush: Instant::now(),
                    first,
                });
            }
            AssistantMessageEvent::ThinkingDelta { delta } => {
                if let Some(region) = self.thinking.as_mut() {
                    region.buffer.push_str(delta);
                    if region.last_flush.elapsed() >= TEXT_FLUSH {
                        region.last_flush = Instant::now();
                        let spec = RowSpec {
                            kind: "thinking",
                            text: region.buffer.clone(),
                            running: true,
                            first: region.first,
                            ..RowSpec::default()
                        };
                        self.ui.set(region.row, spec);
                    }
                }
            }
            AssistantMessageEvent::ThinkingEnd => self.finish_thinking(),
            AssistantMessageEvent::Error { reason } if reason != "aborted" => {
                self.note("error", format!("model error: {reason}"));
            }
            _ => {}
        }
    }

    fn finish_stream(&mut self) {
        self.flush_stream(true);
        self.stream = None;
    }

    fn finish_thinking(&mut self) {
        if let Some(region) = self.thinking.take() {
            let spec = RowSpec {
                kind: "thinking",
                text: region.buffer,
                running: false,
                first: region.first,
                ..RowSpec::default()
            };
            self.ui.set(region.row, spec);
        }
    }

    fn tool_start(&mut self, id: &str, name: &str, args: &serde_json::Value) {
        let summary = tool_summary(name, args);
        let args_pretty = serde_json::to_string_pretty(args).unwrap_or_default();
        let first = std::mem::take(&mut self.pending_first);
        let row = self.push_row(RowSpec {
            kind: "tool",
            text: format!("⚙ {summary}"),
            detail: args_pretty.clone(),
            running: true,
            first,
            ..RowSpec::default()
        });
        self.tools.insert(
            id.to_string(),
            ToolRun {
                row,
                name: name.to_string(),
                summary,
                args_pretty,
                started: Instant::now(),
                last_flush: Instant::now(),
            },
        );
    }

    fn tool_update(&mut self, id: &str, partial: &serde_json::Value) {
        let Some(run) = self.tools.get_mut(id) else {
            return;
        };
        if run.last_flush.elapsed() < TOOL_FLUSH {
            return;
        }
        run.last_flush = Instant::now();
        let output = tail(&content_text(partial), TOOL_DETAIL_LIMIT);
        let spec = RowSpec {
            kind: "tool",
            text: format!("⚙ {}", run.summary),
            detail: format!("{}\n───\n{output}", run.args_pretty),
            running: true,
            ..RowSpec::default()
        };
        self.ui.set(run.row, spec);
    }

    fn tool_end(&mut self, id: &str, result: &serde_json::Value, is_error: bool) {
        let Some(run) = self.tools.remove(id) else {
            return;
        };
        let output = tail(&content_text(result), TOOL_DETAIL_LIMIT);
        let mark = if is_error { "✗" } else { "✓" };
        let elapsed = format_elapsed(run.started.elapsed());
        let spec = RowSpec {
            kind: "tool",
            text: format!("{mark} {}", run.summary),
            detail: if output.is_empty() {
                run.args_pretty
            } else {
                format!("{}\n───\n{output}", run.args_pretty)
            },
            elapsed,
            ..RowSpec::default()
        };
        let _ = run.name;
        self.ui.set(run.row, spec);
    }
}

#[cfg(test)]
mod transcript_tests {
    use super::*;
    use crate::recording_ui_sink::{RecordingUiSink, UiEvent};

    fn new_transcript() -> (Transcript, RecordingUiSink) {
        let sink = RecordingUiSink::new();
        let dark = Arc::new(AtomicBool::new(false));
        let transcript = Transcript::new(Box::new(sink.clone()), dark);
        (transcript, sink)
    }

    /// Unwrap a row event's `RowSpec`, panicking with a useful message on any
    /// non-row event (`set_status`, `set_streaming`, ...).
    fn row_spec(event: &UiEvent) -> &RowSpec {
        match event {
            UiEvent::Push(spec) | UiEvent::Set(_, spec) => spec,
            other => panic!("expected a row event, got {other:?}"),
        }
    }

    #[test]
    fn agent_start_and_settled_toggle_streaming() {
        let (mut transcript, sink) = new_transcript();
        transcript.apply(&Event::AgentStart);
        transcript.apply(&Event::AgentSettled);
        assert_eq!(
            sink.events(),
            vec![UiEvent::SetStreaming(true), UiEvent::SetStreaming(false),]
        );
    }

    #[test]
    fn streaming_text_ends_as_a_single_first_prose_row() {
        let (mut transcript, sink) = new_transcript();
        transcript.apply(&Event::MessageStart {
            message: serde_json::json!({"role": "assistant"}),
        });
        transcript.apply(&mk_delta(AssistantMessageEvent::TextStart {
            content_index: 0,
        }));
        transcript.apply(&mk_delta(AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "hel".to_string(),
        }));
        transcript.apply(&mk_delta(AssistantMessageEvent::TextEnd {
            content_index: 0,
            content: "hello world".to_string(),
        }));

        let rows = sink.rows();
        let last = row_spec(rows.last().expect("at least one row event"));
        assert_eq!(last.kind, "prose");
        assert_eq!(last.markdown.as_deref(), Some("hello world"));
        assert!(
            last.first,
            "first content row of a fresh assistant message starts a new group"
        );
        // Every row event before the last one addressed the same shadow
        // index (0) — the coalesced-delta path only ever grows the
        // transcript by one row for a single unbroken text stream.
        for event in &rows {
            match event {
                UiEvent::Push(_) => {}
                UiEvent::Set(index, _) => assert_eq!(*index, 0),
                other => panic!("unexpected row event {other:?}"),
            }
        }
    }

    #[test]
    fn thinking_block_collapses_to_one_not_running_row() {
        let (mut transcript, sink) = new_transcript();
        transcript.apply(&Event::MessageStart {
            message: serde_json::json!({"role": "assistant"}),
        });
        transcript.apply(&mk_delta(AssistantMessageEvent::ThinkingStart));
        transcript.apply(&mk_delta(AssistantMessageEvent::ThinkingDelta {
            delta: "pondering".to_string(),
        }));
        transcript.apply(&mk_delta(AssistantMessageEvent::ThinkingEnd));

        let rows = sink.rows();
        let first_push = rows
            .iter()
            .find_map(|e| match e {
                UiEvent::Push(spec) => Some(spec),
                _ => None,
            })
            .expect("thinking start pushes a row immediately (so later deltas can address it)");
        assert_eq!(first_push.kind, "thinking");
        assert!(first_push.running, "still running when first pushed");
        assert!(first_push.first);

        let last = row_spec(rows.last().expect("at least one row event"));
        assert_eq!(last.kind, "thinking");
        assert_eq!(last.text, "pondering");
        assert!(!last.running, "ThinkingEnd marks the row finished");
    }

    #[test]
    fn tool_call_lifecycle_updates_the_same_row_from_running_to_done() {
        let (mut transcript, sink) = new_transcript();
        transcript.apply(&Event::ToolExecutionStart {
            tool_call_id: "call_1".to_string(),
            tool_name: "bash".to_string(),
            args: serde_json::json!({"command": "cargo test"}),
        });
        transcript.apply(&Event::ToolExecutionEnd {
            tool_call_id: "call_1".to_string(),
            tool_name: "bash".to_string(),
            result: serde_json::json!({"content": [{"type": "text", "text": "ok"}]}),
            is_error: false,
        });

        let rows = sink.rows();
        assert_eq!(rows.len(), 2, "one push (start) + one set (end)");
        let UiEvent::Push(start) = &rows[0] else {
            panic!("expected the first event to be a push, got {:?}", rows[0]);
        };
        assert_eq!(start.kind, "tool");
        assert!(start.running);
        assert!(start.text.contains("bash"));

        let UiEvent::Set(index, end) = &rows[1] else {
            panic!("expected the second event to be a set, got {:?}", rows[1]);
        };
        assert_eq!(
            *index, 0,
            "the tool result updates the row the start pushed"
        );
        assert!(!end.running);
        assert!(end.text.starts_with('✓'), "text was {:?}", end.text);
        assert!(end.detail.contains("ok"));
    }

    #[test]
    fn tool_error_result_marks_the_row_failed_not_just_finished() {
        let (mut transcript, sink) = new_transcript();
        transcript.apply(&Event::ToolExecutionStart {
            tool_call_id: "call_1".to_string(),
            tool_name: "bash".to_string(),
            args: serde_json::json!({"command": "false"}),
        });
        transcript.apply(&Event::ToolExecutionEnd {
            tool_call_id: "call_1".to_string(),
            tool_name: "bash".to_string(),
            result: serde_json::json!({"content": [{"type": "text", "text": "boom"}]}),
            is_error: true,
        });

        let rows = sink.rows();
        let end = row_spec(&rows[1]);
        assert!(end.text.starts_with('✗'), "text was {:?}", end.text);
    }

    #[test]
    fn compaction_start_and_end_set_and_clear_the_status_text() {
        let (mut transcript, sink) = new_transcript();
        transcript.apply(&Event::CompactionStart {
            reason: "context_limit".to_string(),
        });
        transcript.apply(&Event::CompactionEnd {
            reason: "context_limit".to_string(),
            result: serde_json::Value::Null,
            aborted: false,
            will_retry: false,
            error_message: None,
        });

        assert_eq!(
            sink.events(),
            vec![
                UiEvent::SetStatus("compacting context…".to_string()),
                UiEvent::SetStatus(String::new()),
            ]
        );
    }

    #[test]
    fn queue_update_reports_steering_then_follow_up_items_in_order() {
        let (mut transcript, sink) = new_transcript();
        transcript.apply(&Event::QueueUpdate {
            steering: vec!["steer this".to_string()],
            follow_up: vec!["then this".to_string(), "and this".to_string()],
        });

        assert_eq!(
            sink.events(),
            vec![UiEvent::SetQueue(vec![
                ("steer", "steer this".to_string()),
                ("follow-up", "then this".to_string()),
                ("follow-up", "and this".to_string()),
            ])]
        );
    }

    #[test]
    fn extension_error_becomes_an_error_note_row() {
        let (mut transcript, sink) = new_transcript();
        transcript.apply(&Event::ExtensionError {
            extension_path: "/ext/foo.js".to_string(),
            event: "tool_call".to_string(),
            error: "boom".to_string(),
        });

        let rows = sink.rows();
        let spec = row_spec(&rows[0]);
        assert_eq!(spec.kind, "error");
        assert!(spec.text.contains("boom"));
    }

    #[test]
    fn reset_clears_the_ui_and_the_shadow_row_counter() {
        let (mut transcript, sink) = new_transcript();
        transcript.note("info", "one");
        transcript.note("info", "two");
        transcript.reset();
        // After reset, the shadow counter is back to 0 — the next pushed row
        // must be addressable at index 0 again, not 2.
        transcript.note("info", "three");

        let rows = sink.rows();
        assert!(matches!(rows[0], UiEvent::Push(_)));
        assert!(matches!(rows[1], UiEvent::Push(_)));
        assert!(matches!(rows[2], UiEvent::Clear));
        let UiEvent::Push(third) = &rows[3] else {
            panic!("expected a push, got {:?}", rows[3]);
        };
        assert_eq!(third.text, "three");
    }
}

/// Shared by the live streaming path (`flush_stream`) and hydration
/// (`hydrate_rowspecs`): a markdown [`Segment`] always maps to the same row,
/// whether it arrived as deltas or as a complete historical message.
fn spec_for_segment(segment: &Segment, first: bool, dark: bool, raw: &str) -> RowSpec {
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

fn tool_summary(tool_name: &str, args: &serde_json::Value) -> String {
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

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

fn tail(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    let mut start = s.len() - limit;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &s[start..])
}

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

// ---------------------------------------------------------------------------
// Hydration: turn a `get_messages` payload (`AgentMessage[]`, per
// docs/session-format.md) into RowSpecs. Same building blocks as the live
// streaming path (`spec_for_segment`, `tool_summary`, `content_text`),
// applied to complete historical messages instead of deltas — so a resumed
// transcript and a freshly-streamed one render identically.
// ---------------------------------------------------------------------------

fn hydrate_rowspecs(messages: &[serde_json::Value], dark: bool) -> Vec<RowSpec> {
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
                for block in blocks {
                    match block.get("type").and_then(|v| v.as_str()) {
                        Some("thinking") => {
                            let thinking =
                                block.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                            let first = std::mem::take(&mut pending_first);
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
                            specs.push(RowSpec {
                                kind: "tool",
                                text: format!("⚙ {summary}"),
                                detail: args_pretty,
                                running: true,
                                first: std::mem::take(&mut pending_first),
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
fn user_content_text(content: &serde_json::Value) -> (String, usize) {
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

// ---------------------------------------------------------------------------
// pi backend
// ---------------------------------------------------------------------------

struct ModelEntry {
    provider: String,
    id: String,
    /// pi's `baseUrl` for this model's provider; "" when absent.
    base_url: String,
    /// Whether `base_url` points at this machine (localhost et al.).
    is_local: bool,
}

/// The composer model picker's state, rebuilt by `refresh_models`: wire
/// entries for `set_model` (indexed the same as the picker), the display
/// labels (shared with the palette's "load model" entries), and the index
/// of pi's currently-selected model (-1 when unknown).
struct ModelsState {
    entries: Vec<ModelEntry>,
    labels: Vec<String>,
    current: i32,
}

impl ModelsState {
    fn get(&self, i: usize) -> Option<&ModelEntry> {
        self.entries.get(i)
    }

    fn current_entry(&self) -> Option<&ModelEntry> {
        usize::try_from(self.current)
            .ok()
            .and_then(|i| self.entries.get(i))
    }
}

/// A `rapid-mlx serve` child this app spawned, plus the alias it serves —
/// the alias is what the status dot's one-click restart re-serves after the
/// child dies.
struct ManagedRapidMlx {
    alias: String,
    server: local::rapid_mlx::ManagedServer,
}

// Status-bar server dot states (mirrored by `server-dot` in app.slint).
const SERVER_DOT_HIDDEN: i32 = 0;
const SERVER_DOT_OK: i32 = 1;
const SERVER_DOT_DOWN: i32 = 2;
/// The server answers, but it's serving a *different* model than pi's
/// current one — rapid-mlx 404s every completion in that state, so a plain
/// green would be a lie.
const SERVER_DOT_MISMATCH: i32 = 3;

/// One `pi --mode rpc` child's lifetime ends either because the app is
/// closing, or because the user switched projects and needs a new child
/// spawned in the new cwd (`switch_session` only works within a cwd).
enum SessionOutcome {
    SwitchProject(PathBuf),
    Exit,
}

/// Sidebar state that outlives any one child process: the session-metadata
/// cache is worth keeping warm across a project switch (a project you switch
/// back to re-hits it), and the current project/search query need to survive
/// the respawn that a project switch triggers.
struct Sidebar {
    sessions_root: Option<PathBuf>,
    meta_cache: pi_sessions::MetaCache,
    cwd: Option<PathBuf>,
    query: String,
    /// Parallel to what's pushed as `project-paths`; not read back, just
    /// documents that Slint (not Rust) resolves a picked label to a path.
    other_projects: Vec<pi_sessions::Project>,
}

impl Sidebar {
    fn new() -> Self {
        Self {
            sessions_root: pi_sessions::default_sessions_root(),
            meta_cache: pi_sessions::MetaCache::new(),
            cwd: None,
            query: String::new(),
            other_projects: Vec::new(),
        }
    }

    fn session_dir(&self) -> Option<PathBuf> {
        Some(pi_sessions::project_session_dir(
            self.sessions_root.as_ref()?,
            self.cwd.as_ref()?,
        ))
    }

    fn refresh_projects(&mut self, ui: &dyn UiSink) {
        let Some(root) = self.sessions_root.clone() else {
            return;
        };
        let cwd_display = self.cwd.as_ref().map(|c| c.display().to_string());
        let all = pi_sessions::list_projects(&root).unwrap_or_default();
        self.other_projects = all
            .into_iter()
            .filter(|p| Some(&p.display_path) != cwd_display.as_ref())
            .collect();
        let labels: Vec<String> = self
            .other_projects
            .iter()
            .map(|p| p.display_path.clone())
            .collect();
        let paths = labels.clone();
        let current_name = self
            .cwd
            .as_ref()
            .and_then(|c| c.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string());
        ui.set_projects(labels, paths, current_name);
    }

    async fn refresh_sessions(&self, client: &PiClient, ui: &dyn UiSink) {
        let active = active_session_path(client).await;
        self.refresh_sessions_with_active(active.as_deref(), ui);
    }

    /// The synchronous part of `refresh_sessions`, split out so demo mode
    /// (no `PiClient` to ask `get_state` for the active session) can drive
    /// it with a locally-tracked path instead.
    fn refresh_sessions_with_active(&self, active: Option<&str>, ui: &dyn UiSink) {
        let Some(dir) = self.session_dir() else {
            ui.set_sidebar_sessions(Vec::new());
            return;
        };
        let all = self.meta_cache.list_sessions(&dir).unwrap_or_default();
        let filtered: Vec<&pi_sessions::SessionMeta> = if self.query.is_empty() {
            all.iter().collect()
        } else {
            pi_sessions::search(&all, &self.query)
        };
        let mut rows: Vec<(String, String, String, bool, String)> = filtered
            .into_iter()
            .map(|m| {
                let path = m.path.to_string_lossy().into_owned();
                let is_active = active == Some(path.as_str());
                (
                    path,
                    m.title().to_string(),
                    relative_time(&m.last_timestamp),
                    is_active,
                    format_cost(m.total_cost),
                )
            })
            .collect();

        // pi doesn't write a session's file until its first message, so a
        // just-switched-to project's brand-new (still empty) session has no
        // file for `meta_cache` to find yet — without this, the sidebar
        // would show every *other* session but not the one you're actually
        // in, until you send a message (which finally creates the file) or
        // switch away and back (which happens to land on a now-existing
        // file). Synthesize its row from what pi already told us via
        // `get_state`, so "where you are" is never silently missing.
        if let Some(active_path) = active {
            if self.query.is_empty() && !rows.iter().any(|(path, ..)| path == active_path) {
                rows.insert(
                    0,
                    (
                        active_path.to_string(),
                        "New session".to_string(),
                        "just now".to_string(),
                        true,
                        String::new(),
                    ),
                );
            }
        }

        ui.set_sidebar_sessions(rows);
    }
}

/// pi's `sessionFile` from `get_state`, or `None` if it can't be fetched.
async fn active_session_path(client: &PiClient) -> Option<String> {
    client
        .get_state()
        .await
        .ok()?
        .get("sessionFile")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Sidebar cost label, e.g. "$0.0231". `""` (rendered as no label at all)
/// below half a cent — mainly for local models, where cost is always 0.
fn format_cost(total_cost: f64) -> String {
    if total_cost < 0.005 {
        String::new()
    } else {
        format!("${total_cost:.2}")
    }
}

fn relative_time(iso_timestamp: &str) -> String {
    let Ok(then) = chrono::DateTime::parse_from_rfc3339(iso_timestamp) else {
        return String::new();
    };
    let secs = chrono::Utc::now()
        .signed_duration_since(then.with_timezone(&chrono::Utc))
        .num_seconds();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else if secs < 86400 * 7 {
        format!("{}d", secs / 86400)
    } else {
        format!("{}w", secs / (86400 * 7))
    }
}

#[cfg(test)]
mod format_cost_tests {
    use super::format_cost;

    #[test]
    fn zero_and_near_zero_cost_yields_empty_label() {
        assert_eq!(format_cost(0.0), "");
        assert_eq!(format_cost(0.001), "");
    }

    #[test]
    fn non_trivial_cost_is_formatted_as_dollars() {
        assert_eq!(format_cost(0.0231), "$0.02");
        assert_eq!(format_cost(1.5), "$1.50");
    }
}

#[cfg(test)]
mod relative_time_tests {
    use super::relative_time;
    use chrono::{Duration, Utc};

    fn iso(ago: Duration) -> String {
        (Utc::now() - ago).to_rfc3339()
    }

    #[test]
    fn buckets_by_magnitude() {
        assert_eq!(relative_time(&iso(Duration::seconds(10))), "just now");
        assert_eq!(relative_time(&iso(Duration::minutes(5))), "5m");
        assert_eq!(relative_time(&iso(Duration::hours(3))), "3h");
        assert_eq!(relative_time(&iso(Duration::days(2))), "2d");
        assert_eq!(relative_time(&iso(Duration::days(15))), "2w");
    }

    #[test]
    fn unparseable_timestamp_yields_empty_string() {
        assert_eq!(relative_time("not a timestamp"), "");
    }
}

pub async fn pi_backend(
    ui: Box<dyn UiSink>,
    dark: Arc<AtomicBool>,
    mut cmd_rx: mpsc::UnboundedReceiver<UiCmd>,
) {
    let mut transcript = Transcript::new(ui, dark);
    let mut sidebar = Sidebar::new();
    // Started once for the app's lifetime: the sessions root doesn't change
    // across project switches, only `cwd` does.
    let mut sessions_changed = spawn_sessions_watcher(sidebar.sessions_root.as_deref());
    let mut cwd = std::env::current_dir().ok();
    // Resuming a *specific* session on startup (as opposed to just the
    // latest one, see `continue_on_first_spawn` below) is reachable for
    // testing/scripting via env var, the same way `SLINTY_DEMO*` gates the
    // demo backend. Only applies to the very first child: it names a
    // session under the *initial* cwd, which a later project switch would
    // leave behind.
    let mut resume_on_first_spawn = std::env::var("SLINTY_RESUME_SESSION").ok();
    // Never lose work: land back in the project's most recent session
    // (pi's own `--continue`) rather than a blank one, unless a specific
    // session was requested instead. One-shot like `resume_on_first_spawn`
    // — later project switches start fresh (the sidebar/palette are how you
    // reach a specific other session at that point).
    let mut continue_on_first_spawn = resume_on_first_spawn.is_none();
    // Independent of pi's session/project lifecycle (switching sessions or
    // projects shouldn't kill a running local model server), so it lives at
    // this outer scope rather than inside `run_session`.
    let mut managed_rapid_mlx: Option<ManagedRapidMlx> = None;

    loop {
        sidebar.cwd = cwd.clone();
        sidebar.query.clear();

        let opts = PiOptions {
            cwd: cwd.clone(),
            extra_args: if continue_on_first_spawn {
                vec!["--continue".to_string()]
            } else {
                vec![]
            },
            ..Default::default()
        };
        let (client, events) = match PiClient::spawn(opts).await {
            Ok(pair) => pair,
            Err(e) => {
                let where_ = cwd
                    .as_ref()
                    .map(|c| format!(" in {}", c.display()))
                    .unwrap_or_default();
                transcript.note(
                    "error",
                    format!(
                        "Failed to start pi{where_}: {e}\nIs `pi` on your PATH? \
                         Install: npm install -g @earendil-works/pi-coding-agent"
                    ),
                );
                match wait_for_project_switch(&mut cmd_rx).await {
                    Some(path) => {
                        cwd = Some(path);
                        continue;
                    }
                    None => return,
                }
            }
        };

        continue_on_first_spawn = false;
        if let Some(path) = resume_on_first_spawn.take() {
            resume_session(&client, &mut transcript, &path).await;
        }

        sidebar.refresh_projects(transcript.ui.as_ref());
        sidebar
            .refresh_sessions(&client, transcript.ui.as_ref())
            .await;

        match run_session(
            &client,
            events,
            &mut cmd_rx,
            &mut sessions_changed,
            &mut transcript,
            &mut sidebar,
            &mut managed_rapid_mlx,
        )
        .await
        {
            SessionOutcome::SwitchProject(path) => {
                cwd = Some(path);
            }
            SessionOutcome::Exit => return,
        }
        // `client` drops here; `kill_on_drop` reaps the old child before the
        // next loop iteration spawns its replacement.
    }
}

/// Bridges `pi_sessions::watch`'s blocking `std::sync::mpsc` channel onto the
/// tokio runtime, so `run_session`'s `select!` can treat "something changed
/// on disk" like any other event source (a dedicated OS thread just forwards
/// each signal). Returns a receiver that never fires if `root` is `None` or
/// the watcher fails to start (e.g. exhausted OS watch handles) — sessions
/// still refresh on every UI-driven action, so this is a liveness nicety,
/// not a correctness requirement.
fn spawn_sessions_watcher(root: Option<&Path>) -> mpsc::UnboundedReceiver<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    let Some(root) = root.map(Path::to_path_buf) else {
        return rx;
    };
    std::thread::spawn(move || {
        let Some(watcher) = pi_sessions::watch(&root) else {
            tracing::warn!(
                root = %root.display(),
                "could not start a sessions-directory watcher; sidebar won't auto-refresh on sessions created by other processes"
            );
            return;
        };
        while watcher.changed.recv().is_ok() {
            if tx.send(()).is_err() {
                break;
            }
        }
    });
    rx
}

/// Drain commands while there is no running child (e.g. spawn failed),
/// looking for a project switch to retry with. Anything else sent in this
/// state (there's no agent to send it to) is dropped.
async fn wait_for_project_switch(cmd_rx: &mut mpsc::UnboundedReceiver<UiCmd>) -> Option<PathBuf> {
    while let Some(cmd) = cmd_rx.recv().await {
        if let UiCmd::SwitchProject(path) = cmd {
            return Some(path);
        }
    }
    None
}

/// Run one `pi --mode rpc` child to completion: either the app is closing
/// (`SessionOutcome::Exit`) or the user asked to switch projects, which this
/// child can't do for itself (`SessionOutcome::SwitchProject`).
async fn run_session(
    client: &PiClient,
    mut events: mpsc::UnboundedReceiver<Event>,
    cmd_rx: &mut mpsc::UnboundedReceiver<UiCmd>,
    sessions_changed: &mut mpsc::UnboundedReceiver<()>,
    transcript: &mut Transcript,
    sidebar: &mut Sidebar,
    managed_rapid_mlx: &mut Option<ManagedRapidMlx>,
) -> SessionOutcome {
    let mut models = refresh_models(client, transcript).await;
    let mut thinking_levels = refresh_thinking(client, transcript).await;
    let mut streaming = false;
    let mut palette_entries: Vec<palette::PaletteEntry> = Vec::new();
    // Status-bar server dot: recomputed every tick (and immediately after
    // model/server changes via `reset_immediately`), pushed to the UI only
    // on change.
    let mut server_dot = SERVER_DOT_HIDDEN;
    let mut dot_interval = tokio::time::interval(Duration::from_secs(5));
    dot_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // (display name, encoded image) pairs queued for the next non-streaming
    // `Send`.
    let mut pending_images: Vec<(String, ImageContent)> = Vec::new();
    // Guards the `sessions_changed` branch below: an unbounded receiver keeps
    // returning `Ready(None)` forever once its sender drops, which would
    // otherwise spin the select loop hot.
    let mut watcher_alive = true;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return SessionOutcome::Exit };
                match cmd {
                    UiCmd::Send(text) => {
                        transcript.user_prompt(&text, streaming);
                        // `prompt_steering` has no images param, so a send
                        // mid-stream leaves any pending attachments queued
                        // for the next non-streaming send instead of
                        // silently dropping them.
                        let result = if streaming {
                            client.prompt_steering(&text).await
                        } else {
                            let images: Vec<ImageContent> =
                                std::mem::take(&mut pending_images).into_iter().map(|(_, i)| i).collect();
                            tracing::debug!(images = images.len(), "send: with attachments");
                            transcript.ui.set_pending_attachments(Vec::new());
                            client.prompt_with_images(&text, images).await
                        };
                        if let Err(e) = result {
                            transcript.note("error", e.to_string());
                        }
                    }
                    UiCmd::AttachPath(path) => {
                        attach_path(client, transcript, &mut pending_images, path).await;
                    }
                    UiCmd::RemoveAttachment(index) => {
                        if index < pending_images.len() {
                            pending_images.remove(index);
                        }
                        transcript.ui.set_pending_attachments(
                            pending_images.iter().map(|(name, _)| name.clone()).collect(),
                        );
                    }
                    UiCmd::Abort => {
                        if streaming {
                            if let Err(e) = client.abort().await {
                                transcript.note("error", format!("abort failed: {e}"));
                            }
                        }
                    }
                    UiCmd::SetModel(i) => {
                        if let Some(entry) = models.get(i) {
                            match client.set_model(&entry.provider, &entry.id).await {
                                Ok(_) => {
                                    models.current = i as i32;
                                    thinking_levels =
                                        refresh_thinking(client, transcript).await;
                                    dot_interval.reset_immediately();
                                }
                                Err(e) => transcript.note("error", e.to_string()),
                            }
                        }
                    }
                    UiCmd::SetThinking(i) => {
                        if let Some(level) = thinking_levels.get(i) {
                            if let Err(e) = client.set_thinking_level(*level).await {
                                transcript.note("error", e.to_string());
                            }
                        }
                    }
                    UiCmd::SwitchSession(path) => {
                        resume_session(client, transcript, &path).await;
                        sidebar.refresh_sessions(client, transcript.ui.as_ref()).await;
                    }
                    UiCmd::SwitchProject(path) => {
                        if streaming {
                            // Best-effort; the child is about to be killed
                            // regardless, so a failed abort isn't fatal.
                            let _ = client.abort().await;
                        }
                        transcript.reset();
                        transcript.note("info", format!("switching to {}…", path.display()));
                        return SessionOutcome::SwitchProject(path);
                    }
                    UiCmd::NewSession => {
                        match client.new_session(None).await {
                            Ok(data) if data.get("cancelled").and_then(|v| v.as_bool()) == Some(true) => {
                                transcript.note("info", "new session cancelled by an extension".to_string());
                            }
                            Ok(_) => transcript.reset(),
                            Err(e) => transcript.note("error", format!("could not start a new session: {e}")),
                        }
                        sidebar.refresh_sessions(client, transcript.ui.as_ref()).await;
                    }
                    UiCmd::DeleteSession(path) => {
                        delete_session(client, transcript, sidebar, &path).await;
                    }
                    UiCmd::RenameSession(name) => {
                        if let Err(e) = client.set_session_name(name).await {
                            transcript.note("error", format!("could not rename session: {e}"));
                        }
                        sidebar.refresh_sessions(client, transcript.ui.as_ref()).await;
                    }
                    UiCmd::SidebarSearch(query) => {
                        sidebar.query = query;
                        sidebar.refresh_sessions(client, transcript.ui.as_ref()).await;
                    }
                    UiCmd::OpenTree => {
                        match fetch_tree_rows(client).await {
                            Ok(rows) => transcript.ui.set_tree(rows),
                            Err(e) => transcript.note("error", format!("could not load tree: {e}")),
                        }
                    }
                    UiCmd::ForkFrom(entry_id) => {
                        fork_from(client, transcript, &entry_id).await;
                        sidebar.refresh_sessions(client, transcript.ui.as_ref()).await;
                    }
                    UiCmd::CloneSession => {
                        clone_session(client, transcript).await;
                        sidebar.refresh_sessions(client, transcript.ui.as_ref()).await;
                    }
                    UiCmd::OpenPalette => {
                        let sessions = sidebar
                            .session_dir()
                            .and_then(|dir| sidebar.meta_cache.list_sessions(&dir).ok())
                            .unwrap_or_default();
                        let commands = client
                            .get_commands()
                            .await
                            .ok()
                            .and_then(|d| d.get("commands").and_then(|v| v.as_array()).cloned())
                            .unwrap_or_default();
                        palette_entries =
                            palette::build_entries(&sessions, &commands, &models.labels);
                        tracing::debug!(entries = palette_entries.len(), "palette: built entries");
                        transcript.ui.set_palette_entries(palette::rank(&palette_entries, ""));
                    }
                    UiCmd::PaletteQuery(query) => {
                        let ranked = palette::rank(&palette_entries, &query);
                        tracing::debug!(
                            query,
                            matches = ranked.len(),
                            top = ranked.first().map(|e| e.id.as_str()),
                            "palette: ranked query"
                        );
                        transcript.ui.set_palette_entries(ranked);
                    }
                    UiCmd::OpenModels => {
                        open_models_panel(transcript).await;
                    }
                    UiCmd::ServeRapidMlxModel(alias) => {
                        serve_rapid_mlx_model(client, transcript, managed_rapid_mlx, &alias).await;
                        models = refresh_models(client, transcript).await;
                        dot_interval.reset_immediately();
                    }
                    UiCmd::LoadRouterModel(model) => {
                        load_router_model(transcript, &model).await;
                        // Not verifiable on this dev machine (no real
                        // llama-server to load against) — double-check the
                        // composer picker actually refreshes against a live
                        // router before relying on it; see the M3 plan's
                        // "nudge pi after catalog changes" note.
                        models = refresh_models(client, transcript).await;
                    }
                    UiCmd::UnloadRouterModel(model) => {
                        unload_router_model(transcript, &model).await;
                    }
                    UiCmd::SearchHfModels(query) => {
                        search_hf_models(transcript, &query).await;
                    }
                    UiCmd::DownloadRouterModel(model) => {
                        download_router_model(transcript, &model).await;
                    }
                    UiCmd::AddOllamaToPi => {
                        if add_ollama_to_pi(transcript).await {
                            // Not verifiable on this dev machine (no
                            // running Ollama) — double-check the composer
                            // picker refreshes against a live Ollama
                            // before relying on it, same caveat as the
                            // router load nudge above.
                            models = refresh_models(client, transcript).await;
                        }
                    }
                    UiCmd::SaveApiKey { provider, key } => {
                        save_api_key(transcript, &provider, &key);
                        // A fresh key can make a configured provider usable;
                        // let the picker pick that up.
                        models = refresh_models(client, transcript).await;
                    }
                    UiCmd::ServerDotClicked => {
                        let dead_alias = managed_rapid_mlx
                            .as_mut()
                            .and_then(|m| (!m.server.is_alive()).then(|| m.alias.clone()));
                        if let Some(alias) = dead_alias {
                            transcript.note(
                                "info",
                                format!("rapid-mlx server for {alias} died — restarting…"),
                            );
                            serve_rapid_mlx_model(client, transcript, managed_rapid_mlx, &alias)
                                .await;
                            models = refresh_models(client, transcript).await;
                            dot_interval.reset_immediately();
                        } else {
                            open_models_panel(transcript).await;
                        }
                    }
                }
            }
            _ = dot_interval.tick() => {
                let dot = compute_server_dot(models.current_entry(), managed_rapid_mlx).await;
                if dot != server_dot {
                    server_dot = dot;
                    transcript.ui.set_server_dot(dot);
                }
            }
            event = events.recv() => {
                let Some(event) = event else {
                    transcript.note("error", "pi exited.");
                    transcript.ui.set_streaming(false);
                    return SessionOutcome::Exit;
                };
                match &event {
                    Event::AgentStart => streaming = true,
                    Event::AgentSettled => {
                        streaming = false;
                    }
                    _ => {}
                }
                transcript.apply(&event);
                if matches!(event, Event::AgentSettled) {
                    update_stats(client, transcript).await;
                    sidebar.refresh_sessions(client, transcript.ui.as_ref()).await;
                }
                if let Event::ExtensionUiRequest(req) = &event {
                    handle_extension_ui(client, transcript, req);
                }
            }
            changed = sessions_changed.recv(), if watcher_alive => {
                if changed.is_none() {
                    watcher_alive = false;
                    continue;
                }
                // Picks up sessions created or renamed by other processes
                // (most notably pi's own TUI) sharing this sessions root.
                tracing::debug!("sessions watcher: change detected, refreshing sidebar");
                sidebar.refresh_projects(transcript.ui.as_ref());
                sidebar.refresh_sessions(client, transcript.ui.as_ref()).await;
            }
        }
    }
}

/// Move a session file to the OS trash. If it was the currently-open
/// session, starts a fresh one so the child keeps working against a file
/// that still exists, then refreshes the sidebar either way.
async fn delete_session(
    client: &PiClient,
    transcript: &mut Transcript,
    sidebar: &Sidebar,
    path: &str,
) {
    let is_active = active_session_path(client).await.as_deref() == Some(path);
    let target = PathBuf::from(path);
    let result = tokio::task::spawn_blocking(move || trash::delete(&target)).await;
    match result {
        Ok(Ok(())) => {
            if is_active {
                // The file is already gone regardless of what an extension
                // thinks; reset either way so the UI matches reality.
                if let Err(e) = client.new_session(None).await {
                    transcript.note(
                        "error",
                        format!("deleted the open session but could not start a new one: {e}"),
                    );
                }
                transcript.reset();
            }
        }
        Ok(Err(e)) => transcript.note("error", format!("could not delete session: {e}")),
        Err(e) => transcript.note("error", format!("delete task failed: {e}")),
    }
    sidebar
        .refresh_sessions(client, transcript.ui.as_ref())
        .await;
}

/// Switch the running child to a different session file and hydrate the
/// transcript from its full history. Same-process only — pi's `switch_session`
/// requires the session to live under the child's current cwd; changing
/// project requires killing and respawning the child instead (M2 item 3).
async fn resume_session(client: &PiClient, transcript: &mut Transcript, session_path: &str) {
    match client.switch_session(session_path).await {
        Ok(data) => {
            if data.get("cancelled").and_then(|v| v.as_bool()) == Some(true) {
                transcript.note(
                    "info",
                    "session switch cancelled by an extension".to_string(),
                );
                return;
            }
        }
        Err(e) => {
            transcript.note("error", format!("could not switch session: {e}"));
            return;
        }
    }
    hydrate_active_session(client, transcript).await;
}

/// Duplicate the active session's active branch into a new session file at
/// the current point. pi rebinds the running child to the new file on
/// success (same as `switch_session`), so this hydrates from it just like
/// [`resume_session`] does.
async fn clone_session(client: &PiClient, transcript: &mut Transcript) {
    match client.clone_session().await {
        Ok(data) => {
            if data.get("cancelled").and_then(|v| v.as_bool()) == Some(true) {
                transcript.note("info", "clone cancelled by an extension".to_string());
                return;
            }
        }
        Err(e) => {
            transcript.note("error", format!("could not clone session: {e}"));
            return;
        }
    }
    hydrate_active_session(client, transcript).await;
}

/// Re-fetch the active child's full message history and replace the
/// transcript with it. Shared by [`resume_session`] (after `switch_session`)
/// and [`fork_from`] (after `fork`) — both move the active branch and need
/// the same reload.
async fn hydrate_active_session(client: &PiClient, transcript: &mut Transcript) {
    match client.get_messages().await {
        Ok(data) => {
            let messages = data
                .get("messages")
                .and_then(|m| m.as_array())
                .cloned()
                .unwrap_or_default();
            tracing::debug!(messages = messages.len(), "hydrate_active_session");
            transcript.reset();
            transcript.hydrate(&messages);
            update_stats(client, transcript).await;
        }
        Err(e) => transcript.note("error", format!("could not load session messages: {e}")),
    }
}

/// Handle an attach-button (or future drop) pick: images are read,
/// base64-encoded, and queued as a chip; anything else is appended to the
/// composer as an `@path` reference. `client` isn't used yet (images are
/// read straight off disk) but matches the other `UiCmd` handlers' shape and
/// will be needed if attachment validation ever needs the running session.
async fn attach_path(
    _client: &PiClient,
    transcript: &mut Transcript,
    pending_images: &mut Vec<(String, ImageContent)>,
    path: PathBuf,
) {
    let Some(mime_type) = attach::image_mime_type(&path) else {
        tracing::debug!(path = %path.display(), "attach: non-image, appended @path");
        transcript.ui.append_composer_text(&path);
        return;
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let data = attach::encode_base64(&bytes);
            tracing::debug!(
                name,
                mime_type,
                bytes = bytes.len(),
                b64_len = data.len(),
                "attach: image queued"
            );
            pending_images.push((
                name,
                ImageContent {
                    kind: "image".to_string(),
                    data,
                    mime_type: mime_type.to_string(),
                },
            ));
            transcript.ui.set_pending_attachments(
                pending_images
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect(),
            );
        }
        Err(e) => transcript.note("error", format!("could not read {}: {e}", path.display())),
    }
}

/// Fork the active session from a prior user message and hydrate the
/// resulting (now-active) branch.
async fn fork_from(client: &PiClient, transcript: &mut Transcript, entry_id: &str) {
    // `fork` rewinds the active branch to *before* entryId and hands back its
    // text — it does not keep that message in context — so the composer
    // needs pre-filling, or the user loses the prompt they meant to redo.
    let prefill = match client.fork(entry_id).await {
        Ok(data) => {
            if data.get("cancelled").and_then(|v| v.as_bool()) == Some(true) {
                transcript.note("info", "fork cancelled by an extension".to_string());
                return;
            }
            data.get("text")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }
        Err(e) => {
            transcript.note("error", format!("could not fork: {e}"));
            return;
        }
    };
    tracing::debug!(entry_id, prefill = prefill.as_deref(), "fork: got prefill");
    hydrate_active_session(client, transcript).await;
    if let Some(text) = prefill {
        transcript.ui.set_composer_text(text);
    }
}

// ---------------------------------------------------------------------------
// Models panel: rapid-mlx + llama.cpp router sections.
//
// rapid-mlx detection/catalog/ps are OS-level (not pi-specific), so
// `collect_rapid_mlx_snapshot` works the same under the demo backend too;
// only `serve_rapid_mlx_model`'s final `set_model` step needs a real
// `PiClient`, so demo mode declines that action instead (see
// `demo_backend`).
//
// The router has no local server to verify against on the dev machine, so
// `format_router_models` (entries -> UI rows) is a pure function shared by
// the live path (`fetch_router_state`, a real `list_models()` call) and the
// demo backend's hand-seeded `Vec<router::ModelEntry>` — verifying the demo
// panel this way actually exercises the formatter the live path runs, not a
// parallel stand-in.
// ---------------------------------------------------------------------------

/// Default port for the app's managed rapid-mlx server — matches rapid-mlx's
/// own `serve` default so an unmanaged/external server on the same port is
/// detected as a conflict rather than silently ignored (see
/// `serve_rapid_mlx_model`).
const RAPID_MLX_PORT: u16 = 8000;

/// Raw rapid-mlx CLI results, collected in one shot. Kept separate from
/// [`RapidMlxPanelData`] so the demo backend can seed a fake snapshot and run
/// it through the same [`format_rapid_mlx_panel`] the live path uses.
struct RapidMlxSnapshot {
    version: Option<String>,
    running: Vec<local::rapid_mlx::RunningServer>,
    cached: Vec<local::rapid_mlx::CachedModel>,
    catalog_count: usize,
}

async fn collect_rapid_mlx_snapshot() -> RapidMlxSnapshot {
    let rmlx = local::rapid_mlx::RapidMlx::default();
    let version = rmlx.version().await;
    let running = rmlx.running_servers().await.unwrap_or_default();
    let cached = rmlx.cached_models().await.unwrap_or_default();
    let catalog_count = rmlx.catalog().await.map(|c| c.len()).unwrap_or(0);
    RapidMlxSnapshot {
        version,
        running,
        cached,
        catalog_count,
    }
}

fn format_rapid_mlx_panel(
    snapshot: RapidMlxSnapshot,
    mem: &local::system_fit::SystemMemory,
) -> RapidMlxPanelData {
    let running_summary = snapshot
        .running
        .first()
        .map(|s| format!("{} running on :{} (uptime {})", s.model, s.port, s.uptime));
    let cached = snapshot
        .cached
        .into_iter()
        .map(|c| {
            let fit = mem.fit_label_for(c.size_bytes).label().to_string();
            (
                c.alias,
                c.hf_repo,
                local::system_fit::human_size(c.size_bytes),
                fit,
            )
        })
        .collect();
    RapidMlxPanelData {
        version: snapshot.version,
        running_summary,
        cached,
        catalog_count: snapshot.catalog_count,
    }
}

fn router_health_label(health: local::router::HealthState) -> &'static str {
    match health {
        local::router::HealthState::Ready => "ready",
        local::router::HealthState::Loading => "loading",
        local::router::HealthState::Unreachable => "unreachable",
    }
}

/// Human status label for one router model row, including progress when
/// loading/downloading (`/models` carries `status.progress` directly, so
/// polling this endpoint is enough to show live progress without also
/// wiring the `/models/sse` stream — see the module-level design note).
fn router_model_status_label(status: &local::router::ModelStatus) -> String {
    use local::router::{StatusProgress, StatusValue};
    if status.failed {
        return match status.exit_code {
            Some(code) => format!("failed (exit {code})"),
            None => "failed".to_string(),
        };
    }
    match status.value {
        StatusValue::Loaded => "loaded".to_string(),
        StatusValue::Unloaded => "unloaded".to_string(),
        StatusValue::Sleeping => "sleeping".to_string(),
        StatusValue::Loading => match &status.progress {
            Some(StatusProgress::Loading(p)) => match (&p.current, p.value) {
                (Some(stage), Some(v)) => format!("loading {stage} {:.0}%", v * 100.0),
                (None, Some(v)) => format!("loading {:.0}%", v * 100.0),
                _ => "loading…".to_string(),
            },
            _ => "loading…".to_string(),
        },
        StatusValue::Downloading => match &status.progress {
            Some(StatusProgress::Downloading(files)) if !files.is_empty() => {
                let (done, total) = files
                    .values()
                    .fold((0u64, 0u64), |(d, t), f| (d + f.done, t + f.total));
                if total > 0 {
                    format!("downloading {:.0}%", done as f64 / total as f64 * 100.0)
                } else {
                    "downloading…".to_string()
                }
            }
            _ => "downloading…".to_string(),
        },
        StatusValue::Unknown => "unknown".to_string(),
    }
}

fn router_model_busy(status: &local::router::ModelStatus) -> bool {
    matches!(
        status.value,
        local::router::StatusValue::Loading | local::router::StatusValue::Downloading
    )
}

/// Pure `/models` entries -> UI rows. Shared by the live path
/// (`fetch_router_state`) and the demo backend's fake entries — see the
/// module-level design note on why this must not be duplicated.
fn format_router_models(
    entries: Vec<local::router::ModelEntry>,
) -> Vec<(String, String, bool, bool)> {
    entries
        .into_iter()
        .map(|e| {
            let label = router_model_status_label(&e.status);
            let loaded = matches!(e.status.value, local::router::StatusValue::Loaded);
            let busy = router_model_busy(&e.status);
            (e.id, label, loaded, busy)
        })
        .collect()
}

async fn fetch_router_state(router: &local::router::LlamaRouter) -> RouterPanelData {
    let health = router.health().await;
    let models = if health == local::router::HealthState::Unreachable {
        Vec::new()
    } else {
        router
            .list_models(false)
            .await
            .map(format_router_models)
            .unwrap_or_default()
    };
    RouterPanelData {
        status_label: router_health_label(health).to_string(),
        base_url: router.base_url().to_string(),
        models,
    }
}

async fn open_models_panel(transcript: &mut Transcript) {
    let mem = local::system_fit::SystemMemory::probe();
    let snapshot = collect_rapid_mlx_snapshot().await;
    transcript
        .ui
        .set_rapid_mlx_panel(format_rapid_mlx_panel(snapshot, &mem));

    let router = local::router::LlamaRouter::default();
    transcript
        .ui
        .set_router_panel(fetch_router_state(&router).await);

    let ollama_models = local::ollama::OllamaProbe::default().list_models().await;
    let (detected, summary, count) = format_ollama_panel(ollama_models);
    transcript.ui.set_ollama_panel(detected, summary, count);

    refresh_auth_entries(transcript);

    transcript.ui.show_models_panel();
}

/// (Re)load auth.json's entry list into the panel. Unreadable/malformed is
/// surfaced as a single pseudo-entry rather than an empty list pretending
/// there are no credentials.
fn refresh_auth_entries(transcript: &mut Transcript) {
    let labels = match local::auth_json::default_path() {
        Some(path) => match local::auth_json::AuthJson::load_or_empty(&path) {
            Ok(doc) => format_auth_entries(&doc.entries()),
            Err(e) => vec![format!("auth.json unreadable: {e}")],
        },
        None => vec!["auth.json: no home directory".to_string()],
    };
    transcript.ui.set_auth_entries(labels);
}

/// Pure entries -> panel labels, shared by the live path and the demo
/// backend's seeded fixture. Only provider ids and form labels — key
/// material never reaches the UI model.
fn format_auth_entries(entries: &[(String, local::auth_json::KeyForm)]) -> Vec<String> {
    entries
        .iter()
        .map(|(provider, form)| format!("{provider} · {}", form.label()))
        .collect()
}

/// Seeded auth.json entry list for demo mode: one of every form the panel
/// renders, run through the same `format_auth_entries` as the live path.
fn seed_demo_auth_entries() -> Vec<(String, local::auth_json::KeyForm)> {
    use local::auth_json::KeyForm;
    vec![
        ("anthropic".to_string(), KeyForm::Literal),
        ("cloudflare-ai-gateway".to_string(), KeyForm::Env),
        ("openai".to_string(), KeyForm::Command),
        ("github-copilot".to_string(), KeyForm::Managed),
    ]
}

/// Write one api_key entry into auth.json (load → edit → atomic 0600
/// write), then refresh the panel's entry list. All notes/errors mention
/// the provider only — never the key.
fn save_api_key(transcript: &mut Transcript, provider: &str, key: &Secret) {
    let Some(path) = local::auth_json::default_path() else {
        transcript.note("error", "auth.json: no home directory");
        return;
    };
    let result = local::auth_json::AuthJson::load_or_empty(&path).and_then(|mut doc| {
        doc.set_api_key(provider, &key.0)?;
        doc.write(&path)?;
        Ok(())
    });
    match result {
        Ok(()) => {
            let provider = provider.trim();
            transcript.note("info", format!("auth.json: key saved for {provider}"));
        }
        Err(e) => transcript.note("error", format!("auth.json: {e}")),
    }
    refresh_auth_entries(transcript);
}

/// Pure Ollama models -> panel summary, shared by the live path (above) and
/// the demo backend's seeded fixture — same shared-formatter guarantee as
/// the router/HF formatters. `None` means undetected (not installed, not
/// running — the panel doesn't distinguish why, see `OllamaProbe`).
fn format_ollama_panel(models: Option<Vec<local::ollama::OllamaModel>>) -> (bool, String, i32) {
    match models {
        None => (false, String::new(), 0),
        Some(models) if models.is_empty() => {
            (true, "detected, no models pulled yet".to_string(), 0)
        }
        Some(models) => {
            let count = models.len();
            let names = models
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            (true, format!("{count} model(s): {names}"), count as i32)
        }
    }
}

/// Writes every currently-detected Ollama model into `~/.pi/agent/
/// models.json` under the canonical `ollama` preset (see
/// `local::ollama::provider_preset`). Refuses to touch a `models.json` it
/// can't parse rather than guessing (the M3 plan's stated corruption
/// mitigation) — an existing `ollama` entry, hand-written or from a prior
/// add, is replaced wholesale, everything else in the file is untouched
/// (verified by `local::models_json`'s round-trip tests). Returns whether
/// the write succeeded, so the caller knows whether to nudge pi to re-read
/// it (see `refresh_models`'s call site in `run_session`).
async fn add_ollama_to_pi(transcript: &mut Transcript) -> bool {
    let Some(models) = local::ollama::OllamaProbe::default().list_models().await else {
        transcript.note("error", "Ollama: no longer detected — nothing to add");
        return false;
    };
    if models.is_empty() {
        transcript.note("info", "Ollama: no models pulled yet — nothing to add");
        return false;
    }
    let Some(path) = local::models_json::default_path() else {
        transcript.note("error", "could not resolve $HOME to locate models.json");
        return false;
    };
    let mut doc = if path.exists() {
        match local::models_json::ModelsJson::load(&path) {
            Ok(doc) => doc,
            Err(e) => {
                transcript.note(
                    "error",
                    format!("{e} — refusing to overwrite a models.json I can't parse"),
                );
                return false;
            }
        }
    } else {
        local::models_json::ModelsJson::empty()
    };
    let ids: Vec<String> = models.into_iter().map(|m| m.name).collect();
    doc.set_provider("ollama", local::ollama::provider_preset(&ids));
    let wrote = match doc.write(&path) {
        Ok(()) => {
            transcript.note(
                "info",
                format!("added {} Ollama model(s) to {}", ids.len(), path.display()),
            );
            true
        }
        Err(e) => {
            transcript.note("error", format!("could not write models.json: {e}"));
            false
        }
    };
    open_models_panel(transcript).await;
    wrote
}

/// Re-fetches and re-renders only the router section — used while polling
/// for load/unload progress, so the (comparatively expensive, multi-process)
/// rapid-mlx snapshot isn't re-collected every tick. Returns whether any
/// model is still loading/downloading, so callers know whether to keep
/// polling.
async fn refresh_router_panel(
    transcript: &mut Transcript,
    router: &local::router::LlamaRouter,
) -> bool {
    let state = fetch_router_state(router).await;
    let busy = state.models.iter().any(|(_, _, _, busy)| *busy);
    transcript.ui.set_router_panel(state);
    busy
}

/// Polls `/models` every 500ms until nothing is loading/downloading anymore
/// (or two minutes elapse), pushing each snapshot to the router section so
/// progress is visible throughout — the same "poll after triggering an
/// action" shape as `serve_rapid_mlx_model`'s `wait_ready`, just non-blocking
/// on the UI since router state can also change over SSE / another client.
async fn poll_router_until_idle(transcript: &mut Transcript, router: &local::router::LlamaRouter) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while refresh_router_panel(transcript, router).await && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn load_router_model(transcript: &mut Transcript, model: &str) {
    let router = local::router::LlamaRouter::default();
    transcript.note("info", format!("router: loading {model}…"));
    if let Err(e) = router.load_model(model).await {
        transcript.note("error", format!("router: failed to load {model}: {e}"));
        return;
    }
    poll_router_until_idle(transcript, &router).await;
}

async fn unload_router_model(transcript: &mut Transcript, model: &str) {
    let router = local::router::LlamaRouter::default();
    if let Err(e) = router.unload_model(model).await {
        transcript.note("error", format!("router: failed to unload {model}: {e}"));
    }
    poll_router_until_idle(transcript, &router).await;
}

/// `POST /models` (download-only, doesn't load) then poll like a load — the
/// downloaded model shows up as a new `downloading` row in `/models` as soon
/// as the router picks it up, so this reuses `poll_router_until_idle`
/// unchanged. Not runnable end-to-end on this dev machine (no real
/// llama-server) — verified via `format_hf_results`'s unit tests and the
/// demo backend's `simulate_router_download` instead.
async fn download_router_model(transcript: &mut Transcript, model: &str) {
    let router = local::router::LlamaRouter::default();
    transcript.note("info", format!("router: downloading {model}…"));
    if let Err(e) = router.download_model(model).await {
        transcript.note(
            "error",
            format!("router: failed to start download of {model}: {e}"),
        );
        return;
    }
    poll_router_until_idle(transcript, &router).await;
}

/// Pure Hugging Face search results -> UI rows, shared by the live path
/// (`search_hf_models`) and the demo backend's seeded fixtures — same
/// shared-formatter guarantee as `format_router_models`.
fn format_hf_results(models: Vec<local::hf::HfModel>) -> Vec<(String, bool, i32, Vec<String>)> {
    models
        .into_iter()
        .map(|m| {
            let quants = local::hf::gguf_quants(&m);
            (m.id, m.gated.is_gated(), m.downloads as i32, quants)
        })
        .collect()
}

const HF_SEARCH_LIMIT: u32 = 20;

async fn search_hf_models(transcript: &mut Transcript, query: &str) {
    if query.trim().is_empty() {
        transcript.ui.set_hf_search_results(Vec::new());
        return;
    }
    match local::hf::HfSearch::default()
        .search_gguf(query, HF_SEARCH_LIMIT)
        .await
    {
        Ok(models) => transcript
            .ui
            .set_hf_search_results(format_hf_results(models)),
        Err(e) => {
            transcript.note("error", format!("Hugging Face search failed: {e}"));
            transcript.ui.set_hf_search_results(Vec::new());
        }
    }
}

// ---------------------------------------------------------------------------
// Models panel: demo-mode fakes.
//
// Item 10 of the M3 plan wants the panel demoable offline — deterministic,
// not dependent on what happens to be installed/cached on the machine
// running the demo. rapid-mlx's live probing (`collect_rapid_mlx_snapshot`)
// happens to work for free on a machine that has it installed, but that's
// not the same as "demoable offline"; both sections get seeded fakes here,
// run through the exact same formatters (`format_rapid_mlx_panel`,
// `format_router_models`) the live path uses.
// ---------------------------------------------------------------------------

fn demo_rapid_mlx_snapshot() -> RapidMlxSnapshot {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    RapidMlxSnapshot {
        version: Some("rapid-mlx 0.11.0".to_string()),
        running: vec![local::rapid_mlx::RunningServer {
            pid: 12345,
            port: RAPID_MLX_PORT,
            model: "mlx-community/Qwen3.5-4B-MLX-4bit".to_string(),
            uptime: "12m".to_string(),
        }],
        cached: vec![
            local::rapid_mlx::CachedModel {
                alias: "qwen3.5-4b-4bit".to_string(),
                hf_repo: "mlx-community/Qwen3.5-4B-MLX-4bit".to_string(),
                size_bytes: (5.7 * GIB) as u64,
                modified: "2d ago".to_string(),
            },
            local::rapid_mlx::CachedModel {
                alias: "gpt-oss-120b".to_string(),
                hf_repo: "mlx-community/gpt-oss-120b-MXFP4-Q8".to_string(),
                size_bytes: (118.1 * GIB) as u64,
                modified: "1d ago".to_string(),
            },
        ],
        catalog_count: 165,
    }
}

/// Seeds one router model in each state the panel can render — loaded,
/// unloaded, loading (with progress), downloading (with progress), and
/// failed — so a single `OpenModels` screenshot exercises every branch of
/// `router_model_status_label` at once.
fn seed_demo_router_entries() -> Vec<local::router::ModelEntry> {
    use local::router::{
        FileProgress, LoadingProgress, ModelEntry, ModelStatus, StatusProgress, StatusValue,
    };
    vec![
        ModelEntry {
            id: "ggml-org/gemma-3-4b-it-GGUF:Q4_K_M".to_string(),
            path: Some("/demo/gemma-3-4b-it.gguf".to_string()),
            status: ModelStatus {
                value: StatusValue::Loaded,
                args: vec!["llama-server".to_string()],
                failed: false,
                exit_code: None,
                progress: None,
            },
            architecture: None,
        },
        ModelEntry {
            id: "unsloth/Qwen3-8B-GGUF:Q4_K_M".to_string(),
            path: Some("/demo/qwen3-8b.gguf".to_string()),
            status: ModelStatus {
                value: StatusValue::Unloaded,
                args: vec![],
                failed: false,
                exit_code: None,
                progress: None,
            },
            architecture: None,
        },
        ModelEntry {
            id: "mlx-community/Llama-3.2-3B-Instruct-4bit".to_string(),
            path: None,
            status: ModelStatus {
                value: StatusValue::Loading,
                args: vec![],
                failed: false,
                exit_code: None,
                progress: Some(StatusProgress::Loading(LoadingProgress {
                    stages: vec!["text_model".to_string()],
                    current: Some("text_model".to_string()),
                    value: Some(0.45),
                })),
            },
            architecture: None,
        },
        ModelEntry {
            id: "TheBloke/Mistral-7B-Instruct-v0.2-GGUF:Q4_K_M".to_string(),
            path: None,
            status: ModelStatus {
                value: StatusValue::Downloading,
                args: vec![],
                failed: false,
                exit_code: None,
                progress: Some(StatusProgress::Downloading(HashMap::from([(
                    "https://huggingface.co/TheBloke/Mistral-7B-Instruct-v0.2-GGUF/model.gguf"
                        .to_string(),
                    FileProgress {
                        done: 60,
                        total: 100,
                    },
                )]))),
            },
            architecture: None,
        },
        ModelEntry {
            id: "broken/does-not-load-GGUF".to_string(),
            path: None,
            status: ModelStatus {
                value: StatusValue::Unloaded,
                args: vec!["llama-server".to_string()],
                failed: true,
                exit_code: Some(1),
                progress: None,
            },
            architecture: None,
        },
    ]
}

async fn render_demo_models_panel(
    transcript: &mut Transcript,
    router_entries: &[local::router::ModelEntry],
) {
    let mem = local::system_fit::SystemMemory::probe();
    transcript
        .ui
        .set_rapid_mlx_panel(format_rapid_mlx_panel(demo_rapid_mlx_snapshot(), &mem));
    transcript.ui.set_router_panel(RouterPanelData {
        status_label: "ready".to_string(),
        base_url: local::router::DEFAULT_BASE_URL.to_string(),
        models: format_router_models(router_entries.to_vec()),
    });
    let (detected, summary, count) = format_ollama_panel(Some(seed_demo_ollama_models()));
    transcript.ui.set_ollama_panel(detected, summary, count);
    transcript.ui.show_models_panel();
}

/// Two seeded Ollama models — deterministic, offline (item 10's demoable
/// requirement applies here too, same reasoning as
/// `demo_rapid_mlx_snapshot`/`seed_demo_router_entries`).
fn seed_demo_ollama_models() -> Vec<local::ollama::OllamaModel> {
    vec![
        local::ollama::OllamaModel {
            name: "llama3.1:8b".to_string(),
            size: 4_920_000_000,
            details: None,
        },
        local::ollama::OllamaModel {
            name: "qwen2.5-coder:7b".to_string(),
            size: 4_680_000_000,
            details: None,
        },
    ]
}

/// Simulates a load: transitions the entry through a couple of progress
/// ticks to `loaded`, rendering after each so progress is visible — the
/// synthetic counterpart to `poll_router_until_idle` against a real router.
async fn simulate_router_load(
    transcript: &mut Transcript,
    entries: &mut [local::router::ModelEntry],
    model: &str,
) {
    use local::router::{LoadingProgress, ModelStatus, StatusProgress, StatusValue};
    for pct in [25u32, 60, 100] {
        if let Some(entry) = entries.iter_mut().find(|e| e.id == model) {
            entry.status = if pct < 100 {
                ModelStatus {
                    value: StatusValue::Loading,
                    args: vec![],
                    failed: false,
                    exit_code: None,
                    progress: Some(StatusProgress::Loading(LoadingProgress {
                        stages: vec![],
                        current: None,
                        value: Some(pct as f64 / 100.0),
                    })),
                }
            } else {
                ModelStatus {
                    value: StatusValue::Loaded,
                    args: vec![],
                    failed: false,
                    exit_code: None,
                    progress: None,
                }
            };
        }
        render_demo_models_panel(transcript, entries).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn simulate_router_unload(
    transcript: &mut Transcript,
    entries: &mut [local::router::ModelEntry],
    model: &str,
) {
    use local::router::{ModelStatus, StatusValue};
    if let Some(entry) = entries.iter_mut().find(|e| e.id == model) {
        entry.status = ModelStatus {
            value: StatusValue::Unloaded,
            args: vec![],
            failed: false,
            exit_code: None,
            progress: None,
        };
    }
    render_demo_models_panel(transcript, entries).await;
}

/// Two seeded HF search results (one gated, one not) covering the quant-chip
/// and gated-warning rendering paths — run through the same
/// `format_hf_results` the live path uses.
fn seed_demo_hf_results() -> Vec<local::hf::HfModel> {
    use local::hf::{Gated, HfModel, Sibling};
    fn siblings(names: &[&str]) -> Vec<Sibling> {
        names
            .iter()
            .map(|n| Sibling {
                rfilename: n.to_string(),
            })
            .collect()
    }
    vec![
        HfModel {
            id: "unsloth/Phi-4-mini-instruct-GGUF".to_string(),
            gated: Gated::Bool(false),
            downloads: 48213,
            siblings: siblings(&[
                "Phi-4-mini-instruct-BF16.gguf",
                "Phi-4-mini-instruct-Q4_K_M.gguf",
                "Phi-4-mini-instruct-Q8_0.gguf",
            ]),
        },
        HfModel {
            id: "meta-llama/Llama-3.1-8B-Instruct-GGUF".to_string(),
            gated: Gated::Kind("manual".to_string()),
            downloads: 1523890,
            siblings: siblings(&[
                "Llama-3.1-8B-Instruct-Q4_K_M.gguf",
                "Llama-3.1-8B-Instruct-Q5_K_M.gguf",
            ]),
        },
    ]
}

/// Simulates a download: inserts a `downloading` entry if `model` (an
/// `owner/repo:quant` string, as built by the HF search panel) isn't already
/// a router entry, ticks progress like `simulate_router_load`, then lands on
/// `unloaded` — a download doesn't load the model, matching the real
/// router's `POST /models` semantics.
async fn simulate_router_download(
    transcript: &mut Transcript,
    entries: &mut Vec<local::router::ModelEntry>,
    model: &str,
) {
    use local::router::{FileProgress, ModelEntry, ModelStatus, StatusProgress, StatusValue};
    if !entries.iter().any(|e| e.id == model) {
        entries.push(ModelEntry {
            id: model.to_string(),
            path: None,
            status: ModelStatus {
                value: StatusValue::Downloading,
                args: vec![],
                failed: false,
                exit_code: None,
                progress: None,
            },
            architecture: None,
        });
    }
    for pct in [25u64, 60, 100] {
        if let Some(entry) = entries.iter_mut().find(|e| e.id == model) {
            entry.status = if pct < 100 {
                ModelStatus {
                    value: StatusValue::Downloading,
                    args: vec![],
                    failed: false,
                    exit_code: None,
                    progress: Some(StatusProgress::Downloading(HashMap::from([(
                        format!("https://huggingface.co/{model}"),
                        FileProgress {
                            done: pct,
                            total: 100,
                        },
                    )]))),
                }
            } else {
                ModelStatus {
                    value: StatusValue::Unloaded,
                    args: vec![],
                    failed: false,
                    exit_code: None,
                    progress: None,
                }
            };
        }
        render_demo_models_panel(transcript, entries).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[cfg(test)]
mod model_label_tests {
    use super::*;

    #[test]
    fn local_base_urls_are_detected() {
        for url in [
            "http://localhost:8000",
            "http://127.0.0.1:8080",
            "http://0.0.0.0:11434/v1",
            "http://[::1]:8080",
            "https://localhost",
        ] {
            assert!(is_local_base_url(url), "{url} should be local");
        }
        for url in [
            "https://api.anthropic.com",
            "https://api.openai.com/v1",
            "http://192.168.1.10:8080",
            "",
        ] {
            assert!(!is_local_base_url(url), "{url} should not be local");
        }
    }

    #[test]
    fn cloud_model_label_shows_provider_and_price() {
        let m = serde_json::json!({
            "id": "claude-sonnet-4",
            "name": "Claude Sonnet 4",
            "provider": "anthropic",
            "baseUrl": "https://api.anthropic.com",
            "cost": {"input": 3.0, "output": 15.0},
        });
        assert_eq!(model_label(&m), "Claude Sonnet 4 · anthropic · $3/$15");
    }

    #[test]
    fn cloud_model_without_cost_omits_the_price() {
        let m = serde_json::json!({
            "id": "some-model",
            "provider": "openai",
            "baseUrl": "https://api.openai.com",
        });
        assert_eq!(model_label(&m), "some-model · openai");
    }

    #[test]
    fn local_model_label_says_free_local_instead_of_price() {
        let m = serde_json::json!({
            "id": "qwen3.5-4b-4bit",
            "provider": "rapid-mlx",
            "baseUrl": "http://localhost:8000",
            "cost": {"input": 0.0, "output": 0.0},
        });
        assert_eq!(
            model_label(&m),
            "qwen3.5-4b-4bit · rapid-mlx · free · local"
        );
    }

    #[test]
    fn fractional_prices_keep_significant_digits() {
        let m = serde_json::json!({
            "id": "m", "provider": "p",
            "baseUrl": "https://api.example.com",
            "cost": {"input": 0.25, "output": 0.6},
        });
        assert_eq!(model_label(&m), "m · p · $0.25/$0.6");
    }
}

#[cfg(test)]
mod server_dot_tests {
    use super::*;
    use local::rapid_mlx::ServerHealth;

    const MODEL: &str = "mlx-community/Qwen3.6-35B-A3B-8bit";

    fn health(model_name: Option<&str>, ready: bool, loaded: bool) -> ServerHealth {
        ServerHealth {
            ready,
            model_loaded: loaded,
            model_name: model_name.map(str::to_string),
        }
    }

    #[test]
    fn serving_the_current_model_is_ok() {
        let h = health(Some(MODEL), true, true);
        assert_eq!(classify_rapid_mlx_dot(MODEL, Some(&h), None), SERVER_DOT_OK);
    }

    #[test]
    fn serving_a_different_model_is_a_mismatch_not_ok() {
        // The exact failure from the field: server up and healthy, but
        // serving another model — completions 404.
        let h = health(
            Some("mlx-community/Qwen3-4B-Instruct-2507-4bit"),
            true,
            true,
        );
        assert_eq!(
            classify_rapid_mlx_dot(MODEL, Some(&h), None),
            SERVER_DOT_MISMATCH
        );
    }

    #[test]
    fn right_model_but_not_ready_yet_is_down() {
        let h = health(Some(MODEL), false, false);
        assert_eq!(
            classify_rapid_mlx_dot(MODEL, Some(&h), None),
            SERVER_DOT_DOWN
        );
    }

    #[test]
    fn no_health_response_is_down_unless_a_managed_child_is_still_alive() {
        assert_eq!(classify_rapid_mlx_dot(MODEL, None, None), SERVER_DOT_DOWN);
        assert_eq!(
            classify_rapid_mlx_dot(MODEL, None, Some(false)),
            SERVER_DOT_DOWN
        );
        assert_eq!(
            classify_rapid_mlx_dot(MODEL, None, Some(true)),
            SERVER_DOT_OK
        );
    }
}

#[cfg(test)]
mod auth_panel_tests {
    use super::*;

    #[test]
    fn labels_cover_every_form_and_never_contain_key_material() {
        let labels = format_auth_entries(&seed_demo_auth_entries());
        assert_eq!(
            labels,
            vec![
                "anthropic · api key",
                "cloudflare-ai-gateway · $ENV — read-only",
                "openai · !command — read-only",
                "github-copilot · managed by pi /login",
            ]
        );
    }

    #[test]
    fn secret_debug_is_redacted() {
        let cmd = UiCmd::SaveApiKey {
            provider: "anthropic".into(),
            key: Secret("sk-ant-super-secret".into()),
        };
        let printed = format!("{cmd:?}");
        assert!(!printed.contains("super-secret"), "{printed}");
        assert!(printed.contains("redacted"));
    }
}

#[cfg(test)]
mod models_panel_tests {
    use super::*;
    use local::router::{FileProgress, LoadingProgress, ModelStatus, StatusProgress, StatusValue};

    #[test]
    fn status_label_covers_every_branch() {
        let loaded = ModelStatus {
            value: StatusValue::Loaded,
            args: vec![],
            failed: false,
            exit_code: None,
            progress: None,
        };
        assert_eq!(router_model_status_label(&loaded), "loaded");

        let loading_with_stage = ModelStatus {
            value: StatusValue::Loading,
            args: vec![],
            failed: false,
            exit_code: None,
            progress: Some(StatusProgress::Loading(LoadingProgress {
                stages: vec![],
                current: Some("text_model".to_string()),
                value: Some(0.45),
            })),
        };
        assert_eq!(
            router_model_status_label(&loading_with_stage),
            "loading text_model 45%"
        );

        let downloading = ModelStatus {
            value: StatusValue::Downloading,
            args: vec![],
            failed: false,
            exit_code: None,
            progress: Some(StatusProgress::Downloading(HashMap::from([(
                "https://x/model.gguf".to_string(),
                FileProgress {
                    done: 60,
                    total: 100,
                },
            )]))),
        };
        assert_eq!(router_model_status_label(&downloading), "downloading 60%");

        let failed = ModelStatus {
            value: StatusValue::Unloaded,
            args: vec![],
            failed: true,
            exit_code: Some(1),
            progress: None,
        };
        assert_eq!(router_model_status_label(&failed), "failed (exit 1)");
    }

    #[test]
    fn busy_is_true_only_while_loading_or_downloading() {
        let loading = ModelStatus {
            value: StatusValue::Loading,
            args: vec![],
            failed: false,
            exit_code: None,
            progress: None,
        };
        assert!(router_model_busy(&loading));

        let loaded = ModelStatus {
            value: StatusValue::Loaded,
            args: vec![],
            failed: false,
            exit_code: None,
            progress: None,
        };
        assert!(!router_model_busy(&loaded));
    }

    /// The demo backend's fake catalog and the live path's real `/models`
    /// response both flow through `format_router_models` — this is the
    /// guarantee that verifying the demo panel actually exercises the
    /// formatter the live path runs, not a parallel stand-in (see the
    /// module-level design note above `RAPID_MLX_PORT`).
    #[test]
    fn seeded_demo_entries_cover_every_row_state_via_the_shared_formatter() {
        let rows = format_router_models(seed_demo_router_entries());
        assert_eq!(rows.len(), 5);

        let (_, status, loaded, busy) = rows.iter().find(|(id, ..)| id.contains("gemma")).unwrap();
        assert_eq!(status, "loaded");
        assert!(loaded);
        assert!(!busy);

        let (_, status, loaded, busy) = rows
            .iter()
            .find(|(id, ..)| id.contains("Qwen3-8B"))
            .unwrap();
        assert_eq!(status, "unloaded");
        assert!(!loaded);
        assert!(!busy);

        let (_, status, loaded, busy) = rows
            .iter()
            .find(|(id, ..)| id.contains("Llama-3.2"))
            .unwrap();
        assert!(status.starts_with("loading"));
        assert!(status.contains('%'));
        assert!(!loaded);
        assert!(busy);

        let (_, status, loaded, busy) =
            rows.iter().find(|(id, ..)| id.contains("Mistral")).unwrap();
        assert!(status.starts_with("downloading"));
        assert!(status.contains('%'));
        assert!(!loaded);
        assert!(busy);

        let (_, status, loaded, busy) = rows.iter().find(|(id, ..)| id.contains("broken")).unwrap();
        assert!(status.starts_with("failed"));
        assert!(!loaded);
        assert!(!busy);
    }

    #[test]
    fn demo_rapid_mlx_snapshot_formats_into_a_fit_labeled_cached_row() {
        let mem = local::system_fit::SystemMemory {
            total_bytes: 32 * 1024 * 1024 * 1024,
            available_bytes: 32 * 1024 * 1024 * 1024,
        };
        let data = format_rapid_mlx_panel(demo_rapid_mlx_snapshot(), &mem);
        assert_eq!(data.version.as_deref(), Some("rapid-mlx 0.11.0"));
        assert!(data.running_summary.unwrap().contains("Qwen3.5-4B"));
        assert_eq!(data.cached.len(), 2);
        let (alias, hf_repo, size, fit) = &data.cached[0];
        assert_eq!(alias, "qwen3.5-4b-4bit");
        assert_eq!(hf_repo, "mlx-community/Qwen3.5-4B-MLX-4bit");
        assert_eq!(size, "5.7 GiB");
        assert_eq!(fit, "Fits");
    }

    /// Same shared-formatter guarantee as the router fixture test above,
    /// for the HF search results path.
    #[test]
    fn seeded_demo_hf_results_cover_gated_and_public_via_the_shared_formatter() {
        let rows = format_hf_results(seed_demo_hf_results());
        assert_eq!(rows.len(), 2);

        let (id, gated, downloads, quants) =
            rows.iter().find(|(id, ..)| id.contains("Phi-4")).unwrap();
        assert_eq!(id, "unsloth/Phi-4-mini-instruct-GGUF");
        assert!(!gated);
        assert!(*downloads > 0);
        assert_eq!(
            quants,
            &vec!["BF16".to_string(), "Q4_K_M".to_string(), "Q8_0".to_string()]
        );

        let (_, gated, _, quants) = rows
            .iter()
            .find(|(id, ..)| id.contains("Llama-3.1"))
            .unwrap();
        assert!(gated);
        assert_eq!(quants, &vec!["Q4_K_M".to_string(), "Q5_K_M".to_string()]);
    }

    #[test]
    fn ollama_panel_distinguishes_undetected_empty_and_populated() {
        let (detected, summary, count) = format_ollama_panel(None);
        assert!(!detected);
        assert_eq!(count, 0);
        assert!(summary.is_empty());

        let (detected, summary, count) = format_ollama_panel(Some(Vec::new()));
        assert!(detected);
        assert_eq!(count, 0);
        assert_eq!(summary, "detected, no models pulled yet");

        let (detected, summary, count) = format_ollama_panel(Some(seed_demo_ollama_models()));
        assert!(detected);
        assert_eq!(count, 2);
        assert!(summary.contains("llama3.1:8b"));
        assert!(summary.contains("qwen2.5-coder:7b"));
    }
}

/// (Re)spawns a managed `rapid-mlx serve <alias>` on `RAPID_MLX_PORT`,
/// stopping any server this app was already managing first (a model switch
/// is a supervised restart, not a hot swap — see the M3 plan). An
/// already-running *external* server (or anything else bound to the port)
/// surfaces as a normal "failed to become ready" error instead of being
/// killed: we only ever stop servers we ourselves spawned.
async fn serve_rapid_mlx_model(
    client: &PiClient,
    transcript: &mut Transcript,
    managed: &mut Option<ManagedRapidMlx>,
    alias: &str,
) {
    if let Some(prev) = managed.take() {
        transcript.note("info", "stopping the previous managed rapid-mlx server…");
        let _ = prev.server.shutdown().await;
    }

    transcript.note("info", format!("starting rapid-mlx serve {alias}…"));
    match local::rapid_mlx::ManagedServer::spawn(
        local::rapid_mlx::DEFAULT_BINARY,
        alias,
        RAPID_MLX_PORT,
    ) {
        Ok(mut server) => match server.wait_ready(Duration::from_secs(180)).await {
            Ok(()) => {
                *managed = Some(ManagedRapidMlx {
                    alias: alias.to_string(),
                    server,
                });
                match client.set_model("rapid-mlx", alias).await {
                    Ok(_) => transcript.note("info", format!("rapid-mlx: {alias} ready")),
                    Err(e) => transcript.note(
                        "error",
                        format!(
                            "rapid-mlx: {alias} is ready but pi couldn't select it \
                                 (is a matching entry configured in models.json?): {e}"
                        ),
                    ),
                }
            }
            Err(e) => transcript.note("error", format!("rapid-mlx: {alias} failed to start: {e}")),
        },
        Err(e) => transcript.note("error", format!("rapid-mlx: could not spawn serve: {e}")),
    }

    open_models_panel(transcript).await;
}

// ---------------------------------------------------------------------------
// Tree view: flatten `get_tree`'s nested `{entry, children, label?}` shape
// into an indented list for the (read-only, no graphical DAG) overlay.
// ---------------------------------------------------------------------------

struct FlatTreeRow {
    id: String,
    depth: i32,
    summary: String,
    label: String,
    can_fork: bool,
}

/// Fetch and flatten the active child's tree. `(id, depth, summary, label,
/// can_fork, is_active_branch)` per row, in depth-first display order.
async fn fetch_tree_rows(
    client: &PiClient,
) -> Result<Vec<(String, i32, String, String, bool, bool)>, PiError> {
    let data = client.get_tree().await?;
    let leaf_id = data
        .get("leafId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let nodes = data
        .get("tree")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut flat = Vec::new();
    let mut parents: HashMap<String, String> = HashMap::new();
    flatten_tree(&nodes, 0, &mut flat, &mut parents);

    let mut active = std::collections::HashSet::new();
    let mut current = leaf_id;
    while let Some(id) = current {
        current = parents.get(&id).cloned();
        active.insert(id);
    }

    Ok(flat
        .into_iter()
        .map(|r| {
            let is_active = active.contains(&r.id);
            (r.id, r.depth, r.summary, r.label, r.can_fork, is_active)
        })
        .collect())
}

fn flatten_tree(
    nodes: &[serde_json::Value],
    depth: i32,
    out: &mut Vec<FlatTreeRow>,
    parents: &mut HashMap<String, String>,
) {
    for node in nodes {
        let Some(entry) = node.get("entry") else {
            continue;
        };
        let Some(id) = entry.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(parent_id) = entry.get("parentId").and_then(|v| v.as_str()) {
            parents.insert(id.to_string(), parent_id.to_string());
        }
        let can_fork = entry.get("type").and_then(|v| v.as_str()) == Some("message")
            && entry.pointer("/message/role").and_then(|v| v.as_str()) == Some("user");
        let label = node
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(FlatTreeRow {
            id: id.to_string(),
            depth,
            summary: tree_node_summary(entry),
            label,
            can_fork,
        });
        if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
            flatten_tree(children, depth + 1, out, parents);
        }
    }
}

/// One-line human summary of a session-tree entry (any of the types in
/// docs/session-format.md), for the tree overlay row text.
fn tree_node_summary(entry: &serde_json::Value) -> String {
    let kind = entry.get("type").and_then(|v| v.as_str()).unwrap_or("?");
    match kind {
        "message" => {
            let message = entry.get("message").unwrap_or(&serde_json::Value::Null);
            match message.get("role").and_then(|v| v.as_str()) {
                Some("user") => {
                    let (text, _) = user_content_text(
                        message.get("content").unwrap_or(&serde_json::Value::Null),
                    );
                    elide_oneline(&text)
                }
                Some("assistant") => {
                    let text =
                        message
                            .get("content")
                            .and_then(|v| v.as_array())
                            .and_then(|blocks| {
                                blocks.iter().find_map(|b| {
                                    (b.get("type").and_then(|v| v.as_str()) == Some("text"))
                                        .then(|| b.get("text").and_then(|v| v.as_str()))
                                        .flatten()
                                })
                            });
                    match text {
                        Some(t) if !t.is_empty() => format!("assistant: {}", elide_oneline(t)),
                        _ => "assistant".to_string(),
                    }
                }
                Some("toolResult") => {
                    let name = message
                        .get("toolName")
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool");
                    format!("→ {name}")
                }
                Some("bashExecution") => {
                    format!(
                        "$ {}",
                        first_line(
                            message
                                .get("command")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                        )
                    )
                }
                Some(role) => role.to_string(),
                None => "message".to_string(),
            }
        }
        "model_change" => format!(
            "model → {}/{}",
            entry
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("?"),
            entry.get("modelId").and_then(|v| v.as_str()).unwrap_or("?"),
        ),
        "thinking_level_change" => format!(
            "thinking → {}",
            entry
                .get("thinkingLevel")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
        ),
        "compaction" => "context compacted".to_string(),
        "branch_summary" => "branch summary".to_string(),
        "custom" => match entry.get("customType").and_then(|v| v.as_str()) {
            Some(t) => format!("custom: {t}"),
            None => "custom".to_string(),
        },
        "custom_message" => "custom message".to_string(),
        "session_info" => match entry.get("name").and_then(|v| v.as_str()) {
            Some(name) => format!("renamed: {name}"),
            None => "session info".to_string(),
        },
        other => other.to_string(),
    }
}

fn elide_oneline(s: &str) -> String {
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= 70 {
        return collapsed;
    }
    let truncated: String = collapsed.chars().take(70).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tree_tests {
    use super::*;
    use serde_json::json;

    fn node(entry: serde_json::Value, children: Vec<serde_json::Value>) -> serde_json::Value {
        json!({"entry": entry, "children": children})
    }

    /// Mirrors get_tree's shape for a session that branched: a user prompt,
    /// two children off it (the original assistant reply and a
    /// branch_summary from switching away), each continuing separately.
    fn sample_tree() -> Vec<serde_json::Value> {
        vec![node(
            json!({"type": "message", "id": "u1", "parentId": null, "message": {"role": "user", "content": "refactor the parser"}}),
            vec![
                node(
                    json!({"type": "message", "id": "a1", "parentId": "u1", "message": {"role": "assistant", "content": [{"type": "text", "text": "sure, let's start"}]}}),
                    vec![],
                ),
                node(
                    json!({"type": "branch_summary", "id": "b1", "parentId": "u1", "fromId": "a1", "summary": "explored a bash approach first"}),
                    vec![node(
                        json!({"type": "message", "id": "u2", "parentId": "b1", "message": {"role": "user", "content": "actually skip the shell-out"}}),
                        vec![],
                    )],
                ),
            ],
        )]
    }

    #[test]
    fn flattens_depth_first_with_correct_depths() {
        let mut flat = Vec::new();
        let mut parents = HashMap::new();
        flatten_tree(&sample_tree(), 0, &mut flat, &mut parents);
        let ids_and_depths: Vec<(&str, i32)> =
            flat.iter().map(|r| (r.id.as_str(), r.depth)).collect();
        assert_eq!(
            ids_and_depths,
            vec![("u1", 0), ("a1", 1), ("b1", 1), ("u2", 2)]
        );
    }

    #[test]
    fn only_user_messages_are_forkable() {
        let mut flat = Vec::new();
        let mut parents = HashMap::new();
        flatten_tree(&sample_tree(), 0, &mut flat, &mut parents);
        let forkable: Vec<&str> = flat
            .iter()
            .filter(|r| r.can_fork)
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(forkable, vec!["u1", "u2"]);
    }

    #[test]
    fn parent_map_enables_active_branch_lookup() {
        let mut flat = Vec::new();
        let mut parents = HashMap::new();
        flatten_tree(&sample_tree(), 0, &mut flat, &mut parents);
        // Active leaf is u2, on the branch_summary side, not a1.
        let mut active = std::collections::HashSet::new();
        let mut current = Some("u2".to_string());
        while let Some(id) = current {
            current = parents.get(&id).cloned();
            active.insert(id);
        }
        assert!(active.contains("u2"));
        assert!(active.contains("b1"));
        assert!(active.contains("u1"));
        assert!(!active.contains("a1"));
    }

    #[test]
    fn summarizes_every_entry_kind() {
        assert_eq!(
            tree_node_summary(
                &json!({"type": "message", "message": {"role": "user", "content": "hello there"}})
            ),
            "hello there"
        );
        assert_eq!(
            tree_node_summary(
                &json!({"type": "message", "message": {"role": "assistant", "content": [{"type": "text", "text": "hi!"}]}})
            ),
            "assistant: hi!"
        );
        assert_eq!(
            tree_node_summary(
                &json!({"type": "message", "message": {"role": "assistant", "content": [{"type": "toolCall", "id": "c1", "name": "bash", "arguments": {}}]}})
            ),
            "assistant"
        );
        assert_eq!(
            tree_node_summary(
                &json!({"type": "message", "message": {"role": "toolResult", "toolName": "bash"}})
            ),
            "→ bash"
        );
        assert_eq!(
            tree_node_summary(
                &json!({"type": "message", "message": {"role": "bashExecution", "command": "cargo test"}})
            ),
            "$ cargo test"
        );
        assert_eq!(
            tree_node_summary(
                &json!({"type": "model_change", "provider": "anthropic", "modelId": "claude-sonnet-4-5"})
            ),
            "model → anthropic/claude-sonnet-4-5"
        );
        assert_eq!(
            tree_node_summary(&json!({"type": "thinking_level_change", "thinkingLevel": "high"})),
            "thinking → high"
        );
        assert_eq!(
            tree_node_summary(&json!({"type": "compaction"})),
            "context compacted"
        );
        assert_eq!(
            tree_node_summary(&json!({"type": "session_info", "name": "my-feature"})),
            "renamed: my-feature"
        );
    }

    #[test]
    fn long_user_message_is_elided() {
        let long = "a ".repeat(60);
        let summary = tree_node_summary(
            &json!({"type": "message", "message": {"role": "user", "content": long}}),
        );
        assert!(summary.ends_with('…'));
        assert!(summary.chars().count() <= 71);
    }
}

async fn refresh_models(client: &PiClient, transcript: &mut Transcript) -> ModelsState {
    let mut entries = Vec::new();
    let mut labels = Vec::new();
    match client.get_available_models().await {
        Ok(data) => {
            if let Some(models) = data.get("models").and_then(|m| m.as_array()) {
                for m in models {
                    let provider = m.get("provider").and_then(|v| v.as_str()).unwrap_or("");
                    let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let base_url = m.get("baseUrl").and_then(|v| v.as_str()).unwrap_or("");
                    entries.push(ModelEntry {
                        provider: provider.to_string(),
                        id: id.to_string(),
                        base_url: base_url.to_string(),
                        is_local: is_local_base_url(base_url),
                    });
                    labels.push(model_label(m));
                }
            }
        }
        Err(e) => transcript.note("error", format!("could not list models: {e}")),
    }
    let current = match client.get_state().await {
        Ok(state) => {
            let id = state.pointer("/model/id").and_then(|v| v.as_str());
            let provider = state.pointer("/model/provider").and_then(|v| v.as_str());
            entries
                .iter()
                .position(|e| Some(e.id.as_str()) == id && Some(e.provider.as_str()) == provider)
                .map(|i| i as i32)
                .unwrap_or(-1)
        }
        Err(_) => -1,
    };
    transcript.ui.set_models(labels.clone(), current);
    ModelsState {
        entries,
        labels,
        current,
    }
}

/// Status-bar dot state for the active model: hidden unless the model is
/// served from this machine. rapid-mlx providers get a real `/health`
/// probe — reachable is not enough there, because a rapid-mlx server 404s
/// every completion whose model id isn't the one it serves (verified live
/// against 0.11.3), so a served-model mismatch gets its own state. Other
/// local providers keep the generic 1s TCP probe; a managed child's
/// process state breaks ties when the port doesn't answer.
async fn compute_server_dot(
    current: Option<&ModelEntry>,
    managed: &mut Option<ManagedRapidMlx>,
) -> i32 {
    let Some(entry) = current.filter(|e| e.is_local) else {
        return SERVER_DOT_HIDDEN;
    };
    if entry.provider == "rapid-mlx" {
        let health = local::rapid_mlx::server_health(&entry.base_url).await;
        return classify_rapid_mlx_dot(
            &entry.id,
            health.as_ref(),
            managed.as_mut().map(|m| m.server.is_alive()),
        );
    }
    if probe_tcp(&entry.base_url).await {
        SERVER_DOT_OK
    } else {
        SERVER_DOT_DOWN
    }
}

/// Pure classification for the rapid-mlx branch of the dot, so the
/// mismatch/down/ok truth table is unit-testable without a live server.
/// `managed_alive` is `Some` only when this app spawned the server.
fn classify_rapid_mlx_dot(
    current_model_id: &str,
    health: Option<&local::rapid_mlx::ServerHealth>,
    managed_alive: Option<bool>,
) -> i32 {
    match health {
        Some(h) => {
            let serves_current = h.model_name.as_deref() == Some(current_model_id);
            if serves_current && h.ready && h.model_loaded {
                SERVER_DOT_OK
            } else if serves_current {
                // Right model, still coming up.
                SERVER_DOT_DOWN
            } else {
                SERVER_DOT_MISMATCH
            }
        }
        // No health response: a live managed child is presumably still
        // binding its port; anything else is down.
        None => match managed_alive {
            Some(true) => SERVER_DOT_OK,
            _ => SERVER_DOT_DOWN,
        },
    }
}

/// One-shot TCP connect to a base URL's host:port, capped at 1s. Cheaper
/// and more universal than an HTTP health endpoint (which local servers
/// don't uniformly have) — "the port answers" is the honesty level the dot
/// promises.
async fn probe_tcp(base_url: &str) -> bool {
    let https = base_url.starts_with("https://");
    let rest = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))
        .unwrap_or(base_url);
    let hostport = rest.split('/').next().unwrap_or("");
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().unwrap_or(if https { 443 } else { 80 })),
        None => (hostport, if https { 443 } else { 80 }),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

/// Whether a provider `baseUrl` points at this machine — the local/cloud
/// split behind the picker badge and the status-bar server dot.
fn is_local_base_url(base_url: &str) -> bool {
    let rest = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))
        .unwrap_or(base_url);
    let host = rest
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or_else(|| rest.split('/').next().unwrap_or(""));
    matches!(
        host,
        "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]" | "::1"
    )
}

/// Picker label for one of pi's `Model` objects: name, provider, and either
/// "free · local" (local endpoint) or the per-Mtok in/out price when known.
fn model_label(m: &serde_json::Value) -> String {
    let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let name = m.get("name").and_then(|v| v.as_str()).unwrap_or(id);
    let provider = m.get("provider").and_then(|v| v.as_str()).unwrap_or("");
    let base_url = m.get("baseUrl").and_then(|v| v.as_str()).unwrap_or("");
    let mut label = if provider.is_empty() {
        name.to_string()
    } else {
        format!("{name} · {provider}")
    };
    if is_local_base_url(base_url) {
        label.push_str(" · free · local");
    } else {
        let price = |key: &str| m.pointer(&format!("/cost/{key}")).and_then(|v| v.as_f64());
        if let (Some(input), Some(output)) = (price("input"), price("output")) {
            // Trim trailing zeros: 3.0 → "$3", 0.25 → "$0.25".
            let fmt = |v: f64| {
                let s = format!("{v:.2}");
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            };
            label.push_str(&format!(" · ${}/${}", fmt(input), fmt(output)));
        }
    }
    label
}

async fn refresh_thinking(client: &PiClient, transcript: &mut Transcript) -> Vec<ThinkingLevel> {
    use ThinkingLevel::*;
    let mut levels = Vec::new();
    let mut labels = Vec::new();
    if let Ok(data) = client.request(Command::GetAvailableThinkingLevels).await {
        if let Some(list) = data
            .as_ref()
            .and_then(|d| d.get("levels"))
            .and_then(|l| l.as_array())
        {
            for l in list {
                if let Some(s) = l.as_str() {
                    let level = match s {
                        "off" => Off,
                        "minimal" => Minimal,
                        "low" => Low,
                        "medium" => Medium,
                        "high" => High,
                        "xhigh" => Xhigh,
                        "max" => Max,
                        _ => continue,
                    };
                    levels.push(level);
                    labels.push(format!("think: {s}"));
                }
            }
        }
    }
    let current = match client.get_state().await {
        Ok(state) => state
            .get("thinkingLevel")
            .and_then(|v| v.as_str())
            .and_then(|s| labels.iter().position(|l| l == &format!("think: {s}")))
            .map(|i| i as i32)
            .unwrap_or(-1),
        Err(_) => -1,
    };
    transcript.ui.set_thinking(labels, current);
    levels
}

async fn update_stats(client: &PiClient, transcript: &Transcript) {
    if let Ok(stats) = client.get_session_stats().await {
        let cost = stats.get("cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let tokens = stats
            .pointer("/tokens/total")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let percent = stats
            .pointer("/contextUsage/percent")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        transcript.ui.set_context_percent(percent as f32);
        transcript
            .ui
            .set_status(format!("{} tok · ${cost:.4}", format_tokens(tokens)));
    }
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn handle_extension_ui(
    client: &PiClient,
    transcript: &mut Transcript,
    req: &pi_rpc::ExtensionUiRequest,
) {
    match req.method.as_str() {
        "select" | "confirm" | "input" | "editor" => {
            let title = req.title.as_deref().unwrap_or(&req.method);
            transcript.note(
                "info",
                format!("extension dialog \"{title}\" auto-dismissed (dialogs land in M4)"),
            );
            let _ = client.reply_extension_ui(&req.id, ExtensionUiReply::Cancelled);
        }
        "notify" => {
            if let Some(msg) = &req.message {
                transcript.note("info", msg.clone());
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Demo backend: synthesizes the same events the pi backend consumes, so the
// entire rendering path is exercised without pi. Perf harness for M1.
// ---------------------------------------------------------------------------

const DEMO_MARKDOWN: &str = "\
Here's what a **rich** streamed answer looks like, with `inline code`, \
a [link](https://pi.dev), and a list:\n\n\
- streaming markdown segmentation\n\
- syntax-highlighted code\n\n\
## A code sample\n\n\
```rust\n\
fn main() {\n    \
let greeting = \"hello, slint\";\n    \
println!(\"{greeting}\");\n\
}\n\
```\n\n\
A paragraph between the code block and a table, to verify segment ordering \
holds up while chunks arrive mid-token.\n\n\
| Language | Highlight |\n\
| --- | --- |\n\
| Rust | syntect spans |\n\
| Markdown | tables |\n\n\
And a closing paragraph after the table. ";

pub async fn demo_backend(
    ui: Box<dyn UiSink>,
    dark: Arc<AtomicBool>,
    mut cmd_rx: mpsc::UnboundedReceiver<UiCmd>,
) {
    let mut transcript = Transcript::new(ui, dark);
    // Through the real `model_label` (not a hand-written string) so the demo
    // exercises the same badge formatting the live picker uses.
    transcript.ui.set_models(
        vec![model_label(&serde_json::json!({
            "id": "demo-model",
            "name": "demo model",
            "provider": "synthetic",
            "baseUrl": "http://localhost:0",
        }))],
        0,
    );
    transcript.ui.set_server_dot(SERVER_DOT_OK);

    // Sessions/hydration are demoable without pi: the sidebar lists
    // pi-sessions' own test fixtures (guaranteed to match the real on-disk
    // format), and switching between them loads directly off disk via
    // `pi_sessions::load_session` instead of a `get_messages` RPC call,
    // since there's no child process here to serve one.
    let demo_project = demo_sessions::setup();
    tracing::debug!(
        sessions_root = %demo_project.sessions_root.display(),
        cwd = %demo_project.cwd.display(),
        "demo: synthesized session dir"
    );
    let mut sidebar = Sidebar::new();
    sidebar.sessions_root = Some(demo_project.sessions_root.clone());
    sidebar.cwd = Some(demo_project.cwd.clone());
    sidebar.refresh_projects(transcript.ui.as_ref());
    let mut current_session: Option<String> = None;
    sidebar.refresh_sessions_with_active(current_session.as_deref(), transcript.ui.as_ref());
    let mut demo_router_entries = seed_demo_router_entries();
    transcript
        .ui
        .set_auth_entries(format_auth_entries(&seed_demo_auth_entries()));

    let rate: u64 = std::env::var("SLINTY_DEMO_RATE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let repeats: usize = std::env::var("SLINTY_DEMO_REPEATS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);

    let mut next: Option<UiCmd> = std::env::var("SLINTY_DEMO_AUTOSEND").ok().map(UiCmd::Send);
    loop {
        let cmd = match next.take() {
            Some(cmd) => cmd,
            None => match cmd_rx.recv().await {
                Some(cmd) => cmd,
                None => break,
            },
        };
        let text = match cmd {
            UiCmd::Send(text) => text,
            UiCmd::SwitchSession(path) => {
                let messages = demo_sessions::hydrate_messages(Path::new(&path));
                tracing::debug!(messages = messages.len(), "demo: switched session");
                transcript.reset();
                transcript.hydrate(&messages);
                current_session = Some(path);
                sidebar.refresh_sessions_with_active(
                    current_session.as_deref(),
                    transcript.ui.as_ref(),
                );
                continue;
            }
            UiCmd::SidebarSearch(query) => {
                sidebar.query = query;
                sidebar.refresh_sessions_with_active(
                    current_session.as_deref(),
                    transcript.ui.as_ref(),
                );
                continue;
            }
            UiCmd::NewSession
            | UiCmd::SwitchProject(_)
            | UiCmd::DeleteSession(_)
            | UiCmd::RenameSession(_)
            | UiCmd::CloneSession => {
                transcript.note("info", "not available in demo mode");
                continue;
            }
            // Both sections are seeded fakes here (not live-probed), so the
            // panel is demoable offline and deterministic — see the
            // "Models panel: demo-mode fakes" section below.
            UiCmd::OpenModels | UiCmd::ServerDotClicked => {
                render_demo_models_panel(&mut transcript, &demo_router_entries).await;
                continue;
            }
            UiCmd::ServeRapidMlxModel(_) => {
                transcript.note("info", "not available in demo mode");
                continue;
            }
            UiCmd::LoadRouterModel(model) => {
                simulate_router_load(&mut transcript, &mut demo_router_entries, &model).await;
                continue;
            }
            UiCmd::UnloadRouterModel(model) => {
                simulate_router_unload(&mut transcript, &mut demo_router_entries, &model).await;
                continue;
            }
            UiCmd::SearchHfModels(query) => {
                let results = if query.trim().is_empty() {
                    Vec::new()
                } else {
                    format_hf_results(seed_demo_hf_results())
                };
                transcript.ui.set_hf_search_results(results);
                continue;
            }
            UiCmd::DownloadRouterModel(model) => {
                simulate_router_download(&mut transcript, &mut demo_router_entries, &model).await;
                continue;
            }
            UiCmd::AddOllamaToPi => {
                transcript.note("info", "not available in demo mode");
                continue;
            }
            // Demo never touches the real ~/.pi/agent/auth.json.
            UiCmd::SaveApiKey { .. } => {
                transcript.note("info", "not available in demo mode");
                continue;
            }
            _ => continue,
        };

        // Test hook: a message starting with `md!` streams the remainder of
        // the message itself as the assistant markdown, so rendering can be
        // exercised with arbitrary input (via MCP or by typing) without
        // rebuilding DEMO_MARKDOWN.
        let (source, repeats) = match text.strip_prefix("md!") {
            Some(md) => (md.trim_start().to_string(), 1),
            None => (DEMO_MARKDOWN.to_string(), repeats),
        };

        transcript.user_prompt(&text, false);
        transcript.apply(&Event::AgentStart);
        transcript.apply(&Event::MessageStart {
            message: serde_json::json!({"role": "assistant"}),
        });

        // Brief thinking phase.
        transcript.apply(&mk_delta(AssistantMessageEvent::ThinkingStart));
        for chunk in chunks("Considering how to demo the transcript…", 6) {
            transcript.apply(&mk_delta(AssistantMessageEvent::ThinkingDelta {
                delta: chunk.to_string(),
            }));
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        transcript.apply(&mk_delta(AssistantMessageEvent::ThinkingEnd));

        // Streamed markdown at the configured token rate.
        let start = Instant::now();
        let mut chars = 0usize;
        transcript.apply(&mk_delta(AssistantMessageEvent::TextStart {
            content_index: 0,
        }));
        let mut interval = tokio::time::interval(Duration::from_micros(1_000_000 / rate.max(1)));
        let mut aborted = false;
        'outer: for _ in 0..repeats {
            for chunk in chunks(&source, 5) {
                tokio::select! {
                    _ = interval.tick() => {
                        chars += chunk.len();
                        transcript.apply(&mk_delta(AssistantMessageEvent::TextDelta {
                            content_index: 0,
                            delta: chunk.to_string(),
                        }));
                    }
                    cmd = cmd_rx.recv() => {
                        if matches!(cmd, Some(UiCmd::Abort) | None) {
                            aborted = true;
                            break 'outer;
                        }
                    }
                }
            }
        }
        transcript.apply(&mk_delta(AssistantMessageEvent::TextEnd {
            content_index: 0,
            content: String::new(),
        }));

        // A demo tool chip.
        if !aborted {
            transcript.apply(&Event::ToolExecutionStart {
                tool_call_id: "demo-1".into(),
                tool_name: "bash".into(),
                args: serde_json::json!({"command": "cargo test -p slinty-pi"}),
            });
            tokio::time::sleep(Duration::from_millis(600)).await;
            transcript.apply(&Event::ToolExecutionEnd {
                tool_call_id: "demo-1".into(),
                tool_name: "bash".into(),
                result: serde_json::json!({"content": [
                    {"type": "text", "text": "test result: ok. 13 passed; 0 failed"}
                ]}),
                is_error: false,
            });
        }

        transcript.apply(&Event::AgentSettled);
        let elapsed = start.elapsed().as_secs_f64();
        transcript.ui.set_status(format!(
            "{chars} chars in {elapsed:.1}s ({rate} tok/s target){}",
            if aborted { " · aborted" } else { "" }
        ));
    }
}

fn mk_delta(delta: AssistantMessageEvent) -> Event {
    Event::MessageUpdate {
        message: serde_json::Value::Null,
        assistant_message_event: delta,
    }
}

/// Split into ~n-char chunks on char boundaries (crude token simulation).
fn chunks(s: &str, n: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut count = 0;
    for (i, _) in s.char_indices() {
        if count == n {
            out.push(&s[start..i]);
            start = i;
            count = 0;
        }
        count += 1;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
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
        assert!(specs[1].first, "thinking row starts the assistant group");
        assert!(!specs[2].first);
        assert_eq!(specs[0].text, "hello");
        assert_eq!(specs[1].text, "pondering");
        assert!(
            !specs[1].running,
            "hydrated thinking is never still-running"
        );
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
