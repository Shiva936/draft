//! `draft-core` — Draft-native v0.3.0 change-control primitives.
//!
//! v0.3.0 is local-first and stores verified changepacks in `.draft/`.
//! Core owns local `.draft/` storage, scanning, snapshots, changepacks,
//! evidence, verification, review, approval, save receipts, and rollback.

pub mod common;
pub mod error;
pub mod fsutil;
pub mod identity;
pub mod lock;
mod v3;

/// The Draft version string used in metadata, receipts, and `--version`.
pub const DRAFT_VERSION: &str = "0.3.0";

pub use v3::*;
