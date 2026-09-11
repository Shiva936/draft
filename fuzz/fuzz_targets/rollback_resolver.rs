#![no_main]
//! Fuzz recovery target handling: an arbitrary reference string must be parsed
//! safely (id-prefix + path safety) without panicking.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // A recovery target must be one of chk_/chg_/evt_ and never a path.
        let _ = s.starts_with("chk_") || s.starts_with("chg_") || s.starts_with("evt_");
        let _ = draft_core::support::pathguard::check_relative(s);
        let _ = draft_core::support::pathguard::is_draft_path(s);
    }
});
