//! `draft-core` — Draft-native v0.3.3 verified stable-base primitives.
//!
//! v0.3.3 is local-first and branchless. Project stability is represented by
//! verified stable base states and `stable_head`, while changepacks remain
//! temporary until `draft save` finalizes and disposes them.

pub mod adapters;
mod app;
pub mod common;
pub mod composition;
pub mod config;
pub mod error;
pub mod event;
pub mod fsutil;
pub mod gc;
pub mod hashing;
pub mod hidden;
pub mod home;
pub mod identity;
pub mod importexport;
pub mod index;
pub mod layout;
pub mod ledger;
pub mod lock;
pub mod lsif;
pub mod pack;
pub mod pathguard;
pub mod policy;
pub mod receipt;
pub mod risk;
pub mod signing;
pub mod stable;
pub mod transparency;
pub mod verification;

/// The Draft version string used in metadata, receipts, and `--version`.
pub const DRAFT_VERSION: &str = "0.3.3";

/// Schema version stamped into every persisted v0.3.3 format.
pub const DRAFT_SCHEMA_VERSION: &str = "0.3.3";

pub use app::*;
