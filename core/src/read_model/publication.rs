//! Per-target publication status, folded from facts rather than tracked.
//!
//! # Why this is a fold and not a stored status field
//!
//! A status field is a second place the truth lives, and the two drift. The
//! attempt journal, the outcome store and the resolution heads are already
//! authoritative; a `status: "succeeded"` column beside them is a cache that
//! can be wrong, and it will be wrong exactly when a crash interrupted the
//! write that would have corrected it.
//!
//! So status is computed on read, from the facts, every time.
//!
//! # The fold order, and why it is that order
//!
//! ```text
//! attempts  →  outcomes  →  active resolutions
//! ```
//!
//! Each stage can only be *overridden* by the one after it, never contradicted
//! by the one before:
//!
//! * an attempt says something was tried;
//! * its outcome says what happened;
//! * an authorized resolution says what a better-informed reading concluded.
//!
//! A resolution always concludes — the frozen contract has only
//! `ResolvedSucceeded` and `ResolvedFailed`. There is no "still unknown"
//! resolution, because a reinterpretation that reinterprets nothing is just
//! the outcome, and creating one would add authority to a non-answer.
//!
//! A resolution is last because it is the only thing permitted to reinterpret
//! a recorded outcome, and it does so without altering it — the original
//! outcome remains, and the resolution sits on top. Reversing the order would
//! let a raw outcome silently overwrite an authorized reinterpretation.
//!
//! # Why "nothing has happened yet" and "we cannot tell" are different
//!
//! A target with no attempts has never been published to. A target whose last
//! attempt is unresolved may or may not have caused an external effect. Both
//! are "not succeeded", and only one of them is safe to retry — so they are
//! separate answers, and neither collapses into a generic `unknown`.

use std::collections::BTreeMap;

use draft_dcg_contract::ids::PublicationAttemptId;
use draft_dcg_contract::publication::{
    PublicationOutcomeKind, PublicationResolutionDigest, PublicationResolutionKind,
};

/// What is known about one attempt, as facts rather than as a verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptFacts {
    pub attempt: PublicationAttemptId,
    pub attempt_number: u32,
    /// The recorded primary outcome, if one has committed.
    ///
    /// `None` covers both "still in flight" and "interrupted before an outcome
    /// committed"; the journal, not this view, distinguishes them.
    pub outcome: Option<PublicationOutcomeKind>,
    /// Whether the attempt reached the durable dispatch boundary.
    ///
    /// A staged attempt that never dispatched is not evidence that anything
    /// was tried externally, and folding it in as one would overstate what
    /// happened.
    pub dispatched: bool,
    /// The active resolution reinterpreting this attempt's outcome, if any.
    pub resolution: Option<ActiveResolution>,
}

/// An authorized reinterpretation currently at the head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveResolution {
    pub digest: PublicationResolutionDigest,
    pub kind: PublicationResolutionKind,
}

/// What a person needs to know about one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetStatus {
    /// Nothing has ever been attempted.
    NeverAttempted,
    /// An attempt is in flight and no outcome has committed.
    InFlight { attempt: PublicationAttemptId },
    /// The most recent conclusion is that the effect occurred.
    Succeeded {
        attempt: PublicationAttemptId,
        /// Set when an authorized resolution, not the raw outcome, is what
        /// says so.
        by_resolution: bool,
    },
    /// The most recent conclusion is that the effect did not occur.
    Failed {
        attempt: PublicationAttemptId,
        by_resolution: bool,
    },
    /// Draft cannot establish whether the effect occurred.
    ///
    /// Distinct from `Failed`: retrying from here may duplicate a real effect,
    /// which is a materially different situation for the person deciding.
    Unresolved { attempt: PublicationAttemptId },
    /// Every attempt was withdrawn before anything was sent.
    NothingDispatched,
}

impl TargetStatus {
    /// Whether the external effect is known to have occurred.
    pub fn is_succeeded(&self) -> bool {
        matches!(self, Self::Succeeded { .. })
    }

    /// Whether another attempt is safe on this status alone.
    ///
    /// Deliberately conservative and deliberately not the inverse of
    /// `is_succeeded`: `Unresolved` is not a success and is still not safe to
    /// retry, because the effect may already have happened.
    pub fn permits_new_attempt(&self) -> bool {
        matches!(
            self,
            Self::NeverAttempted | Self::NothingDispatched | Self::Failed { .. }
        )
    }
}

