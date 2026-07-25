//! slinty-pi M0: minimal window that drives `pi --mode rpc`.
//!
//! Architecture per PRODUCT_PLAN.md §6: Slint owns the main thread; a tokio
//! runtime on background threads owns the pi child process. All UI mutation
//! happens on the Slint thread via `Weak::upgrade_in_event_loop`; streaming
//! deltas are coalesced (~33 ms) before touching the transcript model.
//!
//! `SLINTY_DEMO=1` runs without pi and streams synthetic tokens instead
//! (`SLINTY_DEMO_RATE` tokens/sec, default 100) — the M0 perf harness.

use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use tokio::sync::mpsc;

use pi_rpc::{
    content_text, AssistantMessageEvent, Event, ExtensionUiReply, PiClient, PiOptions,
};

slint::include_modules!();

/// Commands from UI callbacks to the backend.
#[derive(Debug)]
enum UiCmd {
    Send(String),
    Abort,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let app = AppWindow::new()?;
    let transcript: Rc<VecModel<TranscriptEntry>> = Rc::new(VecModel::default());
    app.set_transcript(ModelRc::from(transcript.clone()));

    let rt = tokio::runtime::Runtime::new()?;
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<UiCmd>();

    {
        let tx = cmd_tx.clone();
        app.on_send(move |text| {
            let _ = tx.send(UiCmd::Send(text.to_string()));
        });
        let tx = cmd_tx.clone();
        app.on_abort(move || {
            let _ = tx.send(UiCmd::Abort);
        });
    }

    let weak = app.as_weak();
    if std::env::var("SLINTY_DEMO").is_ok() {
        rt.spawn(demo_backend(weak, cmd_rx));
    } else {
        rt.spawn(pi_backend(weak, cmd_rx));
    }

    app.invoke_focus_input();
    app.run()?;
    rt.shutdown_background();
    Ok(())
}

// ---------------------------------------------------------------------------
// UI-thread helpers. The backend tracks a shadow length so it can address the
// entry it appended without reading UI state; all closures below run on the
// Slint thread in submission order, which keeps the shadow count consistent.
// ---------------------------------------------------------------------------

struct Ui {
    weak: Weak<AppWindow>,
    shadow_len: usize,
}

impl Ui {
    fn new(weak: Weak<AppWindow>) -> Self {
        Self { weak, shadow_len: 0 }
    }

    fn with_transcript(
        &self,
        f: impl FnOnce(&AppWindow, &VecModel<TranscriptEntry>) + Send + 'static,
    ) {
        let _ = self.weak.upgrade_in_event_loop(move |app| {
            let model = app.get_transcript();
            let model = model
                .as_any()
                .downcast_ref::<VecModel<TranscriptEntry>>()
                .expect("transcript is a VecModel");
            f(&app, model);
        });
    }

    /// Append an entry; returns its index for later updates.
    fn push(&mut self, role: &'static str, text: String) -> usize {
        let index = self.shadow_len;
        self.shadow_len += 1;
        self.with_transcript(move |app, model| {
            model.push(TranscriptEntry {
                role: role.into(),
                text: text.into(),
            });
            app.invoke_scroll_to_end();
        });
        index
    }

    fn set_text(&self, index: usize, text: String) {
        self.with_transcript(move |app, model| {
            if let Some(mut entry) = model.row_data(index) {
                entry.text = text.into();
                model.set_row_data(index, entry);
                app.invoke_scroll_to_end();
            }
        });
    }

    fn set_streaming(&self, streaming: bool) {
        let _ = self.weak.upgrade_in_event_loop(move |app| {
            app.set_streaming(streaming);
        });
    }

    fn set_model_name(&self, name: String) {
        let _ = self.weak.upgrade_in_event_loop(move |app| {
            app.set_model_name(SharedString::from(name));
        });
    }

    fn set_status(&self, status: String) {
        let _ = self.weak.upgrade_in_event_loop(move |app| {
            app.set_status_text(SharedString::from(status));
        });
    }
}

/// Coalesces streaming text for one in-progress transcript entry, flushing to
/// the UI at most every `FLUSH_INTERVAL` (plus a final flush on block end).
struct StreamBlock {
    index: usize,
    buffer: String,
    last_flush: std::time::Instant,
}

const FLUSH_INTERVAL: Duration = Duration::from_millis(33);

