//! The authoritative observation set, and the three roots built from it.
//!
//! An observation is a claim by one binding, at one moment, that a Resource
//! was in a particular state. A Baseline is built from a *set* of such claims —
//! exactly one authoritative claim per Resource, plus the coverage claims that
//! justify every Resource absent from it.
//!
//! ```text
//! ObservationRun    one enumeration attempt: what it tried, what it committed
//! Observation       one Resource's state, and which binding established it
//! CoverageEvidence  what a binding claims about a whole domain
//!         ↓
//! ProjectStateRoot  StateEvidenceRoot  CoverageEvidenceRoot
//! ```
//!
//! # Why a run is separate from its observations
//!
//! The run records what enumeration *attempted* against what it *committed*.
//! That difference is the whole of what a failed enumeration looks like: an
//! observation set that is silently short is indistinguishable from a project
//! that is genuinely smaller, and only the run says which happened.
//!
//! So a run that attempted a domain and did not commit it is `Partial`, and
//! the coverage evidence for that domain says `NotObserved` with `attempted`
//! set. Absence then has an owner.
//!
//! # Why exactly one observation is authoritative per Resource
//!
//! `StateEvidenceRoot` binds one *primary* observation per Resource, with
//! corroborating observations beside it. Two observations disagreeing about
//! the same Resource is not a state Draft resolves by picking one — that would
//! silently discard a claim — so composition refuses it and the conflict is
//! surfaced.
//!
//! # Why nothing here invents a run
//!
//! `CoverageEvidence.observation_run` is `None` only when nothing was
//! attempted. Filling it with a synthetic run to make a field non-empty would
//! be fabricating provenance for an enumeration that never happened, and that
//! fabrication would end up inside a `BaselineId` that promotions cite.

use std::collections::BTreeMap;

use draft_dcg_contract::coverage::{CoverageEvidence, CoverageStatus};
use draft_dcg_contract::ids::{ObservationId, ObservationRunId, ResourceId};
use draft_dcg_contract::observation::{Observation, ObservationRun, ObservationRunRef};
use draft_dcg_contract::roots::{
    BaselineStateEvidenceEntry, CoverageEvidenceRoot, CoverageEvidenceRootBuilder,
    ProjectStateRoot, ProjectStateRootBuilder, StateEvidenceRoot, StateEvidenceRootBuilder,
};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::immutable_store::{ImmutableFactStore, StoreOutcome};

fn format_error(error: draft_dcg_contract::FormatError) -> DraftError {
    DraftError::new(DraftErrorKind::CorruptData, error.to_string())
}

/// Observations and the runs that produced them, written once.
#[derive(Debug, Clone)]
pub struct ObservationStore {
    observations: ImmutableFactStore<Observation>,
    runs: ImmutableFactStore<ObservationRun>,
}

impl ObservationStore {
    /// Open the store over `observations/`.
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        let directory = directory.into();
        Self {
            observations: ImmutableFactStore::new(directory.join("observations")),
            runs: ImmutableFactStore::new(directory.join("runs")),
        }
    }

    /// Record a run. Validated first, so an incoherent run never becomes a
    /// fact something else can cite.
    pub fn put_run(&self, run: &ObservationRun) -> DraftResult<StoreOutcome> {
        run.validate().map_err(format_error)?;
        self.runs.put(&run.id.to_string(), run)
    }

    pub fn run(&self, id: &ObservationRunId) -> DraftResult<Option<ObservationRun>> {
        self.runs.get(&id.to_string())
    }

    /// Record an observation, requiring the run it claims to belong to.
    ///
    /// The run is verified by exact reference: an observation naming a run
    /// whose bytes differ is claiming provenance from something other than
    /// what actually happened.
    pub fn put(&self, observation: &Observation) -> DraftResult<StoreOutcome> {
        let run = self.run(&observation.run.id)?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "observation '{}' names run '{}', which does not exist",
                    observation.id, observation.run.id
                ),
            )
        })?;
        let recomputed = run.reference().map_err(format_error)?;
        if recomputed != observation.run {
            crate::support::telemetry::Counter::ObservationRunDigestMismatches.increment();
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "observation '{}' names a run whose bytes differ from the stored run '{}'",
                    observation.id, observation.run.id
                ),
            ));
        }
        self.observations
            .put(&observation.id.to_string(), observation)
    }

    pub fn get(&self, id: &ObservationId) -> DraftResult<Option<Observation>> {
        self.observations.get(&id.to_string())
    }

    /// Resolve an exact reference: load by id, then require the digest.
    ///
    /// The §2.9 boundary. A caller holding an `ObservationRef` is relying on
    /// the exact bytes it names, so a stored observation whose canonical
    /// digest no longer matches is corruption rather than a newer version of
    /// the same fact.
    pub fn resolve(
        &self,
        reference: &draft_dcg_contract::observation::ObservationRef,
    ) -> DraftResult<Option<Observation>> {
        let Some(observation) = self.get(&reference.id)? else {
            return Ok(None);
        };
        let recomputed = observation.reference().map_err(format_error)?;
        if &recomputed != reference {
            crate::support::telemetry::Counter::ObservationDigestMismatches.increment();
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "observation '{}' no longer hashes to the digest the reference names",
                    reference.id
                ),
            ));
        }
        Ok(Some(observation))
    }
}

