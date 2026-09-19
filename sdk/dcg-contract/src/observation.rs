//! Observations and observation runs — both content-bound.
//!
//! An Observation says: *this Resource was in this exact state, established by
//! this binding under this semantic definition, as part of this run*. An
//! ObservationRun is that run's provenance, independently addressable because
//! coverage evidence needs to point at it.
//!
//! Both carry exact references rather than bare ids. The reason is
//! substitution: a Baseline's evidence root names the Observations that
//! establish its state, and if those were named by id alone, the bytes beneath
//! an id could be swapped — re-attributing accepted state to a different
//! provider, a different moment, or a different stability claim — while every
//! digest still verified.
//!
//! # Stability is not state
//!
//! [`ObservationStability`] lives on the Observation and **never** enters
//! `ResourceStateDigest`. How confident an observer is about what it saw is a
//! statement about the observation, not about the thing observed. Letting it
//! into state identity would make "we became more sure" indistinguishable from
//! "it changed".

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::coverage::CoverageDomainRef;
use crate::digest::{canonical_digest, Digest};
use crate::ids::{ExecutionId, ObservationId, ObservationRunId, ProviderBindingId, ResourceId};
use crate::producer::ProducerIdentity;
use crate::provider::ProviderSemanticDefinitionDigest;
use crate::state::ResourceStateDigest;
use crate::value::Timestamp;
use crate::{FormatError, FormatResult};

/// The frozen domain separator for an observation digest.
pub const OBSERVATION_DIGEST_DOMAIN: &str = "draft.dcg.observation/v1";
/// The frozen domain separator for an observation run digest.
pub const OBSERVATION_RUN_DIGEST_DOMAIN: &str = "draft.dcg.observation-run/v1";

/// How much an observer trusts that what it saw will still be true.
///
/// Provenance quality, never material state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStability {
    /// The observation was taken under conditions that make it reproducible.
    Stable,
    /// The observation may not reproduce — the source was changing, or the
    /// observer could not fence it.
    Volatile,
    /// The observer could not establish whether it was stable.
    Unknown,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationTerminalStatus {
    /// Every attempted domain was committed.
    Completed,
    /// The run finished, but not every attempted domain was committed.
    Partial,
    /// The run failed before it could commit its results.
    Failed,
}

/// One observation of one Resource's state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub id: ObservationId,
    /// The Resource observed.
    pub resource: ResourceId,
    /// The exact state established.
    pub state: ResourceStateDigest,
    /// The binding that established it.
    pub provider_binding: ProviderBindingId,
    /// The semantic definition in force. Together with the binding this is the
    /// provenance a Baseline exposes — never an operational profile.
    pub provider_semantic_definition: ProviderSemanticDefinitionDigest,
    /// How much the observer trusts this. Not part of `ResourceStateDigest`.
    pub stability: ObservationStability,
    /// The effective observation semantics this was taken under.
    pub observation_context: Digest,
    /// The run this observation belongs to.
    pub run: ObservationRunRef,
    /// The execution that produced it, where there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionId>,
    /// When it was observed.
    ///
    /// Inside the canonical object, so it affects `ObservationDigest` and
    /// therefore `StateEvidenceRoot` and `BaselineId` — while leaving
    /// `ResourceStateDigest` untouched. Two observations of identical material
    /// state at different moments are the same state and different evidence.
    pub observed_at: Timestamp,
}

impl Observation {
    /// This observation's canonical digest.
    ///
    /// Computed over the whole canonical object. There is no self-referential
    /// digest field to exclude: the digest lives in the [`ObservationRef`] that
    /// points here, never inside the observation itself.
    pub fn digest(&self) -> FormatResult<ObservationDigest> {
        Ok(ObservationDigest(canonical_digest(
            OBSERVATION_DIGEST_DOMAIN,
            self,
        )?))
    }

    /// The exact reference to this observation.
    pub fn reference(&self) -> FormatResult<ObservationRef> {
        Ok(ObservationRef {
            id: self.id.clone(),
            digest: self.digest()?,
        })
    }
}

/// The canonical digest of an [`Observation`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObservationDigest(Digest);

impl ObservationDigest {
    pub fn new(digest: Digest) -> Self {
        Self(digest)
    }

    pub fn digest(&self) -> &Digest {
        &self.0
    }
}

impl std::fmt::Display for ObservationDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// An exact reference to one Observation: id **and** digest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationRef {
    pub id: ObservationId,
    pub digest: ObservationDigest,
}

