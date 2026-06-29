//! The provider trait surface that core depends on.

use std::path::Path;

use super::capabilities::ProviderCapabilities;
use super::detection::ProviderDetection;
use super::errors::ProviderError;
use super::types::*;
use crate::workspace::Workspace;

/// A version-control (or filesystem) backend integration.
///
/// A `VcsProvider` is cheap to construct and stateless w.r.t. a specific
/// workspace; `open` produces a `VcsRepository` bound to one workspace.
pub trait VcsProvider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn name(&self) -> &'static str;
    /// Returns a human one-line description, including experimental status.
    fn description(&self) -> &'static str {
        ""
    }
    /// Whether this provider is production-ready (`false` => experimental).
    fn is_experimental(&self) -> bool {
        false
    }
    fn detect(&self, path: &Path) -> Result<ProviderDetection, ProviderError>;
    fn open(&self, workspace: &Workspace) -> Result<Box<dyn VcsRepository>, ProviderError>;
    fn capabilities(&self) -> ProviderCapabilities;
}

/// A provider bound to a specific opened workspace.
pub trait VcsRepository: Send + Sync {
    fn provider_id(&self) -> ProviderId;

    fn current_view(&self) -> Result<ProviderView, ProviderError>;
    fn status(&self) -> Result<ProviderStatus, ProviderError>;
    fn diff(&self, input: DiffInput) -> Result<ProviderDelta, ProviderError>;

    fn ignore_rules(&self) -> Result<IgnoreRules, ProviderError>;
    fn conflicts(&self) -> Result<ConflictSet, ProviderError>;

    fn create_checkpoint(
        &self,
        input: CheckpointInput,
    ) -> Result<ProviderCheckpoint, ProviderError>;

    fn restore_checkpoint(
        &self,
        checkpoint: ProviderCheckpointRef,
    ) -> Result<ProviderRestoreResult, ProviderError>;

    fn prepare_finalization(
        &self,
        input: ProviderFinalizationInput,
    ) -> Result<ProviderFinalizationPlan, ProviderError>;

    fn finalize(
        &self,
        plan: ProviderFinalizationPlan,
    ) -> Result<ProviderFinalizationResult, ProviderError>;

    fn undo_provider_action(
        &self,
        input: ProviderUndoInput,
    ) -> Result<ProviderUndoResult, ProviderError>;
}
