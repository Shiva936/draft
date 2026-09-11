//! Authorized reinterpretation of a recorded outcome — and the one shape that
//! every such transaction shares.
//!
//! A primary outcome is immutable and never re-competed. When better
//! information arrives — the external system's real answer after Draft had to
//! record `Indeterminate`, say — the only way it becomes authoritative history
//! is an authorized [`PublicationResolution`] advancing the resolution head.
//!
//! # Why recovery runs before, and independently of, authorization
//!
//! The transaction has two phases, in this order:
//!
//! ```text
//! Phase R  recover any prior unresolved transaction   NO current authority
//! Phase C  create a new one                           CURRENT authority required
//! ```
//!
//! Finishing something that already committed is not a new decision. If Phase
//! R needed current authority, then a grant revoked after a Resolution
//! committed would leave that Resolution permanently unfinalizable — its
//! receipt unsigned and its fact undrained — because the authority that
//! *authorized* it is gone. The commit already happened; refusing to finish
//! the bookkeeping would not undo it, it would only leave it unreadable.
//!
//! So Phase R works entirely from the frozen commit-time snapshot the journal
//! recorded, and a later revocation does not block it.
//!
//! The converse is equally deliberate: **Phase C never skips current
//! authority**. Proposing something genuinely new is always a fresh decision,
//! evaluated against the security state as it is now.
//!
//! # Why Phase C restarts rather than recovering in place
//!
//! Phase C holds `ProjectControlStore` — and, when policy has global trust
//! dependencies, `TrustReadFence`. If a *new* unresolved transaction appeared
//! between Phase R and Phase C, recovering it right there would mean running
//! recovery under current-authority guards, which is precisely what the phase
//! split exists to prevent.
//!
//! So Phase C releases both guards, restarts from Phase R, and — because the
//! security state it validated has now been released and may have moved —
//! evaluates fresh current authority again before proposing anything new.
//!
//! # Why a resolving grant must be serialized against security state
//!
//! A `SecurityFactRef` that resolves proves *which* grant was cited, not that
//! the grant was live at the moment of commit. Without serializing against
//! current security state, a revocation could land between validation and the
//! head compare-exchange.
//!
//! Holding the control lock across both makes exactly one order win:
//! revocation first and the Resolution refuses; Resolution first and it is
//! historical fact that a later revocation does not erase.

use serde::{Deserialize, Serialize};

use draft_dcg_contract::publication::{PublicationOutcomeDigest, PublicationResolutionDigest};

/// How far a linearized fact-creation transaction got.
///
/// Shared by Resolution creation and retry-authorization creation. The two are
/// the same model, not merely a similar one: an immutable fact, a head or
/// journal advanced by compare-exchange, and receipt and audit work that
/// finishes afterwards from a frozen snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactCreationState {
    /// The candidate and its commit-time snapshot are durable. Whether the
    /// candidate became authoritative is decided by the head, not by this.
    Prepared,
    /// The compare-exchange landed. The fact is authoritative; receipt and
    /// audit work may still be outstanding.
    Committed,
    /// Everything finished.
    Finalized,
}

/// How the current head compares to the transaction's exact values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadMatch {
    /// Still the head the transaction expected: the compare-exchange did not
    /// land.
    Expected,
    /// The candidate itself: the compare-exchange landed.
    Candidate,
    /// Neither.
    Neither,
}

impl HeadMatch {
    /// Classify the current head against a transaction's exact pair.
    ///
    /// `None` on either side is a real value, not a missing one: the first
    /// Resolution for an outcome legitimately expects no head at all.
    pub fn classify(
        current: Option<&PublicationResolutionDigest>,
        expected: Option<&PublicationResolutionDigest>,
        candidate: &PublicationResolutionDigest,
    ) -> Self {
        match current {
            value if value == expected => Self::Expected,
            Some(value) if value == candidate => Self::Candidate,
            _ => Self::Neither,
        }
    }
}

/// What Phase R concluded about a prior transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PriorTransaction {
    /// Nothing outstanding. Phase C may proceed.
    None,
    /// The candidate never became authoritative. Nothing was authorized, so
    /// there is nothing to finalize — the orphaned candidate is collectable.
    ///
    /// Never "completed anyway": committing it now would use authority that
    /// was evaluated at a moment which has passed.
    AbandonOrphan,
    /// The candidate is authoritative. Finalize its receipt and fact exactly
    /// once, from the frozen commit-time snapshot.
    ///
    /// A grant revoked since the commit does not block this.
    FinalizeFromFrozenSnapshot,
    /// Already finished.
    AlreadyFinalized,
    /// The head is neither the expected value nor the candidate.
    Inconsistent { detail: &'static str },
}

/// Classify a prior transaction — Phase R, in full.
///
/// Takes only already-read local values, and deliberately takes **no**
/// authority or security input: there is nothing for it to evaluate them
/// against, and accepting them would invite a caller to make finalization
/// contingent on them.
pub fn classify_prior(state: Option<FactCreationState>, head: HeadMatch) -> PriorTransaction {
    match state {
        None => PriorTransaction::None,
        Some(FactCreationState::Finalized) => PriorTransaction::AlreadyFinalized,
        Some(FactCreationState::Prepared) => match head {
            HeadMatch::Expected => PriorTransaction::AbandonOrphan,
            HeadMatch::Candidate => PriorTransaction::FinalizeFromFrozenSnapshot,
            HeadMatch::Neither => PriorTransaction::Inconsistent {
                detail: "a prepared transaction whose head is neither the value it expected nor \
                         the candidate it offered",
            },
        },
        Some(FactCreationState::Committed) => match head {
            HeadMatch::Candidate => PriorTransaction::FinalizeFromFrozenSnapshot,
            HeadMatch::Expected => PriorTransaction::Inconsistent {
                detail: "a transaction marked committed whose head never advanced",
            },
            HeadMatch::Neither => PriorTransaction::Inconsistent {
                detail: "a transaction marked committed whose head is neither its expected value \
                         nor its candidate",
            },
        },
    }
}

