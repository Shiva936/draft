//! Coverage evidence: proving what was *not* observed.
//!
//! An empty set of Resources is not proof that a project is empty — it is
//! equally consistent with "nothing was looked at" and "everything failed".
//! Coverage evidence is what makes those different facts, so a Baseline can
//! justify absence rather than merely omit it.
//!
//! # `attempted` is not `committed`
//!
//! The distinction is preserved deliberately and can never be collapsed: an
//! attempt that failed must never be promotable to `Complete`. Stronger
//! coverage over identical material state yields the same `ProjectStateRoot`, a
//! different `CoverageEvidenceRoot`, and therefore a different `BaselineId` —
//! knowing more about the same state is a different accepted historical node.
//!
//! There is deliberately **no `completeness_proof` field in v1**. An undefined
//! digest whose meaning nobody could state would be worse than its absence.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::digest::Digest;
use crate::identifier::ScopedId;
use crate::ids::ProviderBindingId;
use crate::observation::ObservationRunRef;
use crate::provider::ProviderSemanticDefinitionDigest;
use crate::{FormatError, FormatResult};

/// Names one coverage domain within the scope that enumerated it.
///
/// Scoped, not global: an adapter's "root" domain means nothing outside its
/// binding, and forcing such names into a shared namespace would invent
/// collisions between unrelated providers.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CoverageDomainRef(ScopedId);

