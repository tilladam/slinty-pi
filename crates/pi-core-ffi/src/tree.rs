//! Session branch tree + fork-from (SW10). Ports (not shares — this crate
//! doesn't depend on `pi-core`, matching its established posture)
//! `pi_core::backend`'s `fetch_tree_rows`/`flatten_tree`/`tree_node_summary`.
//!
//! Deliberately *not* built on `pi_sessions::tree::SessionTree`: that type
//! reads one session's JSONL file straight off disk and its own doc comment
//! warns its `leaf_id` is a best-effort heuristic that "can be wrong for a
//! session closed right after `/tree`-jumping or forking to an earlier
//! point with nothing sent afterward" — exactly the live-session scenario
//! this overlay is opened against. It also computes none of `depth`/
//! `summary`/`label`/`can_fork` — reusing it would mean re-deriving this
//! module's logic anyway, on top of a real active-branch mis-highlighting
//! risk. `client.get_tree()`'s live `leafId` has no such staleness problem.

use std::collections::{HashMap, HashSet};

use pi_render::{first_line, user_content_text};
use pi_rpc::PiClient;

/// One flattened row of the branch tree, in depth-first display order.
#[derive(Clone, uniffi::Record)]
pub struct TreeRowRecord {
    pub id: String,
    pub depth: i32,
    pub summary: String,
    pub label: String,
    pub can_fork: bool,
    pub is_active: bool,
}

struct FlatTreeRow {
    id: String,
    depth: i32,
    summary: String,
    label: String,
    can_fork: bool,
}

/// Fetches and flattens the session's branch tree, marking every row on the
/// path from the live `leafId` back to the root as active. Ports
/// `pi_core::backend::fetch_tree_rows`. Empty on any request failure —
/// tolerant, matches this crate's posture throughout.
pub async fn fetch_tree_rows(client: &PiClient) -> Vec<TreeRowRecord> {
    let Ok(data) = client.get_tree().await else {
        return Vec::new();
    };
    let leaf_id = data
        .get("leafId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let nodes = data
        .get("tree")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut flat = Vec::new();
    let mut parents: HashMap<String, String> = HashMap::new();
    flatten_tree(&nodes, 0, &mut flat, &mut parents);

    let mut active = HashSet::new();
    let mut current = leaf_id;
    while let Some(id) = current {
        current = parents.get(&id).cloned();
        active.insert(id);
    }

    flat.into_iter()
        .map(|r| {
            let is_active = active.contains(&r.id);
            TreeRowRecord {
                id: r.id,
                depth: r.depth,
                summary: r.summary,
                label: r.label,
                can_fork: r.can_fork,
                is_active,
            }
        })
        .collect()
}

fn flatten_tree(
    nodes: &[serde_json::Value],
    depth: i32,
    out: &mut Vec<FlatTreeRow>,
    parents: &mut HashMap<String, String>,
) {
    for node in nodes {
        let Some(entry) = node.get("entry") else {
            continue;
        };
        let Some(id) = entry.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(parent_id) = entry.get("parentId").and_then(|v| v.as_str()) {
            parents.insert(id.to_string(), parent_id.to_string());
        }
        let can_fork = entry.get("type").and_then(|v| v.as_str()) == Some("message")
            && entry.pointer("/message/role").and_then(|v| v.as_str()) == Some("user");
        let label = node
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(FlatTreeRow {
            id: id.to_string(),
            depth,
            summary: tree_node_summary(entry),
            label,
            can_fork,
        });
        if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
            flatten_tree(children, depth + 1, out, parents);
        }
    }
}

