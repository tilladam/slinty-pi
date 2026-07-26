//! Live filesystem watch over the sessions root, so a long-running caller
//! (the sidebar) notices session files created or changed by *other*
//! processes — most importantly pi's own TUI running against the same
//! `~/.pi/agent/sessions` tree. Debounced ~300ms so a burst of writes during
//! active streaming collapses into a single "something changed" signal.
//!
//! This only ever tells the caller *that* something changed, never *what* —
//! callers already have cheap, cached re-scan via [`crate::MetaCache`], so
//! there's no need to thread per-file event data through here.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode, Debouncer};

const DEBOUNCE: Duration = Duration::from_millis(300);

/// A running watch over the sessions root. Dropping it stops watching.
pub struct SessionsWatcher {
    _debouncer: Debouncer<notify_debouncer_mini::notify::RecommendedWatcher>,
    /// Receives `()` after each debounced batch of filesystem changes.
    pub changed: mpsc::Receiver<()>,
}

/// Start watching `root` (recursively) for session file changes.
///
/// Returns `None` if the watcher can't be started (missing directory,
/// exhausted OS watch handles, ...) — callers should keep working with
/// on-demand refreshes only, since this is a liveness nicety, not a
/// correctness requirement.
pub fn watch(root: &Path) -> Option<SessionsWatcher> {
    let (tx, rx) = mpsc::channel();
    let mut debouncer = new_debouncer(
        DEBOUNCE,
        move |res: notify_debouncer_mini::DebounceEventResult| {
            if res.is_ok() {
                let _ = tx.send(());
            }
        },
    )
    .ok()?;
    debouncer
        .watcher()
        .watch(root, RecursiveMode::Recursive)
        .ok()?;
    Some(SessionsWatcher {
        _debouncer: debouncer,
        changed: rx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration as StdDuration;

    #[test]
    fn signals_on_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = watch(dir.path()).expect("watcher should start on a real directory");

        fs::write(dir.path().join("new-session.jsonl"), "{}\n").unwrap();

        watcher
            .changed
            .recv_timeout(StdDuration::from_secs(5))
            .expect("expected a change notification after writing a file");
    }

    #[test]
    fn missing_root_returns_none() {
        assert!(watch(Path::new("/definitely/does/not/exist")).is_none());
    }
}
