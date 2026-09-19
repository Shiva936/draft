//! Security fixture suite — runs on stable `cargo test` so
//! CI always exercises it. Every malicious fixture must fail closed: the parser
//! rejects it and nothing is mutated. The `fuzz/` crate provides libFuzzer
//! targets over the same parsers for deeper, nightly fuzzing.

use draft_core::dcg::change_pack_store::ChangePackManifest;
use draft_core::support::pathguard::{self, PathViolation};
use draft_dcg_contract::receipt::ReceiptEnvelope;

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
fn manifest_parser_rejects_corrupt_and_wrong_schema() {
    // Corrupt JSON.
    assert!(serde_json::from_str::<ChangePackManifest>("{ not json ").is_err());
    // An unowned intent is refused at decode: an identifier nobody owns is
    // exactly what lets two publishers collide.
    assert!(
        serde_json::from_value::<ChangePackManifest>(serde_json::json!({
            "schema_version": 1,
            "change_pack_id": "cpk_x",
            "manifest_digest": "sha256:manifest",
            "name": "n",
            "description": "",
            "intent": "feature",
            "provenance": {"origin": "local"},
            "author_id": "actor_a",
            "candidate_id": null,
            "declared_dependencies": [],
            "created_at": "2026-01-01T00:00:00Z"
        }))
        .is_err()
    );
    // Wrong schema version fails the support check.
    let m: ChangePackManifest = serde_json::from_value(serde_json::json!({
        "schema_version": 2,
        "change_pack_id": "cpk_x",
        "manifest_digest": "sha256:manifest",
        "name": "n",
        "description": "",
        "intent": "draft.software.project/feature",
        "provenance": {"origin": "local"},
        "author_id": "actor_a",
        "candidate_id": null,
        "declared_dependencies": [],
        "created_at": "2026-01-01T00:00:00Z"
    }))
    .unwrap();
    assert!(m.ensure_supported().is_err());
}

#[test]
fn receipt_parser_rejects_corrupt() {
    assert!(serde_json::from_str::<ReceiptEnvelope>("").is_err());
    assert!(serde_json::from_str::<ReceiptEnvelope>(r#"{"receipt_id":"rcp_x"}"#).is_err());
}

// ---- Activity Ledger parser fixtures ------------------------------------

#[test]
fn activity_parser_rejects_a_corrupt_record() {
    use draft_core::activity::LedgerRecord;
    assert!(serde_json::from_str::<LedgerRecord>("{ garbage").is_err());
}
