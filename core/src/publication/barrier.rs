//! Phase 0 — the per-Publication local bookkeeping barrier.
//!
//! Before a Publication may allocate a new external attempt, everything Draft
//! already knows about that Publication locally must be resolved. The barrier
//! is what asks that question, and its answer is the only gate to a new
//! attempt.
//!
//! # Why it is local-first
//!
//! The barrier never acquires `TrustReadFence`, `ProjectControlStore` or
//! the binding store, and never waits on anything remote. Two separate
//! reasons, both load-bearing.
//!
//! **Lock order.** Phase 0 holds the `PublicationLease` (order 3). Phase 1
//! must take `TrustReadFence` (order 1) first. If Phase 0 kept its lease into
//! Phase 1, that acquisition would be `hold 3 → acquire 1` — a reverse
//! acquisition §2.28 forbids. So Phase 0 ends holding **nothing at all**,
//! lease included, and Phase 1 starts clean. Hoisting the fence over Phase 0
//! would "fix" the order by making a purely local barrier hold a global trust
//! lock it does not need, which is the wrong direction.
//!
//! **Availability.** If the barrier could block on the external system, an
//! outage would stop local Publication bookkeeping from converging. The
//! barrier waits only on work Draft can finish by itself: journal
//! convergence, receipt finalization, fact draining and the exact control
//! clear.
//!
//! # Why the result is an explicit value
//!
//! The barrier is a classifier and a local finalizer — not the executor of
//! every recovery step. When an already-allocated attempt needs current
//! authority or route validation, that validation must happen *after* every
//! Phase-0 lock and lease is released, so the barrier hands off rather than
//! reaching for the locks itself.
//!
//! Making the handoff a value rather than a control-flow fall-through is what
//! stops recovery and creation becoming one implicit transaction. In
//! particular, [`PublicationBookkeepingResult::Clean`] is the **only** value
//! that permits a new attempt — which is also the only point at which new
//! attempt-local identities may be minted. Discovering that an old attempt
//! must be recovered never leaves a freshly minted `pat_B` behind.

use draft_dcg_contract::ids::PublicationAttemptId;

use crate::publication::restart::AttemptResolution;

/// What Phase 0 concluded for one Publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationBookkeepingResult {
    /// No unresolved local state. A new attempt may be allocated, and only now
    /// may its identities be minted.
    Clean,
    /// An earlier attempt's allocation committed. That exact attempt is
    /// authoritative and must be resumed; no new attempt may be allocated.
    RecoverAllocatedAttempt { attempt: PublicationAttemptId },
    /// An earlier attempt may have caused an external effect Draft cannot yet
    /// describe. Nothing local blocks — but no new external attempt may be
    /// made for **this** Publication until the effect is resolved.
    PendingExternalResolution { attempt: PublicationAttemptId },
    /// The local records contradict each other. Doctor/Recovery.
    Inconsistent { detail: String },
}

impl PublicationBookkeepingResult {
    /// Whether a new attempt may be allocated.
    ///
    /// The single question the whole barrier exists to answer, so it is asked
    /// through one predicate rather than by matching on the variant at each
    /// call site — where a later variant could quietly fall into an `_ =>`
    /// arm that permits allocation.
    pub fn permits_new_attempt(&self) -> bool {
        matches!(self, Self::Clean)
    }
}

/// What the barrier found about one Publication's local state.
///
/// Only already-read local values. The barrier has no remote handle, no
/// network client and no way to reach one, so it structurally cannot block on
/// anything external — a future change that wanted to consult something remote
/// here would have to alter this type, which is exactly the review moment such
/// a change deserves.
#[derive(Debug, Clone)]
pub struct BarrierInputs<'a> {
    /// Every attempt journal rooted by this Publication, with the resolution
    /// classification already computed for each.
    pub attempts: &'a [(PublicationAttemptId, AttemptResolution)],
    /// The attempt the control record currently holds in flight.
    pub in_flight_attempt: Option<&'a PublicationAttemptId>,
    /// Whether the control record could be read at all.
    ///
    /// A missing control record is not an absent in-flight attempt. Treating
    /// them alike would let an unreadable record read as "idle".
    pub control_readable: bool,
    /// Whether any local audit fact or outbox entry belonging to this
    /// Publication is still undrained.
    pub undrained_local_work: bool,
    /// Whether a receipt finalization is still outstanding.
    pub unfinished_receipt_finalization: bool,
}

