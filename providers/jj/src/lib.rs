//! Jujutsu (jj) provider — **experimental scaffold**. Detects a `.jj` workspace
//! and declares capabilities; most operations return structured
//! unsupported-operation errors in v0.2.0.

use std::path::Path;

use draft_core::vcs::capabilities::ProviderCapabilities;
use draft_core::vcs::detection::ProviderDetection;
use draft_core::vcs::errors::ProviderError;
use draft_core::vcs::traits::{VcsProvider, VcsRepository};
use draft_core::vcs::types::*;
use draft_core::workspace::Workspace;

use draft_core::vcs::experimental::{detect_marker, ExperimentalRepository};

pub fn provider_id() -> ProviderId {
    ProviderId::new("jj")
}

#[derive(Debug, Default, Clone)]
pub struct JjProvider;

impl JjProvider {
    pub fn new() -> Self {
        JjProvider
    }
}

impl VcsProvider for JjProvider {
    fn id(&self) -> ProviderId {
        provider_id()
    }
    fn name(&self) -> &'static str {
        "Jujutsu"
    }
    fn description(&self) -> &'static str {
        "Experimental jj provider (detection + capabilities only)."
    }
    fn is_experimental(&self) -> bool {
        true
    }
    fn detect(&self, path: &Path) -> Result<ProviderDetection, ProviderError> {
        Ok(detect_marker(
            path,
            ".jj",
            provider_id(),
            "found .jj workspace",
        ))
    }
    fn open(&self, workspace: &Workspace) -> Result<Box<dyn VcsRepository>, ProviderError> {
        Ok(Box::new(ExperimentalRepository::new(
            provider_id(),
            workspace.provider_root.clone(),
            "jj",
        )))
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            has_operation_log: true,
            has_change_ids: true,
            has_mutable_working_change: true,
            supports_history_rewrite: true,
            supports_multiple_workspaces: true,
            ..ProviderCapabilities::NONE
        }
    }
}
