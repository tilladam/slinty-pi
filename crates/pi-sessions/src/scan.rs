//! Cheap per-file metadata scan (for session lists) and project discovery,
//! independent of any running `pi` process — pi stays the writer, this crate
//! only ever reads.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde_json::Value;

use crate::types::{content_preview, entry_cost, entry_tokens, EntryKind, SessionEntry, SessionHeader};

/// A project is a session-storage directory keyed by pi's `--<cwd>--`
/// encoding. `dir_name` is the raw, lossless identity; `display_path` is a
/// best-effort human-readable path (read from a session header when
/// possible, else decoded from `dir_name`, which is lossy — see
/// [`decode_project_dir`]).
#[derive(Debug, Clone)]
pub struct Project {
    pub dir_name: String,
    pub display_path: String,
    pub session_dir: PathBuf,
}

/// Lightweight metadata about a session file, cheap enough to compute for
/// every session on disk (see [`MetaCache`] for a cached, repeated-call
/// friendly version).
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub path: PathBuf,
    pub id: String,
    pub cwd: String,
    pub created: String,
    pub name: Option<String>,
    pub first_user_text: Option<String>,
    pub entry_count: usize,
    pub last_timestamp: String,
    pub total_cost: f64,
    pub total_tokens: u64,
    pub parent_session: Option<String>,
}

impl SessionMeta {
    /// Display title: explicit `/name`, else the first user message
    /// (elided), else the session id.
    pub fn title(&self) -> &str {
        self.name
            .as_deref()
            .or(self.first_user_text.as_deref())
            .unwrap_or(&self.id)
    }
}

/// `~/.pi/agent/sessions`, if `$HOME` is resolvable.
pub fn default_sessions_root() -> Option<PathBuf> {
    #[allow(deprecated)]
    std::env::home_dir().map(|h| h.join(".pi").join("agent").join("sessions"))
}

/// Encode a cwd into pi's `--<cwd-with-slashes-as-dashes>--` session
/// directory name. Exact, unlike [`decode_project_dir`]: replacing every `/`
/// with `-` loses no information going forward, only when read back.
pub fn encode_project_dir(cwd: &Path) -> String {
    format!("--{}--", cwd.to_string_lossy().trim_start_matches('/').replace('/', "-"))
}

/// The session-storage directory pi would use for `cwd`, without scanning
/// the sessions root — for jumping straight to "the current project's
/// sessions" (e.g. the sidebar) rather than enumerating every project.
pub fn project_session_dir(sessions_root: &Path, cwd: &Path) -> PathBuf {
    sessions_root.join(encode_project_dir(cwd))
}

/// Decode a `--<cwd-with-slashes-as-dashes>--` directory name back into a
/// path. **Lossy**: a project directory whose real name contains a literal
/// `-` is indistinguishable from a path separator here. Never treat this as
/// authoritative identity — use `dir_name`/`session_dir` for that; this is
/// display-only, and [`list_projects`] prefers the real `cwd` recorded in a
/// session header when one is available.
pub fn decode_project_dir(dir_name: &str) -> String {
    let stripped = dir_name
        .strip_prefix("--")
        .and_then(|s| s.strip_suffix("--"))
        .unwrap_or(dir_name);
    format!("/{}", stripped.replace('-', "/"))
}

/// Scan the sessions root for project directories.
pub fn list_projects(root: &Path) -> io::Result<Vec<Project>> {
    let mut projects = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let session_dir = entry.path();
        let display_path =
            first_session_cwd(&session_dir).unwrap_or_else(|| decode_project_dir(&dir_name));
        projects.push(Project {
            dir_name,
            display_path,
            session_dir,
        });
    }
    projects.sort_by(|a, b| a.display_path.cmp(&b.display_path));
    Ok(projects)
}

fn first_session_cwd(dir: &Path) -> Option<String> {
    let mut entries: Vec<_> = fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let file = fs::File::open(&path).ok()?;
        let mut line = String::new();
        if BufReader::new(file).read_line(&mut line).ok()? == 0 {
            continue;
        }
        if let Ok(header) = serde_json::from_str::<SessionHeader>(line.trim()) {
            return Some(header.cwd);
        }
    }
    None
}

/// Parse one session file into [`SessionMeta`]. Reads every line (tolerating
/// malformed trailing lines) but keeps only lightweight per-entry fields, so
/// this stays cheap even for large sessions.
pub fn parse_meta(path: &Path) -> io::Result<SessionMeta> {
    let file = fs::File::open(path)?;
    let mut lines = BufReader::new(file).lines();
    let header_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty session file"))??;
    let header: SessionHeader = serde_json::from_str(&header_line)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut meta = SessionMeta {
        path: path.to_path_buf(),
        id: header.id,
        cwd: header.cwd,
        created: header.timestamp.clone(),
        name: None,
        first_user_text: None,
        entry_count: 0,
        last_timestamp: header.timestamp,
        total_cost: 0.0,
        total_tokens: 0,
        parent_session: header.parent_session,
    };

    for line in lines {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let entry: SessionEntry = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        meta.entry_count += 1;
        meta.last_timestamp = entry.timestamp.clone();
        meta.total_cost += entry_cost(&entry.kind);
        meta.total_tokens += entry_tokens(&entry.kind);
        match &entry.kind {
            EntryKind::SessionInfo { name } => meta.name = Some(name.clone()),
            EntryKind::Message { message }
                if meta.first_user_text.is_none()
                    && message.get("role").and_then(Value::as_str) == Some("user") =>
            {
                let preview = content_preview(message.get("content").unwrap_or(&Value::Null));
                if !preview.is_empty() {
                    meta.first_user_text = Some(elide(&preview, 80));
                }
            }
            _ => {}
        }
    }
    Ok(meta)
}

