//! Turning an enumeration into canonical observations.
//!
//! A source enumerates and reports what it saw. This is where that becomes
//! evidence: one [`ObservationRun`] recording what was attempted against what
//! was committed, and one [`Observation`] per Resource naming the binding and
//! semantic definition that established its state.
//!
//! # Why the run is written even when enumeration fails
//!
//! A run that attempted three domains and committed one is the *only* record
//! that the other two were tried. Without it, a project that lost two domains
//! to a failure is indistinguishable from a project that only ever had one,
//! and every Baseline built afterwards would silently claim the smaller set
//! was complete.
//!
//! So the run is recorded with `Partial` or `Failed`, the uncommitted domains
//! get `NotObserved` coverage with `attempted` set, and absence has an owner.
//!
//! # Why observation ids are derived, not minted
//!
//! An observation's id is derived from what it observed — the run, the
//! resource and the established state. Re-observing an unchanged Resource in a
//! new run therefore produces a genuinely new observation (the run differs),
//! while replaying the *same* run is idempotent rather than duplicating
//! evidence.
//!
//! A random id would make replay produce two observations of identical
//! content, both landing in the evidence root, changing the `BaselineId`
//! without changing what anybody knows.

use draft_dcg_contract::coverage::{CoverageDomainRef, CoverageEvidence};
use draft_dcg_contract::ids::{ObservationId, ObservationRunId, ProviderBindingId, ResourceId};
use draft_dcg_contract::observation::{
    Observation, ObservationRun, ObservationStability, ObservationTerminalStatus,
};
use draft_dcg_contract::producer::ProducerIdentity;
use draft_dcg_contract::state::ResourceStateDigest;
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::{Digest, ProviderSemanticDefinitionDigest};

use crate::dcg::observation_set::{
    domain_covered, domain_not_attempted, AuthoritativeObservations,
};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// The identity an observation run is made under.
#[derive(Debug, Clone)]
pub struct ObservingBinding {
    pub binding: ProviderBindingId,
    pub semantic_definition: ProviderSemanticDefinitionDigest,
    pub producer: ProducerIdentity,
    /// The effective observation semantics in force.
    pub observation_context: Digest,
}

/// One Resource an enumeration established.
#[derive(Debug, Clone)]
pub struct ObservedResource {
    pub resource: ResourceId,
    pub state: ResourceStateDigest,
    pub domain: CoverageDomainRef,
    pub stability: ObservationStability,
}

/// What an enumeration set out to do and what it achieved.
#[derive(Debug, Clone)]
pub struct EnumerationResult {
    /// Every domain the run tried to enumerate.
    pub attempted_domains: Vec<CoverageDomainRef>,
    /// The domains it actually finished.
    pub committed_domains: Vec<CoverageDomainRef>,
    pub resources: Vec<ObservedResource>,
    pub started_at: Timestamp,
    pub completed_at: Timestamp,
}

/// A run and the observations it produced.
#[derive(Debug, Clone)]
pub struct ObservationOutcome {
    pub run: ObservationRun,
    pub observations: Vec<Observation>,
    pub coverage: Vec<CoverageEvidence>,
}

impl ObservationOutcome {
    /// The authoritative set these observations form.
    pub fn authoritative(&self) -> DraftResult<AuthoritativeObservations> {
        let mut set = AuthoritativeObservations::new();
        for observation in &self.observations {
            set.insert(observation.clone())?;
        }
        Ok(set)
    }
}

/// The hex body of a digest, without its algorithm prefix.
///
/// Ids are `[a-z0-9-.]`, and `sha256:` is not — the prefix belongs to the
/// digest's own display form, not to anything derived from it.
fn short_hex(digest: &Digest) -> String {
    digest
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect()
}

