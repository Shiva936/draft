//! How Draft knows what it can see, and how much of it it actually saw.
//!
//! Three separate ideas live here, and keeping them apart is what stops an
//! observation problem from being reported as a project change:
//!
//! * **Coverage** — which parts of the observable universe were enumerated
//!   completely. Absence is only meaningful inside a domain that was observed
//!   completely; everywhere else, absence proves nothing.
//! * **Context** — the effective semantics that decided *what* is observable at
//!   all: which adapter, running which executable or engine revision, under
//!   which view rules. It participates in snapshot identity.
//! * **Run provenance** — which implementation revision actually performed each
//!   observation. It does *not* participate in snapshot identity, so two
//!   two semantically equivalent observers of the same state produce the same
//!   authoritative snapshot while history still records which one ran.

use crate::support::common::{OperationId, Timestamp};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing;
use draft_extension_contract::EngineId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// Coverage
// ---------------------------------------------------------------------------

/// Stable identity of one adapter binding inside an observation context.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AdapterBindingId(pub String);

/// Stable identity of one declarative view-rule binding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ViewRuleBindingId(pub String);

/// An adapter-local coverage domain name. Opaque, and meaningful only inside the
/// binding that minted it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CoverageDomainLocalId(pub String);

/// A coverage domain, scoped to the adapter binding that defined it.
///
/// Scoping is not decoration. Two independent adapters may each legitimately
/// call a domain `root`, and comparing those as equal would let absence observed
/// by one adapter "prove" absence in the other's universe. Equality is only ever
/// meaningful within one binding, and the type makes that structural.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageDomainRef {
    pub adapter_binding_id: AdapterBindingId,
    pub local_id: CoverageDomainLocalId,
}

impl CoverageDomainRef {
    pub fn new(adapter_binding_id: AdapterBindingId, local_id: impl Into<String>) -> Self {
        Self {
            adapter_binding_id,
            local_id: CoverageDomainLocalId(local_id.into()),
        }
    }
}

impl std::fmt::Display for CoverageDomainRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}:{}",
            self.adapter_binding_id.0, self.local_id.0
        )
    }
}

/// Whether absence inside one domain was authoritatively established.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum CoverageStatus {
    /// Enumeration of this domain completed; absence within it is authoritative.
    Complete,
    /// One or more gaps apply; absence within this domain proves nothing.
    Incomplete { gap_ids: Vec<ObservationGapId> },
}

impl CoverageStatus {
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationCoverage {
    pub domain: CoverageDomainRef,
    pub status: CoverageStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceCoverageMembership {
    pub resource_id: super::resource::ResourceId,
    pub domain: CoverageDomainRef,
}

/// Which domains a snapshot covered, and which resource belongs to which.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotObservationMap {
    /// Sorted and unique by domain.
    pub domains: Vec<ObservationCoverage>,
    /// Sorted and unique by resource id. Exactly one membership per observed
    /// resource in this revision.
    pub resource_membership: Vec<ResourceCoverageMembership>,
}

impl SnapshotObservationMap {
    /// The coverage status of one domain, if the snapshot knows it at all.
    pub fn status_of(&self, domain: &CoverageDomainRef) -> Option<&CoverageStatus> {
        self.domains
            .iter()
            .find(|coverage| &coverage.domain == domain)
            .map(|coverage| &coverage.status)
    }

    /// The domain a resource was observed in.
    pub fn domain_of(
        &self,
        resource_id: &super::resource::ResourceId,
    ) -> Option<&CoverageDomainRef> {
        self.resource_membership
            .iter()
            .find(|membership| &membership.resource_id == resource_id)
            .map(|membership| &membership.domain)
    }

    /// Whether absence in `domain` is authoritatively established here.
    ///
    /// A domain this snapshot never covered is *not* complete: it is unknown,
    /// which is the whole point of tracking coverage separately from content.
    pub fn proves_absence_in(&self, domain: &CoverageDomainRef) -> bool {
        self.status_of(domain)
            .is_some_and(CoverageStatus::is_complete)
    }

    pub fn sort(&mut self) {
        self.domains.sort_by(|a, b| a.domain.cmp(&b.domain));
        self.resource_membership
            .sort_by(|a, b| a.resource_id.cmp(&b.resource_id));
    }

