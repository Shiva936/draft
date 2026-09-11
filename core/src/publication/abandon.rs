//! Withdrawing an attempt whose dispatch is refused before it happens.
//!
//! An allocated attempt that never reached the dispatch boundary is the one
//! interruption Draft can resolve without asking anybody: nothing external
//! happened, and nothing external will. But it cannot simply be dropped. The
//! allocation may have consumed an attempt number and a one-shot retry
//! authorization, and a Publication whose control record still names an
//! in-flight attempt refuses every later one — so the withdrawal is a recorded
//! transaction, not a deletion.
//!
//! ```text
//! AR0   classify     journal (6) alone, then control (7) alone, each released
//! AR1   revalidate   fence (1) → lease (3) → control (4) → binding (5)
//! AC    commit       the SAME AR1 guards, then journal (6) → control (7)
//! ```
//!
//! # Why this exists at all
//!
//! Without it, an attempt interrupted between its allocation and its dispatch
//! boundary blocks its Publication permanently. The barrier correctly refuses
//! to start a second attempt — it cannot prove the first caused no effect — and
//! nothing was able to prove the opposite either. This is what proves it: the
//! journal never reached `Dispatching`, so no external call was ever made.
//!
//! # Why the two withdrawal paths are different states
//!
//! [`WithdrawalPath`] distinguishes them, and the distinction is what the
//! journal's `Abandoned` and `AbandonedBeforeDispatch` preserve. An allocation
//! that never committed consumed nothing and is simply marked terminal. One
//! that did consumed a number — a legal gap in the sequence — and possibly a
//! one-shot authorization, so it carries the exact control clear and an
//! abandonment fact to drain.
//!
//! Collapsing them would either invent a consumed number that nothing spent,
//! or silently reuse one that something did.
//!
//! # Why AC commits under AR1's guards
//!
//! Releasing them and taking fresh ones would mean deciding to abandon on the
//! strength of one security snapshot and committing under another, and the two
//! can disagree about whether abandoning was even the right answer. So the
//! boundary carries the *expectation* rather than the conclusion, and the
//! mutation re-requires the exact values before it commits — if anything
//! moved, the validation is discarded and the caller reclassifies.

use draft_dcg_contract::ids::{ActivityEventId, PublicationAttemptId};
use draft_dcg_contract::value::Timestamp;

use crate::publication::control::ControlMatch;
use crate::publication::dispatch::DispatchStores;
use crate::publication::journal::{
    AttemptJournalState, ControlTransition, NonCommitEvidence, TerminalDisposition,
};
use crate::publication::recovery::{
    withdrawal_path, BoundaryCheck, FreshValidation, PhaseBoundary, WithdrawalPath,
};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// What withdrawing an attempt concluded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Withdrawn {
    /// The allocation never committed. Nothing was spent; the attempt is
    /// terminal and the Publication was never held by it.
    NothingWasSpent { attempt: PublicationAttemptId },
    /// The allocation committed. Its number is a legal gap, the control record
    /// is cleared, and the abandonment fact is durable.
    AllocationWithdrawn {
        attempt: PublicationAttemptId,
        attempt_number: u32,
        reason: String,
    },
    /// Something moved between classifying and committing. The caller
    /// reclassifies from authoritative state rather than acting on what it
    /// read before.
    Moved { detail: &'static str },
    /// The attempt is not in a state that may be withdrawn.
    NotWithdrawable { detail: String },
}

/// Classify an allocated attempt, returning the boundary to re-require later.
///
/// Phase AR0. Reads the journal and the control record, each under its own
/// lock and each released before the next — so nothing higher is held when the
/// next phase begins and no cycle can form.
pub fn classify(
    stores: &DispatchStores,
    attempt: &PublicationAttemptId,
) -> DraftResult<Option<PhaseBoundary>> {
    let Some(record) = stores.journals.read_unlocked(attempt)? else {
        return Ok(None);
    };
    // Only an attempt that never reached the dispatch boundary may be
    // withdrawn. One at `Dispatching` or beyond may have caused an effect, and
    // abandoning it would be asserting that it did not.
    if !matches!(record.state, AttemptJournalState::AttemptPrepared { .. }) {
        return Ok(None);
    }
    let Some(control) = stores.control.read_unlocked(&record.publication)? else {
        return Ok(None);
    };
    Ok(Some(PhaseBoundary {
        attempt: attempt.clone(),
        expected_journal: record.state,
        expected_control: control,
    }))
}

