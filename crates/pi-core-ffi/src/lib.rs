//! UniFFI boundary for the SwiftUI app: a chat session over `pi --mode rpc`,
//! session/project browsing, history hydration/rendering (SW3), and (SW4)
//! local-model browsing/management, exposed to Swift.
//!
//! `ChatSink` stays smaller than `pi_core::backend`'s full `UiSink` (no
//! palette/tree surface), but pushes rendered rows: `pi-render` (the same
//! crate `pi-core` uses for its Slint live-streaming path) turns a
//! `get_messages` payload into `RowSpec`s, mirrored across FFI as
//! `RowRecord`. See the SW3 milestone in the project's swiftui-branch plan
//! for the rendering-strategy rationale. `LocalModelIndex` similarly reuses
//! `pi-local` (the same crate `pi-core` uses for rapid-mlx/router/HF/Ollama/
//! auth) rather than reimplementing that HTTP/CLI/file-I/O logic — see the
//! SW4 milestone.
//!
//! Threading contract mirrors `pi_core::backend::UiSink`: `ChatSink` methods
//! are `Send + Sync`, fire-and-forget, called from a tokio worker thread
//! owned by this crate. The Swift implementation is responsible for hopping
//! to `@MainActor` on every callback, the same responsibility
//! `Weak::upgrade_in_event_loop` discharges on the Slint side.

mod attach;
mod dialog;
mod live_preview;
mod local_models;
mod models;
mod row;
mod session_index;
mod thinking;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use pi_rpc::{AssistantMessageEvent, Event, ImageContent, PiClient, PiError, PiOptions};
use tokio::sync::{mpsc, oneshot};

pub use dialog::{ExtensionDialogRecord, ExtensionDialogReply};
pub use local_models::{
    CachedModelRecord, HfResultRecord, LocalModelError, LocalModelIndex, OllamaPanelRecord,
    RapidMlxPanelRecord, RouterModelRecord, RouterPanelRecord,
};
pub use models::{ModelRecord, ServerDotState};
pub use row::{CodeLineRecord, ColoredSpanRecord, RowRecord, TableCellRecord};
pub use session_index::{ProjectRecord, SessionIndex, SessionRecord};
pub use thinking::{SessionStatsRecord, ThinkingLevelKind, ThinkingLevelRecord};

uniffi::setup_scaffolding!();

/// Installs `tracing-oslog` as the process's `tracing` subscriber, so a
/// compiled `SwiftyPi.app` has somewhere for its diagnostics to go — without
/// this, every `tracing::*!` call in this crate and `pi-rpc` (including one
/// that forwards `pi`'s own child-process stderr) is a silent no-op, since
/// nothing else installs a subscriber inside the shipped app. Idempotent
/// (the `Once` guard) and safe to call from every constructor that might run
/// first (`PiSession::new`/`new_demo`, `LocalModelIndex::new`).
///
/// `examples/spike_check.rs` already installs its own `tracing_subscriber::
/// fmt()` subscriber in its `main()`, *before* constructing a `PiSession` —
/// `tracing`'s global default can only be set once process-wide, so
/// `try_init()`'s `Err` there is expected and deliberately discarded: that
/// dev tool keeps its own stderr output unchanged, and only a real app
/// (where nothing has claimed the slot yet) actually gets `tracing-oslog`.
fn ensure_logging_initialized() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = tracing_subscriber::registry()
            .with(tracing_oslog::OsLogger::new(
                "dev.slinty-pi.swifty-pi",
                "rust",
            ))
            .try_init();
    });
}

