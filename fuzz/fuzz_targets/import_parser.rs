#![no_main]
//! Fuzz the untrusted `.draftpack` import parser (the security boundary).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fuzz.draftpack");
    if std::fs::write(&path, data).is_ok() {
        // Must never panic and must fail closed on malformed input.
        let _ = draft_core::importexport::read_archive(&path);
    }
});
