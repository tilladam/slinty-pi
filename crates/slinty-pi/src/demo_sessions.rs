//! Synthesizes a fake session directory so the sidebar and hydration are
//! demoable (`SLINTY_DEMO=1`) without a live pi process.
//!
//! Reuses `pi-sessions`' own test fixtures rather than hand-writing new
//! session content: they're already guaranteed to match the on-disk format
//! `pi_sessions::list_sessions` and `load_session` expect (that's what they
//! exist to test), so there's nothing here that can drift out of sync with
//! the real format.

use std::path::{Path, PathBuf};

const FIXTURE_BASIC: &str = include_str!("../../pi-sessions/tests/fixtures/basic.jsonl");
const FIXTURE_BRANCHING: &str = include_str!("../../pi-sessions/tests/fixtures/branching.jsonl");

pub struct DemoProject {
    pub sessions_root: PathBuf,
    pub cwd: PathBuf,
}

/// Writes the demo session files fresh into a scratch directory under the
/// OS temp dir, laid out the same way pi encodes a project's session
/// directory on disk. Safe to call every launch — it just overwrites.
pub fn setup() -> DemoProject {
    let cwd = PathBuf::from("/demo/slinty-pi");
    let sessions_root = std::env::temp_dir().join("slinty-pi-demo-sessions");
    let dir = pi_sessions::project_session_dir(&sessions_root, &cwd);
    let _ = std::fs::create_dir_all(&dir);
    for (name, content) in [
        ("2026-01-01T10-00-00-000Z_00000000-0000-0000-0000-000000000001.jsonl", FIXTURE_BASIC),
        ("2026-01-02T11-30-00-000Z_00000000-0000-0000-0000-000000000002.jsonl", FIXTURE_BRANCHING),
    ] {
        let _ = std::fs::write(dir.join(name), content);
    }
    DemoProject { sessions_root, cwd }
}

/// The active branch's messages, in the shape `Transcript::hydrate` expects
/// (a plain `AgentMessage`-shaped array, not the session-file tree
/// envelope) — the same conversion the real app would get from pi's
/// `get_messages` RPC, computed locally instead since demo mode has no
/// child process to ask.
pub fn hydrate_messages(path: &Path) -> Vec<serde_json::Value> {
    let Ok(tree) = pi_sessions::load_session(path) else {
        return Vec::new();
    };
    tree.active_branch()
        .into_iter()
        .filter_map(|entry| match &entry.kind {
            pi_sessions::EntryKind::Message { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_writes_sessions_pi_sessions_can_list() {
        let demo = setup();
        let dir = pi_sessions::project_session_dir(&demo.sessions_root, &demo.cwd);
        let sessions = pi_sessions::list_sessions(&dir).expect("demo session dir should be listable");
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn hydrate_messages_extracts_only_message_entries_in_branch_order() {
        let demo = setup();
        let dir = pi_sessions::project_session_dir(&demo.sessions_root, &demo.cwd);
        let sessions = pi_sessions::list_sessions(&dir).unwrap();
        let basic = sessions
            .iter()
            .find(|s| s.path.to_string_lossy().contains("000000000001"))
            .expect("basic fixture present");
        let messages = hydrate_messages(&basic.path);
        assert!(!messages.is_empty());
        assert!(messages.iter().all(|m| m.get("role").is_some()), "every entry is a plain message, not a tree envelope");
        assert_eq!(messages[0].get("role").and_then(|v| v.as_str()), Some("user"));
    }
}