/// Backend -> UI push. Implemented in Swift; called from Rust on a tokio
/// worker thread.
#[uniffi::export(with_foreign)]
pub trait ChatSink: Send + Sync {
    fn on_text_delta(&self, delta: String);
    fn on_turn_end(&self);
    fn on_streaming_changed(&self, streaming: bool);
    fn on_error(&self, message: String);
    /// Fired once right after the session comes up, and again after every
    /// successful action that changes *which* session is active
    /// (`switch_project`, `new_session`, `switch_session`, or
    /// `delete_session` when the deleted path was active). `None` when
    /// there's no active session file yet. Purely a path/sidebar-
    /// highlighting signal as of SW3 — feed it into `SessionIndex.
    /// list_sessions`'s `active_path` param; `on_history_replaced` (not
    /// this) is the single source of truth for what's currently rendered in
    /// the transcript.
    fn on_active_session_changed(&self, path: Option<String>);
    /// Replaces the entire rendered transcript with `rows`, richly rendered
    /// (markdown/code/tables — see `RowRecord`). Fired after every
    /// session-changing action (empty for a fresh session, populated for a
    /// resumed one) and after every turn settles, re-rendering the
    /// just-finalized turn in place of the plain-text streaming bubble
    /// `on_text_delta` built up during it.
    fn on_history_replaced(&self, rows: Vec<RowRecord>);
    /// Fired for the four blocking extension-UI dialog kinds (`select`/
    /// `confirm`/`input`/`editor`) — pi's sanctioned tool-call
    /// permission-gating pattern (a `tool_call` extension + `confirm`/
    /// `select` dialog) surfaces through this same callback, not a separate
    /// protocol. Swift must eventually call `PiSession.
    /// reply_extension_dialog` with this request's `id`, or the gated
    /// extension call hangs until pi's own `timeout` (if any) elapses.
    /// Fire-and-forget signals (`notify`, `setStatus`, `setWidget`,
    /// `setTitle`, `set_editor_text`) are an explicit SW5 scope cut — not
    /// delivered via this or any callback yet.
    fn on_extension_dialog(&self, request: ExtensionDialogRecord);
    /// Status-bar server-dot state, pushed only when it changes — polled
    /// every 5s (and immediately after `set_model`/`serve_rapid_mlx_model`
    /// succeed) against whichever model `refresh_models`/`set_model` last
    /// resolved as current. The model *list* itself stays pull-based (see
    /// `refresh_models`/`set_model`) — this is the one value with no
    /// natural user action to hang a re-fetch off of.
    fn on_server_dot_changed(&self, state: ServerDotState);
    /// Once (`running:true`, empty text) on `ThinkingStart`, throttled
    /// ~33ms on `ThinkingDelta`, once more (`running:false`) on
    /// `ThinkingEnd`. `id` is a synthetic per-region sequence number —
    /// thinking has no id of its own on the wire, and pi allows more than
    /// one thinking block per turn (each becomes its own row, never
    /// reusing a prior region's). No "removed" signal: a finished region
    /// simply stops updating until `on_history_replaced` clears it.
    fn on_thinking_row_changed(&self, id: String, row: RowRecord);
    /// Once (`running:true`) on `ToolExecutionStart`, throttled ~100ms on
    /// `ToolExecutionUpdate`, once more (`running:false`, ✓/✗ mark,
    /// elapsed time) on `ToolExecutionEnd`. `id` is the real
    /// `tool_call_id` — multiple calls can be live at once. Same
    /// no-"removed"-signal contract as `on_thinking_row_changed`.
    fn on_tool_row_changed(&self, id: String, row: RowRecord);
    /// Session-size/cost snapshot (SW8), pushed once inside
    /// `hydrate_and_push` — which already runs after every session-changing
    /// hydration (resume, `switch_session`) *and* after every turn settles
    /// (`Event::AgentSettled` calls it too), so this one call site covers
    /// every trigger `pi_core::backend::update_stats` has, with no need for
    /// a second push site.
    fn on_session_stats_changed(&self, stats: SessionStatsRecord);
    /// Pushed after every `attach_path`/`remove_attachment` call with the
    /// running list of queued image display names (SW9) — the composer's
    /// attachment-chip row. Also cleared (empty `Vec`) right before a
    /// `send` that consumes queued images.
    fn on_pending_attachments_changed(&self, names: Vec<String>);
    /// A non-image `attach_path` call pushes the path here (as an `@path`
    /// reference) for Swift to splice into the composer's draft text — the
    /// first concrete use of this "push text into the composer" pattern
    /// (previously only a hypothetical during SW5's parked `set_editor_text`
    /// research).
    fn on_composer_append(&self, text: String);
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum PiSessionError {
    #[error("failed to start pi: {0}")]
    Spawn(String),
    #[error("{0}")]
    Action(String),
}

enum ChatCmd {
    Send(String),
    Abort,
    SwitchProject {
        path: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    NewSession {
        reply: oneshot::Sender<Result<(), String>>,
    },
    DeleteSession {
        path: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    RenameSession {
        name: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Load a different session file within the same project (no respawn) —
    /// the sidebar-click "switch" action, and (via `PiSession::new`'s
    /// `resume_session_path`) the launch-time "restore" one.
    SwitchSession {
        path: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// (Re)spawns a managed `rapid-mlx serve <alias>` and, once ready, makes
    /// it pi's active model — the one local-model action that needs the
    /// live `PiClient` (`client.set_model(...)`), unlike everything else in
    /// `LocalModelIndex`. See the SW4 plan's Design section.
    ServeRapidMlxModel {
        alias: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Fire-and-forget, unlike the reply-carrying variants above —
    /// `PiClient::reply_extension_ui` is itself a non-blocking stdin write
    /// with no correlated response line from pi, so there's nothing
    /// meaningful to await (mirrors `Send`/`Abort`).
    ReplyExtensionDialog {
        request_id: String,
        reply: ExtensionDialogReply,
    },
    /// `GetAvailableModels` + `GetState`, re-fetched fresh every call — the
    /// composer picker's pull-based refresh, mirroring
    /// `pi_core::backend::refresh_models`'s full-re-fetch shape (used
    /// uniformly here, not just after actions that could change
    /// availability, since there's no push signal to hang a targeted
    /// refresh off of — see `ChatSink::on_server_dot_changed`'s doc
    /// comment).
    RefreshModels {
        reply: oneshot::Sender<Result<Vec<ModelRecord>, String>>,
    },
    /// The composer picker's "switch model" action: `client.set_model(...)`
    /// then the same refresh `RefreshModels` does.
    SetModel {
        provider: String,
        model_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// `GetAvailableThinkingLevels` + `GetState`, re-fetched fresh every
    /// call — same pull-based shape as `RefreshModels`.
    RefreshThinkingLevels {
        reply: oneshot::Sender<Result<Vec<ThinkingLevelRecord>, String>>,
    },
    /// The thinking-level picker's "switch level" action:
    /// `client.set_thinking_level(...)`.
    SetThinkingLevel {
        level: ThinkingLevelKind,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Fire-and-forget, matching `Send`/`Abort` — mirrors Slint's own
    /// fire-and-forget `UiCmd::AttachPath`. Errors surface via `ChatSink::
    /// on_error`, not a return value.
    AttachPath {
        path: String,
    },
    /// Fire-and-forget, matching `AttachPath` — mirrors `UiCmd::
    /// RemoveAttachment`.
    RemoveAttachment {
        index: usize,
    },
}

/// A single `pi --mode rpc` child process plus the tokio runtime that owns
/// it end-to-end, mirroring `pi_core::backend::pi_backend`'s shape (a
/// background runtime, a command channel from the UI, an event loop pushing
/// through the sink) but scoped to exactly what this spike needs.
#[derive(uniffi::Object)]
pub struct PiSession {
    cmd_tx: mpsc::UnboundedSender<ChatCmd>,
    /// Read by `hydrate_and_push` on every hydrate/settle, mirroring
    /// `pi_core::backend::Transcript`'s own `dark: Arc<AtomicBool>` field —
    /// a theme flip doesn't retroactively re-color already-rendered rows,
    /// only the next hydrate/settle, same staleness Slint accepts today.
    dark: Arc<AtomicBool>,
    // Keeps the runtime (and therefore the spawned event-loop task and the
    // `pi` child it owns) alive for as long as Swift holds this object.
    _runtime: tokio::runtime::Runtime,
}

#[uniffi::export]
impl PiSession {
    /// Spawns `pi --mode rpc` in `cwd` and starts forwarding its events to
    /// `sink`. Blocks (briefly — a process spawn + RPC handshake) until the
    /// child is up or has failed to start. If `resume_session_path` is
    /// given, resumes and hydrates it before the first
    /// `on_active_session_changed`/`on_history_replaced` pair fires —
    /// launch-time session restore, mirroring `pi_core::backend::
    /// pi_backend`'s `resume_on_first_spawn`. That hydration itself happens
    /// asynchronously on the spawned event-loop task, not here, so this
    /// constructor doesn't block on it (a large session's `get_messages`
    /// fetch shouldn't stall whatever thread calls `PiSession.init`).
    #[uniffi::constructor]
    pub fn new(
        sink: Arc<dyn ChatSink>,
        cwd: String,
        resume_session_path: Option<String>,
        dark: bool,
    ) -> Result<Self, PiSessionError> {
        ensure_logging_initialized();
        let runtime = tokio::runtime::Runtime::new().map_err(|e| {
            let message = format!("could not start a tokio runtime: {e}");
            tracing::error!("{message}");
            PiSessionError::Spawn(message)
        })?;
        runtime.block_on(ensure_usable_path());
        let opts = PiOptions {
            cwd: Some(PathBuf::from(cwd)),
            ..Default::default()
        };
        let (client, events) = runtime.block_on(PiClient::spawn(opts)).map_err(|e| {
            let message = e.to_string();
            tracing::error!("failed to start pi: {message}");
            PiSessionError::Spawn(message)
        })?;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let dark = Arc::new(AtomicBool::new(dark));
        runtime.spawn(run(
            client,
            events,
            cmd_rx,
            sink,
            dark.clone(),
            resume_session_path,
        ));
        Ok(Self {
            cmd_tx,
            dark,
            _runtime: runtime,
        })
    }

    /// A synthetic session that never spawns `pi`: every `send` streams a
    /// short canned reply through the same `ChatSink` callbacks a real
    /// session uses, at roughly the same cadence, then pushes a couple of
    /// synthetic `RowRecord`s (prose + syntax-highlighted code) through
    /// `on_history_replaced` so the rich-rendering path is exercisable
    /// without `pi` installed. Mirrors `SLINTY_DEMO=1`'s role for the Slint
    /// app — demoable without `pi`, and a display-less perf/frame-rate
    /// check independent of a live model.
    ///
    /// Deliberately its own small synthetic streamer rather than reusing
    /// `pi_core::backend::demo_backend`: that function drives the full
    /// `UiSink` surface (local-model panels, palette, tree — SW4+ scope
    /// this crate's `ChatSink` doesn't expose), so there's nothing for it to
    /// plug into here.
    #[uniffi::constructor]
    pub fn new_demo(sink: Arc<dyn ChatSink>) -> Self {
        ensure_logging_initialized();
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime for demo session");
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let dark = Arc::new(AtomicBool::new(true));
        runtime.spawn(run_demo(cmd_rx, sink, dark.clone()));
        Self {
            cmd_tx,
            dark,
            _runtime: runtime,
        }
    }

    /// Send a prompt. Fire-and-forget: errors surface via
    /// `ChatSink::on_error`, not a return value, matching this crate's
    /// push-based design.
    pub fn send(&self, prompt: String) {
        let _ = self.cmd_tx.send(ChatCmd::Send(prompt));
    }

    pub fn abort(&self) {
        let _ = self.cmd_tx.send(ChatCmd::Abort);
    }

    /// Answers a dialog delivered via `ChatSink::on_extension_dialog`.
    /// Fire-and-forget, same shape as `send`/`abort` — see
    /// `ChatCmd::ReplyExtensionDialog`'s doc comment for why there's no
    /// meaningful result to await.
    pub fn reply_extension_dialog(&self, request_id: String, reply: ExtensionDialogReply) {
        let _ = self
            .cmd_tx
            .send(ChatCmd::ReplyExtensionDialog { request_id, reply });
    }

    /// Updates the theme `hydrate_and_push` highlights against on the next
    /// hydrate/settle (not retroactively). Swift calls this once at startup
    /// and again on every `colorScheme` change.
    pub fn set_dark_mode(&self, dark: bool) {
        self.dark.store(dark, Ordering::Relaxed);
    }

    /// Attaches a file (picked or dropped) to the next prompt — an image
    /// is queued (see `ChatSink::on_pending_attachments_changed`); anything
    /// else pushes an `@path` reference via `ChatSink::on_composer_append`.
    /// Fire-and-forget, same shape as `send`/`abort`.
    pub fn attach_path(&self, path: String) {
        let _ = self.cmd_tx.send(ChatCmd::AttachPath { path });
    }

    /// Removes a queued image attachment by its index in the chip row.
    /// Fire-and-forget, same shape as `attach_path`. `u32`, not `usize` —
    /// UniFFI doesn't export `usize`/`isize` directly.
    pub fn remove_attachment(&self, index: u32) {
        let _ = self.cmd_tx.send(ChatCmd::RemoveAttachment {
            index: index as usize,
        });
    }
}

/// Session-lifecycle actions: `async`/`Result`-returning, unlike `send`/
/// `abort`. Those are fire-and-forget because streaming is inherently async
/// and pushed; these are one-shot RPCs with a real completion point, and
/// Swift's "trigger the action, then re-fetch the session list" pattern
/// would otherwise race the RPC against the refetch if these were also
/// fire-and-forget.
#[uniffi::export(async_runtime = "tokio")]
impl PiSession {
    /// Kills the current `pi` child and respawns it in `path`, always with a
    /// fresh session (never `--continue`) — so there's never a "transcript
    /// looks empty but pi secretly has context" mismatch. Fires
    /// `on_active_session_changed` on success.
    pub async fn switch_project(&self, path: String) -> Result<(), PiSessionError> {
        self.call(|reply| ChatCmd::SwitchProject { path, reply })
            .await
    }

    /// Starts a fresh session within the current project. Fires
    /// `on_active_session_changed` on success.
    pub async fn new_session(&self) -> Result<(), PiSessionError> {
        self.call(|reply| ChatCmd::NewSession { reply }).await
    }

    /// Moves any listed session's file to the OS trash — not just the
    /// active one. If the deleted path was active, also starts a fresh
    /// session (so the live child keeps working against a file that still
    /// exists) and fires `on_active_session_changed`.
    pub async fn delete_session(&self, path: String) -> Result<(), PiSessionError> {
        self.call(|reply| ChatCmd::DeleteSession { path, reply })
            .await
    }

    /// Renames the currently *active* session — `pi`'s `set_session_name`
    /// takes no path argument, so this can't target any other session
    /// without first switching to it.
    pub async fn rename_session(&self, name: String) -> Result<(), PiSessionError> {
        self.call(|reply| ChatCmd::RenameSession { name, reply })
            .await
    }

    /// Loads `path` (must be a session file within the current project) and
    /// replaces the transcript with its history via `on_history_replaced`.
    /// Fires `on_active_session_changed` on success — the sidebar's
    /// click-to-resume action.
    pub async fn switch_session(&self, path: String) -> Result<(), PiSessionError> {
        self.call(|reply| ChatCmd::SwitchSession { path, reply })
            .await
    }

    /// (Re)spawns a managed `rapid-mlx serve <alias>`, waits for it to
    /// become ready, then makes it pi's active model. Stops any
    /// previously-managed server first — a model switch is a supervised
    /// restart, not a hot swap, matching `pi_core::backend::
    /// serve_rapid_mlx_model`. An already-running *external* server (or
    /// anything else bound to the port) surfaces as a normal
    /// "failed to become ready" error rather than being killed: this only
    /// ever stops servers it itself spawned.
    pub async fn serve_rapid_mlx_model(&self, alias: String) -> Result<(), PiSessionError> {
        self.call(|reply| ChatCmd::ServeRapidMlxModel { alias, reply })
            .await
    }

    /// Lists pi's currently-configured models with picker-ready labels,
    /// flagging whichever one pi's `GetState` reports as active. Pull-based
    /// — call again after any action that could plausibly change model
    /// availability (mirrors `pi_core::backend::refresh_models`'s call
    /// sites): session start, `set_model`, `serve_rapid_mlx_model`, and
    /// (Swift-side) the router/HF/Ollama actions in `LocalModelIndex`.
    pub async fn refresh_models(&self) -> Result<Vec<ModelRecord>, PiSessionError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(ChatCmd::RefreshModels { reply: reply_tx });
        reply_rx
            .await
            .map_err(|_| PiSessionError::Action("session task ended".to_string()))?
            .map_err(PiSessionError::Action)
    }

    /// Switches pi's active model, then refreshes the picker list (so the
    /// new selection's `is_current` flag is correct on the caller's next
    /// `refresh_models` call) and resets the server-dot poll timer so it
    /// reflects the switch without waiting up to 5s.
    pub async fn set_model(
        &self,
        provider: String,
        model_id: String,
    ) -> Result<(), PiSessionError> {
        self.call(|reply| ChatCmd::SetModel {
            provider,
            model_id,
            reply,
        })
        .await
    }

    /// Lists pi's currently-available thinking levels with picker-ready
    /// labels, flagging whichever one pi's `GetState` reports as active.
    /// Pull-based, same shape as `refresh_models` — call again after
    /// session start and after `set_model` (available levels differ per
    /// model).
    pub async fn refresh_thinking_levels(
        &self,
    ) -> Result<Vec<ThinkingLevelRecord>, PiSessionError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self
            .cmd_tx
            .send(ChatCmd::RefreshThinkingLevels { reply: reply_tx });
        reply_rx
            .await
            .map_err(|_| PiSessionError::Action("session task ended".to_string()))?
            .map_err(PiSessionError::Action)
    }

    /// Switches pi's active thinking level. Mirrors `set_model`'s
    /// fire-then-Swift-refetches pattern: the caller is expected to call
    /// `refresh_thinking_levels` again afterward to pick up the new
    /// `is_current` flag, rather than this method returning it directly.
    pub async fn set_thinking_level(&self, level: ThinkingLevelKind) -> Result<(), PiSessionError> {
        self.call(|reply| ChatCmd::SetThinkingLevel { level, reply })
            .await
    }

    /// Richly-renders `markdown` (typically the in-flight streaming buffer)
    /// without touching `client`/`events`/the command channel at all — a
    /// stateless, side-effect-free segment-and-map, reusing whatever theme
    /// `set_dark_mode` last set. Swift calls this on a throttled ~33ms
    /// cadence while a reply streams, mirroring (not literally porting)
    /// `pi_core::backend::Transcript::flush_stream`'s cadence — see the
    /// "Live markdown/code rendering while a reply is streaming" plan
    /// section for the full rationale.
    pub async fn preview_rows(&self, markdown: String) -> Vec<RowRecord> {
        preview_rows(&markdown, self.dark.load(Ordering::Relaxed))
    }
}

// Plain (non-`#[uniffi::export]`) impl block: `#[uniffi::export]` treats
// every method in an attributed block as exportable, and `impl Trait`
// arguments (used by `call` below) aren't valid in that position — so this
// private helper has to live outside both exported blocks.
impl PiSession {
    async fn call(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<(), String>>) -> ChatCmd,
    ) -> Result<(), PiSessionError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(build(reply_tx));
        reply_rx
            .await
            .map_err(|_| PiSessionError::Action("session task ended".to_string()))?
            .map_err(PiSessionError::Action)
    }
}

/// A managed `rapid-mlx serve` child this `PiSession` itself spawned (never
/// an external/unmanaged server) — stopped before starting a replacement,
/// and (implicitly, via `Drop`/`kill_on_drop`) when `run()`'s task ends.
struct Managed {
    server: pi_local::rapid_mlx::ManagedServer,
}

/// Owns the live `PiClient` end-to-end: drains UI commands and pi's event
/// stream, forwarding the minimal slice `ChatSink` cares about. Exits (and
/// drops `client`, killing the child) when the command channel closes —
/// Swift dropping its last `PiSession` reference is what triggers that.
///
/// `client`/`events` are `let mut` locals, not fields — `SwitchProject`
/// reassigns them in place to respawn `pi` in a new cwd, the same
/// loop-scoped-mutable-local pattern `pi_core::backend::run_session` already
/// uses for its own per-spawn state (`models`, `thinking_levels`,
/// `streaming`). The old `client` (and its child, `kill_on_drop`) is only
/// dropped once the *new* one has spawned successfully, so a failed
/// `switch_project` leaves the session fully usable rather than clientless.
///
/// If `resume_session_path` is set, resumes and hydrates it before the
/// first `on_active_session_changed` — see `PiSession::new`.
async fn run(
    mut client: PiClient,
    mut events: mpsc::UnboundedReceiver<Event>,
    mut cmd_rx: mpsc::UnboundedReceiver<ChatCmd>,
    sink: Arc<dyn ChatSink>,
    dark: Arc<AtomicBool>,
    resume_session_path: Option<String>,
) {
    // Set once `ServeRapidMlxModel` spawns a managed child, so a later call
    // knows to stop it before starting a replacement — same loop-scoped-
    // mutable-local pattern as `client`/`events` above.
    let mut managed_rapid_mlx: Option<Managed> = None;
    // Set by `RefreshModels`/`SetModel`/`ServeRapidMlxModel`; read by the
    // `dot_interval` tick below — same loop-scoped-mutable-local pattern as
    // `managed_rapid_mlx` above.
    let mut current_model: Option<models::ModelEntry> = None;
    let mut last_dot = ServerDotState::Hidden;
    let mut dot_interval = tokio::time::interval(Duration::from_secs(5));
    dot_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Live thinking/tool-call preview state (SW7) — read/written by
    // `live_preview::apply_live_preview`, reset defensively on
    // `AgentSettled` below (guards a tool call whose `ToolExecutionEnd`
    // never arrives).
    let mut live_thinking: Option<live_preview::ThinkingRegionState> = None;
    let mut live_tools: std::collections::HashMap<String, live_preview::ToolRunState> =
        std::collections::HashMap::new();
    let mut thinking_seq: u64 = 0;
    // Attachments queued for the next `Send` (SW9) — loop-scoped mutable
    // local, same pattern as `managed_rapid_mlx`/`live_thinking` above.
    let mut pending_images: Vec<(String, ImageContent)> = Vec::new();
    if let Some(path) = resume_session_path {
        match do_switch_session(&client, &path).await {
            Ok(()) => hydrate_and_push(&client, sink.as_ref(), &dark).await,
            Err(e) => report_error(
                sink.as_ref(),
                format!("could not restore last session: {e}"),
            ),
        }
    }
    sink.on_active_session_changed(active_session_path(&client).await);
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(ChatCmd::Send(text)) => {
                        let images: Vec<ImageContent> = std::mem::take(&mut pending_images)
                            .into_iter()
                            .map(|(_, img)| img)
                            .collect();
                        if !images.is_empty() {
                            sink.on_pending_attachments_changed(Vec::new());
                        }
                        let result = if images.is_empty() {
                            client.prompt(text).await
                        } else {
                            client.prompt_with_images(text, images).await
                        };
                        if let Err(e) = result {
                            report_error(sink.as_ref(), describe(&e));
                        }
                    }
                    Some(ChatCmd::Abort) => {
                        if let Err(e) = client.abort().await {
                            report_error(sink.as_ref(), describe(&e));
                        }
                    }
                    Some(ChatCmd::SwitchProject { path, reply }) => {
                        let opts = PiOptions { cwd: Some(PathBuf::from(&path)), ..Default::default() };
                        match PiClient::spawn(opts).await {
                            Ok((new_client, new_events)) => {
                                client = new_client; // old client (+ child, kill_on_drop) dropped here
                                events = new_events;
                                // A brand-new project has no active session yet — an
                                // unconditional clear, not a `get_messages` round trip
                                // (that's `hydrate_and_push`, reserved for a session
                                // known to already exist), matching
                                // `pi_core::backend`'s own plain `transcript.reset()`
                                // handling of `SwitchProject`/`NewSession`.
                                sink.on_history_replaced(Vec::new());
                                sink.on_active_session_changed(active_session_path(&client).await);
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(describe(&e))); // old client/events untouched
                            }
                        }
                    }
                    Some(ChatCmd::NewSession { reply }) => {
                        match client.new_session(None).await {
                            Ok(data) if data.get("cancelled").and_then(|v| v.as_bool()) == Some(true) => {
                                let _ = reply.send(Err("cancelled by an extension".to_string()));
                            }
                            Ok(_) => {
                                sink.on_history_replaced(Vec::new());
                                sink.on_active_session_changed(active_session_path(&client).await);
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(describe(&e)));
                            }
                        }
                    }
                    Some(ChatCmd::DeleteSession { path, reply }) => {
                        let is_active = active_session_path(&client).await.as_deref() == Some(path.as_str());
                        let target = PathBuf::from(&path);
                        let outcome = match tokio::task::spawn_blocking(move || trash::delete(&target)).await {
                            Ok(Ok(())) => Ok(()),
                            Ok(Err(e)) => Err(format!("could not delete session: {e}")),
                            Err(e) => Err(format!("delete task failed: {e}")),
                        };
                        if outcome.is_ok() && is_active {
                            // The file is already gone regardless of what an extension
                            // thinks; start fresh either way so the session stays usable.
                            if let Err(e) = client.new_session(None).await {
                                report_error(
                                    sink.as_ref(),
                                    format!(
                                        "deleted the open session but could not start a new one: {e}"
                                    ),
                                );
                            }
                            sink.on_history_replaced(Vec::new());
                            sink.on_active_session_changed(active_session_path(&client).await);
                        }
                        let _ = reply.send(outcome);
                    }
                    Some(ChatCmd::RenameSession { name, reply }) => {
                        let outcome = client.set_session_name(name).await.map_err(|e| describe(&e));
                        let _ = reply.send(outcome);
                    }
                    Some(ChatCmd::SwitchSession { path, reply }) => {
                        match do_switch_session(&client, &path).await {
                            Ok(()) => {
                                hydrate_and_push(&client, sink.as_ref(), &dark).await;
                                sink.on_active_session_changed(active_session_path(&client).await);
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(e));
                            }
                        }
                    }
                    Some(ChatCmd::ServeRapidMlxModel { alias, reply }) => {
                        if let Some(prev) = managed_rapid_mlx.take() {
                            let _ = prev.server.shutdown().await;
                        }
                        match pi_local::rapid_mlx::ManagedServer::spawn(
                            pi_local::rapid_mlx::DEFAULT_BINARY,
                            &alias,
                            pi_local::panel::RAPID_MLX_PORT,
                        ) {
                            Ok(mut server) => match server.wait_ready(Duration::from_secs(180)).await {
                                Ok(()) => {
                                    managed_rapid_mlx = Some(Managed {
                                        server,
                                    });
                                    let outcome = client
                                        .set_model("rapid-mlx", &alias)
                                        .await
                                        .map(|_| ())
                                        .map_err(|e| {
                                            format!(
                                                "rapid-mlx: {alias} is ready but pi couldn't \
                                                 select it (is a matching entry configured in \
                                                 models.json?): {e}"
                                            )
                                        });
                                    if outcome.is_ok() {
                                        (_, current_model) = models::refresh_models_and_state(&client).await;
                                        dot_interval.reset_immediately();
                                    }
                                    let _ = reply.send(outcome);
                                }
                                Err(e) => {
                                    let _ = reply
                                        .send(Err(format!("rapid-mlx: {alias} failed to start: {e}")));
                                }
                            },
                            Err(e) => {
                                let _ =
                                    reply.send(Err(format!("rapid-mlx: could not spawn serve: {e}")));
                            }
                        }
                    }
                    Some(ChatCmd::ReplyExtensionDialog { request_id, reply }) => {
                        if let Err(e) = client.reply_extension_ui(&request_id, reply.into()) {
                            report_error(
                                sink.as_ref(),
                                format!("could not reply to extension dialog: {e}"),
                            );
                        }
                    }
                    Some(ChatCmd::RefreshModels { reply }) => {
                        let (records, entry) = models::refresh_models_and_state(&client).await;
                        current_model = entry;
                        let _ = reply.send(Ok(records));
                    }
                    Some(ChatCmd::SetModel { provider, model_id, reply }) => {
                        match client.set_model(&provider, &model_id).await {
                            Ok(_) => {
                                (_, current_model) = models::refresh_models_and_state(&client).await;
                                dot_interval.reset_immediately();
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(describe(&e)));
                            }
                        }
                    }
                    Some(ChatCmd::RefreshThinkingLevels { reply }) => {
                        let records = thinking::refresh_thinking_levels(&client).await;
                        let _ = reply.send(Ok(records));
                    }
                    Some(ChatCmd::SetThinkingLevel { level, reply }) => {
                        match client.set_thinking_level(level.into()).await {
                            Ok(()) => {
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(describe(&e)));
                            }
                        }
                    }
                    Some(ChatCmd::AttachPath { path }) => {
                        attach::attach_path(&mut pending_images, Path::new(&path), sink.as_ref())
                            .await;
                    }
                    Some(ChatCmd::RemoveAttachment { index }) => {
                        if index < pending_images.len() {
                            pending_images.remove(index);
                            sink.on_pending_attachments_changed(
                                pending_images.iter().map(|(n, _)| n.clone()).collect(),
                            );
                        }
                    }
                    None => return,
                }
            }
            event = events.recv() => {
                let Some(event) = event else {
                    report_error(sink.as_ref(), "pi exited".to_string());
                    return;
                };
                apply(&event, sink.as_ref());
                live_preview::apply_live_preview(
                    &event,
                    sink.as_ref(),
                    &mut live_thinking,
                    &mut live_tools,
                    &mut thinking_seq,
                );
                // Re-render the whole transcript from `get_messages` truth
                // once the turn is fully done, rather than porting
                // `pi_core::backend::Transcript`'s incremental live-flush
                // machinery across FFI — see the SW3 plan's Design section.
                if matches!(event, Event::AgentSettled) {
                    hydrate_and_push(&client, sink.as_ref(), &dark).await;
                    // Defensive reset (SW7): guards a tool call whose
                    // `ToolExecutionEnd` never arrives. The Swift-side
                    // equivalent is already cleared by `on_history_replaced`.
                    live_thinking = None;
                    live_tools.clear();
                }
            }
            _ = dot_interval.tick() => {
                let managed_alive = managed_rapid_mlx.as_mut().map(|m| m.server.is_alive());
                let dot = models::compute_server_dot(current_model.as_ref(), managed_alive).await;
                if dot != last_dot {
                    last_dot = dot;
                    sink.on_server_dot_changed(dot);
                }
            }
        }
    }
}

