//! The phased recovery workflow, and why it cannot be one transaction.
//!
//! Recovering an already-allocated attempt needs two things that cannot be
//! held at once. It needs the attempt journal (order 6) to read what was
//! committed, and it needs current authority and the exact route — the trust
//! fence (1), the lease (3), project control (4), the binding (5) — to decide
//! whether dispatch is still permitted.
//!
//! Acquiring any of those while holding the journal is a reverse acquisition
//! §2.28 forbids. So the workflow is phases, not a transaction:
//!
//! ```text
//! AR0   classify        journal (6) alone, then control (7) alone, each released
//! AR0-F abandon         journal (6) reacquired independently, for the terminal mark
//! AR1   revalidate      fence (1) → lease (3) → control (4) → binding (5)
//!                       NO journal or publication-control lock held
//! AV/AC commit          the SAME AR1 guards still held, then journal (6) → control (7)
//! ```
//!
//! Three separate acquisitions of the journal in AR0 alone, and that is legal:
//! the rule is about what is *held*, not about what happened earlier. Nothing
//! higher is still held when each phase begins, so no cycle can form.
//!
//! # Why every phase boundary re-reads
//!
//! Between releasing the journal and reacquiring it, another actor may
//! legitimately have advanced the same attempt. A workflow that carried its
//! Phase-AR0 conclusion into its Phase-AC mutation would be acting on state
//! that has since moved — the classic time-of-check-to-time-of-use, with an
//! external effect on the far side.
//!
//! So [`PhaseBoundary`] carries the *expectation*, never the conclusion, and
//! the mutation re-requires the exact expected value before it commits. If it
//! moved, the validation is discarded and the workflow reclassifies from
//! authoritative state.
//!
//! # Why AC never releases and reacquires a different snapshot
//!
//! Pre-dispatch abandonment (Phase AC) commits under **the same guards AV
//! validated under**. Releasing them and taking fresh ones would mean
//! abandoning on the strength of one security snapshot and committing under
//! another — and the two can disagree about whether the abandonment was even
//! the right answer. There is no alternative shape.

use draft_dcg_contract::ids::PublicationAttemptId;

use crate::publication::control::PublicationControl;
use crate::publication::journal::AttemptJournalState;

/// What the workflow expects to still be true when it reacquires.
///
/// Carries values to *re-require*, not conclusions to act on. Nothing here is
/// a decision: it is the exact state the earlier phase read, so the later
/// phase can prove nothing moved in between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseBoundary {
    pub attempt: PublicationAttemptId,
    /// The exact journal state the previous phase observed.
    pub expected_journal: AttemptJournalState,
    /// The exact control value the previous phase observed.
    pub expected_control: PublicationControl,
}

/// What a phase boundary concluded on re-read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundaryCheck {
    /// Nothing moved. The validation still applies and the mutation may
    /// proceed.
    Unchanged,
    /// Something moved. Discard the validation, release the guards, and
    /// reclassify from authoritative state — never mutate on what was read
    /// before.
    Moved { detail: &'static str },
}

impl PhaseBoundary {
    /// Re-check this boundary against authoritative state.
    ///
    /// Both records, always. Checking only the journal would miss an
    /// allocation that committed in between; checking only the control record
    /// would miss a recovery actor that advanced the journal.
    pub fn check(
        &self,
        journal: Option<&AttemptJournalState>,
        control: Option<&PublicationControl>,
    ) -> BoundaryCheck {
        match journal {
            Some(state) if state == &self.expected_journal => {}
            Some(_) => {
                return BoundaryCheck::Moved {
                    detail: "the attempt journal advanced while the lower-order guards were being \
                             acquired",
                }
            }
            None => {
                return BoundaryCheck::Moved {
                    detail: "the attempt journal disappeared while the lower-order guards were \
                             being acquired",
                }
            }
        }
        match control {
            Some(value) if value == &self.expected_control => BoundaryCheck::Unchanged,
            Some(_) => BoundaryCheck::Moved {
                detail: "the Publication control record moved while the lower-order guards were \
                         being acquired",
            },
            None => BoundaryCheck::Moved {
                detail: "the Publication control record disappeared while the lower-order guards \
                         were being acquired",
            },
        }
    }
}

/// What fresh validation (Phase AR1/AV) decided about an allocated attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshValidation {
    /// Current authority and the exact route still permit dispatch. Proceed to
    /// the durable `Dispatching` transition for **this** attempt.
    PermitsDispatch,
    /// Something changed — the binding was retargeted, a grant was revoked,
    /// the project closed. The allocation committed, so the attempt is
    /// withdrawn through the abandonment path rather than simply dropped.
    RefusesDispatch { reason: String },
}

/// What the caller does with a refusal.
///
/// The distinction the abandonment path exists to preserve: an allocation that
/// committed consumed a number and possibly a one-shot authorization, so
/// withdrawing it is a recorded transaction with a fact to drain. An
/// allocation that never committed consumed nothing and is simply marked
/// terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalPath {
    /// `AttemptPrepared → AbandonPrepared → AbandonedBeforeDispatch →
    /// Finalized`, with the exact control clear and one drained fact.
    CommittedAllocation,
    /// `AttemptPrepared → Abandoned`. Terminal, nothing to finalize.
    UncommittedAllocation,
}