fn elide(s: &str, max_chars: usize) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let truncated: String = collapsed.chars().take(max_chars).collect();
    format!("{truncated}…")
}

/// All sessions in a project's storage directory, newest activity first.
/// Malformed files are skipped.
pub fn list_sessions(session_dir: &Path) -> io::Result<Vec<SessionMeta>> {
    let mut metas = Vec::new();
    for entry in fs::read_dir(session_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        if let Ok(meta) = parse_meta(&path) {
            metas.push(meta);
        }
    }
    metas.sort_by(|a, b| b.last_timestamp.cmp(&a.last_timestamp));
    Ok(metas)
}

/// Brute-force substring search over title + cwd. Corpora are small (a
/// handful to a few hundred sessions), so no index is needed.
pub fn search<'a>(sessions: &'a [SessionMeta], query: &str) -> Vec<&'a SessionMeta> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return sessions.iter().collect();
    }
    sessions
        .iter()
        .filter(|m| {
            m.title().to_lowercase().contains(&query) || m.cwd.to_lowercase().contains(&query)
        })
        .collect()
}

struct CacheEntry {
    mtime: SystemTime,
    size: u64,
    meta: SessionMeta,
}

/// Caches [`parse_meta`] results keyed on `(mtime, size)`, so repeated sidebar
/// refreshes only re-parse files that actually changed.
#[derive(Default)]
pub struct MetaCache {
    entries: Mutex<HashMap<PathBuf, CacheEntry>>,
}

impl MetaCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_parse(&self, path: &Path) -> io::Result<SessionMeta> {
        let fs_meta = fs::metadata(path)?;
        let mtime = fs_meta.modified()?;
        let size = fs_meta.len();
        if let Some(cached) = self.entries.lock().unwrap().get(path) {
            if cached.mtime == mtime && cached.size == size {
                return Ok(cached.meta.clone());
            }
        }
        let meta = parse_meta(path)?;
        self.entries.lock().unwrap().insert(
            path.to_path_buf(),
            CacheEntry {
                mtime,
                size,
                meta: meta.clone(),
            },
        );
        Ok(meta)
    }

    /// Cache-backed equivalent of [`list_sessions`].
    pub fn list_sessions(&self, session_dir: &Path) -> io::Result<Vec<SessionMeta>> {
        let mut metas = Vec::new();
        for entry in fs::read_dir(session_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            if let Ok(meta) = self.get_or_parse(&path) {
                metas.push(meta);
            }
        }
        metas.sort_by(|a, b| b.last_timestamp.cmp(&a.last_timestamp));
        Ok(metas)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    fn fixture(name: &str) -> PathBuf {
        fixtures_dir().join(name)
    }

    #[test]
    fn decode_project_dir_round_trips_simple_paths() {
        assert_eq!(
            decode_project_dir("--Users-dev-example--"),
            "/Users/dev/example"
        );
    }

    #[test]
    fn encode_project_dir_matches_pi_on_disk() {
        // The real directory name for this repo, observed on disk.
        assert_eq!(
            encode_project_dir(Path::new("/Users/till/Code/Rust/slint/slinty-pi")),
            "--Users-till-Code-Rust-slint-slinty-pi--"
        );
    }

    #[test]
    fn encode_then_decode_round_trips_when_no_literal_dashes() {
        let cwd = Path::new("/Users/dev/example");
        let encoded = encode_project_dir(cwd);
        assert_eq!(decode_project_dir(&encoded), cwd.to_string_lossy());
    }

    #[test]
    fn parses_basic_fixture_meta() {
        let meta = parse_meta(&fixture("basic.jsonl")).unwrap();
        assert_eq!(meta.id, "019f98a4-83d5-7e2e-9c80-9e9ec0700133");
        assert_eq!(meta.cwd, "/Users/dev/example-project");
        assert_eq!(meta.name.as_deref(), Some("Test run walkthrough"));
        assert_eq!(meta.first_user_text.as_deref(), Some("hello, which model are you?"));
        assert_eq!(meta.entry_count, 9); // all lines after the header
        assert!(meta.total_cost > 0.0);
        assert!(meta.total_tokens > 0);
        assert_eq!(meta.title(), "Test run walkthrough");
    }

    #[test]
    fn falls_back_to_first_user_text_without_session_info() {
        let meta = parse_meta(&fixture("branching.jsonl")).unwrap();
        assert!(meta.name.is_none());
        assert_eq!(meta.title(), "refactor the parser module");
    }

    #[test]
    fn handles_string_and_array_message_content() {
        // branching.jsonl's last user message uses a bare string `content`.
        let meta = parse_meta(&fixture("branching.jsonl")).unwrap();
        assert_eq!(meta.entry_count, 9);
    }

    #[test]
    fn list_sessions_sorts_newest_first() {
        let dir = fixtures_dir();
        let sessions = list_sessions(&dir).unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions[0].last_timestamp >= sessions[1].last_timestamp);
    }

    #[test]
    fn search_matches_title_case_insensitively() {
        let dir = fixtures_dir();
        let sessions = list_sessions(&dir).unwrap();
        let hits = search(&sessions, "PARSER");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title(), "refactor the parser module");
    }

    #[test]
    fn meta_cache_reuses_unchanged_files() {
        let cache = MetaCache::new();
        let path = fixture("basic.jsonl");
        let a = cache.get_or_parse(&path).unwrap();
        let b = cache.get_or_parse(&path).unwrap();
        assert_eq!(a.id, b.id);
    }
}
