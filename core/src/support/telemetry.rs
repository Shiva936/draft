//! Operational counters — and the proof of which ones anything actually emits.
//!
//! # Telemetry is never evidence
//!
//! A counter says how often Draft took a path. It never says what the project
//! decided. Metrics, traces and logs never become canonical Change Graph state
//! and never substitute for an Activity event: no counter value participates in
//! any canonical digest or receipt payload identity, and none is durable.
//! Restarting Draft resets every counter here, which is exactly right for a
//! number that describes a process rather than a project.
//!
//! Nothing observable through this module may carry a credential secret, a
//! token, a private key, a raw secret handle or a sensitive extension
//! environment value. The type makes that structural rather than a rule to
//! remember: a [`Counter`] carries a `u64` and a frozen name, and there is
//! nowhere to put a string.
//!
//! # Why an unemitted counter reads as unknown, not zero
//!
//! The vocabulary below is frozen and complete. The *emission* is not: some
//! counters name paths Draft does not yet have. A registry that returned `0`
//! for those would make "we never saw this" and "nothing can see this"
//! indistinguishable — the same mistake as inferring complete observation from
//! an empty Resource set.
//!
//! So [`Counter::observed`] returns `Option<u64>`, and `None` means *no site
//! emits this yet*. Which counters those are is not a comment: it is
//! [`EMITTED`], and `scripts/check-telemetry-completeness.sh` fails if that
//! list disagrees with the call sites actually present in the tree.

use std::sync::atomic::{AtomicU64, Ordering};

/// The frozen v1 operational counter vocabulary.
///
/// Names are part of the operational contract: an operator's dashboards and
/// alerts are written against them, so a rename is a breaking change and a
/// counter is retired rather than repurposed. No name may imply a canonical
/// Publication fact type — `publication_late_result_observations` counts
/// reconciliation *inputs* observed, not a `PublicationOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(usize)]
pub enum Counter {
    ProjectControlCasConflicts,
    ProviderBindingCasConflicts,
    ChangeDefinitionCasConflicts,
    PublicationControlCasConflicts,
    PublicationRegistryConflicts,
    PublicationOutcomeConflicts,
    PublicationResolutionHeadConflicts,
    PublicationOutcomeRecoveryFastForwards,
    PublicationAbandonedBeforeDispatch,
    PublicationAttemptNumberGaps,
    PublicationAllocationCasFailures,
    PublicationAbandonPreparedRecoveries,
    PublicationJournalTransitionConflicts,
    PublicationLateResultObservations,
    PublicationRouteStalenessRefusals,
    ImmutableFactIntegrityViolations,
    PublicationSelfConsistencyRejections,
    PublicationAuthorityLinearizationRefusals,
    TaskRecordCasConflicts,
    /// Accumulated microseconds spent waiting for a [`crate::support::process_lock::ProcessFileLock`].
    ///
    /// Microseconds rather than the seconds the name says, because an integer
    /// count of seconds would round every ordinary wait to zero and report a
    /// contended system as an idle one. Read it through
    /// [`Counter::process_lock_wait_seconds`].
    ProcessLockWaitMicros,
    StaleLeaseFenceRejections,
    /// Accumulated microseconds spent waiting to append to the Activity Ledger.
    ///
    /// A duration rather than a tally of waits: "somebody waited" is true on
    /// every busy system, and how long they waited is what tells ordinary
    /// contention from a stuck holder.
    ActivityAppendContention,
    ActivityTornTailTruncations,
    ActivityHardCorruptions,
    ActivityChainVerifyFailures,
    MutationJournalAbandoned,
    MutationJournalRecovered,
    PromotionRecoveryFinalizations,
    PromotionChangeCompletionRecoveries,
    PromotionInconsistentStates,
    PublicationIndeterminateTotal,
    PublicationNoEffectTotal,
    PublicationReconciliationTotal,
    PublicationUnsafeRetryAuthorizations,
    PublicationInconsistentStates,
    ObservationDigestMismatches,
    ObservationRunDigestMismatches,
    PublicationDigestMismatches,
    PublicationAttemptDigestMismatches,
    SecurityFactDigestMismatches,
    CoverageEvidenceVerifyFailures,
    SemanticsContractConflicts,
    GcObjectsMarked,
    GcObjectsCollected,
    GcRecoveryRootsPreserved,
    ReadModelStaleRejections,
}

