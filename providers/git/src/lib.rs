//! Git provider — the first complete reference [`VcsProvider`] for Draft.
//!
//! All Git-specific behavior lives here. Core, CLI, TUI, and services never call
//! Git directly; they go through the provider-neutral trait surface.

mod capabilities;
mod checkpoint;
mod command;
mod conflicts;
mod detection;
mod diff;
mod finalization;
mod parse;
mod repository;
mod status;

use std::path::Path;

use draft_core::vcs::capabilities::ProviderCapabilities;
use draft_core::vcs::detection::ProviderDetection;
use draft_core::vcs::errors::ProviderError;
use draft_core::vcs::traits::{VcsProvider, VcsRepository};
use draft_core::vcs::types::ProviderId;
use draft_core::workspace::Workspace;

pub use command::GitCommand;
pub use repository::GitRepository;

/// The canonical provider id for Git.
pub fn provider_id() -> ProviderId {
    ProviderId::new("git")
}

/// The Git provider.
#[derive(Debug, Default, Clone)]
pub struct GitProvider;

impl GitProvider {
    pub fn new() -> Self {
        GitProvider
    }
}

impl VcsProvider for GitProvider {
    fn id(&self) -> ProviderId {
        provider_id()
    }

    fn name(&self) -> &'static str {
        "Git"
    }

    fn description(&self) -> &'static str {
        "Complete reference provider backed by the system `git` binary."
    }

    fn is_experimental(&self) -> bool {
        false
    }

    fn detect(&self, path: &Path) -> Result<ProviderDetection, ProviderError> {
        detection::detect(path)
    }

    fn open(&self, workspace: &Workspace) -> Result<Box<dyn VcsRepository>, ProviderError> {
        Ok(Box::new(GitRepository::new(
            workspace.provider_root.clone(),
        )))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        capabilities::git_capabilities()
    }
}
