//! The per-attempt Publication journal.
//!
//! Not a scratch marker: this is the authoritative local record of what Draft
//! durably committed for one `pat_`. Several local actors legitimately touch
//! the same attempt — the original worker, recovery, reconciliation, a
//! late-result handler, a daemon-restart finalizer — so it can never be
//! modified by unguarded read-and-overwrite. Every read that decides anything,
//! and every transition, happens under [`PublicationJournalStore`]'s lock.
//!
//! # The state machine
//!
//! ```text
//! allocation committed:
//!     AttemptPrepared → Dispatching → OutcomePrepared → OutcomeRecorded → Finalized
//!
//! allocation committed, then refused before dispatch:
//!     AttemptPrepared → AbandonPrepared → AbandonedBeforeDispatch → Finalized
//!
//! allocation proven never to have committed:
//!     AttemptPrepared → Abandoned                    (TERMINAL)
//! ```
//!
//! # Why `Abandoned` and `AbandonedBeforeDispatch` are different states
//!
//! They describe opposite facts about whether anything became authoritative.
//!
//! `Abandoned` means the allocation mutation is *proven never to have
//! committed*: no attempt number was consumed, no retry authorization was
//! spent, no immutable attempt exists, nothing was dispatched, and there is no
//! audit fact awaiting a drain. There is nothing to finalize, so it is
//! terminal in itself — there is no `Abandoned → Finalized`.
//!
//! `AbandonedBeforeDispatch` means the allocation *did* commit and fresh
//! validation then refused the dispatch. The number is consumed (this is the
//! only source of a legal gap in the sequence), the retry authorization is
//! spent if one was supplied, and exactly one
//! `PublicationAbandonedBeforeDispatch` fact must be drained — so it finishes
//! at `Finalized` like the outcome path does.
//!
//! Collapsing them would make recovery guess which of those two worlds it is
//! in, and the two answers differ on whether an external attempt number and a
//! one-shot authorization were spent.
//!
//! # Why `Finalized` carries its disposition
//!
//! A finalized attempt does not necessarily have a primary outcome. The
//! outcome path must have one; the abandonment path must not. If `Finalized`
//! were an opaque marker, recovery would have to infer which path ran from
//! current mutable state — which by then has legitimately moved on — and would
//! either demand an outcome that correctly does not exist or accept a missing
//! one that should have been there.
//!
//! So [`TerminalDisposition`] is part of the durable state, and the absence of
//! an outcome under `AbandonedBeforeDispatch` is a checkable fact rather than
//! an unexplained gap.

use serde::{Deserialize, Serialize};

use draft_dcg_contract::ids::{ActivityEventId, PublicationAttemptId, ReceiptId};
use draft_dcg_contract::publication::{
    PublicationAttemptRef, PublicationOutcomeDigest, PublicationRetryAuthorizationDigest,
};

use crate::publication::control::PublicationControl;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::lock_order::LockOrder;
use crate::support::record_guard::{
    ExpectedRecordState, RecordGuard, RevisionedRecord, RevisionedRecordStore, DEFAULT_LOCK_TIMEOUT,
};

/// The exact control-state pair a transaction was planned against.
///
/// Always the whole value on both sides. Recovery classifies a crash around a
/// commit by comparing the current record against these in full — never by
/// inspecting `in_flight_attempt`, which cannot distinguish a record that was
/// cleared from one that never moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlTransition {
    pub expected: PublicationControl,
    pub planned: PublicationControl,
}

/// Why an attempt whose allocation never committed was abandoned.
///
/// Persisted, not recomputed. The conclusion "this allocation never committed"
/// is proved once, at classification time, by an exact whole-value comparison;
/// afterwards the current control record has legitimately moved on and the
/// comparison can no longer be re-run. Keeping the evidence is what makes the
/// terminal state verifiable later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NonCommitEvidence {
    /// The candidate number this attempt would have claimed. Never consumed.
    pub candidate_attempt_number: u32,
    /// The exact control value observed at classification time, which equalled
    /// the allocation's expected value and so proved the commit never landed.
    pub observed_control: PublicationControl,
    pub classified_at: draft_dcg_contract::value::Timestamp,
}

