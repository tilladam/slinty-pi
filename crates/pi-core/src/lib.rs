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
pub mod highlight;
pub mod local;
pub mod palette;
#[cfg(test)]
pub mod recording_ui_sink;
pub mod segmenter;
