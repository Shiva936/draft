#![no_main]
//! Fuzz AG-UI request-body parsing and the canonical JSON serializer that the
//! trust layer relies on.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
            let canonical = draft_core::hashing::canonical_json(&v);
            let _ = draft_core::hashing::sha256_hex(canonical.as_bytes());
        }
    }
});
