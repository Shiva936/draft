//! The local-first recovery barrier.
//!
//! # What it guarantees
//!
//! > No new ChangePack mutation begins while a promotion's ChangePack lifecycle
//! > finalization is still outstanding.
//!
//! A promotion that committed has already moved the accepted Baseline. If the
//! crash landed between that commit and the ChangePack's `Active → Completed`
//! transition, the ChangePack is still open — and a new mutation on it would let
//! the same work be revised and promoted a second time, accepting it twice
//! into a Baseline that already contains it (Scenario DI).
//!
//! # Why it is local-first
//!
//! Startup recovery must not depend on anything remote. If it did, an
//! unreachable external service would block every local mutation in the
//! project — turning somebody else's outage into "Draft will not let me edit
//! my own work" (Scenario AT).
//!
//! That is enforced structurally rather than promised: [`enforce`] takes only
//! local stores. It has no remote handle, no network client and no way to
//! reach one, so there is nothing for it to block on. A future change that
//! wanted to consult something remote here could not do so without altering
//! the signature, which is exactly the review moment it deserves.

use crate::promotion::journal::{
    resolve, ChangePackMatch, ControlMatch, PromotionJournalState, PromotionResolution,
};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// What the barrier requires before a new ChangePack mutation may proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BarrierOutcome {
    /// Nothing outstanding. The mutation may proceed.
    Clear,
    /// A committed promotion still owes its ChangePack completion.
    ///
    /// The caller must finish that first. Deliberately not done silently here:
    /// completing a ChangePack is an audited mutation with its own journal, and
    /// burying it inside a barrier check would hide a durable state change in
    /// what reads like a precondition.
    CompletionOutstanding,
    /// The records disagree in a way no legal sequence produces.
    Inconsistent { detail: &'static str },
}

/// Evaluate the barrier for one pending promotion.
///
/// Takes the already-read local state rather than reading it: the caller holds
/// the locks that make these values coherent with each other, and re-reading
/// inside would produce a pair that was never simultaneously true.
pub fn enforce(
    journal: PromotionJournalState,
    control: ControlMatch,
    change: ChangePackMatch,
) -> BarrierOutcome {
    match resolve(journal, control, change) {
        // Nothing was accepted, or everything already finished.
        PromotionResolution::DidNotCommit | PromotionResolution::AlreadyFinalized => {
            BarrierOutcome::Clear
        }
        // The Baseline moved but the ChangePack did not. This is the case the
        // barrier exists for.
        PromotionResolution::CompleteChangePackThenFinalize => {
            BarrierOutcome::CompletionOutstanding
        }
        // Committed and completed; only idempotent finalization remains, which
        // no longer constrains new work on the ChangePack.
        PromotionResolution::ContinueFinalization => BarrierOutcome::Clear,
        PromotionResolution::Inconsistent { detail } => BarrierOutcome::Inconsistent { detail },
    }
}

impl BarrierOutcome {
    /// Refuse the mutation unless the barrier is clear.
    pub fn require_clear(&self) -> DraftResult<()> {
        match self {
            Self::Clear => Ok(()),
            Self::CompletionOutstanding => Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "a promotion accepted this work into the Baseline but its ChangePack completion did \
                 not finish; that must be completed before the ChangePack is mutated again",
            )
            .with_suggestion(
                "run `draft doctor` to finish the outstanding completion, then retry",
            )),
            Self::Inconsistent { detail } => Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("promotion state is inconsistent: {detail}"),
            )
            .with_suggestion("run `draft doctor` — this needs an explicit recovery decision")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ChangePackMatch as C;
    use ControlMatch as K;
    use PromotionJournalState as J;

    #[test]
    fn an_uncommitted_promotion_does_not_block_new_work() {
        // Nothing was accepted, so there is nothing to finish. Blocking here
        // would strand a project behind an attempt that never happened.
        assert_eq!(
            enforce(J::Prepared, K::Expected, C::ExpectedActive),
            BarrierOutcome::Clear
        );
        enforce(J::Prepared, K::Expected, C::ExpectedActive)
            .require_clear()
            .unwrap();
    }

    #[test]
    fn a_committed_promotion_with_an_open_change_blocks_until_completed() {
        // Scenario DI. Allowing a mutation here would let the same work be
        // revised and promoted again, accepting it twice.
        let outcome = enforce(J::Committed, K::Planned, C::ExpectedActive);
        assert_eq!(outcome, BarrierOutcome::CompletionOutstanding);
        let error = outcome.require_clear().unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    }

    #[test]
    fn a_finished_promotion_stops_constraining_new_work() {
        for control in [K::Expected, K::Planned, K::Neither] {
            assert_eq!(
                enforce(J::Finalized, control, C::Neither),
                BarrierOutcome::Clear
            );
        }
        // And a committed-and-completed promotion only owes idempotent
        // finalization, which does not constrain the ChangePack.
        assert_eq!(
            enforce(J::Committed, K::Planned, C::PlannedCompleted),
            BarrierOutcome::Clear
        );
    }

    #[test]
    fn an_inconsistent_pair_is_refused_rather_than_guessed() {
        let outcome = enforce(J::Committed, K::Neither, C::ExpectedActive);
        assert!(matches!(outcome, BarrierOutcome::Inconsistent { .. }));
        assert_eq!(
            outcome.require_clear().unwrap_err().kind,
            DraftErrorKind::CorruptData
        );
    }
}
