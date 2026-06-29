//! Workspace lock manager (FR-SVC-006). Thin, named wrapper over the core
//! advisory file lock ([`draft_core::lock::FileGuard`]). Locks live under
//! `.draft/locks/` so they coordinate across the CLI (embedded) and `draftd`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use draft_core::error::DraftResult;
use draft_core::lock::FileGuard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockType {
    WorkspaceRead,
    WorkspaceWrite,
    OperationLogAppend,
    Finalization,
    CheckpointRestore,
    VerificationRun,
}

impl LockType {
    pub fn file_name(&self) -> &'static str {
        match self {
            LockType::WorkspaceRead => "workspace-read.lock",
            LockType::WorkspaceWrite => "workspace-write.lock",
            LockType::OperationLogAppend => "operation-log.lock",
            LockType::Finalization => "finalization.lock",
            LockType::CheckpointRestore => "checkpoint-restore.lock",
            LockType::VerificationRun => "verification-run.lock",
        }
    }
}

/// Manages locks for a single workspace's `.draft/locks/` directory.
pub struct LockManager {
    locks_dir: PathBuf,
}

impl LockManager {
    pub fn new(draft_dir: &Path) -> Self {
        LockManager {
            locks_dir: draft_dir.join("locks"),
        }
    }

    /// Acquire a named lock, waiting up to `timeout`. The returned guard
    /// releases the lock on drop.
    pub fn acquire(&self, lock: LockType, timeout: Duration) -> DraftResult<FileGuard> {
        FileGuard::acquire(&self.locks_dir.join(lock.file_name()), timeout)
    }
}
