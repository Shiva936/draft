//! Physical `.draft/` layout: directory names and path helpers.

use std::path::{Path, PathBuf};

use crate::error::DraftResult;
use crate::fsutil::ensure_dir;

/// Name of the workspace-local Draft directory.
pub const DRAFT_DIR: &str = ".draft";

/// Subdirectories created on workspace init (FR-WS-002).
pub const SUBDIRS: &[&str] = &[
    "operations",
    "changes",
    "reviews",
    "checkpoints",
    "verification",
    "verification/logs",
    "receipts",
    "locks",
    "objects/blobs",
    "backup",
];

/// Resolves paths inside a workspace's `.draft/` directory.
#[derive(Debug, Clone)]
pub struct DraftLayout {
    pub draft_dir: PathBuf,
}

impl DraftLayout {
    pub fn new(draft_dir: PathBuf) -> Self {
        DraftLayout { draft_dir }
    }

    pub fn for_root(root: &Path) -> Self {
        DraftLayout {
            draft_dir: root.join(DRAFT_DIR),
        }
    }

    pub fn exists(&self) -> bool {
        self.draft_dir.is_dir()
    }

    pub fn create_all(&self) -> DraftResult<()> {
        ensure_dir(&self.draft_dir)?;
        for sub in SUBDIRS {
            ensure_dir(&self.draft_dir.join(sub))?;
        }
        Ok(())
    }

    pub fn config_toml(&self) -> PathBuf {
        self.draft_dir.join("config.toml")
    }
    pub fn workspace_json(&self) -> PathBuf {
        self.draft_dir.join("workspace.json")
    }
    pub fn identity_json(&self) -> PathBuf {
        self.draft_dir.join("identity.json")
    }
    pub fn operations_dir(&self) -> PathBuf {
        self.draft_dir.join("operations")
    }
    pub fn changes_dir(&self) -> PathBuf {
        self.draft_dir.join("changes")
    }
    pub fn reviews_dir(&self) -> PathBuf {
        self.draft_dir.join("reviews")
    }
    pub fn checkpoints_dir(&self) -> PathBuf {
        self.draft_dir.join("checkpoints")
    }
    pub fn verification_dir(&self) -> PathBuf {
        self.draft_dir.join("verification")
    }
    pub fn verification_logs_dir(&self) -> PathBuf {
        self.draft_dir.join("verification/logs")
    }
    pub fn receipts_dir(&self) -> PathBuf {
        self.draft_dir.join("receipts")
    }
    pub fn locks_dir(&self) -> PathBuf {
        self.draft_dir.join("locks")
    }
    pub fn backup_dir(&self) -> PathBuf {
        self.draft_dir.join("backup")
    }
}