/// Derive the run id for an enumeration.
///
/// From the binding, the context and the window, so replaying the identical
/// enumeration converges on one run rather than accumulating runs that each
/// claim to have observed the same thing.
fn run_id(
    observer: &ObservingBinding,
    result: &EnumerationResult,
) -> DraftResult<ObservationRunId> {
    let seed = Digest::of_bytes(
        format!(
            "{}|{}|{}|{}",
            observer.binding,
            observer.observation_context,
            result.started_at.as_unix_nanos(),
            result.completed_at.as_unix_nanos()
        )
        .as_bytes(),
    );
    ObservationRunId::parse(format!("run_{}", short_hex(&seed)))
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

/// Derive an observation's id from what it observed.
fn observation_id(
    run: &ObservationRunId,
    resource: &ResourceId,
    state: &ResourceStateDigest,
) -> DraftResult<ObservationId> {
    let seed = Digest::of_bytes(format!("{run}|{resource}|{}", state.digest()).as_bytes());
    ObservationId::parse(format!("obs_{}", short_hex(&seed)))
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

/// Record an enumeration as canonical observations.
pub fn record(
    observer: &ObservingBinding,
    result: &EnumerationResult,
) -> DraftResult<ObservationOutcome> {
    let attempted: std::collections::BTreeSet<_> =
        result.attempted_domains.iter().cloned().collect();
    let committed: std::collections::BTreeSet<_> =
        result.committed_domains.iter().cloned().collect();

    // The status follows from the domains, never from a caller's opinion:
    // "this succeeded" and "it committed everything it attempted" must be the
    // same statement.
    let terminal_status = if committed.is_empty() && !attempted.is_empty() {
        ObservationTerminalStatus::Failed
    } else if committed == attempted {
        ObservationTerminalStatus::Completed
    } else {
        ObservationTerminalStatus::Partial
    };

    let run = ObservationRun {
        id: run_id(observer, result)?,
        producer: observer.producer.clone(),
        execution: None,
        attempted_domains: attempted.clone(),
        committed_domains: committed.clone(),
        observation_context: observer.observation_context.clone(),
        started_at: result.started_at,
        completed_at: result.completed_at,
        terminal_status,
    };
    run.validate()
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;
    let reference = run
        .reference()
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;

    let mut observations = Vec::new();
    for observed in &result.resources {
        // A Resource in a domain the run did not commit is not established.
        // Recording it anyway would put state into a Baseline that the run
        // itself says was never finished.
        if !committed.contains(&observed.domain) {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "resource '{}' was reported in domain '{}', which the run did not commit",
                    observed.resource, observed.domain
                ),
            ));
        }
        observations.push(Observation {
            id: observation_id(&run.id, &observed.resource, &observed.state)?,
            resource: observed.resource.clone(),
            state: observed.state.clone(),
            provider_binding: observer.binding.clone(),
            provider_semantic_definition: observer.semantic_definition.clone(),
            stability: observed.stability,
            observation_context: observer.observation_context.clone(),
            run: reference.clone(),
            execution: None,
            observed_at: result.completed_at,
        });
    }

    let mut coverage = Vec::new();
    for domain in &attempted {
        coverage.push(if committed.contains(domain) {
            domain_covered(
                observer.binding.clone(),
                observer.semantic_definition.clone(),
                domain.clone(),
                reference.clone(),
            )
        } else {
            // Attempted and not committed. The run is carried, because one
            // did happen — this is the case `attempted` exists to separate
            // from "nobody looked".
            CoverageEvidence {
                provider_binding: observer.binding.clone(),
                provider_semantic_definition: observer.semantic_definition.clone(),
                domain: domain.clone(),
                status: draft_dcg_contract::coverage::CoverageStatus::NotObserved,
                observation_run: Some(reference.clone()),
                attempted: true,
                committed: false,
                known_gaps: Default::default(),
            }
        });
    }

    // Every claim is validated before it leaves this function. The cross-field
    // table is what makes a coverage claim mean anything — `attempted == false`
    // beside a run reference, or `Complete` with known gaps, would let a
    // Baseline justify an absence that nothing established.
    for claim in &coverage {
        claim.validate().map_err(|error| {
            crate::support::telemetry::Counter::CoverageEvidenceVerifyFailures.increment();
            DraftError::new(DraftErrorKind::CorruptData, error.to_string())
        })?;
    }

    Ok(ObservationOutcome {
        run,
        observations,
        coverage,
    })
}

