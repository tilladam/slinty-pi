//! Read-only index over pi's on-disk session JSONL tree
//! (`~/.pi/agent/sessions`), independent of any running `pi` process — pi
//! stays the sole writer, this crate only ever reads. See
//! `docs/session-format.md` in pi-coding-agent for the on-disk schema this
//! is built against.

mod scan;
mod tree;
pub mod types;
mod watch;

pub use scan::{
    decode_project_dir, default_sessions_root, encode_project_dir, list_projects, list_sessions,
    parse_meta, project_session_dir, search, MetaCache, Project, SessionMeta,
};
pub use tree::{load_session, SessionTree};
pub use types::{EntryKind, SessionEntry, SessionHeader};
pub use watch::{watch, SessionsWatcher};