impl ObservationRef {
    /// Verify this reference against the observation it names.
    ///
    /// Load by id, recompute, require equality. A mismatch is an integrity
    /// violation — never a warning, and never repaired in place.
    pub fn verify(&self, observation: &Observation) -> FormatResult<()> {
        if observation.id != self.id {
            return Err(FormatError::Integrity(format!(
                "observation reference names '{}' but the loaded observation is '{}'",
                self.id, observation.id
            )));
        }
        let recomputed = observation.digest()?;
        if recomputed != self.digest {
            return Err(FormatError::Integrity(format!(
                "observation '{}' was expected to be {} but its stored bytes compute to {}",
                self.id, self.digest, recomputed
            )));
        }
        Ok(())
    }
}

/// The provenance of one batch of observations and coverage claims.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationRun {
    pub id: ObservationRunId,
    /// What performed the run.
    pub producer: ProducerIdentity,
    /// The execution it ran under, where there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionId>,
    /// Which domains the run set out to enumerate.
    pub attempted_domains: BTreeSet<CoverageDomainRef>,
    /// Which domains it actually committed.
    ///
    /// Kept separate from `attempted_domains` on purpose: the difference is
    /// what a failed enumeration looks like, and collapsing them would make a
    /// failure indistinguishable from a success.
    pub committed_domains: BTreeSet<CoverageDomainRef>,
    /// The effective observation semantics the run operated under.
    pub observation_context: Digest,
    pub started_at: Timestamp,
    pub completed_at: Timestamp,
    pub terminal_status: ObservationTerminalStatus,
}

impl ObservationRun {
    /// Enforce the run's internal coherence.
    pub fn validate(&self) -> FormatResult<()> {
        // Committing a domain that was never attempted is not possible.
        if let Some(uncommitted) = self
            .committed_domains
            .difference(&self.attempted_domains)
            .next()
        {
            return Err(FormatError::Consistency(format!(
                "run '{}' committed domain '{uncommitted}' without attempting it",
                self.id
            )));
        }
        if self.completed_at.as_unix_nanos() < self.started_at.as_unix_nanos() {
            return Err(FormatError::Consistency(format!(
                "run '{}' completed before it started",
                self.id
            )));
        }
        let all_committed = self.attempted_domains == self.committed_domains;
        match self.terminal_status {
            ObservationTerminalStatus::Completed if !all_committed => {
                Err(FormatError::Consistency(format!(
                    "run '{}' reports Completed but did not commit every attempted domain",
                    self.id
                )))
            }
            ObservationTerminalStatus::Partial if all_committed => {
                Err(FormatError::Consistency(format!(
                    "run '{}' reports Partial but committed every attempted domain",
                    self.id
                )))
            }
            ObservationTerminalStatus::Failed if !self.committed_domains.is_empty() => {
                Err(FormatError::Consistency(format!(
                    "run '{}' reports Failed but committed {} domain(s)",
                    self.id,
                    self.committed_domains.len()
                )))
            }
            _ => Ok(()),
        }
    }

    /// This run's canonical digest.
    pub fn digest(&self) -> FormatResult<ObservationRunDigest> {
        Ok(ObservationRunDigest(canonical_digest(
            OBSERVATION_RUN_DIGEST_DOMAIN,
            self,
        )?))
    }

    /// The exact reference to this run.
    pub fn reference(&self) -> FormatResult<ObservationRunRef> {
        Ok(ObservationRunRef {
            id: self.id.clone(),
            digest: self.digest()?,
        })
    }
}

/// The canonical digest of an [`ObservationRun`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObservationRunDigest(Digest);

impl ObservationRunDigest {
    pub fn new(digest: Digest) -> Self {
        Self(digest)
    }

    pub fn digest(&self) -> &Digest {
        &self.0
    }
}

impl std::fmt::Display for ObservationRunDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// An exact reference to one ObservationRun: id **and** digest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationRunRef {
    pub id: ObservationRunId,
    pub digest: ObservationRunDigest,
}

