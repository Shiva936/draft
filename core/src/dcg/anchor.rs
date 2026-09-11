//! Retained evidence that a past state can be recreated — and the honest limits
//! on that claim.
//!
//! Observation and restoration are different capabilities. Knowing a historical
//! `state_digest` proves what was there; it does not prove Draft can put it
//! back. A [`RecoveryAnchor`] is the separate, contemporaneous evidence that it
//! can, and everything here exists to keep those two facts from being confused:
//!
//! * An anchor is captured **at the target time, under live fencing**, from the
//!   very generation whose digest it claims. Draft never manufactures one
//!   retroactively from current state, because current state is not evidence
//!   about the past.
//! * An anchor carries enough material to recreate the **complete**
//!   `RawResourceState`, not merely its bytes. A file's mode is part of its
//!   state digest; restoring the content alone would not restore the state.
//! * Anchors are **per-resource**. A coverage domain that was never observed has
//!   no resources and therefore no anchors, and no domain-level anchor exists to
//!   paper over that. Such a target is permanently unverifiable, and later
//!   observability does not change it: observing that domain today reveals
//!   today's state, not the target's.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::contracts::ProducerRef;
use crate::contracts::{current_version, ContractId, VersionedContract};
use crate::dcg::observation::{AdapterBindingId, CoverageDomainRef, ObservationRunId};
use crate::dcg::resource::{ResourceId, ResourceLocator};
use crate::dcg::state::Snapshot;
use crate::support::common::Timestamp;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::canonical_hash;
use draft_extension_contract::ObservationConsistency;

/// The Core semantics behind anchor selection, restore planning and rollback
/// outcome classification.
pub const RECOVERY_PLANNER_REVISION: u32 = 1;

/// What an adapter can do about putting a past state back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryCapability {
    /// Nothing. State is observable but not restorable.
    None,
    /// Draft retains the material and writes it back.
    ContentRestore,
    /// The backend holds an immutable version Draft can ask it to restore.
    VersionRestore,
    /// The adapter owns the whole mechanism behind an opaque handle.
    AdapterManaged,
}

/// A reference to a canonical document in Draft's object store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalObjectRef {
    pub object_digest: String,
    pub length: u64,
}

/// What an anchor retains, and therefore what it can put back.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "material", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryMaterial {
    /// Content in Draft's object store, plus the canonical restore-state
    /// document covering every other property that participates in this
    /// adapter's `state_digest`.
    ///
    /// `restore_state` is required whenever bytes alone do not determine the
    /// state digest — which, for the filesystem adapter, they never do: the
    /// executable bit and the resource's form are part of its identity.
    ContentObject {
        object_digest: String,
        length: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        restore_state: Option<CanonicalObjectRef>,
    },
    /// A canonical, schema-validated adapter state document.
    CanonicalState {
        payload: CanonicalObjectRef,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        referenced_objects: Vec<String>,
    },
    /// An immutable external handle. Durability is the backend's, and is
    /// reported as such rather than assumed.
    AdapterSnapshotRef {
        immutable_reference: String,
        reference_digest: String,
    },
}

/// How a capture was fenced to the generation it claims.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "fencing", rename_all = "snake_case", deny_unknown_fields)]
pub enum FencingEvidence {
    /// The adapter's live token still matched when the material was taken.
    TokenMatched { consistency: ObservationConsistency },
    /// The state digest was revalidated before and after. Both must equal the
    /// target digest, or the capture is refused.
    DigestRevalidated { before: String, after: String },
}

/// Proof that material was captured from the generation it claims — not from a
/// later or racing one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorCapture {
    pub observed_state_digest: String,
    pub observation_run_id: ObservationRunId,
    pub fenced_with: FencingEvidence,
    pub captured_at: Timestamp,
}

