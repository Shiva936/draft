//! A whole domain Draft knows nothing about, reaching the end of the lifecycle.
//!
//! The adapter mechanics are proved next door. What is proved here is the claim
//! those mechanics exist to support: that a catalogue of SKUs — no paths, no
//! hierarchy, no bytes in the filesystem sense — is a first-class subject.
//! Recovery works because the adapter declared how to capture and how to
//! restore, not because Draft understood the payload; and Core, having handled
//! all of it, still cannot name a single thing in the domain.

#![cfg(unix)]

use draft_core::dcg::observation::ObservationRunId;
use draft_core::dcg::resource::ResourceLocator;
use draft_core::dcg::source::{
    AnchorRequest, MutationStep, ResourceMutationPlan, ResourceSource, ViewRules,
};

mod support;
use support::catalog::{fixture, source};

#[test]
fn a_declared_capture_yields_an_anchor_that_a_declared_restore_puts_back() {
    let fixture = fixture();
    let adapter = source(&fixture, true);

    let observed = adapter
        .describe(&ResourceLocator::new("catalog", "B-200"))
        .unwrap();
    let anchor = adapter
        .capture_anchor(
            &observed,
            &AnchorRequest {
                observation_run_id: ObservationRunId("run_1".into()),
            },
        )
        .unwrap()
        .expect("an adapter declaring AdapterManaged recovery can capture");

    // The anchor is bound to the exact state it can restore, and carries the
    // producer — unlike Core's own observer, which has none.
    assert_eq!(anchor.target_state_digest, observed.state.state_digest);
    assert_eq!(
        anchor.capture.observed_state_digest,
        observed.state.state_digest
    );
    assert!(anchor.producer.is_some());
    assert_eq!(anchor.adapter_binding_id.0, "ext.catalog");

    // Destroy the resource, then put it back from the anchor alone.
    adapter
        .mutate(&ResourceMutationPlan {
            operation_id: draft_core::support::common::OperationId::generate(),
            attribution: draft_core::execution::workspace::EditAttribution::Task {
                id: "tsk_1".into(),
            },
            preconditions: Vec::new(),
            steps: vec![MutationStep::Remove {
                locator: ResourceLocator::new("catalog", "B-200"),
                recursive: false,
            }],
        })
        .unwrap();
    assert!(adapter
        .describe(&ResourceLocator::new("catalog", "B-200"))
        .is_err());

    // A target snapshot containing exactly the resource being restored, so the
    // anchor set can be bound to it the way a real one is.
    let snapshot = draft_core::dcg::state::Snapshot {
        schema_version: draft_core::contracts::current_version(
            draft_core::contracts::ContractId::WorkspaceSnapshot,
        ),
        id: draft_core::support::common::SnapshotId::generate(),
        workspace_id: draft_dcg_contract::ids::ProjectId::parse("prj_catalog").unwrap(),
        observation_context_digest: "sha256:catalog-context".into(),
        resources: vec![observed.state.clone()],
        observation_map: draft_core::dcg::observation::SnapshotObservationMap {
            domains: vec![draft_core::dcg::observation::ObservationCoverage {
                domain: observed.coverage_domain.clone(),
                status: draft_core::dcg::observation::CoverageStatus::Complete,
            }],
            resource_membership: vec![draft_core::dcg::observation::ResourceCoverageMembership {
                resource_id: observed.state.resource_id.clone(),
                domain: observed.coverage_domain.clone(),
            }],
        },
        gaps: Vec::new(),
        untrackable: Vec::new(),
        identity_proofs: Vec::new(),
        content_object_refs: Vec::new(),
        created_at: draft_core::support::common::now(),
        created_by: draft_core::support::actor::ActorRef {
            id: draft_core::support::common::ActorId::new("act_test"),
            kind: draft_core::support::actor::ActorKind::Service,
            display_name: "test".into(),
        },
        snapshot_digest: String::new(),
    }
    .seal();
    let plan = draft_core::dcg::anchor::ResourceRestorePlan {
        schema_version: draft_core::contracts::current_version(
            draft_core::contracts::ContractId::ResourceRestorePlan,
        ),
        operation_id: draft_core::support::common::OperationId::generate(),
        target_snapshot_digest: snapshot.snapshot_digest.clone(),
        anchor_set_digest: String::new(),
        restore_targets: vec![draft_core::dcg::anchor::RestoreTarget {
            resource_id: observed.state.resource_id.clone(),
            target_locator: observed.state.locator.clone(),
            target_state_digest: observed.state.state_digest.clone(),
            anchor_digest: anchor.anchor_digest.clone(),
        }],
        absence_targets: Vec::new(),
        known_uncertainties: Vec::new(),
        planner_revision: draft_core::dcg::anchor::RECOVERY_PLANNER_REVISION,
    };
    let with_anchor =
        draft_core::dcg::anchor::RecoveryAnchorSet::build(&snapshot, vec![anchor.clone()]).unwrap();

    adapter.restore(&plan, &with_anchor).unwrap();
    let restored = adapter
        .describe(&ResourceLocator::new("catalog", "B-200"))
        .unwrap();
    // Restored to the exact state the anchor named — not merely to something
    // with the same name.
    assert_eq!(restored.state.state_digest, observed.state.state_digest);
}

#[test]
fn core_never_learns_the_adapters_vocabulary() {
    let fixture = fixture();
    let adapter = source(&fixture, true);
    let outcome = adapter.enumerate(&ViewRules::default()).unwrap();

    // Everything Core holds about these resources is scheme, opaque body,
    // intrinsic shape and a digest it computed. No SKU parsing, no region
    // hierarchy, no ancestry.
    for observed in &outcome.resources {
        let encoded = serde_json::to_value(&observed.state).unwrap();
        for domain_specific in ["region", "sku", "price", "name"] {
            assert!(
                encoded.get(domain_specific).is_none(),
                "authoritative state must not carry the adapter's own vocabulary"
            );
        }
    }
}
