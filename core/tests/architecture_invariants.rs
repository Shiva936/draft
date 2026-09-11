//! The invariants that keep Draft domain-neutral, proved rather than asserted.
//!
//! Each test here exercises a production path and checks a property the model
//! depends on. They are grouped by the confusion each one prevents, because
//! that is what makes them worth keeping: every one of these is a mistake the
//! platform could plausibly make again.

use std::collections::{BTreeMap, BTreeSet};

use draft_core::dcg::anchor::{RecoveryAnchorSet, SnapshotRecoveryStatus};
use draft_core::dcg::change_set::{derive_change_set, ChangeAspect};
use draft_core::dcg::observation::{
    AdapterBindingId, CoverageDomainRef, CoverageStatus, ObservationCoverage, ObservationGap,
    ObservationGapKind, ResourceCoverageMembership, SnapshotObservationMap,
};
use draft_core::dcg::representation::{reconcile, ClaimRelation, ConflictClaim, ConflictScope};
use draft_core::dcg::resource::ResourceLocator;
use draft_core::dcg::state::Snapshot;
use draft_core::evidence::classification::classify_snapshot;
use draft_core::evidence::verification::{aggregate, VerificationState};
use draft_core::extension::{
    ActiveContributions, Contributed, ExtensionCapabilityKind, NamespacedId, ResourceView,
};

mod support;
use support::{domain, resource, sealed_snapshot, unsealed_snapshot};

// ---------------------------------------------------------------------------
// Coverage domains are opaque and adapter-scoped
// ---------------------------------------------------------------------------

#[test]
fn coverage_domains_cannot_collide_across_adapters() {
    // Two adapters both emit a local id of "root". They are different domains,
    // and no reasoning may cross between them — which is what stops one
    // adapter's complete scan being read as proof about another's universe.
    let filesystem = CoverageDomainRef::new(AdapterBindingId("core.filesystem".into()), "root");
    let catalog = CoverageDomainRef::new(AdapterBindingId("example.catalog".into()), "root");

    assert_ne!(filesystem, catalog);
    assert_eq!(filesystem.local_id, catalog.local_id);
}

#[test]
fn a_locator_body_is_never_parsed_for_structure() {
    // Bodies whose lexical prefixes are meaningless. Nothing in the coverage or
    // change machinery may read structure into them.
    let opaque = ResourceLocator {
        scheme: "example.catalog".into(),
        body: "a/b/c".into(),
    };
    let sibling = ResourceLocator {
        scheme: "example.catalog".into(),
        body: "a/b".into(),
    };
    assert_ne!(opaque, sibling);
    assert!(!opaque.is_file(), "only the file scheme is path-shaped");

    // A path predicate does not match a non-file body that merely looks like a
    // path, which is the same rule stated from the predicate side.
    let attributes = BTreeMap::new();
    let view = ResourceView {
        locator_scheme: opaque.scheme.as_str(),
        locator_body: opaque.body.as_str(),
        media_type: None,
        form: None,
        attributes: &attributes,
        content_size: None,
    };
    assert!(!draft_core::extension::matches_raw(
        &draft_extension_contract::RawResourcePredicate::PathGlob {
            glob: "a/**".into()
        },
        &view
    ));
}

// ---------------------------------------------------------------------------
// Absence must be proved
// ---------------------------------------------------------------------------

#[test]
fn an_incomplete_observation_cannot_fake_a_removal() {
    let base = sealed_snapshot("prj_absence", &["kept.txt", "vanished.txt"]);

    // The result observed less, and knows it: the domain is incomplete.
    let mut result = unsealed_snapshot("prj_absence");
    let kept = resource("kept.txt");
    result
        .observation_map
        .resource_membership
        .push(ResourceCoverageMembership {
            resource_id: kept.resource_id.clone(),
            domain: domain("root"),
        });
    result.resources.push(kept);
    let gap = ObservationGap::new(
        ObservationGapKind::PermissionDenied,
        "permission-denied",
        vec![domain("root")],
        Some(AdapterBindingId("core.filesystem".into())),
        "the scope could not be read",
    );
    result.observation_map.domains = vec![ObservationCoverage {
        domain: domain("root"),
        status: CoverageStatus::Incomplete {
            gap_ids: vec![gap.gap_id.clone()],
        },
    }];
    result.gaps.push(gap);
    let result = result.seal();

    let change_set = derive_change_set(&base, &result).unwrap();
    assert!(
        change_set
            .resources
            .iter()
            .all(|change| !change.aspects.contains(&ChangeAspect::Removed)),
        "a resource inside an unreadable scope is uncertain, not deleted"
    );
    assert!(
        !change_set.derivation_gaps.is_empty(),
        "and the uncertainty is recorded as evidence"
    );
    assert!(!change_set.derivation_status().is_complete());
}

