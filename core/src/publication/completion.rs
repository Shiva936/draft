//! Phase A — the one path by which a primary outcome is written.
//!
//! # Why there is exactly one path
//!
//! A primary outcome is the durable answer to "did the external mutation
//! occur?". Several very different situations produce one: the ordinary
//! external return, crash recovery under idempotent delivery, reconciliation
//! by client key, a `NonIdempotent` attempt that can only honestly record
//! `Indeterminate`, and deterministic manual recovery.
//!
//! Each of those, written separately, would be a chance to skip the durable
//! candidate — and the candidate is what makes the outcome recoverable. So
//! every one of them goes through
//! [`prepare_and_record_primary_outcome`], and nothing calls
//! [`PublicationOutcomeStore::record_once`] straight out of `Dispatching`.
//!
//! # Why the returning worker re-reads first
//!
//! While the external call was in flight, **no lock was held** — that is the
//! whole point of the dispatch sequence. So recovery, reconciliation or a
//! late-result handler may legitimately have advanced this same attempt in the
//! meantime.
//!
//! The returning worker therefore converges with the authoritative journal
//! state rather than overwriting it. Assuming `Dispatching` on return would
//! mean a slow external reply could overwrite an outcome another actor already
//! committed — replacing a recorded answer about an external effect with a
//! staler one.
//!
//! # Why a result and a late answer are different things
//!
//! Once a primary outcome is authoritative, an external reply arriving
//! afterwards is *information*, not a competing candidate. It is real and
//! worth keeping, but it is operational recovery input: it never enters
//! `record_once`, never becomes a second outcome, and never regresses the
//! journal. Only an authorized Resolution can make a better-informed
//! interpretation authoritative.

use draft_dcg_contract::publication::{PublicationOutcome, PublicationOutcomeDigest};

use crate::publication::journal::{AttemptJournalGuard, AttemptJournalState};
use crate::publication::outcome::{PrimaryOutcomeIdentity, PublicationOutcomeStore, RecordOnce};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// What the returning worker should do, having re-read the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturningWorker {
    /// The journal is still `Dispatching`. Prepare and record the candidate.
    RecordCandidate,
    /// A candidate identical to this one is already durable. Continue
    /// idempotently from `record_once`.
    ContinueFromDurableCandidate,
    /// The primary outcome is already authoritative. This reply is late
    /// information, never a second candidate.
    ResultIsLateInformation,
    /// The journal and the reply cannot both be true.
    Inconsistent { detail: &'static str },
}

/// Decide what a worker returning from the external call must do.
///
/// Takes the authoritative journal state — read under the guard, never assumed
/// — and the digest of the candidate this worker is holding.
pub fn converge(
    journal: &AttemptJournalState,
    candidate: &PublicationOutcomeDigest,
) -> ReturningWorker {
    use AttemptJournalState as J;

    match journal {
        J::Dispatching { .. } => ReturningWorker::RecordCandidate,
        J::OutcomePrepared {
            candidate: durable, ..
        } => {
            if durable == candidate {
                ReturningWorker::ContinueFromDurableCandidate
            } else {
                // Candidate selection is serialized by the journal guard: the
                // second holder observes the first's candidate rather than
                // choosing another. Two different ones cannot both be durable.
                ReturningWorker::Inconsistent {
                    detail: "a durable candidate exists that is not this worker's, which \
                             serialized candidate selection makes unreachable",
                }
            }
        }
        J::OutcomeRecorded { .. } | J::Finalized { .. } => ReturningWorker::ResultIsLateInformation,
        J::AttemptPrepared { .. } => ReturningWorker::Inconsistent {
            detail: "the external system replied for an attempt whose journal never reached the \
                     dispatch boundary",
        },
        J::AbandonPrepared { .. } | J::AbandonedBeforeDispatch { .. } | J::Abandoned { .. } => {
            ReturningWorker::Inconsistent {
                detail: "the external system replied for an attempt Draft recorded as never \
                         dispatched",
            }
        }
    }
}

