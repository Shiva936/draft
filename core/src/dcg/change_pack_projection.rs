//! Relating a revision's review progress to the ChangePack's own lifecycle.
//!
//! # The two are on different axes
//!
//! This is the finding that shaped the migration off the Pack ontology.
//! `PackLifecycle` — now [`ReviewProgressState`] — conflates two independent
//! questions:
//!
//! ```text
//! Draft  Verified  Reviewing  Approved  Rejected   how far through review?
//! Submitted                                        is the work finished?
//! ```
//!
//! Five of its six states answer the first, and only one answers the second. So
//! a revision sitting in `Rejected` is not finished work — it is *open work
//! whose review said no*, which is exactly the state from which somebody
//! revises and tries again. Storing that as the work's lifecycle meant "where
//! is this in review" and "is this still being worked on" could not be asked
//! separately.
//!
//! [`ChangePackLifecycle`] answers only the second question. Review progress is
//! evidence, decisions and gate evaluations — facts *about* a revision, not a
//! state the work is in.
//!
//! # Why the mapping is not a bijection
//!
//! No revision state means **abandoned**. A ChangePack somebody decided to stop had
//! nowhere to go: leaving it in `Draft` claims it is still being worked on, and
//! moving it to `Rejected` claims a reviewer turned it down. Both misdescribe a
//! decision that was neither.
//!
//! So [`ChangePackLifecycle::Abandoned`] has no revision state to project back to.
//! That asymmetry is the point rather than a defect: it is the gap the ChangePack
//! lifecycle exists to fill.

use crate::dcg::change_pack::ChangePackLifecycle;
use crate::dcg::revision_pack::ReviewProgressState;

/// How far a revision has got through review.
///
/// What [`ReviewProgressState`] tracks in five of its six states. Derived from
/// evidence and decisions rather than stored as the work's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReviewProgress {
    /// Nothing has been established about this revision yet.
    Drafted,
    /// Evidence has been produced.
    Verified,
    /// A review is under way.
    UnderReview,
    /// A reviewer approved it.
    Approved,
    /// A reviewer declined it. The work remains open.
    Declined,
    /// Its work was promoted.
    Promoted,
}

impl ReviewProgress {
    /// The review progress a revision state stands for.
    pub fn of(state: ReviewProgressState) -> Self {
        match state {
            ReviewProgressState::Draft => Self::Drafted,
            ReviewProgressState::Verified => Self::Verified,
            ReviewProgressState::Reviewing => Self::UnderReview,
            ReviewProgressState::Approved => Self::Approved,
            ReviewProgressState::Rejected => Self::Declined,
            ReviewProgressState::Submitted => Self::Promoted,
        }
    }
}

/// The work lifecycle a revision state implies.
///
/// Only `Submitted` is a statement about the work being finished. Everything
/// else — including `Rejected` — describes open work.
pub fn change_pack_lifecycle_of(state: ReviewProgressState) -> ChangePackLifecycle {
    match state {
        ReviewProgressState::Submitted => ChangePackLifecycle::Completed,
        ReviewProgressState::Draft
        | ReviewProgressState::Verified
        | ReviewProgressState::Reviewing
        | ReviewProgressState::Approved
        | ReviewProgressState::Rejected => ChangePackLifecycle::Active,
    }
}

/// The revision state a ChangePack lifecycle projects back to, where one exists.
///
/// `None` for [`ChangePackLifecycle::Abandoned`], because no revision state can say
/// it. A caller must not substitute `Rejected`: that would record a
/// reviewer's decision that nobody made.
pub fn review_progress_state_of(change: ChangePackLifecycle) -> Option<ReviewProgressState> {
    match change {
        ChangePackLifecycle::Active => Some(ReviewProgressState::Draft),
        ChangePackLifecycle::Completed => Some(ReviewProgressState::Submitted),
        ChangePackLifecycle::Abandoned => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_REVISION_STATES: &[ReviewProgressState] = &[
        ReviewProgressState::Draft,
        ReviewProgressState::Verified,
        ReviewProgressState::Reviewing,
        ReviewProgressState::Approved,
        ReviewProgressState::Rejected,
        ReviewProgressState::Submitted,
    ];

    #[test]
    fn only_submission_says_the_work_is_finished() {
        for state in ALL_REVISION_STATES {
            let expected = if *state == ReviewProgressState::Submitted {
                ChangePackLifecycle::Completed
            } else {
                ChangePackLifecycle::Active
            };
            assert_eq!(change_pack_lifecycle_of(*state), expected, "{state:?}");
        }
    }

    #[test]
    fn a_declined_review_leaves_the_work_open() {
        // The conflation this untangles: Rejected is a review outcome, and the
        // state somebody revises from — not a finished change.
        assert_eq!(
            change_pack_lifecycle_of(ReviewProgressState::Rejected),
            ChangePackLifecycle::Active
        );
        assert!(change_pack_lifecycle_of(ReviewProgressState::Rejected).accepts_work());
        assert_eq!(
            ReviewProgress::of(ReviewProgressState::Rejected),
            ReviewProgress::Declined
        );
    }

    #[test]
    fn review_progress_and_work_lifecycle_vary_independently() {
        // Four different review positions, one work state. Storing them as one
        // value is what made the two unaskable separately.
        let open: Vec<ReviewProgress> = [
            ReviewProgressState::Draft,
            ReviewProgressState::Verified,
            ReviewProgressState::Reviewing,
            ReviewProgressState::Approved,
        ]
        .iter()
        .map(|state| {
            assert_eq!(
                change_pack_lifecycle_of(*state),
                ChangePackLifecycle::Active
            );
            ReviewProgress::of(*state)
        })
        .collect();

        let distinct: std::collections::BTreeSet<_> = open.iter().collect();
        assert_eq!(distinct.len(), 4, "four review positions, one work state");
    }

    #[test]
    fn abandonment_has_no_pack_state_to_project_back_to() {
        // The gap the migration exists to close. A caller must not substitute
        // Rejected: that would record a reviewer's decision nobody made.
        assert_eq!(
            review_progress_state_of(ChangePackLifecycle::Abandoned),
            None
        );
        assert_eq!(
            review_progress_state_of(ChangePackLifecycle::Active),
            Some(ReviewProgressState::Draft)
        );
        assert_eq!(
            review_progress_state_of(ChangePackLifecycle::Completed),
            Some(ReviewProgressState::Submitted)
        );
    }

    #[test]
    fn projecting_a_pack_state_and_back_never_invents_a_review_outcome() {
        // Round-tripping loses review progress, which is expected — but it must
        // never come back as a decision. Anything open projects to Draft, the
        // state that asserts nothing.
        for state in ALL_REVISION_STATES {
            let round_tripped = review_progress_state_of(change_pack_lifecycle_of(*state));
            assert_ne!(
                round_tripped,
                Some(ReviewProgressState::Rejected),
                "{state:?} must not project back to a reviewer's decision"
            );
            assert_ne!(
                round_tripped,
                Some(ReviewProgressState::Approved),
                "{state:?}"
            );
        }
    }

    #[test]
    fn every_pack_state_maps_to_a_review_position() {
        let positions: std::collections::BTreeSet<_> = ALL_REVISION_STATES
            .iter()
            .map(|s| ReviewProgress::of(*s))
            .collect();
        assert_eq!(
            positions.len(),
            ALL_REVISION_STATES.len(),
            "the projection must not collapse two review positions into one"
        );
    }
}