#[test]
fn a_known_change_survives_an_unrelated_gap() {
    // A gap somewhere else must not suppress a comparison Draft can actually
    // make: content comparison needs no coverage proof at all.
    let base = sealed_snapshot("prj_unrelated", &["a.txt"]);
    let mut result = unsealed_snapshot("prj_unrelated");
    let mut changed = resource("a.txt");
    changed.state_digest = "sha256:state-a.txt-changed".into();
    changed.content_digest = Some("sha256:content-a.txt-changed".into());
    result
        .observation_map
        .resource_membership
        .push(ResourceCoverageMembership {
            resource_id: changed.resource_id.clone(),
            domain: domain("root"),
        });
    result.resources.push(changed);
    let gap = ObservationGap::new(
        ObservationGapKind::PermissionDenied,
        "permission-denied",
        vec![domain("elsewhere")],
        Some(AdapterBindingId("core.filesystem".into())),
        "an unrelated scope could not be read",
    );
    result.observation_map.domains.push(ObservationCoverage {
        domain: domain("elsewhere"),
        status: CoverageStatus::Incomplete {
            gap_ids: vec![gap.gap_id.clone()],
        },
    });
    result.gaps.push(gap);
    let result = result.seal();

    let change_set = derive_change_set(&base, &result).unwrap();
    let change = change_set
        .resources
        .iter()
        .find(|change| {
            change.resource_id == draft_core::dcg::resource::resource_id_for_locator("file:a.txt")
        })
        .expect("the resource Draft could see still compares");
    assert!(change.aspects.contains(&ChangeAspect::ContentChanged));
}

#[test]
fn change_derivation_requires_one_observation_context() {
    let base = sealed_snapshot("prj_context", &["a.txt"]);
    let mut result = unsealed_snapshot("prj_context");
    result.observation_context_digest = "sha256:different-semantics".into();
    let result = result.seal();

    // Comparing states observed under different rules would be comparing
    // different questions.
    assert!(derive_change_set(&base, &result).is_err());
}

// ---------------------------------------------------------------------------
// Identity is independent of interpretation
// ---------------------------------------------------------------------------

#[test]
fn change_identity_is_independent_of_every_derived_layer() {
    let base = sealed_snapshot("prj_identity", &["a.txt"]);
    let mut result = unsealed_snapshot("prj_identity");
    let mut changed = resource("a.txt");
    changed.state_digest = "sha256:state-a.txt-changed".into();
    result
        .observation_map
        .resource_membership
        .push(ResourceCoverageMembership {
            resource_id: changed.resource_id.clone(),
            domain: domain("root"),
        });
    result.resources.push(changed);
    let result = result.seal();

    let bare = derive_change_set(&base, &result).unwrap();

    // Installing a classifier teaches Draft something new about unchanged
    // state. It must not change what the transition *is*.
    let contributions = ActiveContributions {
        classifications: vec![Contributed::new(
            "example.publisher",
            draft_extension_contract::ResourceClassificationRule {
                class_id: NamespacedId::parse("example.publisher/document").unwrap(),
                display_name: "Document".into(),
                applies_to: draft_extension_contract::RawResourcePredicate::PathSuffix {
                    suffix: ".txt".into(),
                },
                attributes: Vec::new(),
            },
        )],
        ..ActiveContributions::default()
    };
    let bundle = classify_snapshot(&result, &contributions);
    assert!(!bundle.assignments.is_empty(), "the classifier applies");

    let again = derive_change_set(&base, &result).unwrap();
    assert_eq!(
        bare.change_set_digest, again.change_set_digest,
        "classification is interpretation, not state"
    );
}

