//! An in-memory fake provider used by core unit tests and as the baseline for
//! the provider-contract test suite. Not registered in production.

use std::path::Path;
use std::sync::Mutex;

use super::capabilities::ProviderCapabilities;
use super::detection::{DetectionConfidence, ProviderDetection};
use super::errors::ProviderError;
use super::traits::{VcsProvider, VcsRepository};
use super::types::*;
use crate::common::now;
use crate::workspace::Workspace;

/// A fully in-memory provider that supports the entire `VcsRepository` contract.
pub struct FakeProvider;

impl FakeProvider {
    pub fn id() -> ProviderId {
        ProviderId::new("fake")
    }
}

impl VcsProvider for FakeProvider {
    fn id(&self) -> ProviderId {
        FakeProvider::id()
    }
    fn name(&self) -> &'static str {
        "Fake (test)"
    }
    fn description(&self) -> &'static str {
        "In-memory provider for tests only."
    }
    fn is_experimental(&self) -> bool {
        true
    }
    fn detect(&self, path: &Path) -> Result<ProviderDetection, ProviderError> {
        Ok(ProviderDetection {
            provider_id: self.id(),
            root: path.to_path_buf(),
            confidence: DetectionConfidence::Low,
            reason: "fake provider matches any path at low confidence".to_string(),
        })
    }
    fn open(&self, workspace: &Workspace) -> Result<Box<dyn VcsRepository>, ProviderError> {
        Ok(Box::new(FakeRepository::new(
            workspace.root.display().to_string(),
        )))
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_local_checkpoints: true,
            supports_finalization: true,
            has_mutable_working_change: true,
            ..ProviderCapabilities::NONE
        }
    }
}

pub struct FakeRepository {
    root: String,
    state: Mutex<FakeState>,
}

#[derive(Default)]
struct FakeState {
    finalized: u32,
    checkpoints: Vec<ProviderCheckpoint>,
    status: ProviderStatus,
}

impl FakeRepository {
    pub fn new(root: String) -> Self {
        FakeRepository {
            root,
            state: Mutex::new(FakeState::default()),
        }
    }

    /// Test helper: set the status this repository reports.
    pub fn set_status(&self, status: ProviderStatus) {
        self.state.lock().unwrap().status = status;
    }
}

impl VcsRepository for FakeRepository {
    fn provider_id(&self) -> ProviderId {
        FakeProvider::id()
    }
    fn current_view(&self) -> Result<ProviderView, ProviderError> {
        Ok(ProviderView {
            provider_id: self.provider_id(),
            revision: Some(ProviderRevisionId::new("fake-rev-0")),
            reference: Some(ProviderRef::new("main")),
            is_dirty: !self.state.lock().unwrap().status.entries.is_empty(),
            description: format!("fake repository at {}", self.root),
        })
    }
    fn status(&self) -> Result<ProviderStatus, ProviderError> {
        Ok(self.state.lock().unwrap().status.clone())
    }
    fn diff(&self, _input: DiffInput) -> Result<ProviderDelta, ProviderError> {
        Ok(ProviderDelta::default())
    }
    fn ignore_rules(&self) -> Result<IgnoreRules, ProviderError> {
        Ok(IgnoreRules {
            draft_dir_excluded: true,
            note: "fake provider does not track files".to_string(),
        })
    }
    fn conflicts(&self) -> Result<ConflictSet, ProviderError> {
        Ok(ConflictSet::default())
    }
    fn create_checkpoint(
        &self,
        input: CheckpointInput,
    ) -> Result<ProviderCheckpoint, ProviderError> {
        let cp = ProviderCheckpoint {
            id: ProviderCheckpointId::new(format!(
                "fake-cp-{}",
                self.state.lock().unwrap().checkpoints.len()
            )),
            kind: ProviderCheckpointKind::WorkingSnapshot,
            provider_refs: vec![],
            provider_revisions: vec![],
            created_at: now(),
            restore_token: input.description,
        };
        self.state.lock().unwrap().checkpoints.push(cp.clone());
        Ok(cp)
    }
    fn restore_checkpoint(
        &self,
        _checkpoint: ProviderCheckpointRef,
    ) -> Result<ProviderRestoreResult, ProviderError> {
        Ok(ProviderRestoreResult {
            restored: true,
            restored_paths: vec![],
            removed_paths: vec![],
            message: "fake restore".to_string(),
        })
    }
    fn prepare_finalization(
        &self,
        input: ProviderFinalizationInput,
    ) -> Result<ProviderFinalizationPlan, ProviderError> {
        Ok(ProviderFinalizationPlan {
            provider_id: self.provider_id(),
            base_revision: Some(ProviderRevisionId::new("fake-rev-0")),
            include_paths: input.include_paths,
            message: input.message,
            trailers: input.trailers,
            summary: "fake finalization".to_string(),
        })
    }
    fn finalize(
        &self,
        _plan: ProviderFinalizationPlan,
    ) -> Result<ProviderFinalizationResult, ProviderError> {
        let mut st = self.state.lock().unwrap();
        st.finalized += 1;
        let n = st.finalized;
        Ok(ProviderFinalizationResult {
            provider_id: self.provider_id(),
            object: ProviderObjectRef {
                provider_id: self.provider_id(),
                object_id: ProviderObjectId::new(format!("fake-object-{n}")),
                kind: "snapshot".to_string(),
                label: Some(format!("fake#{n}")),
            },
            base_revision: Some(ProviderRevisionId::new("fake-rev-0")),
            new_revision: Some(ProviderRevisionId::new(format!("fake-rev-{n}"))),
            reference: Some(ProviderRef::new("main")),
        })
    }
    fn undo_provider_action(
        &self,
        _input: ProviderUndoInput,
    ) -> Result<ProviderUndoResult, ProviderError> {
        Ok(ProviderUndoResult {
            undone: true,
            message: "fake undo".to_string(),
            provider_history_changed: false,
        })
    }
}