/// Fold one target's attempts into a status.
///
/// Attempts are folded in `attempt_number` order and the last one that
/// concluded anything wins, because a later attempt is a later statement about
/// the same target.
pub fn status(attempts: &[AttemptFacts]) -> TargetStatus {
    if attempts.is_empty() {
        return TargetStatus::NeverAttempted;
    }

    let mut ordered: Vec<&AttemptFacts> = attempts.iter().collect();
    ordered.sort_by_key(|facts| facts.attempt_number);

    let mut answer = TargetStatus::NothingDispatched;
    for facts in ordered {
        if !facts.dispatched {
            continue;
        }
        answer = match (&facts.outcome, &facts.resolution) {
            // A resolution is the last word, and the only thing permitted to
            // reinterpret a recorded outcome.
            (_, Some(active)) => match &active.kind {
                PublicationResolutionKind::ResolvedSucceeded { .. } => TargetStatus::Succeeded {
                    attempt: facts.attempt.clone(),
                    by_resolution: true,
                },
                PublicationResolutionKind::ResolvedFailed { .. } => TargetStatus::Failed {
                    attempt: facts.attempt.clone(),
                    by_resolution: true,
                },
            },
            (Some(PublicationOutcomeKind::Succeeded { .. }), None) => TargetStatus::Succeeded {
                attempt: facts.attempt.clone(),
                by_resolution: false,
            },
            (
                Some(
                    PublicationOutcomeKind::Failed { .. } | PublicationOutcomeKind::NoEffect { .. },
                ),
                None,
            ) => TargetStatus::Failed {
                attempt: facts.attempt.clone(),
                by_resolution: false,
            },
            (Some(PublicationOutcomeKind::Indeterminate { .. }), None) => {
                TargetStatus::Unresolved {
                    attempt: facts.attempt.clone(),
                }
            }
            (None, None) => TargetStatus::InFlight {
                attempt: facts.attempt.clone(),
            },
        };
    }
    answer
}

