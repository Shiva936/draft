//! Frozen v1 DraftPack format vectors.
//!
//! These assert the *bytes*, not merely the behaviour. A refactor that changed
//! a canonical encoding, a domain separator or a limit would keep every
//! behavioural test passing while silently making every previously exported
//! pack unverifiable — so the digests below are written out in full and
//! compared literally.
//!
//! If one of these fails, the question is never "what should the new value be".
//! It is whether the format was meant to change at all, which for v1 it is not.

use std::collections::{BTreeMap, BTreeSet};

use draft_dcg_contract::identifier::NamespacedId;
use draft_dcg_contract::ids::{ActorId, ProjectId};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::{BaselineId, Digest, ProducerIdentity};
use draft_draftpack_contract::*;

fn path(value: &str) -> SafeEntryPath {
    SafeEntryPath::parse(value).unwrap()
}

fn manifest() -> DraftpackManifest {
    DraftpackManifest {
        format_revision: DRAFTPACK_FORMAT_REVISION,
        project: ProjectId::parse("prj_000000000001").unwrap(),
        baseline: BaselineId::new(Digest::of_bytes(b"frozen-baseline")),
        entries: BTreeSet::from([
            ArchiveEntryMetadata::describe(path("baseline/manifest.json"), b"{}").unwrap(),
            ArchiveEntryMetadata::describe(path("objects/blake3/aa"), b"payload").unwrap(),
        ]),
        receipts: Vec::new(),
        exported_by: ActorId::parse("act_000000000001").unwrap(),
        exported_at: Timestamp::from_unix_nanos(1_700_000_000_000_000_000),
        producer: ProducerIdentity::new(
            NamespacedId::parse("draft.core/draftpack").unwrap(),
            "0.3.4",
        )
        .unwrap(),
    }
}

#[test]
fn the_format_revision_and_media_type_are_frozen() {
    assert_eq!(DRAFTPACK_FORMAT_REVISION, 1);
    assert_eq!(DRAFTPACK_MEDIA_TYPE, "application/vnd.draft.draftpack");
    assert_eq!(MANIFEST_ENTRY_PATH, "draftpack.json");
}

#[test]
fn the_signature_domain_is_frozen_and_distinct_from_a_receipts() {
    assert_eq!(DRAFTPACK_SIGNATURE_DOMAIN, "draft.draftpack.signature/v1");
    assert_ne!(
        DRAFTPACK_SIGNATURE_DOMAIN,
        draft_dcg_contract::RECEIPT_SIGNATURE_DOMAIN
    );
}

#[test]
fn the_format_limits_are_frozen() {
    // Raising any of these would accept archives every other implementation
    // refuses, so they are part of the format rather than configuration.
    assert_eq!(MAX_ENTRY_BYTES, 100 * 1024 * 1024);
    assert_eq!(MAX_TOTAL_BYTES, 512 * 1024 * 1024);
    assert_eq!(MAX_ENTRIES, 20_000);
    assert_eq!(MAX_ENTRY_PATH_LENGTH, 1024);
}

#[test]
fn the_manifest_canonical_form_is_frozen() {
    let encoded = serde_json::to_string(&manifest()).unwrap();
    assert_eq!(
        encoded,
        concat!(
            r#"{"format_revision":1,"project":"prj_000000000001","#,
            r#""baseline":"sha256:b96aab73cfef12d8c0fe3292a21934816c23b8d3863e253ba62ea3b621e4abb8","#,
            r#""entries":[{"path":"baseline/manifest.json","size":2,"#,
            r#""digest":"sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"},"#,
            r#"{"path":"objects/blake3/aa","size":7,"#,
            r#""digest":"sha256:239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5"}],"#,
            r#""exported_by":"act_000000000001","exported_at":1700000000000000000,"#,
            r#""producer":{"producer":"draft.core/draftpack","version":"0.3.4"}}"#
        )
    );
}

#[test]
fn entries_are_ordered_by_path_regardless_of_insertion() {
    // Canonical ordering is what makes the manifest digest reproducible across
    // implementations that build the entry set in different orders.
    let encoded = serde_json::to_string(&manifest()).unwrap();
    let first = encoded.find("baseline/manifest.json").unwrap();
    let second = encoded.find("objects/blake3/aa").unwrap();
    assert!(first < second, "entries must be sorted by path");
}

#[test]
fn a_manifest_round_trips_through_its_wire_form() {
    let original = manifest();
    let encoded = serde_json::to_string(&original).unwrap();
    let decoded: DraftpackManifest = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, original);
    assert_eq!(decoded.digest().unwrap(), original.digest().unwrap());
}

#[test]
fn verification_distinguishes_the_three_ways_an_archive_can_be_wrong() {
    let manifest = manifest();
    let good: BTreeMap<SafeEntryPath, Vec<u8>> = BTreeMap::from([
        (path("baseline/manifest.json"), b"{}".to_vec()),
        (path("objects/blake3/aa"), b"payload".to_vec()),
    ]);
    assert!(manifest.verify(&good).is_complete());

    let mut altered = good.clone();
    altered.insert(path("objects/blake3/aa"), b"altered".to_vec());
    assert_eq!(manifest.verify(&altered).corrupted.len(), 1);

    let mut truncated = good.clone();
    truncated.remove(&path("objects/blake3/aa"));
    assert_eq!(manifest.verify(&truncated).missing.len(), 1);

    let mut smuggled = good;
    smuggled.insert(path("extra"), b"x".to_vec());
    assert_eq!(manifest.verify(&smuggled).unexpected.len(), 1);
}

#[test]
fn the_safe_path_grammar_is_frozen() {
    for accepted in ["draftpack.json", "objects/blake3/aa", "..foo", ".hidden"] {
        assert!(SafeEntryPath::parse(accepted).is_ok(), "{accepted}");
    }
    for refused in [
        "../escape",
        "/absolute",
        "C:/drive",
        ".draft/control.json",
        "back\\slash",
        "a//b",
        "a/./b",
        "",
    ] {
        assert!(SafeEntryPath::parse(refused).is_err(), "{refused}");
    }
}
