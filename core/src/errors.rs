use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DraftError {
    #[error("Not a Git repository. Draft must be run inside a Git repository.")]
    NotGitRepo,

    #[error("Unsupported repository state: {0}")]
    UnsupportedRepoState(String),

    #[error("Git command '{command}' failed with exit code {exit_code:?}: {stderr}")]
    GitCommandFailed {
        command: String,
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Failed to parse diff: {0}")]
    DiffParseError(String),

    #[error("Verification failed: {0}")]
    VerificationFailed(String),

    #[error("Commit blocked: {0}")]
    CommitBlocked(String),

    #[error("Conflict detected in files: {0:?}")]
    ConflictDetected(Vec<PathBuf>),

    #[error("Pre-commit checkpoint is missing.")]
    CheckpointMissing,

    #[error("Commit receipt is missing.")]
    ReceiptMissing,

    #[error("I/O error: {0}")]
    Io(String),
}

impl From<std::io::Error> for DraftError {
    fn from(err: std::io::Error) -> Self {
        DraftError::Io(err.to_string())
    }
}
