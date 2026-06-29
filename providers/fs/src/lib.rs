//! Filesystem provider — an **experimental, limited** provider for plain
//! folders. It supports detection and a shallow status scan; it has no native
//! version control, so finalization is unsupported (returns a structured error).

use std::path::{Path, PathBuf};

use draft_core::common::WorkspacePath;
use draft_core::vcs::capabilities::ProviderCapabilities;
use draft_core::vcs::detection::{DetectionConfidence, ProviderDetection};
use draft_core::vcs::errors::ProviderError;
use draft_core::vcs::traits::{VcsProvider, VcsRepository};
use draft_core::vcs::types::*;
use draft_core::workspace::Workspace;

pub fn provider_id() -> ProviderId {
    ProviderId::new("fs")
}

#[derive(Debug, Default, Clone)]
pub struct FsProvider;

impl FsProvider {
    pub fn new() -> Self {
        FsProvider
    }
}

impl VcsProvider for FsProvider {
    fn id(&self) -> ProviderId {
        provider_id()
    }
    fn name(&self) -> &'static str {
        "Filesystem"
    }
    fn description(&self) -> &'static str {
        "Experimental plain-folder provider (no native finalization)."
    }
    fn is_experimental(&self) -> bool {
        true
    }
    fn detect(&self, path: &Path) -> Result<ProviderDetection, ProviderError> {
        // Matches any existing directory, at low confidence so real VCS
        // providers win when present.
        if path.is_dir() {
            Ok(ProviderDetection {
                provider_id: provider_id(),
                root: path.to_path_buf(),
                confidence: DetectionConfidence::Low,
                reason: "plain directory".to_string(),
            })
        } else {
            Ok(ProviderDetection::none(provider_id(), path.to_path_buf()))
        }
    }
    fn open(&self, workspace: &Workspace) -> Result<Box<dyn VcsRepository>, ProviderError> {
        Ok(Box::new(FsRepository {
            root: workspace.provider_root.clone(),
        }))
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            has_mutable_working_change: true,
            ..ProviderCapabilities::NONE
        }
    }
}

pub struct FsRepository {
    root: PathBuf,
}

impl VcsRepository for FsRepository {
    fn provider_id(&self) -> ProviderId {
        provider_id()
    }
    fn current_view(&self) -> Result<ProviderView, ProviderError> {
        Ok(ProviderView {
            provider_id: provider_id(),
            revision: None,
            reference: None,
            is_dirty: false,
            description: format!("plain folder at {}", self.root.display()),
        })
    }
    fn status(&self) -> Result<ProviderStatus, ProviderError> {
        // Shallow scan: list top-level files (excluding .draft) as untracked.
        let mut entries = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.root) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name == ".draft" || name.starts_with('.') {
                    continue;
                }
                if e.path().is_file() {
                    entries.push(StatusEntry {
                        path: WorkspacePath::new(name),
                        status: FileStatus::Untracked,
                        old_path: None,
                    });
                }
            }
        }
        Ok(ProviderStatus {
            entries,
            has_staged_changes: false,
            has_conflicts: false,
        })
    }
    fn diff(&self, _input: DiffInput) -> Result<ProviderDelta, ProviderError> {
        // The filesystem provider has no base to diff against.
        Ok(ProviderDelta::default())
    }
    fn ignore_rules(&self) -> Result<IgnoreRules, ProviderError> {
        Ok(IgnoreRules {
            draft_dir_excluded: true,
            note: "filesystem provider does not track history".to_string(),
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
            "checkpoints (filesystem provider is limited)",
        ))
    }
    fn restore_checkpoint(
        &self,
        _checkpoint: ProviderCheckpointRef,
    ) -> Result<ProviderRestoreResult, ProviderError> {
        Err(ProviderError::unsupported("checkpoint restore"))
    }
    fn prepare_finalization(
        &self,
        _input: ProviderFinalizationInput,
    ) -> Result<ProviderFinalizationPlan, ProviderError> {
        Err(ProviderError::unsupported(
            "finalization (no native version control)",
        ))
    }
    fn finalize(
        &self,
        _plan: ProviderFinalizationPlan,
    ) -> Result<ProviderFinalizationResult, ProviderError> {
        Err(ProviderError::unsupported(
            "finalization (no native version control)",
        ))
    }
    fn undo_provider_action(
        &self,
        _input: ProviderUndoInput,
    ) -> Result<ProviderUndoResult, ProviderError> {
        Err(ProviderError::unsupported("undo"))
    }
}
