//! UI-toolkit-agnostic core for driving the `pi` coding agent.
//!
//! Everything here is Slint-free (and toolkit-free in general): the pi-rpc
//! event stream is projected onto plain `RowSpec` values pushed through a
//! `UiSink` trait, so any frontend (Slint today, others later) can implement
//! that one trait instead of duplicating the state machine that drives pi.

pub mod attach;
pub mod backend;
pub mod demo_sessions;
pub mod density;
pub mod local;
pub mod palette;
#[cfg(test)]
pub mod recording_ui_sink;

// Re-exported so existing `pi_core::highlight`/`pi_core::segmenter` call
// sites (slinty-pi's `code_lines_model`/`table_rows_model`) keep resolving
// unchanged now that these modules live in the lean, shared `pi-render`
// crate (also depended on directly by `pi-core-ffi`, which doesn't otherwise
// depend on `pi-core`).
pub use pi_render::{highlight, segmenter};
