//! Git repository detection.

use std::path::Path;

use draft_core::vcs::detection::{DetectionConfidence, ProviderDetection};
use draft_core::vcs::errors::ProviderError;
use draft_core::vcs::types::ProviderId;

use crate::command::GitCommand;
use crate::provider_id;

/// Detect a Git repository at or above `path`.
///
/// Uses `git rev-parse --show-toplevel` which correctly handles worktrees and
/// nested repositories. Confidence is `Exact` when git resolves a toplevel.
pub fn detect(path: &Path) -> Result<ProviderDetection, ProviderError> {
    let git = GitCommand::new(path);
    match git.toplevel() {
        Ok(root) => Ok(ProviderDetection {
            provider_id: provider_id(),
            root,
            confidence: DetectionConfidence::Exact,
            reason: "git resolved a repository toplevel".to_string(),
        }),
        Err(_) => Ok(ProviderDetection::none(
            ProviderId::new("git"),
            path.to_path_buf(),
        )),
    }
}
