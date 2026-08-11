//! UI-toolkit-agnostic core for driving the `pi` coding agent.
//!
//! Everything here is Slint-free (and toolkit-free in general): the pi-rpc
//! event stream is projected onto plain `RowSpec` values pushed through a
//! `UiSink` trait, so any frontend (Slint today, others later) can implement
//! that one trait instead of duplicating the state machine that drives pi.
