#![no_main]
//! Fuzz the central path-safety guard over raw (possibly non-UTF-8) bytes.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = draft_core::support::pathguard::from_bytes(data);
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = draft_core::support::pathguard::check_relative(s);
        let _ = draft_core::support::pathguard::is_draft_path(s);
    }
});
