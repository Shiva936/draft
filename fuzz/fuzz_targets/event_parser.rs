#![no_main]
//! Fuzz the canonical Activity record parser and its chain-hash recomputation.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(record) = serde_json::from_str::<draft_core::activity::LedgerRecord>(s) {
            let _ = record.recompute_hash();
        }
    }
});