#[test]
fn change_identity_uses_snapshot_digests_not_record_ids() {
    let base = sealed_snapshot("prj_records", &["a.txt"]);
    let result = sealed_snapshot("prj_records", &["a.txt", "b.txt"]);
    let first = derive_change_set(&base, &result).unwrap();

    // The same authoritative states, persisted under different record ids.
    let mut renamed_base = base.clone();
    renamed_base.id = draft_core::support::common::SnapshotId::generate();
    let mut renamed_result = result.clone();
    renamed_result.id = draft_core::support::common::SnapshotId::generate();
    let second = derive_change_set(&renamed_base, &renamed_result).unwrap();

    assert_ne!(first.base_snapshot_id, second.base_snapshot_id);
    assert_eq!(
        first.change_set_digest, second.change_set_digest,
        "re-recording a transition does not make it a different transition"
    );
}

// ---------------------------------------------------------------------------
// Classification composes
// ---------------------------------------------------------------------------

#[test]
fn one_resource_carries_every_class_that_recognizes_it() {
    let rule = |extension: &str, class: &str| {
        Contributed::new(
            extension,
            draft_extension_contract::ResourceClassificationRule {
                class_id: NamespacedId::parse(class).unwrap(),
                display_name: class.into(),
                applies_to: draft_extension_contract::RawResourcePredicate::PathSuffix {
                    suffix: ".rs".into(),
                },
                attributes: Vec::new(),
            },
        )
    };
    let contributions = ActiveContributions {
        classifications: vec![
            rule("draft.text.document", "draft.text.document/document"),
            rule("draft.language.rust", "draft.language.rust/source"),
        ],
        ..ActiveContributions::default()
    };
    let snapshot = sealed_snapshot("prj_classes", &["src/main.rs"]);
    let bundle = classify_snapshot(&snapshot, &contributions);

    assert_eq!(
        bundle.classes_of(&draft_core::dcg::resource::resource_id_for_locator(
            "file:src/main.rs"
        )),
        BTreeSet::from([
            NamespacedId::parse("draft.language.rust/source").unwrap(),
            NamespacedId::parse("draft.text.document/document").unwrap(),
        ]),
        "two correct classifiers are two facts, not a conflict"
    );
    assert!(bundle.collisions.is_empty());
}

// ---------------------------------------------------------------------------
// The conflict algebra fails closed
// ---------------------------------------------------------------------------

#[test]
fn the_conflict_matrix_holds_in_every_row() {
    let whole = vec![ConflictClaim {
        id: "whole".into(),
        scope: ConflictScope::Whole,
    }];
    let line = |start: u64, length: u64| {
        vec![ConflictClaim {
            id: format!("line:{start}"),
            scope: ConflictScope::LinearRegion {
                coordinate_space: "draft.text.document/line".into(),
                start,
                length,
            },
        }]
    };
    let frame = vec![ConflictClaim {
        id: "frame:0".into(),
        scope: ConflictScope::LinearRegion {
            coordinate_space: "example/frame".into(),
            start: 0,
            length: 1,
        },
    }];
    let key = |space: &str, k: &str| {
        vec![ConflictClaim {
            id: k.into(),
            scope: ConflictScope::OpaqueKey {
                key_space: space.into(),
                key: k.into(),
            },
        }]
    };

    // A whole-resource claim admits no neighbour.
    assert!(!reconcile(&whole, &line(0, 1)).is_composable());
    // Same space, disjoint regions: independent.
    assert_eq!(
        reconcile(&line(0, 1), &line(5, 1)),
        ClaimRelation::Independent
    );
    // Same space, overlapping regions: conflicting.
    assert!(matches!(
        reconcile(&line(0, 3), &line(1, 3)),
        ClaimRelation::Conflicting { .. }
    ));
    // Two coordinate systems Core cannot relate: indeterminate, not independent.
    assert!(matches!(
        reconcile(&line(0, 1), &frame),
        ClaimRelation::Indeterminate { .. }
    ));
    // Distinct keys in one space are distinct things.
    assert_eq!(
        reconcile(&key("example/node", "hull"), &key("example/node", "keel")),
        ClaimRelation::Independent
    );
    // The same key in one space is the same thing.
    assert!(matches!(
        reconcile(&key("example/node", "hull"), &key("example/node", "hull")),
        ClaimRelation::Conflicting { .. }
    ));
    // Key spaces are not comparable.
    assert!(matches!(
        reconcile(&key("example/node", "hull"), &key("other/part", "hull")),
        ClaimRelation::Indeterminate { .. }
    ));
    // Incomparable claim shapes.
    assert!(matches!(
        reconcile(&line(0, 1), &key("example/node", "hull")),
        ClaimRelation::Indeterminate { .. }
    ));
    // Claims on one side and silence on the other: the silent side may have
    // touched anything.
    assert!(matches!(
        reconcile(&line(0, 1), &[]),
        ClaimRelation::Indeterminate { .. }
    ));
}

