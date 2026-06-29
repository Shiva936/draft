//! Shared helpers for experimental providers (jj, mercurial, pijul).
//!
//! These providers ship as detection + capability scaffolds in v0.2.0; most
//! repository operations return structured unsupported-operation errors.

use std::path::{Path, PathBuf};

use super::detection::{DetectionConfidence, ProviderDetection};
use super::errors::ProviderError;
use super::traits::VcsRepository;
use super::types::*;

/// Walk up from `path` looking for a marker directory (e.g. `.hg`). Returns an
/// `Exact` detection rooted at the directory that contains it.
pub fn detect_marker(
    path: &Path,
    marker: &str,
    provider_id: ProviderId,
    reason: &str,
) -> ProviderDetection {
    let mut cur = Some(path);
    while let Some(dir) = cur {
        if dir.join(marker).exists() {
            return ProviderDetection {
                provider_id,
                root: dir.to_path_buf(),
                confidence: DetectionConfidence::Exact,
                reason: reason.to_string(),
            };
        }
        cur = dir.parent();
    }
    ProviderDetection::none(provider_id, path.to_path_buf())
}

/// A repository that reports a view but returns unsupported for state-changing
/// (and most read) operations, suitable for experimental provider scaffolds.
pub struct ExperimentalRepository {
    provider_id: ProviderId,
    root: PathBuf,
    label: &'static str,
}

impl ExperimentalRepository {
    pub fn new(provider_id: ProviderId, root: PathBuf, label: &'static str) -> Self {
        ExperimentalRepository {
            provider_id,
            root,
            label,
        }
    }
}

impl VcsRepository for ExperimentalRepository {
    fn provider_id(&self) -> ProviderId {
        self.provider_id.clone()
    }
    fn current_view(&self) -> Result<ProviderView, ProviderError> {
        Ok(ProviderView {
            provider_id: self.provider_id.clone(),
            revision: None,
            reference: None,
            is_dirty: false,
            description: format!(
                "{} workspace at {} (experimental)",
                self.label,
                self.root.display()
            ),
        })
    }
    fn status(&self) -> Result<ProviderStatus, ProviderError> {
        Err(ProviderError::unsupported("status (experimental provider)"))
    }
    fn diff(&self, _input: DiffInput) -> Result<ProviderDelta, ProviderError> {
        Err(ProviderError::unsupported("diff (experimental provider)"))
    }
    fn ignore_rules(&self) -> Result<IgnoreRules, ProviderError> {
        Ok(IgnoreRules {
            draft_dir_excluded: false,
            note: "experimental provider does not manage ignores".to_string(),
        })
    }
    fn conflicts(&self) -> Result<ConflictSet, ProviderError> {
        Ok(ConflictSet::default())
    }
    fn create_checkpoint(
        &self,
        _input: CheckpointInput,
    ) -> Result<ProviderCheckpoint, ProviderError> {
        Err(ProviderError::unsupported(
            "checkpoints (experimental provider)",
        ))
    }
    fn restore_checkpoint(
        &self,
        _checkpoint: ProviderCheckpointRef,
    ) -> Result<ProviderRestoreResult, ProviderError> {
        Err(ProviderError::unsupported(
            "checkpoint restore (experimental provider)",
        ))
    }
    fn prepare_finalization(
        &self,
        _input: ProviderFinalizationInput,
    ) -> Result<ProviderFinalizationPlan, ProviderError> {
        Err(ProviderError::unsupported(
            "finalization (experimental provider)",
        ))
    }
    fn finalize(
        &self,
        _plan: ProviderFinalizationPlan,
    ) -> Result<ProviderFinalizationResult, ProviderError> {
        Err(ProviderError::unsupported(
            "finalization (experimental provider)",
        ))
    }
    fn undo_provider_action(
        &self,
        _input: ProviderUndoInput,
    ) -> Result<ProviderUndoResult, ProviderError> {
        Err(ProviderError::unsupported("undo (experimental provider)"))
    }
}
