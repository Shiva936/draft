//! The three canonical roots a Baseline is built from.
//!
//! | Root | Answers |
//! |---|---|
//! | [`ProjectStateRoot`] | what material state is accepted? |
//! | [`StateEvidenceRoot`] | what exact provenance establishes the entries that exist? |
//! | [`CoverageEvidenceRoot`] | what exact coverage claims justify absence? |
//!
//! # Invalid combinations are unrepresentable, not merely rejected
//!
//! [`BaselineStateEvidenceEntry`] is a sum type rather than a struct with
//! optional fields. A Resource subject **cannot structurally carry** relation
//! evidence, a Relation subject cannot carry an observation, and a Resource has
//! **exactly one** primary observation because the type says so. Enforcing
//! those by validation would leave a window in which an irrelevant piece of
//! evidence could reach the root and change `BaselineId`; enforcing them by
//! construction closes it.
//!
//! # Primary and corroborating are disjoint
//!
//! Duplicating the primary observation into the corroborating set would change
//! `StateEvidenceRoot`, and so `BaselineId`, while adding no evidence
//! whatsoever. Construction rejects the overlap.
//!
//! # The bijection
//!
//! Every material entry in the state root has **exactly one** evidence entry,
//! and the evidence root contains no subject absent from the state root. State
//! without provenance and provenance for state nobody accepted are both
//! refused.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::coverage::CoverageEvidence;
use crate::digest::{domain_hash, Digest};
use crate::ids::ResourceId;
use crate::merkle::merkle_root;
use crate::observation::ObservationRef;
use crate::relation::{RelationRecordDigest, RelationStateDigest, StateBearingDeclarationDigest};
use crate::state::ResourceStateDigest;
use crate::{FormatError, FormatResult};

/// The frozen domain separator for the project state root.
pub const PROJECT_STATE_ROOT_DOMAIN: &str = "draft.dcg.project-state-root/v1";
/// The frozen sub-domain for the resource half of the project state root.
pub const PROJECT_RESOURCE_ROOT_DOMAIN: &str = "draft.dcg.project-state-root.resources/v1";
/// The frozen sub-domain for the relation half of the project state root.
pub const PROJECT_RELATION_ROOT_DOMAIN: &str = "draft.dcg.project-state-root.relations/v1";
/// The frozen domain separator for the state evidence root.
pub const STATE_EVIDENCE_ROOT_DOMAIN: &str = "draft.dcg.state-evidence-root/v1";
/// The frozen domain separator for the coverage evidence root.
pub const COVERAGE_EVIDENCE_ROOT_DOMAIN: &str = "draft.dcg.coverage-evidence-root/v1";

/// One accepted Resource state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectResourceStateEntry {
    pub resource_id: ResourceId,
    pub state: ResourceStateDigest,
}

/// One accepted state-bearing Relation state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRelationStateEntry {
    pub state: RelationStateDigest,
}

