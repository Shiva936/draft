//! Assembles the default [`ProviderRegistry`] from the concrete providers.
//!
//! This crate exists so that `core` never depends on `providers/*` (avoiding a
//! dependency cycle): clients (CLI, `draftd`, tests) build the registry here.

use std::sync::Arc;

use draft_core::vcs::registry::ProviderRegistry;

/// Build a registry with the Git reference provider and all experimental
/// providers registered. Git is registered first so it wins ties at equal
/// detection confidence is unnecessary (Git detects at `Exact`).
pub fn default_registry() -> ProviderRegistry {
    let mut reg = ProviderRegistry::new();
    reg.register(Arc::new(draft_provider_git::GitProvider::new()));
    reg.register(Arc::new(draft_provider_jj::JjProvider::new()));
    reg.register(Arc::new(draft_provider_mercurial::MercurialProvider::new()));
    reg.register(Arc::new(draft_provider_pijul::PijulProvider::new()));
    // The filesystem provider matches any directory at low confidence, so it is
    // registered last as a fallback.
    reg.register(Arc::new(draft_provider_fs::FsProvider::new()));
    reg
}