/// The coverage claim for a domain nothing attempted.
pub fn unattempted(observer: &ObservingBinding, domain: CoverageDomainRef) -> CoverageEvidence {
    domain_not_attempted(
        observer.binding.clone(),
        observer.semantic_definition.clone(),
        domain,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::identifier::NamespacedId;

    fn observer() -> ObservingBinding {
        ObservingBinding {
            binding: crate::dcg::filesystem_provider::filesystem_binding_id(),
            semantic_definition: ProviderSemanticDefinitionDigest::new(Digest::of_bytes(b"SD1")),
            producer: ProducerIdentity::new(
                NamespacedId::parse("draft.core/filesystem-observer").unwrap(),
                "1",
            )
            .unwrap(),
            observation_context: Digest::of_bytes(b"context"),
        }
    }

    fn domain(name: &str) -> CoverageDomainRef {
        CoverageDomainRef::parse(name).unwrap()
    }

    fn resource(seed: &str) -> ResourceId {
        ResourceId::parse(format!("res_00000000000{seed}")).unwrap()
    }

    fn observed(seed: &str, state: &[u8], in_domain: &str) -> ObservedResource {
        ObservedResource {
            resource: resource(seed),
            state: ResourceStateDigest::new(Digest::of_bytes(state)),
            domain: domain(in_domain),
            stability: ObservationStability::Stable,
        }
    }

    fn enumeration(
        attempted: &[&str],
        committed: &[&str],
        resources: Vec<ObservedResource>,
    ) -> EnumerationResult {
        EnumerationResult {
            attempted_domains: attempted.iter().map(|name| domain(name)).collect(),
            committed_domains: committed.iter().map(|name| domain(name)).collect(),
            resources,
            started_at: Timestamp::from_unix_nanos(0),
            completed_at: Timestamp::from_unix_nanos(1),
        }
    }

    #[test]
    fn a_full_enumeration_completes_and_covers_every_domain() {
        let outcome = record(
            &observer(),
            &enumeration(
                &["root"],
                &["root"],
                vec![observed("1", b"state-1", "root")],
            ),
        )
        .unwrap();

        assert_eq!(
            outcome.run.terminal_status,
            ObservationTerminalStatus::Completed
        );
        assert_eq!(outcome.observations.len(), 1);
        assert_eq!(outcome.coverage.len(), 1);
        assert!(outcome.coverage[0].committed);
        outcome.authoritative().unwrap().build_roots().unwrap();
    }

    #[test]
    fn a_partial_enumeration_records_what_it_could_not_finish() {
        // Without this the project would look smaller rather than
        // incompletely observed, and the next Baseline would claim the short
        // set was everything.
        let outcome = record(
            &observer(),
            &enumeration(
                &["root", "records"],
                &["root"],
                vec![observed("1", b"state-1", "root")],
            ),
        )
        .unwrap();

        assert_eq!(
            outcome.run.terminal_status,
            ObservationTerminalStatus::Partial
        );
        let unfinished = outcome
            .coverage
            .iter()
            .find(|claim| claim.domain == domain("records"))
            .unwrap();
        assert!(unfinished.attempted && !unfinished.committed);
        assert!(
            unfinished.observation_run.is_some(),
            "a run did happen, which is what separates this from nobody looking"
        );
    }

    #[test]
    fn an_enumeration_that_committed_nothing_is_failed() {
        let outcome = record(&observer(), &enumeration(&["root"], &[], vec![])).unwrap();
        assert_eq!(
            outcome.run.terminal_status,
            ObservationTerminalStatus::Failed
        );
        assert!(outcome.observations.is_empty());
    }

    #[test]
    fn a_resource_in_an_uncommitted_domain_is_refused() {
        // Recording it would put state into a Baseline that the run itself
        // says was never finished.
        let error = record(
            &observer(),
            &enumeration(
                &["root", "records"],
                &["root"],
                vec![observed("1", b"state-1", "records")],
            ),
        )
        .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn replaying_the_identical_enumeration_is_idempotent() {
        // Derived ids, so a replay converges instead of producing a second
        // observation of identical content that would change the BaselineId
        // without changing what anybody knows.
        let first = record(
            &observer(),
            &enumeration(
                &["root"],
                &["root"],
                vec![observed("1", b"state-1", "root")],
            ),
        )
        .unwrap();
        let second = record(
            &observer(),
            &enumeration(
                &["root"],
                &["root"],
                vec![observed("1", b"state-1", "root")],
            ),
        )
        .unwrap();

        assert_eq!(first.run.id, second.run.id);
        assert_eq!(first.observations[0].id, second.observations[0].id);
        assert_eq!(
            first.authoritative().unwrap().build_roots().unwrap(),
            second.authoritative().unwrap().build_roots().unwrap()
        );
    }

    #[test]
    fn re_observing_the_same_state_in_a_new_run_is_new_evidence() {
        // Same material state, a later run. The state root is unchanged and
        // the evidence root is not: the project knows it more recently.
        let early = record(
            &observer(),
            &enumeration(
                &["root"],
                &["root"],
                vec![observed("1", b"state-1", "root")],
            ),
        )
        .unwrap();
        let mut later_window = enumeration(
            &["root"],
            &["root"],
            vec![observed("1", b"state-1", "root")],
        );
        later_window.completed_at = Timestamp::from_unix_nanos(9_999);
        let later = record(&observer(), &later_window).unwrap();

        assert_ne!(early.run.id, later.run.id);
        let (early_state, early_evidence) = early.authoritative().unwrap().build_roots().unwrap();
        let (later_state, later_evidence) = later.authoritative().unwrap().build_roots().unwrap();
        assert_eq!(early_state, later_state, "the material state is the same");
        assert_ne!(
            early_evidence, later_evidence,
            "a later observation is different evidence for it"
        );
    }

    #[test]
    fn a_domain_nobody_attempted_carries_no_run() {
        let claim = unattempted(&observer(), domain("timeline"));
        assert!(!claim.attempted);
        assert!(claim.observation_run.is_none());
    }
}
