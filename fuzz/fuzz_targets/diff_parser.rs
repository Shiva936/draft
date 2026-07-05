#![no_main]
//! Fuzz the diff (`changes.patch`) parser — a serialized PatchSet.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<draft_core::PatchSet>(s);
    }
});