/// Retained, per-resource, target-state-bound restoration evidence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryAnchor {
    pub schema_version: u32,
    pub resource_id: ResourceId,
    /// The exact state this anchor can restore. Not "roughly this resource".
    pub target_state_digest: String,
    /// The locator the resource occupied at the target time. A restore puts it
    /// back *here*, not wherever it happens to live now.
    pub target_locator: ResourceLocator,
    pub adapter_binding_id: AdapterBindingId,
    pub recovery_material: RecoveryMaterial,
    pub capture: AnchorCapture,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<ProducerRef>,
    pub recorded_at: Timestamp,
    pub anchor_digest: String,
}

impl RecoveryAnchor {
    /// Seal an anchor, refusing one whose capture does not match its claim.
    ///
    /// This is the structural guarantee behind every later `Complete` rollback:
    /// an anchor whose captured digest differs from the state it claims to
    /// restore is not a weaker anchor, it is a wrong one.
    pub fn seal(mut self) -> DraftResult<Self> {
        if self.capture.observed_state_digest != self.target_state_digest {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "recovery anchor for {} claims state {} but captured {}",
                    self.resource_id.as_str(),
                    self.target_state_digest,
                    self.capture.observed_state_digest
                ),
            ));
        }
        if let FencingEvidence::DigestRevalidated { before, after } = &self.capture.fenced_with {
            if before != &self.target_state_digest || after != &self.target_state_digest {
                return Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "recovery anchor for {} was captured across a state change",
                        self.resource_id.as_str()
                    ),
                ));
            }
        }
        self.anchor_digest = String::new();
        self.anchor_digest = canonical_hash(&self);
        Ok(self)
    }

    /// Every object in Draft's store this anchor depends on.
    ///
    /// Garbage collection follows exactly this: material an accepted receipt
    /// relies on must not disappear because an unrelated pass thought it
    /// unreachable.
    pub fn referenced_objects(&self) -> Vec<String> {
        match &self.recovery_material {
            RecoveryMaterial::ContentObject {
                object_digest,
                restore_state,
                ..
            } => {
                let mut refs = vec![object_digest.clone()];
                refs.extend(
                    restore_state
                        .iter()
                        .map(|state| state.object_digest.clone()),
                );
                refs
            }
            RecoveryMaterial::CanonicalState {
                payload,
                referenced_objects,
            } => {
                let mut refs = vec![payload.object_digest.clone()];
                refs.extend(referenced_objects.iter().cloned());
                refs
            }
            // Nothing local to pin. Durability is external, and saying so is the
            // point: an anchor Draft cannot guarantee must not look like one it
            // can.
            RecoveryMaterial::AdapterSnapshotRef { .. } => Vec::new(),
        }
    }

    /// Whether this anchor's material is locally guaranteed.
    pub fn is_locally_retained(&self) -> bool {
        !matches!(
            self.recovery_material,
            RecoveryMaterial::AdapterSnapshotRef { .. }
        )
    }
}

/// The anchors retained for one target snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryAnchorSet {
    pub schema_version: u32,
    /// The exact snapshot this set can restore toward. A restore plan may only
    /// reference a set bound to the snapshot it is restoring to.
    pub target_snapshot_digest: String,
    pub anchors: Vec<RecoveryAnchor>,
    pub planner_revision: u32,
    pub anchor_set_digest: String,
}

impl VersionedContract for RecoveryAnchorSet {
    const CONTRACT: ContractId = ContractId::RecoveryAnchorSet;
}

impl RecoveryAnchorSet {
    /// Build and seal a set, validating every anchor against its target.
    pub fn build(snapshot: &Snapshot, anchors: Vec<RecoveryAnchor>) -> DraftResult<Self> {
        let mut set = Self {
            schema_version: current_version(ContractId::RecoveryAnchorSet),
            target_snapshot_digest: snapshot.snapshot_digest.clone(),
            anchors,
            planner_revision: RECOVERY_PLANNER_REVISION,
            anchor_set_digest: String::new(),
        };
        set.anchors.sort();
        set.anchors.dedup();
        set.validate_against(snapshot)?;
        set.anchor_set_digest = String::new();
        set.anchor_set_digest = canonical_hash(&set);
        Ok(set)
    }

