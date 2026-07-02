//! Reserved service boundary.
//!
//! Draft v0.3.1 is local-only. This crate intentionally performs no network I/O
//! and exists only to keep the service workspace layout stable for later
//! design work.

/// External synchronization is disabled in v0.3.1.
pub const SYNC_ENABLED: bool = false;

/// Returns a human description of sync availability.
pub fn status() -> &'static str {
    "external synchronization is not available in v0.3.1"
}