/// Declares a canonical root digest.
macro_rules! root_digest {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Digest);

        impl $name {
            pub fn new(digest: Digest) -> Self {
                Self(digest)
            }

            pub fn digest(&self) -> &Digest {
                &self.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

root_digest!(
    /// What material state a Baseline accepts.
    ProjectStateRoot);
root_digest!(
    /// What exact provenance establishes that state.
    StateEvidenceRoot);
root_digest!(
    /// What exact coverage claims justify absence and completeness.
    CoverageEvidenceRoot);

/// Accumulates accepted material state, enforcing the duplicate rules.
#[derive(Debug, Default, Clone)]
pub struct ProjectStateRootBuilder {
    resources: BTreeMap<ResourceId, ResourceStateDigest>,
    relations: BTreeSet<RelationStateDigest>,
}

impl ProjectStateRootBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept one Resource state.
    ///
    /// Re-inserting the identical state is idempotent. Inserting a *different*
    /// state for the same Resource is a hard construction error: a Resource has
    /// one accepted state, and silently keeping either one would make the root
    /// depend on insertion order.
    pub fn insert_resource(
        &mut self,
        resource_id: ResourceId,
        state: ResourceStateDigest,
    ) -> FormatResult<()> {
        match self.resources.get(&resource_id) {
            Some(existing) if *existing == state => Ok(()),
            Some(existing) => Err(FormatError::Consistency(format!(
                "resource '{resource_id}' already has accepted state {existing}, which conflicts \
                 with {state}"
            ))),
            None => {
                self.resources.insert(resource_id, state);
                Ok(())
            }
        }
    }

    /// Accept one state-bearing Relation state.
    ///
    /// Idempotent: identical canonical state fields are one logical edge.
    pub fn insert_relation(&mut self, state: RelationStateDigest) {
        self.relations.insert(state);
    }

    /// The subjects this state root will contain, for the bijection check.
    pub fn subjects(&self) -> BTreeSet<EvidenceSubject> {
        self.resources
            .keys()
            .map(|resource| EvidenceSubject::Resource(resource.clone()))
            .chain(
                self.relations
                    .iter()
                    .map(|state| EvidenceSubject::Relation(state.clone())),
            )
            .collect()
    }

    /// Whether a Resource is accepted with exactly this state.
    pub fn accepts_resource(&self, resource_id: &ResourceId, state: &ResourceStateDigest) -> bool {
        self.resources.get(resource_id) == Some(state)
    }

    /// Whether a Relation state is accepted.
    pub fn accepts_relation(&self, state: &RelationStateDigest) -> bool {
        self.relations.contains(state)
    }

    /// Compute the root.
    pub fn build(&self) -> FormatResult<ProjectStateRoot> {
        let resource_leaves = self
            .resources
            .iter()
            .map(|(resource_id, state)| {
                crate::canonical::canonical_bytes(&ProjectResourceStateEntry {
                    resource_id: resource_id.clone(),
                    state: state.clone(),
                })
            })
            .collect::<FormatResult<Vec<_>>>()?;
        let relation_leaves = self
            .relations
            .iter()
            .map(|state| {
                crate::canonical::canonical_bytes(&ProjectRelationStateEntry {
                    state: state.clone(),
                })
            })
            .collect::<FormatResult<Vec<_>>>()?;

        let resource_root = merkle_root(PROJECT_RESOURCE_ROOT_DOMAIN, &resource_leaves);
        let relation_root = merkle_root(PROJECT_RELATION_ROOT_DOMAIN, &relation_leaves);
        Ok(ProjectStateRoot(domain_hash(
            PROJECT_STATE_ROOT_DOMAIN,
            [
                resource_root.as_str().as_bytes(),
                relation_root.as_str().as_bytes(),
            ],
        )))
    }
}

/// What a piece of evidence is about.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EvidenceSubject {
    Resource(ResourceId),
    Relation(RelationStateDigest),
}

impl std::fmt::Display for EvidenceSubject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resource(resource) => write!(formatter, "resource '{resource}'"),
            Self::Relation(state) => write!(formatter, "relation state {state}"),
        }
    }
}

/// How a Relation's state-bearing status is proved.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "evidence", rename_all = "snake_case")]
pub enum RelationStateEvidenceRef {
    /// An authoritative record observed the edge directly.
    RelationRecord { record: RelationRecordDigest },
    /// A derived edge was promoted by an authorized declaration.
    StateBearingDeclaration {
        declaration: StateBearingDeclarationDigest,
    },
}

/// One evidence entry, typed so irrelevant evidence is unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "subject", rename_all = "snake_case")]
pub enum BaselineStateEvidenceEntry {
    /// Evidence for one Resource's accepted state.
    Resource {
        resource_id: ResourceId,
        state: ResourceStateDigest,
        /// Exactly one primary observation — structurally, not by validation.
        primary: ObservationRef,
        /// Further observations that prove the same exact state. Disjoint from
        /// `primary`.
        corroborating: BTreeSet<ObservationRef>,
    },
    /// Evidence that one Relation state is state-bearing.
    Relation {
        state: RelationStateDigest,
        evidence: BTreeSet<RelationStateEvidenceRef>,
    },
}