    /// Refuse a set that does not describe the snapshot it names.
    ///
    /// An orphan anchor, or one whose target digest disagrees with the
    /// snapshot's own record of that resource, would let a restore claim to
    /// reach a state the snapshot never contained.
    pub fn validate_against(&self, snapshot: &Snapshot) -> DraftResult<()> {
        if self.target_snapshot_digest != snapshot.snapshot_digest {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "recovery anchor set is bound to a different snapshot",
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for anchor in &self.anchors {
            if !seen.insert(&anchor.resource_id) {
                return Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "recovery anchor set has two anchors for {}",
                        anchor.resource_id.as_str()
                    ),
                ));
            }
            let Some(state) = snapshot.resource(&anchor.resource_id) else {
                return Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "recovery anchor names {}, which the target snapshot does not contain",
                        anchor.resource_id.as_str()
                    ),
                ));
            };
            if state.state_digest != anchor.target_state_digest {
                return Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "recovery anchor for {} claims a state the target snapshot does not have",
                        anchor.resource_id.as_str()
                    ),
                ));
            }
        }
        Ok(())
    }

    pub fn anchor_for(&self, resource_id: &ResourceId) -> Option<&RecoveryAnchor> {
        self.anchors
            .iter()
            .find(|anchor| &anchor.resource_id == resource_id)
    }

    /// Every object this set depends on, for pinning against collection.
    pub fn referenced_objects(&self) -> Vec<String> {
        let mut refs: Vec<String> = self
            .anchors
            .iter()
            .flat_map(RecoveryAnchor::referenced_objects)
            .collect();
        refs.sort();
        refs.dedup();
        refs
    }

    /// How completely this set covers its target.
    ///
    /// A deterministic projection, never stored: recovery readiness cannot
    /// contradict the anchors it is computed from.
    pub fn status(&self, snapshot: &Snapshot) -> SnapshotRecoveryStatus {
        let missing_resources: Vec<ResourceId> = snapshot
            .resources
            .iter()
            .filter(|state| self.anchor_for(&state.resource_id).is_none())
            .map(|state| state.resource_id.clone())
            .collect();
        // A domain that was never observed has no resources here at all, so it
        // can never be anchored. Recording it explicitly is what stops a later
        // reader assuming the absence of anchors means the absence of content.
        let missing_domains: Vec<CoverageDomainRef> = snapshot
            .observation_map
            .domains
            .iter()
            .filter(|coverage| !coverage.status.is_complete())
            .map(|coverage| coverage.domain.clone())
            .collect();

        if missing_resources.is_empty() && missing_domains.is_empty() {
            SnapshotRecoveryStatus::FullyAnchored
        } else if self.anchors.is_empty() {
            SnapshotRecoveryStatus::NotAnchored
        } else {
            SnapshotRecoveryStatus::PartiallyAnchored {
                missing_resources,
                missing_domains,
            }
        }
    }
}

/// How completely a snapshot can be restored.
///
/// A separate dimension from observation completeness: a snapshot may be
/// perfectly `Complete` and entirely unrestorable. `Snapshot Complete` never
/// means `Snapshot restorable`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "recovery", rename_all = "snake_case")]
pub enum SnapshotRecoveryStatus {
    FullyAnchored,
    PartiallyAnchored {
        missing_resources: Vec<ResourceId>,
        missing_domains: Vec<CoverageDomainRef>,
    },
    NotAnchored,
}

impl SnapshotRecoveryStatus {
    pub fn is_fully_anchored(&self) -> bool {
        matches!(self, Self::FullyAnchored)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FullyAnchored => "fully_anchored",
            Self::PartiallyAnchored { .. } => "partially_anchored",
            Self::NotAnchored => "not_anchored",
        }
    }
}

