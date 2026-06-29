//! `draft-core` — the provider-neutral heart of Draft v0.2.0.
//!
//! Core owns the workspace model, operation log, change/review/risk/verification/
//! checkpoint/finalization/receipt/identity/conflict engines, and the provider
//! abstraction (`vcs`). It contains **no** Git (or any provider) specific code;
//! providers live under `providers/*` and are assembled by clients into a
//! [`vcs::registry::ProviderRegistry`].

pub mod app;
pub mod changes;
pub mod checkpoint;
pub mod common;
pub mod conflict;
pub mod error;
pub mod finalization;
pub mod fsutil;
pub mod identity;
pub mod lock;
pub mod migration;
pub mod operations;
pub mod receipts;
pub mod review;
pub mod risk;
pub mod vcs;
pub mod verification;
pub mod workspace;

/// The Draft version string used in metadata, receipts, and `--version`.
pub const DRAFT_VERSION: &str = "0.2.0";

#[cfg(test)]
mod tests_engines;
#[cfg(test)]
mod tests_foundation;
