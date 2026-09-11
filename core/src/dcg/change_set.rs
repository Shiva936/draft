//! The authoritative state transition, and the neutral algebra over it.
//!
//! A change set says what changed between two authoritative observations. Its
//! canonical identity is computed from the two snapshot **digests**, the shared
//! observation context, the proved resource changes and the derivation gaps —
//! and from nothing else. Classification, representations, evidence, identity
//! proofs, run provenance, recovery anchors and acceptance policy are all
//! excluded, so installing an extension that explains a change better can never
//! alter what the change *is*.
//!
//! Two rules do most of the work here:
//!
//! * **Absence must be proved.** A resource missing from an observation that did
//!   not cover its domain is unknown, not deleted. Uncertainty becomes an
//!   explicit derivation gap and never a `Removed`.
//! * **Comparison needs one context.** Two snapshots taken under different
//!   observation semantics are not comparable, because the difference between
//!   them is partly a difference in what Draft could see.

use crate::dcg::observation::{CoverageDomainRef, ObservationGapId};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing;
// Re-exported so the change vocabulary reads where it is used, while the type
// itself lives beside the resource states it describes.
pub use crate::dcg::resource::ChangeAspect;

use crate::dcg::resource::{ResourceId, ResourceLocator};
use crate::dcg::state::Snapshot;
use draft_extension_contract::{AttributeValue, ResourceForm};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The Core semantics that derive a change set from two snapshots.
///
/// Authoritative, and deliberately separate from every derived-layer revision:
/// a change to how representations are hashed must never perturb what a state
/// transition *is*.
pub const CHANGE_DERIVATION_REVISION: u32 = 1;

/// One side of a transition: the authoritative state as observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceStateSummary {
    pub locator: ResourceLocator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<ResourceForm>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    pub state_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_size: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, AttributeValue>,
}

impl From<&crate::dcg::resource::RawResourceState> for ResourceStateSummary {
    fn from(state: &crate::dcg::resource::RawResourceState) -> Self {
        Self {
            locator: state.locator.clone(),
            form: state.form,
            media_type: state.media_type.clone(),
            state_digest: state.state_digest.clone(),
            content_digest: state.content_digest.clone(),
            metadata_digest: state.metadata_digest.clone(),
            content_size: state.content_size,
            attributes: state.attributes.clone(),
        }
    }
}

/// One resource's transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceChange {
    pub resource_id: ResourceId,
    /// `None` when the resource did not exist in the base — and the base
    /// authoritatively covered where it would have been.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ResourceStateSummary>,
    /// `None` when the resource is absent from the result — and the result
    /// authoritatively covered where it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ResourceStateSummary>,
    pub aspects: BTreeSet<ChangeAspect>,
}

/// Why Draft could not establish whether something changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeDerivationGapKind {
    /// Present in the result, but the base did not cover where it would have
    /// been, so Draft cannot say it is new.
    PresenceUncertain,
    /// Present in the base, absent from the result, but the result did not cover
    /// where it was, so Draft cannot say it is gone.
    AbsenceUncertain,
}

/// Which snapshot lacked the coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivationSide {
    Base,
    Result,
}

/// A change Draft could not establish. Evidence, never a `ResourceChange`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeDerivationGap {
    pub kind: ChangeDerivationGapKind,
    pub resource_id: ResourceId,
    /// Carried for display only. Never parsed, and never treated as a path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<ResourceLocator>,
    pub uncovered_side: DerivationSide,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_domain: Option<CoverageDomainRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observation_gap_ids: Vec<ObservationGapId>,
}

/// A change set's aggregate derivation status.
///
/// A projection of the gap set, like the snapshot's observation status: there is
/// no field to set independently, so it cannot contradict the gaps it summarises.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ChangeSetDerivationStatus {
    Complete,
    Incomplete { gap_count: usize },
}

impl ChangeSetDerivationStatus {
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }
}