impl StreamBlock {
    fn new(index: usize) -> Self {
        Self {
            index,
            buffer: String::new(),
            last_flush: std::time::Instant::now(),
        }
    }

    fn append(&mut self, ui: &Ui, delta: &str) {
        self.buffer.push_str(delta);
        if self.last_flush.elapsed() >= FLUSH_INTERVAL {
            self.flush(ui);
        }
    }

    fn flush(&mut self, ui: &Ui) {
        ui.set_text(self.index, self.buffer.clone());
        self.last_flush = std::time::Instant::now();
    }
}

// ---------------------------------------------------------------------------
// pi backend
// ---------------------------------------------------------------------------

async fn pi_backend(weak: Weak<AppWindow>, mut cmd_rx: mpsc::UnboundedReceiver<UiCmd>) {
    let mut ui = Ui::new(weak);

    let opts = PiOptions {
        cwd: std::env::current_dir().ok(),
        ..Default::default()
    };
    let (client, mut events) = match PiClient::spawn(opts).await {
        Ok(pair) => pair,
        Err(e) => {
            ui.set_model_name("pi not available".into());
            ui.push("error", format!("Failed to start pi: {e}\nIs `pi` on your PATH? Install: npm install -g @earendil-works/pi-coding-agent"));
            return;
        }
    };

    match client.get_state().await {
        Ok(state) => {
            let name = state
                .pointer("/model/name")
                .or_else(|| state.pointer("/model/id"))
                .and_then(|v| v.as_str())
                .unwrap_or("no model configured");
            ui.set_model_name(name.to_string());
        }
        Err(e) => ui.set_model_name(format!("state error: {e}")),
    }

    let mut streaming = false;
    let mut current: Option<StreamBlock> = None;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    UiCmd::Send(text) => {
                        let result = if streaming {
                            ui.push("user", format!("↪ {text}"));
                            client.prompt_steering(&text).await
                        } else {
                            ui.push("user", text.clone());
                            client.prompt(&text).await
                        };
                        if let Err(e) = result {
                            ui.push("error", format!("{e}"));
                        }
                    }
                    UiCmd::Abort => {
                        if let Err(e) = client.abort().await {
                            ui.push("error", format!("abort failed: {e}"));
                        }
                    }
                }
            }
            event = events.recv() => {
                let Some(event) = event else {
                    ui.push("error", "pi exited.".into());
                    ui.set_streaming(false);
                    break;
                };
                handle_event(event, &client, &mut ui, &mut streaming, &mut current).await;
            }
        }
    }
}