/// Which terminal path a `Finalized` attempt took.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum TerminalDisposition {
    /// The attempt dispatched and concluded. An exact primary outcome must
    /// exist and verify against this digest.
    OutcomeFinalized {
        attempt: PublicationAttemptRef,
        outcome: PublicationOutcomeDigest,
        receipt: ReceiptId,
        /// The exact clear that ran before this state became durable.
        control_clear: ControlTransition,
    },
    /// The allocation committed and dispatch was refused before it happened.
    /// **No** primary outcome exists, and that absence is correct.
    AbandonedBeforeDispatch {
        /// The number this attempt consumed — the source of a legal gap.
        attempt_number: u32,
        reason: String,
        /// The exact clear that ran before this state became durable.
        control_clear: ControlTransition,
        /// The one abandonment fact, drained exactly once.
        abandonment_event: ActivityEventId,
    },
}

impl TerminalDisposition {
    /// Whether this disposition requires a primary outcome to exist.
    pub fn expects_primary_outcome(&self) -> bool {
        matches!(self, Self::OutcomeFinalized { .. })
    }
}

/// What Draft has durably committed for one attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AttemptJournalState {
    /// The allocation is planned and written; whether it committed is decided
    /// by comparing the control record against `allocation`.
    AttemptPrepared {
        candidate_attempt_number: u32,
        allocation: ControlTransition,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_authorization: Option<PublicationRetryAuthorizationDigest>,
    },
    /// The dispatch boundary is durable. A complete immutable attempt exists
    /// and verifies against this reference, and the dispatch fact is durable
    /// in this same state — so a crash before the Activity append cannot lose
    /// it, and `app/activity` emits it once rather than reconstructing it from
    /// mutable state.
    Dispatching {
        attempt: PublicationAttemptRef,
        /// The authoritative number, carried forward from the committed
        /// allocation. Retained rather than re-derived: the immutable attempt
        /// also states its number, so checking one against the other needs a
        /// second, independent record of what was actually reserved.
        attempt_number: u32,
        dispatch_event: ActivityEventId,
    },
    /// A candidate outcome and its audit material are durable, before the
    /// outcome itself commits. One attempt has one candidate: a second actor
    /// that acquires the guard observes this and may not prepare a different
    /// one.
    OutcomePrepared {
        attempt: PublicationAttemptRef,
        attempt_number: u32,
        candidate: PublicationOutcomeDigest,
        receipt: ReceiptId,
        outcome_event: ActivityEventId,
    },
    /// The primary outcome is committed. The exact clear this attempt will
    /// perform is frozen here, whole-value on both sides.
    OutcomeRecorded {
        attempt: PublicationAttemptRef,
        attempt_number: u32,
        outcome: PublicationOutcomeDigest,
        receipt: ReceiptId,
        control_clear: ControlTransition,
    },
    /// The committed allocation is being withdrawn before dispatch. The exact
    /// clear is frozen here before it runs.
    AbandonPrepared {
        attempt_number: u32,
        reason: String,
        control_clear: ControlTransition,
        abandonment_event: ActivityEventId,
    },
    /// The clear committed. The abandonment fact may still need draining.
    AbandonedBeforeDispatch {
        attempt_number: u32,
        reason: String,
        control_clear: ControlTransition,
        abandonment_event: ActivityEventId,
    },
    /// Terminal. The allocation is proven never to have committed, so there is
    /// nothing to finalize and no `Finalized` transition exists from here.
    Abandoned { evidence: NonCommitEvidence },
    /// Terminal, retaining which path got here.
    Finalized {
        terminal_disposition: TerminalDisposition,
    },
}

