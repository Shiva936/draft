//! What each delivery class lets recovery conclude about an unseen effect.
//!
//! A crash after dispatch leaves one question: did the external mutation
//! happen? Draft cannot answer it by looking at itself. What it *can* do is
//! read what the delivery semantics guarantee, which is why those semantics
//! are canonical rather than operational — they decide whether a retry is safe
//! or whether retrying would duplicate a real-world effect.
//!
//! ```text
//! IdempotentByKey       re-sending under the same key cannot duplicate
//! ReconcileByClientKey  the external system can reconcile by our key
//! QueryByClientKey      the external system can be asked what happened
//! NonIdempotent         re-sending may duplicate. Never automatic.
//! ```
//!
//! # Why two of these still need the external system
//!
//! `ReconcileByClientKey` and `QueryByClientKey` can establish the truth — but
//! only by asking. That makes them the classes that produce
//! `PendingExternalResolution`: the bookkeeping barrier must never wait on
//! remote availability, so it releases everything and reports the Publication
//! as pending rather than blocking on a system that may be down.
//!
//! # Why `NonIdempotent` resolves locally and still forbids a retry
//!
//! These look contradictory and are not. Draft can honestly record
//! `Indeterminate` without asking anyone — "we do not know" is a true
//! statement derived from local facts alone, and recording it closes the
//! attempt so unrelated local work proceeds.
//!
//! What it must not do is *retry*. Not knowing whether an effect occurred is
//! exactly the situation in which repeating it might duplicate it. So the
//! attempt closes locally and a new one requires explicit retry authority
//! carrying an acknowledged duplicate risk.
//!
//! Collapsing "can close this attempt" into "can start another" is the mistake
//! that turns a recorded uncertainty into a duplicated external effect.

use draft_dcg_contract::publication::DeliverySemantics;

use crate::publication::restart::DispatchRecoveryClass;

/// Whether another attempt may be made without explicit retry authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryPermission {
    /// A new attempt is safe on the delivery semantics alone.
    SafeToRetry,
    /// A new attempt needs an explicit authorization acknowledging that it may
    /// duplicate a real-world effect.
    RequiresExplicitAuthority,
}

/// How an unresolved dispatch may be classified under these semantics.
///
/// This is the only place the mapping lives. Reading it off the enum at each
/// call site would let one site quietly disagree with another about whether a
/// class needs the external system — and the two answers differ by a
/// duplicated external effect.
pub fn recovery_class(semantics: DeliverySemantics) -> DispatchRecoveryClass {
    match semantics {
        // Re-sending is safe, so Draft can proceed without asking anyone.
        DeliverySemantics::IdempotentByKey => DispatchRecoveryClass::ResolvableLocally,
        // The truth is knowable, but only by asking. The barrier must not wait
        // on that, so it hands back a pending Publication instead.
        DeliverySemantics::ReconcileByClientKey | DeliverySemantics::QueryByClientKey => {
            DispatchRecoveryClass::NeedsExternalResolution
        }
        // Nothing can be asked and nothing may be repeated. Draft records what
        // it honestly knows — that it does not know — and closes the attempt.
        DeliverySemantics::NonIdempotent => DispatchRecoveryClass::ResolvableLocally,
    }
}

/// Whether a *new* attempt may follow, once the previous one is closed.
///
/// Deliberately a separate question from [`recovery_class`]. Closing an
/// attempt locally and being allowed to make another are different
/// permissions, and `NonIdempotent` is exactly the class where the first is
/// granted and the second is not.
pub fn retry_permission(semantics: DeliverySemantics) -> RetryPermission {
    match semantics {
        DeliverySemantics::IdempotentByKey => RetryPermission::SafeToRetry,
        DeliverySemantics::ReconcileByClientKey
        | DeliverySemantics::QueryByClientKey
        | DeliverySemantics::NonIdempotent => RetryPermission::RequiresExplicitAuthority,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_idempotent_delivery_may_retry_on_the_semantics_alone() {
        assert_eq!(
            retry_permission(DeliverySemantics::IdempotentByKey),
            RetryPermission::SafeToRetry
        );
        for semantics in [
            DeliverySemantics::ReconcileByClientKey,
            DeliverySemantics::QueryByClientKey,
            DeliverySemantics::NonIdempotent,
        ] {
            assert_eq!(
                retry_permission(semantics),
                RetryPermission::RequiresExplicitAuthority,
                "{semantics:?} must not retry without acknowledged duplicate risk"
            );
        }
    }

    #[test]
    fn the_classes_that_can_only_learn_by_asking_never_block_the_barrier() {
        for semantics in [
            DeliverySemantics::ReconcileByClientKey,
            DeliverySemantics::QueryByClientKey,
        ] {
            assert_eq!(
                recovery_class(semantics),
                DispatchRecoveryClass::NeedsExternalResolution,
                "{semantics:?} establishes the truth by asking, which the barrier must not wait on"
            );
        }
    }

    #[test]
    fn non_idempotent_closes_locally_yet_still_forbids_an_automatic_retry() {
        // The pair that looks contradictory: Draft can record "we do not know"
        // from local facts alone, but must not repeat an effect it cannot rule
        // out having caused.
        assert_eq!(
            recovery_class(DeliverySemantics::NonIdempotent),
            DispatchRecoveryClass::ResolvableLocally
        );
        assert_eq!(
            retry_permission(DeliverySemantics::NonIdempotent),
            RetryPermission::RequiresExplicitAuthority
        );
    }

    #[test]
    fn idempotent_delivery_both_resolves_and_retries_without_asking() {
        assert_eq!(
            recovery_class(DeliverySemantics::IdempotentByKey),
            DispatchRecoveryClass::ResolvableLocally
        );
        assert_eq!(
            retry_permission(DeliverySemantics::IdempotentByKey),
            RetryPermission::SafeToRetry
        );
    }
}
