//! Restart classification for one attempt — journal × control × outcome.
//!
//! # Why three dimensions and not one
//!
//! The journal marker alone is never the authority on what happened. Each of
//! the three durable records answers a question the others cannot:
//!
//! * the **journal** says what this worker had committed to doing;
//! * the **control record** says whether the allocation or the clear landed;
//! * the **outcome store** says whether a primary outcome exists.
//!
//! A crash lands between them, so the interesting states are precisely the
//! ones where they disagree. Reading fewer than three would make some of those
//! disagreements invisible — and an invisible disagreement is resolved by
//! assumption, which for Publication means either re-dispatching an external
//! effect that already happened or abandoning one that did.
//!
//! # Why so many combinations are hard failures
//!
//! Most impossible combinations are impossible *because of the write order*,
//! not because of luck. `Dispatching` with a primary outcome already present
//! contradicts `Dispatching → OutcomePrepared → record_once`; `Finalized` still
//! referenced by the control record contradicts clear-before-finalize. When
//! one of those appears, something wrote outside the Stores. Guessing which
//! record to believe would convert a detected bypass into an accepted one, so
//! they route to Doctor/Recovery instead.
//!
//! # Why historical states tolerate a moved control record
//!
//! `Abandoned` and `Finalized` are conclusions that were *proved* at
//! classification time, against a control value that has since legitimately
//! moved on — a later `pat_B`, an advanced generation, further consumed
//! authorizations. Requiring the old comparison to still hold would invalidate
//! correct history as soon as the project made progress. What they still
//! require is the one thing that would contradict them: the control record
//! must not claim the historical attempt as current.

use crate::publication::control::{ControlMatch, PublicationControl};
use crate::publication::journal::{AttemptJournalState, TerminalDisposition};
use draft_dcg_contract::ids::PublicationAttemptId;
use draft_dcg_contract::publication::PublicationOutcomeDigest;

/// What the outcome store holds for this attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomePresence {
    /// No primary outcome has been recorded.
    Absent,
    /// A primary outcome exists with this digest.
    Present(PublicationOutcomeDigest),
}

impl OutcomePresence {
    fn matches(&self, expected: &PublicationOutcomeDigest) -> bool {
        matches!(self, Self::Present(digest) if digest == expected)
    }

    fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }
}

/// How the delivery semantics permit an unresolved dispatch to be closed.
///
/// Read from the Publication, never inferred. It decides the one question a
/// crashed dispatch cannot answer locally: may Draft conclude anything about
/// an external effect it did not see the result of?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchRecoveryClass {
    /// The outcome can be established locally without contacting anything —
    /// the delivery semantics let Draft record `Indeterminate` and close the
    /// attempt safely.
    ResolvableLocally,
    /// Establishing the outcome needs the external system, which the barrier
    /// must never wait on.
    NeedsExternalResolution,
}

/// What recovery concluded for one attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptResolution {
    /// The allocation is proven never to have committed. Move the journal to
    /// the terminal `Abandoned` state; no number, no authorization, no event
    /// and no outcome are owed.
    AllocationDidNotCommit,
    /// The allocation committed. This exact attempt is authoritative and must
    /// be resumed — never replaced by a fresh one.
    ResumeAllocatedAttempt { attempt: PublicationAttemptId },
    /// The attempt may have reached the external system. The outcome must be
    /// established through the primary-outcome path.
    EstablishOutcome,
    /// The outcome cannot be established without the external system.
    /// Nothing local blocks, and no new attempt may be allocated for this
    /// Publication.
    AwaitExternalResolution { attempt: PublicationAttemptId },
    /// A candidate is durable and the outcome commit may be retried
    /// idempotently.
    RetryOutcomeCommit,
    /// The outcome is committed; advance the journal to `OutcomeRecorded`.
    AdvanceToOutcomeRecorded,
    /// Finish the receipt and audit work, then run the exact control clear.
    CompleteThenClear,
    /// The clear already committed; verify and finalize.
    VerifyThenFinalize,
    /// Drain the abandonment fact, then finalize.
    DrainThenFinalize,
    /// A terminal state that verifies. Nothing to do.
    AlreadyTerminal,
    /// The records disagree in a way no legal write order produces.
    Inconsistent { detail: &'static str },
}