// ---------------------------------------------------------------------------
// Missing capability is its own state
// ---------------------------------------------------------------------------

#[test]
fn no_checks_can_never_mean_passed() {
    assert!(matches!(
        aggregate(&[]),
        VerificationState::NotApplicable { .. }
    ));
    assert!(!aggregate(&[]).satisfies_gate_unconditionally());
}

#[test]
fn a_capability_gap_never_names_a_package() {
    let gap = draft_core::extension::CapabilityGap::new(
        ExtensionCapabilityKind::Verification,
        vec!["src/main.rs".to_string()],
        "no installed extension contributes a check for these resources",
    );
    let encoded = serde_json::to_value(&gap).unwrap();
    for forbidden in [
        "extension_id",
        "source_id",
        "package",
        "catalog_id",
        "install",
    ] {
        assert!(
            encoded.get(forbidden).is_none(),
            "a gap must not advertise on a publisher's behalf: {forbidden}"
        );
    }
}

// ---------------------------------------------------------------------------
// Observation completeness is not recovery readiness
// ---------------------------------------------------------------------------

#[test]
fn a_complete_observation_is_not_a_restorable_one() {
    let snapshot = sealed_snapshot("prj_recovery", &["a.txt", "b.txt"]);
    assert!(
        snapshot.is_complete(),
        "the observation established everything"
    );

    let anchors = RecoveryAnchorSet::build(&snapshot, vec![]).unwrap();
    assert_eq!(
        anchors.status(&snapshot),
        SnapshotRecoveryStatus::NotAnchored,
        "and nothing about it can be put back — the two are separate dimensions"
    );
}

// ---------------------------------------------------------------------------
// The control plane is unreachable
// ---------------------------------------------------------------------------

#[test]
fn the_draft_control_plane_can_never_become_project_state() {
    // Structural, not conventional: the path guard refuses regardless of any
    // contributed view rule, and it refuses every spelling.
    for candidate in [".draft", ".draft/config.toml", "nested/.draft/x", ".draft/"] {
        assert!(
            draft_core::support::pathguard::is_draft_path(candidate),
            "{candidate} must be recognized as control plane"
        );
    }
}

#[test]
fn an_observation_map_must_be_structurally_consistent() {
    // A membership pointing at a domain the map does not declare would let
    // later reasoning draw a conclusion the observation never supported.
    let mut snapshot: Snapshot = unsealed_snapshot("prj_integrity");
    let observed = resource("a.txt");
    snapshot
        .observation_map
        .resource_membership
        .push(ResourceCoverageMembership {
            resource_id: observed.resource_id.clone(),
            domain: domain("undeclared"),
        });
    snapshot.resources.push(observed);
    let snapshot = snapshot.seal();
    assert!(snapshot.validate().is_err());

    // A domain claiming completeness while naming a gap is likewise refused.
    let mut contradictory = SnapshotObservationMap {
        domains: vec![ObservationCoverage {
            domain: domain("root"),
            status: CoverageStatus::Incomplete { gap_ids: vec![] },
        }],
        resource_membership: vec![],
    };
    contradictory.sort();
    assert!(contradictory.validate(&[]).is_err());
}