/// Why a rollback could not be fully proved.
///
/// Typed rather than a message, so a surface can say precisely what went wrong
/// instead of "rollback incomplete".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "uncertainty", rename_all = "snake_case")]
pub enum RollbackUncertainty {
    /// The target state for these domains was never authoritatively observed.
    /// Permanent for this target: no later observation can supply it.
    TargetStateUnknown { domains: Vec<CoverageDomainRef> },
    /// The target state is known, but the material to recreate it is not
    /// retained or no longer available.
    RecoveryMaterialUnavailable { resources: Vec<ResourceId> },
    /// The post-restore observation could not establish the whole claimed scope.
    CurrentObservationIncomplete {
        gap_ids: Vec<crate::dcg::observation::ObservationGapId>,
    },
    /// Restoration ran, but the resulting state does not equal the target.
    RestoreVerificationFailed { resources: Vec<ResourceId> },
    /// The owning adapter cannot restore at all.
    AdapterRecoveryUnavailable { binding_id: AdapterBindingId },
    /// The target was observed under semantics the current context cannot
    /// reinterpret.
    ContextIncompatible {
        target_context_digest: String,
        active_context_digest: String,
    },
}

/// What a rollback actually achieved.
///
/// `Complete` is a strong claim and is made only when every part of it holds. A
/// successful mutation is not completion: without post-restore verification
/// Draft has applied changes, not proved a state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RollbackOutcome {
    Complete {
        target_snapshot_digest: String,
        resulting_snapshot_digest: String,
    },
    Incomplete {
        target_snapshot_digest: String,
        resulting_known_snapshot_digest: String,
        uncertainties: Vec<RollbackUncertainty>,
        /// Domains whose target state can never be verified for this target,
        /// however observable they become later.
        permanently_unverifiable_target_domains: Vec<CoverageDomainRef>,
        restored_known_resources: Vec<ResourceId>,
    },
    Refused {
        reason: String,
        uncertainties: Vec<RollbackUncertainty>,
    },
}

impl RollbackOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Complete { .. } => "complete",
            Self::Incomplete { .. } => "incomplete",
            Self::Refused { .. } => "refused",
        }
    }

    /// Whether the target was fully reached and proved.
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete { .. })
    }

    pub fn uncertainties(&self) -> &[RollbackUncertainty] {
        match self {
            Self::Complete { .. } => &[],
            Self::Incomplete { uncertainties, .. } | Self::Refused { uncertainties, .. } => {
                uncertainties
            }
        }
    }
}

/// One resource a restore must bring back into existence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreTarget {
    pub resource_id: ResourceId,
    /// From the target snapshot, never guessed from where the resource sits now.
    pub target_locator: ResourceLocator,
    pub target_state_digest: String,
    pub anchor_digest: String,
}

/// One resource a restore must remove, because the target proves it was absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbsenceRestoreTarget {
    pub current_resource_id: ResourceId,
    pub current_locator: ResourceLocator,
    /// The domain whose `Complete` coverage at target time proves the absence.
    /// Without this the deletion has no authority and is refused.
    pub target_coverage_domain: CoverageDomainRef,
}

/// A Draft-authored plan to restore target presence *and* target absence.
///
/// Both halves matter. Restoring only what an anchor covers does not restore the
/// target: a resource that exists now but was authoritatively absent then must
/// go, or the result is not the target state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRestorePlan {
    pub schema_version: u32,
    pub operation_id: crate::support::common::OperationId,
    pub target_snapshot_digest: String,
    /// Must be bound to `target_snapshot_digest`.
    pub anchor_set_digest: String,
    pub restore_targets: Vec<RestoreTarget>,
    pub absence_targets: Vec<AbsenceRestoreTarget>,
    /// What the plan already knows it cannot achieve, before it runs.
    pub known_uncertainties: Vec<RollbackUncertainty>,
    pub planner_revision: u32,
}

