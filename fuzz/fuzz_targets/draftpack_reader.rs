#![no_main]
//! Fuzz the `.draftpack` archive reader over in-memory bytes on disk.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pack.draftpack");
    if std::fs::write(&path, data).is_ok() {
        if let Ok(archive) = draft_core::importexport::read_archive(&path) {
            // Reading known members must also be panic-free.
            let _ = archive.get("manifest.json");
        }
    }
});