    /// Reject a structurally inconsistent map.
    ///
    /// These invariants are checked at the contract boundary rather than trusted
    /// from a caller: a dangling membership or a `Complete` domain that also
    /// lists a gap would let later reasoning draw a conclusion the observation
    /// never supported.
    pub fn validate(&self, gaps: &[ObservationGap]) -> DraftResult<()> {
        let corrupt = |detail: String| DraftError::new(DraftErrorKind::CorruptData, detail);

        let mut seen_domains = BTreeSet::new();
        for coverage in &self.domains {
            if !seen_domains.insert(&coverage.domain) {
                return Err(corrupt(format!(
                    "coverage domain {} is listed more than once",
                    coverage.domain
                )));
            }
        }
        if self
            .domains
            .windows(2)
            .any(|pair| pair[0].domain > pair[1].domain)
        {
            return Err(corrupt(
                "coverage domains are not canonically sorted".into(),
            ));
        }

        let known_gaps: BTreeSet<&ObservationGapId> = gaps.iter().map(|gap| &gap.gap_id).collect();
        for coverage in &self.domains {
            match &coverage.status {
                CoverageStatus::Complete => {}
                CoverageStatus::Incomplete { gap_ids } => {
                    if gap_ids.is_empty() {
                        return Err(corrupt(format!(
                            "coverage domain {} is incomplete but names no gap",
                            coverage.domain
                        )));
                    }
                    for gap_id in gap_ids {
                        let Some(gap) = gaps.iter().find(|gap| &gap.gap_id == gap_id) else {
                            return Err(corrupt(format!(
                                "coverage domain {} references unknown gap {}",
                                coverage.domain, gap_id.0
                            )));
                        };
                        if !gap.coverage_domains.contains(&coverage.domain) {
                            return Err(corrupt(format!(
                                "gap {} does not list coverage domain {}",
                                gap_id.0, coverage.domain
                            )));
                        }
                    }
                }
            }
        }
        for gap in gaps {
            if !known_gaps.contains(&gap.gap_id) {
                continue;
            }
            for domain in &gap.coverage_domains {
                if self.status_of(domain).is_none() {
                    return Err(corrupt(format!(
                        "gap {} references coverage domain {} the snapshot does not cover",
                        gap.gap_id.0, domain
                    )));
                }
            }
        }

        let mut seen_resources = BTreeSet::new();
        for membership in &self.resource_membership {
            if !seen_resources.insert(&membership.resource_id) {
                return Err(corrupt(format!(
                    "resource {} has more than one coverage membership",
                    membership.resource_id
                )));
            }
            if self.status_of(&membership.domain).is_none() {
                return Err(corrupt(format!(
                    "resource {} is a member of unknown coverage domain {}",
                    membership.resource_id, membership.domain
                )));
            }
        }
        if self
            .resource_membership
            .windows(2)
            .any(|pair| pair[0].resource_id > pair[1].resource_id)
        {
            return Err(corrupt(
                "coverage memberships are not canonically sorted".into(),
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Gaps
// ---------------------------------------------------------------------------

/// Canonical identity of one observation gap.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObservationGapId(pub String);

/// Why part of the observable universe could not be established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationGapKind {
    UntrackableResource,
    AdapterUnavailable,
    AdapterAmbiguous,
    EnumerationFailed,
    PermissionDenied,
    StaleObservation,
    ObservationBudgetExceeded,
    InvalidAdapterResponse,
    ObserverIdentityUnverified,
}

/// A part of the universe Draft knows it did not establish.
///
/// Canonical identity is over the *semantic* fields only. `detail`,
/// `evidence_ref` and `producer` are explanation and provenance: an OS-specific
/// message or an incidental timestamp must never move project identity, or a
/// reworded diagnostic between builds would look like a state change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationGap {
    pub gap_id: ObservationGapId,
    pub kind: ObservationGapKind,
    /// Machine-stable cause code, e.g. `adapter.permission_denied`.
    pub stable_code: String,
    pub coverage_domains: Vec<CoverageDomainRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<AdapterBindingId>,
    /// A narrowly specified deterministic cause, when a machine fact genuinely
    /// changes what the gap *means*.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_cause_digest: Option<String>,
    // ---- below: provenance and explanation; never identity inputs ----
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ref: Option<String>,
    /// Adapter-supplied, human-readable. Rendered as given; never parsed, and
    /// never assumed to describe a path.
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
}

impl ObservationGap {
    /// Build a gap, deriving its canonical identity from its semantic fields.
    pub fn new(
        kind: ObservationGapKind,
        stable_code: impl Into<String>,
        mut coverage_domains: Vec<CoverageDomainRef>,
        binding_id: Option<AdapterBindingId>,
        detail: impl Into<String>,
    ) -> Self {
        coverage_domains.sort();
        coverage_domains.dedup();
        let stable_code = stable_code.into();
        let gap_id = ObservationGapId(canonical_gap_id(
            kind,
            &stable_code,
            &coverage_domains,
            binding_id.as_ref(),
            None,
        ));
        Self {
            gap_id,
            kind,
            stable_code,
            coverage_domains,
            binding_id,
            semantic_cause_digest: None,
            evidence_ref: None,
            detail: detail.into(),
            producer: None,
        }
    }

    /// The canonical identity this gap's semantic fields imply.
    pub fn canonical_id(&self) -> ObservationGapId {
        ObservationGapId(canonical_gap_id(
            self.kind,
            &self.stable_code,
            &self.coverage_domains,
            self.binding_id.as_ref(),
            self.semantic_cause_digest.as_deref(),
        ))
    }
}

fn canonical_gap_id(
    kind: ObservationGapKind,
    stable_code: &str,
    coverage_domains: &[CoverageDomainRef],
    binding_id: Option<&AdapterBindingId>,
    semantic_cause_digest: Option<&str>,
) -> String {
    let digest = hashing::canonical_hash(&serde_json::json!({
        "kind": kind,
        "stable_code": stable_code,
        "coverage_domains": coverage_domains,
        "binding_id": binding_id,
        "semantic_cause_digest": semantic_cause_digest,
    }));
    format!("gap_{}", &digest[..16])
}

/// A snapshot's aggregate observation status.
///
/// This is a *projection*, never independently stored state: computing it from
/// the coverage map and gap set is the only way to obtain it, so it cannot
/// contradict them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SnapshotObservationStatus {
    Complete,
    Incomplete { gap_ids: Vec<ObservationGapId> },
}

impl SnapshotObservationStatus {
    /// Derive the status from the authoritative data.
    pub fn project(map: &SnapshotObservationMap, gaps: &[ObservationGap]) -> Self {
        let mut gap_ids: Vec<ObservationGapId> = map
            .domains
            .iter()
            .filter_map(|coverage| match &coverage.status {
                CoverageStatus::Complete => None,
                CoverageStatus::Incomplete { gap_ids } => Some(gap_ids.clone()),
            })
            .flatten()
            .chain(gaps.iter().map(|gap| gap.gap_id.clone()))
            .collect();
        gap_ids.sort();
        gap_ids.dedup();
        if gap_ids.is_empty() {
            Self::Complete
        } else {
            Self::Incomplete { gap_ids }
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// What actually performs an observation, in enough detail that a change to it
/// cannot go unnoticed.
///
/// A declarative contribution is not sufficient on its own: the same declaration
/// behaves differently if its executable, engine revision, configuration or
/// schemas change. Everything capable of altering *what is observed* is here;
/// unrelated package bytes are not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mechanism", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectiveObservationMechanism {
    Engine {
        engine: EngineId,
        engine_revision: u32,
        engine_config_digest: String,
        request_schema_digest: String,
        response_schema_digest: String,
        coverage_domain_semantics_digest: String,
    },
    Command {
        executable_identity: VerifiedExecutableIdentity,
        command_config_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        declared_dependency_digest: Option<String>,
        request_schema_digest: String,
        response_schema_digest: String,
        coverage_domain_semantics_digest: String,
    },
}

/// An executable identity strong enough to be an authoritative observer.
///
/// Authoritative observation requires this rather than a path-and-reason string:
/// two unverifiable executables must never compare equal, or a silent binary
/// swap would keep the old context digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedExecutableIdentity {
    pub resolved_path: String,
    pub digest: String,
    pub platform: String,
    pub arch: String,
}

/// An adapter binding: something that executes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterObservationBinding {
    pub binding_id: AdapterBindingId,
    pub contribution_id: String,
    pub contribution_semantics_digest: String,
    pub mechanism: EffectiveObservationMechanism,
}

/// A view-rule binding: purely declarative.
///
/// It has no mechanism, structurally. A rule that says "exclude this" executes
/// nothing, and giving it a mechanism field would invite a fake one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewRuleBinding {
    pub binding_id: ViewRuleBindingId,
    pub contribution_id: String,
    pub contribution_semantics_digest: String,
}

/// The effective semantics that decide what is observable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationContext {
    pub schema_version: u32,
    pub adapter_bindings: Vec<AdapterObservationBinding>,
    pub view_rule_bindings: Vec<ViewRuleBinding>,
    pub context_digest: String,
}

impl crate::contracts::VersionedContract for ObservationContext {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::ObservationContext;
}

impl ObservationContext {
    /// Build a context, deriving its digest from effective semantics alone.
    pub fn build(
        mut adapter_bindings: Vec<AdapterObservationBinding>,
        mut view_rule_bindings: Vec<ViewRuleBinding>,
    ) -> Self {
        adapter_bindings.sort_by(|a, b| a.binding_id.cmp(&b.binding_id));
        view_rule_bindings.sort_by(|a, b| a.binding_id.cmp(&b.binding_id));
        let context_digest = hashing::canonical_hash(&serde_json::json!({
            "adapter_bindings": adapter_bindings,
            "view_rule_bindings": view_rule_bindings,
        }));
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ObservationContext,
            ),
            adapter_bindings,
            view_rule_bindings,
            context_digest,
        }
    }

    /// The binding owning `binding_id`, if this context has one.
    pub fn adapter(&self, binding_id: &AdapterBindingId) -> Option<&AdapterObservationBinding> {
        self.adapter_bindings
            .iter()
            .find(|binding| &binding.binding_id == binding_id)
    }
}

