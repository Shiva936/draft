//! Workspace scan and snapshot records.
//!
//! A snapshot is what Draft authoritatively observed at one moment: the state of
//! every resource it could establish, which parts of the observable universe it
//! covered, and what it knows it could not see. All three participate in snapshot
//! identity, because an observation that missed something is not the same
//! observation as one that did not.

use crate::dcg::observation::{ObservationGap, SnapshotObservationMap, SnapshotObservationStatus};
use crate::dcg::resource::{RawResourceState, ResourceIdentityProof, Untrackable};
use crate::support::actor::ActorRef;
use crate::support::common::SnapshotId;
use crate::support::error::DraftResult;
use crate::support::hashing;
use chrono::{DateTime, Utc};
use draft_dcg_contract::ids::ProjectId;
use serde::{Deserialize, Serialize};

/// A live scan of the workspace, before it becomes an authoritative snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStatus {
    pub workspace_id: ProjectId,
    pub root_path: String,
    pub scanned_at: DateTime<Utc>,
    pub changes: Vec<ResourceChangeSummary>,
    pub ignored_count: usize,
    pub has_draft_dir_violation: bool,
}

/// A single observed difference, for status reporting only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceChangeSummary {
    pub locator: crate::dcg::resource::ResourceLocator,
    pub aspects: Vec<crate::dcg::resource::ChangeAspect>,
    pub before_state_digest: Option<String>,
    pub after_state_digest: Option<String>,
}

/// What Draft authoritatively observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub id: SnapshotId,
    pub workspace_id: ProjectId,
    /// The effective observation semantics this snapshot was taken under.
    pub observation_context_digest: String,
    /// Successfully observed resources only. Anything that could not be
    /// established is a gap, never an omission.
    pub resources: Vec<RawResourceState>,
    /// Which coverage domains were enumerated, and which resource belongs where.
    pub observation_map: SnapshotObservationMap,
    /// What Draft knows it did not establish. Canonical and sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<ObservationGap>,
    /// Resources an adapter could see but could not describe deterministically.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub untrackable: Vec<Untrackable>,
    /// Continuity evidence. Provenance, not state: this is not a digest input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identity_proofs: Vec<ResourceIdentityProof>,
    pub content_object_refs: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub created_by: ActorRef,
    /// The canonical identity of this observation.
    pub snapshot_digest: String,
}

impl crate::contracts::VersionedContract for Snapshot {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::WorkspaceSnapshot;
}

impl Snapshot {
    /// Seal a snapshot, deriving its canonical identity.
    ///
    /// The digest covers the observed states, the observation context, the
    /// coverage map and the gap set — and nothing else. Identity proofs, run
    /// provenance and recovery anchors are all excluded on purpose: two
    /// observations of the same state that differ only in *who observed it* or
    /// *why continuity was believed* are the same authoritative state.
    pub fn seal(mut self) -> Self {
        self.resources
            .sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
        self.gaps
            .sort_by(|left, right| left.gap_id.cmp(&right.gap_id));
        self.untrackable
            .sort_by(|left, right| left.locator.cmp(&right.locator));
        self.observation_map.sort();
        self.content_object_refs.sort();
        self.content_object_refs.dedup();
        self.snapshot_digest = hashing::canonical_hash(&serde_json::json!({
            "observation_context_digest": self.observation_context_digest,
            "resources": self.resources,
            "observation_map": self.observation_map,
            "gaps": self.gaps,
        }));
        self
    }

    /// The aggregate observation status.
    ///
    /// A projection, never stored: computing it from the coverage map and gap
    /// set is the only way to obtain it, so it cannot contradict them.
    pub fn observation_status(&self) -> SnapshotObservationStatus {
        SnapshotObservationStatus::project(&self.observation_map, &self.gaps)
    }

    /// Whether this observation established everything it set out to.
    pub fn is_complete(&self) -> bool {
        self.observation_status().is_complete()
    }

    /// Validate everything about a snapshot that is checkable from itself.
    pub fn validate(&self) -> DraftResult<()> {
        for resource in &self.resources {
            crate::dcg::resource::require_state_digest(resource)?;
        }
        self.observation_map.validate(&self.gaps)?;
        Ok(())
    }

    pub fn resource(
        &self,
        resource_id: &crate::dcg::resource::ResourceId,
    ) -> Option<&RawResourceState> {
        self.resources
            .iter()
            .find(|resource| &resource.resource_id == resource_id)
    }
}

/// Snapshot builders shared by the tests of modules that consume snapshots.
///
/// Kept here rather than duplicated per module so every test observes the same
/// sealing rules the production path uses.
#[cfg(test)]
pub mod tests_support {
    use super::*;
    use crate::dcg::observation::{
        AdapterBindingId, CoverageDomainRef, CoverageStatus, ObservationCoverage,
        ResourceCoverageMembership,
    };
    use crate::dcg::resource::ResourceLocator;
    use crate::support::actor::ActorKind;
    use crate::support::common::{now, ActorId};
    use draft_extension_contract::ResourceForm;
    use std::collections::BTreeMap;

    pub fn domain(local: &str) -> CoverageDomainRef {
        CoverageDomainRef::new(AdapterBindingId("core.filesystem".into()), local)
    }

