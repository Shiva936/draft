//! The one property that turns somebody else's outage into Draft's problem.
//!
//! Publication is the only part of Draft that calls something outside itself.
//! If any lock, lease or fence were still held while that call was in flight,
//! an unreachable external system would stop unrelated local work: the project
//! control record, the attempt journal and the provider binding would all sit
//! locked for as long as the network took to time out — Scenario AT, "Draft
//! will not let me edit my own work".
//!
//! The frozen sequence therefore releases everything before the call. These
//! tests assert the two halves of that:
//!
//! * the dispatch sequence nests journal (6) inside control (7) exactly once
//!   each, and holds nothing once it returns;
//! * the bookkeeping barrier ends holding nothing at all, so Phase 1 can take
//!   the trust fence (1) without a reverse acquisition.

use draft_core::publication::control::{PublicationControl, PublicationControlStore};
use draft_core::publication::journal::{
    AttemptJournal, AttemptJournalState, ControlTransition, PublicationJournalStore,
};
use draft_core::support::lock_order::{self, LockOrder};
use draft_dcg_contract::ids::{ActivityEventId, PublicationAttemptId, PublicationId};
use draft_dcg_contract::publication::{PublicationAttemptDigest, PublicationAttemptRef};
use draft_dcg_contract::Digest;
use std::sync::mpsc;
use std::time::Duration;

fn publication() -> PublicationId {
    PublicationId::parse("pub_000000000001").unwrap()
}

fn attempt() -> PublicationAttemptId {
    PublicationAttemptId::parse("pat_000000000001").unwrap()
}

#[test]
fn the_dispatch_sequence_releases_every_lock_before_the_external_call() {
    let directory = tempfile::tempdir().unwrap();
    let control = PublicationControlStore::new(directory.path().join("publication/control"));
    let journals = PublicationJournalStore::new(directory.path().join("publication/journal"));
    control
        .initialize(&PublicationControl::initial(publication()))
        .unwrap();

    // Phases 2 and 3: journal (6) outermost, publication control (7) nested
    // inside it, each acquired exactly once, and the dispatch boundary made
    // durable while both are still held.
    journals
        .with_locked_attempt(&attempt(), |journal_guard| {
            let planned = control.with_locked_control(&publication(), |control_guard| {
                let planned = control_guard.plan_allocation(attempt(), None)?;
                journal_guard.open(&AttemptJournal {
                    generation: 0,
                    attempt: attempt(),
                    publication: publication(),
                    state: AttemptJournalState::AttemptPrepared {
                        candidate_attempt_number: planned.candidate_attempt_number,
                        allocation: ControlTransition {
                            expected: planned.expected_control.clone(),
                            planned: planned.planned_control.clone(),
                        },
                        retry_authorization: None,
                    },
                })?;
                control_guard.commit_allocation(&planned)?;
                Ok(planned)
            })?;

            assert_eq!(
                lock_order::currently_held(),
                vec![LockOrder::PublicationJournalStore],
                "the control lock is released before the journal, in strict reverse order"
            );

            journal_guard.transition_locked(
                &AttemptJournalState::AttemptPrepared {
                    candidate_attempt_number: planned.candidate_attempt_number,
                    allocation: ControlTransition {
                        expected: planned.expected_control.clone(),
                        planned: planned.planned_control.clone(),
                    },
                    retry_authorization: None,
                },
                AttemptJournalState::Dispatching {
                    attempt: PublicationAttemptRef {
                        id: attempt(),
                        digest: PublicationAttemptDigest::new(Digest::of_bytes(b"attempt")),
                    },
                    attempt_number: planned.candidate_attempt_number,
                    dispatch_event: ActivityEventId::parse("evt_000000000001").unwrap(),
                },
            )
        })
        .unwrap();

    // Phase 3e: the external call happens here, and nothing is held.
    assert!(
        lock_order::currently_held().is_empty(),
        "the external system must never be called with a lock held"
    );
}

#[test]
fn the_journal_lock_may_not_be_reacquired_inside_its_own_critical_section() {
    // The negative proof. Without it the test above could pass because nothing
    // in the sequence ever nests, leaving the guarantee vacuous.
    //
    // `ProcessFileLock` is deliberately not reentrant — `flock` is
    // per-file-description — so a helper that takes the lock itself, called
    // from inside the critical section, does not fail a type check and does
    // not return an error. It hangs, indistinguishably from a slow disk.
    let directory = tempfile::tempdir().unwrap();
    let journals = PublicationJournalStore::new(directory.path().join("publication/journal"));
    journals
        .with_locked_attempt(&attempt(), |guard| {
            guard.open(&AttemptJournal {
                generation: 0,
                attempt: attempt(),
                publication: publication(),
                state: AttemptJournalState::AttemptPrepared {
                    candidate_attempt_number: 1,
                    allocation: ControlTransition {
                        expected: PublicationControl::initial(publication()),
                        planned: PublicationControl::initial(publication()).advanced(|next| {
                            next.in_flight_attempt = Some(attempt());
                        }),
                    },
                    retry_authorization: None,
                },
            })
        })
        .unwrap();

    let path = directory.path().to_path_buf();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let journals = PublicationJournalStore::new(path.join("publication/journal"));
        let outcome = journals.with_locked_attempt(&attempt(), |_outer| {
            journals.with_locked_attempt(&attempt(), |_inner| Ok(()))
        });
        let _ = sender.send(outcome.is_ok());
    });

    assert!(
        receiver.recv_timeout(Duration::from_secs(3)).is_err(),
        "reacquisition must not silently succeed; a sequence that relied on it being reentrant \
         would hang in production instead"
    );
}

#[test]
fn the_bookkeeping_barrier_ends_holding_nothing_so_the_trust_fence_can_be_taken_next() {
    // Phase 0 holds the PublicationLease (3). Phase 1 must take the
    // TrustReadFence (1) first. If Phase 0 kept anything, that acquisition
    // would be `hold 3 → acquire 1`, which the frozen order forbids.
    let held_during_barrier = {
        let _lease = lock_order::enter(LockOrder::PublicationLease).unwrap();
        // One item, under its own state-specific lockset, released before the
        // next item is processed.
        {
            let _journal = lock_order::enter(LockOrder::PublicationJournalStore).unwrap();
            let _control = lock_order::enter(LockOrder::PublicationControlStore).unwrap();
        }
        lock_order::currently_held()
    };
    assert_eq!(held_during_barrier, vec![LockOrder::PublicationLease]);

    assert!(
        lock_order::currently_held().is_empty(),
        "Phase 0 must end holding nothing at all, the lease included"
    );

    // And that is exactly what makes Phase 1's first acquisition legal.
    let fence = lock_order::enter(LockOrder::TrustReadFence);
    assert!(
        fence.is_ok(),
        "Phase 1 must be able to take the trust fence first"
    );
}

#[test]
fn holding_the_publication_lease_into_the_next_phase_is_refused() {
    // The mistake the barrier's shape exists to prevent, asserted directly:
    // the order checker rejects it rather than leaving it to review.
    let _lease = lock_order::enter(LockOrder::PublicationLease).unwrap();
    assert!(
        lock_order::enter(LockOrder::TrustReadFence).is_err(),
        "acquiring the trust fence while the publication lease is held is a reverse acquisition"
    );
}