/// The refusal returned when the observer that is about to run is not the one
/// the active context committed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationMechanismDrift {
    pub affected_binding: AdapterBindingId,
    pub active_binding_digest: String,
    pub observed_binding_digest: String,
}

impl ObservationMechanismDrift {
    pub fn into_error(self) -> DraftError {
        DraftError::new(
            DraftErrorKind::ConflictDetected,
            format!(
                "observer for binding '{}' changed since this observation context was adopted \
                 (context expects {}, found {}); re-observe and adopt the new context before \
                 producing an authoritative snapshot",
                self.affected_binding.0, self.active_binding_digest, self.observed_binding_digest
            ),
        )
    }
}

/// Digest one binding's effective semantics, for drift comparison.
pub fn binding_semantics_digest(binding: &AdapterObservationBinding) -> String {
    hashing::canonical_hash(&serde_json::json!({
        "contribution_id": binding.contribution_id,
        "contribution_semantics_digest": binding.contribution_semantics_digest,
        "mechanism": binding.mechanism,
    }))
}

// ---------------------------------------------------------------------------
// Run provenance
// ---------------------------------------------------------------------------

/// Identity of one observation run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObservationRunId(pub String);

/// Who observed.
///
/// Core and extension observers are structurally distinct, so Draft's own
/// filesystem observer is never described by a fabricated extension producer,
/// attestation or grant it does not have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "observed_by", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservationProvider {
    Core {
        component: String,
        implementation_revision: u32,
    },
    Extension {
        producer: crate::contracts::ProducerRef,
        artifact_attestation_digest: String,
    },
}