impl VersionedContract for ResourceRestorePlan {
    const CONTRACT: ContractId = ContractId::ResourceRestorePlan;
}

impl ResourceRestorePlan {
    /// Plan a restore from a target snapshot, its anchor set and current state.
    ///
    /// Every deletion here is backed by a coverage proof, and every restoration
    /// by an anchor. What cannot be backed either way becomes a recorded
    /// uncertainty rather than an optimistic action.
    pub fn plan(
        operation_id: crate::support::common::OperationId,
        target: &Snapshot,
        anchors: &RecoveryAnchorSet,
        current: &Snapshot,
    ) -> DraftResult<Self> {
        anchors.validate_against(target)?;
        let mut restore_targets = Vec::new();
        let mut known_uncertainties = Vec::new();
        let mut unavailable = Vec::new();

        for state in &target.resources {
            match anchors.anchor_for(&state.resource_id) {
                Some(anchor) => restore_targets.push(RestoreTarget {
                    resource_id: state.resource_id.clone(),
                    target_locator: state.locator.clone(),
                    target_state_digest: state.state_digest.clone(),
                    anchor_digest: anchor.anchor_digest.clone(),
                }),
                None => unavailable.push(state.resource_id.clone()),
            }
        }
        if !unavailable.is_empty() {
            known_uncertainties.push(RollbackUncertainty::RecoveryMaterialUnavailable {
                resources: unavailable,
            });
        }

        // Absence needs no anchor — absence has no bytes — but it does need
        // proof. The authority is the target-time coverage: only a domain that
        // was `Complete` then can testify that a resource was not there.
        //
        // The domain refs are comparable only because both snapshots share one
        // observation context; under different semantics the same local id would
        // mean different things, and the deletion would have no authority.
        let comparable_contexts =
            target.observation_context_digest == current.observation_context_digest;
        let mut absence_targets = Vec::new();
        let mut unprovable_domains = Vec::new();
        for state in &current.resources {
            if target.resource(&state.resource_id).is_some() {
                continue;
            }
            let domain = comparable_contexts
                .then(|| current.observation_map.domain_of(&state.resource_id))
                .flatten()
                .cloned();
            match domain {
                Some(domain) if target.observation_map.proves_absence_in(&domain) => {
                    absence_targets.push(AbsenceRestoreTarget {
                        current_resource_id: state.resource_id.clone(),
                        current_locator: state.locator.clone(),
                        target_coverage_domain: domain,
                    });
                }
                // Draft will not delete on a guess. The resource stays, and the
                // rollback reports that it could not prove the target's absence.
                Some(domain) => unprovable_domains.push(domain),
                None => unprovable_domains.extend(
                    target
                        .observation_map
                        .domains
                        .iter()
                        .filter(|coverage| !coverage.status.is_complete())
                        .map(|coverage| coverage.domain.clone()),
                ),
            }
        }
        unprovable_domains.sort();
        unprovable_domains.dedup();
        if !unprovable_domains.is_empty() {
            known_uncertainties.push(RollbackUncertainty::TargetStateUnknown {
                domains: unprovable_domains,
            });
        }

        Ok(Self {
            schema_version: current_version(ContractId::ResourceRestorePlan),
            operation_id,
            target_snapshot_digest: target.snapshot_digest.clone(),
            anchor_set_digest: anchors.anchor_set_digest.clone(),
            restore_targets,
            absence_targets,
            known_uncertainties,
            planner_revision: RECOVERY_PLANNER_REVISION,
        })
    }

    /// Whether the plan, as planned, could reach the target completely.
    pub fn can_be_complete(&self) -> bool {
        self.known_uncertainties.is_empty()
    }
}

