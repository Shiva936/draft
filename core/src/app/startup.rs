//! The recovery barrier, run before a project is used.
//!
//! Every restart table in Draft assumes somebody actually consults it. Until
//! now nothing did: `ActivityLog::recover` existed, was tested, and was called
//! from no path a running Draft ever took. A torn tail from a crash would sit
//! there until the next append tripped over it, which is the worst possible
//! moment to discover it — mid-mutation, with a caller waiting.
//!
//! So the barrier runs when a project is opened, before anything reads or
//! writes it.
//!
//! # Why it runs once per process, not once per open
//!
//! `open` is called constantly — every command, every request. Recovery takes
//! the Activity lock and scans the log, which is cheap once and wasteful on
//! every call.
//!
//! Running it once per process per project is sound because recovery is
//! idempotent and because a *second* process that damages the log holds the
//! same lock this one takes: it cannot interleave with an append. What this
//! memo cannot see is damage arriving from outside Draft entirely, which no
//! amount of re-checking would catch either — a check on every open would just
//! narrow the window while costing every caller.
//!
//! # Why the doctor path skips it
//!
//! Hard corruption makes the barrier refuse, and refusing at `open` would make
//! the project unopenable — including by the tool meant to diagnose it. A
//! recovery-mode open therefore bypasses the barrier, exactly as it already
//! bypasses the retired-profile checks, so `draft doctor` can still reach a
//! project the barrier will not clear.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::activity::ActivityLog;
use crate::project::layout::DraftLayout;
use crate::support::error::DraftResult;

/// Projects this process has already recovered.
///
// ponytail: one process-wide set, guarded by a Mutex. Contention is a lock
// acquisition per project open and the set never exceeds the number of
// projects one process touches; a per-project OnceLock would only matter if
// that stopped being true.
static RECOVERED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

/// What the barrier did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// This process had already recovered this project.
    AlreadyRecovered,
    /// Nothing needed repair.
    Clean { records: usize },
    /// An incomplete final frame was truncated and the index rebuilt.
    ///
    /// Reported rather than silent: a crash discarded work somebody may have
    /// believed was recorded, and that is worth being able to see afterwards.
    TruncatedTornTail { records: usize, discarded: u64 },
}

/// Run the barrier for `paths`, at most once per process.
///
/// Returns an error only for damage recovery must not repair on its own —
/// hard corruption, where truncating would destroy a committed record to make
/// the log parse.
pub fn ensure_recovered(paths: &DraftLayout, ledger_id: &str) -> DraftResult<RecoveryOutcome> {
    let root = paths.draft_dir.clone();
    if already_recovered(&root) {
        return Ok(RecoveryOutcome::AlreadyRecovered);
    }

    let log = ActivityLog::new(paths.events_dir(), ledger_id);
    let report = log.recover()?;

    // Marked only after recovery succeeds. A failed barrier must be re-run by
    // the next caller rather than remembered as done.
    mark_recovered(root);

    Ok(if report.truncated_bytes == 0 {
        RecoveryOutcome::Clean {
            records: report.records,
        }
    } else {
        RecoveryOutcome::TruncatedTornTail {
            records: report.records,
            discarded: report.truncated_bytes,
        }
    })
}

fn already_recovered(root: &Path) -> bool {
    RECOVERED
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|seen| seen.contains(root)))
        .unwrap_or(false)
}

fn mark_recovered(root: PathBuf) {
    if let Ok(mut guard) = RECOVERED.lock() {
        guard.get_or_insert_with(HashSet::new).insert(root);
    }
}