/// Every counter, in declaration order.
pub const ALL: &[Counter] = &[
    Counter::ProjectControlCasConflicts,
    Counter::ProviderBindingCasConflicts,
    Counter::ChangeDefinitionCasConflicts,
    Counter::PublicationControlCasConflicts,
    Counter::PublicationRegistryConflicts,
    Counter::PublicationOutcomeConflicts,
    Counter::PublicationResolutionHeadConflicts,
    Counter::PublicationOutcomeRecoveryFastForwards,
    Counter::PublicationAbandonedBeforeDispatch,
    Counter::PublicationAttemptNumberGaps,
    Counter::PublicationAllocationCasFailures,
    Counter::PublicationAbandonPreparedRecoveries,
    Counter::PublicationJournalTransitionConflicts,
    Counter::PublicationLateResultObservations,
    Counter::PublicationRouteStalenessRefusals,
    Counter::ImmutableFactIntegrityViolations,
    Counter::PublicationSelfConsistencyRejections,
    Counter::PublicationAuthorityLinearizationRefusals,
    Counter::TaskRecordCasConflicts,
    Counter::ProcessLockWaitMicros,
    Counter::StaleLeaseFenceRejections,
    Counter::ActivityAppendContention,
    Counter::ActivityTornTailTruncations,
    Counter::ActivityHardCorruptions,
    Counter::ActivityChainVerifyFailures,
    Counter::MutationJournalAbandoned,
    Counter::MutationJournalRecovered,
    Counter::PromotionRecoveryFinalizations,
    Counter::PromotionChangeCompletionRecoveries,
    Counter::PromotionInconsistentStates,
    Counter::PublicationIndeterminateTotal,
    Counter::PublicationNoEffectTotal,
    Counter::PublicationReconciliationTotal,
    Counter::PublicationUnsafeRetryAuthorizations,
    Counter::PublicationInconsistentStates,
    Counter::ObservationDigestMismatches,
    Counter::ObservationRunDigestMismatches,
    Counter::PublicationDigestMismatches,
    Counter::PublicationAttemptDigestMismatches,
    Counter::SecurityFactDigestMismatches,
    Counter::CoverageEvidenceVerifyFailures,
    Counter::SemanticsContractConflicts,
    Counter::GcObjectsMarked,
    Counter::GcObjectsCollected,
    Counter::GcRecoveryRootsPreserved,
    Counter::ReadModelStaleRejections,
];

/// The counters some code path in this tree actually increments.
///
/// Kept honest by `scripts/check-telemetry-completeness.sh`, which fails three
/// ways: a counter listed here with no production call site, a call site for a
/// counter not listed here, and — now that the vocabulary is fully wired — any
/// frozen counter missing from this list at all. A counter absent from it
/// reports `None` rather than `0`, so an operator would never be told a path
/// was never taken when the truth is that nothing was watching.
///
/// It currently holds every frozen §2.57 name. That is the finished state, not
/// a coincidence: each counter names an architectural boundary, and a boundary
/// with no counter is a boundary Draft cannot see.
pub const EMITTED: &[Counter] = &[
    Counter::ActivityAppendContention,
    Counter::ActivityChainVerifyFailures,
    Counter::ActivityHardCorruptions,
    Counter::ActivityTornTailTruncations,
    Counter::ChangeDefinitionCasConflicts,
    Counter::CoverageEvidenceVerifyFailures,
    Counter::GcObjectsCollected,
    Counter::GcObjectsMarked,
    Counter::GcRecoveryRootsPreserved,
    Counter::ImmutableFactIntegrityViolations,
    Counter::MutationJournalAbandoned,
    Counter::MutationJournalRecovered,
    Counter::ObservationDigestMismatches,
    Counter::ObservationRunDigestMismatches,
    Counter::ProcessLockWaitMicros,
    Counter::ProjectControlCasConflicts,
    Counter::PromotionChangeCompletionRecoveries,
    Counter::PromotionInconsistentStates,
    Counter::PromotionRecoveryFinalizations,
    Counter::ProviderBindingCasConflicts,
    Counter::PublicationAbandonPreparedRecoveries,
    Counter::PublicationAbandonedBeforeDispatch,
    Counter::PublicationAllocationCasFailures,
    Counter::PublicationAttemptDigestMismatches,
    Counter::PublicationAttemptNumberGaps,
    Counter::PublicationAuthorityLinearizationRefusals,
    Counter::PublicationControlCasConflicts,
    Counter::PublicationDigestMismatches,
    Counter::PublicationInconsistentStates,
    Counter::PublicationIndeterminateTotal,
    Counter::PublicationJournalTransitionConflicts,
    Counter::PublicationLateResultObservations,
    Counter::PublicationNoEffectTotal,
    Counter::PublicationOutcomeConflicts,
    Counter::PublicationOutcomeRecoveryFastForwards,
    Counter::PublicationReconciliationTotal,
    Counter::PublicationRegistryConflicts,
    Counter::PublicationResolutionHeadConflicts,
    Counter::PublicationRouteStalenessRefusals,
    Counter::PublicationSelfConsistencyRejections,
    Counter::PublicationUnsafeRetryAuthorizations,
    Counter::ReadModelStaleRejections,
    Counter::SecurityFactDigestMismatches,
    Counter::SemanticsContractConflicts,
    Counter::StaleLeaseFenceRejections,
    Counter::TaskRecordCasConflicts,
];

