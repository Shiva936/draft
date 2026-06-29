//! A minimal advisory file lock for serializing local writers.
//!
//! Used by the operation-log append protocol and the service lock manager. This
//! is intentionally simple (create-new lock file + stale takeover) rather than
//! an OS advisory lock, so it behaves consistently across platforms and is easy
//! to reason about for a local, single-user tool.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::{DraftError, DraftErrorKind, DraftResult};

/// How long a lock file may exist before it is considered stale and taken over.
const STALE_AFTER: Duration = Duration::from_secs(30);

/// An acquired lock; releases (best-effort) on drop.
#[derive(Debug)]
pub struct FileGuard {
    path: PathBuf,
}

impl FileGuard {
    /// Acquire the lock at `path`, waiting up to `timeout` for it to be free.
    pub fn acquire(path: &Path, timeout: Duration) -> DraftResult<FileGuard> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let start = Instant::now();
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(mut f) => {
                    let _ = writeln!(f, "{}", std::process::id());
                    return Ok(FileGuard {
                        path: path.to_path_buf(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Take over a stale lock left by a crashed process.
                    if let Ok(meta) = fs::metadata(path) {
                        if let Ok(modified) = meta.modified() {
                            if modified.elapsed().map(|d| d > STALE_AFTER).unwrap_or(false) {
                                let _ = fs::remove_file(path);
                                continue;
                            }
                        }
                    }
                    if start.elapsed() >= timeout {
                        return Err(DraftError::new(
                            DraftErrorKind::LockTimeout,
                            format!("timed out acquiring lock {}", path.display()),
                        )
                        .with_suggestion("Another Draft operation may be in progress."));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => return Err(DraftError::from(e)),
            }
        }
    }
}

impl Drop for FileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