/// Classify a rollback from what was planned and what was afterwards observed.
///
/// The comparison is over the **complete** state: existence, absence, locators
/// and every state digest. Anything less would let a rollback that restored
/// content but not mode report `Complete`.
pub fn classify_rollback(
    plan: &ResourceRestorePlan,
    target: &Snapshot,
    observed: &Snapshot,
) -> RollbackOutcome {
    let mut uncertainties = plan.known_uncertainties.clone();
    let mut restored = Vec::new();
    let mut mismatched = Vec::new();

    for restore_target in &plan.restore_targets {
        match observed.resource(&restore_target.resource_id) {
            Some(state)
                if state.state_digest == restore_target.target_state_digest
                    && state.locator == restore_target.target_locator =>
            {
                restored.push(restore_target.resource_id.clone());
            }
            _ => mismatched.push(restore_target.resource_id.clone()),
        }
    }
    // Absence is verified too: a resource the plan meant to remove that is still
    // present means the target was not reached.
    for absence in &plan.absence_targets {
        if observed.resource(&absence.current_resource_id).is_some() {
            mismatched.push(absence.current_resource_id.clone());
        }
    }
    if !mismatched.is_empty() {
        mismatched.sort();
        mismatched.dedup();
        uncertainties.push(RollbackUncertainty::RestoreVerificationFailed {
            resources: mismatched,
        });
    }

    // Without a complete post-restore observation there is nothing to compare
    // against — applying the changes is not the same as proving the state.
    let observation = observed.observation_status();
    if !observation.is_complete() {
        uncertainties.push(RollbackUncertainty::CurrentObservationIncomplete {
            gap_ids: observed.gaps.iter().map(|gap| gap.gap_id.clone()).collect(),
        });
    }

    // A domain unobserved at target time has no resources and therefore no
    // anchors. It is permanently unverifiable for this target, whatever happens
    // to its observability afterwards.
    let permanently_unverifiable_target_domains: Vec<CoverageDomainRef> = target
        .observation_map
        .domains
        .iter()
        .filter(|coverage| !coverage.status.is_complete())
        .map(|coverage| coverage.domain.clone())
        .collect();

    if uncertainties.is_empty() && permanently_unverifiable_target_domains.is_empty() {
        RollbackOutcome::Complete {
            target_snapshot_digest: target.snapshot_digest.clone(),
            resulting_snapshot_digest: observed.snapshot_digest.clone(),
        }
    } else {
        RollbackOutcome::Incomplete {
            target_snapshot_digest: target.snapshot_digest.clone(),
            resulting_known_snapshot_digest: observed.snapshot_digest.clone(),
            uncertainties,
            permanently_unverifiable_target_domains,
            restored_known_resources: restored,
        }
    }
}

