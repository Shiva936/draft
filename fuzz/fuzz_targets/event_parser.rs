#![no_main]
//! Fuzz the canonical event-record parser and hash recomputation.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(e) = serde_json::from_str::<draft_core::event::EventRecord>(s) {
            let _ = e.recompute_hash();
        }
    }
});
