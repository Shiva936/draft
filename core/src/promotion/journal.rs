//! The promotion restart table (§2.34), as one total function.

use serde::{Deserialize, Serialize};

/// How far the promotion transaction got before the crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionJournalState {
    /// The intent is durable; nothing has moved yet.
    Prepared,
    /// The control state was advanced. Finalization may be incomplete.
    Committed,
    /// Everything finished. The journal is history.
    Finalized,
}

/// How the current `ProjectControlState` compares to the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlMatch {
    /// Still the state the journal expected: the commit never landed.
    Expected,
    /// The state the journal planned: the commit landed.
    Planned,
    /// Neither. Something else moved it.
    Neither,
}

impl ControlMatch {
    /// How `current` compares to what a journal expected and planned.
    ///
    /// One implementation, because recovery and any read-only report of where
    /// recovery stands must classify identically — a reader shown `Planned`
    /// where recovery would see `Neither` would be told an interrupted
    /// promotion is resumable when it is not.
    pub fn classify(
        current: &draft_dcg_contract::Digest,
        expected: &draft_dcg_contract::Digest,
        planned: &draft_dcg_contract::Digest,
    ) -> Self {
        if current == expected {
            Self::Expected
        } else if current == planned {
            Self::Planned
        } else {
            Self::Neither
        }
    }
}

/// How the current `Change` compares to the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeMatch {
    /// Still `Active`, as the journal expected.
    ExpectedActive,
    /// Already `Completed`, as the journal planned.
    PlannedCompleted,
    /// Neither.
    Neither,
}

/// What recovery concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionResolution {
    /// The promotion never committed. Abandon it; its orphans become
    /// collectable once the journal is cleared.
    ///
    /// Safe precisely because the control state never moved: nothing accepted
    /// anything, so discarding the attempt discards no acceptance.
    DidNotCommit,
    /// The promotion committed but the Change is still `Active`.
    ///
    /// Completing it is mandatory, not optional. The project has already
    /// accepted this work into its Baseline; leaving the Change open would
    /// let it be revised and re-promoted, accepting the same work twice.
    CompleteChangeThenFinalize,
    /// The promotion committed and the Change is already `Completed`.
    /// Remaining finalization is idempotent.
    ContinueFinalization,
    /// A historical fact. Current mutable state says nothing about it.
    AlreadyFinalized,
    /// The two records disagree in a way no legal sequence produces.
    ///
    /// Never resolved automatically. Guessing here would either discard an
    /// acceptance or manufacture one, and both are worse than stopping.
    Inconsistent { detail: &'static str },
}

/// The restart table.
///
/// # Why `committed` does not accept "any / any"
///
/// A committed-but-unfinalized promotion is still inside the local recovery
/// barrier, so no later conflicting mutation should have passed it. Accepting
/// any current state would mean treating "something else has since moved the
/// control state" as normal — which is exactly the situation where finishing
/// finalization would write a receipt describing a Baseline the project no
/// longer has.
///
/// Only at `finalized` does Draft stop requiring anything of current mutable
/// state: by then the immutable record, receipt and audit history are what the
/// promotion means, and later legitimate work is free to move on.
pub fn resolve(
    journal: PromotionJournalState,
    control: ControlMatch,
    change: ChangeMatch,
) -> PromotionResolution {
    let resolution = classify(journal, control, change);
    // Counted at the one place that classifies. Each is its own operational
    // fact: a finalization completing after a crash, a Change completion that
    // had to be finished separately, and a pair of records that no legal
    // sequence produces — which is the only one that needs a person.
    match &resolution {
        PromotionResolution::ContinueFinalization => {
            crate::support::telemetry::Counter::PromotionRecoveryFinalizations.increment();
        }
        PromotionResolution::CompleteChangeThenFinalize => {
            crate::support::telemetry::Counter::PromotionChangeCompletionRecoveries.increment();
        }
        PromotionResolution::Inconsistent { .. } => {
            crate::support::telemetry::Counter::PromotionInconsistentStates.increment();
        }
        PromotionResolution::DidNotCommit | PromotionResolution::AlreadyFinalized => {}
    }
    resolution
}