impl ObservationProvider {
    /// The extension producer, when there is one. A Core observer has none.
    pub fn producer(&self) -> Option<&crate::contracts::ProducerRef> {
        match self {
            Self::Core { .. } => None,
            Self::Extension { producer, .. } => Some(producer),
        }
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservationRunOutcome {
    Succeeded,
    Partial { gap_ids: Vec<ObservationGapId> },
    Failed { gap_ids: Vec<ObservationGapId> },
}

/// One observation run.
///
/// `attempted_domains` and `committed_domains` are deliberately different: a
/// retry after a transient failure attempts a domain that an earlier run also
/// attempted, and only one of them ends up owning the final observation. Keeping
/// both preserves the fact that the first attempt failed, which a record that
/// stored only the winner would erase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationRun {
    pub run_id: ObservationRunId,
    pub binding_id: AdapterBindingId,
    pub effective_binding_digest: String,
    pub observation_provider: ObservationProvider,
    /// What this run tried to observe. May overlap other runs.
    pub attempted_domains: Vec<CoverageDomainRef>,
    /// What this run's observations were retained for. Empty for a failed run.
    pub committed_domains: Vec<CoverageDomainRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authorization_decisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_identity: Option<VerifiedExecutableIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_metadata_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operation_ids: Vec<OperationId>,
    pub started_at: Timestamp,
    pub completed_at: Timestamp,
    pub outcome: ObservationRunOutcome,
}

/// A view-rule's provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewRuleProvenance {
    pub binding_id: ViewRuleBindingId,
    pub source: ObservationProvider,
}

/// A Change or receipt's binding to one exact historical observation.
///
/// Two fields rather than one, and that is the whole point. The snapshot digest
/// says *what state*; the provenance digest says *which observation of it*. One
/// authoritative state can be observed many times — a retry, a re-check, a
/// semantics-equivalent build — and each observation is its own immutable
/// record. A Change that stored only the snapshot digest and resolved "the latest
/// provenance for this state" at read time would quietly change what it claimed
/// every time somebody looked again.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotObservationRef {
    pub snapshot_digest: String,
    pub observation_provenance_digest: String,
}

impl SnapshotObservationRef {
    pub fn new(
        snapshot_digest: impl Into<String>,
        observation_provenance_digest: impl Into<String>,
    ) -> Self {
        Self {
            snapshot_digest: snapshot_digest.into(),
            observation_provenance_digest: observation_provenance_digest.into(),
        }
    }

