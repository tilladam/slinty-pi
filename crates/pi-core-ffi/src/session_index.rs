//! Stateless session/project browsing — no live `pi` child needed (mirrors
//! `pi_core::backend::Sidebar::refresh_projects`, which also never touches a
//! `PiClient`). Kept as its own object, separate from [`crate::PiSession`],
//! so the sidebar can render before/independent of `pi` spawning.

use pi_sessions::MetaCache;

#[derive(uniffi::Record)]
pub struct ProjectRecord {
    pub display_path: String,
}

#[derive(uniffi::Record)]
pub struct SessionRecord {
    pub path: String,
    pub title: String,
    pub relative_time: String,
    pub active: bool,
    pub cost: String,
}

impl From<pi_sessions::SidebarRow> for SessionRecord {
    fn from(row: pi_sessions::SidebarRow) -> Self {
        Self {
            path: row.path,
            title: row.title,
            relative_time: row.relative_time,
            active: row.active,
            cost: row.cost,
        }
    }
}

/// `meta_cache` is the caching payoff: a plain field (no `Arc<Mutex<>>`
/// needed) since `MetaCache` is already internally `Mutex`-guarded and
/// `Sync` — concurrent Swift calls share the same warm cache. No
/// `tokio::runtime::Runtime` field either, unlike `PiSession`: UniFFI's
/// `async_runtime = "tokio"` exports drive their own future (via
/// `async_compat`) without needing an ambient runtime already running.
#[derive(uniffi::Object, Default)]
pub struct SessionIndex {
    meta_cache: MetaCache,
}

#[uniffi::export(async_runtime = "tokio")]
impl SessionIndex {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every project with sessions on disk, display-sorted. No "current
    /// project" filtering here — that's a pure Swift-side decision against
    /// whatever cwd it's already tracking.
    pub async fn list_projects(&self) -> Vec<ProjectRecord> {
        let Some(root) = pi_sessions::default_sessions_root() else {
            return Vec::new();
        };
        projects_at(&root)
    }

    /// Sessions under `cwd`, filtered by `query` (empty = no filter), marked
    /// against `active_path` (from `ChatSink::on_active_session_changed`) —
    /// including the synthesized placeholder row for an active session `pi`
    /// hasn't written to disk yet (see `pi_sessions::sidebar_rows`).
    pub async fn list_sessions(
        &self,
        cwd: String,
        query: String,
        active_path: Option<String>,
    ) -> Vec<SessionRecord> {
        let Some(root) = pi_sessions::default_sessions_root() else {
            return Vec::new();
        };
        sessions_at(
            &self.meta_cache,
            &root,
            &cwd,
            &query,
            active_path.as_deref(),
        )
    }
}

/// Split out from `list_projects` purely so tests can point it at a fixture
/// directory instead of the real `$HOME` — `default_sessions_root()` has no
/// override, and isn't worth adding one just for this.
fn projects_at(root: &std::path::Path) -> Vec<ProjectRecord> {
    pi_sessions::list_projects(root)
        .unwrap_or_default()
        .into_iter()
        .map(|p| ProjectRecord {
            display_path: p.display_path,
        })
        .collect()
}

/// Split out from `list_sessions` for the same reason as `projects_at`.
fn sessions_at(
    meta_cache: &MetaCache,
    root: &std::path::Path,
    cwd: &str,
    query: &str,
    active: Option<&str>,
) -> Vec<SessionRecord> {
    let session_dir = pi_sessions::project_session_dir(root, std::path::Path::new(cwd));
    pi_sessions::sidebar_rows(meta_cache, &session_dir, query, active)
        .into_iter()
        .map(SessionRecord::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn pi_sessions_fixtures_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("pi-sessions")
            .join("tests")
            .join("fixtures")
    }

    /// Copies pi-sessions' fixture session files into a real
    /// `<root>/--<encoded cwd>--/` layout under a temp dir, the same shape
    /// `project_session_dir` expects on real disk — so `sessions_at`'s own
    /// `project_session_dir` join gets exercised for real, not bypassed.
    fn seed_project(root: &Path, cwd: &str) {
        let session_dir = pi_sessions::project_session_dir(root, Path::new(cwd));
        std::fs::create_dir_all(&session_dir).unwrap();
        for name in ["basic.jsonl", "branching.jsonl"] {
            std::fs::copy(
                pi_sessions_fixtures_dir().join(name),
                session_dir.join(name),
            )
            .unwrap();
        }
    }

    #[test]
    fn sessions_at_lists_fixture_sessions_and_marks_active() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = "/demo/pi-core-ffi-test";
        seed_project(tmp.path(), cwd);
        let cache = MetaCache::new();

        let records = sessions_at(&cache, tmp.path(), cwd, "", None);
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r.title == "Test run walkthrough"));
        assert!(records.iter().all(|r| !r.active));

        let existing_path = records[0].path.clone();
        let with_active = sessions_at(&cache, tmp.path(), cwd, "", Some(&existing_path));
        assert_eq!(with_active.len(), 2, "no row synthesized for a real path");
        assert!(with_active
            .iter()
            .any(|r| r.path == existing_path && r.active));
    }

    #[test]
    fn projects_at_missing_root_returns_empty_not_a_panic() {
        let missing = std::path::PathBuf::from("/does/not/exist/anywhere");
        assert!(projects_at(&missing).is_empty());
    }

    #[test]
    fn projects_at_lists_a_seeded_project() {
        let tmp = tempfile::tempdir().unwrap();
        seed_project(tmp.path(), "/demo/pi-core-ffi-test");
        let projects = projects_at(tmp.path());
        assert_eq!(projects.len(), 1);
    }

    #[test]
    fn session_record_from_sidebar_row_preserves_every_field() {
        let row = pi_sessions::SidebarRow {
            path: "/a/b.jsonl".to_string(),
            title: "hello".to_string(),
            relative_time: "5m".to_string(),
            active: true,
            cost: "$0.02".to_string(),
        };
        let record = SessionRecord::from(row);
        assert_eq!(record.path, "/a/b.jsonl");
        assert_eq!(record.title, "hello");
        assert_eq!(record.relative_time, "5m");
        assert!(record.active);
        assert_eq!(record.cost, "$0.02");
    }
}