/// Every object reachable from a set of retained anchor sets.
///
/// The garbage collector consults this: recovery material an accepted receipt
/// relies on is pinned, and its later disappearance is an integrity failure
/// rather than a silent downgrade of what rollback can promise.
pub fn pinned_objects(sets: &[RecoveryAnchorSet]) -> BTreeMap<String, usize> {
    let mut pinned: BTreeMap<String, usize> = BTreeMap::new();
    for set in sets {
        for object in set.referenced_objects() {
            *pinned.entry(object).or_insert(0) += 1;
        }
    }
    pinned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcg::state::tests_support;
    use crate::support::common::now;

    fn anchor(resource: &str, state_digest: &str) -> RecoveryAnchor {
        RecoveryAnchor {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::RecoveryAnchorSet,
            ),
            resource_id: crate::dcg::resource::resource_id_for_locator(&format!("file:{resource}")),
            target_state_digest: state_digest.to_string(),
            target_locator: ResourceLocator::file(resource),
            adapter_binding_id: AdapterBindingId("core.filesystem".into()),
            recovery_material: RecoveryMaterial::ContentObject {
                object_digest: format!("b3:{resource}"),
                length: 8,
                restore_state: Some(CanonicalObjectRef {
                    object_digest: format!("b3:{resource}-state"),
                    length: 32,
                }),
            },
            capture: AnchorCapture {
                observed_state_digest: state_digest.to_string(),
                observation_run_id: ObservationRunId("run_1".into()),
                fenced_with: FencingEvidence::TokenMatched {
                    consistency: ObservationConsistency::BestEffortGeneration,
                },
                captured_at: now(),
            },
            producer: None,
            recorded_at: now(),
            anchor_digest: String::new(),
        }
        .seal()
        .unwrap()
    }

    fn target(bodies: &[&str]) -> Snapshot {
        tests_support::sealed("prj_recovery", bodies)
    }

    fn anchors_for(snapshot: &Snapshot) -> RecoveryAnchorSet {
        let anchors = snapshot
            .resources
            .iter()
            .map(|state| {
                let mut built = anchor(&state.locator.body, &state.state_digest);
                built.resource_id = state.resource_id.clone();
                built.target_locator = state.locator.clone();
                built.capture.observed_state_digest = state.state_digest.clone();
                built.anchor_digest = String::new();
                built.seal().unwrap()
            })
            .collect();
        RecoveryAnchorSet::build(snapshot, anchors).unwrap()
    }

    #[test]
    fn a_live_token_can_never_be_persisted_as_recovery_material() {
        // The two are kept apart structurally, not by convention. A token
        // identifies a generation for the adapter that minted it and carries
        // nothing that could recreate anything, so a record that stored one as
        // material would claim a restorability it does not have.
        let sealed = anchor("a.txt", &crate::support::hashing::sha256_hex(b"a"));
        let mut document = serde_json::to_value(&sealed).unwrap();

        // It is not in the anchor to begin with, at any depth.
        assert!(
            !serde_json::to_string(&document)
                .unwrap()
                .contains("observation_token"),
            "a persisted anchor holds no live fencing token"
        );

        // And one cannot be smuggled in: the contract refuses the field.
        document["observation_token"] = serde_json::json!("gen-7");
        let refused = serde_json::from_value::<RecoveryAnchor>(document);
        assert!(
            refused.is_err(),
            "an anchor carrying a token must fail to decode, not decode and be ignored"
        );

        // What the anchor *does* keep is evidence about the capture: proof it
        // was taken from the generation whose state it claims.
        assert_eq!(
            sealed.capture.observed_state_digest, sealed.target_state_digest,
            "the capture is fenced to the state the anchor restores"
        );
    }

    #[test]
    fn an_anchor_whose_capture_disagrees_with_its_claim_is_refused() {
        let mut mismatched = anchor("a.txt", "sha256:target");
        mismatched.capture.observed_state_digest = "sha256:something-else".into();
        mismatched.anchor_digest = String::new();
        let error = mismatched.seal().unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn a_capture_across_a_state_change_is_refused() {
        let mut racing = anchor("a.txt", "sha256:target");
        racing.capture.fenced_with = FencingEvidence::DigestRevalidated {
            before: "sha256:target".into(),
            after: "sha256:moved-on".into(),
        };
        racing.anchor_digest = String::new();
        assert!(racing.seal().is_err(), "a racing capture yields no anchor");
    }

    #[test]
    fn an_anchor_set_is_bound_to_one_target_snapshot() {
        let first = target(&["a.txt"]);
        let second = target(&["b.txt"]);
        let set = anchors_for(&first);
        assert!(set.validate_against(&first).is_ok());
        assert!(
            set.validate_against(&second).is_err(),
            "a set must not validate against a snapshot it does not describe"
        );
    }

    #[test]
    fn an_orphan_anchor_is_rejected() {
        let snapshot = target(&["a.txt"]);
        let stray = anchor("nowhere.txt", "sha256:whatever");
        let error = RecoveryAnchorSet::build(&snapshot, vec![stray]).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn observable_does_not_imply_restorable() {
        let snapshot = target(&["a.txt", "b.txt"]);
        assert!(snapshot.is_complete(), "the observation is complete");
        let empty = RecoveryAnchorSet::build(&snapshot, vec![]).unwrap();
        // Complete observation, nothing restorable. The two dimensions are
        // genuinely independent.
        assert_eq!(empty.status(&snapshot), SnapshotRecoveryStatus::NotAnchored);
    }

    #[test]
    fn a_plan_restores_presence_and_absence_and_records_what_it_cannot() {
        let target_state = target(&["a.txt"]);
        let anchors = anchors_for(&target_state);
        // Current state has an extra resource the target proves absent.
        let current = target(&["a.txt", "extra.txt"]);
        let plan = ResourceRestorePlan::plan(
            crate::support::common::OperationId::new("op_restore"),
            &target_state,
            &anchors,
            &current,
        )
        .unwrap();
        assert_eq!(plan.restore_targets.len(), 1);
        assert_eq!(plan.absence_targets.len(), 1);
        assert_eq!(
            plan.absence_targets[0].current_resource_id,
            crate::dcg::resource::resource_id_for_locator("file:extra.txt")
        );
        assert!(plan.can_be_complete());
    }

    #[test]
    fn a_mutation_that_did_not_reach_the_target_is_not_complete() {
        let target_state = target(&["a.txt"]);
        let anchors = anchors_for(&target_state);
        let current = target(&["a.txt"]);
        let plan = ResourceRestorePlan::plan(
            crate::support::common::OperationId::new("op_restore"),
            &target_state,
            &anchors,
            &current,
        )
        .unwrap();

        // Post-restore observation shows a different state for the resource.
        let mut observed = tests_support::empty("prj_recovery");
        let mut drifted = tests_support::resource("a.txt");
        drifted.state_digest = "sha256:not-the-target".into();
        observed.observation_map.resource_membership.push(
            crate::dcg::observation::ResourceCoverageMembership {
                resource_id: drifted.resource_id.clone(),
                domain: tests_support::domain("root"),
            },
        );
        observed.resources.push(drifted);
        let observed = observed.seal();

        let outcome = classify_rollback(&plan, &target_state, &observed);
        assert!(!outcome.is_complete(), "{outcome:?}");
        assert!(outcome.uncertainties().iter().any(|uncertainty| matches!(
            uncertainty,
            RollbackUncertainty::RestoreVerificationFailed { .. }
        )));
    }

    #[test]
    fn a_verified_restore_is_complete() {
        let target_state = target(&["a.txt"]);
        let anchors = anchors_for(&target_state);
        let plan = ResourceRestorePlan::plan(
            crate::support::common::OperationId::new("op_restore"),
            &target_state,
            &anchors,
            &target_state,
        )
        .unwrap();
        let outcome = classify_rollback(&plan, &target_state, &target_state);
        assert!(outcome.is_complete(), "{outcome:?}");
    }

    #[test]
    fn material_referenced_by_an_anchor_is_pinned() {
        let snapshot = target(&["a.txt"]);
        let set = anchors_for(&snapshot);
        let pinned = pinned_objects(std::slice::from_ref(&set));
        // Both the content and the restore-state document: bytes alone would not
        // reproduce the full state digest.
        assert_eq!(pinned.len(), 2, "{pinned:?}");
        assert!(pinned.contains_key("b3:a.txt"));
        assert!(pinned.contains_key("b3:a.txt-state"));
    }

    #[test]
    fn an_external_handle_is_reported_as_unpinnable_rather_than_assumed_durable() {
        let mut external = anchor("a.txt", "sha256:target");
        external.recovery_material = RecoveryMaterial::AdapterSnapshotRef {
            immutable_reference: "backend://version/42".into(),
            reference_digest: "sha256:ref".into(),
        };
        assert!(external.referenced_objects().is_empty());
        assert!(!external.is_locally_retained());
    }
}