/// Commit the withdrawal — Phase AC.
///
/// Called with AR1's guards still held. Re-requires the exact values the
/// boundary carries before mutating anything.
pub fn withdraw(
    stores: &DispatchStores,
    boundary: &PhaseBoundary,
    validation: &FreshValidation,
    abandonment_event: ActivityEventId,
    now: Timestamp,
) -> DraftResult<Withdrawn> {
    let FreshValidation::RefusesDispatch { reason } = validation else {
        return Ok(Withdrawn::NotWithdrawable {
            detail: "fresh validation permits dispatch, so there is nothing to withdraw".into(),
        });
    };

    let AttemptJournalState::AttemptPrepared {
        candidate_attempt_number,
        allocation,
        ..
    } = &boundary.expected_journal
    else {
        return Ok(Withdrawn::NotWithdrawable {
            detail: format!(
                "an attempt at {} never reached an allocation to withdraw",
                boundary.expected_journal.name()
            ),
        });
    };

    // Re-read both records and require them unmoved. Checking only one would
    // miss an allocation that committed in between, or a recovery actor that
    // advanced the journal.
    let current_journal = stores
        .journals
        .read_unlocked(&boundary.attempt)?
        .map(|record| record.state);
    let publication = publication_of(stores, &boundary.attempt)?;
    let current_control = stores.control.read_unlocked(&publication)?;
    if let BoundaryCheck::Moved { detail } =
        boundary.check(current_journal.as_ref(), current_control.as_ref())
    {
        return Ok(Withdrawn::Moved { detail });
    }

    // Whether the allocation committed, decided by comparing the control
    // record against the exact pair the journal froze — never by whether the
    // dispatch was refused, which says nothing about what was already spent.
    let committed = matches!(
        ControlMatch::classify(
            current_control.as_ref(),
            &allocation.expected,
            &allocation.planned
        ),
        ControlMatch::Planned
    );

    match withdrawal_path(committed) {
        WithdrawalPath::UncommittedAllocation => {
            let evidence = NonCommitEvidence {
                candidate_attempt_number: *candidate_attempt_number,
                observed_control: allocation.expected.clone(),
                classified_at: now,
            };
            stores
                .journals
                .with_locked_attempt(&boundary.attempt, |journal| {
                    journal.transition_locked(
                        &boundary.expected_journal,
                        AttemptJournalState::Abandoned { evidence },
                    )
                })?;
            Ok(Withdrawn::NothingWasSpent {
                attempt: boundary.attempt.clone(),
            })
        }
        WithdrawalPath::CommittedAllocation => {
            // The exact clear, frozen before it runs, so a crash mid-way
            // leaves a state the next pass can finish rather than guess at.
            let clear = ControlTransition {
                expected: allocation.planned.clone(),
                planned: allocation.planned.advanced(|next| {
                    next.in_flight_attempt = None;
                }),
            };

            stores
                .journals
                .with_locked_attempt(&boundary.attempt, |journal| {
                    journal.transition_locked(
                        &boundary.expected_journal,
                        AttemptJournalState::AbandonPrepared {
                            attempt_number: *candidate_attempt_number,
                            reason: reason.clone(),
                            control_clear: clear.clone(),
                            abandonment_event: abandonment_event.clone(),
                        },
                    )
                })?;

            stores
                .control
                .with_locked_control(&publication, |control| {
                    control.commit_exact(&clear.expected, &clear.planned)
                })?;

            stores
                .journals
                .with_locked_attempt(&boundary.attempt, |journal| {
                    let prepared = AttemptJournalState::AbandonPrepared {
                        attempt_number: *candidate_attempt_number,
                        reason: reason.clone(),
                        control_clear: clear.clone(),
                        abandonment_event: abandonment_event.clone(),
                    };
                    journal.transition_locked(
                        &prepared,
                        AttemptJournalState::AbandonedBeforeDispatch {
                            attempt_number: *candidate_attempt_number,
                            reason: reason.clone(),
                            control_clear: clear.clone(),
                            abandonment_event: abandonment_event.clone(),
                        },
                    )?;
                    journal.transition_locked(
                        &AttemptJournalState::AbandonedBeforeDispatch {
                            attempt_number: *candidate_attempt_number,
                            reason: reason.clone(),
                            control_clear: clear.clone(),
                            abandonment_event: abandonment_event.clone(),
                        },
                        AttemptJournalState::Finalized {
                            terminal_disposition: TerminalDisposition::AbandonedBeforeDispatch {
                                attempt_number: *candidate_attempt_number,
                                reason: reason.clone(),
                                control_clear: clear,
                                abandonment_event,
                            },
                        },
                    )
                })?;

            // The committed allocation is now abandoned. This is the only
            // legitimate source of a gap in the attempt sequence: the number
            // stays consumed and no attempt ever claims it.
            crate::support::telemetry::Counter::PublicationAbandonedBeforeDispatch.increment();
            crate::support::telemetry::Counter::PublicationAttemptNumberGaps.increment();
            Ok(Withdrawn::AllocationWithdrawn {
                attempt: boundary.attempt.clone(),
                attempt_number: *candidate_attempt_number,
                reason: reason.clone(),
            })
        }
    }
}

fn publication_of(
    stores: &DispatchStores,
    attempt: &PublicationAttemptId,
) -> DraftResult<draft_dcg_contract::ids::PublicationId> {
    stores
        .journals
        .read_unlocked(attempt)?
        .map(|record| record.publication)
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!("attempt '{attempt}' lost its journal while being withdrawn"),
            )
        })
}