    /// Check that a resolved record really is the observation this names.
    ///
    /// Both halves are verified. A record whose digest matches but which
    /// describes a different snapshot is a corrupted binding, not a near miss,
    /// and is refused rather than accepted on the strength of the half that
    /// happened to line up.
    pub fn validate_against(&self, provenance: &ObservationRunProvenance) -> DraftResult<()> {
        if provenance.provenance_digest != self.observation_provenance_digest {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "observation record {} was resolved for reference {}",
                    provenance.provenance_digest, self.observation_provenance_digest
                ),
            ));
        }
        if provenance.snapshot_digest != self.snapshot_digest {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "observation record {} describes state {} but is referenced as an observation \
                     of {}",
                    provenance.provenance_digest, provenance.snapshot_digest, self.snapshot_digest
                ),
            ));
        }
        Ok(())
    }
}

/// Which implementations actually produced one authoritative snapshot.
///
/// Not an input to any state digest. That is the point: a semantics-preserving
/// semantics-preserving upgrade must not change what the state *is*, while history
/// must still record which build observed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationRunProvenance {
    pub schema_version: u32,
    pub snapshot_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<crate::support::common::SnapshotId>,
    pub runs: Vec<ObservationRun>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub view_rule_sources: Vec<ViewRuleProvenance>,
    pub assembled_at: Timestamp,
    /// Immutable identity of this historical observation assembly. A later
    /// re-observation of the same snapshot digest gets its own record rather
    /// than overwriting this one.
    pub provenance_digest: String,
}

impl crate::contracts::VersionedContract for ObservationRunProvenance {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::ObservationRunProvenance;
}

