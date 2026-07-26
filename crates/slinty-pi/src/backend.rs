//! Backend: owns the pi child process (or the demo synthesizer) and projects
//! agent events onto the transcript model.
//!
//! Threading contract: this module runs on tokio worker threads; every UI
//! touch goes through `Weak::upgrade_in_event_loop`. Closures are executed on
//! the Slint thread in submission order, so the backend tracks a shadow row
//! count to address rows it appended without reading UI state back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{Model, ModelRc, SharedString, StyledText, VecModel, Weak};
use tokio::sync::mpsc;

use pi_rpc::{
    content_text, AssistantMessageEvent, Command, Event, ExtensionUiReply, ImageContent, PiClient,
    PiError, PiOptions, ThinkingLevel,
};

use crate::attach;
use crate::demo_sessions;
use crate::palette;
use crate::segmenter::{segment_markdown, Segment};
use crate::{highlight, AppWindow, PaletteRow, QueueItem, Row, SessionRow, TreeRow};

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
    /// Sidebar search box changed; re-filter the session list.
    SidebarSearch(String),
    /// Fetch and display the active session's branch tree.
    OpenTree,
    /// Fork the active session from a prior user message (by entry id).
    ForkFrom(String),
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
}

// ---------------------------------------------------------------------------
// Row specs: Send-able row descriptions, turned into `Row` on the UI thread.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct RowSpec {
    kind: &'static str,
    /// Markdown for the styled field (prose, or colored code markdown).
    markdown: Option<String>,
    /// Plain-text fallback if `markdown` fails to parse (raw code).
    fallback: Option<String>,
    text: String,
    lang: String,
    level: i32,
    detail: String,
    running: bool,
    elapsed: String,
    first: bool,
    /// The full original markdown of this row's enclosing message/text
    /// block, shared by every row segmented out of it — used for the
    /// per-message copy affordance, which copies the source text rather
    /// than any one rendered/segmented piece of it. Empty where a group
    /// copy isn't offered (thinking/tool/info rows).
    raw: String,
}

impl RowSpec {
    fn note(kind: &'static str, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            ..Self::default()
        }
    }

    fn to_row(&self) -> Row {
        let styled = match &self.markdown {
            Some(md) => StyledText::from_markdown(md).unwrap_or_else(|_| {
                StyledText::from_plain_text(self.fallback.as_deref().unwrap_or(md))
            }),
            None => StyledText::default(),
        };
        Row {
            kind: self.kind.into(),
            styled,
            text: self.text.as_str().into(),
            lang: self.lang.as_str().into(),
            level: self.level,
            expanded: false,
            detail: self.detail.as_str().into(),
            running: self.running,
            elapsed: self.elapsed.as_str().into(),
            first: self.first,
            raw: self.raw.as_str().into(),
        }
    }
}

// ---------------------------------------------------------------------------
// UI handle
// ---------------------------------------------------------------------------

pub struct Ui {
    weak: Weak<AppWindow>,
    rows: usize,
    pub dark: Arc<AtomicBool>,
}

impl Ui {
    pub fn new(weak: Weak<AppWindow>, dark: Arc<AtomicBool>) -> Self {
        Self {
            weak,
            rows: 0,
            dark,
        }
    }

    fn with_app(&self, f: impl FnOnce(&AppWindow) + Send + 'static) {
        let _ = self.weak.upgrade_in_event_loop(move |app| f(&app));
    }

    fn with_transcript(&self, f: impl FnOnce(&AppWindow, &VecModel<Row>) + Send + 'static) {
        self.with_app(move |app| {
            let model = app.get_transcript();
            let model = model
                .as_any()
                .downcast_ref::<VecModel<Row>>()
                .expect("transcript is a VecModel");
            f(app, model);
        });
    }

    fn push(&mut self, spec: RowSpec) -> usize {
        let index = self.rows;
        self.rows += 1;
        self.with_transcript(move |app, model| {
            model.push(spec.to_row());
            app.invoke_scroll_to_end();
        });
        index
    }

    /// Replace a row, preserving its user-toggled expansion state.
    fn set(&self, index: usize, spec: RowSpec) {
        self.with_transcript(move |app, model| {
            if index < model.row_count() {
                let mut row = spec.to_row();
                if let Some(old) = model.row_data(index) {
                    row.expanded = old.expanded;
                }
                model.set_row_data(index, row);
                app.invoke_scroll_to_end();
            }
        });
    }

    /// Append many rows in a handful of event-loop hops instead of one per
    /// row, so hydrating a large session stays responsive.
    fn push_all(&mut self, specs: Vec<RowSpec>) {
        const BATCH: usize = 100;
        self.rows += specs.len();
        let mut iter = specs.into_iter().peekable();
        while iter.peek().is_some() {
            let chunk: Vec<RowSpec> = iter.by_ref().take(BATCH).collect();
            self.with_transcript(move |app, model| {
                for spec in &chunk {
                    model.push(spec.to_row());
                }
                app.invoke_scroll_to_end();
            });
        }
    }

