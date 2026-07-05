//! Security fixture suite (NFRD §10, PRD §12) — runs on stable `cargo test` so
//! CI always exercises it. Every malicious fixture must fail closed: the parser
//! rejects it and nothing is mutated. The `fuzz/` crate provides libFuzzer
//! targets over the same parsers for deeper, nightly fuzzing.

use draft_core::importexport::read_archive;
use draft_core::pack::PackManifest;
use draft_core::pathguard::{self, PathViolation};
use draft_core::receipt::ReceiptRecord;

/// Build a raw ustar archive with an arbitrary (possibly malicious) entry.
fn raw_tar(entries: &[(&str, &[u8], u8)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, data, typeflag) in entries {
        let mut block = [0u8; 512];
        let nb = name.as_bytes();
        let n = nb.len().min(100);
        block[..n].copy_from_slice(&nb[..n]);
        block[100..108].copy_from_slice(b"0000644\0");
        block[108..116].copy_from_slice(b"0000000\0");
        block[116..124].copy_from_slice(b"0000000\0");
        block[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        block[136..148].copy_from_slice(b"00000000000\0");
        block[156] = *typeflag;
        block[257..263].copy_from_slice(b"ustar\0");
        block[263..265].copy_from_slice(b"00");
        for b in block.iter_mut().skip(148).take(8) {
            *b = b' ';
        }
        let sum: u32 = block.iter().map(|&b| b as u32).sum();
        block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        out.extend_from_slice(&block);
        out.extend_from_slice(data);
        let pad = (512 - data.len() % 512) % 512;
        out.resize(out.len() + pad, 0u8);
    }
    out.extend_from_slice(&[0u8; 1024]);
    out
}

fn write_artifact(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.draftpack");
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

// ---- Path sanitizer fixtures --------------------------------------------

#[test]
fn path_sanitizer_rejects_all_unsafe_classes() {
    assert_eq!(
        pathguard::check_relative("../etc/passwd"),
        Err(PathViolation::ParentTraversal)
    );
    assert_eq!(
        pathguard::check_relative("/etc/passwd"),
        Err(PathViolation::Absolute)
    );
    assert_eq!(
        pathguard::check_relative("C:\\Windows\\x"),
        Err(PathViolation::WindowsPrefix)
    );
    assert_eq!(
        pathguard::check_relative(".draft/keys/signing.key"),
        Err(PathViolation::DraftReserved)
    );
    // Case-insensitive `.DRAFT/`.
    assert_eq!(
        pathguard::check_relative("a/.DRAFT/b"),
        Err(PathViolation::DraftReserved)
    );
    // Invalid UTF-8 archive entry name.
    assert_eq!(
        pathguard::from_bytes(&[0x66, 0xff, 0xfe]),
        Err(PathViolation::InvalidEncoding)
    );
    // Embedded NUL.
    assert_eq!(
        pathguard::check_relative("a\0b"),
        Err(PathViolation::InvalidEncoding)
    );
}

// ---- Import parser fixtures ---------------------------------------------

#[test]
fn import_rejects_path_traversal() {
    let (_d, p) = write_artifact(&raw_tar(&[("../escape.txt", b"x", b'0')]));
    let err = read_archive(&p).unwrap_err();
    assert!(err.message.contains("traversal"), "{}", err.message);
}

#[test]
fn import_rejects_absolute_path() {
    let (_d, p) = write_artifact(&raw_tar(&[("/etc/cron.d/evil", b"x", b'0')]));
    assert!(read_archive(&p).unwrap_err().message.contains("absolute"));
}

#[test]
fn import_rejects_draft_write() {
    let (_d, p) = write_artifact(&raw_tar(&[(".draft/keys/signing.key", b"x", b'0')]));
    assert!(read_archive(&p).unwrap_err().message.contains(".draft/"));
}

#[test]
fn import_rejects_symlink_and_hardlink_and_device() {
    // typeflag '2' = symlink, '1' = hardlink, '3' = char device.
    for (tf, needle) in [(b'2', "symlink"), (b'1', "hardlink"), (b'3', "device")] {
        let (_d, p) = write_artifact(&raw_tar(&[("payload", b"", tf)]));
        let err = read_archive(&p).unwrap_err();
        assert!(
            err.message.contains(needle),
            "expected {needle}: {}",
            err.message
        );
    }
}

#[test]
fn import_rejects_zip_bomb_declared_size() {
    // A header that declares a gigantic size (no real data) must be rejected by
    // the uncompressed-size guard before any read.
    let mut block = [0u8; 512];
    block[..7].copy_from_slice(b"big.bin");
    block[100..108].copy_from_slice(b"0000644\0");
    block[108..116].copy_from_slice(b"0000000\0");
    block[116..124].copy_from_slice(b"0000000\0");
    // 0o7777777777777 octal is enormous (> limit).
    block[124..136].copy_from_slice(b"77777777777\0");
    block[136..148].copy_from_slice(b"00000000000\0");
    block[156] = b'0';
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    for b in block.iter_mut().skip(148).take(8) {
        *b = b' ';
    }
    let sum: u32 = block.iter().map(|&b| b as u32).sum();
    block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    let mut bytes = block.to_vec();
    bytes.extend_from_slice(&[0u8; 1024]);
    let (_d, p) = write_artifact(&bytes);
    let err = read_archive(&p).unwrap_err();
    assert!(err.message.contains("zip bomb"), "{}", err.message);
}

#[test]
fn import_rejects_invalid_utf8_entry_name() {
    // Entry name with an invalid UTF-8 byte.
    let (_d, p) = write_artifact(&raw_tar(&[("na\u{00ff}me", b"x", b'0')]));
    // The name is valid UTF-8 here (é etc.), so instead craft raw invalid bytes:
    let bad = {
        let mut block = [0u8; 512];
        block[0] = 0x66;
        block[1] = 0xff; // invalid UTF-8
        block[100..108].copy_from_slice(b"0000644\0");
        block[124..136].copy_from_slice(b"00000000001\0");
        block[156] = b'0';
        block[257..263].copy_from_slice(b"ustar\0");
        block[263..265].copy_from_slice(b"00");
        for b in block.iter_mut().skip(148).take(8) {
            *b = b' ';
        }
        let sum: u32 = block.iter().map(|&b| b as u32).sum();
        block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let mut v = block.to_vec();
        v.push(b'x');
        v.resize(v.len() + 511, 0);
        v.extend_from_slice(&[0u8; 1024]);
        v
    };
    let (_d2, p2) = write_artifact(&bad);
    let _ = p; // first fixture may or may not be rejected depending on name
    assert!(read_archive(&p2).is_err());
}

// ---- Manifest & receipt parser fixtures ---------------------------------

#[test]
fn manifest_parser_rejects_corrupt_and_wrong_schema() {
    // Corrupt JSON.
    assert!(serde_json::from_str::<PackManifest>("{ not json ").is_err());
    // Wrong schema version fails the support check.
    let m: PackManifest = serde_json::from_str(
        r#"{"schema_version":"0.2.0","pack_id":"pck_x","name":"n","description":"",
            "intent":"feature","origin":"local","actor":"a","candidate":null,
            "created_at":"t","base_workspace_hash":"h","target_workspace_hash":"h",
            "changes_hash":"h","risk_hash":"","verify_hash":"","lsif_hash":"",
            "receipt_hashes":[],"import_state":"none","approval_state":"pending",
            "save_state":"unsaved"}"#,
    )
    .unwrap();
    assert!(m.ensure_supported().is_err());
}

#[test]
fn receipt_parser_rejects_corrupt() {
    assert!(serde_json::from_str::<ReceiptRecord>("").is_err());
    assert!(serde_json::from_str::<ReceiptRecord>(r#"{"receipt_id":"rcp_x"}"#).is_err());
}

// ---- Event log parser fixtures ------------------------------------------

#[test]
fn event_parser_rejects_corrupt_line() {
    use draft_core::event::EventRecord;
    assert!(serde_json::from_str::<EventRecord>("{ garbage").is_err());
}