impl ObservationRunProvenance {
    pub fn build(
        snapshot_digest: impl Into<String>,
        snapshot_id: Option<crate::support::common::SnapshotId>,
        mut runs: Vec<ObservationRun>,
        mut view_rule_sources: Vec<ViewRuleProvenance>,
        assembled_at: Timestamp,
    ) -> Self {
        runs.sort_by(|a, b| (a.started_at, &a.run_id).cmp(&(b.started_at, &b.run_id)));
        view_rule_sources.sort_by(|a, b| a.binding_id.cmp(&b.binding_id));
        let snapshot_digest = snapshot_digest.into();
        let provenance_digest = hashing::canonical_hash(&serde_json::json!({
            "snapshot_digest": snapshot_digest,
            "runs": runs,
            "view_rule_sources": view_rule_sources,
            "assembled_at": assembled_at,
        }));
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ObservationRunProvenance,
            ),
            snapshot_digest,
            snapshot_id,
            runs,
            view_rule_sources,
            assembled_at,
            provenance_digest,
        }
    }

    /// Reject a provenance record that does not account for the snapshot exactly
    /// once.
    pub fn validate(&self, map: &SnapshotObservationMap) -> DraftResult<()> {
        let corrupt = |detail: String| DraftError::new(DraftErrorKind::CorruptData, detail);
        if self.runs.is_empty() {
            return Err(corrupt(
                "an observation provenance record must describe at least one run".into(),
            ));
        }
        let mut run_ids = BTreeSet::new();
        for run in &self.runs {
            if !run_ids.insert(&run.run_id) {
                return Err(corrupt(format!(
                    "observation run {} is recorded more than once",
                    run.run_id.0
                )));
            }
            if matches!(run.outcome, ObservationRunOutcome::Failed { .. })
                && !run.committed_domains.is_empty()
            {
                return Err(corrupt(format!(
                    "failed observation run {} claims committed domains",
                    run.run_id.0
                )));
            }
            if matches!(run.outcome, ObservationRunOutcome::Succeeded)
                && run.committed_domains.is_empty()
            {
                return Err(corrupt(format!(
                    "successful observation run {} committed nothing",
                    run.run_id.0
                )));
            }
        }

        // Every final domain is owned by exactly one run: attempts may overlap,
        // but the observation that was kept must be unambiguous.
        let mut owner: BTreeMap<&CoverageDomainRef, &ObservationRunId> = BTreeMap::new();
        for run in &self.runs {
            for domain in &run.committed_domains {
                if let Some(existing) = owner.insert(domain, &run.run_id) {
                    return Err(corrupt(format!(
                        "coverage domain {domain} is committed by both run {} and run {}",
                        existing.0, run.run_id.0
                    )));
                }
            }
        }
        let covered: BTreeSet<&CoverageDomainRef> = map
            .domains
            .iter()
            .map(|coverage| &coverage.domain)
            .collect();
        let committed: BTreeSet<&CoverageDomainRef> = owner.keys().copied().collect();
        if covered != committed {
            return Err(corrupt(format!(
                "observation provenance covers {} domain(s) but the snapshot has {}",
                committed.len(),
                covered.len()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcg::resource::ResourceId;
    use crate::support::common::now;

    fn binding(name: &str) -> AdapterBindingId {
        AdapterBindingId(name.into())
    }

    fn domain(binding_name: &str, local: &str) -> CoverageDomainRef {
        CoverageDomainRef::new(binding(binding_name), local)
    }

    fn map(
        domains: Vec<ObservationCoverage>,
        membership: Vec<ResourceCoverageMembership>,
    ) -> SnapshotObservationMap {
        let mut map = SnapshotObservationMap {
            domains,
            resource_membership: membership,
        };
        map.sort();
        map
    }

    #[test]
    fn coverage_domains_are_scoped_to_their_adapter_binding() {
        // Two independent adapters may each call a domain "root". Treating those
        // as equal would let absence observed by one prove absence in the other's
        // universe.
        let filesystem = domain("core.filesystem", "root");
        let catalog = domain("example.catalog", "root");
        assert_ne!(filesystem, catalog);
        assert_eq!(filesystem.local_id, catalog.local_id);

        let observed = map(
            vec![ObservationCoverage {
                domain: filesystem.clone(),
                status: CoverageStatus::Complete,
            }],
            vec![],
        );
        assert!(observed.proves_absence_in(&filesystem));
        assert!(
            !observed.proves_absence_in(&catalog),
            "another adapter's domain is unknown, not complete"
        );
    }

    #[test]
    fn an_uncovered_domain_proves_nothing() {
        let observed = map(vec![], vec![]);
        assert!(!observed.proves_absence_in(&domain("core.filesystem", "root")));
    }

    #[test]
    fn gap_identity_ignores_wording_evidence_and_producer() {
        let subject = vec![domain("core.filesystem", "sub")];
        let first = ObservationGap::new(
            ObservationGapKind::PermissionDenied,
            "adapter.permission_denied",
            subject.clone(),
            Some(binding("core.filesystem")),
            "Permission denied while opening /home/a/sub",
        );
        let mut reworded = ObservationGap::new(
            ObservationGapKind::PermissionDenied,
            "adapter.permission_denied",
            subject,
            Some(binding("core.filesystem")),
            "EACCES: could not read directory",
        );
        reworded.evidence_ref = Some("sha256:deadbeef".into());
        reworded.producer = Some("example.adapter".into());
        // A reworded diagnostic between builds must not look like a change.
        assert_eq!(first.gap_id, reworded.gap_id);
        assert_eq!(first.canonical_id(), reworded.canonical_id());

        // A genuinely different cause is a different gap.
        let other = ObservationGap::new(
            ObservationGapKind::EnumerationFailed,
            "adapter.enumeration_failed",
            vec![domain("core.filesystem", "sub")],
            Some(binding("core.filesystem")),
            "same words",
        );
        assert_ne!(first.gap_id, other.gap_id);
    }

    #[test]
    fn observation_status_is_a_projection_of_coverage_and_gaps() {
        let gap = ObservationGap::new(
            ObservationGapKind::PermissionDenied,
            "adapter.permission_denied",
            vec![domain("core.filesystem", "sub")],
            None,
            "denied",
        );
        let complete = map(
            vec![ObservationCoverage {
                domain: domain("core.filesystem", "root"),
                status: CoverageStatus::Complete,
            }],
            vec![],
        );
        assert!(SnapshotObservationStatus::project(&complete, &[]).is_complete());

        let incomplete = map(
            vec![
                ObservationCoverage {
                    domain: domain("core.filesystem", "root"),
                    status: CoverageStatus::Complete,
                },
                ObservationCoverage {
                    domain: domain("core.filesystem", "sub"),
                    status: CoverageStatus::Incomplete {
                        gap_ids: vec![gap.gap_id.clone()],
                    },
                },
            ],
            vec![],
        );
        let status = SnapshotObservationStatus::project(&incomplete, std::slice::from_ref(&gap));
        assert_eq!(
            status,
            SnapshotObservationStatus::Incomplete {
                gap_ids: vec![gap.gap_id.clone()]
            }
        );
    }

    #[test]
    fn a_structurally_inconsistent_map_is_refused() {
        let gap = ObservationGap::new(
            ObservationGapKind::PermissionDenied,
            "adapter.permission_denied",
            vec![domain("core.filesystem", "sub")],
            None,
            "denied",
        );
        let root = domain("core.filesystem", "root");

        // A complete domain that also lists a gap would let a caller conclude
        // absence the observation never established.
        let contradictory = map(
            vec![ObservationCoverage {
                domain: root.clone(),
                status: CoverageStatus::Incomplete { gap_ids: vec![] },
            }],
            vec![],
        );
        assert!(contradictory.validate(&[]).is_err());

        // A gap id nothing resolves.
        let dangling = map(
            vec![ObservationCoverage {
                domain: root.clone(),
                status: CoverageStatus::Incomplete {
                    gap_ids: vec![ObservationGapId("gap_missing".into())],
                },
            }],
            vec![],
        );
        assert!(dangling.validate(&[]).is_err());

        // A membership pointing at a domain the snapshot never covered.
        let orphan = map(
            vec![ObservationCoverage {
                domain: root.clone(),
                status: CoverageStatus::Complete,
            }],
            vec![ResourceCoverageMembership {
                resource_id: ResourceId::parse("res_1").unwrap(),
                domain: domain("core.filesystem", "elsewhere"),
            }],
        );
        assert!(orphan.validate(&[]).is_err());

        // Two memberships for one resource.
        let duplicated = map(
            vec![ObservationCoverage {
                domain: root.clone(),
                status: CoverageStatus::Complete,
            }],
            vec![
                ResourceCoverageMembership {
                    resource_id: ResourceId::parse("res_1").unwrap(),
                    domain: root.clone(),
                },
                ResourceCoverageMembership {
                    resource_id: ResourceId::parse("res_1").unwrap(),
                    domain: root.clone(),
                },
            ],
        );
        assert!(duplicated.validate(&[]).is_err());

        // A well-formed map validates, gap and all.
        let good = map(
            vec![ObservationCoverage {
                domain: domain("core.filesystem", "sub"),
                status: CoverageStatus::Incomplete {
                    gap_ids: vec![gap.gap_id.clone()],
                },
            }],
            vec![],
        );
        good.validate(std::slice::from_ref(&gap)).unwrap();
    }

    fn mechanism(revision: u32) -> EffectiveObservationMechanism {
        EffectiveObservationMechanism::Engine {
            engine: EngineId::ResourceEnumeration,
            engine_revision: revision,
            engine_config_digest: "sha256:config".into(),
            request_schema_digest: "sha256:req".into(),
            response_schema_digest: "sha256:res".into(),
            coverage_domain_semantics_digest: "sha256:domains".into(),
        }
    }

    #[test]
    fn context_identity_follows_effective_semantics_not_package_bytes() {
        let make = |revision| {
            ObservationContext::build(
                vec![AdapterObservationBinding {
                    binding_id: binding("core.filesystem"),
                    contribution_id: "core.filesystem".into(),
                    contribution_semantics_digest: "sha256:semantics".into(),
                    mechanism: mechanism(revision),
                }],
                vec![ViewRuleBinding {
                    binding_id: ViewRuleBindingId("draft.software.project".into()),
                    contribution_id: "view-rules".into(),
                    contribution_semantics_digest: "sha256:view".into(),
                }],
            )
        };
        assert_eq!(make(1).context_digest, make(1).context_digest);
        // A changed engine revision is a changed observer, even though the
        // declaration is byte-identical.
        assert_ne!(make(1).context_digest, make(2).context_digest);
    }

    #[test]
    fn context_identity_is_installation_order_independent() {
        let first = AdapterObservationBinding {
            binding_id: binding("a.adapter"),
            contribution_id: "a".into(),
            contribution_semantics_digest: "sha256:a".into(),
            mechanism: mechanism(1),
        };
        let second = AdapterObservationBinding {
            binding_id: binding("z.adapter"),
            contribution_id: "z".into(),
            contribution_semantics_digest: "sha256:z".into(),
            mechanism: mechanism(1),
        };
        let forwards = ObservationContext::build(vec![first.clone(), second.clone()], vec![]);
        let backwards = ObservationContext::build(vec![second, first], vec![]);
        assert_eq!(forwards.context_digest, backwards.context_digest);
    }

    #[test]
    fn a_view_rule_binding_structurally_has_no_mechanism() {
        let encoded = serde_json::to_value(ViewRuleBinding {
            binding_id: ViewRuleBindingId("draft.software.project".into()),
            contribution_id: "view-rules".into(),
            contribution_semantics_digest: "sha256:view".into(),
        })
        .unwrap();
        let keys: Vec<&str> = encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert!(!keys.contains(&"mechanism"));
        assert!(!keys.contains(&"executable_identity"));
    }

    fn run(
        id: &str,
        committed: Vec<CoverageDomainRef>,
        attempted: Vec<CoverageDomainRef>,
        outcome: ObservationRunOutcome,
    ) -> ObservationRun {
        ObservationRun {
            run_id: ObservationRunId(id.into()),
            binding_id: binding("core.filesystem"),
            effective_binding_digest: "sha256:binding".into(),
            observation_provider: ObservationProvider::Core {
                component: "filesystem".into(),
                implementation_revision: 1,
            },
            attempted_domains: attempted,
            committed_domains: committed,
            authorization_decisions: vec![],
            executable_identity: None,
            environment_metadata_digest: None,
            operation_ids: vec![],
            started_at: now(),
            completed_at: now(),
            outcome,
        }
    }

    #[test]
    fn a_core_observer_never_carries_an_extension_producer() {
        let observed_by = ObservationProvider::Core {
            component: "filesystem".into(),
            implementation_revision: 1,
        };
        assert!(observed_by.producer().is_none());
        let encoded = serde_json::to_value(&observed_by).unwrap();
        let keys: Vec<&str> = encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for forbidden in ["producer", "artifact_attestation_digest"] {
            assert!(
                !keys.contains(&forbidden),
                "Core provenance must not carry {forbidden}"
            );
        }
    }

    #[test]
    fn retried_attempts_may_overlap_while_committed_domains_partition_exactly_once() {
        let sub = domain("core.filesystem", "sub");
        let root = domain("core.filesystem", "root");
        let gap = ObservationGap::new(
            ObservationGapKind::EnumerationFailed,
            "adapter.enumeration_failed",
            vec![sub.clone()],
            None,
            "transient failure",
        );
        // The first attempt failed over `sub`; the retry succeeded. Both
        // attempted it, only the retry owns the result, and the failure survives
        // in the record.
        let failed = run(
            "run_1",
            vec![],
            vec![sub.clone()],
            ObservationRunOutcome::Failed {
                gap_ids: vec![gap.gap_id.clone()],
            },
        );
        let retried = run(
            "run_2",
            vec![sub.clone(), root.clone()],
            vec![sub.clone(), root.clone()],
            ObservationRunOutcome::Succeeded,
        );
        let observed = map(
            vec![
                ObservationCoverage {
                    domain: root,
                    status: CoverageStatus::Complete,
                },
                ObservationCoverage {
                    domain: sub,
                    status: CoverageStatus::Complete,
                },
            ],
            vec![],
        );
        let provenance = ObservationRunProvenance::build(
            "sha256:snapshot",
            None,
            vec![failed, retried],
            vec![],
            now(),
        );
        provenance.validate(&observed).unwrap();
        assert_eq!(provenance.runs.len(), 2, "the failed attempt is retained");
    }

    #[test]
    fn two_runs_cannot_own_the_same_final_domain() {
        let root = domain("core.filesystem", "root");
        let observed = map(
            vec![ObservationCoverage {
                domain: root.clone(),
                status: CoverageStatus::Complete,
            }],
            vec![],
        );
        let provenance = ObservationRunProvenance::build(
            "sha256:snapshot",
            None,
            vec![
                run(
                    "run_1",
                    vec![root.clone()],
                    vec![root.clone()],
                    ObservationRunOutcome::Succeeded,
                ),
                run(
                    "run_2",
                    vec![root.clone()],
                    vec![root],
                    ObservationRunOutcome::Succeeded,
                ),
            ],
            vec![],
            now(),
        );
        assert!(provenance.validate(&observed).is_err());
    }

    #[test]
    fn the_same_snapshot_digest_admits_several_immutable_provenance_records() {
        let root = domain("core.filesystem", "root");
        let observed = map(
            vec![ObservationCoverage {
                domain: root.clone(),
                status: CoverageStatus::Complete,
            }],
            vec![],
        );
        let first = ObservationRunProvenance::build(
            "sha256:snapshot",
            None,
            vec![run(
                "run_1",
                vec![root.clone()],
                vec![root.clone()],
                ObservationRunOutcome::Succeeded,
            )],
            vec![],
            Timestamp::from_timestamp_nanos(1_000),
        );
        let second = ObservationRunProvenance::build(
            "sha256:snapshot",
            None,
            vec![run(
                "run_2",
                vec![root.clone()],
                vec![root],
                ObservationRunOutcome::Succeeded,
            )],
            vec![],
            Timestamp::from_timestamp_nanos(2_000),
        );
        first.validate(&observed).unwrap();
        second.validate(&observed).unwrap();
        // Same authoritative state, two distinct historical observations.
        assert_eq!(first.snapshot_digest, second.snapshot_digest);
        assert_ne!(first.provenance_digest, second.provenance_digest);
    }
}
