//! Pijul provider — **experimental scaffold**. Detects a `.pijul` repository
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
    ProviderId::new("pijul")
}

#[derive(Debug, Default, Clone)]
pub struct PijulProvider;

impl PijulProvider {
    pub fn new() -> Self {
        PijulProvider
    }
}

impl VcsProvider for PijulProvider {
    fn id(&self) -> ProviderId {
        provider_id()
    }
    fn name(&self) -> &'static str {
        "Pijul"
    }
    fn description(&self) -> &'static str {
        "Experimental Pijul provider (detection + capabilities only)."
    }
    fn is_experimental(&self) -> bool {
        true
    }
    fn detect(&self, path: &Path) -> Result<ProviderDetection, ProviderError> {
        Ok(detect_marker(
            path,
            ".pijul",
            provider_id(),
            "found .pijul repository",
        ))
    }
    fn open(&self, workspace: &Workspace) -> Result<Box<dyn VcsRepository>, ProviderError> {
        Ok(Box::new(ExperimentalRepository::new(
            provider_id(),
            workspace.provider_root.clone(),
            "pijul",
        )))
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            has_mutable_working_change: true,
            supports_patch_identity: true,
            supports_remote_publish: true,
            ..ProviderCapabilities::NONE
        }
    }
}
