//! The one lock partial order, enforced while locks are held.
//!
//! ```text
//!  1. TrustReadFence                 global trust registry
//!  2. ProjectControlLease            product lease
//!  3. PublicationLease               product lease
//!  4. ProjectControlStore            project/control.lock
//!  5. ProviderBindingStore           provider-bindings/<pbd_>.lock
//!  6. PublicationJournalStore        publication/journal/<pat_>.lock
//!  7. PublicationControlStore        publication/control/<pub_>.lock
//!  8. Per-record domain Stores       changes/<cpk_>.lock, tasks/<tsk_>.lock
//!  9. Publication registry / outcome-head / resolution-head / retry-authorization
//! 10. Activity Ledger                events/events.lock
//! ```
//!
//! # The rule is about what is HELD, not what happened earlier
//!
//! The whole rule is one sentence:
//!
//! > on acquiring lock X: every correctness lock **currently held** must have
//! > order < X.
//!
//! That is deliberately *not* "an operation's acquisitions increase over its
//! lifetime". The difference decides whether correct code passes. This is
//! legal, because nothing higher is still held when the second phase begins:
//!
//! ```text
//! acquire 6 -> acquire 9 -> release 9 -> release 6 -> acquire 6 -> acquire 7
//! ```
//!
//! and this is not:
//!
//! ```text
//! hold 9 -> acquire 7
//! ```
//!
//! A checker that required chronological monotonicity would reject the first,
//! which is the ordinary shape of a multi-phase publication completion. Since
//! no reverse acquisition is possible while anything higher is held, no cycle
//! can form, so no deadlock can.
//!
//! # Group exclusivity
//!
//! At most one group-8 lock is held at a time, and likewise at most one
//! group-9. Two records at the same rank have no order between them, so
//! holding both is exactly the situation in which two operations could take
//! them in opposite orders.
//!
//! # Smallest sufficient lockset
//!
//! An operation acquires only what it needs. Taking a lock merely because it
//! sits earlier in the order would serialize unrelated work and, worse, make
//! the acquisition set depend on the table rather than on the operation.

use std::cell::RefCell;

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// Where a correctness lock sits in the partial order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LockOrder {
    TrustReadFence = 1,
    ProjectControlLease = 2,
    PublicationLease = 3,
    ProjectControlStore = 4,
    ProviderBindingStore = 5,
    PublicationJournalStore = 6,
    PublicationControlStore = 7,
    /// Per-record domain Stores: `changes/<cpk_>.lock`, `tasks/<tsk_>.lock`.
    DomainRecordStore = 8,
    /// Publication registry, outcome heads, resolution heads and retry
    /// authorizations. Mutually exclusive with one another.
    PublicationAuxiliary = 9,
    ActivityLedger = 10,
}

impl LockOrder {
    pub fn rank(self) -> u8 {
        self as u8
    }

    /// Whether only one lock of this rank may be held at a time.
    ///
    /// Ranks 8 and 9 hold several distinct records, so two of them have no
    /// order between them — which is precisely when two operations could take
    /// them in opposite orders.
    pub fn is_exclusive_within_rank(self) -> bool {
        matches!(self, Self::DomainRecordStore | Self::PublicationAuxiliary)
    }

    fn describe(self) -> &'static str {
        match self {
            Self::TrustReadFence => "TrustReadFence",
            Self::ProjectControlLease => "ProjectControlLease",
            Self::PublicationLease => "PublicationLease",
            Self::ProjectControlStore => "ProjectControlStore",
            Self::ProviderBindingStore => "ProviderBindingStore",
            Self::PublicationJournalStore => "PublicationJournalStore",
            Self::PublicationControlStore => "PublicationControlStore",
            Self::DomainRecordStore => "a per-record domain Store",
            Self::PublicationAuxiliary => "a Publication auxiliary lock",
            Self::ActivityLedger => "the Activity Ledger",
        }
    }
}

thread_local! {
    static HELD: RefCell<Vec<LockOrder>> = const { RefCell::new(Vec::new()) };
}