/// pi's `sessionFile` from `get_state`, or `None` if it can't be fetched —
/// reimplemented here (not pulled from `pi-core`) matching this crate's
/// existing posture, see the crate doc comment.
async fn active_session_path(client: &PiClient) -> Option<String> {
    client
        .get_state()
        .await
        .ok()?
        .get("sessionFile")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// `get_messages` -> `pi_render::hydrate_rowspecs` -> `on_history_replaced`,
/// mirroring `pi_core::backend::hydrate_active_session`'s shape exactly.
/// Re-renders the *entire* current transcript from `get_messages` truth
/// rather than incrementally patching in new rows — sidesteps needing to
/// replicate `hydrate_rowspecs`' tool-result-patches-an-earlier-row logic
/// against a partial tail. Only called where a session file is already
/// known to exist (an explicit resume, or right after a turn just wrote
/// one) — never for a brand-new/empty session, where `on_history_replaced(
/// Vec::new())` is used directly instead (see the `SwitchProject`/
/// `NewSession`/`DeleteSession` arms in [`run`]).
async fn hydrate_and_push(client: &PiClient, sink: &dyn ChatSink, dark: &AtomicBool) {
    match client.get_messages().await {
        Ok(data) => {
            let messages = data
                .get("messages")
                .and_then(|m| m.as_array())
                .cloned()
                .unwrap_or_default();
            let rows = pi_render::hydrate_rowspecs(&messages, dark.load(Ordering::Relaxed));
            sink.on_history_replaced(rows.into_iter().map(RowRecord::from).collect());
        }
        Err(e) => report_error(sink, format!("could not load session messages: {e}")),
    }
    // SW8: this single call site already covers every trigger
    // `pi_core::backend::update_stats` has — launch-time resume,
    // `switch_session`, and every `AgentSettled` (whose handler in `run()`
    // calls this function as its first line) — see `ChatSink::
    // on_session_stats_changed`'s doc comment.
    if let Some(stats) = thinking::fetch_session_stats(client).await {
        sink.on_session_stats_changed(stats);
    }
}

/// Segments `markdown` fresh (no diffing/state) and maps each segment to a
/// `RowRecord` — the same building blocks `pi_core::backend::Transcript::
/// flush_stream` uses for Slint's live 33ms flush, minus the index-aligned
/// `set`/`truncate` bookkeeping that only makes sense against an imperative,
/// addressable row model. Swift instead fully replaces its `streamingRows`
/// array on every throttled call, so a plain fresh `Vec` is all this needs
/// to produce. `i == 0` mirrors `flush_stream`'s own `first` flag (only the
/// first segment of a streaming region counts as a message's lead row).
fn preview_rows(markdown: &str, dark: bool) -> Vec<RowRecord> {
    pi_render::segmenter::segment_markdown(markdown)
        .iter()
        .enumerate()
        .map(|(i, seg)| RowRecord::from(pi_render::spec_for_segment(seg, i == 0, dark, markdown)))
        .collect()
}

#[cfg(test)]
mod preview_rows_tests {
    use super::preview_rows;

    #[test]
    fn empty_markdown_yields_no_rows() {
        assert!(preview_rows("", true).is_empty());
    }

    #[test]
    fn unclosed_fence_streams_as_a_code_row() {
        let rows = preview_rows("Look:\n\n```python\nprint('hi')\nx = ", true);
        assert_eq!(
            rows.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>(),
            vec!["prose", "code"]
        );
        assert_eq!(rows[1].lang, "python");
        assert!(rows[1].text.contains("print('hi')"));
    }

    #[test]
    fn only_the_first_segment_is_flagged_first() {
        let rows = preview_rows("# Heading\n\nSome prose.", true);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].first);
        assert!(!rows[1].first);
    }

    #[test]
    fn growing_buffer_yields_more_rows() {
        let first = preview_rows("# Heading", true);
        let second = preview_rows("# Heading\n\nMore text arrives.", true);
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 2);
    }
}