    fn clear(&mut self) {
        self.rows = 0;
        self.with_transcript(|_, model| {
            while model.row_count() > 0 {
                model.remove(model.row_count() - 1);
            }
        });
    }

    fn truncate(&mut self, len: usize) {
        self.rows = len;
        self.with_transcript(move |_, model| {
            while model.row_count() > len {
                model.remove(model.row_count() - 1);
            }
        });
    }

    fn set_streaming(&self, streaming: bool) {
        self.with_app(move |app| app.set_streaming(streaming));
    }

    fn set_status(&self, status: String) {
        self.with_app(move |app| app.set_status_text(SharedString::from(status)));
    }

    fn set_context_percent(&self, percent: f32) {
        self.with_app(move |app| app.set_context_percent(percent));
    }

    fn set_queue(&self, items: Vec<(&'static str, String)>) {
        self.with_app(move |app| {
            let rows: Vec<QueueItem> = items
                .into_iter()
                .map(|(kind, text)| QueueItem {
                    kind: kind.into(),
                    text: text.as_str().into(),
                })
                .collect();
            app.set_queue(ModelRc::new(VecModel::from(rows)));
        });
    }

    fn set_models(&self, labels: Vec<String>, index: i32) {
        self.with_app(move |app| {
            let labels: Vec<SharedString> = labels.iter().map(|l| l.as_str().into()).collect();
            app.set_model_list(ModelRc::new(VecModel::from(labels)));
            app.set_model_index(index);
        });
    }

    fn set_thinking(&self, labels: Vec<String>, index: i32) {
        self.with_app(move |app| {
            let labels: Vec<SharedString> = labels.iter().map(|l| l.as_str().into()).collect();
            app.set_thinking_list(ModelRc::new(VecModel::from(labels)));
            app.set_thinking_index(index);
        });
    }

    /// `labels`/`paths` are parallel arrays (projects other than the current
    /// one); Slint resolves the picked label back to a path itself, so
    /// there's no index bookkeeping to keep in sync on the Rust side.
    fn set_projects(&self, labels: Vec<String>, paths: Vec<String>, current_name: String) {
        self.with_app(move |app| {
            let label_model: Vec<SharedString> = labels.iter().map(|l| l.as_str().into()).collect();
            let path_model: Vec<SharedString> = paths.iter().map(|p| p.as_str().into()).collect();
            app.set_project_list(ModelRc::new(VecModel::from(label_model)));
            app.set_project_paths(ModelRc::new(VecModel::from(path_model)));
            app.set_project_index(-1);
            app.set_current_project_name(SharedString::from(current_name));
        });
    }

    /// `rows` are `(path, title, relative_time, active)`.
    fn set_sidebar_sessions(&self, rows: Vec<(String, String, String, bool)>) {
        self.with_app(move |app| {
            let rows: Vec<SessionRow> = rows
                .into_iter()
                .map(|(path, title, relative_time, active)| SessionRow {
                    path: path.as_str().into(),
                    title: title.as_str().into(),
                    relative_time: relative_time.as_str().into(),
                    active,
                })
                .collect();
            app.set_sidebar_sessions(ModelRc::new(VecModel::from(rows)));
        });
    }

    /// `rows` are `(id, depth, summary, label, can_fork, active)`. Opens the
    /// overlay once the (freshly-fetched) rows land.
    fn set_tree(&self, rows: Vec<(String, i32, String, String, bool, bool)>) {
        self.with_app(move |app| {
            let rows: Vec<TreeRow> = rows
                .into_iter()
                .map(|(id, depth, summary, label, can_fork, active)| TreeRow {
                    id: id.as_str().into(),
                    depth,
                    summary: summary.as_str().into(),
                    label: label.as_str().into(),
                    can_fork,
                    active,
                })
                .collect();
            app.set_tree_rows(ModelRc::new(VecModel::from(rows)));
            app.set_tree_visible(true);
        });
    }

    fn set_palette_entries(&self, entries: Vec<palette::PaletteEntry>) {
        self.with_app(move |app| {
            let rows: Vec<PaletteRow> = entries
                .into_iter()
                .map(|e| PaletteRow {
                    id: e.id.as_str().into(),
                    kind: e.kind.into(),
                    label: e.label.as_str().into(),
                    detail: e.detail.as_str().into(),
                })
                .collect();
            app.set_palette_entries(ModelRc::new(VecModel::from(rows)));
        });
    }

    /// Prefill the composer (used after a fork, which hands back the text of
    /// the message it forked from instead of keeping it in context).
    fn set_composer_text(&self, text: String) {
        self.with_app(move |app| {
            app.invoke_set_composer_text(text.as_str().into());
        });
    }

    /// Append `@path` to the composer (non-image attachment); Slint owns
    /// the spacing, since it has the current text and this doesn't.
    fn append_composer_text(&self, path: &Path) {
        let text = format!("@{}", path.display());
        self.with_app(move |app| {
            app.invoke_append_to_composer(text.as_str().into());
        });
    }

    /// Chip labels (file names) for queued image attachments.
    fn set_pending_attachments(&self, names: Vec<String>) {
        self.with_app(move |app| {
            let rows: Vec<SharedString> = names.into_iter().map(|n| n.as_str().into()).collect();
            app.set_pending_attachments(ModelRc::new(VecModel::from(rows)));
        });
    }
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
    ui: Ui,
    stream: Option<StreamRegion>,
    thinking: Option<ThinkingRegion>,
    tools: HashMap<String, ToolRun>,
    /// The next content row starts a new visual message group.
    pending_first: bool,
}

impl Transcript {
    pub fn new(ui: Ui) -> Self {
        Self {
            ui,
            stream: None,
            thinking: None,
            tools: HashMap::new(),
            pending_first: false,
        }
    }