/// The observations a Baseline would be built from.
///
/// One primary per Resource, plus corroborating observations that prove the
/// same exact state. Corroboration is only meaningful when it agrees: an
/// observation of a *different* state is a conflict, not corroboration.
#[derive(Debug, Clone, Default)]
pub struct AuthoritativeObservations {
    primary: BTreeMap<ResourceId, Observation>,
    corroborating: BTreeMap<ResourceId, Vec<Observation>>,
}

impl AuthoritativeObservations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept an observation as this Resource's authoritative state.
    ///
    /// A second observation of the same Resource is corroborating when it
    /// establishes the identical state, and a conflict otherwise. Draft does
    /// not choose between disagreeing observations: choosing would discard a
    /// claim somebody's binding actually made.
    pub fn insert(&mut self, observation: Observation) -> DraftResult<()> {
        let resource = observation.resource.clone();
        match self.primary.get(&resource) {
            None => {
                self.primary.insert(resource, observation);
            }
            Some(existing) if existing.state == observation.state => {
                if existing.id != observation.id {
                    self.corroborating
                        .entry(resource)
                        .or_default()
                        .push(observation);
                }
            }
            Some(existing) => {
                return Err(DraftError::new(
                    DraftErrorKind::ConflictDetected,
                    format!(
                        "resource '{resource}' has two observations establishing different \
                         states ('{}' and '{}'); Draft will not choose between them",
                        existing.id, observation.id
                    ),
                ))
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.primary.is_empty()
    }

    pub fn len(&self) -> usize {
        self.primary.len()
    }

    /// The primary observation for a Resource.
    pub fn primary_for(&self, resource: &ResourceId) -> Option<&Observation> {
        self.primary.get(resource)
    }

    /// The evidence entries this set establishes, one per Resource.
    ///
    /// Exposed as well as built into the root because the root is a digest:
    /// it proves the entries have not changed and says nothing about what they
    /// were. Composition needs the entries themselves, and deriving them a
    /// second way would let the two disagree about what a Baseline accepted.
    pub fn state_evidence_entries(&self) -> DraftResult<Vec<BaselineStateEvidenceEntry>> {
        let mut entries = Vec::new();
        for (resource, observation) in &self.primary {
            let mut corroborating = std::collections::BTreeSet::new();
            for other in self.corroborating.get(resource).into_iter().flatten() {
                // Never the primary itself: duplicating it would change the
                // evidence root, and therefore the BaselineId, while adding no
                // evidence at all.
                let reference = other.reference().map_err(format_error)?;
                if reference != observation.reference().map_err(format_error)? {
                    corroborating.insert(reference);
                }
            }
            entries.push(BaselineStateEvidenceEntry::Resource {
                resource_id: resource.clone(),
                state: observation.state.clone(),
                primary: observation.reference().map_err(format_error)?,
                corroborating,
            });
        }
        Ok(entries)
    }

    /// Build the state and evidence roots from this set.
    ///
    /// Both together, because their bijection is the invariant: every state
    /// entry has exactly one evidence entry and the evidence root names no
    /// subject the state root lacks.
    pub fn build_roots(&self) -> DraftResult<(ProjectStateRoot, StateEvidenceRoot)> {
        let mut state = ProjectStateRootBuilder::new();
        let mut evidence = StateEvidenceRootBuilder::new();

        for (resource, observation) in &self.primary {
            state
                .insert_resource(resource.clone(), observation.state.clone())
                .map_err(format_error)?;
        }
        for entry in self.state_evidence_entries()? {
            evidence.insert(entry).map_err(format_error)?;
        }

        crate::dcg::baseline::build_state_and_evidence(&state, &evidence)
    }
}

/// Build the coverage root from claims that have already been made.
///
/// Takes `CoverageEvidence` values rather than deriving them, because a
/// coverage claim is an assertion by a binding about a domain and Core is not
/// in a position to invent one.
pub fn build_coverage_root(
    claims: impl IntoIterator<Item = CoverageEvidence>,
) -> DraftResult<CoverageEvidenceRoot> {
    let mut builder = CoverageEvidenceRootBuilder::new();
    for claim in claims {
        builder.insert(claim).map_err(format_error)?;
    }
    builder.build().map_err(format_error)
}

/// The coverage claim a completed domain enumeration makes.
///
/// A helper for the ordinary case, so the field combination that means
/// "enumerated this domain successfully" is written once rather than at every
/// call site where one field could be forgotten.
pub fn domain_covered(
    binding: draft_dcg_contract::ids::ProviderBindingId,
    semantic_definition: draft_dcg_contract::ProviderSemanticDefinitionDigest,
    domain: draft_dcg_contract::coverage::CoverageDomainRef,
    run: ObservationRunRef,
) -> CoverageEvidence {
    CoverageEvidence {
        provider_binding: binding,
        provider_semantic_definition: semantic_definition,
        domain,
        status: CoverageStatus::Complete,
        observation_run: Some(run),
        attempted: true,
        committed: true,
        known_gaps: Default::default(),
    }
}

/// The coverage claim an enumeration that was never attempted makes.
///
/// `observation_run` is `None`, and that is the point: there is no run,
/// because nothing ran.
pub fn domain_not_attempted(
    binding: draft_dcg_contract::ids::ProviderBindingId,
    semantic_definition: draft_dcg_contract::ProviderSemanticDefinitionDigest,
    domain: draft_dcg_contract::coverage::CoverageDomainRef,
) -> CoverageEvidence {
    CoverageEvidence {
        provider_binding: binding,
        provider_semantic_definition: semantic_definition,
        domain,
        status: CoverageStatus::NotObserved,
        observation_run: None,
        attempted: false,
        committed: false,
        known_gaps: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::coverage::CoverageDomainRef;
    use draft_dcg_contract::identifier::NamespacedId;
    use draft_dcg_contract::ids::ProviderBindingId;
    use draft_dcg_contract::observation::{ObservationStability, ObservationTerminalStatus};
    use draft_dcg_contract::producer::ProducerIdentity;
    use draft_dcg_contract::state::ResourceStateDigest;
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::{Digest, ProviderSemanticDefinitionDigest};

    fn binding() -> ProviderBindingId {
        crate::dcg::filesystem_provider::filesystem_binding_id()
    }

    fn semantics() -> ProviderSemanticDefinitionDigest {
        ProviderSemanticDefinitionDigest::new(Digest::of_bytes(b"SD1"))
    }

    fn domain(name: &str) -> CoverageDomainRef {
        CoverageDomainRef::parse(name).unwrap()
    }

    fn producer() -> ProducerIdentity {
        ProducerIdentity::new(
            NamespacedId::parse("draft.core/filesystem-observer").unwrap(),
            "1",
        )
        .unwrap()
    }

    fn run(status: ObservationTerminalStatus, committed: &[&str]) -> ObservationRun {
        ObservationRun {
            id: ObservationRunId::parse("run_000000000001").unwrap(),
            producer: producer(),
            execution: None,
            attempted_domains: [domain("root")].into_iter().collect(),
            committed_domains: committed.iter().map(|name| domain(name)).collect(),
            observation_context: Digest::of_bytes(b"context"),
            started_at: Timestamp::from_unix_nanos(0),
            completed_at: Timestamp::from_unix_nanos(1),
            terminal_status: status,
        }
    }

    fn completed_run() -> ObservationRun {
        run(ObservationTerminalStatus::Completed, &["root"])
    }

    fn resource(seed: &str) -> ResourceId {
        ResourceId::parse(format!("res_00000000000{seed}")).unwrap()
    }

    fn observation(id: &str, resource_seed: &str, state: &[u8]) -> Observation {
        Observation {
            id: ObservationId::parse(format!("obs_00000000000{id}")).unwrap(),
            resource: resource(resource_seed),
            state: ResourceStateDigest::new(Digest::of_bytes(state)),
            provider_binding: binding(),
            provider_semantic_definition: semantics(),
            stability: ObservationStability::Stable,
            observation_context: Digest::of_bytes(b"context"),
            run: completed_run().reference().unwrap(),
            execution: None,
            observed_at: Timestamp::from_unix_nanos(1),
        }
    }

    #[test]
    fn observations_compose_into_a_state_and_evidence_root_pair() {
        let mut set = AuthoritativeObservations::new();
        set.insert(observation("1", "1", b"state-1")).unwrap();
        set.insert(observation("2", "2", b"state-2")).unwrap();

        let (state_root, evidence_root) = set.build_roots().unwrap();
        assert_eq!(set.len(), 2);
        // Recomposing identical observations is deterministic; a root that
        // varied run to run could not identify anything.
        let (again_state, again_evidence) = set.build_roots().unwrap();
        assert_eq!((state_root, evidence_root), (again_state, again_evidence));
    }

    #[test]
    fn two_observations_of_the_same_state_corroborate() {
        // Same resource, same state, different observers. The second is
        // evidence beside the first, not a replacement for it.
        let mut set = AuthoritativeObservations::new();
        set.insert(observation("1", "1", b"state-1")).unwrap();
        set.insert(observation("2", "1", b"state-1")).unwrap();

        assert_eq!(set.len(), 1, "one resource, one authoritative state");
        set.build_roots().unwrap();
    }

    #[test]
    fn corroboration_never_duplicates_the_primary_reference() {
        // Re-inserting the identical observation must not add itself as its
        // own corroboration: that would change the evidence root, and so the
        // BaselineId, while adding no evidence.
        let mut once = AuthoritativeObservations::new();
        once.insert(observation("1", "1", b"state-1")).unwrap();
        let mut twice = AuthoritativeObservations::new();
        twice.insert(observation("1", "1", b"state-1")).unwrap();
        twice.insert(observation("1", "1", b"state-1")).unwrap();

        assert_eq!(once.build_roots().unwrap(), twice.build_roots().unwrap());
    }

    #[test]
    fn two_observations_of_different_states_are_a_conflict_not_a_choice() {
        // Picking one would silently discard a claim a binding actually made.
        let mut set = AuthoritativeObservations::new();
        set.insert(observation("1", "1", b"state-1")).unwrap();
        let error = set.insert(observation("2", "1", b"state-2")).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    }

    #[test]
    fn a_run_that_committed_less_than_it_attempted_cannot_claim_completion() {
        // The difference between attempted and committed is what a failed
        // enumeration looks like; a Completed status over a short set would
        // erase it.
        let directory = tempfile::tempdir().unwrap();
        let store = ObservationStore::new(directory.path());
        assert!(store
            .put_run(&run(ObservationTerminalStatus::Completed, &[]))
            .is_err());
        store
            .put_run(&run(ObservationTerminalStatus::Partial, &[]))
            .unwrap();
    }

    #[test]
    fn an_observation_naming_a_run_that_does_not_exist_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let store = ObservationStore::new(directory.path());
        let error = store.put(&observation("1", "1", b"state-1")).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn an_observation_naming_a_run_whose_bytes_differ_is_refused() {
        // Same run id, different content: the observation would be claiming
        // provenance from something other than what happened.
        let directory = tempfile::tempdir().unwrap();
        let store = ObservationStore::new(directory.path());
        store
            .put_run(&run(ObservationTerminalStatus::Partial, &[]))
            .unwrap();

        let error = store.put(&observation("1", "1", b"state-1")).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn an_observation_stored_under_a_verified_run_is_accepted_and_immutable() {
        let directory = tempfile::tempdir().unwrap();
        let store = ObservationStore::new(directory.path());
        store.put_run(&completed_run()).unwrap();
        store.put(&observation("1", "1", b"state-1")).unwrap();
        store.put(&observation("1", "1", b"state-1")).unwrap();

        // Same id, different established state: refused, because everything
        // citing this observation would otherwise change meaning.
        assert!(store.put(&observation("1", "1", b"state-2")).is_err());
    }

    #[test]
    fn an_unattempted_domain_carries_no_run_because_nothing_ran() {
        // Inventing a run to fill the field would fabricate provenance that
        // ends up inside a BaselineId.
        let claim = domain_not_attempted(binding(), semantics(), domain("records"));
        assert!(claim.observation_run.is_none());
        assert!(!claim.attempted && !claim.committed);
        build_coverage_root([claim]).unwrap();
    }

    #[test]
    fn a_covered_domain_carries_the_run_that_covered_it() {
        let claim = domain_covered(
            binding(),
            semantics(),
            domain("root"),
            completed_run().reference().unwrap(),
        );
        assert_eq!(claim.status, CoverageStatus::Complete);
        assert!(claim.attempted && claim.committed);
        build_coverage_root([claim]).unwrap();
    }

    #[test]
    fn an_empty_observation_set_still_yields_defined_roots() {
        // A project with nothing observed is a real state, and its Baseline
        // must be identifiable rather than a special case.
        let set = AuthoritativeObservations::new();
        assert!(set.is_empty());
        set.build_roots().unwrap();
        build_coverage_root([]).unwrap();
    }
}