/// A held position in the order. Releases when dropped.
#[derive(Debug)]
pub struct LockOrderGuard {
    order: LockOrder,
}

impl Drop for LockOrderGuard {
    fn drop(&mut self) {
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            // Locks nest, so the innermost is the one being released. Search
            // from the top rather than assuming, so an out-of-order drop is
            // handled rather than corrupting the stack.
            if let Some(position) = held.iter().rposition(|entry| *entry == self.order) {
                held.remove(position);
            }
        });
    }
}

/// Record that `order` is about to be held, refusing a reverse acquisition.
pub fn enter(order: LockOrder) -> DraftResult<LockOrderGuard> {
    HELD.with(|held| {
        let mut held = held.borrow_mut();
        if let Some(blocking) = held.iter().find(|entry| entry.rank() > order.rank()) {
            return Err(reverse_acquisition(*blocking, order));
        }
        if order.is_exclusive_within_rank() && held.contains(&order) {
            return Err(DraftError::new(
                DraftErrorKind::Internal,
                format!(
                    "two {} locks would be held at once; locks of equal rank have no order \
                     between them, so two operations could take them in opposite orders",
                    order.describe()
                ),
            ));
        }
        held.push(order);
        Ok(LockOrderGuard { order })
    })
}

fn reverse_acquisition(held: LockOrder, wanted: LockOrder) -> DraftError {
    DraftError::new(
        DraftErrorKind::Internal,
        format!(
            "reverse lock acquisition: {} (order {}) is held while acquiring {} (order {}). \
             Release the higher-order lock first; a phase boundary is legal, a nested \
             reverse acquisition is not.",
            held.describe(),
            held.rank(),
            wanted.describe(),
            wanted.rank()
        ),
    )
}