impl AttemptJournalState {
    /// A short stable name, for errors and diagnostics.
    pub fn name(&self) -> &'static str {
        match self {
            Self::AttemptPrepared { .. } => "AttemptPrepared",
            Self::Dispatching { .. } => "Dispatching",
            Self::OutcomePrepared { .. } => "OutcomePrepared",
            Self::OutcomeRecorded { .. } => "OutcomeRecorded",
            Self::AbandonPrepared { .. } => "AbandonPrepared",
            Self::AbandonedBeforeDispatch { .. } => "AbandonedBeforeDispatch",
            Self::Abandoned { .. } => "Abandoned",
            Self::Finalized { .. } => "Finalized",
        }
    }

    /// The exact attempt this state dispatched, from `Dispatching` onwards.
    ///
    /// Distinct from [`Self::concluded_attempt`]: this says an effect was
    /// attempted, that one says an outcome was recorded. Collapsing them would
    /// make "we sent it and do not know" indistinguishable from "we never
    /// sent it".
    pub fn dispatched_attempt(&self) -> Option<&PublicationAttemptRef> {
        match self {
            Self::Dispatching { attempt, .. }
            | Self::OutcomePrepared { attempt, .. }
            | Self::OutcomeRecorded { attempt, .. } => Some(attempt),
            _ => None,
        }
    }

    /// The exact attempt this state concluded, where it concluded one.
    ///
    /// Only the states past the outcome commit answer. An attempt that reached
    /// the dispatch boundary and no further concluded nothing, and reporting
    /// its reference here would let a reader look for an outcome that does not
    /// exist and read the absence as a failure.
    pub fn concluded_attempt(&self) -> Option<&PublicationAttemptRef> {
        match self {
            Self::OutcomeRecorded { attempt, .. } => Some(attempt),
            _ => None,
        }
    }

    /// The attempt number this state records, where it records one.
    ///
    /// `AttemptPrepared` holds a *candidate*; every state after a committed
    /// allocation holds the authoritative number. Both are the same integer,
    /// and they are read from different variants precisely so the distinction
    /// survives — a caller that needs the authoritative one cannot silently
    /// receive a candidate.
    pub fn attempt_number(&self) -> Option<u32> {
        match self {
            Self::AttemptPrepared {
                candidate_attempt_number,
                ..
            } => Some(*candidate_attempt_number),
            Self::Dispatching { attempt_number, .. }
            | Self::OutcomePrepared { attempt_number, .. }
            | Self::OutcomeRecorded { attempt_number, .. }
            | Self::AbandonPrepared { attempt_number, .. }
            | Self::AbandonedBeforeDispatch { attempt_number, .. } => Some(*attempt_number),
            Self::Abandoned { evidence } => Some(evidence.candidate_attempt_number),
            Self::Finalized {
                terminal_disposition,
            } => match terminal_disposition {
                TerminalDisposition::AbandonedBeforeDispatch { attempt_number, .. } => {
                    Some(*attempt_number)
                }
                TerminalDisposition::OutcomeFinalized { .. } => None,
            },
        }
    }

    /// Whether this state admits no further transition.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Abandoned { .. } | Self::Finalized { .. })
    }

    /// Whether `next` is a legal successor of this state.
    ///
    /// The transition set is closed and exhaustive rather than a list of
    /// forbidden moves: a state pair nobody thought about is refused, not
    /// allowed by omission.
    pub fn permits(&self, next: &Self) -> bool {
        use AttemptJournalState as S;
        matches!(
            (self, next),
            (S::AttemptPrepared { .. }, S::Dispatching { .. })
                | (S::AttemptPrepared { .. }, S::AbandonPrepared { .. })
                | (S::AttemptPrepared { .. }, S::Abandoned { .. })
                | (S::Dispatching { .. }, S::OutcomePrepared { .. })
                | (S::OutcomePrepared { .. }, S::OutcomeRecorded { .. })
                | (S::OutcomeRecorded { .. }, S::Finalized { .. })
                | (S::AbandonPrepared { .. }, S::AbandonedBeforeDispatch { .. })
                | (S::AbandonedBeforeDispatch { .. }, S::Finalized { .. })
        )
    }
}

/// One attempt's journal record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptJournal {
    /// Advances on every transition, covering every field below.
    pub generation: u64,
    pub attempt: PublicationAttemptId,
    pub publication: draft_dcg_contract::ids::PublicationId,
    pub state: AttemptJournalState,
}

impl RevisionedRecord for AttemptJournal {
    fn generation(&self) -> u64 {
        self.generation
    }
}

/// Per-attempt journals, on `publication/journal/<pat_>.lock`.
#[derive(Debug, Clone)]
pub struct PublicationJournalStore {
    records: RevisionedRecordStore<AttemptJournal>,
}