/// Fold every target at once.
pub fn statuses<Target: Ord + Clone>(
    per_target: &BTreeMap<Target, Vec<AttemptFacts>>,
) -> BTreeMap<Target, TargetStatus> {
    per_target
        .iter()
        .map(|(target, attempts)| (target.clone(), status(attempts)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::Digest;

    fn attempt(number: u32) -> PublicationAttemptId {
        PublicationAttemptId::parse(format!("pat_00000000000{number}")).unwrap()
    }

    fn facts(number: u32, outcome: Option<PublicationOutcomeKind>) -> AttemptFacts {
        AttemptFacts {
            attempt: attempt(number),
            attempt_number: number,
            outcome,
            dispatched: true,
            resolution: None,
        }
    }

    fn succeeded() -> PublicationOutcomeKind {
        PublicationOutcomeKind::Succeeded {
            external_reference: "remote-1".into(),
        }
    }

    fn failed() -> PublicationOutcomeKind {
        PublicationOutcomeKind::Failed {
            reason: "refused".into(),
        }
    }

    fn indeterminate() -> PublicationOutcomeKind {
        PublicationOutcomeKind::Indeterminate {
            reason: "the connection dropped".into(),
        }
    }

    fn resolution(kind: PublicationResolutionKind) -> ActiveResolution {
        ActiveResolution {
            digest: PublicationResolutionDigest::new(Digest::of_bytes(b"resolution")),
            kind,
        }
    }

    #[test]
    fn a_target_nobody_has_published_to_is_never_attempted() {
        assert_eq!(status(&[]), TargetStatus::NeverAttempted);
        assert!(status(&[]).permits_new_attempt());
    }

    #[test]
    fn the_latest_attempt_is_the_current_statement_about_the_target() {
        let history = [facts(1, Some(failed())), facts(2, Some(succeeded()))];
        assert_eq!(
            status(&history),
            TargetStatus::Succeeded {
                attempt: attempt(2),
                by_resolution: false
            }
        );
    }

    #[test]
    fn attempts_fold_in_number_order_not_in_the_order_they_were_read() {
        // A store enumerating out of order must not change the answer.
        let jumbled = [facts(2, Some(succeeded())), facts(1, Some(failed()))];
        assert!(status(&jumbled).is_succeeded());
    }

    #[test]
    fn a_resolution_reinterprets_an_outcome_without_replacing_it() {
        // The outcome still says Indeterminate; the resolution says what a
        // better-informed reading concluded, and the status follows it.
        let mut resolved = facts(1, Some(indeterminate()));
        resolved.resolution = Some(resolution(PublicationResolutionKind::ResolvedSucceeded {
            external_reference: "remote-1".into(),
        }));

        assert_eq!(
            status(&[resolved.clone()]),
            TargetStatus::Succeeded {
                attempt: attempt(1),
                by_resolution: true
            }
        );
        assert_eq!(
            resolved.outcome,
            Some(indeterminate()),
            "the recorded outcome is untouched"
        );
    }

    #[test]
    fn a_resolution_outranks_the_raw_outcome_in_both_directions() {
        // Reversing the fold order would let the raw outcome silently
        // overwrite an authorized reinterpretation.
        let mut overturned = facts(1, Some(succeeded()));
        overturned.resolution = Some(resolution(PublicationResolutionKind::ResolvedFailed {
            reason: "the target reports no such record".into(),
        }));
        assert_eq!(
            status(&[overturned]),
            TargetStatus::Failed {
                attempt: attempt(1),
                by_resolution: true
            }
        );
    }

    #[test]
    fn an_indeterminate_outcome_with_no_resolution_stays_unresolved() {
        // There is no "still unknown" resolution to create: the outcome
        // already says that, and wrapping it in an authorized fact would add
        // authority to a non-answer.
        assert_eq!(
            status(&[facts(1, Some(indeterminate()))]),
            TargetStatus::Unresolved {
                attempt: attempt(1)
            }
        );
    }

    #[test]
    fn unresolved_is_not_failed_and_does_not_permit_a_retry() {
        // The distinction people act on: retrying a failure re-attempts
        // something that did not happen; retrying an unresolved attempt may
        // duplicate something that did.
        let unresolved = status(&[facts(1, Some(indeterminate()))]);
        let failed_status = status(&[facts(1, Some(failed()))]);

        assert!(!unresolved.is_succeeded() && !failed_status.is_succeeded());
        assert!(!unresolved.permits_new_attempt());
        assert!(failed_status.permits_new_attempt());
    }

    #[test]
    fn no_effect_is_a_failure_because_the_effect_provably_did_not_happen() {
        let proven = facts(
            1,
            Some(PublicationOutcomeKind::NoEffect {
                evidence: "the target reports no such record".into(),
            }),
        );
        assert_eq!(
            status(&[proven]),
            TargetStatus::Failed {
                attempt: attempt(1),
                by_resolution: false
            }
        );
    }

    #[test]
    fn a_staged_attempt_that_never_dispatched_asserts_nothing_externally() {
        // Counting it as an attempt would overstate what happened: nothing was
        // sent, so the target is in the same position as before.
        let mut staged = facts(1, None);
        staged.dispatched = false;
        assert_eq!(status(&[staged]), TargetStatus::NothingDispatched);
        assert!(status(&[]).permits_new_attempt());
    }

    #[test]
    fn a_dispatched_attempt_with_no_outcome_yet_is_in_flight() {
        assert_eq!(
            status(&[facts(1, None)]),
            TargetStatus::InFlight {
                attempt: attempt(1)
            }
        );
        assert!(!status(&[facts(1, None)]).permits_new_attempt());
    }

    #[test]
    fn a_later_staged_attempt_does_not_erase_an_earlier_conclusion() {
        // pat_2 was withdrawn before dispatch, so it says nothing. The target
        // is still whatever pat_1 established.
        let mut staged = facts(2, None);
        staged.dispatched = false;
        assert!(status(&[facts(1, Some(succeeded())), staged]).is_succeeded());
    }

    #[test]
    fn every_target_folds_independently() {
        let per_target: BTreeMap<&str, Vec<AttemptFacts>> = [
            ("staging", vec![facts(1, Some(succeeded()))]),
            ("production", vec![facts(1, Some(indeterminate()))]),
            ("archive", Vec::new()),
        ]
        .into_iter()
        .collect();

        let folded = statuses(&per_target);
        assert!(folded["staging"].is_succeeded());
        assert!(matches!(
            folded["production"],
            TargetStatus::Unresolved { .. }
        ));
        assert_eq!(folded["archive"], TargetStatus::NeverAttempted);
    }
}