/// The locks currently held on this thread, outermost first.
///
/// For assertions and diagnostics.
pub fn currently_held() -> Vec<LockOrder> {
    HELD.with(|held| held.borrow().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_descending_nest_is_permitted() {
        let _fence = enter(LockOrder::TrustReadFence).unwrap();
        let _lease = enter(LockOrder::PublicationLease).unwrap();
        let _control = enter(LockOrder::ProjectControlStore).unwrap();
        let _binding = enter(LockOrder::ProviderBindingStore).unwrap();
        let _journal = enter(LockOrder::PublicationJournalStore).unwrap();
        let _publication = enter(LockOrder::PublicationControlStore).unwrap();
        assert_eq!(currently_held().len(), 6);
    }

    #[test]
    fn a_reverse_acquisition_is_refused() {
        let _journal = enter(LockOrder::PublicationJournalStore).unwrap();
        // Every one of these is explicitly forbidden by the architecture.
        for lower in [
            LockOrder::TrustReadFence,
            LockOrder::PublicationLease,
            LockOrder::ProjectControlStore,
            LockOrder::ProviderBindingStore,
        ] {
            let error = enter(lower).unwrap_err();
            assert!(
                error.message.contains("reverse lock acquisition"),
                "{}",
                error.message
            );
        }
    }

    #[test]
    fn a_lease_may_not_be_held_while_taking_the_trust_fence() {
        // hold 3 -> acquire 1, the bookkeeping barrier's specific hazard.
        let _lease = enter(LockOrder::PublicationLease).unwrap();
        assert!(enter(LockOrder::TrustReadFence).is_err());
    }

    #[test]
    fn a_group_nine_lock_may_not_reach_back_to_publication_control() {
        // hold 9 -> acquire 7.
        let _auxiliary = enter(LockOrder::PublicationAuxiliary).unwrap();
        assert!(enter(LockOrder::PublicationControlStore).is_err());
    }

    #[test]
    fn release_and_reacquire_across_a_phase_boundary_is_legal() {
        // The sequence a chronological-monotonicity checker would wrongly
        // reject: 6 -> 9 -> release both -> 6 -> 7. This is the ordinary shape
        // of publication completion.
        {
            let _journal = enter(LockOrder::PublicationJournalStore).unwrap();
            let _auxiliary = enter(LockOrder::PublicationAuxiliary).unwrap();
        }
        assert!(currently_held().is_empty());

        let _journal = enter(LockOrder::PublicationJournalStore).unwrap();
        let _control = enter(LockOrder::PublicationControlStore).unwrap();
        assert_eq!(
            currently_held(),
            vec![
                LockOrder::PublicationJournalStore,
                LockOrder::PublicationControlStore
            ]
        );
    }

    #[test]
    fn a_barrier_that_ends_holding_nothing_may_then_take_the_fence() {
        // Phase 0 releases its lease before Phase 1 takes TrustReadFence.
        // Retaining the lease would make that a reverse acquisition.
        {
            let _lease = enter(LockOrder::PublicationLease).unwrap();
            let _journal = enter(LockOrder::PublicationJournalStore).unwrap();
        }
        assert!(currently_held().is_empty());
        let _fence = enter(LockOrder::TrustReadFence).unwrap();
        let _fresh_lease = enter(LockOrder::PublicationLease).unwrap();
    }

    #[test]
    fn only_one_lock_of_an_exclusive_rank_is_held_at_a_time() {
        // Two records of equal rank have no order between them, so holding
        // both is exactly when two operations could deadlock.
        let _first = enter(LockOrder::DomainRecordStore).unwrap();
        assert!(enter(LockOrder::DomainRecordStore).is_err());

        let _auxiliary = enter(LockOrder::PublicationAuxiliary).unwrap();
        assert!(enter(LockOrder::PublicationAuxiliary).is_err());
    }

    #[test]
    fn a_non_exclusive_rank_is_not_restricted_to_one() {
        // Only ranks 8 and 9 hold multiple distinct records; the singular
        // locks are naturally unique and need no extra rule.
        let _control = enter(LockOrder::ProjectControlStore).unwrap();
        assert!(enter(LockOrder::ProjectControlStore).is_ok());
    }

    #[test]
    fn releasing_out_of_order_does_not_corrupt_the_held_set() {
        let outer = enter(LockOrder::ProjectControlStore).unwrap();
        let inner = enter(LockOrder::PublicationJournalStore).unwrap();
        drop(outer);
        assert_eq!(currently_held(), vec![LockOrder::PublicationJournalStore]);
        drop(inner);
        assert!(currently_held().is_empty());
    }

    #[test]
    fn the_promotion_commit_lockset_is_permitted() {
        // TrustReadFence (1) -> ProjectControlLease (2) -> control.lock (4)
        // -> changes/<cpk_>.lock (8).
        let _fence = enter(LockOrder::TrustReadFence).unwrap();
        let _lease = enter(LockOrder::ProjectControlLease).unwrap();
        let _control = enter(LockOrder::ProjectControlStore).unwrap();
        let _change = enter(LockOrder::DomainRecordStore).unwrap();
        let _ledger = enter(LockOrder::ActivityLedger).unwrap();
        assert_eq!(currently_held().len(), 5);
    }

    #[test]
    fn the_ledger_is_innermost_so_nothing_is_taken_while_appending() {
        let _ledger = enter(LockOrder::ActivityLedger).unwrap();
        for earlier in [
            LockOrder::TrustReadFence,
            LockOrder::ProjectControlStore,
            LockOrder::PublicationJournalStore,
            LockOrder::PublicationAuxiliary,
        ] {
            assert!(enter(earlier).is_err(), "{earlier:?}");
        }
    }

    #[test]
    fn the_order_is_a_total_rank_with_no_duplicates() {
        let all = [
            LockOrder::TrustReadFence,
            LockOrder::ProjectControlLease,
            LockOrder::PublicationLease,
            LockOrder::ProjectControlStore,
            LockOrder::ProviderBindingStore,
            LockOrder::PublicationJournalStore,
            LockOrder::PublicationControlStore,
            LockOrder::DomainRecordStore,
            LockOrder::PublicationAuxiliary,
            LockOrder::ActivityLedger,
        ];
        let ranks: Vec<u8> = all.iter().map(|order| order.rank()).collect();
        assert_eq!(ranks, (1..=10).collect::<Vec<u8>>());
    }
}
