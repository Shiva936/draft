//! Reserved service boundary.
//!
//! Draft is local-only. This crate intentionally performs no network I/O
//! and exists only to keep the service workspace layout stable for later
//! design work.

/// External synchronization is disabled.
pub const SYNC_ENABLED: bool = false;

/// Returns a human description of sync availability.
pub fn status() -> &'static str {
    "external synchronization is not available"
}
