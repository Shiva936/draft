#![no_main]
//! Fuzz the signed-receipt parser and its signable serialization.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(r) = serde_json::from_str::<draft_core::receipt::ReceiptRecord>(s) {
            let _ = r.signable_bytes();
        }
    }
});