    /// One observed byte resource addressed by a filesystem locator.
    pub fn resource(body: &str) -> RawResourceState {
        RawResourceState {
            resource_id: crate::dcg::resource::resource_id_for_locator(&format!("file:{body}")),
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
    ///
    /// Callers push resources and call `seal()`; membership is filled in here so
    /// every resource added this way is covered exactly once.
    pub fn empty(workspace: &str) -> Snapshot {
        Snapshot {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::WorkspaceSnapshot,
            ),
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
    pub fn sealed(workspace: &str, bodies: &[&str]) -> Snapshot {
        let mut snapshot = empty(workspace);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcg::observation::{
        AdapterBindingId, CoverageDomainRef, CoverageStatus, ObservationCoverage,
        ObservationGapKind, ResourceCoverageMembership,
    };
    use crate::dcg::resource::{IdentityBasis, ResourceId, ResourceLocator};
    use crate::support::actor::ActorKind;
    use crate::support::common::{now, ActorId};
    use draft_extension_contract::ResourceForm;
    use std::collections::BTreeMap;

    fn domain(local: &str) -> CoverageDomainRef {
        CoverageDomainRef::new(AdapterBindingId("core.filesystem".into()), local)
    }

    fn resource(id: &str, digest: &str) -> RawResourceState {
        RawResourceState {
            resource_id: ResourceId::parse(id).unwrap(),
            locator: ResourceLocator::file(format!("{id}.txt")),
            form: Some(ResourceForm::Bytes),
            media_type: None,
            attributes: BTreeMap::new(),
            state_digest: digest.into(),
            content_digest: Some(format!("sha256:{digest}")),
            metadata_digest: None,
            content_size: Some(4),
        }
    }

    fn snapshot(resources: Vec<RawResourceState>, gaps: Vec<ObservationGap>) -> Snapshot {
        let membership = resources
            .iter()
            .map(|resource| ResourceCoverageMembership {
                resource_id: resource.resource_id.clone(),
                domain: domain("root"),
            })
            .collect();
        let mut domains = vec![ObservationCoverage {
            domain: domain("root"),
            status: CoverageStatus::Complete,
        }];
        for gap in &gaps {
            for gap_domain in &gap.coverage_domains {
                domains.push(ObservationCoverage {
                    domain: gap_domain.clone(),
                    status: CoverageStatus::Incomplete {
                        gap_ids: vec![gap.gap_id.clone()],
                    },
                });
            }
        }
        Snapshot {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::WorkspaceSnapshot,
            ),
            id: SnapshotId::generate(),
            workspace_id: ProjectId::parse("prj_1").unwrap(),
            observation_context_digest: "sha256:context".into(),
            resources,
            observation_map: SnapshotObservationMap {
                domains,
                resource_membership: membership,
            },
            gaps,
            untrackable: vec![],
            identity_proofs: vec![],
            content_object_refs: vec![],
            created_at: now(),
            created_by: ActorRef {
                kind: ActorKind::Human,
                id: ActorId::new("act_test"),
                display_name: "test".into(),
            },
            snapshot_digest: String::new(),
        }
        .seal()
    }

    #[test]
    fn snapshot_identity_covers_state_context_coverage_and_gaps() {
        let base = snapshot(vec![resource("res_1", "d1")], vec![]);
        let same = snapshot(vec![resource("res_1", "d1")], vec![]);
        assert_eq!(base.snapshot_digest, same.snapshot_digest);

        // A gap changes what the observation established, so it changes identity.
        let gap = ObservationGap::new(
            ObservationGapKind::PermissionDenied,
            "adapter.permission_denied",
            vec![domain("sub")],
            None,
            "denied",
        );
        let with_gap = snapshot(vec![resource("res_1", "d1")], vec![gap]);
        assert_ne!(base.snapshot_digest, with_gap.snapshot_digest);
        assert!(base.is_complete());
        assert!(!with_gap.is_complete());
    }

    #[test]
    fn identity_proofs_are_provenance_and_do_not_change_state_identity() {
        // The same observed state, believed continuous for two different
        // reasons, is the same authoritative state.
        let mut recorded = snapshot(vec![resource("res_1", "d1")], vec![]);
        let digest_before = recorded.snapshot_digest.clone();
        recorded.identity_proofs = vec![ResourceIdentityProof {
            resource_id: ResourceId::parse("res_1").unwrap(),
            basis: IdentityBasis::LocatorStable,
            evidence: None,
            recorded_at: now(),
        }];
        let resealed = recorded.clone().seal();
        assert_eq!(resealed.snapshot_digest, digest_before);

        let mut asserted = resealed.clone();
        asserted.identity_proofs = vec![ResourceIdentityProof {
            resource_id: ResourceId::parse("res_1").unwrap(),
            basis: IdentityBasis::AdapterAsserted {
                external_identity: "inode:42".into(),
            },
            evidence: None,
            recorded_at: now(),
        }];
        assert_eq!(asserted.seal().snapshot_digest, digest_before);
    }

    #[test]
    fn a_snapshot_validates_its_own_structure() {
        let good = snapshot(vec![resource("res_1", "d1")], vec![]);
        good.validate().unwrap();

        // A resource with no deterministic identity must never have been
        // admitted in the first place.
        let mut broken = good.clone();
        broken.resources[0].state_digest = String::new();
        assert!(broken.validate().is_err());
    }

    #[test]
    fn observation_status_cannot_be_set_independently() {
        // There is no field to set: the status is only obtainable by projecting
        // the coverage map and the gap set.
        let observed = snapshot(vec![resource("res_1", "d1")], vec![]);
        let encoded = serde_json::to_value(&observed).unwrap();
        assert!(encoded.get("observation_status").is_none());
        assert!(observed.observation_status().is_complete());
    }
}