impl ObservationRunRef {
    /// Verify this reference against the run it names.
    pub fn verify(&self, run: &ObservationRun) -> FormatResult<()> {
        if run.id != self.id {
            return Err(FormatError::Integrity(format!(
                "run reference names '{}' but the loaded run is '{}'",
                self.id, run.id
            )));
        }
        let recomputed = run.digest()?;
        if recomputed != self.digest {
            return Err(FormatError::Integrity(format!(
                "run '{}' was expected to be {} but its stored bytes compute to {}",
                self.id, self.digest, recomputed
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::NamespacedId;

    fn domain(name: &str) -> CoverageDomainRef {
        CoverageDomainRef::parse(name).unwrap()
    }

    fn run() -> ObservationRun {
        ObservationRun {
            id: ObservationRunId::parse("run_a1b2c3").unwrap(),
            producer: ProducerIdentity::new(
                NamespacedId::parse("draft.core/filesystem").unwrap(),
                "0.3.4",
            )
            .unwrap(),
            execution: None,
            attempted_domains: BTreeSet::from([domain("root")]),
            committed_domains: BTreeSet::from([domain("root")]),
            observation_context: Digest::of_bytes(b"context"),
            started_at: Timestamp::from_unix_nanos(1_000),
            completed_at: Timestamp::from_unix_nanos(2_000),
            terminal_status: ObservationTerminalStatus::Completed,
        }
    }

    fn observation() -> Observation {
        Observation {
            id: ObservationId::parse("obs_a1b2c3").unwrap(),
            resource: ResourceId::parse("res_a1b2c3").unwrap(),
            state: ResourceStateDigest::new(Digest::of_bytes(b"state")),
            provider_binding: ProviderBindingId::parse("pbd_a1").unwrap(),
            provider_semantic_definition: ProviderSemanticDefinitionDigest::new(Digest::of_bytes(
                b"SD1",
            )),
            stability: ObservationStability::Stable,
            observation_context: Digest::of_bytes(b"context"),
            run: run().reference().unwrap(),
            execution: None,
            observed_at: Timestamp::from_unix_nanos(1_500),
        }
    }

    #[test]
    fn a_reference_verifies_against_its_own_object() {
        let observation = observation();
        observation
            .reference()
            .unwrap()
            .verify(&observation)
            .unwrap();
        let run = run();
        run.reference().unwrap().verify(&run).unwrap();
    }

    #[test]
    fn substituting_the_bytes_under_an_id_is_detected() {
        // The attack the exact reference exists to stop: keep `obs_a1b2c3`,
        // re-attribute the accepted state to a different provider.
        let reference = observation().reference().unwrap();
        let mut substituted = observation();
        substituted.provider_binding = ProviderBindingId::parse("pbd_evil").unwrap();
        let error = reference.verify(&substituted).unwrap_err();
        assert!(matches!(error, FormatError::Integrity(_)), "{error}");
    }

    #[test]
    fn stability_changes_the_observation_but_not_the_state_it_names() {
        let confident = observation();
        let mut unsure = observation();
        unsure.stability = ObservationStability::Unknown;
        // Different evidence...
        assert_ne!(confident.digest().unwrap(), unsure.digest().unwrap());
        // ...about the very same material state.
        assert_eq!(confident.state, unsure.state);
    }

    #[test]
    fn observation_timing_changes_evidence_without_changing_state() {
        let earlier = observation();
        let mut later = observation();
        later.observed_at = Timestamp::from_unix_nanos(9_999);
        assert_ne!(earlier.digest().unwrap(), later.digest().unwrap());
        assert_eq!(earlier.state, later.state);
    }

    #[test]
    fn a_run_cannot_commit_a_domain_it_never_attempted() {
        let mut invalid = run();
        invalid.committed_domains.insert(domain("extra"));
        assert!(matches!(
            invalid.validate(),
            Err(FormatError::Consistency(_))
        ));
    }

    #[test]
    fn terminal_status_must_match_what_was_committed() {
        let mut partial = run();
        partial.attempted_domains.insert(domain("other"));
        assert!(partial.validate().is_err(), "Completed with a shortfall");
        partial.terminal_status = ObservationTerminalStatus::Partial;
        partial.validate().unwrap();

        let mut failed = run();
        failed.terminal_status = ObservationTerminalStatus::Failed;
        assert!(failed.validate().is_err(), "Failed yet committed a domain");
        failed.committed_domains.clear();
        failed.validate().unwrap();
    }

    #[test]
    fn a_run_cannot_finish_before_it_starts() {
        let mut backwards = run();
        backwards.completed_at = Timestamp::from_unix_nanos(0);
        assert!(backwards.validate().is_err());
    }

    #[test]
    fn a_reference_will_not_verify_against_another_object() {
        let reference = observation().reference().unwrap();
        let mut other = observation();
        other.id = ObservationId::parse("obs_999999").unwrap();
        assert!(reference.verify(&other).is_err());
    }

    #[test]
    fn the_wire_forms_round_trip() {
        let observation = observation();
        let encoded = serde_json::to_string(&observation).unwrap();
        assert_eq!(
            serde_json::from_str::<Observation>(&encoded).unwrap(),
            observation
        );
        let run = run();
        let encoded = serde_json::to_string(&run).unwrap();
        assert_eq!(
            serde_json::from_str::<ObservationRun>(&encoded).unwrap(),
            run
        );
    }
}