/// One-line human summary of a session-tree entry (any of the types in
/// docs/session-format.md), for the tree overlay row text. Ports
/// `pi_core::backend::tree_node_summary` verbatim.
fn tree_node_summary(entry: &serde_json::Value) -> String {
    let kind = entry.get("type").and_then(|v| v.as_str()).unwrap_or("?");
    match kind {
        "message" => {
            let message = entry.get("message").unwrap_or(&serde_json::Value::Null);
            match message.get("role").and_then(|v| v.as_str()) {
                Some("user") => {
                    let (text, _) = user_content_text(
                        message.get("content").unwrap_or(&serde_json::Value::Null),
                    );
                    elide_oneline(&text)
                }
                Some("assistant") => {
                    let text =
                        message
                            .get("content")
                            .and_then(|v| v.as_array())
                            .and_then(|blocks| {
                                blocks.iter().find_map(|b| {
                                    (b.get("type").and_then(|v| v.as_str()) == Some("text"))
                                        .then(|| b.get("text").and_then(|v| v.as_str()))
                                        .flatten()
                                })
                            });
                    match text {
                        Some(t) if !t.is_empty() => format!("assistant: {}", elide_oneline(t)),
                        _ => "assistant".to_string(),
                    }
                }
                Some("toolResult") => {
                    let name = message
                        .get("toolName")
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool");
                    format!("→ {name}")
                }
                Some("bashExecution") => {
                    format!(
                        "$ {}",
                        first_line(
                            message
                                .get("command")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                        )
                    )
                }
                Some(role) => role.to_string(),
                None => "message".to_string(),
            }
        }
        "model_change" => format!(
            "model → {}/{}",
            entry
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("?"),
            entry.get("modelId").and_then(|v| v.as_str()).unwrap_or("?"),
        ),
        "thinking_level_change" => format!(
            "thinking → {}",
            entry
                .get("thinkingLevel")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
        ),
        "compaction" => "context compacted".to_string(),
        "branch_summary" => "branch summary".to_string(),
        "custom" => match entry.get("customType").and_then(|v| v.as_str()) {
            Some(t) => format!("custom: {t}"),
            None => "custom".to_string(),
        },
        "custom_message" => "custom message".to_string(),
        "session_info" => match entry.get("name").and_then(|v| v.as_str()) {
            Some(name) => format!("renamed: {name}"),
            None => "session info".to_string(),
        },
        other => other.to_string(),
    }
}

fn elide_oneline(s: &str) -> String {
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= 70 {
        return collapsed;
    }
    let truncated: String = collapsed.chars().take(70).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(entry: serde_json::Value, children: Vec<serde_json::Value>) -> serde_json::Value {
        json!({"entry": entry, "children": children})
    }

    #[test]
    fn user_message_summary_is_elided_content_text() {
        let entry = json!({
            "id": "1", "type": "message",
            "message": {"role": "user", "content": "hello there"}
        });
        assert_eq!(tree_node_summary(&entry), "hello there");
    }

    #[test]
    fn assistant_message_summary_has_a_prefix() {
        let entry = json!({
            "id": "2", "type": "message",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "hi!"}]}
        });
        assert_eq!(tree_node_summary(&entry), "assistant: hi!");
    }

    #[test]
    fn empty_assistant_text_falls_back_to_bare_label() {
        let entry = json!({
            "id": "3", "type": "message",
            "message": {"role": "assistant", "content": []}
        });
        assert_eq!(tree_node_summary(&entry), "assistant");
    }

    #[test]
    fn tool_result_summary_shows_the_tool_name() {
        let entry = json!({
            "id": "4", "type": "message",
            "message": {"role": "toolResult", "toolName": "read"}
        });
        assert_eq!(tree_node_summary(&entry), "→ read");
    }

    #[test]
    fn bash_execution_summary_shows_the_first_command_line() {
        let entry = json!({
            "id": "5", "type": "message",
            "message": {"role": "bashExecution", "command": "ls -la\necho done"}
        });
        assert_eq!(tree_node_summary(&entry), "$ ls -la");
    }

    #[test]
    fn model_change_summary_shows_provider_and_model() {
        let entry =
            json!({"id": "6", "type": "model_change", "provider": "openai", "modelId": "gpt-5"});
        assert_eq!(tree_node_summary(&entry), "model → openai/gpt-5");
    }

    #[test]
    fn long_summary_is_elided_at_seventy_chars() {
        let long = "word ".repeat(30);
        let entry =
            json!({"id": "7", "type": "message", "message": {"role": "user", "content": long}});
        let summary = tree_node_summary(&entry);
        assert!(summary.ends_with('…'));
        assert!(summary.chars().count() <= 71);
    }

    #[test]
    fn flatten_tree_computes_depth_and_marks_forkable_user_messages() {
        let tree = vec![node(
            json!({"id": "root", "type": "message", "message": {"role": "user", "content": "hi"}}),
            vec![node(
                json!({"id": "child", "parentId": "root", "type": "message", "message": {"role": "assistant", "content": [{"type": "text", "text": "hello"}]}}),
                vec![],
            )],
        )];
        let mut flat = Vec::new();
        let mut parents = HashMap::new();
        flatten_tree(&tree, 0, &mut flat, &mut parents);
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].depth, 0);
        assert!(flat[0].can_fork);
        assert_eq!(flat[1].depth, 1);
        assert!(!flat[1].can_fork);
        assert_eq!(parents.get("child"), Some(&"root".to_string()));
    }
}
