//! Builders and setup shared by the proofs.
//!
//! Deliberately thin: every test builds the states it is about, so the property
//! under test is visible in the test itself rather than hidden in a fixture.
//!
//! Each integration test binary compiles this module separately and uses only
//! the part it needs, which is why unused items are not a defect here.
#![allow(dead_code)]

/// Setup for the end-to-end observation and acceptance proofs.
pub mod lifecycle;

/// Two provider bindings, for behaviour a single binding cannot distinguish.
pub mod providers;

/// A non-`file` domain implemented by a real declarative command adapter.
#[cfg(unix)]
pub mod catalog;

/// A project that can actually publish: promoted, granted, and bound.
pub mod publishing;

use draft_core::dcg::observation::{
    AdapterBindingId, CoverageDomainRef, CoverageStatus, ObservationCoverage,
    ResourceCoverageMembership, SnapshotObservationMap,
};
use draft_core::dcg::resource::{RawResourceState, ResourceLocator};
use draft_core::dcg::state::Snapshot;
use draft_core::support::actor::{ActorKind, ActorRef};
use draft_core::support::common::{now, ActorId, SnapshotId};
use draft_dcg_contract::ids::ProjectId;
use draft_extension_contract::ResourceForm;
use std::collections::BTreeMap;

pub fn domain(local: &str) -> CoverageDomainRef {
    CoverageDomainRef::new(AdapterBindingId("core.filesystem".into()), local)
}

/// One observed byte resource addressed by a filesystem locator.
pub fn resource(body: &str) -> RawResourceState {
    RawResourceState {
        resource_id: draft_core::dcg::resource::resource_id_for_locator(&format!("file:{body}")),
        locator: ResourceLocator::file(body),
        form: Some(ResourceForm::Bytes),
        media_type: None,
        attributes: BTreeMap::new(),
        state_digest: format!("sha256:state-{body}"),
        content_digest: Some(format!("sha256:content-{body}")),
        metadata_digest: None,
        content_size: Some(body.len() as u64),
    }
}

/// An unsealed snapshot with one complete root domain and no resources.
pub fn unsealed_snapshot(workspace: &str) -> Snapshot {
    Snapshot {
        schema_version: 1,
        id: SnapshotId::generate(),
        workspace_id: ProjectId::parse(workspace).unwrap(),
        observation_context_digest: "sha256:context".into(),
        resources: vec![],
        observation_map: SnapshotObservationMap {
            domains: vec![ObservationCoverage {
                domain: domain("root"),
                status: CoverageStatus::Complete,
            }],
            resource_membership: vec![],
        },
        gaps: vec![],
        untrackable: vec![],
        identity_proofs: vec![],
        content_object_refs: vec![],
        created_at: now(),
        created_by: ActorRef {
            id: ActorId::new("act_test"),
            kind: ActorKind::Service,
            display_name: "test".into(),
        },
        snapshot_digest: String::new(),
    }
}

/// A sealed snapshot covering every named resource in one complete domain.
pub fn sealed_snapshot(workspace: &str, bodies: &[&str]) -> Snapshot {
    let mut snapshot = unsealed_snapshot(workspace);
    for body in bodies {
        let observed = resource(body);
        snapshot
            .observation_map
            .resource_membership
            .push(ResourceCoverageMembership {
                resource_id: observed.resource_id.clone(),
                domain: domain("root"),
            });
        snapshot.resources.push(observed);
    }
    snapshot.seal()
}