/// Classify one Publication's local state.
///
/// # Why the order of checks is the order of severity
///
/// An inconsistency is reported even when a pending resolution also applies:
/// "these records contradict each other" is a different instruction to a human
/// than "wait for the external system", and reporting the milder one would
/// leave a real bypass sitting behind a retry loop.
pub fn classify(inputs: &BarrierInputs<'_>) -> PublicationBookkeepingResult {
    let result = classify_inner(inputs);
    // Counted once per classification, at the one place that decides. Each
    // barrier result is its own operational fact: an inconsistency needs a
    // human, a pending external resolution needs the provider back, and an
    // `AbandonPrepared` resumption is the recovery §2.57 names separately.
    match &result {
        PublicationBookkeepingResult::Inconsistent { .. } => {
            crate::support::telemetry::Counter::PublicationInconsistentStates.increment();
        }
        PublicationBookkeepingResult::PendingExternalResolution { .. } => {
            crate::support::telemetry::Counter::PublicationReconciliationTotal.increment();
        }
        PublicationBookkeepingResult::RecoverAllocatedAttempt { attempt } => {
            if inputs.attempts.iter().any(|(id, resolution)| {
                id == attempt
                    && matches!(
                        resolution,
                        AttemptResolution::CompleteThenClear | AttemptResolution::DrainThenFinalize
                    )
            }) {
                crate::support::telemetry::Counter::PublicationAbandonPreparedRecoveries
                    .increment();
            }
        }
        PublicationBookkeepingResult::Clean => {}
    }
    result
}

