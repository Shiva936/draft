//! A publication survives being interrupted anywhere, and never twice-dispatches.
//!
//! The unit tests check each rule in isolation. What they cannot show is that
//! the rules compose — that the state a crash leaves behind is one the restart
//! table classifies, and that acting on that classification reaches the same
//! terminal state the uninterrupted run does.
//!
//! So these walk the whole lifecycle, stopping at each durable boundary and
//! asking the recovery classifier what it sees.

use draft_core::publication::completion::prepare_and_record_primary_outcome;
use draft_core::publication::control::{PublicationControl, PublicationControlStore};
use draft_core::publication::journal::{
    AttemptJournal, AttemptJournalState, ControlTransition, NonCommitEvidence,
    PublicationJournalStore, TerminalDisposition,
};
use draft_core::publication::outcome::{PrimaryOutcomeIdentity, PublicationOutcomeStore};
use draft_core::publication::restart::{
    classify, AttemptResolution, AttemptSnapshot, DispatchRecoveryClass, OutcomePresence,
};
use draft_dcg_contract::identifier::NamespacedId;
use draft_dcg_contract::ids::{
    ActivityEventId, ActorId, PublicationAttemptId, PublicationId, ReceiptId,
};
use draft_dcg_contract::producer::ProducerIdentity;
use draft_dcg_contract::publication::{
    PublicationAttemptDigest, PublicationAttemptRef, PublicationOutcome, PublicationOutcomeKind,
};
use draft_dcg_contract::receipt::ReceiptSignerBinding;
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

fn attempt_ref(id: &PublicationAttemptId) -> PublicationAttemptRef {
    PublicationAttemptRef {
        id: id.clone(),
        digest: PublicationAttemptDigest::new(Digest::of_bytes(b"attempt")),
    }
}

fn identity(id: &PublicationAttemptId) -> PrimaryOutcomeIdentity {
    PrimaryOutcomeIdentity {
        attempt: attempt_ref(id),
        receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
        signer: ReceiptSignerBinding::new(
            ActorId::parse("act_000000000001").unwrap(),
            "key-1",
            "ed25519",
        )
        .unwrap(),
    }
}

fn succeeded(id: &PublicationAttemptId) -> PublicationOutcome {
    PublicationOutcome {
        attempt: attempt_ref(id),
        receipt_id: identity(id).receipt,
        receipt_signer: identity(id).signer,
        outcome: PublicationOutcomeKind::Succeeded {
            external_reference: "remote-1".into(),
        },
        concluded_at: Timestamp::from_unix_nanos(0),
        provenance: ProducerIdentity::new(
            NamespacedId::parse("draft.core/publication").unwrap(),
            "1",
        )
        .unwrap(),
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    control: PublicationControlStore,
    journals: PublicationJournalStore,
    outcomes: PublicationOutcomeStore,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let control = PublicationControlStore::new(directory.path().join("publication/control"));
        control
            .initialize(&PublicationControl::initial(publication()))
            .unwrap();
        Self {
            journals: PublicationJournalStore::new(directory.path().join("publication/journal")),
            outcomes: PublicationOutcomeStore::new(directory.path().join("publication/outcome")),
            control,
            _directory: directory,
        }
    }

    /// Phases 2 and 3: allocate, then make the dispatch boundary durable.
    ///
    /// `commit` chooses whether the allocation mutation lands, which is the one
    /// difference between the two worlds recovery must tell apart.
    fn allocate(&self, id: &PublicationAttemptId, commit: bool) -> ControlTransition {
        self.journals
            .with_locked_attempt(id, |journal_guard| {
                self.control.with_locked_control(&publication(), |control| {
                    let planned = control.plan_allocation(id.clone(), None)?;
                    let allocation = ControlTransition {
                        expected: planned.expected_control.clone(),
                        planned: planned.planned_control.clone(),
                    };
                    journal_guard.open(&AttemptJournal {
                        generation: 0,
                        attempt: id.clone(),
                        publication: publication(),
                        state: AttemptJournalState::AttemptPrepared {
                            candidate_attempt_number: planned.candidate_attempt_number,
                            allocation: allocation.clone(),
                            retry_authorization: None,
                        },
                    })?;
                    if commit {
                        control.commit_allocation(&planned)?;
                    }
                    Ok(allocation)
                })
            })
            .unwrap()
    }

    fn dispatch(&self, id: &PublicationAttemptId, allocation: &ControlTransition) {
        self.journals
            .with_locked_attempt(id, |guard| {
                let current = guard.current_state()?.unwrap();
                guard.transition_locked(
                    &current,
                    AttemptJournalState::Dispatching {
                        attempt: attempt_ref(id),
                        attempt_number: 1,
                        dispatch_event: ActivityEventId::parse("evt_000000000001").unwrap(),
                    },
                )?;
                let _ = allocation;
                Ok(())
            })
            .unwrap();
    }

    fn classify_now(&self, id: &PublicationAttemptId) -> AttemptResolution {
        let journal = self.journals.read_unlocked(id).unwrap().unwrap().state;
        let control = self.control.read_unlocked(&publication()).unwrap();
        let outcome = match self.outcomes.primary_outcome(&attempt_ref(id)).unwrap() {
            Some(value) => OutcomePresence::Present(value.digest().unwrap()),
            None => OutcomePresence::Absent,
        };
        classify(&AttemptSnapshot {
            attempt: id,
            journal: &journal,
            control: control.as_ref(),
            outcome: &outcome,
            recovery_class: DispatchRecoveryClass::ResolvableLocally,
        })
    }
}