    /// Drop all live-stream/tool state and clear the transcript, in
    /// preparation for hydrating a different session.
    pub fn reset(&mut self) {
        self.stream = None;
        self.thinking = None;
        self.tools.clear();
        self.pending_first = false;
        self.ui.clear();
    }

    /// Render a session's full message history (as returned by `get_messages`
    /// after a `switch_session`) without replaying events. Appends to
    /// whatever is already in the transcript — call [`Transcript::reset`]
    /// first when switching sessions.
    pub fn hydrate(&mut self, messages: &[serde_json::Value]) {
        let dark = self.ui.dark.load(Ordering::Relaxed);
        let specs = hydrate_rowspecs(messages, dark);
        self.ui.push_all(specs);
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
        self.ui.push(spec);
        self.pending_first = true;
    }

    pub fn note(&mut self, kind: &'static str, text: impl Into<String>) {
        self.ui.push(RowSpec::note(kind, text));
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
        let dark = self.ui.dark.load(Ordering::Relaxed);
        let segments = segment_markdown(&region.buffer);
        for (i, segment) in segments.iter().enumerate() {
            if region.prev.get(i) == Some(segment) {
                continue;
            }
            let spec = spec_for_segment(segment, region.first && i == 0, dark, &region.buffer);
            let index = region.start + i;
            if index < self.ui.rows {
                self.ui.set(index, spec);
            } else {
                self.ui.push(spec);
            }
        }
        if segments.len() < region.prev.len() {
            self.ui.truncate(region.start + segments.len());
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
                    start: self.ui.rows,
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
                let row = self.ui.push(RowSpec {
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
        let row = self.ui.push(RowSpec {
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
            markdown: Some(highlight::code_markdown(code, lang, dark)),
            fallback: Some(code.clone()),
            text: code.clone(),
            lang: lang.clone(),
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
}

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

    fn refresh_projects(&mut self, ui: &Ui) {
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

    async fn refresh_sessions(&self, client: &PiClient, ui: &Ui) {
        let active = active_session_path(client).await;
        self.refresh_sessions_with_active(active.as_deref(), ui);
    }

    /// The synchronous part of `refresh_sessions`, split out so demo mode
    /// (no `PiClient` to ask `get_state` for the active session) can drive
    /// it with a locally-tracked path instead.
    fn refresh_sessions_with_active(&self, active: Option<&str>, ui: &Ui) {
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
        let rows = filtered
            .into_iter()
            .map(|m| {
                let path = m.path.to_string_lossy().into_owned();
                let is_active = active == Some(path.as_str());
                (
                    path,
                    m.title().to_string(),
                    relative_time(&m.last_timestamp),
                    is_active,
                )
            })
            .collect();
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
    weak: Weak<AppWindow>,
    dark: Arc<AtomicBool>,
    mut cmd_rx: mpsc::UnboundedReceiver<UiCmd>,
) {
    let ui = Ui::new(weak, dark);
    let mut transcript = Transcript::new(ui);
    let mut sidebar = Sidebar::new();
    let mut cwd = std::env::current_dir().ok();
    // A session picker lands with the sidebar (M2); until then, resuming a
    // specific session on startup is reachable for testing/scripting via env
    // var, the same way `SLINTY_DEMO*` gates the demo backend. Only applies
    // to the very first child: it names a session under the *initial* cwd,
    // which a later project switch would leave behind.
    let mut resume_on_first_spawn = std::env::var("SLINTY_RESUME_SESSION").ok();

    loop {
        sidebar.cwd = cwd.clone();
        sidebar.query.clear();

        let opts = PiOptions {
            cwd: cwd.clone(),
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

        if let Some(path) = resume_on_first_spawn.take() {
            resume_session(&client, &mut transcript, &path).await;
        }

        sidebar.refresh_projects(&transcript.ui);
        sidebar.refresh_sessions(&client, &transcript.ui).await;

        match run_session(&client, events, &mut cmd_rx, &mut transcript, &mut sidebar).await {
            SessionOutcome::SwitchProject(path) => {
                cwd = Some(path);
            }
            SessionOutcome::Exit => return,
        }
        // `client` drops here; `kill_on_drop` reaps the old child before the
        // next loop iteration spawns its replacement.
    }
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
    transcript: &mut Transcript,
    sidebar: &mut Sidebar,
) -> SessionOutcome {
    let models = refresh_models(client, transcript).await;
    let mut thinking_levels = refresh_thinking(client, transcript).await;
    let mut streaming = false;
    let mut palette_entries: Vec<palette::PaletteEntry> = Vec::new();
    // (display name, encoded image) pairs queued for the next non-streaming
    // `Send`.
    let mut pending_images: Vec<(String, ImageContent)> = Vec::new();

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
                                    thinking_levels =
                                        refresh_thinking(client, transcript).await;
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
                        sidebar.refresh_sessions(client, &transcript.ui).await;
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
                        sidebar.refresh_sessions(client, &transcript.ui).await;
                    }
                    UiCmd::DeleteSession(path) => {
                        delete_session(client, transcript, sidebar, &path).await;
                    }
                    UiCmd::SidebarSearch(query) => {
                        sidebar.query = query;
                        sidebar.refresh_sessions(client, &transcript.ui).await;
                    }
                    UiCmd::OpenTree => {
                        match fetch_tree_rows(client).await {
                            Ok(rows) => transcript.ui.set_tree(rows),
                            Err(e) => transcript.note("error", format!("could not load tree: {e}")),
                        }
                    }
                    UiCmd::ForkFrom(entry_id) => {
                        fork_from(client, transcript, &entry_id).await;
                        sidebar.refresh_sessions(client, &transcript.ui).await;
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
                        palette_entries = palette::build_entries(&sessions, &commands);
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
                    sidebar.refresh_sessions(client, &transcript.ui).await;
                }
                if let Event::ExtensionUiRequest(req) = &event {
                    handle_extension_ui(client, transcript, req);
                }
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
    sidebar.refresh_sessions(client, &transcript.ui).await;
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

async fn refresh_models(client: &PiClient, transcript: &mut Transcript) -> Vec<ModelEntry> {
    let mut entries = Vec::new();
    let mut labels = Vec::new();
    match client.get_available_models().await {
        Ok(data) => {
            if let Some(models) = data.get("models").and_then(|m| m.as_array()) {
                for m in models {
                    let provider = m.get("provider").and_then(|v| v.as_str()).unwrap_or("");
                    let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let name = m.get("name").and_then(|v| v.as_str()).unwrap_or(id);
                    entries.push(ModelEntry {
                        provider: provider.to_string(),
                        id: id.to_string(),
                    });
                    labels.push(format!("{name} · {provider}"));
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
    transcript.ui.set_models(labels, current);
    entries
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
fn main() {\n\
    let greeting = \"hello, slint\";\n\
    println!(\"{greeting}\");\n\
}\n\
```\n\n\
And a closing paragraph after the code block to verify segment ordering \
holds up while chunks arrive mid-token. ";

pub async fn demo_backend(
    weak: Weak<AppWindow>,
    dark: Arc<AtomicBool>,
    mut cmd_rx: mpsc::UnboundedReceiver<UiCmd>,
) {
    let ui = Ui::new(weak, dark);
    let mut transcript = Transcript::new(ui);
    transcript
        .ui
        .set_models(vec!["demo model · synthetic".into()], 0);

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
    sidebar.refresh_projects(&transcript.ui);
    let mut current_session: Option<String> = None;
    sidebar.refresh_sessions_with_active(current_session.as_deref(), &transcript.ui);

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
                sidebar.refresh_sessions_with_active(current_session.as_deref(), &transcript.ui);
                continue;
            }
            UiCmd::SidebarSearch(query) => {
                sidebar.query = query;
                sidebar.refresh_sessions_with_active(current_session.as_deref(), &transcript.ui);
                continue;
            }
            UiCmd::NewSession | UiCmd::SwitchProject(_) | UiCmd::DeleteSession(_) => {
                transcript.note("info", "not available in demo mode");
                continue;
            }
            _ => continue,
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
            for chunk in chunks(DEMO_MARKDOWN, 5) {
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
