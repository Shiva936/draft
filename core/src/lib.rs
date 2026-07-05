//! `draft-core` — Draft-native v0.3.2 verified-changepack primitives.
//!
//! v0.3.2 is local-first and daemonless. It maintains two hidden `.draft/`
//! metadata stores — a global user/device store (`~/.draft/`) and a project
//! store (`<root>/.draft/`) — and owns scanning, snapshots, changepacks,
//! signed receipts, hash-chained events, a tamper-evident transparency log,
//! portable `.draftpack` import/export, verification/risk/LSIF evidence,
//! and hardened save/rollback.

pub mod adapters;
mod app;
pub mod common;
pub mod config;
pub mod error;
pub mod event;
pub mod fsutil;
pub mod hashing;
pub mod hidden;
pub mod home;
pub mod identity;
pub mod importexport;
pub mod layout;
pub mod ledger;
pub mod lock;
pub mod lsif;
pub mod pack;
pub mod pathguard;
pub mod policy;
pub mod receipt;
pub mod riskv2;
pub mod signing;
pub mod transparency;
pub mod verifyv2;

/// The Draft version string used in metadata, receipts, and `--version`.
pub const DRAFT_VERSION: &str = "0.3.2";

/// Schema version stamped into every persisted v0.3.2 format.
pub const DRAFT_SCHEMA_VERSION: &str = "0.3.2";

pub use app::*;
