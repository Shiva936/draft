//! Mercurial provider — **experimental scaffold**. Detects a `.hg` repository
//! and declares capabilities; operations return structured unsupported errors.

use std::path::Path;

use draft_core::vcs::capabilities::ProviderCapabilities;
use draft_core::vcs::detection::ProviderDetection;
use draft_core::vcs::errors::ProviderError;
use draft_core::vcs::experimental::{detect_marker, ExperimentalRepository};
use draft_core::vcs::traits::{VcsProvider, VcsRepository};
use draft_core::vcs::types::*;
use draft_core::workspace::Workspace;

pub fn provider_id() -> ProviderId {
    ProviderId::new("mercurial")
}

#[derive(Debug, Default, Clone)]
pub struct MercurialProvider;

impl MercurialProvider {
    pub fn new() -> Self {
        MercurialProvider
    }
}

impl VcsProvider for MercurialProvider {
    fn id(&self) -> ProviderId {
        provider_id()
    }
    fn name(&self) -> &'static str {
        "Mercurial"
    }
    fn description(&self) -> &'static str {
        "Experimental Mercurial provider (detection + capabilities only)."
    }
    fn is_experimental(&self) -> bool {
        true
    }
    fn detect(&self, path: &Path) -> Result<ProviderDetection, ProviderError> {
        Ok(detect_marker(
            path,
            ".hg",
            provider_id(),
            "found .hg repository",
        ))
    }
    fn open(&self, workspace: &Workspace) -> Result<Box<dyn VcsRepository>, ProviderError> {
        Ok(Box::new(ExperimentalRepository::new(
            provider_id(),
            workspace.provider_root.clone(),
            "mercurial",
        )))
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            has_mutable_working_change: true,
            supports_phases_or_publish_state: true,
            supports_remote_publish: true,
            supports_history_rewrite: true,
            ..ProviderCapabilities::NONE
        }
    }
}