impl BaselineStateEvidenceEntry {
    /// What this entry is about.
    pub fn subject(&self) -> EvidenceSubject {
        match self {
            Self::Resource { resource_id, .. } => EvidenceSubject::Resource(resource_id.clone()),
            Self::Relation { state, .. } => EvidenceSubject::Relation(state.clone()),
        }
    }

    /// Enforce the rules the type cannot express.
    pub fn validate(&self) -> FormatResult<()> {
        match self {
            Self::Resource {
                resource_id,
                primary,
                corroborating,
                ..
            } => {
                if corroborating.contains(primary) {
                    return Err(FormatError::Consistency(format!(
                        "resource '{resource_id}' lists its primary observation '{}' as \
                         corroborating; duplicating it would change the evidence root without \
                         adding evidence",
                        primary.id
                    )));
                }
                Ok(())
            }
            Self::Relation { state, evidence } => {
                if evidence.is_empty() {
                    return Err(FormatError::Consistency(format!(
                        "relation state {state} is accepted with no evidence that it is \
                         state-bearing"
                    )));
                }
                Ok(())
            }
        }
    }
}

/// Accumulates evidence entries, enforcing subject uniqueness.
#[derive(Debug, Default, Clone)]
pub struct StateEvidenceRootBuilder {
    entries: BTreeMap<EvidenceSubject, BaselineStateEvidenceEntry>,
}