/// `client.switch_session(path)` plus the extension-cancelled check —
/// shared by launch-time restore and an explicit `switch_session` action.
/// Mirrors `pi_core::backend::resume_session`'s cancelled-check, minus the
/// hydration step (the caller decides when to call [`hydrate_and_push`]).
async fn do_switch_session(client: &PiClient, path: &str) -> Result<(), String> {
    match client.switch_session(path).await {
        Ok(data) if data.get("cancelled").and_then(|v| v.as_bool()) == Some(true) => {
            Err("cancelled by an extension".to_string())
        }
        Ok(_) => Ok(()),
        Err(e) => Err(describe(&e)),
    }
}

const DEMO_REPLY: &str =
    "Hello from demo mode — this reply is synthetic, streamed without spawning pi.";
const DEMO_CHUNK_CHARS: usize = 5;
const DEMO_CHUNK_DELAY: Duration = Duration::from_millis(60);
const DEMO_CODE: &str = "fn main() {\n    println!(\"hello from demo mode\");\n}";

/// Synthetic counterpart to [`run`]: never touches a real `PiClient`, just
/// streams [`DEMO_REPLY`] in small chunks through the same `ChatSink`
/// callbacks on every `Send`, abortable mid-stream like the real path, then
/// (unless aborted) pushes [`demo_rows`] through `on_history_replaced` —
/// the demo-mode counterpart to [`run`]'s real `AgentSettled` -> `hydrate_
/// and_push` hydration, so the rich-rendering path is exercisable without
/// `pi` installed. The session-lifecycle actions reply `Ok(())` immediately
/// (there's no real session to act on) rather than being silently dropped —
/// a dropped `oneshot::Sender` would otherwise resolve Swift's `await` to
/// an error, making demo mode look broken for every sidebar action.
async fn run_demo(
    mut cmd_rx: mpsc::UnboundedReceiver<ChatCmd>,
    sink: Arc<dyn ChatSink>,
    dark: Arc<AtomicBool>,
) {
    sink.on_active_session_changed(None);
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            ChatCmd::Send(_) => {
                sink.on_streaming_changed(true);
                let mut aborted = false;
                for chunk in chunks(DEMO_REPLY, DEMO_CHUNK_CHARS) {
                    tokio::select! {
                        _ = tokio::time::sleep(DEMO_CHUNK_DELAY) => {
                            sink.on_text_delta(chunk.to_string());
                        }
                        next = cmd_rx.recv() => {
                            match next {
                                Some(ChatCmd::Abort) | None => {
                                    aborted = true;
                                    break;
                                }
                                // A sidebar action arriving mid-stream: reply so
                                // Swift's await doesn't error on a dropped
                                // sender, without otherwise disturbing the
                                // in-flight demo stream.
                                Some(other) => reply_demo_action(other, sink.as_ref()),
                            }
                        }
                    }
                }
                if !aborted {
                    sink.on_turn_end();
                    sink.on_history_replaced(demo_rows(dark.load(Ordering::Relaxed)));
                }
                sink.on_streaming_changed(false);
            }
            ChatCmd::Abort => {} // nothing streaming; no-op
            other => reply_demo_action(other, sink.as_ref()),
        }
    }
}