/// Prepare the candidate durably, then commit it — Phase A5.
///
/// The ordering is the outbox invariant: no authoritative primary outcome
/// commits before the candidate and the audit material describing that commit
/// are durable. A crash between the two leaves `OutcomePrepared`, which
/// recovery finishes by retrying `record_once` idempotently.
///
/// `identity` carries the attempt reference, the **preallocated** receipt id
/// and the **frozen** signer binding, all fixed before the external call, so
/// the outcome cannot acquire a different receipt or signer on the way back.
///
/// A [`RecordOnce::Conflict`] is returned as an integrity failure rather than
/// retried. Two workers cannot legitimately offer different outcomes for one
/// attempt, so a conflict means something wrote outside the Stores —
/// "converging" on one of them would silently discard a recorded claim about
/// whether an external effect occurred.
pub fn prepare_and_record_primary_outcome(
    guard: &mut AttemptJournalGuard<'_, '_>,
    outcomes: &PublicationOutcomeStore,
    identity: &PrimaryOutcomeIdentity,
    candidate: &PublicationOutcome,
    outcome_event: draft_dcg_contract::ids::ActivityEventId,
    control_clear: crate::publication::journal::ControlTransition,
) -> DraftResult<()> {
    let digest = candidate
        .digest()
        .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?;

    let current = guard.current_state()?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            "the attempt journal does not exist, so no outcome can be recorded against it",
        )
    })?;

    // Carried forward from the state that already holds it, never re-supplied
    // by the caller: a number passed in again is a number that can be passed
    // in wrong.
    let attempt_number = current.attempt_number().ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!(
                "attempt '{}' is at {} and carries no authoritative attempt number",
                identity.attempt.id,
                current.name()
            ),
        )
    })?;

    // Counted where the primary outcome becomes authoritative, and only for
    // the two kinds §2.57 names. Every writer passes through here, so an
    // outcome cannot be counted twice by a retry that converges, nor missed by
    // a recovery path that establishes one.
    match candidate.outcome {
        draft_dcg_contract::publication::PublicationOutcomeKind::Indeterminate { .. } => {
            crate::support::telemetry::Counter::PublicationIndeterminateTotal.increment();
        }
        draft_dcg_contract::publication::PublicationOutcomeKind::NoEffect { .. } => {
            crate::support::telemetry::Counter::PublicationNoEffectTotal.increment();
        }
        _ => {}
    }

    let prepared = match converge(&current, &digest) {
        ReturningWorker::RecordCandidate => {
            let prepared = AttemptJournalState::OutcomePrepared {
                attempt: identity.attempt.clone(),
                attempt_number,
                candidate: digest.clone(),
                receipt: identity.receipt.clone(),
                outcome_event,
            };
            guard.transition_locked(&current, prepared.clone())?;
            prepared
        }
        ReturningWorker::ContinueFromDurableCandidate => {
            // The candidate was already durable, so this call is finishing a
            // transaction an earlier one prepared.
            crate::support::telemetry::Counter::PublicationOutcomeRecoveryFastForwards.increment();
            current
        }
        ReturningWorker::ResultIsLateInformation => {
            // Real information, and deliberately not a second candidate. §2.57
            // counts it as a reconciliation *input*, which is why the metric
            // does not name a canonical Publication fact.
            crate::support::telemetry::Counter::PublicationLateResultObservations.increment();
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "the primary outcome is already authoritative; a later result is operational \
                 recovery input and never a second candidate",
            ));
        }
        ReturningWorker::Inconsistent { detail } => {
            crate::support::telemetry::Counter::PublicationInconsistentStates.increment();
            return Err(DraftError::new(DraftErrorKind::CorruptData, detail));
        }
    };

    match outcomes.record_once(identity, candidate)? {
        RecordOnce::Created | RecordOnce::ExistingSame => {}
        RecordOnce::Conflict { existing, offered } => {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "attempt '{}' already has primary outcome {existing} but {offered} was \
                     offered; serialized candidate selection makes this unreachable, so a Store \
                     was bypassed",
                    identity.attempt.id
                ),
            ))
        }
    }

    guard.transition_locked(
        &prepared,
        AttemptJournalState::OutcomeRecorded {
            attempt: identity.attempt.clone(),
            attempt_number,
            outcome: digest,
            receipt: identity.receipt.clone(),
            control_clear,
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication::control::PublicationControl;
    use crate::publication::journal::{
        AttemptJournal, ControlTransition, NonCommitEvidence, PublicationJournalStore,
        TerminalDisposition,
    };
    use draft_dcg_contract::identifier::NamespacedId;
    use draft_dcg_contract::ids::{
        ActivityEventId, ActorId, PublicationAttemptId, PublicationId, ReceiptId,
    };
    use draft_dcg_contract::producer::ProducerIdentity;
    use draft_dcg_contract::publication::{
        PublicationAttemptDigest, PublicationAttemptRef, PublicationOutcomeKind,
    };
    use draft_dcg_contract::receipt::ReceiptSignerBinding;
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::Digest;

    fn publication() -> PublicationId {
        PublicationId::parse("pub_000000000001").unwrap()
    }

    fn attempt_id() -> PublicationAttemptId {
        PublicationAttemptId::parse("pat_000000000001").unwrap()
    }

    fn attempt_ref() -> PublicationAttemptRef {
        PublicationAttemptRef {
            id: attempt_id(),
            digest: PublicationAttemptDigest::new(Digest::of_bytes(b"attempt")),
        }
    }

    fn identity() -> PrimaryOutcomeIdentity {
        PrimaryOutcomeIdentity {
            attempt: attempt_ref(),
            receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
            signer: ReceiptSignerBinding::new(
                ActorId::parse("act_000000000001").unwrap(),
                "key-1",
                "ed25519",
            )
            .unwrap(),
        }
    }

    fn outcome(kind: PublicationOutcomeKind) -> PublicationOutcome {
        PublicationOutcome {
            attempt: attempt_ref(),
            receipt_id: identity().receipt,
            receipt_signer: identity().signer,
            outcome: kind,
            concluded_at: Timestamp::from_unix_nanos(0),
            provenance: ProducerIdentity::new(
                NamespacedId::parse("draft.core/publication").unwrap(),
                "1",
            )
            .unwrap(),
        }
    }

    fn succeeded() -> PublicationOutcome {
        outcome(PublicationOutcomeKind::Succeeded {
            external_reference: "remote-1".into(),
        })
    }

    fn indeterminate() -> PublicationOutcome {
        outcome(PublicationOutcomeKind::Indeterminate {
            reason: "the connection dropped before the reply".into(),
        })
    }

    fn control(generation: u64, in_flight: Option<PublicationAttemptId>) -> PublicationControl {
        let mut value = PublicationControl::initial(publication());
        value.generation = generation;
        value.in_flight_attempt = in_flight;
        value
    }

    fn clear() -> ControlTransition {
        ControlTransition {
            expected: control(1, Some(attempt_id())),
            planned: control(2, None),
        }
    }

    fn dispatching() -> AttemptJournalState {
        AttemptJournalState::Dispatching {
            attempt: attempt_ref(),
            attempt_number: 1,
            dispatch_event: ActivityEventId::parse("evt_000000000001").unwrap(),
        }
    }

    fn outcome_event() -> ActivityEventId {
        ActivityEventId::parse("evt_000000000002").unwrap()
    }

    /// A journal already at `Dispatching`, ready for an external reply.
    fn dispatched(directory: &tempfile::TempDir) -> PublicationJournalStore {
        let store = PublicationJournalStore::new(directory.path());
        let prepared = AttemptJournalState::AttemptPrepared {
            candidate_attempt_number: 1,
            allocation: ControlTransition {
                expected: control(0, None),
                planned: control(1, Some(attempt_id())),
            },
            retry_authorization: None,
        };
        store
            .with_locked_attempt(&attempt_id(), |guard| {
                guard.open(&AttemptJournal {
                    generation: 0,
                    attempt: attempt_id(),
                    publication: publication(),
                    state: prepared.clone(),
                })?;
                guard.transition_locked(&prepared, dispatching())
            })
            .unwrap();
        store
    }

    #[test]
    fn the_candidate_is_durable_before_the_outcome_commits() {
        let directory = tempfile::tempdir().unwrap();
        let journals = dispatched(&directory);
        let outcomes = PublicationOutcomeStore::new(directory.path().join("outcome"));

        journals
            .with_locked_attempt(&attempt_id(), |guard| {
                prepare_and_record_primary_outcome(
                    guard,
                    &outcomes,
                    &identity(),
                    &succeeded(),
                    outcome_event(),
                    clear(),
                )
            })
            .unwrap();

        let state = journals
            .read_unlocked(&attempt_id())
            .unwrap()
            .unwrap()
            .state;
        assert!(matches!(state, AttemptJournalState::OutcomeRecorded { .. }));
        assert_eq!(
            outcomes.primary_outcome(&attempt_ref()).unwrap(),
            Some(succeeded())
        );
    }

    #[test]
    fn a_crash_between_the_candidate_and_the_commit_is_finished_idempotently() {
        // The recoverable window: OutcomePrepared is durable, record_once did
        // not run. Re-entering with the same candidate must converge.
        let directory = tempfile::tempdir().unwrap();
        let journals = dispatched(&directory);
        let outcomes = PublicationOutcomeStore::new(directory.path().join("outcome"));

        let digest = succeeded().digest().unwrap();
        journals
            .with_locked_attempt(&attempt_id(), |guard| {
                guard.transition_locked(
                    &dispatching(),
                    AttemptJournalState::OutcomePrepared {
                        attempt: attempt_ref(),
                        attempt_number: 1,
                        candidate: digest.clone(),
                        receipt: identity().receipt,
                        outcome_event: outcome_event(),
                    },
                )
            })
            .unwrap();

        journals
            .with_locked_attempt(&attempt_id(), |guard| {
                prepare_and_record_primary_outcome(
                    guard,
                    &outcomes,
                    &identity(),
                    &succeeded(),
                    outcome_event(),
                    clear(),
                )
            })
            .unwrap();

        assert_eq!(
            outcomes.primary_outcome(&attempt_ref()).unwrap(),
            Some(succeeded())
        );
    }

    #[test]
    fn a_slow_reply_never_overwrites_an_outcome_another_actor_committed() {
        // Recovery recorded Indeterminate while the call was still in flight —
        // legitimate, because no lock was held. The external system's real answer
        // then arrives. It is information, not a candidate.
        let directory = tempfile::tempdir().unwrap();
        let journals = dispatched(&directory);
        let outcomes = PublicationOutcomeStore::new(directory.path().join("outcome"));

        journals
            .with_locked_attempt(&attempt_id(), |guard| {
                prepare_and_record_primary_outcome(
                    guard,
                    &outcomes,
                    &identity(),
                    &indeterminate(),
                    outcome_event(),
                    clear(),
                )
            })
            .unwrap();

        let error = journals
            .with_locked_attempt(&attempt_id(), |guard| {
                prepare_and_record_primary_outcome(
                    guard,
                    &outcomes,
                    &identity(),
                    &succeeded(),
                    outcome_event(),
                    clear(),
                )
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
        assert_eq!(
            outcomes.primary_outcome(&attempt_ref()).unwrap(),
            Some(indeterminate()),
            "the authoritative outcome must survive the later reply"
        );
    }

    #[test]
    fn the_returning_worker_branches_on_what_it_reads_not_on_what_it_assumes() {
        let digest = succeeded().digest().unwrap();
        let other = indeterminate().digest().unwrap();

        assert_eq!(
            converge(&dispatching(), &digest),
            ReturningWorker::RecordCandidate
        );
        assert_eq!(
            converge(
                &AttemptJournalState::OutcomePrepared {
                    attempt: attempt_ref(),
                    attempt_number: 1,
                    candidate: digest.clone(),
                    receipt: identity().receipt,
                    outcome_event: outcome_event(),
                },
                &digest
            ),
            ReturningWorker::ContinueFromDurableCandidate
        );
        assert!(matches!(
            converge(
                &AttemptJournalState::OutcomePrepared {
                    attempt: attempt_ref(),
                    attempt_number: 1,
                    candidate: other,
                    receipt: identity().receipt,
                    outcome_event: outcome_event(),
                },
                &digest
            ),
            ReturningWorker::Inconsistent { .. }
        ));
        assert_eq!(
            converge(
                &AttemptJournalState::OutcomeRecorded {
                    attempt: attempt_ref(),
                    attempt_number: 1,
                    outcome: digest.clone(),
                    receipt: identity().receipt,
                    control_clear: clear(),
                },
                &digest
            ),
            ReturningWorker::ResultIsLateInformation
        );
    }

    #[test]
    fn a_reply_for_an_attempt_recorded_as_never_dispatched_is_an_inconsistency() {
        // Both halves: never reached the dispatch boundary, and recorded as
        // abandoned. Nothing external can have replied in either world.
        let digest = succeeded().digest().unwrap();

        assert!(matches!(
            converge(
                &AttemptJournalState::AttemptPrepared {
                    candidate_attempt_number: 1,
                    allocation: ControlTransition {
                        expected: control(0, None),
                        planned: control(1, Some(attempt_id())),
                    },
                    retry_authorization: None,
                },
                &digest
            ),
            ReturningWorker::Inconsistent { .. }
        ));
        assert!(matches!(
            converge(
                &AttemptJournalState::Abandoned {
                    evidence: NonCommitEvidence {
                        candidate_attempt_number: 1,
                        observed_control: control(0, None),
                        classified_at: Timestamp::from_unix_nanos(0),
                    },
                },
                &digest
            ),
            ReturningWorker::Inconsistent { .. }
        ));
    }

    #[test]
    fn a_finalized_abandonment_treats_a_reply_as_late_information() {
        // The attempt was withdrawn before dispatch, so nothing should reply —
        // but if something does, it is late information about a Publication,
        // never a second candidate to compete with.
        let digest = succeeded().digest().unwrap();
        assert_eq!(
            converge(
                &AttemptJournalState::Finalized {
                    terminal_disposition: TerminalDisposition::AbandonedBeforeDispatch {
                        attempt_number: 1,
                        reason: "the binding was retargeted".into(),
                        control_clear: clear(),
                        abandonment_event: ActivityEventId::parse("evt_000000000003").unwrap(),
                    },
                },
                &digest
            ),
            ReturningWorker::ResultIsLateInformation
        );
    }
}