const NAMES: [&str; ALL.len()] = [
    "project_control_cas_conflicts",
    "provider_binding_cas_conflicts",
    "change_definition_cas_conflicts",
    "publication_control_cas_conflicts",
    "publication_registry_conflicts",
    "publication_outcome_conflicts",
    "publication_resolution_head_conflicts",
    "publication_outcome_recovery_fast_forwards",
    "publication_abandoned_before_dispatch",
    "publication_attempt_number_gaps",
    "publication_allocation_cas_failures",
    "publication_abandon_prepared_recoveries",
    "publication_journal_transition_conflicts",
    "publication_late_result_observations",
    "publication_route_staleness_refusals",
    "immutable_fact_integrity_violations",
    "publication_self_consistency_rejections",
    "publication_authority_linearization_refusals",
    "task_record_cas_conflicts",
    "process_lock_wait_seconds",
    "stale_lease_fence_rejections",
    "activity_append_contention",
    "activity_torn_tail_truncations",
    "activity_hard_corruptions",
    "activity_chain_verify_failures",
    "mutation_journal_abandoned",
    "mutation_journal_recovered",
    "promotion_recovery_finalizations",
    "promotion_change_completion_recoveries",
    "promotion_inconsistent_states",
    "publication_indeterminate_total",
    "publication_no_effect_total",
    "publication_reconciliation_total",
    "publication_unsafe_retry_authorizations",
    "publication_inconsistent_states",
    "observation_digest_mismatches",
    "observation_run_digest_mismatches",
    "publication_digest_mismatches",
    "publication_attempt_digest_mismatches",
    "security_fact_digest_mismatches",
    "coverage_evidence_verify_failures",
    "semantics_contract_conflicts",
    "gc_objects_marked",
    "gc_objects_collected",
    "gc_recovery_roots_preserved",
    "read_model_stale_rejections",
];

// One slot per counter, process-local and reset by a restart. `const` rather
// than a lazily built map so there is no initialization order to get wrong and
// no lock on the increment path.
#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU64 = AtomicU64::new(0);
static SLOTS: [AtomicU64; ALL.len()] = [ZERO; ALL.len()];

impl Counter {
    /// The frozen operational name.
    pub const fn name(self) -> &'static str {
        NAMES[self as usize]
    }

    /// Whether any code path in this tree increments this counter.
    pub fn is_emitted(self) -> bool {
        EMITTED.contains(&self)
    }

    /// Count one occurrence.
    ///
    /// `Relaxed` deliberately: these are independent tallies, and nothing reads
    /// one to decide anything, so ordering them against other memory would buy
    /// a guarantee no caller needs and pay for it on every increment.
    pub fn increment(self) {
        self.add(1);
    }

    /// Count `amount` occurrences.
    pub fn add(self, amount: u64) {
        SLOTS[self as usize].fetch_add(amount, Ordering::Relaxed);
    }

    /// What this process has seen, or `None` when nothing emits this counter.
    pub fn observed(self) -> Option<u64> {
        self.is_emitted()
            .then(|| SLOTS[self as usize].load(Ordering::Relaxed))
    }
}

/// Record a wait for a correctness lock.
pub fn record_lock_wait(waited: std::time::Duration) {
    Counter::ProcessLockWaitMicros.add(waited.as_micros() as u64);
}

/// The whole registry, for a diagnostic that renders it.
///
/// Ordered by name so two renderings of the same process compare directly, and
/// carrying `None` through rather than flattening it to zero.
pub fn snapshot() -> Vec<(&'static str, Option<u64>)> {
    let mut rows: Vec<_> = ALL
        .iter()
        .map(|counter| (counter.name(), counter.observed()))
        .collect();
    rows.sort_by_key(|(name, _)| *name);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_counter_has_its_own_frozen_name() {
        let mut names: Vec<_> = ALL.iter().map(|counter| counter.name()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "two counters share a name");
        assert_eq!(total, NAMES.len());
    }

    /// The finished state: every frozen name is watched by something.
    #[test]
    fn every_frozen_counter_is_attached_to_a_boundary() {
        for counter in ALL {
            assert!(
                counter.observed().is_some(),
                "{} reports unknown, so nothing emits it",
                counter.name()
            );
        }
    }

    /// The distinction the whole module exists for, kept representable.
    ///
    /// `observed()` returns `Option<u64>` rather than `u64` even now that
    /// nothing is unknown. A counter added ahead of the boundary it describes
    /// must report *nothing is watching this*, not a `0` that reads as *this
    /// never happened* — the same mistake as inferring complete observation
    /// from an empty Resource set.
    ///
    /// `EMITTED` is what decides, and
    /// `scripts/check-telemetry-completeness.sh` keeps it honest against the
    /// call sites actually in the tree.
    #[test]
    fn unknown_stays_representable_even_though_nothing_is_unknown() {
        assert_eq!(EMITTED.len(), ALL.len());
        assert!(ALL.iter().all(|counter| counter.is_emitted()));
    }

    #[test]
    fn a_snapshot_reports_every_counter_in_name_order() {
        let rows = snapshot();
        assert_eq!(rows.len(), ALL.len());
        assert!(rows.iter().all(|(_, value)| value.is_some()));
        assert!(rows.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    }

    #[test]
    fn counting_accumulates() {
        let before = Counter::MutationJournalRecovered.observed().unwrap();
        Counter::MutationJournalRecovered.increment();
        Counter::MutationJournalRecovered.add(2);
        assert_eq!(
            Counter::MutationJournalRecovered.observed().unwrap(),
            before + 3
        );
    }
}