impl PublicationJournalStore {
    /// Open the store over `publication/journal/`.
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            records: RevisionedRecordStore::new(directory)
                .with_order(LockOrder::PublicationJournalStore),
        }
    }

    /// The stable sidecar this attempt's journal lock is held on.
    ///
    /// The journal file itself is replaced by atomic rename and is never the
    /// lock target: a lock on the record's own inode would stop protecting
    /// anything the moment a write landed.
    pub fn lock_path(&self, attempt: &PublicationAttemptId) -> std::path::PathBuf {
        self.records.lock_path(&attempt.to_string())
    }

    /// Read without locking.
    ///
    /// For display and read models only. Every read that *decides* anything
    /// goes through [`Self::with_locked_attempt`], because a decision made
    /// from an unlocked read can be invalidated before it is acted on.
    pub fn read_unlocked(
        &self,
        attempt: &PublicationAttemptId,
    ) -> DraftResult<Option<AttemptJournal>> {
        self.records.read_unlocked(&attempt.to_string())
    }

    /// Every attempt that has a journal.
    ///
    /// Reads the record directory rather than an index, so a read model can
    /// never report fewer attempts than the project actually made.
    pub fn list(&self) -> DraftResult<Vec<PublicationAttemptId>> {
        let mut found = Vec::new();
        for key in self.records.keys()? {
            if let Ok(attempt) = PublicationAttemptId::parse(&key) {
                found.push(attempt);
            }
        }
        Ok(found)
    }

    /// Run `body` with this attempt's journal held for exactly one acquisition.
    ///
    /// The external call is **never** made inside this closure. The lock is
    /// released first, precisely so an unreachable remote system cannot hold a
    /// local journal hostage.
    pub fn with_locked_attempt<R>(
        &self,
        attempt: &PublicationAttemptId,
        body: impl FnOnce(&mut AttemptJournalGuard<'_, '_>) -> DraftResult<R>,
    ) -> DraftResult<R> {
        self.records.with_locked_record(
            &attempt.to_string(),
            DEFAULT_LOCK_TIMEOUT,
            |record_guard| {
                let mut guard = AttemptJournalGuard {
                    inner: record_guard,
                };
                body(&mut guard)
            },
        )
    }
}

/// A live, exclusive hold on one attempt's journal.
pub struct AttemptJournalGuard<'a, 'b> {
    inner: &'b mut RecordGuard<'a, AttemptJournal>,
}

impl AttemptJournalGuard<'_, '_> {
    /// The authoritative current record.
    pub fn current(&self) -> DraftResult<Option<AttemptJournal>> {
        self.inner.current()
    }

    /// The authoritative current state, if the journal exists.
    pub fn current_state(&self) -> DraftResult<Option<AttemptJournalState>> {
        Ok(self.current()?.map(|record| record.state))
    }