impl StateEvidenceRootBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one evidence entry.
    ///
    /// An exact duplicate is idempotent; a second, different entry for the same
    /// subject is a hard error.
    pub fn insert(&mut self, entry: BaselineStateEvidenceEntry) -> FormatResult<()> {
        entry.validate()?;
        let subject = entry.subject();
        match self.entries.get(&subject) {
            Some(existing) if *existing == entry => Ok(()),
            Some(_) => Err(FormatError::Consistency(format!(
                "{subject} already has a different evidence entry"
            ))),
            None => {
                self.entries.insert(subject, entry);
                Ok(())
            }
        }
    }

    /// The subjects this evidence root covers.
    pub fn subjects(&self) -> BTreeSet<EvidenceSubject> {
        self.entries.keys().cloned().collect()
    }

    /// Check the bijection against the accepted state, and that every entry
    /// describes state the project actually accepts.
    pub fn verify_bijection(&self, state: &ProjectStateRootBuilder) -> FormatResult<()> {
        let accepted = state.subjects();
        let evidenced = self.subjects();

        if let Some(unevidenced) = accepted.difference(&evidenced).next() {
            return Err(FormatError::Consistency(format!(
                "{unevidenced} is accepted state with no evidence entry"
            )));
        }
        if let Some(unaccepted) = evidenced.difference(&accepted).next() {
            return Err(FormatError::Consistency(format!(
                "{unaccepted} has an evidence entry but is not accepted state"
            )));
        }

        // The subject matching above is not enough: a Resource entry could name
        // an accepted resource while claiming evidence for a state that
        // resource does not hold.
        for entry in self.entries.values() {
            match entry {
                BaselineStateEvidenceEntry::Resource {
                    resource_id,
                    state: evidenced_state,
                    ..
                } => {
                    if !state.accepts_resource(resource_id, evidenced_state) {
                        return Err(FormatError::Consistency(format!(
                            "evidence claims resource '{resource_id}' is in state \
                             {evidenced_state}, which is not the accepted state"
                        )));
                    }
                }
                BaselineStateEvidenceEntry::Relation {
                    state: evidenced_state,
                    ..
                } => {
                    if !state.accepts_relation(evidenced_state) {
                        return Err(FormatError::Consistency(format!(
                            "evidence describes relation state {evidenced_state}, which is not \
                             accepted state"
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Compute the root.
    pub fn build(&self) -> FormatResult<StateEvidenceRoot> {
        let leaves = self
            .entries
            .values()
            .map(crate::canonical::canonical_bytes)
            .collect::<FormatResult<Vec<_>>>()?;
        Ok(StateEvidenceRoot(merkle_root(
            STATE_EVIDENCE_ROOT_DOMAIN,
            &leaves,
        )))
    }
}

/// Accumulates coverage claims, enforcing one claim per binding, definition and
/// domain.
#[derive(Debug, Default, Clone)]
pub struct CoverageEvidenceRootBuilder {
    claims: BTreeMap<String, CoverageEvidence>,
}

impl CoverageEvidenceRootBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one coverage claim.
    ///
    /// The claim's cross-field rules are enforced here, so an incoherent claim
    /// can never reach a root. An exact duplicate is idempotent; a conflicting
    /// claim for the same key is a hard error.
    pub fn insert(&mut self, claim: CoverageEvidence) -> FormatResult<()> {
        claim.validate()?;
        let (binding, definition, domain) = claim.key();
        let key = format!("{binding}\u{0}{definition}\u{0}{domain}");
        match self.claims.get(&key) {
            Some(existing) if *existing == claim => Ok(()),
            Some(_) => Err(FormatError::Consistency(format!(
                "coverage for binding '{binding}' domain '{domain}' already has a different claim"
            ))),
            None => {
                self.claims.insert(key, claim);
                Ok(())
            }
        }
    }

    /// Compute the root.
    pub fn build(&self) -> FormatResult<CoverageEvidenceRoot> {
        let leaves = self
            .claims
            .values()
            .map(crate::canonical::canonical_bytes)
            .collect::<FormatResult<Vec<_>>>()?;
        Ok(CoverageEvidenceRoot(merkle_root(
            COVERAGE_EVIDENCE_ROOT_DOMAIN,
            &leaves,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::{CoverageDomainRef, CoverageStatus};
    use crate::ids::{ObservationId, ObservationRunId, ProviderBindingId};
    use crate::observation::{ObservationDigest, ObservationRunDigest, ObservationRunRef};
    use crate::provider::ProviderSemanticDefinitionDigest;

    fn resource(name: &str) -> ResourceId {
        ResourceId::parse(format!("res_{name}")).unwrap()
    }

    fn state(seed: &[u8]) -> ResourceStateDigest {
        ResourceStateDigest::new(Digest::of_bytes(seed))
    }

    fn relation_state(seed: &[u8]) -> RelationStateDigest {
        RelationStateDigest::new(Digest::of_bytes(seed))
    }

    fn observation(name: &str, seed: &[u8]) -> ObservationRef {
        ObservationRef {
            id: ObservationId::parse(format!("obs_{name}")).unwrap(),
            digest: ObservationDigest::new(Digest::of_bytes(seed)),
        }
    }

    fn resource_entry() -> BaselineStateEvidenceEntry {
        BaselineStateEvidenceEntry::Resource {
            resource_id: resource("a"),
            state: state(b"state-a"),
            primary: observation("primary", b"p"),
            corroborating: BTreeSet::new(),
        }
    }

    fn state_builder() -> ProjectStateRootBuilder {
        let mut builder = ProjectStateRootBuilder::new();
        builder
            .insert_resource(resource("a"), state(b"state-a"))
            .unwrap();
        builder
    }

    fn evidence_builder() -> StateEvidenceRootBuilder {
        let mut builder = StateEvidenceRootBuilder::new();
        builder.insert(resource_entry()).unwrap();
        builder
    }

    #[test]
    fn re_accepting_identical_state_is_idempotent() {
        let mut builder = ProjectStateRootBuilder::new();
        builder.insert_resource(resource("a"), state(b"s")).unwrap();
        builder.insert_resource(resource("a"), state(b"s")).unwrap();
        assert_eq!(builder.subjects().len(), 1);
    }

    #[test]
    fn a_resource_cannot_hold_two_accepted_states() {
        let mut builder = ProjectStateRootBuilder::new();
        builder
            .insert_resource(resource("a"), state(b"one"))
            .unwrap();
        let error = builder
            .insert_resource(resource("a"), state(b"two"))
            .unwrap_err();
        assert!(matches!(error, FormatError::Consistency(_)), "{error}");
    }

    #[test]
    fn identical_relation_states_are_one_logical_edge() {
        let mut builder = ProjectStateRootBuilder::new();
        builder.insert_relation(relation_state(b"edge"));
        builder.insert_relation(relation_state(b"edge"));
        assert_eq!(builder.subjects().len(), 1);
    }

    #[test]
    fn insertion_order_does_not_change_the_state_root() {
        let mut forward = ProjectStateRootBuilder::new();
        forward.insert_resource(resource("a"), state(b"1")).unwrap();
        forward.insert_resource(resource("b"), state(b"2")).unwrap();
        let mut reversed = ProjectStateRootBuilder::new();
        reversed
            .insert_resource(resource("b"), state(b"2"))
            .unwrap();
        reversed
            .insert_resource(resource("a"), state(b"1"))
            .unwrap();
        assert_eq!(forward.build().unwrap(), reversed.build().unwrap());
    }

    #[test]
    fn an_empty_project_has_a_defined_state_root() {
        let empty = ProjectStateRootBuilder::new().build().unwrap();
        assert_ne!(empty, state_builder().build().unwrap());
    }

    #[test]
    fn a_primary_observation_may_not_also_corroborate() {
        let primary = observation("primary", b"p");
        let entry = BaselineStateEvidenceEntry::Resource {
            resource_id: resource("a"),
            state: state(b"state-a"),
            primary: primary.clone(),
            corroborating: BTreeSet::from([primary]),
        };
        let error = entry.validate().unwrap_err();
        assert!(matches!(error, FormatError::Consistency(_)), "{error}");
    }

    #[test]
    fn genuine_corroboration_is_accepted_and_changes_the_root() {
        let bare = evidence_builder().build().unwrap();
        let mut corroborated = StateEvidenceRootBuilder::new();
        corroborated
            .insert(BaselineStateEvidenceEntry::Resource {
                resource_id: resource("a"),
                state: state(b"state-a"),
                primary: observation("primary", b"p"),
                corroborating: BTreeSet::from([
                    observation("second", b"q"),
                    observation("third", b"r"),
                ]),
            })
            .unwrap();
        assert_ne!(bare, corroborated.build().unwrap());
    }

    #[test]
    fn a_relation_entry_must_prove_why_it_is_state_bearing() {
        let unevidenced = BaselineStateEvidenceEntry::Relation {
            state: relation_state(b"edge"),
            evidence: BTreeSet::new(),
        };
        assert!(unevidenced.validate().is_err());

        let evidenced = BaselineStateEvidenceEntry::Relation {
            state: relation_state(b"edge"),
            evidence: BTreeSet::from([RelationStateEvidenceRef::RelationRecord {
                record: RelationRecordDigest::new(Digest::of_bytes(b"record")),
            }]),
        };
        evidenced.validate().unwrap();
    }

    #[test]
    fn the_bijection_refuses_state_without_evidence() {
        let mut state = state_builder();
        state
            .insert_resource(resource("b"), state_digest())
            .unwrap();
        let error = evidence_builder().verify_bijection(&state).unwrap_err();
        assert!(matches!(error, FormatError::Consistency(_)), "{error}");
    }

    fn state_digest() -> ResourceStateDigest {
        state(b"state-b")
    }

    #[test]
    fn the_bijection_refuses_evidence_for_unaccepted_state() {
        let mut evidence = evidence_builder();
        evidence
            .insert(BaselineStateEvidenceEntry::Resource {
                resource_id: resource("ghost"),
                state: state(b"phantom"),
                primary: observation("primary", b"p"),
                corroborating: BTreeSet::new(),
            })
            .unwrap();
        assert!(evidence.verify_bijection(&state_builder()).is_err());
    }

    #[test]
    fn evidence_must_describe_the_state_actually_accepted() {
        // Same subject on both sides, so subject matching passes — and the
        // evidence still describes a state the project does not accept.
        let mut evidence = StateEvidenceRootBuilder::new();
        evidence
            .insert(BaselineStateEvidenceEntry::Resource {
                resource_id: resource("a"),
                state: state(b"a-different-state"),
                primary: observation("primary", b"p"),
                corroborating: BTreeSet::new(),
            })
            .unwrap();
        let error = evidence.verify_bijection(&state_builder()).unwrap_err();
        assert!(matches!(error, FormatError::Consistency(_)), "{error}");
    }

    #[test]
    fn a_complete_bijection_verifies() {
        evidence_builder()
            .verify_bijection(&state_builder())
            .unwrap();
    }

    #[test]
    fn one_subject_cannot_have_two_different_evidence_entries() {
        let mut builder = evidence_builder();
        builder.insert(resource_entry()).unwrap();
        let conflicting = BaselineStateEvidenceEntry::Resource {
            resource_id: resource("a"),
            state: state(b"state-a"),
            primary: observation("other", b"z"),
            corroborating: BTreeSet::new(),
        };
        assert!(builder.insert(conflicting).is_err());
    }

    fn coverage(domain: &str, status: CoverageStatus) -> CoverageEvidence {
        CoverageEvidence {
            provider_binding: ProviderBindingId::parse("pbd_a1").unwrap(),
            provider_semantic_definition: ProviderSemanticDefinitionDigest::new(Digest::of_bytes(
                b"SD1",
            )),
            domain: CoverageDomainRef::parse(domain).unwrap(),
            status,
            observation_run: Some(ObservationRunRef {
                id: ObservationRunId::parse("run_a1").unwrap(),
                digest: ObservationRunDigest::new(Digest::of_bytes(b"run")),
            }),
            attempted: true,
            committed: true,
            known_gaps: BTreeSet::new(),
        }
    }

    #[test]
    fn an_incoherent_coverage_claim_never_reaches_a_root() {
        let mut builder = CoverageEvidenceRootBuilder::new();
        let mut broken = coverage("root", CoverageStatus::Complete);
        broken.committed = false;
        assert!(builder.insert(broken).is_err());
    }

    #[test]
    fn stronger_coverage_of_identical_state_changes_the_coverage_root() {
        // The property that makes coverage part of Baseline identity: same
        // material state, better knowledge of it, different accepted node.
        let mut complete = CoverageEvidenceRootBuilder::new();
        complete
            .insert(coverage("root", CoverageStatus::Complete))
            .unwrap();

        let mut unobserved = CoverageEvidenceRootBuilder::new();
        let mut claim = coverage("root", CoverageStatus::NotObserved);
        claim.attempted = false;
        claim.committed = false;
        claim.observation_run = None;
        unobserved.insert(claim).unwrap();

        assert_ne!(complete.build().unwrap(), unobserved.build().unwrap());
    }

    #[test]
    fn conflicting_coverage_for_one_key_is_refused() {
        let mut builder = CoverageEvidenceRootBuilder::new();
        builder
            .insert(coverage("root", CoverageStatus::Complete))
            .unwrap();
        builder
            .insert(coverage("root", CoverageStatus::Complete))
            .unwrap();
        let mut different = coverage("root", CoverageStatus::Incomplete);
        different
            .known_gaps
            .insert(crate::coverage::ObservationGapRef::new(Digest::of_bytes(
                b"gap",
            )));
        assert!(builder.insert(different).is_err());
    }

    #[test]
    fn the_roots_are_in_different_domains() {
        let state = state_builder().build().unwrap();
        let evidence = evidence_builder().build().unwrap();
        assert_ne!(state.digest(), evidence.digest());
    }
}
