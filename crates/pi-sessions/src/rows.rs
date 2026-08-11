//! Human-facing session rows (sidebar/session-list display), shared by every
//! frontend: the "synthesize a placeholder row for a session pi hasn't
//! written to disk yet" behavior lives here once, tested here once, instead
//! of being duplicated per UI toolkit.

use std::path::Path;

use crate::{search, MetaCache, SessionMeta};

/// One session, ready to render in a session list: `title`/`relative_time`/
/// `cost` are pre-formatted display strings, `active` marks the row matching
/// the caller's current session pointer (if any).
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarRow {
    pub path: String,
    pub title: String,
    pub relative_time: String,
    pub active: bool,
    pub cost: String,
}

/// Sessions under `session_dir`, filtered by `query` (empty = no filter),
/// marked against `active`. If `active` names a session not yet in
/// `meta_cache` (pi doesn't write a session's file until its first message),
/// synthesizes a placeholder row at index 0 so "where you are" is never
/// silently missing from a just-switched-to project's session list.
pub fn sidebar_rows(
    meta_cache: &MetaCache,
    session_dir: &Path,
    query: &str,
    active: Option<&str>,
) -> Vec<SidebarRow> {
    let all = meta_cache.list_sessions(session_dir).unwrap_or_default();
    let filtered: Vec<&SessionMeta> = if query.is_empty() {
        all.iter().collect()
    } else {
        search(&all, query)
    };
    let mut rows: Vec<SidebarRow> = filtered
        .into_iter()
        .map(|m| {
            let path = m.path.to_string_lossy().into_owned();
            let is_active = active == Some(path.as_str());
            SidebarRow {
                path,
                title: m.title().to_string(),
                relative_time: relative_time(&m.last_timestamp),
                active: is_active,
                cost: format_cost(m.total_cost),
            }
        })
        .collect();

    // pi doesn't write a session's file until its first message, so a
    // just-switched-to project's brand-new (still empty) session has no
    // file for `meta_cache` to find yet — without this, the session list
    // would show every *other* session but not the one you're actually in,
    // until you send a message (which finally creates the file) or switch
    // away and back (which happens to land on a now-existing file).
    // Synthesize its row from the active path the caller already knows, so
    // "where you are" is never silently missing.
    if let Some(active_path) = active {
        if query.is_empty() && !rows.iter().any(|r| r.path == active_path) {
            rows.insert(
                0,
                SidebarRow {
                    path: active_path.to_string(),
                    title: "New session".to_string(),
                    relative_time: "just now".to_string(),
                    active: true,
                    cost: String::new(),
                },
            );
        }
    }

    rows
}

/// Sidebar cost label, e.g. "$0.0231". `""` (rendered as no label at all)
/// below half a cent — mainly for local models, where cost is always 0.
pub fn format_cost(total_cost: f64) -> String {
    if total_cost < 0.005 {
        String::new()
    } else {
        format!("${total_cost:.2}")
    }
}

pub fn relative_time(iso_timestamp: &str) -> String {
    let Ok(then) = chrono::DateTime::parse_from_rfc3339(iso_timestamp) else {
        return String::new();
    };
    let secs = chrono::Utc::now()
        .signed_duration_since(then.with_timezone(&chrono::Utc))
        .num_seconds();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else if secs < 86400 * 7 {
        format!("{}d", secs / 86400)
    } else {
        format!("{}w", secs / (86400 * 7))
    }
}

#[cfg(test)]
mod format_cost_tests {
    use super::format_cost;

    #[test]
    fn zero_and_near_zero_cost_yields_empty_label() {
        assert_eq!(format_cost(0.0), "");
        assert_eq!(format_cost(0.001), "");
    }

    #[test]
    fn non_trivial_cost_is_formatted_as_dollars() {
        assert_eq!(format_cost(0.0231), "$0.02");
        assert_eq!(format_cost(1.5), "$1.50");
    }
}

#[cfg(test)]
mod relative_time_tests {
    use super::relative_time;
    use chrono::{Duration, Utc};

    fn iso(ago: Duration) -> String {
        (Utc::now() - ago).to_rfc3339()
    }

    #[test]
    fn buckets_by_magnitude() {
        assert_eq!(relative_time(&iso(Duration::seconds(10))), "just now");
        assert_eq!(relative_time(&iso(Duration::minutes(5))), "5m");
        assert_eq!(relative_time(&iso(Duration::hours(3))), "3h");
        assert_eq!(relative_time(&iso(Duration::days(2))), "2d");
        assert_eq!(relative_time(&iso(Duration::days(15))), "2w");
    }

    #[test]
    fn unparseable_timestamp_yields_empty_string() {
        assert_eq!(relative_time("not a timestamp"), "");
    }
}

#[cfg(test)]
mod sidebar_rows_tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    #[test]
    fn lists_both_fixture_sessions_with_none_active_when_active_is_unset() {
        let cache = MetaCache::new();
        let rows = sidebar_rows(&cache, &fixtures_dir(), "", None);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| !r.active));
    }

    #[test]
    fn marks_the_matching_row_active_without_synthesizing_one() {
        let cache = MetaCache::new();
        let all = cache.list_sessions(&fixtures_dir()).unwrap();
        let existing = all[0].path.to_string_lossy().into_owned();
        let rows = sidebar_rows(&cache, &fixtures_dir(), "", Some(&existing));
        assert_eq!(rows.len(), 2, "no extra row synthesized");
        let active_rows: Vec<&SidebarRow> = rows.iter().filter(|r| r.active).collect();
        assert_eq!(active_rows.len(), 1);
        assert_eq!(active_rows[0].path, existing);
    }

    #[test]
    fn synthesizes_a_row_for_an_active_session_not_yet_on_disk() {
        let cache = MetaCache::new();
        let brand_new = "/does/not/exist/on/disk.jsonl";
        let rows = sidebar_rows(&cache, &fixtures_dir(), "", Some(brand_new));
        assert_eq!(rows.len(), 3, "2 fixture sessions + 1 synthesized");
        assert_eq!(rows[0].path, brand_new);
        assert_eq!(rows[0].title, "New session");
        assert!(rows[0].active);
    }

    #[test]
    fn query_filters_before_synthesis_is_considered() {
        let cache = MetaCache::new();
        // A non-empty query suppresses synthesis even if `active` is unmatched
        // (mirrors the original Sidebar behavior: synthesis only applies to
        // the unfiltered view).
        let rows = sidebar_rows(
            &cache,
            &fixtures_dir(),
            "parser",
            Some("/does/not/exist.jsonl"),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "refactor the parser module");
    }
}
