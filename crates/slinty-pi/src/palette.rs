//! Command palette entry model and fuzzy ranking.
//!
//! Entries are a tagged union over three sources: static app actions, the
//! current project's sessions, and pi's `get_commands` slash commands. The
//! `id` prefix (`action:` / `session:` / `command:`) is how the palette's
//! `exec` dispatch (in `main.rs`) tells them apart — see `PaletteRow` in
//! `ui/palette.slint` for the Slint-side mirror of this shape.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

#[derive(Clone, Debug, PartialEq)]
pub struct PaletteEntry {
    pub id: String,
    pub kind: &'static str,
    pub label: String,
    pub detail: String,
}

const ACTIONS: &[(&str, &str, &str)] = &[
    (
        "action:new-session",
        "New session",
        "start fresh in this project",
    ),
    (
        "action:open-tree",
        "Open session tree",
        "browse and fork from any point",
    ),
    (
        "action:open-models",
        "Models panel",
        "browse and serve local rapid-mlx models",
    ),
    (
        "action:clone-session",
        "Clone session",
        "duplicate this session at the current point",
    ),
    (
        "action:cycle-density",
        "Cycle density",
        "Verbose / Normal / Summary",
    ),
    ("action:toggle-sidebar", "Toggle sidebar", ""),
    ("action:abort", "Abort", "stop the current turn"),
];

/// Build the full unranked entry list from the current project's sessions
/// (already-scanned metadata) and pi's `get_commands` response data.
pub fn build_entries(
    sessions: &[pi_sessions::SessionMeta],
    commands: &[serde_json::Value],
) -> Vec<PaletteEntry> {
    let mut entries: Vec<PaletteEntry> = ACTIONS
        .iter()
        .map(|(id, label, detail)| PaletteEntry {
            id: id.to_string(),
            kind: "action",
            label: label.to_string(),
            detail: detail.to_string(),
        })
        .collect();

    entries.extend(sessions.iter().map(|m| PaletteEntry {
        id: format!("session:{}", m.path.display()),
        kind: "session",
        label: m.title().to_string(),
        detail: m.last_timestamp.clone(),
    }));

    entries.extend(commands.iter().filter_map(|c| {
        let name = c.get("name").and_then(|v| v.as_str())?;
        let detail = c.get("description").and_then(|v| v.as_str()).unwrap_or("");
        Some(PaletteEntry {
            id: format!("command:{name}"),
            kind: "command",
            label: format!("/{name}"),
            detail: detail.to_string(),
        })
    }));

    entries
}

struct Candidate {
    idx: usize,
    haystack: String,
}

impl AsRef<str> for Candidate {
    fn as_ref(&self) -> &str {
        &self.haystack
    }
}

/// Fuzzy-rank `entries` against `query`, most relevant first, capped to a
/// reasonable list size. An empty query returns entries in their built
/// (action, then session, then command) order.
pub fn rank(entries: &[PaletteEntry], query: &str) -> Vec<PaletteEntry> {
    const LIMIT: usize = 60;
    if query.trim().is_empty() {
        return entries.iter().take(LIMIT).cloned().collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let candidates = entries.iter().enumerate().map(|(idx, e)| Candidate {
        idx,
        haystack: format!("{} {}", e.label, e.detail),
    });
    pattern
        .match_list(candidates, &mut matcher)
        .into_iter()
        .take(LIMIT)
        .map(|(c, _score)| entries[c.idx].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, kind: &'static str, label: &str) -> PaletteEntry {
        PaletteEntry {
            id: id.to_string(),
            kind,
            label: label.to_string(),
            detail: String::new(),
        }
    }

    #[test]
    fn empty_query_returns_build_order() {
        let entries = vec![
            entry("a", "action", "New session"),
            entry("b", "command", "/compact"),
        ];
        assert_eq!(rank(&entries, ""), entries);
    }

    #[test]
    fn fuzzy_query_matches_subsequence_and_ranks_closer_matches_first() {
        let entries = vec![
            entry("action:new-session", "action", "New session"),
            entry("command:compact", "command", "/compact"),
            entry("session:x", "session", "unrelated title"),
        ];
        let ranked = rank(&entries, "nsess");
        assert_eq!(ranked[0].id, "action:new-session");
        assert!(ranked.iter().all(|e| e.id != "session:x"));
    }

    #[test]
    fn non_matching_query_drops_entries() {
        let entries = vec![entry("a", "action", "New session")];
        assert!(rank(&entries, "zzzzz").is_empty());
    }

    #[test]
    fn build_entries_includes_all_three_sources_with_correct_id_prefixes() {
        let sessions = vec![pi_sessions::SessionMeta {
            path: "/x/y.jsonl".into(),
            id: "id1".into(),
            cwd: "/x".into(),
            created: "2026-01-01T00:00:00Z".into(),
            name: Some("my session".into()),
            first_user_text: None,
            entry_count: 1,
            last_timestamp: "2026-01-01T00:00:00Z".into(),
            total_cost: 0.0,
            total_tokens: 0,
            parent_session: None,
        }];
        let commands =
            vec![serde_json::json!({"name": "compact", "description": "compact context"})];
        let entries = build_entries(&sessions, &commands);

        assert!(entries
            .iter()
            .any(|e| e.kind == "action" && e.id.starts_with("action:")));
        assert!(entries
            .iter()
            .any(|e| e.kind == "session" && e.id == "session:/x/y.jsonl"));
        assert!(entries
            .iter()
            .any(|e| e.kind == "command" && e.id == "command:compact" && e.label == "/compact"));
    }
}