impl CoverageDomainRef {
    pub fn parse(value: impl Into<String>) -> FormatResult<Self> {
        Ok(Self(ScopedId::parse(value)?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for CoverageDomainRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A reference to one recorded gap — something an observer knows it did not
/// establish.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObservationGapRef(Digest);

impl ObservationGapRef {
    pub fn new(digest: Digest) -> Self {
        Self(digest)
    }

    pub fn digest(&self) -> &Digest {
        &self.0
    }
}

impl std::fmt::Display for ObservationGapRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// How completely one domain was covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    /// The domain was enumerated in full and nothing is outstanding.
    Complete,
    /// The domain was enumerated, but known gaps remain.
    Incomplete,
    /// The domain was not established — either never attempted, or attempted
    /// and failed. Which of the two is recorded in `attempted`.
    NotObserved,
}

/// One coverage claim: what a binding established about one domain.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageEvidence {
    /// The binding that made the claim.
    pub provider_binding: ProviderBindingId,
    /// The semantic definition it was operating under.
    pub provider_semantic_definition: ProviderSemanticDefinitionDigest,
    /// The domain this claim is about.
    pub domain: CoverageDomainRef,
    /// How completely the domain was covered.
    pub status: CoverageStatus,
    /// The run that produced this claim.
    ///
    /// `None` **only** when nothing was attempted. A synthetic run invented to
    /// fill this field would be a fabricated observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_run: Option<ObservationRunRef>,
    /// Whether enumeration was attempted at all.
    pub attempted: bool,
    /// Whether the enumeration's results were committed.
    pub committed: bool,
    /// What is known to be missing, canonically ordered.
    pub known_gaps: BTreeSet<ObservationGapRef>,
}

impl CoverageEvidence {
    /// The canonical key this claim is filed under: one claim per
    /// binding, definition and domain.
    pub fn key(
        &self,
    ) -> (
        &ProviderBindingId,
        &ProviderSemanticDefinitionDigest,
        &CoverageDomainRef,
    ) {
        (
            &self.provider_binding,
            &self.provider_semantic_definition,
            &self.domain,
        )
    }

    /// Enforce the cross-field rules.
    ///
    /// Every combination not permitted here is a hard construction error, not a
    /// warning: a coverage claim that contradicts itself is exactly the thing
    /// that would let absence go unjustified.
    pub fn validate(&self) -> FormatResult<()> {
        // Nothing attempted cannot have produced a run.
        if !self.attempted && self.observation_run.is_some() {
            return Err(FormatError::Consistency(format!(
                "coverage for domain '{}' was not attempted, so it cannot name an observation run",
                self.domain
            )));
        }
        // Results cannot be committed by an enumeration that never ran.
        if self.committed && !self.attempted {
            return Err(FormatError::Consistency(format!(
                "coverage for domain '{}' claims committed results without an attempt",
                self.domain
            )));
        }
        match self.status {
            CoverageStatus::Complete => {
                if !self.attempted || !self.committed {
                    return Err(FormatError::Consistency(format!(
                        "coverage for domain '{}' claims Complete, which requires an attempted \
                         and committed enumeration",
                        self.domain
                    )));
                }
                if self.observation_run.is_none() {
                    return Err(FormatError::Consistency(format!(
                        "Complete coverage for domain '{}' must name the run that established it",
                        self.domain
                    )));
                }
                if !self.known_gaps.is_empty() {
                    return Err(FormatError::Consistency(format!(
                        "coverage for domain '{}' claims Complete while recording known gaps",
                        self.domain
                    )));
                }
            }
            CoverageStatus::Incomplete => {
                if !self.attempted {
                    return Err(FormatError::Consistency(format!(
                        "coverage for domain '{}' claims Incomplete without an attempt; \
                         unattempted coverage is NotObserved",
                        self.domain
                    )));
                }
                if self.observation_run.is_none() {
                    return Err(FormatError::Consistency(format!(
                        "Incomplete coverage for domain '{}' must name the run that established \
                         what it did",
                        self.domain
                    )));
                }
                if self.known_gaps.is_empty() {
                    return Err(FormatError::Consistency(format!(
                        "coverage for domain '{}' claims Incomplete but records no gap, so it \
                         does not say what is missing",
                        self.domain
                    )));
                }
            }
            CoverageStatus::NotObserved => {
                // Both shapes are legal and mean different things: never
                // attempted, or attempted and failed. What is not legal is
                // claiming the results were committed.
                if self.committed {
                    return Err(FormatError::Consistency(format!(
                        "coverage for domain '{}' claims NotObserved with committed results",
                        self.domain
                    )));
                }
                if self.attempted && self.observation_run.is_none() {
                    return Err(FormatError::Consistency(format!(
                        "coverage for domain '{}' records a failed attempt, so it must name the \
                         run that failed",
                        self.domain
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ObservationRunId;
    use crate::observation::ObservationRunDigest;

    fn run() -> ObservationRunRef {
        ObservationRunRef {
            id: ObservationRunId::parse("run_a1b2c3").unwrap(),
            digest: ObservationRunDigest::new(Digest::of_bytes(b"run")),
        }
    }

    fn evidence(status: CoverageStatus) -> CoverageEvidence {
        CoverageEvidence {
            provider_binding: ProviderBindingId::parse("pbd_a1").unwrap(),
            provider_semantic_definition: ProviderSemanticDefinitionDigest::new(Digest::of_bytes(
                b"SD1",
            )),
            domain: CoverageDomainRef::parse("root").unwrap(),
            status,
            observation_run: Some(run()),
            attempted: true,
            committed: true,
            known_gaps: BTreeSet::new(),
        }
    }

    fn gap() -> ObservationGapRef {
        ObservationGapRef::new(Digest::of_bytes(b"gap"))
    }

    #[test]
    fn a_complete_claim_is_attempted_committed_and_gapless() {
        evidence(CoverageStatus::Complete).validate().unwrap();
    }

    #[test]
    fn complete_coverage_may_not_hide_a_known_gap() {
        let mut claim = evidence(CoverageStatus::Complete);
        claim.known_gaps.insert(gap());
        assert!(matches!(claim.validate(), Err(FormatError::Consistency(_))));
    }

    #[test]
    fn a_failed_attempt_can_never_be_promoted_to_complete() {
        // The invariant that keeps `attempted` and `committed` distinct.
        let mut claim = evidence(CoverageStatus::Complete);
        claim.committed = false;
        assert!(claim.validate().is_err());
    }

    #[test]
    fn incomplete_coverage_must_say_what_is_missing() {
        let mut claim = evidence(CoverageStatus::Incomplete);
        assert!(claim.validate().is_err(), "no gap recorded");
        claim.known_gaps.insert(gap());
        claim.validate().unwrap();
    }

    #[test]
    fn nothing_attempted_names_no_run() {
        let mut claim = evidence(CoverageStatus::NotObserved);
        claim.attempted = false;
        claim.committed = false;
        claim.observation_run = None;
        claim.validate().unwrap();

        // A synthetic run for an attempt that never happened is refused.
        claim.observation_run = Some(run());
        assert!(matches!(claim.validate(), Err(FormatError::Consistency(_))));
    }

    #[test]
    fn a_failed_attempt_is_a_different_fact_from_never_looking() {
        let mut failed = evidence(CoverageStatus::NotObserved);
        failed.attempted = true;
        failed.committed = false;
        failed.known_gaps.insert(gap());
        failed.validate().unwrap();

        let mut never = evidence(CoverageStatus::NotObserved);
        never.attempted = false;
        never.committed = false;
        never.observation_run = None;

        // Both are NotObserved, and they are not the same evidence.
        assert_ne!(failed, never);
    }

    #[test]
    fn a_failed_attempt_must_name_the_run_that_failed() {
        let mut claim = evidence(CoverageStatus::NotObserved);
        claim.attempted = true;
        claim.committed = false;
        claim.observation_run = None;
        assert!(claim.validate().is_err());
    }

    #[test]
    fn committed_results_require_an_attempt() {
        let mut claim = evidence(CoverageStatus::NotObserved);
        claim.attempted = false;
        claim.committed = true;
        assert!(claim.validate().is_err());
    }

    #[test]
    // retired-architecture-ok: the test's subject is the field's absence.
    fn the_wire_form_round_trips_and_has_no_completeness_proof() {
        let claim = evidence(CoverageStatus::Complete);
        let encoded = serde_json::to_string(&claim).unwrap();
        // retired-architecture-ok: proving the field is absent must name it.
        assert!(!encoded.contains("completeness_proof"));
        assert_eq!(
            serde_json::from_str::<CoverageEvidence>(&encoded).unwrap(),
            claim
        );
        // v1 has no such field, so a document carrying one is refused rather
        // than silently accepted with it ignored.
        let widened = encoded.replace(
            "{\"provider_binding\"",
            // retired-architecture-ok: the rejected document must carry it.
            "{\"completeness_proof\":\"x\",\"provider_binding\"",
        );
        assert!(serde_json::from_str::<CoverageEvidence>(&widened).is_err());
    }
}
