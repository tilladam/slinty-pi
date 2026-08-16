//! Test-only `UiSink` implementation that records every call instead of
//! rendering anything, so `Transcript`'s live event-processing path (text/
//! thinking streaming, tool-call lifecycle, compaction, extension-UI
//! requests, ...) can be unit tested directly. Before `UiSink` existed as a
//! trait, this wasn't possible: the only implementation (Slint's `Ui`)
//! needed a real `slint::Weak<AppWindow>`, so that path was only ever
//! exercised manually (demo mode, the `SLINTY_*_AFTER` env hooks) or through
//! `hydrate_rowspecs`' separate replay path.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::backend::{RapidMlxPanelData, RouterPanelData, RowSpec, UiSink};
use crate::palette::PaletteEntry;

/// One recorded `UiSink` call, in the same shape as the method that produced
/// it.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    Push(RowSpec),
    Set(usize, RowSpec),
    PushAll(Vec<RowSpec>),
    Clear,
    Truncate(usize),
    SetStreaming(bool),
    SetStatus(String),
    SetContextPercent(f32),
    SetQueue(Vec<(&'static str, String)>),
    SetModels(Vec<String>, i32),
    SetServerDot(i32),
    SetThinking(Vec<String>, i32),
    SetProjects(Vec<String>, Vec<String>, String),
    SetSidebarSessions(Vec<(String, String, String, bool, String)>),
    SetTree(Vec<(String, i32, String, String, bool, bool)>),
    SetRapidMlxPanel(RapidMlxPanelData),
    SetRouterPanel(RouterPanelData),
    ShowModelsPanel,
    SetHfSearchResults(Vec<(String, bool, i32, Vec<String>)>),
    SetAuthEntries(Vec<String>),
    SetOllamaPanel(bool, String, i32),
    SetPaletteEntries(Vec<PaletteEntry>),
    SetComposerText(String),
    AppendComposerText(PathBuf),
    SetPendingAttachments(Vec<String>),
    SetDragHover(bool),
}

/// Records every `UiSink` call into an in-order log instead of rendering
/// anything, so tests can assert on exactly what the backend told the UI to
/// do. `Clone` shares the same underlying log (via `Arc`), so a test can
/// hand one clone to a `Transcript` (which takes ownership of its `UiSink`
/// as `Box<dyn UiSink>`) and keep another to inspect afterward.
#[derive(Default, Clone)]
pub struct RecordingUiSink {
    log: Arc<Mutex<Vec<UiEvent>>>,
}

impl RecordingUiSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// A snapshot of every event recorded so far, in call order.
    pub fn events(&self) -> Vec<UiEvent> {
        self.log.lock().unwrap().clone()
    }

    /// Just the row-mutating events (push/set/push_all/clear/truncate) — what
    /// most `Transcript` tests care about, without the noise of status/model
    /// setters in between.
    pub fn rows(&self) -> Vec<UiEvent> {
        self.events()
            .into_iter()
            .filter(|e| {
                matches!(
                    e,
                    UiEvent::Push(_)
                        | UiEvent::Set(..)
                        | UiEvent::PushAll(_)
                        | UiEvent::Clear
                        | UiEvent::Truncate(_)
                )
            })
            .collect()
    }

    fn record(&self, event: UiEvent) {
        self.log.lock().unwrap().push(event);
    }
}

impl UiSink for RecordingUiSink {
    fn push(&self, spec: RowSpec) {
        self.record(UiEvent::Push(spec));
    }

    fn set(&self, index: usize, spec: RowSpec) {
        self.record(UiEvent::Set(index, spec));
    }

    fn push_all(&self, specs: Vec<RowSpec>) {
        self.record(UiEvent::PushAll(specs));
    }

    fn clear(&self) {
        self.record(UiEvent::Clear);
    }

    fn truncate(&self, len: usize) {
        self.record(UiEvent::Truncate(len));
    }

    fn set_streaming(&self, streaming: bool) {
        self.record(UiEvent::SetStreaming(streaming));
    }

    fn set_status(&self, status: String) {
        self.record(UiEvent::SetStatus(status));
    }

    fn set_context_percent(&self, percent: f32) {
        self.record(UiEvent::SetContextPercent(percent));
    }

    fn set_queue(&self, items: Vec<(&'static str, String)>) {
        self.record(UiEvent::SetQueue(items));
    }

    fn set_models(&self, labels: Vec<String>, index: i32) {
        self.record(UiEvent::SetModels(labels, index));
    }

    fn set_server_dot(&self, state: i32) {
        self.record(UiEvent::SetServerDot(state));
    }

    fn set_thinking(&self, labels: Vec<String>, index: i32) {
        self.record(UiEvent::SetThinking(labels, index));
    }

    fn set_projects(&self, labels: Vec<String>, paths: Vec<String>, current_name: String) {
        self.record(UiEvent::SetProjects(labels, paths, current_name));
    }

    fn set_sidebar_sessions(&self, rows: Vec<(String, String, String, bool, String)>) {
        self.record(UiEvent::SetSidebarSessions(rows));
    }

    fn set_tree(&self, rows: Vec<(String, i32, String, String, bool, bool)>) {
        self.record(UiEvent::SetTree(rows));
    }

    fn set_rapid_mlx_panel(&self, data: RapidMlxPanelData) {
        self.record(UiEvent::SetRapidMlxPanel(data));
    }

    fn set_router_panel(&self, data: RouterPanelData) {
        self.record(UiEvent::SetRouterPanel(data));
    }

    fn show_models_panel(&self) {
        self.record(UiEvent::ShowModelsPanel);
    }

    fn set_hf_search_results(&self, results: Vec<(String, bool, i32, Vec<String>)>) {
        self.record(UiEvent::SetHfSearchResults(results));
    }

    fn set_auth_entries(&self, labels: Vec<String>) {
        self.record(UiEvent::SetAuthEntries(labels));
    }

    fn set_ollama_panel(&self, detected: bool, summary: String, model_count: i32) {
        self.record(UiEvent::SetOllamaPanel(detected, summary, model_count));
    }

    fn set_palette_entries(&self, entries: Vec<PaletteEntry>) {
        self.record(UiEvent::SetPaletteEntries(entries));
    }

    fn set_composer_text(&self, text: String) {
        self.record(UiEvent::SetComposerText(text));
    }

    fn append_composer_text(&self, path: &std::path::Path) {
        self.record(UiEvent::AppendComposerText(path.to_path_buf()));
    }

    fn set_pending_attachments(&self, names: Vec<String>) {
        self.record(UiEvent::SetPendingAttachments(names));
    }

    fn set_drag_hover(&self, hovering: bool) {
        self.record(UiEvent::SetDragHover(hovering));
    }
}
