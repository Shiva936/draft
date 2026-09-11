//! A DraftPack carries an accepted Baseline; it does not carry acceptance.
//!
//! The distinction is the whole point of the format. Export writes what this
//! project already decided. Import reads bytes from anywhere, so it proves the
//! archive matches the manifest the exporter signed — and then stops. The
//! recipient's own Promotion is the only thing that can make an imported
//! Baseline authoritative here, and nothing inside an archive can substitute
//! for it: a sender that could would be vouching for itself.

mod support;

use support::publishing::PublishingProject;

/// The whole round trip, including what the recipient deliberately did not get.
#[test]
fn a_pack_verifies_on_arrival_and_still_grants_nothing() {
    let sender = PublishingProject::new();
    let artifact = sender.root.join("out.draftpack");
    let exported = sender
        .app
        .export_baseline(&sender.root, None, Some(&artifact))
        .unwrap();

    // The Baseline's manifest, acceptance record, composition and lineage,
    // plus the signed envelope. The composition travels because the evidence
    // root is a digest: it proves the entries did not change and cannot say
    // what they were.
    assert_eq!(exported.members, 5);
    assert!(artifact.is_file());

    let recipient = PublishingProject::new();
    let before = draft_core::dcg::baseline::current_baseline(&recipient.workspace.layout)
        .unwrap()
        .unwrap();

    let report = recipient
        .app
        .import_baseline(&recipient.root, &artifact, false)
        .unwrap();

    assert_eq!(report.baseline, exported.baseline);
    assert_eq!(report.manifest_digest, exported.manifest_digest);
    // Level 2 of three: these bytes are what the holder of that key sent.
    assert!(report.signature_verified);
    // Level 3 is the recipient's own question, and the answer is no.
    assert!(!report.locally_accepted);

    let quarantine = report.quarantine.expect("a real import quarantines");
    assert!(quarantine.join("baseline/manifest.json").is_file());

    // Nothing about the recipient's authority moved.
    let after = draft_core::dcg::baseline::current_baseline(&recipient.workspace.layout)
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
    assert_ne!(after.to_string(), exported.baseline);
}

/// A member changed in transit is a manifest mismatch, not a smaller pack.
///
/// Per-entry digests are what make this a *named* failure: a single archive
/// digest would prove only that something differs.
#[test]
fn a_rewritten_member_is_refused() {
    let sender = PublishingProject::new();
    let artifact = sender.root.join("out.draftpack");
    sender
        .app
        .export_baseline(&sender.root, None, Some(&artifact))
        .unwrap();

    let archive = draft_core::draftpack::read_archive(&artifact).unwrap();
    let tampered: Vec<(String, Vec<u8>)> = archive
        .entries
        .into_iter()
        .map(|(name, bytes)| {
            if name == "baseline/composition.json" {
                (name, b"{}\n".to_vec())
            } else {
                (name, bytes)
            }
        })
        .collect();
    draft_core::draftpack::write_archive(&artifact, &tampered).unwrap();

    let recipient = PublishingProject::new();
    let error = recipient
        .app
        .import_baseline(&recipient.root, &artifact, false)
        .unwrap_err();
    assert!(
        error.message.contains("signed manifest"),
        "{}",
        error.message
    );
    assert!(
        std::fs::read_dir(recipient.workspace.layout.quarantine_dir())
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true)
    );
}

/// A dry run answers the question without writing the answer down.
#[test]
fn a_dry_run_quarantines_nothing() {
    let sender = PublishingProject::new();
    let artifact = sender.root.join("out.draftpack");
    sender
        .app
        .export_baseline(&sender.root, None, Some(&artifact))
        .unwrap();

    let recipient = PublishingProject::new();
    let report = recipient
        .app
        .import_baseline(&recipient.root, &artifact, true)
        .unwrap();
    assert!(report.signature_verified);
    assert!(report.quarantine.is_none());
    assert!(
        std::fs::read_dir(recipient.workspace.layout.quarantine_dir())
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true)
    );
}