#[test]
fn an_uninterrupted_publication_reaches_finalized_with_its_outcome() {
    let fixture = Fixture::new();
    let allocation = fixture.allocate(&attempt_a(), true);
    fixture.dispatch(&attempt_a(), &allocation);

    // The external call happens here, holding nothing, and returns.
    let clear = ControlTransition {
        expected: allocation.planned.clone(),
        planned: allocation.planned.advanced(|next| {
            next.in_flight_attempt = None;
        }),
    };
    fixture
        .journals
        .with_locked_attempt(&attempt_a(), |guard| {
            prepare_and_record_primary_outcome(
                guard,
                &fixture.outcomes,
                &identity(&attempt_a()),
                &succeeded(&attempt_a()),
                ActivityEventId::parse("evt_000000000002").unwrap(),
                clear.clone(),
            )
        })
        .unwrap();

    // Phase C: clear the exact in-flight attempt, then finalize. The order is
    // frozen — clear first, so a Finalized attempt is never still referenced.
    assert_eq!(
        fixture.classify_now(&attempt_a()),
        AttemptResolution::CompleteThenClear
    );
    fixture
        .control
        .with_locked_control(&publication(), |control| {
            control.commit_exact(&clear.expected, &clear.planned)
        })
        .unwrap();
    assert_eq!(
        fixture.classify_now(&attempt_a()),
        AttemptResolution::VerifyThenFinalize
    );

    let outcome = succeeded(&attempt_a()).digest().unwrap();
    fixture
        .journals
        .with_locked_attempt(&attempt_a(), |guard| {
            let current = guard.current_state()?.unwrap();
            guard.transition_locked(
                &current,
                AttemptJournalState::Finalized {
                    terminal_disposition: TerminalDisposition::OutcomeFinalized {
                        attempt: attempt_ref(&attempt_a()),
                        outcome: outcome.clone(),
                        receipt: identity(&attempt_a()).receipt,
                        control_clear: clear.clone(),
                    },
                },
            )
        })
        .unwrap();

    assert_eq!(
        fixture.classify_now(&attempt_a()),
        AttemptResolution::AlreadyTerminal
    );
}

#[test]
fn a_crash_before_the_allocation_committed_never_dispatches_that_attempt() {
    // The world where nothing was spent. pat_A is marked terminal, and pat_B
    // legitimately receives the number pat_A only ever proposed.
    let fixture = Fixture::new();
    fixture.allocate(&attempt_a(), false);

    assert_eq!(
        fixture.classify_now(&attempt_a()),
        AttemptResolution::AllocationDidNotCommit
    );

    let prepared = fixture
        .journals
        .read_unlocked(&attempt_a())
        .unwrap()
        .unwrap()
        .state;
    let observed = fixture
        .control
        .read_unlocked(&publication())
        .unwrap()
        .unwrap();
    fixture
        .journals
        .with_locked_attempt(&attempt_a(), |guard| {
            guard.transition_locked(
                &prepared,
                AttemptJournalState::Abandoned {
                    evidence: NonCommitEvidence {
                        candidate_attempt_number: 1,
                        observed_control: observed,
                        classified_at: Timestamp::from_unix_nanos(0),
                    },
                },
            )
        })
        .unwrap();

    let allocation = fixture.allocate(&attempt_b(), true);
    assert_eq!(
        allocation.planned.next_attempt_number, 2,
        "pat_B takes the number pat_A never consumed"
    );

    // And pat_A's terminal state survives pat_B's legitimate progress.
    assert_eq!(
        fixture.classify_now(&attempt_a()),
        AttemptResolution::AlreadyTerminal
    );
}

#[test]
fn a_crash_after_the_allocation_committed_resumes_the_same_attempt() {
    // The world where a number was spent. Allocating pat_B here would be a
    // second external effect against a Publication already committed to one.
    let fixture = Fixture::new();
    fixture.allocate(&attempt_a(), true);

    assert_eq!(
        fixture.classify_now(&attempt_a()),
        AttemptResolution::ResumeAllocatedAttempt {
            attempt: attempt_a()
        }
    );

    let refused = fixture
        .control
        .with_locked_control(&publication(), |control| {
            control.plan_allocation(attempt_b(), None)
        });
    assert!(
        refused.is_err(),
        "a Publication with an attempt in flight must refuse a second allocation"
    );
}