/// The authoritative state transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeSet {
    pub schema_version: u32,
    pub id: ChangeSetId,
    /// Persisted references, for navigation. Convenient, not canonical.
    pub base_snapshot_id: crate::support::common::SnapshotId,
    pub result_snapshot_id: crate::support::common::SnapshotId,
    /// Canonical authoritative subject identity.
    pub base_snapshot_digest: String,
    pub result_snapshot_digest: String,
    /// Shared by both snapshots; comparison across contexts is refused.
    pub observation_context_digest: String,
    pub resources: Vec<ResourceChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derivation_gaps: Vec<ChangeDerivationGap>,
    pub change_derivation_revision: u32,
    pub change_set_digest: String,
}

/// Identity of one change set.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChangeSetId(pub String);

impl std::fmt::Display for ChangeSetId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl ChangeSetId {
    pub fn generate() -> Self {
        Self(format!(
            "chg_{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ))
    }
}

impl crate::contracts::VersionedContract for ChangeSet {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::ChangeSet;
}

impl ChangeSet {
    /// The aggregate derivation status.
    pub fn derivation_status(&self) -> ChangeSetDerivationStatus {
        if self.derivation_gaps.is_empty() {
            ChangeSetDerivationStatus::Complete
        } else {
            ChangeSetDerivationStatus::Incomplete {
                gap_count: self.derivation_gaps.len(),
            }
        }
    }

    /// Validate the change set against the snapshots it claims to describe.
    ///
    /// Record ids are navigation aids, so a mismatch between an id and the
    /// digest stored alongside it means the transition no longer describes what
    /// it says it does.
    pub fn validate_against(&self, base: &Snapshot, result: &Snapshot) -> DraftResult<()> {
        let corrupt = |detail: String| DraftError::new(DraftErrorKind::CorruptData, detail);
        if base.id != self.base_snapshot_id || result.id != self.result_snapshot_id {
            return Err(corrupt(
                "change set references different snapshot records than the ones supplied".into(),
            ));
        }
        if base.snapshot_digest != self.base_snapshot_digest {
            return Err(corrupt(format!(
                "base snapshot {} has digest {} but the change set records {}",
                base.id, base.snapshot_digest, self.base_snapshot_digest
            )));
        }
        if result.snapshot_digest != self.result_snapshot_digest {
            return Err(corrupt(format!(
                "result snapshot {} has digest {} but the change set records {}",
                result.id, result.snapshot_digest, self.result_snapshot_digest
            )));
        }
        if self.change_set_digest != self.compute_digest() {
            return Err(corrupt(
                "change set digest does not match its authoritative content".into(),
            ));
        }
        Ok(())
    }

    /// Seal this change set, deriving its canonical identity.
    ///
    /// The only way to produce a valid `change_set_digest`. Hashing the struct
    /// as a whole would be wrong: `ChangeSetId`, `SnapshotId` and
    /// `schema_version` are persistence bookkeeping, and folding them in would
    /// make the same transition, re-recorded, a different transition.
    pub fn seal(mut self) -> Self {
        self.resources
            .sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
        self.derivation_gaps.sort();
        self.derivation_gaps.dedup();
        self.change_set_digest = self.compute_digest();
        self
    }

    /// The canonical identity of this transition.
    pub fn compute_digest(&self) -> String {
        hashing::canonical_hash(&serde_json::json!({
            "base_snapshot_digest": self.base_snapshot_digest,
            "result_snapshot_digest": self.result_snapshot_digest,
            "observation_context_digest": self.observation_context_digest,
            "resources": self.resources,
            "derivation_gaps": self.derivation_gaps,
            "change_derivation_revision": self.change_derivation_revision,
        }))
    }
}

/// Refuse to compare two snapshots taken under different observation semantics.
fn observation_context_mismatch(base: &Snapshot, result: &Snapshot) -> DraftError {
    DraftError::new(
        DraftErrorKind::ConflictDetected,
        format!(
            "cannot derive a change set across observation contexts: the base snapshot was \
             observed under {} and the result under {}. Differences between them are partly \
             differences in what Draft could see, not changes to the project; adopt the new \
             observation context to establish a new baseline instead",
            base.observation_context_digest, result.observation_context_digest
        ),
    )
}

/// Derive the authoritative transition between two observations.
///
/// Consults no classification and no comparison capability: what changed is a
/// question about observed state, and the answer must not depend on which
/// extensions happen to be installed.
pub fn derive_change_set(base: &Snapshot, result: &Snapshot) -> DraftResult<ChangeSet> {
    if base.observation_context_digest != result.observation_context_digest {
        return Err(observation_context_mismatch(base, result));
    }

    let base_by_id: BTreeMap<&ResourceId, &crate::dcg::resource::RawResourceState> = base
        .resources
        .iter()
        .map(|resource| (&resource.resource_id, resource))
        .collect();
    let result_by_id: BTreeMap<&ResourceId, &crate::dcg::resource::RawResourceState> = result
        .resources
        .iter()
        .map(|resource| (&resource.resource_id, resource))
        .collect();

    let mut resources = Vec::new();
    let mut derivation_gaps = Vec::new();

    let every_id: BTreeSet<&ResourceId> = base_by_id
        .keys()
        .chain(result_by_id.keys())
        .copied()
        .collect();
    for resource_id in every_id {
        match (base_by_id.get(resource_id), result_by_id.get(resource_id)) {
            // Known on both sides: compare directly. Unrelated coverage gaps
            // elsewhere are irrelevant — this resource's state is established.
            (Some(before), Some(after)) => {
                let aspects = derive_aspects(before, after);
                if !aspects.is_empty() {
                    resources.push(ResourceChange {
                        resource_id: resource_id.clone(),
                        before: Some((*before).into()),
                        after: Some((*after).into()),
                        aspects,
                    });
                }
            }
            // Present in the base, absent from the result. Only a *complete*
            // result-side domain proves it is gone.
            (Some(before), None) => {
                let domain = base.observation_map.domain_of(resource_id);
                match domain.filter(|domain| result.observation_map.proves_absence_in(domain)) {
                    Some(_) => resources.push(ResourceChange {
                        resource_id: resource_id.clone(),
                        before: Some((*before).into()),
                        after: None,
                        aspects: BTreeSet::from([ChangeAspect::Removed]),
                    }),
                    None => derivation_gaps.push(ChangeDerivationGap {
                        kind: ChangeDerivationGapKind::AbsenceUncertain,
                        resource_id: resource_id.clone(),
                        locator: Some(before.locator.clone()),
                        uncovered_side: DerivationSide::Result,
                        coverage_domain: domain.cloned(),
                        observation_gap_ids: gap_ids_for(result, domain),
                    }),
                }
            }
            // Absent from the base, present in the result. Only a *complete*
            // base-side domain proves it is new.
            (None, Some(after)) => {
                let domain = result.observation_map.domain_of(resource_id);
                match domain.filter(|domain| base.observation_map.proves_absence_in(domain)) {
                    Some(_) => resources.push(ResourceChange {
                        resource_id: resource_id.clone(),
                        before: None,
                        after: Some((*after).into()),
                        aspects: BTreeSet::from([ChangeAspect::Added]),
                    }),
                    None => derivation_gaps.push(ChangeDerivationGap {
                        kind: ChangeDerivationGapKind::PresenceUncertain,
                        resource_id: resource_id.clone(),
                        locator: Some(after.locator.clone()),
                        uncovered_side: DerivationSide::Base,
                        coverage_domain: domain.cloned(),
                        observation_gap_ids: gap_ids_for(base, domain),
                    }),
                }
            }
            (None, None) => unreachable!("id came from one of the two maps"),
        }
    }

    resources.sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
    derivation_gaps.sort_by(|left, right| {
        (&left.resource_id, left.kind).cmp(&(&right.resource_id, right.kind))
    });

    Ok(ChangeSet {
        schema_version: crate::contracts::current_version(crate::contracts::ContractId::ChangeSet),
        id: ChangeSetId::generate(),
        base_snapshot_id: base.id.clone(),
        result_snapshot_id: result.id.clone(),
        base_snapshot_digest: base.snapshot_digest.clone(),
        result_snapshot_digest: result.snapshot_digest.clone(),
        observation_context_digest: base.observation_context_digest.clone(),
        resources,
        derivation_gaps,
        change_derivation_revision: CHANGE_DERIVATION_REVISION,
        change_set_digest: String::new(),
    }
    .seal())
}

/// The gaps a snapshot recorded over one domain, for attribution.
fn gap_ids_for(snapshot: &Snapshot, domain: Option<&CoverageDomainRef>) -> Vec<ObservationGapId> {
    let Some(domain) = domain else {
        return Vec::new();
    };
    snapshot
        .gaps
        .iter()
        .filter(|gap| gap.coverage_domains.contains(domain))
        .map(|gap| gap.gap_id.clone())
        .collect()
}

/// Every way in which two observed states of one resource differ.
fn derive_aspects(
    before: &crate::dcg::resource::RawResourceState,
    after: &crate::dcg::resource::RawResourceState,
) -> BTreeSet<ChangeAspect> {
    let mut aspects = BTreeSet::new();
    if before.state_digest == after.state_digest {
        return aspects;
    }
    if before.locator != after.locator {
        aspects.insert(ChangeAspect::Relocated);
    }
    if before.content_digest != after.content_digest || before.content_size != after.content_size {
        aspects.insert(ChangeAspect::ContentChanged);
    }
    if before.metadata_digest != after.metadata_digest {
        aspects.insert(ChangeAspect::MetadataChanged);
    }
    if before.form != after.form {
        aspects.insert(ChangeAspect::FormChanged);
    }
    if before.attributes != after.attributes {
        aspects.insert(ChangeAspect::AttributesChanged);
    }
    // The digests differ, so something did. If nothing more specific accounts
    // for it, say so rather than reporting an empty change.
    if aspects.is_empty() {
        aspects.insert(ChangeAspect::MetadataChanged);
    }
    aspects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcg::observation::{
        AdapterBindingId, CoverageStatus, ObservationCoverage, ObservationGap, ObservationGapKind,
        ResourceCoverageMembership, SnapshotObservationMap,
    };
    use crate::dcg::resource::RawResourceState;
    use crate::support::actor::{ActorKind, ActorRef};
    use crate::support::common::ActorId;
    use crate::support::common::{now, SnapshotId};
    use draft_dcg_contract::ids::ProjectId;

    fn domain(local: &str) -> CoverageDomainRef {
        CoverageDomainRef::new(AdapterBindingId("core.filesystem".into()), local)
    }

    fn resource(id: &str, body: &str, content: &str) -> RawResourceState {
        RawResourceState {
            resource_id: ResourceId::parse(id).unwrap(),
            locator: ResourceLocator::file(body),
            form: Some(ResourceForm::Bytes),
            media_type: None,
            attributes: BTreeMap::new(),
            state_digest: format!("state:{body}:{content}"),
            content_digest: Some(format!("sha256:{content}")),
            metadata_digest: None,
            content_size: Some(content.len() as u64),
        }
    }

    /// Build a snapshot where each resource sits in a named domain, and each
    /// listed domain has the given coverage.
    fn snapshot(
        context: &str,
        placed: &[(&str, RawResourceState)],
        coverage: &[(&str, CoverageStatus)],
        gaps: Vec<ObservationGap>,
    ) -> Snapshot {
        Snapshot {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ChangeSet,
            ),
            id: SnapshotId::generate(),
            workspace_id: ProjectId::parse("prj_1").unwrap(),
            observation_context_digest: context.into(),
            resources: placed.iter().map(|(_, state)| state.clone()).collect(),
            observation_map: SnapshotObservationMap {
                domains: coverage
                    .iter()
                    .map(|(local, status)| ObservationCoverage {
                        domain: domain(local),
                        status: status.clone(),
                    })
                    .collect(),
                resource_membership: placed
                    .iter()
                    .map(|(local, state)| ResourceCoverageMembership {
                        resource_id: state.resource_id.clone(),
                        domain: domain(local),
                    })
                    .collect(),
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

    fn complete(local: &str) -> (&str, CoverageStatus) {
        (local, CoverageStatus::Complete)
    }

    #[test]
    fn a_content_change_is_derived_from_state_alone() {
        let base = snapshot(
            "ctx",
            &[("root", resource("res_1", "a.txt", "one"))],
            &[complete("root")],
            vec![],
        );
        let result = snapshot(
            "ctx",
            &[("root", resource("res_1", "a.txt", "two"))],
            &[complete("root")],
            vec![],
        );
        let change = derive_change_set(&base, &result).unwrap();
        assert_eq!(change.resources.len(), 1);
        assert!(change.resources[0]
            .aspects
            .contains(&ChangeAspect::ContentChanged));
        assert!(change.derivation_status().is_complete());
        change.validate_against(&base, &result).unwrap();
    }

    #[test]
    fn a_single_transition_can_carry_several_aspects() {
        // Relocated and edited at once. Collapsing this to one verdict would
        // hide half of what a reviewer needs to see.
        let mut moved = resource("res_1", "b.txt", "two");
        moved.form = Some(ResourceForm::Reference);
        let base = snapshot(
            "ctx",
            &[("root", resource("res_1", "a.txt", "one"))],
            &[complete("root")],
            vec![],
        );
        let result = snapshot("ctx", &[("root", moved)], &[complete("root")], vec![]);
        let change = derive_change_set(&base, &result).unwrap();
        let aspects = &change.resources[0].aspects;
        assert!(aspects.contains(&ChangeAspect::Relocated));
        assert!(aspects.contains(&ChangeAspect::ContentChanged));
        assert!(aspects.contains(&ChangeAspect::FormChanged));
    }

    #[test]
    fn an_incomplete_result_domain_cannot_fake_a_removal() {
        let gap = ObservationGap::new(
            ObservationGapKind::PermissionDenied,
            "adapter.permission_denied",
            vec![domain("sub")],
            None,
            "denied",
        );
        let base = snapshot(
            "ctx",
            &[("sub", resource("res_1", "sub/a.txt", "one"))],
            &[complete("root"), ("sub", CoverageStatus::Complete)],
            vec![],
        );
        let result = snapshot(
            "ctx",
            &[],
            &[
                complete("root"),
                (
                    "sub",
                    CoverageStatus::Incomplete {
                        gap_ids: vec![gap.gap_id.clone()],
                    },
                ),
            ],
            vec![gap.clone()],
        );
        let change = derive_change_set(&base, &result).unwrap();
        assert!(
            change.resources.is_empty(),
            "an unreadable scope must not be reported as a deletion"
        );
        assert_eq!(change.derivation_gaps.len(), 1);
        assert_eq!(
            change.derivation_gaps[0].kind,
            ChangeDerivationGapKind::AbsenceUncertain
        );
        assert_eq!(
            change.derivation_gaps[0].observation_gap_ids,
            vec![gap.gap_id]
        );
        assert!(!change.derivation_status().is_complete());
    }

    #[test]
    fn an_incomplete_base_domain_cannot_fake_an_addition() {
        let gap = ObservationGap::new(
            ObservationGapKind::PermissionDenied,
            "adapter.permission_denied",
            vec![domain("sub")],
            None,
            "denied",
        );
        let base = snapshot(
            "ctx",
            &[],
            &[
                complete("root"),
                (
                    "sub",
                    CoverageStatus::Incomplete {
                        gap_ids: vec![gap.gap_id.clone()],
                    },
                ),
            ],
            vec![gap],
        );
        let result = snapshot(
            "ctx",
            &[("sub", resource("res_1", "sub/a.txt", "one"))],
            &[complete("root"), ("sub", CoverageStatus::Complete)],
            vec![],
        );
        let change = derive_change_set(&base, &result).unwrap();
        assert!(change.resources.is_empty());
        assert_eq!(
            change.derivation_gaps[0].kind,
            ChangeDerivationGapKind::PresenceUncertain
        );
    }

    #[test]
    fn absence_proved_by_complete_coverage_is_a_removal() {
        let base = snapshot(
            "ctx",
            &[("root", resource("res_1", "a.txt", "one"))],
            &[complete("root")],
            vec![],
        );
        let result = snapshot("ctx", &[], &[complete("root")], vec![]);
        let change = derive_change_set(&base, &result).unwrap();
        assert_eq!(change.resources.len(), 1);
        assert!(change.resources[0].aspects.contains(&ChangeAspect::Removed));
        assert!(change.derivation_gaps.is_empty());
    }

    #[test]
    fn an_unrelated_incomplete_domain_does_not_suppress_a_known_change() {
        let gap = ObservationGap::new(
            ObservationGapKind::PermissionDenied,
            "adapter.permission_denied",
            vec![domain("sub")],
            None,
            "denied",
        );
        let coverage = |gap_id: Option<&ObservationGapId>| {
            vec![
                complete("root"),
                (
                    "sub",
                    match gap_id {
                        Some(id) => CoverageStatus::Incomplete {
                            gap_ids: vec![id.clone()],
                        },
                        None => CoverageStatus::Complete,
                    },
                ),
            ]
        };
        let base = snapshot(
            "ctx",
            &[("root", resource("res_1", "a.txt", "one"))],
            &coverage(Some(&gap.gap_id)),
            vec![gap.clone()],
        );
        let result = snapshot(
            "ctx",
            &[("root", resource("res_1", "a.txt", "two"))],
            &coverage(Some(&gap.gap_id)),
            vec![gap],
        );
        let change = derive_change_set(&base, &result).unwrap();
        // `res_1` was fully observed on both sides; a problem elsewhere is not a
        // reason to withhold a change Draft actually established.
        assert_eq!(change.resources.len(), 1);
        assert!(change.resources[0]
            .aspects
            .contains(&ChangeAspect::ContentChanged));
    }

    #[test]
    fn snapshots_from_different_observation_contexts_cannot_be_compared() {
        let base = snapshot(
            "ctx-a",
            &[("root", resource("res_1", "a.txt", "one"))],
            &[complete("root")],
            vec![],
        );
        let result = snapshot("ctx-b", &[], &[complete("root")], vec![]);
        let error = derive_change_set(&base, &result).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
        assert!(error.to_string().contains("observation context"));
    }

    #[test]
    fn change_identity_follows_snapshot_digests_not_record_ids() {
        let states = [("root", resource("res_1", "a.txt", "one"))];
        let after = [("root", resource("res_1", "a.txt", "two"))];
        let first = derive_change_set(
            &snapshot("ctx", &states, &[complete("root")], vec![]),
            &snapshot("ctx", &after, &[complete("root")], vec![]),
        )
        .unwrap();
        // Fresh snapshot records, same authoritative observations.
        let second = derive_change_set(
            &snapshot("ctx", &states, &[complete("root")], vec![]),
            &snapshot("ctx", &after, &[complete("root")], vec![]),
        )
        .unwrap();
        assert_ne!(first.base_snapshot_id, second.base_snapshot_id);
        assert_eq!(
            first.change_set_digest, second.change_set_digest,
            "record identity must not leak into the canonical transition"
        );
    }

    #[test]
    fn a_substituted_snapshot_is_rejected_on_validation() {
        let base = snapshot(
            "ctx",
            &[("root", resource("res_1", "a.txt", "one"))],
            &[complete("root")],
            vec![],
        );
        let result = snapshot(
            "ctx",
            &[("root", resource("res_1", "a.txt", "two"))],
            &[complete("root")],
            vec![],
        );
        let change = derive_change_set(&base, &result).unwrap();

        // Same record id, different content: the digest binding catches it.
        let mut swapped = snapshot(
            "ctx",
            &[("root", resource("res_1", "a.txt", "three"))],
            &[complete("root")],
            vec![],
        );
        swapped.id = result.id.clone();
        let error = change.validate_against(&base, &swapped).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn derivation_status_is_a_projection_of_the_gap_set() {
        let base = snapshot("ctx", &[], &[complete("root")], vec![]);
        let change = derive_change_set(&base, &base).unwrap();
        let encoded = serde_json::to_value(&change).unwrap();
        assert!(encoded.get("derivation_status").is_none());
        assert!(change.derivation_status().is_complete());
    }
}