/// A prose row plus a syntax-highlighted code row — enough for `RowRecord`
/// rendering to be visibly exercised (markdown *and* `code_lines`/
/// `ColoredSpan`s) without a real `pi` session to hydrate from.
fn demo_rows(dark: bool) -> Vec<RowRecord> {
    let prose = pi_render::RowSpec {
        kind: "prose",
        markdown: Some(DEMO_REPLY.to_string()),
        first: true,
        raw: DEMO_REPLY.to_string(),
        ..pi_render::RowSpec::default()
    };
    let code = pi_render::RowSpec {
        kind: "code",
        code_lines: pi_render::highlight::highlight_lines(DEMO_CODE, "rust", dark),
        text: DEMO_CODE.to_string(),
        lang: "rust".to_string(),
        raw: DEMO_CODE.to_string(),
        ..pi_render::RowSpec::default()
    };
    vec![prose, code].into_iter().map(RowRecord::from).collect()
}

fn reply_demo_action(cmd: ChatCmd, sink: &dyn ChatSink) {
    match cmd {
        ChatCmd::SwitchProject { reply, .. }
        | ChatCmd::NewSession { reply }
        | ChatCmd::DeleteSession { reply, .. } => {
            sink.on_history_replaced(Vec::new());
            sink.on_active_session_changed(None);
            let _ = reply.send(Ok(()));
        }
        ChatCmd::SwitchSession { reply, .. } => {
            sink.on_history_replaced(demo_rows(true));
            sink.on_active_session_changed(None);
            let _ = reply.send(Ok(()));
        }
        ChatCmd::RenameSession { reply, .. } => {
            let _ = reply.send(Ok(()));
        }
        ChatCmd::ServeRapidMlxModel { reply, .. } => {
            let _ = reply.send(Ok(()));
        }
        ChatCmd::ReplyExtensionDialog { .. } => {} // no-op: demo mode never emits dialogs
        ChatCmd::RefreshModels { reply } => {
            let _ = reply.send(Ok(demo_models()));
        }
        ChatCmd::SetModel { reply, .. } => {
            // No real server to probe — `Hidden` keeps the demo dot state
            // consistent with `demo_models`'s synthetic entries never
            // reporting `is_local`.
            sink.on_server_dot_changed(ServerDotState::Hidden);
            let _ = reply.send(Ok(()));
        }
        ChatCmd::RefreshThinkingLevels { reply } => {
            let _ = reply.send(Ok(Vec::new())); // no thinking levels in demo mode
        }
        ChatCmd::SetThinkingLevel { reply, .. } => {
            let _ = reply.send(Ok(()));
        }
        ChatCmd::AttachPath { .. } => {} // no-op: demo mode has no real files to attach
        ChatCmd::RemoveAttachment { .. } => {} // no-op: demo mode never queues attachments
        ChatCmd::Send(_) | ChatCmd::Abort => {
            unreachable!("Send/Abort are handled by run_demo's own match arms")
        }
    }
}

