//! Stateless message/segment -> row rendering, shared by `pi-core` (Slint's
//! live streaming path) and `pi-core-ffi` (Swift's history hydration).
//!
//! Kept separate from `pi-core` (rather than folded in) so a lean FFI
//! consumer can depend on just this — no `reqwest`/`sysinfo`/`directories`/
//! `nucleo-matcher` — for markdown segmentation, syntax highlighting, and
//! turning a `get_messages` payload into rows.

pub mod highlight;
pub mod segmenter;

mod hydrate;

pub use hydrate::{
    first_line, format_tokens, hydrate_rowspecs, spec_for_segment, tail, tool_summary,
    user_content_text, RowSpec, TOOL_DETAIL_LIMIT,
};
