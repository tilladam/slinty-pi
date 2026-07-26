//! Full parse of a session file into an id-indexed tree, for transcript
//! hydration and the (read-only, v1) tree/fork view.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use crate::types::{SessionEntry, SessionHeader};

pub struct SessionTree {
    pub header: SessionHeader,
    /// Entries in file (append) order.
    pub entries: Vec<SessionEntry>,
    by_id: HashMap<String, usize>,
    /// Best-effort leaf: the last entry appended to the file. This holds
    /// whenever the branch you're on gets a new message after you switch to
    /// it (the common case — appending is what makes a branch visible in the
    /// file at all). It can be wrong for a session closed right after
    /// `/tree`-jumping or forking to an earlier point with nothing sent
    /// afterward, since that only moves pi's in-memory leaf pointer without
    /// appending anything. A *running* session's true leaf is authoritative
    /// via RPC (`get_tree`/`get_entries` both return `leafId`) — prefer that
    /// when a session is live; this is the fallback for reading closed files.
    pub leaf_id: Option<String>,
}

impl SessionTree {
    pub fn get(&self, id: &str) -> Option<&SessionEntry> {
        self.by_id.get(id).map(|&i| &self.entries[i])
    }

    pub fn children_of<'a>(
        &'a self,
        id: Option<&'a str>,
    ) -> impl Iterator<Item = &'a SessionEntry> {
        self.entries
            .iter()
            .filter(move |e| e.parent_id.as_deref() == id)
    }

    /// Entries from the root to the current leaf, in root-to-leaf order —
    /// the active branch a resumed transcript should hydrate from.
    pub fn active_branch(&self) -> Vec<&SessionEntry> {
        let mut path = Vec::new();
        let mut current = self.leaf_id.as_deref();
        while let Some(id) = current {
            let Some(entry) = self.get(id) else { break };
            path.push(entry);
            current = entry.parent_id.as_deref();
        }
        path.reverse();
        path
    }
}

pub fn load_session(path: &Path) -> io::Result<SessionTree> {
    let file = fs::File::open(path)?;
    let mut lines = BufReader::new(file).lines();
    let header_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty session file"))??;
    let header: SessionHeader = serde_json::from_str(&header_line)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut entries = Vec::new();
    for line in lines {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<SessionEntry>(&line) {
            entries.push(entry);
        }
    }
    let leaf_id = entries.last().map(|e| e.id.clone());
    let by_id = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.clone(), i))
        .collect();
    Ok(SessionTree {
        header,
        entries,
        by_id,
        leaf_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn loads_full_tree() {
        let tree = load_session(&fixture("basic.jsonl")).unwrap();
        assert_eq!(tree.header.version, 3);
        assert_eq!(tree.entries.len(), 9);
        assert_eq!(tree.leaf_id.as_deref(), Some("a1b2c3d4"));
        assert_eq!(tree.active_branch().len(), 9);
    }

    #[test]
    fn finds_branch_point_children() {
        let tree = load_session(&fixture("branching.jsonl")).unwrap();
        // e1000001 has two children on-disk: e1000002 (the original
        // continuation) and e1000004 (the branch_summary from switching away).
        let children: Vec<_> = tree
            .children_of(Some("e1000001"))
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(children, vec!["e1000002", "e1000004"]);
    }

    #[test]
    fn active_branch_follows_the_last_written_leaf() {
        let tree = load_session(&fixture("branching.jsonl")).unwrap();
        let branch: Vec<_> = tree.active_branch().iter().map(|e| e.id.clone()).collect();
        // The branch summary path, not the original e1000002/e1000003 bash detour.
        assert!(branch.contains(&"e1000004".to_string()));
        assert!(!branch.contains(&"e1000002".to_string()));
        assert_eq!(branch.last().unwrap(), "e1000009");
    }
}