#[test]
fn a_crash_between_the_candidate_and_the_outcome_converges_on_one_answer() {
    // The recoverable window inside the completion path: OutcomePrepared is
    // durable, the outcome is not. A retry finishes it; it never re-competes.
    let fixture = Fixture::new();
    let allocation = fixture.allocate(&attempt_a(), true);
    fixture.dispatch(&attempt_a(), &allocation);

    let clear = ControlTransition {
        expected: allocation.planned.clone(),
        planned: allocation.planned.advanced(|next| {
            next.in_flight_attempt = None;
        }),
    };
    let digest = succeeded(&attempt_a()).digest().unwrap();
    fixture
        .journals
        .with_locked_attempt(&attempt_a(), |guard| {
            let current = guard.current_state()?.unwrap();
            guard.transition_locked(
                &current,
                AttemptJournalState::OutcomePrepared {
                    attempt: attempt_ref(&attempt_a()),
                    attempt_number: 1,
                    candidate: digest.clone(),
                    receipt: identity(&attempt_a()).receipt,
                    outcome_event: ActivityEventId::parse("evt_000000000002").unwrap(),
                },
            )
        })
        .unwrap();

    assert_eq!(
        fixture.classify_now(&attempt_a()),
        AttemptResolution::RetryOutcomeCommit
    );

    fixture
        .journals
        .with_locked_attempt(&attempt_a(), |guard| {
            prepare_and_record_primary_outcome(
                guard,
                &fixture.outcomes,
                &identity(&attempt_a()),
                &succeeded(&attempt_a()),
                ActivityEventId::parse("evt_000000000002").unwrap(),
                clear,
            )
        })
        .unwrap();

    assert_eq!(
        fixture
            .outcomes
            .primary_outcome(&attempt_ref(&attempt_a()))
            .unwrap(),
        Some(succeeded(&attempt_a()))
    );
    assert_eq!(
        fixture.classify_now(&attempt_a()),
        AttemptResolution::CompleteThenClear
    );
}

#[test]
fn the_pre_dispatch_abandonment_path_consumes_its_number_and_leaves_a_gap() {
    // The committed allocation whose dispatch is refused. The number is spent,
    // so the next attempt gets the following one — the only legal source of a
    // gap in the sequence.
    let fixture = Fixture::new();
    let allocation = fixture.allocate(&attempt_a(), true);
    let clear = ControlTransition {
        expected: allocation.planned.clone(),
        planned: allocation.planned.advanced(|next| {
            next.in_flight_attempt = None;
        }),
    };
    let event = ActivityEventId::parse("evt_000000000003").unwrap();

    let abandon_prepared = AttemptJournalState::AbandonPrepared {
        attempt_number: 1,
        reason: "the binding was retargeted".into(),
        control_clear: clear.clone(),
        abandonment_event: event.clone(),
    };
    let prepared = fixture
        .journals
        .read_unlocked(&attempt_a())
        .unwrap()
        .unwrap()
        .state;
    fixture
        .journals
        .with_locked_attempt(&attempt_a(), |guard| {
            guard.transition_locked(&prepared, abandon_prepared.clone())
        })
        .unwrap();

    assert_eq!(
        fixture.classify_now(&attempt_a()),
        AttemptResolution::CompleteThenClear
    );

    fixture
        .control
        .with_locked_control(&publication(), |control| {
            control.commit_exact(&clear.expected, &clear.planned)
        })
        .unwrap();
    assert_eq!(
        fixture.classify_now(&attempt_a()),
        AttemptResolution::DrainThenFinalize
    );

    let abandoned = AttemptJournalState::AbandonedBeforeDispatch {
        attempt_number: 1,
        reason: "the binding was retargeted".into(),
        control_clear: clear.clone(),
        abandonment_event: event.clone(),
    };
    fixture
        .journals
        .with_locked_attempt(&attempt_a(), |guard| {
            guard.transition_locked(&abandon_prepared, abandoned.clone())?;
            guard.transition_locked(
                &abandoned,
                AttemptJournalState::Finalized {
                    terminal_disposition: TerminalDisposition::AbandonedBeforeDispatch {
                        attempt_number: 1,
                        reason: "the binding was retargeted".into(),
                        control_clear: clear,
                        abandonment_event: event,
                    },
                },
            )
        })
        .unwrap();

    // No primary outcome, and that absence is correct rather than a gap.
    assert_eq!(
        fixture.classify_now(&attempt_a()),
        AttemptResolution::AlreadyTerminal
    );

    let next = fixture.allocate(&attempt_b(), true);
    assert_eq!(
        next.planned.next_attempt_number, 3,
        "pat_B takes 2, leaving 1 consumed by the abandoned attempt"
    );
}
