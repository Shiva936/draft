#![no_main]
//! Fuzz the pack manifest parser + schema-version check.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(m) = serde_json::from_str::<draft_core::pack::PackManifest>(s) {
            let _ = m.ensure_supported();
            let _ = m.lifecycle();
        }
    }
});
