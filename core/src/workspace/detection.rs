//! Workspace + provider detection helpers.

use std::path::{Path, PathBuf};

use crate::error::{DraftError, DraftErrorKind, DraftResult};
use crate::vcs::registry::{ProviderRegistry, ProviderSelection};

use super::layout::DRAFT_DIR;

/// Walk up from `start` looking for an existing `.draft/` directory.
pub fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(dir) = cur {
        if dir.join(DRAFT_DIR).is_dir() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// Detect which provider owns `path` using the registry.
pub fn detect_provider(registry: &ProviderRegistry, path: &Path) -> DraftResult<ProviderSelection> {
    if registry.is_empty() {
        return Err(DraftError::new(
            DraftErrorKind::ProviderNotDetected,
            "No providers are registered.",
        ));
    }
    registry.detect(path).map_err(DraftError::from)
}
