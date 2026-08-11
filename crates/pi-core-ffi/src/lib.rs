//! UniFFI boundary for the SW1 spike: a minimal chat session over
//! `pi --mode rpc`, exposed to Swift.
//!
//! Deliberately smaller than `pi_core::backend`'s full `UiSink`/`RowSpec`
//! surface — this proves the FFI mechanism (a Rust trait implemented in
//! Swift, called from a tokio worker thread) and one real prompt round-trip,
//! without guessing the eventual Swift-facing shape of markdown/table
//! rendering before any real Swift UI exists to consume it. See
//! `docs/plans/SW1-ffi-spike-and-chat-window.md`.
//!
//! Threading contract mirrors `pi_core::backend::UiSink`: `ChatSink` methods
//! are `Send + Sync`, fire-and-forget, called from a tokio worker thread
//! owned by this crate. The Swift implementation is responsible for hopping
//! to `@MainActor` on every callback, the same responsibility
//! `Weak::upgrade_in_event_loop` discharges on the Slint side.

use std::sync::Arc;

use pi_rpc::{AssistantMessageEvent, Event, PiClient, PiError, PiOptions};
use tokio::sync::mpsc;

uniffi::setup_scaffolding!();

/// Backend -> UI push. Implemented in Swift; called from Rust on a tokio
/// worker thread.
#[uniffi::export(with_foreign)]
pub trait ChatSink: Send + Sync {
    fn on_text_delta(&self, delta: String);
    fn on_turn_end(&self);
    fn on_streaming_changed(&self, streaming: bool);
    fn on_error(&self, message: String);
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum PiSessionError {
    #[error("failed to start pi: {0}")]
    Spawn(String),
}

enum ChatCmd {
    Send(String),
    Abort,
}

/// A single `pi --mode rpc` child process plus the tokio runtime that owns
/// it end-to-end, mirroring `pi_core::backend::pi_backend`'s shape (a
/// background runtime, a command channel from the UI, an event loop pushing
/// through the sink) but scoped to exactly what this spike needs.
#[derive(uniffi::Object)]
pub struct PiSession {
    cmd_tx: mpsc::UnboundedSender<ChatCmd>,
    // Keeps the runtime (and therefore the spawned event-loop task and the
    // `pi` child it owns) alive for as long as Swift holds this object.
    _runtime: tokio::runtime::Runtime,
}

#[uniffi::export]
impl PiSession {
    /// Spawns `pi --mode rpc` in the current working directory and starts
    /// forwarding its events to `sink`. Blocks (briefly — a process spawn +
    /// RPC handshake) until the child is up or has failed to start.
    #[uniffi::constructor]
    pub fn new(sink: Arc<dyn ChatSink>) -> Result<Self, PiSessionError> {
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| PiSessionError::Spawn(format!("could not start a tokio runtime: {e}")))?;
        let (client, events) = runtime
            .block_on(PiClient::spawn(PiOptions::default()))
            .map_err(|e| PiSessionError::Spawn(e.to_string()))?;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        runtime.spawn(run(client, events, cmd_rx, sink));
        Ok(Self {
            cmd_tx,
            _runtime: runtime,
        })
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
}

/// Owns the live `PiClient` end-to-end: drains UI commands and pi's event
/// stream, forwarding the minimal slice `ChatSink` cares about. Exits (and
/// drops `client`, killing the child) when the command channel closes —
/// Swift dropping its last `PiSession` reference is what triggers that.
async fn run(
    client: PiClient,
    mut events: mpsc::UnboundedReceiver<Event>,
    mut cmd_rx: mpsc::UnboundedReceiver<ChatCmd>,
    sink: Arc<dyn ChatSink>,
) {
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(ChatCmd::Send(text)) => {
                        if let Err(e) = client.prompt(text).await {
                            sink.on_error(describe(&e));
                        }
                    }
                    Some(ChatCmd::Abort) => {
                        if let Err(e) = client.abort().await {
                            sink.on_error(describe(&e));
                        }
                    }
                    None => return,
                }
            }
            event = events.recv() => {
                let Some(event) = event else {
                    sink.on_error("pi exited".to_string());
                    return;
                };
                apply(&event, sink.as_ref());
            }
        }
    }
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
        } if reason != "aborted" => sink.on_error(format!("model error: {reason}")),
        _ => {}
    }
}

fn describe(e: &PiError) -> String {
    e.to_string()
}

#[cfg(test)]
mod apply_tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<String>>,
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
    }

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
}