async fn handle_event(
    event: Event,
    client: &PiClient,
    ui: &mut Ui,
    streaming: &mut bool,
    current: &mut Option<StreamBlock>,
) {
    match event {
        Event::AgentStart => {
            *streaming = true;
            ui.set_streaming(true);
        }
        Event::AgentSettled => {
            *streaming = false;
            if let Some(block) = current.as_mut() {
                block.flush(ui);
            }
            *current = None;
            ui.set_streaming(false);
            if let Ok(stats) = client.get_session_stats().await {
                let cost = stats.get("cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let tokens = stats
                    .pointer("/tokens/total")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                ui.set_status(format!("{tokens} tok · ${cost:.4}"));
            }
        }
        Event::MessageUpdate {
            assistant_message_event,
            ..
        } => match assistant_message_event {
            AssistantMessageEvent::TextStart { .. } => {
                let index = ui.push("assistant", String::new());
                *current = Some(StreamBlock::new(index));
            }
            AssistantMessageEvent::TextDelta { delta, .. } => {
                if let Some(block) = current.as_mut() {
                    block.append(ui, &delta);
                }
            }
            AssistantMessageEvent::TextEnd { content, .. } => {
                if let Some(block) = current.as_mut() {
                    if !content.is_empty() {
                        block.buffer = content;
                    }
                    block.flush(ui);
                }
                *current = None;
            }
            AssistantMessageEvent::ThinkingStart => {
                let index = ui.push("thinking", String::new());
                *current = Some(StreamBlock::new(index));
            }
            AssistantMessageEvent::ThinkingDelta { delta } => {
                if let Some(block) = current.as_mut() {
                    block.append(ui, &delta);
                }
            }
            AssistantMessageEvent::ThinkingEnd => {
                if let Some(block) = current.as_mut() {
                    block.flush(ui);
                }
                *current = None;
            }
            AssistantMessageEvent::Error { reason } => {
                if reason != "aborted" {
                    ui.push("error", format!("model error: {reason}"));
                }
            }
            _ => {}
        },
        Event::ToolExecutionStart {
            tool_name, args, ..
        } => {
            let summary = tool_summary(&tool_name, &args);
            ui.push("tool", format!("⚙ {summary}"));
        }
        Event::ToolExecutionEnd {
            tool_name,
            result,
            is_error,
            ..
        } => {
            let text = content_text(&result);
            let first_line = text.lines().next().unwrap_or("").to_string();
            if is_error {
                ui.push("error", format!("✗ {tool_name}: {first_line}"));
            } else if !first_line.is_empty() {
                ui.push("tool", format!("✓ {first_line}"));
            }
        }
        Event::CompactionStart { .. } => {
            ui.set_status("compacting context…".into());
        }
        Event::AutoRetryStart {
            attempt,
            max_attempts,
            ..
        } => {
            ui.set_status(format!("retrying ({attempt}/{max_attempts})…"));
        }
        Event::ExtensionUiRequest(req) => {
            // M0: no dialog surfaces yet. Cancel dialogs so extensions don't
            // hang; surface notifications in the transcript.
            match req.method.as_str() {
                "select" | "confirm" | "input" | "editor" => {
                    let title = req.title.as_deref().unwrap_or(&req.method);
                    ui.push(
                        "info",
                        format!("extension dialog \"{title}\" auto-dismissed (M0)"),
                    );
                    let _ = client.reply_extension_ui(&req.id, ExtensionUiReply::Cancelled);
                }
                "notify" => {
                    if let Some(msg) = req.message {
                        ui.push("info", msg);
                    }
                }
                _ => {}
            }
        }
        _ => {}
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
        Some(d) => format!("{tool_name}: {d}"),
        None => tool_name.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Demo backend: synthetic token stream, no pi needed. Perf harness for M0.
// ---------------------------------------------------------------------------

async fn demo_backend(weak: Weak<AppWindow>, mut cmd_rx: mpsc::UnboundedReceiver<UiCmd>) {
    let mut ui = Ui::new(weak);
    ui.set_model_name("demo model (synthetic)".into());

    let rate: u64 = std::env::var("SLINTY_DEMO_RATE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let words = "The quick brown fox jumps over the lazy dog while streaming \
                 tokens into a Slint transcript to measure frame pacing and \
                 update coalescing under sustained load. ";

    let mut abort = false;
    // SLINTY_DEMO_AUTOSEND=<prompt> streams immediately without typing —
    // lets the perf harness run unattended.
    let mut next: Option<UiCmd> = std::env::var("SLINTY_DEMO_AUTOSEND").ok().map(UiCmd::Send);
    loop {
        let cmd = match next.take() {
            Some(cmd) => cmd,
            None => match cmd_rx.recv().await {
                Some(cmd) => cmd,
                None => break,
            },
        };
        match cmd {
            UiCmd::Send(text) => {
                ui.push("user", text);
                ui.set_streaming(true);
                let index = ui.push("assistant", String::new());
                let mut block = StreamBlock::new(index);
                let start = std::time::Instant::now();
                let mut interval =
                    tokio::time::interval(Duration::from_micros(1_000_000 / rate.max(1)));
                let tokens: Vec<&str> = words.split_inclusive(' ').collect();
                // Stream ~30 seconds worth of tokens or until aborted.
                for i in 0..(rate * 30) {
                    tokio::select! {
                        _ = interval.tick() => {
                            block.append(&mut ui, tokens[i as usize % tokens.len()]);
                        }
                        cmd = cmd_rx.recv() => {
                            if matches!(cmd, Some(UiCmd::Abort) | None) {
                                abort = true;
                                break;
                            }
                        }
                    }
                }
                block.flush(&mut ui);
                ui.set_streaming(false);
                let elapsed = start.elapsed().as_secs_f64();
                let streamed = block.buffer.len();
                ui.set_status(format!(
                    "{streamed} chars in {elapsed:.1}s ({:.0} tok/s target){}",
                    rate as f64,
                    if abort { " · aborted" } else { "" }
                ));
                abort = false;
            }
            UiCmd::Abort => {}
        }
    }
}
