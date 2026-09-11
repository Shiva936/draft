#![no_main]
//! Fuzz the Change manifest parser, its schema-version check and its digest.
//!
//! The digest recomputation is the interesting half: a manifest that parses
//! but whose stored digest disagrees with its own canonical form is exactly
//! the shape a corrupted or forged record takes, and recomputing must never
//! panic on one.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(m) = serde_json::from_str::<draft_core::dcg::change_store::ChangeManifest>(s) {
            let _ = m.ensure_supported();
            let _ = m.recompute_manifest_digest();
        }
    }
});