/// Enough entries to exercise the composer picker's checkmark/switch UI
/// without a real `pi` — mirrors `demo_rows`' role for the transcript.
fn demo_models() -> Vec<ModelRecord> {
    vec![
        ModelRecord {
            provider: "demo".to_string(),
            id: "demo-model".to_string(),
            label: "Demo Model · demo · free · local".to_string(),
            is_current: true,
        },
        ModelRecord {
            provider: "demo".to_string(),
            id: "demo-model-2".to_string(),
            label: "Demo Model 2 · demo · $1/$2".to_string(),
            is_current: false,
        },
    ]
}

/// Split into ~n-char chunks on char boundaries (crude token simulation) —
/// same shape as `pi_core::backend`'s demo chunker, reimplemented here since
/// this crate doesn't depend on pi-core (see the crate doc comment).
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

fn apply(event: &Event, sink: &dyn ChatSink) {
    match event {
        Event::AgentStart => sink.on_streaming_changed(true),
        Event::AgentSettled => sink.on_streaming_changed(false),
        Event::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::TextDelta { delta, .. },
            ..
        } => sink.on_text_delta(delta.clone()),
        Event::MessageEnd { .. } => sink.on_turn_end(),
        Event::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::Error { reason },
            ..
        } if reason != "aborted" => report_error(sink, format!("model error: {reason}")),
        Event::ExtensionUiRequest(req)
            if matches!(
                req.method.as_str(),
                "select" | "confirm" | "input" | "editor"
            ) =>
        {
            sink.on_extension_dialog(ExtensionDialogRecord::from(req.clone()));
        }
        // `notify`/`setStatus`/`setWidget`/`setTitle`/`set_editor_text` (and any future
        // method) — explicit SW5 scope cut, not delivered anywhere yet. See the plan's
        // Design section for why: these are fire-and-forget, non-dialog signals, out of
        // scope for a milestone named "dialogs & permission gating."
        Event::ExtensionUiRequest(_) => {}
        Event::ExtensionError { error, .. } => {
            report_error(sink, format!("extension error: {error}"))
        }
        _ => {}
    }
}

fn describe(e: &PiError) -> String {
    e.to_string()
}

/// `ChatSink::on_error`'s single call site, everywhere — guarantees every
/// push-based chat/session error is also captured via `tracing` (and, once
/// `ensure_logging_initialized` has run, macOS unified logging), not just
/// shown as a one-line status caption that gets overwritten by the next
/// event.
fn report_error(sink: &dyn ChatSink, message: String) {
    tracing::error!("{message}");
    sink.on_error(message);
}

/// Ensures `pi` — and whatever `pi` itself shells out to (it's commonly a
/// `#!/usr/bin/env node` script, so the OS needs `node` on `PATH` just to
/// execute the shebang; `pi`'s own tool calls need a working `PATH` too) —
/// is actually reachable by this process, not merely locatable in
/// isolation. Works around a macOS-specific gap: a `.app` bundle launched
/// normally (Finder double-click, Dock, Xcode's own Run) gets a minimal
/// default `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`) that doesn't include
/// wherever the user's shell rc files put `pi`/`node` (Homebrew's
/// `/opt/homebrew/bin`, npm global bins, nvm/asdf shims, ...) — only a
/// terminal-launched process (`cargo run`, a shell) inherits that.
///
/// Merges the *whole* `PATH` from the user's login shell into this
/// process's environment (the same fix Electron's `fix-path` / VS Code use
/// for this exact problem) rather than just resolving the one directory
/// `pi` happens to live in, since `pi` and its children need more than that
/// one binary to be reachable. A no-op (and doesn't bother asking the
/// shell) whenever `pi` already resolves — the common case for a
/// terminal-launched dev build.
async fn ensure_usable_path() {
    if let Some(path_var) = std::env::var_os("PATH") {
        if find_in_path_var("pi", &path_var).is_some() {
            return;
        }
    }
    let Some(login_path) = login_shell_path().await else {
        tracing::warn!(
            "pi not found on PATH, and the login-shell PATH lookup itself failed or timed out \
             — proceeding with the original PATH unchanged"
        );
        return;
    };
    let current = std::env::var_os("PATH").unwrap_or_default();
    let merged = merge_path_vars(&current, &login_path);
    tracing::debug!("pi not found on PATH; merged in the login shell's PATH");
    // SAFETY: called once, synchronously, before `PiSession::new` spawns any
    // other thread that could read `PATH` concurrently.
    unsafe {
        std::env::set_var("PATH", merged);
    }
}

