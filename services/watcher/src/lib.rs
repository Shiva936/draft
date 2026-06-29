//! Workspace file watcher. Debounces filesystem events and ignores Draft
//! write-back paths to avoid rescan loops.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};

/// Paths whose presence anywhere in a changed path means we ignore the event.
pub const IGNORED_SEGMENTS: &[&str] = &[
    "/.draft/operations/",
    "/.draft/receipts/",
    "/.draft/locks/",
    "/.draft/events/",
    "/.draft/objects/",
];

/// Returns true if `path` should be ignored by the watcher.
pub fn should_ignore(path: &Path) -> bool {
    let s = format!("/{}/", path.to_string_lossy().replace('\\', "/"));
    IGNORED_SEGMENTS.iter().any(|seg| s.contains(seg))
}

/// A running watcher. Dropping it stops watching.
pub struct Watcher {
    _inner: RecommendedWatcher,
    rx: Receiver<PathBuf>,
}

impl Watcher {
    /// Begin watching `root` recursively. Returns a [`Watcher`] whose
    /// [`Watcher::poll_debounced`] yields a coalesced batch of changed paths.
    pub fn start(root: &Path) -> notify::Result<Watcher> {
        let (tx, rx) = channel::<PathBuf>();
        let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                for path in event.paths {
                    if !should_ignore(&path) {
                        let _ = tx.send(path);
                    }
                }
            }
        })?;
        inner.watch(root, RecursiveMode::Recursive)?;
        Ok(Watcher { _inner: inner, rx })
    }

    /// Block until at least one non-ignored change occurs, then coalesce all
    /// changes that arrive within `debounce` and return their unique paths.
    /// Returns an empty vec if nothing arrives within `max_wait`.
    pub fn poll_debounced(&self, debounce: Duration, max_wait: Duration) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        let first = match self.rx.recv_timeout(max_wait) {
            Ok(p) => p,
            Err(_) => return paths,
        };
        paths.push(first);
        let deadline = Instant::now() + debounce;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match self.rx.recv_timeout(remaining) {
                Ok(p) => paths.push(p),
                Err(_) => break,
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }
}