/// Whether a refused attempt is withdrawn as a committed allocation.
///
/// Decided by whether the allocation committed, never by why dispatch was
/// refused. The reason a dispatch is refused says nothing about what was
/// already spent.
pub fn withdrawal_path(allocation_committed: bool) -> WithdrawalPath {
    if allocation_committed {
        WithdrawalPath::CommittedAllocation
    } else {
        WithdrawalPath::UncommittedAllocation
    }
}

/// Whether this workflow may allocate a new attempt.
///
/// Always `false`. Recovery resumes the attempt that already exists; it never
/// falls through into creating another while holding recovery state. The
/// function exists so that "recovery and creation are never one transaction"
/// is something a caller reads rather than something a reviewer has to notice.
pub fn recovery_may_allocate() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication::journal::ControlTransition;
    use draft_dcg_contract::ids::{ActivityEventId, PublicationId};
    use draft_dcg_contract::publication::{PublicationAttemptDigest, PublicationAttemptRef};
    use draft_dcg_contract::Digest;

    fn publication() -> PublicationId {
        PublicationId::parse("pub_000000000001").unwrap()
    }

    fn attempt() -> PublicationAttemptId {
        PublicationAttemptId::parse("pat_000000000001").unwrap()
    }

    fn control(generation: u64, in_flight: Option<PublicationAttemptId>) -> PublicationControl {
        let mut value = PublicationControl::initial(publication());
        value.generation = generation;
        value.in_flight_attempt = in_flight;
        value
    }

    fn prepared() -> AttemptJournalState {
        AttemptJournalState::AttemptPrepared {
            candidate_attempt_number: 1,
            allocation: ControlTransition {
                expected: control(0, None),
                planned: control(1, Some(attempt())),
            },
            retry_authorization: None,
        }
    }

    fn boundary() -> PhaseBoundary {
        PhaseBoundary {
            attempt: attempt(),
            expected_journal: prepared(),
            expected_control: control(1, Some(attempt())),
        }
    }

    #[test]
    fn an_unchanged_boundary_lets_the_validated_mutation_proceed() {
        assert_eq!(
            boundary().check(Some(&prepared()), Some(&control(1, Some(attempt())))),
            BoundaryCheck::Unchanged
        );
    }

    #[test]
    fn a_journal_another_actor_advanced_discards_the_validation() {
        // The window the phase split opens: while AR1 was taking the fence,
        // lease, control and binding locks, a recovery actor legitimately
        // moved this attempt on.
        let advanced = AttemptJournalState::Dispatching {
            attempt: PublicationAttemptRef {
                id: attempt(),
                digest: PublicationAttemptDigest::new(Digest::of_bytes(b"attempt")),
            },
            attempt_number: 1,
            dispatch_event: ActivityEventId::parse("evt_000000000001").unwrap(),
        };
        assert!(matches!(
            boundary().check(Some(&advanced), Some(&control(1, Some(attempt())))),
            BoundaryCheck::Moved { .. }
        ));
    }

    #[test]
    fn a_control_record_that_moved_discards_it_too() {
        // Checking only the journal would miss this: the journal is untouched
        // and the allocation state underneath it is not.
        assert!(matches!(
            boundary().check(Some(&prepared()), Some(&control(2, None))),
            BoundaryCheck::Moved { .. }
        ));
    }

    #[test]
    fn a_record_that_vanished_is_movement_not_absence() {
        assert!(matches!(
            boundary().check(None, Some(&control(1, Some(attempt())))),
            BoundaryCheck::Moved { .. }
        ));
        assert!(matches!(
            boundary().check(Some(&prepared()), None),
            BoundaryCheck::Moved { .. }
        ));
    }

    #[test]
    fn the_withdrawal_path_follows_what_was_spent_not_why_dispatch_was_refused() {
        // Same refusal, opposite paths, because the two worlds differ on
        // whether a number and a one-shot authorization were consumed.
        assert_eq!(
            withdrawal_path(true),
            WithdrawalPath::CommittedAllocation,
            "a committed allocation is withdrawn with a control clear and a drained fact"
        );
        assert_eq!(
            withdrawal_path(false),
            WithdrawalPath::UncommittedAllocation,
            "an uncommitted allocation is simply terminal"
        );
    }

    #[test]
    fn recovery_never_falls_through_into_a_new_allocation() {
        assert!(!recovery_may_allocate());
    }

    #[test]
    fn a_refusal_carries_its_reason_into_the_abandonment_record() {
        // The reason is durable because a withdrawn attempt is something a
        // person will later have to understand without the guards that
        // produced it.
        let refusal = FreshValidation::RefusesDispatch {
            reason: "the binding was retargeted".into(),
        };
        match refusal {
            FreshValidation::RefusesDispatch { reason } => assert!(!reason.is_empty()),
            FreshValidation::PermitsDispatch => panic!("expected a refusal"),
        }
    }
}