/// What Phase C must do, having re-read under the head lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationStep {
    /// Nothing appeared in between. Persist, write the immutable fact, and
    /// compare-exchange the head.
    Commit,
    /// A new unresolved transaction appeared since Phase R. Release the head
    /// lock, the control lock and the trust fence, and restart from Phase R —
    /// then re-evaluate current authority before proposing anything new.
    ReleaseAndRestartFromPhaseR,
}

/// Decide Phase C's next step.
pub fn creation_step(observed_prior: Option<FactCreationState>) -> CreationStep {
    match observed_prior {
        None | Some(FactCreationState::Finalized) => CreationStep::Commit,
        Some(FactCreationState::Prepared) | Some(FactCreationState::Committed) => {
            CreationStep::ReleaseAndRestartFromPhaseR
        }
    }
}

/// A candidate Resolution's self-consistency, independent of its digest.
///
/// A digest proves the bytes are unchanged. It does not prove the object was
/// filed under the right key, or that a supersession chain stayed within one
/// outcome — so both are checked rather than assumed.
pub fn resolution_targets_head(
    resolution_outcome: &PublicationOutcomeDigest,
    head_outcome: &PublicationOutcomeDigest,
) -> bool {
    resolution_outcome == head_outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::Digest;

    fn digest(bytes: &[u8]) -> PublicationResolutionDigest {
        PublicationResolutionDigest::new(Digest::of_bytes(bytes))
    }

    #[test]
    fn a_committed_resolution_is_finalized_from_its_frozen_snapshot() {
        // The property a revocation must not break: the commit already
        // happened, so refusing to finish would leave an authoritative fact
        // with an unsigned receipt rather than undoing anything.
        assert_eq!(
            classify_prior(Some(FactCreationState::Committed), HeadMatch::Candidate),
            PriorTransaction::FinalizeFromFrozenSnapshot
        );
        assert_eq!(
            classify_prior(Some(FactCreationState::Prepared), HeadMatch::Candidate),
            PriorTransaction::FinalizeFromFrozenSnapshot,
            "a crash between the head advance and the state mark still committed"
        );
    }

    #[test]
    fn a_candidate_that_never_advanced_the_head_is_abandoned_not_completed() {
        // Committing it now would use authority evaluated at a moment that has
        // passed. Phase R has no authority input precisely so it cannot.
        assert_eq!(
            classify_prior(Some(FactCreationState::Prepared), HeadMatch::Expected),
            PriorTransaction::AbandonOrphan
        );
    }

    #[test]
    fn a_committed_transaction_whose_head_never_moved_is_a_contradiction() {
        assert!(matches!(
            classify_prior(Some(FactCreationState::Committed), HeadMatch::Expected),
            PriorTransaction::Inconsistent { .. }
        ));
        assert!(matches!(
            classify_prior(Some(FactCreationState::Prepared), HeadMatch::Neither),
            PriorTransaction::Inconsistent { .. }
        ));
    }

    #[test]
    fn nothing_outstanding_lets_creation_proceed() {
        assert_eq!(
            classify_prior(None, HeadMatch::Expected),
            PriorTransaction::None
        );
        assert_eq!(
            classify_prior(Some(FactCreationState::Finalized), HeadMatch::Candidate),
            PriorTransaction::AlreadyFinalized
        );
        assert_eq!(creation_step(None), CreationStep::Commit);
        assert_eq!(
            creation_step(Some(FactCreationState::Finalized)),
            CreationStep::Commit
        );
    }

    #[test]
    fn a_transaction_that_appeared_since_phase_r_forces_a_restart() {
        // Recovering it in place would run recovery under the current-authority
        // guards Phase C holds, which is exactly what the phase split prevents.
        for state in [FactCreationState::Prepared, FactCreationState::Committed] {
            assert_eq!(
                creation_step(Some(state)),
                CreationStep::ReleaseAndRestartFromPhaseR,
                "{state:?} must force a restart, not an in-place recovery"
            );
        }
    }

    #[test]
    fn the_first_resolution_for_an_outcome_expects_no_head_at_all() {
        let candidate = digest(b"first");
        assert_eq!(
            HeadMatch::classify(None, None, &candidate),
            HeadMatch::Expected
        );
        assert_eq!(
            HeadMatch::classify(Some(&candidate), None, &candidate),
            HeadMatch::Candidate
        );
    }

    #[test]
    fn a_head_that_moved_to_somebody_elses_resolution_matches_neither() {
        let expected = digest(b"previous");
        let candidate = digest(b"mine");
        let elsewhere = digest(b"theirs");
        assert_eq!(
            HeadMatch::classify(Some(&elsewhere), Some(&expected), &candidate),
            HeadMatch::Neither
        );
        assert_eq!(
            HeadMatch::classify(Some(&expected), Some(&expected), &candidate),
            HeadMatch::Expected
        );
    }

    #[test]
    fn a_supersession_chain_may_not_cross_outcomes() {
        let one = PublicationOutcomeDigest::new(Digest::of_bytes(b"outcome-1"));
        let other = PublicationOutcomeDigest::new(Digest::of_bytes(b"outcome-2"));
        assert!(resolution_targets_head(&one, &one));
        assert!(!resolution_targets_head(&one, &other));
    }
}