/// First `PATH`-entry match for `name`, if any.
fn find_in_path_var(name: &str, path_var: &std::ffi::OsStr) -> Option<std::path::PathBuf> {
    std::env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// `current`'s directories, followed by any of `additional`'s directories
/// not already present — order preserved, no duplicates. Drops empty
/// components from either side: `split_paths` yields one for an empty
/// `OsStr`, and on Unix an empty `PATH` component conventionally means "the
/// current directory," not "nothing" — never worth adding implicitly.
fn merge_path_vars(current: &std::ffi::OsStr, additional: &str) -> std::ffi::OsString {
    let is_real = |dir: &std::path::PathBuf| !dir.as_os_str().is_empty();
    let mut dirs: Vec<std::path::PathBuf> =
        std::env::split_paths(current).filter(is_real).collect();
    for dir in std::env::split_paths(additional).filter(is_real) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| current.to_os_string())
}

/// Runs `$SHELL -l -c 'echo -n "$PATH"'` (login, not interactive — Homebrew's
/// own install instructions put its `shellenv` line in `.zprofile`/`.profile`,
/// which a login shell sources regardless of interactivity, so `-l` alone is
/// enough and avoids `-i`'s hazards: prompt theming, plugin managers, or
/// anything else that assumes a real TTY). Bounded by a timeout since an rc
/// file could in principle hang; `None` on any failure leaves `PATH` alone.
async fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let output = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new(&shell)
            .arg("-l")
            .arg("-c")
            .arg("echo -n \"$PATH\"")
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

#[cfg(test)]
mod find_in_path_var_tests {
    use super::find_in_path_var;
    use std::env::join_paths;
    use std::ffi::OsStr;
    use std::fs;

    #[test]
    fn empty_path_finds_nothing() {
        assert_eq!(find_in_path_var("pi", OsStr::new("")), None);
    }

    #[test]
    fn finds_an_executable_in_one_of_several_directories() {
        let empty_dir = tempfile::tempdir().unwrap();
        let bin_dir = tempfile::tempdir().unwrap();
        let pi_path = bin_dir.path().join("pi");
        fs::write(&pi_path, "#!/bin/sh\necho hi\n").unwrap();

        let path_var = join_paths([empty_dir.path(), bin_dir.path()]).unwrap();
        let found = find_in_path_var("pi", &path_var).expect("pi should be found");
        assert_eq!(found, pi_path);
    }

    #[test]
    fn returns_the_first_match_in_path_order() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        fs::write(first.path().join("pi"), "first").unwrap();
        fs::write(second.path().join("pi"), "second").unwrap();

        let path_var = join_paths([first.path(), second.path()]).unwrap();
        let found = find_in_path_var("pi", &path_var).unwrap();
        assert_eq!(found, first.path().join("pi"));
    }

    #[test]
    fn a_directory_named_pi_does_not_count_as_a_match() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("pi")).unwrap();

        let path_var = join_paths([dir.path()]).unwrap();
        assert_eq!(find_in_path_var("pi", &path_var), None);
    }
}

#[cfg(test)]
mod merge_path_vars_tests {
    use super::merge_path_vars;
    use std::env::split_paths;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    fn dirs(path_var: &std::ffi::OsStr) -> Vec<PathBuf> {
        split_paths(path_var).collect()
    }

    #[test]
    fn current_directories_come_first_unchanged() {
        let merged = merge_path_vars(OsStr::new("/usr/bin:/bin"), "/opt/homebrew/bin");
        assert_eq!(
            dirs(&merged),
            vec![
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
                PathBuf::from("/opt/homebrew/bin"),
            ]
        );
    }

    #[test]
    fn duplicates_from_additional_are_dropped() {
        let merged = merge_path_vars(
            OsStr::new("/usr/bin:/opt/homebrew/bin"),
            "/opt/homebrew/bin:/opt/homebrew/sbin",
        );
        assert_eq!(
            dirs(&merged),
            vec![
                PathBuf::from("/usr/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/opt/homebrew/sbin"),
            ]
        );
    }

    #[test]
    fn empty_current_still_picks_up_everything_additional() {
        let merged = merge_path_vars(OsStr::new(""), "/opt/homebrew/bin");
        assert_eq!(dirs(&merged), vec![PathBuf::from("/opt/homebrew/bin")]);
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{ChatSink, ExtensionDialogRecord, RowRecord, ServerDotState, SessionStatsRecord};
    use std::sync::Mutex;

    /// Records every `ChatSink` call as a short tagged string, in order —
    /// shared by `apply_tests` (synchronous, one event at a time) and
    /// `run_demo_tests` (a real streamed sequence).
    #[derive(Default)]
    pub struct RecordingSink {
        pub events: Mutex<Vec<String>>,
    }

    impl ChatSink for RecordingSink {
        fn on_text_delta(&self, delta: String) {
            self.events.lock().unwrap().push(format!("delta:{delta}"));
        }
        fn on_turn_end(&self) {
            self.events.lock().unwrap().push("turn_end".to_string());
        }
        fn on_streaming_changed(&self, streaming: bool) {
            self.events
                .lock()
                .unwrap()
                .push(format!("streaming:{streaming}"));
        }
        fn on_error(&self, message: String) {
            self.events.lock().unwrap().push(format!("error:{message}"));
        }
        fn on_active_session_changed(&self, path: Option<String>) {
            self.events
                .lock()
                .unwrap()
                .push(format!("active_session:{path:?}"));
        }
        fn on_history_replaced(&self, rows: Vec<RowRecord>) {
            self.events
                .lock()
                .unwrap()
                .push(format!("history_replaced:{}", rows.len()));
        }
        fn on_extension_dialog(&self, request: ExtensionDialogRecord) {
            self.events.lock().unwrap().push(format!(
                "extension_dialog:{}:{}",
                request.method, request.id
            ));
        }
        fn on_server_dot_changed(&self, state: ServerDotState) {
            self.events
                .lock()
                .unwrap()
                .push(format!("server_dot:{state:?}"));
        }
        fn on_thinking_row_changed(&self, id: String, row: RowRecord) {
            self.events.lock().unwrap().push(format!(
                "thinking_row:{id}:running={}:text={}",
                row.running, row.text
            ));
        }
        fn on_tool_row_changed(&self, id: String, row: RowRecord) {
            self.events.lock().unwrap().push(format!(
                "tool_row:{id}:running={}:{}",
                row.running, row.text
            ));
        }
        fn on_session_stats_changed(&self, stats: SessionStatsRecord) {
            self.events.lock().unwrap().push(format!(
                "session_stats:{}:{}:{}",
                stats.tokens_label, stats.cost, stats.context_percent
            ));
        }
        fn on_pending_attachments_changed(&self, names: Vec<String>) {
            self.events
                .lock()
                .unwrap()
                .push(format!("pending_attachments:{names:?}"));
        }
        fn on_composer_append(&self, text: String) {
            self.events
                .lock()
                .unwrap()
                .push(format!("composer_append:{text}"));
        }
    }
}

#[cfg(test)]
mod apply_tests {
    use super::test_support::RecordingSink;
    use super::*;

    fn mk_delta(delta: AssistantMessageEvent) -> Event {
        Event::MessageUpdate {
            message: serde_json::Value::Null,
            assistant_message_event: delta,
        }
    }

    #[test]
    fn agent_start_and_settled_toggle_streaming() {
        let sink = RecordingSink::default();
        apply(&Event::AgentStart, &sink);
        apply(&Event::AgentSettled, &sink);
        assert_eq!(
            *sink.events.lock().unwrap(),
            vec!["streaming:true".to_string(), "streaming:false".to_string()]
        );
    }

    #[test]
    fn text_delta_forwards_the_chunk_verbatim() {
        let sink = RecordingSink::default();
        apply(
            &mk_delta(AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hel".to_string(),
            }),
            &sink,
        );
        assert_eq!(*sink.events.lock().unwrap(), vec!["delta:hel".to_string()]);
    }

    #[test]
    fn message_end_fires_turn_end() {
        let sink = RecordingSink::default();
        apply(
            &Event::MessageEnd {
                message: serde_json::Value::Null,
            },
            &sink,
        );
        assert_eq!(*sink.events.lock().unwrap(), vec!["turn_end".to_string()]);
    }

    #[test]
    fn non_aborted_model_error_is_reported() {
        let sink = RecordingSink::default();
        apply(
            &mk_delta(AssistantMessageEvent::Error {
                reason: "context_limit".to_string(),
            }),
            &sink,
        );
        assert_eq!(
            *sink.events.lock().unwrap(),
            vec!["error:model error: context_limit".to_string()]
        );
    }