/// The restart table itself, with nothing counted.
///
/// [`resolve`] is the recovery boundary and increments the operational
/// counters; this is the same decision for a reader that is only *reporting*
/// the position. A read-only view calling `resolve` would report a recovery
/// that never ran, and `promotion_recovery_finalizations` would then count
/// page loads.
pub fn classify(
    journal: PromotionJournalState,
    control: ControlMatch,
    change: ChangeMatch,
) -> PromotionResolution {
    use ChangeMatch as C;
    use ControlMatch as K;
    use PromotionJournalState as J;
    use PromotionResolution as R;

    match (journal, control, change) {
        // Finalized is history. It tolerates any current state, because later
        // legitimate work is allowed to have moved on.
        (J::Finalized, _, _) => R::AlreadyFinalized,

        // Prepared and the control state never moved: the commit did not land.
        // The Change is not consulted — nothing could have completed it.
        (J::Prepared, K::Expected, _) => R::DidNotCommit,

        // Committed, in either journal state, is decided by the Change.
        (J::Prepared | J::Committed, K::Planned, C::ExpectedActive) => {
            R::CompleteChangeThenFinalize
        }
        (J::Prepared | J::Committed, K::Planned, C::PlannedCompleted) => R::ContinueFinalization,
        (J::Prepared | J::Committed, K::Planned, C::Neither) => R::Inconsistent {
            detail: "the control state committed but the Change is neither the Active state the \
                     promotion expected nor the Completed state it planned",
        },

        // Prepared with a control state that is neither: something moved it
        // that this promotion did not.
        (J::Prepared, K::Neither, _) => R::Inconsistent {
            detail: "a prepared promotion found the control state neither as expected nor as \
                     planned; another writer moved it",
        },

        // Committed with a control state that is not the planned one. Inside
        // the recovery barrier this cannot legally happen.
        (J::Committed, K::Expected | K::Neither, _) => R::Inconsistent {
            detail: "a committed promotion found the control state moved away from its planned \
                     value while still inside the recovery barrier",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ChangeMatch as C;
    use ControlMatch as K;
    use PromotionJournalState as J;
    use PromotionResolution as R;

    #[test]
    fn a_prepared_promotion_whose_control_never_moved_did_not_commit() {
        // The Change is irrelevant here: with the control state untouched
        // nothing was accepted, so there is nothing a Change state could mean.
        for change in [C::ExpectedActive, C::PlannedCompleted, C::Neither] {
            assert_eq!(resolve(J::Prepared, K::Expected, change), R::DidNotCommit);
        }
    }

    #[test]
    fn a_committed_promotion_with_an_open_change_must_finish_completing_it() {
        // Leaving the Change Active after its work was accepted would let it be
        // revised and promoted again — accepting the same work twice.
        for journal in [J::Prepared, J::Committed] {
            assert_eq!(
                resolve(journal, K::Planned, C::ExpectedActive),
                R::CompleteChangeThenFinalize
            );
        }
    }

    #[test]
    fn a_fully_advanced_promotion_only_needs_idempotent_finalization() {
        for journal in [J::Prepared, J::Committed] {
            assert_eq!(
                resolve(journal, K::Planned, C::PlannedCompleted),
                R::ContinueFinalization
            );
        }
    }

    #[test]
    fn finalized_tolerates_any_current_state() {
        // By now the promotion means its immutable record, receipt and audit
        // history. Requiring anything of current mutable state would make
        // later legitimate work look like corruption.
        for control in [K::Expected, K::Planned, K::Neither] {
            for change in [C::ExpectedActive, C::PlannedCompleted, C::Neither] {
                assert_eq!(resolve(J::Finalized, control, change), R::AlreadyFinalized);
            }
        }
    }

    #[test]
    fn committed_never_accepts_an_arbitrary_control_state() {
        // The row §2.34 deliberately removes. Treating this as "just finish
        // finalization" would write a receipt describing a Baseline the
        // project no longer has.
        for control in [K::Expected, K::Neither] {
            for change in [C::ExpectedActive, C::PlannedCompleted, C::Neither] {
                assert!(
                    matches!(
                        resolve(J::Committed, control, change),
                        R::Inconsistent { .. }
                    ),
                    "committed/{control:?}/{change:?} must not resolve automatically"
                );
            }
        }
    }

    #[test]
    fn a_committed_control_with_an_unrecognised_change_is_inconsistent() {
        for journal in [J::Prepared, J::Committed] {
            assert!(matches!(
                resolve(journal, K::Planned, C::Neither),
                R::Inconsistent { .. }
            ));
        }
    }

    #[test]
    fn every_combination_has_a_decided_answer() {
        // The table is total by construction. This asserts it stays that way:
        // a future variant added without a row would fail to compile, and a
        // row that silently fell through to a default would show up here as an
        // unexpected resolution rather than as a plausible-looking guess.
        let mut decided = 0;
        for journal in [J::Prepared, J::Committed, J::Finalized] {
            for control in [K::Expected, K::Planned, K::Neither] {
                for change in [C::ExpectedActive, C::PlannedCompleted, C::Neither] {
                    let resolution = resolve(journal, control, change);
                    // Never a silent success where the table says otherwise.
                    if journal == J::Committed && control != K::Planned {
                        assert!(matches!(resolution, R::Inconsistent { .. }));
                    }
                    decided += 1;
                }
            }
        }
        assert_eq!(decided, 27, "every journal/control/change combination");
    }
}
