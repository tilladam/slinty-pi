//! Client for the pi coding agent's RPC mode.
//!
//! Spawns `pi --mode rpc` as a child process and speaks its JSONL protocol:
//! commands with correlated responses on stdin/stdout, plus a stream of agent
//! events (streaming deltas, tool executions, queue changes, compaction, and
//! extension UI requests).
//!
//! ```no_run
//! # async fn demo() -> Result<(), pi_rpc::PiError> {
//! use pi_rpc::{PiClient, PiOptions, Event, AssistantMessageEvent};
//!
//! let (client, mut events) = PiClient::spawn(PiOptions::default()).await?;
//! client.prompt("Hello!").await?;
//! while let Some(event) = events.recv().await {
//!     if let Event::MessageUpdate { assistant_message_event: AssistantMessageEvent::TextDelta { delta, .. }, .. } = event {
//!         print!("{delta}");
//!     }
//! }
//! # Ok(()) }
//! ```

mod client;
mod types;

pub use client::{refresh_model_catalog, PiClient, PiError, PiOptions};
pub use types::{
    content_text, AssistantMessageEvent, Command, Event, ExtensionUiReply, ExtensionUiRequest,
    ImageContent, Response, StreamingBehavior, ThinkingLevel,
};