    #[test]
    fn aborted_model_error_is_swallowed() {
        let sink = RecordingSink::default();
        apply(
            &mk_delta(AssistantMessageEvent::Error {
                reason: "aborted".to_string(),
            }),
            &sink,
        );
        assert!(sink.events.lock().unwrap().is_empty());
    }

    #[test]
    fn unrelated_events_are_ignored() {
        let sink = RecordingSink::default();
        apply(&Event::TurnStart, &sink);
        assert!(sink.events.lock().unwrap().is_empty());
    }

    fn mk_dialog_request(method: &str) -> pi_rpc::ExtensionUiRequest {
        pi_rpc::ExtensionUiRequest {
            id: "req-1".to_string(),
            method: method.to_string(),
            title: None,
            message: None,
            options: None,
            placeholder: None,
            prefill: None,
            timeout: None,
            rest: serde_json::Map::new(),
        }
    }

    #[test]
    fn blocking_dialog_methods_reach_on_extension_dialog() {
        for method in ["select", "confirm", "input", "editor"] {
            let sink = RecordingSink::default();
            apply(&Event::ExtensionUiRequest(mk_dialog_request(method)), &sink);
            assert_eq!(
                *sink.events.lock().unwrap(),
                vec![format!("extension_dialog:{method}:req-1")]
            );
        }
    }

    #[test]
    fn fire_and_forget_dialog_methods_are_dropped() {
        for method in [
            "notify",
            "setStatus",
            "setWidget",
            "setTitle",
            "set_editor_text",
        ] {
            let sink = RecordingSink::default();
            apply(&Event::ExtensionUiRequest(mk_dialog_request(method)), &sink);
            assert!(sink.events.lock().unwrap().is_empty(), "method={method}");
        }
    }

    #[test]
    fn extension_error_is_reported() {
        let sink = RecordingSink::default();
        apply(
            &Event::ExtensionError {
                extension_path: "some/ext".to_string(),
                event: "tool_call".to_string(),
                error: "boom".to_string(),
            },
            &sink,
        );
        assert_eq!(
            *sink.events.lock().unwrap(),
            vec!["error:extension error: boom".to_string()]
        );
    }
}

#[cfg(test)]
mod run_demo_tests {
    use super::test_support::RecordingSink;
    use super::*;

    fn spawn_demo(
        sink: Arc<RecordingSink>,
    ) -> (mpsc::UnboundedSender<ChatCmd>, tokio::task::JoinHandle<()>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let dark = Arc::new(AtomicBool::new(true));
        (cmd_tx, tokio::spawn(run_demo(cmd_rx, sink, dark)))
    }

    #[tokio::test]
    async fn a_send_streams_the_full_reply_then_settles() {
        let sink = Arc::new(RecordingSink::default());
        let (cmd_tx, demo) = spawn_demo(sink.clone());
        cmd_tx.send(ChatCmd::Send("hi".to_string())).unwrap();
        // A closed *and empty* command channel makes the inner select's
        // `cmd_rx.recv()` branch resolve immediately (there's nothing left
        // to wait for), which run_demo treats the same as an explicit Abort
        // — so the sender must outlive the whole reply, not just the Send.
        // Real-time wait (not paused time): the full reply is well under a
        // second at DEMO_CHUNK_DELAY's cadence, so a generous fixed margin
        // keeps this simple and doesn't depend on paused-time/spawned-task
        // interaction subtleties.
        tokio::time::sleep(Duration::from_secs(3)).await;
        drop(cmd_tx);
        demo.await.unwrap();

        let events = sink.events.lock().unwrap();
        assert_eq!(
            events.first(),
            Some(&"active_session:None".to_string()),
            "run_demo fires an initial on_active_session_changed(None) before anything streams"
        );
        assert_eq!(events[1], "streaming:true");
        assert_eq!(events.last(), Some(&"streaming:false".to_string()));
        assert_eq!(
            events[events.len() - 3],
            "turn_end",
            "turn_end fires before history_replaced and the final streaming:false"
        );
        assert_eq!(
            events[events.len() - 2],
            "history_replaced:2",
            "settling pushes the synthetic prose+code demo rows"
        );
        let reassembled: String = events[2..events.len() - 3]
            .iter()
            .map(|e| e.strip_prefix("delta:").expect("only deltas in between"))
            .collect();
        assert_eq!(reassembled, DEMO_REPLY);
    }

    #[tokio::test]
    async fn abort_mid_stream_stops_before_turn_end() {
        let sink = Arc::new(RecordingSink::default());
        let (cmd_tx, demo) = spawn_demo(sink.clone());
        cmd_tx.send(ChatCmd::Send("hi".to_string())).unwrap();
        // Give the first chunk's sleep a moment to be in-flight, then abort
        // before the reply finishes streaming.
        tokio::time::sleep(DEMO_CHUNK_DELAY / 2).await;
        cmd_tx.send(ChatCmd::Abort).unwrap();
        drop(cmd_tx);
        demo.await.unwrap();

        let events = sink.events.lock().unwrap();
        assert!(
            !events.contains(&"turn_end".to_string()),
            "aborted streams never fire turn_end: {events:?}"
        );
        assert_eq!(events.last(), Some(&"streaming:false".to_string()));
    }
}

#[cfg(test)]
mod reply_demo_action_tests {
    use super::test_support::RecordingSink;
    use super::*;

    #[tokio::test]
    async fn switch_project_replies_ok_and_clears_history_and_active_session() {
        let sink = RecordingSink::default();
        let (tx, rx) = oneshot::channel();
        reply_demo_action(
            ChatCmd::SwitchProject {
                path: "/x".to_string(),
                reply: tx,
            },
            &sink,
        );
        assert_eq!(rx.await.unwrap(), Ok(()));
        assert_eq!(
            *sink.events.lock().unwrap(),
            vec![
                "history_replaced:0".to_string(),
                "active_session:None".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn new_session_replies_ok_and_clears_history_and_active_session() {
        let sink = RecordingSink::default();
        let (tx, rx) = oneshot::channel();
        reply_demo_action(ChatCmd::NewSession { reply: tx }, &sink);
        assert_eq!(rx.await.unwrap(), Ok(()));
        assert_eq!(
            *sink.events.lock().unwrap(),
            vec![
                "history_replaced:0".to_string(),
                "active_session:None".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn delete_session_replies_ok_and_clears_history_and_active_session() {
        let sink = RecordingSink::default();
        let (tx, rx) = oneshot::channel();
        reply_demo_action(
            ChatCmd::DeleteSession {
                path: "/x".to_string(),
                reply: tx,
            },
            &sink,
        );
        assert_eq!(rx.await.unwrap(), Ok(()));
        assert_eq!(
            *sink.events.lock().unwrap(),
            vec![
                "history_replaced:0".to_string(),
                "active_session:None".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn switch_session_replies_ok_and_pushes_demo_history() {
        let sink = RecordingSink::default();
        let (tx, rx) = oneshot::channel();
        reply_demo_action(
            ChatCmd::SwitchSession {
                path: "/x".to_string(),
                reply: tx,
            },
            &sink,
        );
        assert_eq!(rx.await.unwrap(), Ok(()));
        assert_eq!(
            *sink.events.lock().unwrap(),
            vec![
                "history_replaced:2".to_string(),
                "active_session:None".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn rename_session_replies_ok_without_touching_active_session() {
        let sink = RecordingSink::default();
        let (tx, rx) = oneshot::channel();
        reply_demo_action(
            ChatCmd::RenameSession {
                name: "x".to_string(),
                reply: tx,
            },
            &sink,
        );
        assert_eq!(rx.await.unwrap(), Ok(()));
        assert!(
            sink.events.lock().unwrap().is_empty(),
            "renaming doesn't change which session is active"
        );
    }

    #[tokio::test]
    async fn serve_rapid_mlx_model_replies_ok_without_touching_active_session() {
        let sink = RecordingSink::default();
        let (tx, rx) = oneshot::channel();
        reply_demo_action(
            ChatCmd::ServeRapidMlxModel {
                alias: "demo-model".to_string(),
                reply: tx,
            },
            &sink,
        );
        assert_eq!(rx.await.unwrap(), Ok(()));
        assert!(
            sink.events.lock().unwrap().is_empty(),
            "there's no real session to act on in demo mode"
        );
    }
}