/// Forget what this process has recovered.
///
/// For tests that need the barrier to run again over the same directory.
#[doc(hidden)]
pub fn forget_recovered_for_tests() {
    if let Ok(mut guard) = RECOVERED.lock() {
        guard.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::error::DraftErrorKind;

    fn layout(directory: &tempfile::TempDir) -> DraftLayout {
        DraftLayout::for_root(directory.path())
    }

    fn log_for(paths: &DraftLayout) -> ActivityLog {
        ActivityLog::new(paths.events_dir(), "prj_000000000001")
    }

    #[test]
    fn a_clean_project_passes_the_barrier() {
        forget_recovered_for_tests();
        let directory = tempfile::tempdir().unwrap();
        let paths = layout(&directory);
        log_for(&paths)
            .append("evt_000000000001", &serde_json::json!({"kind": "test"}))
            .unwrap();

        assert_eq!(
            ensure_recovered(&paths, "prj_000000000001").unwrap(),
            RecoveryOutcome::Clean { records: 1 }
        );
    }

    #[test]
    fn the_barrier_runs_once_per_process_for_one_project() {
        forget_recovered_for_tests();
        let directory = tempfile::tempdir().unwrap();
        let paths = layout(&directory);
        log_for(&paths)
            .append("evt_000000000001", &serde_json::json!({"kind": "test"}))
            .unwrap();

        assert!(matches!(
            ensure_recovered(&paths, "prj_000000000001").unwrap(),
            RecoveryOutcome::Clean { .. }
        ));
        assert_eq!(
            ensure_recovered(&paths, "prj_000000000001").unwrap(),
            RecoveryOutcome::AlreadyRecovered,
            "open is called constantly; rescanning the log each time would be waste"
        );
    }

    #[test]
    fn an_incomplete_final_frame_is_truncated_and_reported() {
        // The crash case the barrier exists for. Truncation is safe here
        // because the frame was never completed, so nothing committed is lost
        // — but it is reported, because somebody may have believed it was.
        forget_recovered_for_tests();
        let directory = tempfile::tempdir().unwrap();
        let paths = layout(&directory);
        let log = log_for(&paths);
        log.append("evt_000000000001", &serde_json::json!({"kind": "test"}))
            .unwrap();

        // A real torn tail is a frame the writer never finished, not
        // arbitrary trailing bytes: append a second record, then cut the file
        // short inside it, exactly as a crash mid-write would.
        let complete = std::fs::metadata(log.log_path()).unwrap().len();
        log.append("evt_000000000002", &serde_json::json!({"kind": "test"}))
            .unwrap();
        let whole = std::fs::read(log.log_path()).unwrap();
        let torn = complete as usize + (whole.len() - complete as usize) / 2;
        std::fs::write(log.log_path(), &whole[..torn]).unwrap();

        match ensure_recovered(&paths, "prj_000000000001").unwrap() {
            RecoveryOutcome::TruncatedTornTail { records, discarded } => {
                assert_eq!(records, 1);
                assert!(discarded > 0);
            }
            other => panic!("expected a truncated torn tail, got {other:?}"),
        }
    }

    #[test]
    fn hard_corruption_refuses_rather_than_repairing() {
        // A complete frame whose contents do not verify is a committed record
        // that was damaged. Truncating it away would destroy history to make
        // the log parse, so the barrier stops and leaves it for Doctor.
        forget_recovered_for_tests();
        let directory = tempfile::tempdir().unwrap();
        let paths = layout(&directory);
        let log = log_for(&paths);
        log.append("evt_000000000001", &serde_json::json!({"kind": "test"}))
            .unwrap();

        let mut bytes = std::fs::read(log.log_path()).unwrap();
        let midpoint = bytes.len() / 2;
        bytes[midpoint] ^= 0xff;
        std::fs::write(log.log_path(), &bytes).unwrap();

        let error = ensure_recovered(&paths, "prj_000000000001").unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::OperationLogCorrupt);
    }

    #[test]
    fn a_failed_barrier_is_not_remembered_as_done() {
        // Otherwise the first caller absorbs the failure and every later one
        // proceeds against a project nothing has cleared.
        forget_recovered_for_tests();
        let directory = tempfile::tempdir().unwrap();
        let paths = layout(&directory);
        let log = log_for(&paths);
        log.append("evt_000000000001", &serde_json::json!({"kind": "test"}))
            .unwrap();

        let mut bytes = std::fs::read(log.log_path()).unwrap();
        let midpoint = bytes.len() / 2;
        bytes[midpoint] ^= 0xff;
        std::fs::write(log.log_path(), &bytes).unwrap();

        assert!(ensure_recovered(&paths, "prj_000000000001").is_err());
        assert!(
            ensure_recovered(&paths, "prj_000000000001").is_err(),
            "the second caller must see the failure too, not a cached success"
        );
    }
}
