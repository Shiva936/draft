#![no_main]
//! Fuzz rollback target handling: an arbitrary reference string must be parsed
//! safely (id-prefix + path safety) without panicking.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // A rollback target must be one of chk_/pck_/rcp_ and never a path.
        let _ = s.starts_with("chk_") || s.starts_with("pck_") || s.starts_with("rcp_");
        let _ = draft_core::pathguard::check_relative(s);
        let _ = draft_core::pathguard::is_draft_path(s);
    }
});