fn classify_inner(inputs: &BarrierInputs<'_>) -> PublicationBookkeepingResult {
    if !inputs.control_readable {
        return PublicationBookkeepingResult::Inconsistent {
            detail: "the Publication's control record is missing or unreadable".into(),
        };
    }

    // Severity first: contradictions, then unresumed allocations, then
    // unresolved external effects.
    for (attempt, resolution) in inputs.attempts {
        if let AttemptResolution::Inconsistent { detail } = resolution {
            return PublicationBookkeepingResult::Inconsistent {
                detail: format!("attempt '{attempt}': {detail}"),
            };
        }
    }

    for (attempt, resolution) in inputs.attempts {
        if let AttemptResolution::ResumeAllocatedAttempt { .. } = resolution {
            return PublicationBookkeepingResult::RecoverAllocatedAttempt {
                attempt: attempt.clone(),
            };
        }
    }

    for (attempt, resolution) in inputs.attempts {
        match resolution {
            AttemptResolution::AwaitExternalResolution { .. } => {
                return PublicationBookkeepingResult::PendingExternalResolution {
                    attempt: attempt.clone(),
                }
            }
            // Everything else is local work the barrier itself finishes. It is
            // reported as an unresumed allocation so the caller drives it to
            // completion rather than allocating past it.
            AttemptResolution::EstablishOutcome
            | AttemptResolution::RetryOutcomeCommit
            | AttemptResolution::AdvanceToOutcomeRecorded
            | AttemptResolution::CompleteThenClear
            | AttemptResolution::VerifyThenFinalize
            | AttemptResolution::DrainThenFinalize => {
                return PublicationBookkeepingResult::RecoverAllocatedAttempt {
                    attempt: attempt.clone(),
                }
            }
            AttemptResolution::AllocationDidNotCommit | AttemptResolution::AlreadyTerminal => {}
            AttemptResolution::ResumeAllocatedAttempt { .. }
            | AttemptResolution::Inconsistent { .. } => {}
        }
    }

    // Every journal is resolved. The control record must agree.
    if let Some(in_flight) = inputs.in_flight_attempt {
        return PublicationBookkeepingResult::Inconsistent {
            detail: format!(
                "the control record holds attempt '{in_flight}' in flight but no journal explains \
                 it"
            ),
        };
    }

    if inputs.undrained_local_work {
        return PublicationBookkeepingResult::Inconsistent {
            detail: "local Publication audit or outbox work is still undrained".into(),
        };
    }
    if inputs.unfinished_receipt_finalization {
        return PublicationBookkeepingResult::Inconsistent {
            detail: "a receipt finalization is still outstanding".into(),
        };
    }

    PublicationBookkeepingResult::Clean
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(suffix: &str) -> PublicationAttemptId {
        PublicationAttemptId::parse(format!("pat_{suffix}")).unwrap()
    }

    fn inputs<'a>(
        attempts: &'a [(PublicationAttemptId, AttemptResolution)],
        in_flight: Option<&'a PublicationAttemptId>,
    ) -> BarrierInputs<'a> {
        BarrierInputs {
            attempts,
            in_flight_attempt: in_flight,
            control_readable: true,
            undrained_local_work: false,
            unfinished_receipt_finalization: false,
        }
    }

    #[test]
    fn nothing_outstanding_is_clean() {
        assert_eq!(
            classify(&inputs(&[], None)),
            PublicationBookkeepingResult::Clean
        );
    }

    #[test]
    fn resolved_history_does_not_block_a_new_attempt() {
        // Terminal attempts and uncommitted candidates are history. A
        // Publication that has been attempted before is not thereby closed.
        let attempts = [
            (attempt("00000000000a"), AttemptResolution::AlreadyTerminal),
            (
                attempt("00000000000b"),
                AttemptResolution::AllocationDidNotCommit,
            ),
        ];
        let result = classify(&inputs(&attempts, None));
        assert_eq!(result, PublicationBookkeepingResult::Clean);
        assert!(result.permits_new_attempt());
    }

    #[test]
    fn a_committed_allocation_is_resumed_not_replaced() {
        let attempts = [(
            attempt("00000000000a"),
            AttemptResolution::ResumeAllocatedAttempt {
                attempt: attempt("00000000000a"),
            },
        )];
        let result = classify(&inputs(&attempts, Some(&attempt("00000000000a"))));
        assert_eq!(
            result,
            PublicationBookkeepingResult::RecoverAllocatedAttempt {
                attempt: attempt("00000000000a")
            }
        );
        assert!(
            !result.permits_new_attempt(),
            "resuming pat_A must never mint pat_B"
        );
    }

    #[test]
    fn an_unresolved_external_effect_blocks_only_this_publication() {
        // Non-blocking is not permission to duplicate: nothing local waits,
        // but this Publication may not dispatch again until the effect is
        // resolved.
        let attempts = [(
            attempt("00000000000a"),
            AttemptResolution::AwaitExternalResolution {
                attempt: attempt("00000000000a"),
            },
        )];
        let result = classify(&inputs(&attempts, Some(&attempt("00000000000a"))));
        assert_eq!(
            result,
            PublicationBookkeepingResult::PendingExternalResolution {
                attempt: attempt("00000000000a")
            }
        );
        assert!(!result.permits_new_attempt());
    }

    #[test]
    fn an_in_flight_reference_no_journal_explains_is_never_clean() {
        // The control record claims an attempt; nothing accounts for it.
        // Draft cannot prove whether that attempt was dispatched, so it must
        // neither clear the reference nor allocate past it.
        let result = classify(&inputs(&[], Some(&attempt("00000000000a"))));
        assert!(matches!(
            result,
            PublicationBookkeepingResult::Inconsistent { .. }
        ));
        assert!(!result.permits_new_attempt());
    }

    #[test]
    fn an_unreadable_control_record_is_never_clean() {
        let mut probe = inputs(&[], None);
        probe.control_readable = false;
        assert!(matches!(
            classify(&probe),
            PublicationBookkeepingResult::Inconsistent { .. }
        ));
    }

    #[test]
    fn an_inconsistency_outranks_a_pending_external_resolution() {
        // "These records contradict each other" and "wait for the external
        // system" are different instructions to a human. Reporting the milder
        // one would leave a bypass behind a retry loop.
        let attempts = [
            (
                attempt("00000000000a"),
                AttemptResolution::AwaitExternalResolution {
                    attempt: attempt("00000000000a"),
                },
            ),
            (
                attempt("00000000000b"),
                AttemptResolution::Inconsistent {
                    detail: "a primary outcome exists for an attempt never dispatched",
                },
            ),
        ];
        assert!(matches!(
            classify(&inputs(&attempts, None)),
            PublicationBookkeepingResult::Inconsistent { .. }
        ));
    }

    #[test]
    fn undrained_local_work_and_unfinished_receipts_are_never_clean() {
        let mut undrained = inputs(&[], None);
        undrained.undrained_local_work = true;
        assert!(!classify(&undrained).permits_new_attempt());

        let mut receipts = inputs(&[], None);
        receipts.unfinished_receipt_finalization = true;
        assert!(!classify(&receipts).permits_new_attempt());
    }

    #[test]
    fn local_finalization_work_is_driven_to_completion_before_a_new_attempt() {
        // Each of these is work the barrier itself can finish. None of them
        // permits allocating past the attempt that owes it.
        for resolution in [
            AttemptResolution::EstablishOutcome,
            AttemptResolution::RetryOutcomeCommit,
            AttemptResolution::AdvanceToOutcomeRecorded,
            AttemptResolution::CompleteThenClear,
            AttemptResolution::VerifyThenFinalize,
            AttemptResolution::DrainThenFinalize,
        ] {
            let attempts = [(attempt("00000000000a"), resolution.clone())];
            let result = classify(&inputs(&attempts, Some(&attempt("00000000000a"))));
            assert!(
                !result.permits_new_attempt(),
                "{resolution:?} must not permit a new attempt"
            );
        }
    }
}