/// Everything recovery reads before it decides.
///
/// Deliberately plain data. [`classify`] takes only values that have already
/// been read from local stores, so it cannot reach for a lock it should not
/// hold, and it cannot block on anything remote.
#[derive(Debug, Clone)]
pub struct AttemptSnapshot<'a> {
    pub attempt: &'a PublicationAttemptId,
    pub journal: &'a AttemptJournalState,
    /// The authoritative control value, or `None` if the record is absent.
    pub control: Option<&'a PublicationControl>,
    pub outcome: &'a OutcomePresence,
    pub recovery_class: DispatchRecoveryClass,
}

impl AttemptSnapshot<'_> {
    /// Whether the control record currently claims this attempt.
    fn control_claims_this_attempt(&self) -> bool {
        self.control
            .and_then(|control| control.in_flight_attempt.as_ref())
            .is_some_and(|in_flight| in_flight == self.attempt)
    }
}

/// The restart table.
pub fn classify(snapshot: &AttemptSnapshot<'_>) -> AttemptResolution {
    use AttemptJournalState as J;
    use AttemptResolution as R;

    match snapshot.journal {
        J::AttemptPrepared { allocation, .. } => {
            match ControlMatch::classify(
                snapshot.control,
                &allocation.expected,
                &allocation.planned,
            ) {
                // The candidate never became authoritative. Nothing was spent
                // and nothing was dispatched, so this is decided without
                // consulting the outcome store at all.
                ControlMatch::Expected => R::AllocationDidNotCommit,
                ControlMatch::Planned => R::ResumeAllocatedAttempt {
                    attempt: snapshot.attempt.clone(),
                },
                ControlMatch::Neither => R::Inconsistent {
                    detail: "an AttemptPrepared journal whose control record is neither the exact \
                             value the allocation expected nor the exact value it planned",
                },
            }
        }

        J::Dispatching { .. } => {
            if !snapshot.outcome.is_absent() {
                // The durable order is Dispatching → OutcomePrepared →
                // record_once, so an outcome here means something wrote past
                // the journal. Never a silent fast-forward.
                return R::Inconsistent {
                    detail: "a primary outcome exists for an attempt whose journal is still \
                             Dispatching, which the durable write order makes unreachable",
                };
            }
            if !snapshot.control_claims_this_attempt() {
                // Inconsistent, but not uniformly hopeless. Where the delivery
                // class can *prove* what happened by client-key lookup, the
                // truth is still recoverable — by asking, which the barrier
                // reports rather than waits on. Where it cannot, nothing local
                // can settle whether the effect occurred.
                return match snapshot.recovery_class {
                    DispatchRecoveryClass::NeedsExternalResolution => R::AwaitExternalResolution {
                        attempt: snapshot.attempt.clone(),
                    },
                    DispatchRecoveryClass::ResolvableLocally => R::Inconsistent {
                        detail: "the journal is Dispatching but the control record does not hold \
                                 this attempt in flight, and the delivery class cannot prove what \
                                 happened",
                    },
                };
            }
            match snapshot.recovery_class {
                DispatchRecoveryClass::ResolvableLocally => R::EstablishOutcome,
                DispatchRecoveryClass::NeedsExternalResolution => R::AwaitExternalResolution {
                    attempt: snapshot.attempt.clone(),
                },
            }
        }

        J::OutcomePrepared { candidate, .. } => {
            if !snapshot.control_claims_this_attempt() {
                return R::Inconsistent {
                    detail: "the journal is OutcomePrepared but the control record does not hold \
                             this attempt in flight, so the clear ran before finalization",
                };
            }
            if snapshot.outcome.is_absent() {
                R::RetryOutcomeCommit
            } else if snapshot.outcome.matches(candidate) {
                R::AdvanceToOutcomeRecorded
            } else {
                R::Inconsistent {
                    detail: "a primary outcome exists that is not this attempt's durable \
                             candidate, which serialized candidate selection makes unreachable",
                }
            }
        }

        J::OutcomeRecorded {
            outcome,
            control_clear,
            ..
        } => {
            if !snapshot.outcome.matches(outcome) {
                return R::Inconsistent {
                    detail: "an OutcomeRecorded journal whose primary outcome is missing or does \
                             not match the recorded digest",
                };
            }
            match ControlMatch::classify(
                snapshot.control,
                &control_clear.expected,
                &control_clear.planned,
            ) {
                ControlMatch::Expected => R::CompleteThenClear,
                ControlMatch::Planned => R::VerifyThenFinalize,
                ControlMatch::Neither => R::Inconsistent {
                    detail: "an OutcomeRecorded journal whose control record is neither the exact \
                             pre-clear value nor the exact cleared value",
                },
            }
        }

        J::AbandonPrepared { control_clear, .. } => {
            if !snapshot.outcome.is_absent() {
                return R::Inconsistent {
                    detail: "a primary outcome exists for an attempt being abandoned before \
                             dispatch",
                };
            }
            match ControlMatch::classify(
                snapshot.control,
                &control_clear.expected,
                &control_clear.planned,
            ) {
                ControlMatch::Expected => R::CompleteThenClear,
                ControlMatch::Planned => R::DrainThenFinalize,
                ControlMatch::Neither => R::Inconsistent {
                    detail: "an AbandonPrepared journal whose control record is neither the exact \
                             pre-clear value nor the exact cleared value",
                },
            }
        }

        J::AbandonedBeforeDispatch { control_clear, .. } => {
            if !snapshot.outcome.is_absent() {
                return R::Inconsistent {
                    detail: "a primary outcome exists for an attempt that was abandoned before \
                             dispatch",
                };
            }
            match ControlMatch::classify(
                snapshot.control,
                &control_clear.expected,
                &control_clear.planned,
            ) {
                // Draining is idempotent, so this is the same answer whether
                // the fact has been drained or not.
                ControlMatch::Planned => R::DrainThenFinalize,
                ControlMatch::Expected => R::Inconsistent {
                    detail: "the journal advanced to AbandonedBeforeDispatch before the exact \
                             control clear committed",
                },
                ControlMatch::Neither => R::Inconsistent {
                    detail: "an AbandonedBeforeDispatch journal whose control record is neither \
                             the exact pre-clear value nor the exact cleared value",
                },
            }
        }

        // Historical and terminal. The current control record has legitimately
        // moved on; what it may not do is claim this attempt.
        J::Abandoned { .. } => {
            if !snapshot.outcome.is_absent() {
                R::Inconsistent {
                    detail: "a primary outcome exists for an attempt whose allocation is proven \
                             never to have committed",
                }
            } else if snapshot.control_claims_this_attempt() {
                R::Inconsistent {
                    detail: "the control record claims an attempt whose allocation is proven \
                             never to have committed",
                }
            } else {
                R::AlreadyTerminal
            }
        }

        J::Finalized {
            terminal_disposition,
        } => {
            if snapshot.control_claims_this_attempt() {
                return R::Inconsistent {
                    detail: "the control record still claims a finalized attempt, though the \
                             exact clear commits before Finalized becomes durable",
                };
            }
            match terminal_disposition {
                TerminalDisposition::OutcomeFinalized { outcome, .. } => {
                    if snapshot.outcome.matches(outcome) {
                        R::AlreadyTerminal
                    } else {
                        R::Inconsistent {
                            detail: "a finalized attempt whose primary outcome is missing or does \
                                     not match its recorded digest",
                        }
                    }
                }
                TerminalDisposition::AbandonedBeforeDispatch { .. } => {
                    if snapshot.outcome.is_absent() {
                        R::AlreadyTerminal
                    } else {
                        R::Inconsistent {
                            detail: "a finalized pre-dispatch abandonment with a primary outcome, \
                                     though that path never produces one",
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication::journal::{ControlTransition, NonCommitEvidence};
    use draft_dcg_contract::ids::{ActivityEventId, PublicationId, ReceiptId};
    use draft_dcg_contract::publication::{PublicationAttemptDigest, PublicationAttemptRef};
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::Digest;

    fn publication() -> PublicationId {
        PublicationId::parse("pub_000000000001").unwrap()
    }

    fn attempt_a() -> PublicationAttemptId {
        PublicationAttemptId::parse("pat_00000000000a").unwrap()
    }

    fn attempt_b() -> PublicationAttemptId {
        PublicationAttemptId::parse("pat_00000000000b").unwrap()
    }

    fn control(generation: u64, in_flight: Option<PublicationAttemptId>) -> PublicationControl {
        let mut value = PublicationControl::initial(publication());
        value.generation = generation;
        value.in_flight_attempt = in_flight;
        value
    }

    fn allocation() -> ControlTransition {
        ControlTransition {
            expected: control(0, None),
            planned: control(1, Some(attempt_a())),
        }
    }

    fn clear() -> ControlTransition {
        ControlTransition {
            expected: control(1, Some(attempt_a())),
            planned: control(2, None),
        }
    }

    fn attempt_ref() -> PublicationAttemptRef {
        PublicationAttemptRef {
            id: attempt_a(),
            digest: PublicationAttemptDigest::new(Digest::of_bytes(b"attempt")),
        }
    }

    fn outcome_digest() -> PublicationOutcomeDigest {
        PublicationOutcomeDigest::new(Digest::of_bytes(b"outcome"))
    }

    fn resolve(
        journal: &AttemptJournalState,
        control: Option<&PublicationControl>,
        outcome: &OutcomePresence,
    ) -> AttemptResolution {
        classify(&AttemptSnapshot {
            attempt: &attempt_a(),
            journal,
            control,
            outcome,
            recovery_class: DispatchRecoveryClass::ResolvableLocally,
        })
    }

    fn prepared() -> AttemptJournalState {
        AttemptJournalState::AttemptPrepared {
            candidate_attempt_number: 1,
            allocation: allocation(),
            retry_authorization: None,
        }
    }

    #[test]
    fn an_allocation_that_never_committed_is_decided_without_the_outcome_store() {
        // Nothing was spent and nothing was dispatched, so no other record can
        // change this answer.
        let expected = control(0, None);
        assert_eq!(
            resolve(&prepared(), Some(&expected), &OutcomePresence::Absent),
            AttemptResolution::AllocationDidNotCommit
        );
    }

    #[test]
    fn a_committed_allocation_resumes_the_same_attempt() {
        // The one that matters most: allocating a fresh attempt here would
        // spend a second number against an external effect already authorized
        // under the first.
        let planned = control(1, Some(attempt_a()));
        assert_eq!(
            resolve(&prepared(), Some(&planned), &OutcomePresence::Absent),
            AttemptResolution::ResumeAllocatedAttempt {
                attempt: attempt_a()
            }
        );
    }

    #[test]
    fn a_dispatching_attempt_with_a_primary_outcome_is_an_integrity_failure() {
        // Not a fast-forward. The durable order makes this combination
        // unreachable, so seeing it means something bypassed the journal.
        let dispatching = AttemptJournalState::Dispatching {
            attempt: attempt_ref(),
            attempt_number: 1,
            dispatch_event: ActivityEventId::parse("evt_000000000001").unwrap(),
        };
        let result = resolve(
            &dispatching,
            Some(&control(1, Some(attempt_a()))),
            &OutcomePresence::Present(outcome_digest()),
        );
        assert!(matches!(result, AttemptResolution::Inconsistent { .. }));
    }

    #[test]
    fn an_unresolvable_dispatch_waits_for_external_resolution_without_blocking() {
        let dispatching = AttemptJournalState::Dispatching {
            attempt: attempt_ref(),
            attempt_number: 1,
            dispatch_event: ActivityEventId::parse("evt_000000000001").unwrap(),
        };
        let in_flight = control(1, Some(attempt_a()));
        assert_eq!(
            classify(&AttemptSnapshot {
                attempt: &attempt_a(),
                journal: &dispatching,
                control: Some(&in_flight),
                outcome: &OutcomePresence::Absent,
                recovery_class: DispatchRecoveryClass::NeedsExternalResolution,
            }),
            AttemptResolution::AwaitExternalResolution {
                attempt: attempt_a()
            }
        );
        assert_eq!(
            classify(&AttemptSnapshot {
                attempt: &attempt_a(),
                journal: &dispatching,
                control: Some(&in_flight),
                outcome: &OutcomePresence::Absent,
                recovery_class: DispatchRecoveryClass::ResolvableLocally,
            }),
            AttemptResolution::EstablishOutcome
        );
    }

    #[test]
    fn a_durable_candidate_is_retried_and_a_committed_one_advances() {
        let outcome_prepared = AttemptJournalState::OutcomePrepared {
            attempt: attempt_ref(),
            attempt_number: 1,
            candidate: outcome_digest(),
            receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
            outcome_event: ActivityEventId::parse("evt_000000000002").unwrap(),
        };
        let in_flight = control(1, Some(attempt_a()));

        assert_eq!(
            resolve(
                &outcome_prepared,
                Some(&in_flight),
                &OutcomePresence::Absent
            ),
            AttemptResolution::RetryOutcomeCommit
        );
        assert_eq!(
            resolve(
                &outcome_prepared,
                Some(&in_flight),
                &OutcomePresence::Present(outcome_digest())
            ),
            AttemptResolution::AdvanceToOutcomeRecorded
        );

        // A different outcome under a serialized candidate cannot arise.
        let other = OutcomePresence::Present(PublicationOutcomeDigest::new(Digest::of_bytes(
            b"somebody-elses-outcome",
        )));
        assert!(matches!(
            resolve(&outcome_prepared, Some(&in_flight), &other),
            AttemptResolution::Inconsistent { .. }
        ));
    }

    #[test]
    fn a_dispatch_the_control_record_disowns_is_recoverable_only_by_asking() {
        // Inconsistent either way, but the two classes differ in what can be
        // done about it: one can still learn the truth from the external
        // system, the other cannot learn it at all.
        let dispatching = AttemptJournalState::Dispatching {
            attempt: attempt_ref(),
            attempt_number: 1,
            dispatch_event: ActivityEventId::parse("evt_000000000001").unwrap(),
        };
        let disowned = control(2, Some(attempt_b()));

        assert_eq!(
            classify(&AttemptSnapshot {
                attempt: &attempt_a(),
                journal: &dispatching,
                control: Some(&disowned),
                outcome: &OutcomePresence::Absent,
                recovery_class: DispatchRecoveryClass::NeedsExternalResolution,
            }),
            AttemptResolution::AwaitExternalResolution {
                attempt: attempt_a()
            }
        );
        assert!(matches!(
            classify(&AttemptSnapshot {
                attempt: &attempt_a(),
                journal: &dispatching,
                control: Some(&disowned),
                outcome: &OutcomePresence::Absent,
                recovery_class: DispatchRecoveryClass::ResolvableLocally,
            }),
            AttemptResolution::Inconsistent { .. }
        ));
    }

    #[test]
    fn a_cleared_control_under_outcome_prepared_is_a_barrier_bypass() {
        // The barrier forbids clearing before finalization, so this is not a
        // window recovery may close by inference.
        let outcome_prepared = AttemptJournalState::OutcomePrepared {
            attempt: attempt_ref(),
            attempt_number: 1,
            candidate: outcome_digest(),
            receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
            outcome_event: ActivityEventId::parse("evt_000000000002").unwrap(),
        };
        assert!(matches!(
            resolve(
                &outcome_prepared,
                Some(&control(2, None)),
                &OutcomePresence::Absent
            ),
            AttemptResolution::Inconsistent { .. }
        ));
        // And a control record claiming a *different* attempt likewise.
        assert!(matches!(
            resolve(
                &outcome_prepared,
                Some(&control(2, Some(attempt_b()))),
                &OutcomePresence::Absent
            ),
            AttemptResolution::Inconsistent { .. }
        ));
    }

    #[test]
    fn the_clear_is_classified_by_whole_value_on_both_sides() {
        let recorded = AttemptJournalState::OutcomeRecorded {
            attempt: attempt_ref(),
            attempt_number: 1,
            outcome: outcome_digest(),
            receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
            control_clear: clear(),
        };
        let present = OutcomePresence::Present(outcome_digest());

        assert_eq!(
            resolve(&recorded, Some(&control(1, Some(attempt_a()))), &present),
            AttemptResolution::CompleteThenClear
        );
        assert_eq!(
            resolve(&recorded, Some(&control(2, None)), &present),
            AttemptResolution::VerifyThenFinalize
        );
        // A missing outcome under OutcomeRecorded never falls back to
        // Dispatching semantics.
        assert!(matches!(
            resolve(
                &recorded,
                Some(&control(1, Some(attempt_a()))),
                &OutcomePresence::Absent
            ),
            AttemptResolution::Inconsistent { .. }
        ));
    }

    #[test]
    fn abandoned_tolerates_a_control_record_that_moved_on_but_not_one_that_claims_it() {
        let abandoned = AttemptJournalState::Abandoned {
            evidence: NonCommitEvidence {
                candidate_attempt_number: 1,
                observed_control: control(0, None),
                classified_at: Timestamp::from_unix_nanos(0),
            },
        };

        // Idle, and legitimately advanced to a later attempt: both fine. The
        // non-commit conclusion was proved once and stays proved.
        assert_eq!(
            resolve(
                &abandoned,
                Some(&control(0, None)),
                &OutcomePresence::Absent
            ),
            AttemptResolution::AlreadyTerminal
        );
        assert_eq!(
            resolve(
                &abandoned,
                Some(&control(7, Some(attempt_b()))),
                &OutcomePresence::Absent
            ),
            AttemptResolution::AlreadyTerminal
        );

        // Claiming pat_A itself contradicts the proof.
        assert!(matches!(
            resolve(
                &abandoned,
                Some(&control(7, Some(attempt_a()))),
                &OutcomePresence::Absent
            ),
            AttemptResolution::Inconsistent { .. }
        ));
        // And an outcome for an attempt that was never dispatched.
        assert!(matches!(
            resolve(
                &abandoned,
                Some(&control(0, None)),
                &OutcomePresence::Present(outcome_digest())
            ),
            AttemptResolution::Inconsistent { .. }
        ));
    }

    #[test]
    fn each_finalized_disposition_demands_the_outcome_its_own_path_produces() {
        let outcome_path = AttemptJournalState::Finalized {
            terminal_disposition: TerminalDisposition::OutcomeFinalized {
                attempt: attempt_ref(),
                outcome: outcome_digest(),
                receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
                control_clear: clear(),
            },
        };
        let abandonment_path = AttemptJournalState::Finalized {
            terminal_disposition: TerminalDisposition::AbandonedBeforeDispatch {
                attempt_number: 1,
                reason: "refused".into(),
                control_clear: clear(),
                abandonment_event: ActivityEventId::parse("evt_000000000003").unwrap(),
            },
        };
        let idle = control(2, None);
        let present = OutcomePresence::Present(outcome_digest());

        assert_eq!(
            resolve(&outcome_path, Some(&idle), &present),
            AttemptResolution::AlreadyTerminal
        );
        assert_eq!(
            resolve(&abandonment_path, Some(&idle), &OutcomePresence::Absent),
            AttemptResolution::AlreadyTerminal,
            "the absence of an outcome is correct on the abandonment path"
        );

        // Each path rejects the other's outcome state.
        assert!(matches!(
            resolve(&outcome_path, Some(&idle), &OutcomePresence::Absent),
            AttemptResolution::Inconsistent { .. }
        ));
        assert!(matches!(
            resolve(&abandonment_path, Some(&idle), &present),
            AttemptResolution::Inconsistent { .. }
        ));
    }

    #[test]
    fn a_control_record_still_claiming_a_finalized_attempt_contradicts_the_write_order() {
        let finalized = AttemptJournalState::Finalized {
            terminal_disposition: TerminalDisposition::OutcomeFinalized {
                attempt: attempt_ref(),
                outcome: outcome_digest(),
                receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
                control_clear: clear(),
            },
        };
        assert!(matches!(
            resolve(
                &finalized,
                Some(&control(1, Some(attempt_a()))),
                &OutcomePresence::Present(outcome_digest())
            ),
            AttemptResolution::Inconsistent { .. }
        ));
    }

    #[test]
    fn an_absent_control_record_never_reads_as_a_matching_one() {
        assert!(matches!(
            resolve(&prepared(), None, &OutcomePresence::Absent),
            AttemptResolution::Inconsistent { .. }
        ));
    }
}
