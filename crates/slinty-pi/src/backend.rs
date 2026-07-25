//! Backend: owns the pi child process (or the demo synthesizer) and projects
//! agent events onto the transcript model.
//!
//! Threading contract: this module runs on tokio worker threads; every UI
//! touch goes through `Weak::upgrade_in_event_loop`. Closures are executed on
//! the Slint thread in submission order, so the backend tracks a shadow row
//! count to address rows it appended without reading UI state back.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{Model, ModelRc, SharedString, StyledText, VecModel, Weak};
use tokio::sync::mpsc;

use pi_rpc::{
    content_text, AssistantMessageEvent, Command, Event, ExtensionUiReply, PiClient, PiOptions,
    ThinkingLevel,
};

use crate::segmenter::{segment_markdown, Segment};
use crate::{highlight, AppWindow, QueueItem, Row};

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

    pub fn user_prompt(&mut self, text: &str, steering: bool) {
        let display = if steering {
            format!("↪ {text}")
        } else {
            text.to_string()
        };
        let mut spec = RowSpec::note("user", display);
        spec.first = true;
        self.ui.push(spec);
        self.pending_first = true;
    }

    pub fn note(&mut self, kind: &'static str, text: impl Into<String>) {
        self.ui.push(RowSpec::note(kind, text));
    }

    fn spec_for_segment(&self, segment: &Segment, first: bool) -> RowSpec {
        let dark = self.ui.dark.load(Ordering::Relaxed);
        match segment {
            Segment::Prose(md) => RowSpec {
                kind: "prose",
                markdown: Some(md.clone()),
                first,
                ..RowSpec::default()
            },
            Segment::Heading { level, text } => RowSpec {
                kind: "heading",
                text: text.clone(),
                level: *level as i32,
                first,
                ..RowSpec::default()
            },
            Segment::Code { lang, code } => RowSpec {
                kind: "code",
                markdown: Some(highlight::code_markdown(code, lang, dark)),
                fallback: Some(code.clone()),
                text: code.clone(),
                lang: lang.clone(),
                first,
                ..RowSpec::default()
            },
        }
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
        let segments = segment_markdown(&region.buffer);
        for (i, segment) in segments.iter().enumerate() {
            if region.prev.get(i) == Some(segment) {
                continue;
            }
            let spec = self.spec_for_segment(segment, region.first && i == 0);
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
        let row = self.ui.push(RowSpec {
            kind: "tool",
            text: format!("⚙ {summary}"),
            detail: args_pretty.clone(),
            running: true,
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
// pi backend
// ---------------------------------------------------------------------------

struct ModelEntry {
    provider: String,
    id: String,
}

pub async fn pi_backend(
    weak: Weak<AppWindow>,
    dark: Arc<AtomicBool>,
    mut cmd_rx: mpsc::UnboundedReceiver<UiCmd>,
) {
    let ui = Ui::new(weak, dark);
    let mut transcript = Transcript::new(ui);

    let opts = PiOptions {
        cwd: std::env::current_dir().ok(),
        ..Default::default()
    };
    let (client, mut events) = match PiClient::spawn(opts).await {
        Ok(pair) => pair,
        Err(e) => {
            transcript.note(
                "error",
                format!(
                    "Failed to start pi: {e}\nIs `pi` on your PATH? \
                     Install: npm install -g @earendil-works/pi-coding-agent"
                ),
            );
            return;
        }
    };

    let models = refresh_models(&client, &mut transcript).await;
    let mut thinking_levels = refresh_thinking(&client, &mut transcript).await;
    let mut streaming = false;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    UiCmd::Send(text) => {
                        transcript.user_prompt(&text, streaming);
                        let result = if streaming {
                            client.prompt_steering(&text).await
                        } else {
                            client.prompt(&text).await
                        };
                        if let Err(e) = result {
                            transcript.note("error", e.to_string());
                        }
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
                                        refresh_thinking(&client, &mut transcript).await;
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
                }
            }
            event = events.recv() => {
                let Some(event) = event else {
                    transcript.note("error", "pi exited.");
                    transcript.ui.set_streaming(false);
                    break;
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
                    update_stats(&client, &transcript).await;
                }
                if let Event::ExtensionUiRequest(req) = &event {
                    handle_extension_ui(&client, &mut transcript, req);
                }
            }
        }
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
        let UiCmd::Send(text) = cmd else { continue };

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
