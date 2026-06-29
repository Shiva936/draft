//! Sync placeholder.
//!
//! Remote/cloud sync is explicitly **out of scope** for v0.2.0 (Known
//! Limitations). This crate exists as the reserved extension point so the
//! `services/` layout and future roadmap are stable. It performs no network I/O.

/// Sync is disabled in v0.2.0.
pub const SYNC_ENABLED: bool = false;

/// Returns a human description of sync availability.
pub fn status() -> &'static str {
    "sync is not available in v0.2.0 (local-first only)"
}