    /// Write the opening `AttemptPrepared` record.
    ///
    /// Requires the journal to be absent, so a second worker cannot re-open an
    /// attempt that already has history.
    pub fn open(&mut self, record: &AttemptJournal) -> DraftResult<()> {
        if !matches!(record.state, AttemptJournalState::AttemptPrepared { .. }) {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "an attempt journal opens at AttemptPrepared, not at {}",
                    record.state.name()
                ),
            ));
        }
        self.inner
            .compare_exchange_locked(&ExpectedRecordState::Absent, record)
    }

    /// Guarded compare-exchange on the journal state.
    ///
    /// Refuses on two independent grounds, and both matter:
    ///
    /// * the current state is not exactly `expected` — a stale caller, or a
    ///   transition another actor already made;
    /// * the move itself is not in the frozen transition set — a caller
    ///   inventing a shortcut, such as jumping `Dispatching → OutcomeRecorded`
    ///   and skipping the durable candidate that makes the outcome
    ///   recoverable.
    ///
    /// There is no last-writer-wins overwrite.
    pub fn transition_locked(
        &mut self,
        expected: &AttemptJournalState,
        replacement: AttemptJournalState,
    ) -> DraftResult<AttemptJournal> {
        let current = self.current()?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                "the attempt journal does not exist, so it cannot be transitioned",
            )
        })?;

        if &current.state != expected {
            crate::support::telemetry::Counter::PublicationJournalTransitionConflicts.increment();
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "attempt '{}' is at {} but the caller expected {}",
                    current.attempt,
                    current.state.name(),
                    expected.name()
                ),
            )
            .with_suggestion("Re-read the authoritative journal state and reclassify from it."));
        }

        if !current.state.permits(&replacement) {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "{} → {} is not a legal attempt transition",
                    current.state.name(),
                    replacement.name()
                ),
            ));
        }

        let next = AttemptJournal {
            generation: current.generation + 1,
            state: replacement,
            ..current.clone()
        };
        let expected_state = ExpectedRecordState::of(&current)?;
        self.inner.compare_exchange_locked(&expected_state, &next)?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::ids::PublicationId;
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::Digest;

    fn publication() -> PublicationId {
        PublicationId::parse("pub_000000000001").unwrap()
    }

    fn attempt_id() -> PublicationAttemptId {
        PublicationAttemptId::parse("pat_000000000001").unwrap()
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
            planned: control(1, Some(attempt_id())),
        }
    }

    fn clear() -> ControlTransition {
        ControlTransition {
            expected: control(1, Some(attempt_id())),
            planned: control(2, None),
        }
    }

    fn prepared() -> AttemptJournalState {
        AttemptJournalState::AttemptPrepared {
            candidate_attempt_number: 1,
            allocation: allocation(),
            retry_authorization: None,
        }
    }

    fn attempt_ref() -> PublicationAttemptRef {
        PublicationAttemptRef {
            id: attempt_id(),
            digest: draft_dcg_contract::publication::PublicationAttemptDigest::new(
                Digest::of_bytes(b"attempt"),
            ),
        }
    }

    fn dispatching() -> AttemptJournalState {
        AttemptJournalState::Dispatching {
            attempt: attempt_ref(),
            attempt_number: 1,
            dispatch_event: ActivityEventId::parse("evt_000000000001").unwrap(),
        }
    }

    fn opened(store: &PublicationJournalStore) -> AttemptJournal {
        let record = AttemptJournal {
            generation: 0,
            attempt: attempt_id(),
            publication: publication(),
            state: prepared(),
        };
        store
            .with_locked_attempt(&attempt_id(), |guard| guard.open(&record))
            .unwrap();
        record
    }

    #[test]
    fn a_journal_opens_at_attempt_prepared_and_advances_through_the_dispatch_path() {
        let directory = tempfile::tempdir().unwrap();
        let store = PublicationJournalStore::new(directory.path());
        opened(&store);

        store
            .with_locked_attempt(&attempt_id(), |guard| {
                guard.transition_locked(&prepared(), dispatching())
            })
            .unwrap();

        let current = store.read_unlocked(&attempt_id()).unwrap().unwrap();
        assert_eq!(current.state, dispatching());
        assert_eq!(current.generation, 1);
    }

    #[test]
    fn a_journal_cannot_be_opened_twice() {
        // The second worker must observe the existing history rather than
        // starting a fresh one over the top of it.
        let directory = tempfile::tempdir().unwrap();
        let store = PublicationJournalStore::new(directory.path());
        let record = opened(&store);

        let error = store
            .with_locked_attempt(&attempt_id(), |guard| guard.open(&record))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    }

    #[test]
    fn a_stale_expected_state_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let store = PublicationJournalStore::new(directory.path());
        opened(&store);
        store
            .with_locked_attempt(&attempt_id(), |guard| {
                guard.transition_locked(&prepared(), dispatching())
            })
            .unwrap();

        // A worker that read `AttemptPrepared` before recovery moved the
        // journal must not be able to act on what it read.
        let error = store
            .with_locked_attempt(&attempt_id(), |guard| {
                guard.transition_locked(&prepared(), dispatching())
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    }

    #[test]
    fn a_shortcut_that_skips_the_durable_candidate_is_refused() {
        // Dispatching → OutcomeRecorded would commit a primary outcome with no
        // preceding OutcomePrepared, which is exactly the outbox invariant the
        // candidate state exists to hold.
        let directory = tempfile::tempdir().unwrap();
        let store = PublicationJournalStore::new(directory.path());
        opened(&store);
        store
            .with_locked_attempt(&attempt_id(), |guard| {
                guard.transition_locked(&prepared(), dispatching())
            })
            .unwrap();

        let error = store
            .with_locked_attempt(&attempt_id(), |guard| {
                guard.transition_locked(
                    &dispatching(),
                    AttemptJournalState::OutcomeRecorded {
                        attempt: attempt_ref(),
                        attempt_number: 1,
                        outcome: PublicationOutcomeDigest::new(Digest::of_bytes(b"outcome")),
                        receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
                        control_clear: clear(),
                    },
                )
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);
    }

    #[test]
    fn abandoned_is_terminal_and_has_no_finalized_transition() {
        // There is nothing to finalize: no number consumed, no authorization
        // spent, no dispatched attempt, no fact awaiting a drain.
        let directory = tempfile::tempdir().unwrap();
        let store = PublicationJournalStore::new(directory.path());
        opened(&store);

        let abandoned = AttemptJournalState::Abandoned {
            evidence: NonCommitEvidence {
                candidate_attempt_number: 1,
                observed_control: control(0, None),
                classified_at: Timestamp::from_unix_nanos(0),
            },
        };
        store
            .with_locked_attempt(&attempt_id(), |guard| {
                guard.transition_locked(&prepared(), abandoned.clone())
            })
            .unwrap();

        let error = store
            .with_locked_attempt(&attempt_id(), |guard| {
                guard.transition_locked(
                    &abandoned,
                    AttemptJournalState::Finalized {
                        terminal_disposition: TerminalDisposition::AbandonedBeforeDispatch {
                            attempt_number: 1,
                            reason: "refused".into(),
                            control_clear: clear(),
                            abandonment_event: ActivityEventId::parse("evt_000000000002").unwrap(),
                        },
                    },
                )
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);
    }

    #[test]
    fn the_pre_dispatch_abandonment_path_ends_at_finalized() {
        let directory = tempfile::tempdir().unwrap();
        let store = PublicationJournalStore::new(directory.path());
        opened(&store);

        let event = ActivityEventId::parse("evt_000000000002").unwrap();
        let abandon_prepared = AttemptJournalState::AbandonPrepared {
            attempt_number: 1,
            reason: "the binding was retargeted".into(),
            control_clear: clear(),
            abandonment_event: event.clone(),
        };
        let abandoned_before_dispatch = AttemptJournalState::AbandonedBeforeDispatch {
            attempt_number: 1,
            reason: "the binding was retargeted".into(),
            control_clear: clear(),
            abandonment_event: event.clone(),
        };
        let finalized = AttemptJournalState::Finalized {
            terminal_disposition: TerminalDisposition::AbandonedBeforeDispatch {
                attempt_number: 1,
                reason: "the binding was retargeted".into(),
                control_clear: clear(),
                abandonment_event: event,
            },
        };

        store
            .with_locked_attempt(&attempt_id(), |guard| {
                guard.transition_locked(&prepared(), abandon_prepared.clone())?;
                guard.transition_locked(&abandon_prepared, abandoned_before_dispatch.clone())?;
                guard.transition_locked(&abandoned_before_dispatch, finalized.clone())
            })
            .unwrap();

        let current = store.read_unlocked(&attempt_id()).unwrap().unwrap();
        match current.state {
            AttemptJournalState::Finalized {
                terminal_disposition,
            } => assert!(
                !terminal_disposition.expects_primary_outcome(),
                "a pre-dispatch abandonment must not expect an outcome"
            ),
            other => panic!("expected Finalized, got {}", other.name()),
        }
    }

    #[test]
    fn finalized_retains_which_terminal_path_ran() {
        // Two Finalized attempts, opposite expectations about whether a
        // primary outcome exists. An opaque marker would leave recovery
        // guessing, and it would guess from state that has moved on.
        let outcome_path = TerminalDisposition::OutcomeFinalized {
            attempt: attempt_ref(),
            outcome: PublicationOutcomeDigest::new(Digest::of_bytes(b"outcome")),
            receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
            control_clear: clear(),
        };
        let abandonment_path = TerminalDisposition::AbandonedBeforeDispatch {
            attempt_number: 1,
            reason: "refused".into(),
            control_clear: clear(),
            abandonment_event: ActivityEventId::parse("evt_000000000002").unwrap(),
        };
        assert!(outcome_path.expects_primary_outcome());
        assert!(!abandonment_path.expects_primary_outcome());
    }

    #[test]
    fn every_state_pair_outside_the_frozen_set_is_refused() {
        // A closed transition set, not a list of forbidden moves: the pair
        // nobody thought about must fail rather than pass by omission.
        let abandoned = AttemptJournalState::Abandoned {
            evidence: NonCommitEvidence {
                candidate_attempt_number: 1,
                observed_control: control(0, None),
                classified_at: Timestamp::from_unix_nanos(0),
            },
        };
        let outcome_recorded = AttemptJournalState::OutcomeRecorded {
            attempt: attempt_ref(),
            attempt_number: 1,
            outcome: PublicationOutcomeDigest::new(Digest::of_bytes(b"outcome")),
            receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
            control_clear: clear(),
        };

        assert!(prepared().permits(&dispatching()));
        assert!(
            dispatching().permits(&AttemptJournalState::OutcomePrepared {
                attempt: attempt_ref(),
                attempt_number: 1,
                candidate: PublicationOutcomeDigest::new(Digest::of_bytes(b"outcome")),
                receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
                outcome_event: ActivityEventId::parse("evt_000000000003").unwrap(),
            })
        );

        // Backwards, sideways and out of a terminal state: all refused.
        assert!(!dispatching().permits(&prepared()));
        assert!(!outcome_recorded.permits(&dispatching()));
        assert!(!abandoned.permits(&dispatching()));
        assert!(!prepared().permits(&outcome_recorded));
        assert!(abandoned.is_terminal());
    }
}
