//! `PublicationControl` — the correctness authority for one Publication.
//!
//! One record per `pub_`, holding everything that decides whether another
//! external attempt may begin:
//!
//! ```text
//! PublicationControl {
//!     generation, publication, in_flight_attempt,
//!     next_attempt_number, consumed_retry_authorizations
//! }
//! ```
//!
//! # Why the attempt number is not authoritative until the commit lands
//!
//! A worker picks `next_attempt_number` as a *candidate* and writes it into
//! the journal before the control mutation commits. If the commit never lands,
//! that candidate was never consumed — so a later attempt may legitimately
//! receive the same number. Only a committed allocation mutation consumes one.
//!
//! Conflating the two would make an interrupted worker burn a number, which
//! sounds harmless until recovery tries to decide *which* attempt owns `N` and
//! finds two journals claiming it. The type distinction here is what makes
//! that question answerable: [`PlannedAllocation`] holds a candidate,
//! [`PublicationControl::in_flight_attempt`] holds an authoritative one.
//!
//! Gaps in the sequence are legal and expected: a committed allocation whose
//! dispatch is later refused consumes its number and produces one.
//!
//! # Why the unconsumed-PRA check lives inside the guard
//!
//! A `PublicationRetryAuthorization` is a one-shot fact. Checking it against
//! `consumed_retry_authorizations` outside the control lock would be a read
//! that a concurrent allocation could invalidate before the caller committed —
//! the classic check-then-act, with a duplicate external effect at the end of
//! it.
//!
//! So the check is reachable only through a live [`PublicationControlGuard`],
//! in the same lock acquisition that commits the allocation. There is no
//! method that checks without holding, and no method that commits an
//! allocation the guard did not plan.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use draft_dcg_contract::ids::{PublicationAttemptId, PublicationId};
use draft_dcg_contract::publication::PublicationRetryAuthorizationDigest;

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::lock_order::LockOrder;
use crate::support::record_guard::{
    ExpectedRecordState, RecordGuard, RevisionedRecord, RevisionedRecordStore, DEFAULT_LOCK_TIMEOUT,
};
use crate::support::telemetry::Counter;

/// What a Publication currently permits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationControl {
    /// Advances on every mutation, covering every field below.
    pub generation: u64,
    pub publication: PublicationId,
    /// The one attempt currently allocated, if any.
    ///
    /// `None` is the only state in which a new attempt may be allocated. It is
    /// also, on its own, never enough to classify a crash — recovery compares
    /// whole values, because a cleared record and a record that never moved
    /// look identical through this field alone.
    pub in_flight_attempt: Option<PublicationAttemptId>,
    /// The number the next allocation would claim.
    pub next_attempt_number: u32,
    /// The retry authorizations a committed allocation has already spent.
    pub consumed_retry_authorizations: BTreeSet<PublicationRetryAuthorizationDigest>,
}

impl RevisionedRecord for PublicationControl {
    fn generation(&self) -> u64 {
        self.generation
    }
}

impl PublicationControl {
    /// The initial state of a Publication that has never been attempted.
    pub fn initial(publication: PublicationId) -> Self {
        Self {
            generation: 0,
            publication,
            in_flight_attempt: None,
            next_attempt_number: 1,
            consumed_retry_authorizations: BTreeSet::new(),
        }
    }

    /// The same state one generation on, with `mutate` applied.
    ///
    /// The only way to build a replacement, so a caller cannot forget to
    /// advance the generation and produce a value that compares equal to a
    /// different one.
    pub fn advanced(&self, mutate: impl FnOnce(&mut Self)) -> Self {
        let mut next = self.clone();
        next.generation += 1;
        mutate(&mut next);
        next
    }

    /// Whether this authorization has already been spent by a committed
    /// allocation.
    pub fn has_consumed(&self, authorization: &PublicationRetryAuthorizationDigest) -> bool {
        self.consumed_retry_authorizations.contains(authorization)
    }
}

/// A planned but uncommitted allocation.
///
/// Carries the **whole** expected and planned control values rather than the
/// fields that changed. Recovery classifies a crash around the commit by
/// comparing the current record against these two values in full, so a partial
/// snapshot here would leave exactly the states that cannot be told apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedAllocation {
    /// The attempt this allocation would make authoritative.
    pub attempt: PublicationAttemptId,
    /// The number this attempt would claim — a candidate until the commit
    /// lands, never authoritative before it.
    pub candidate_attempt_number: u32,
    /// The exact control value the allocation was planned against.
    pub expected_control: PublicationControl,
    /// The exact control value the allocation would commit.
    pub planned_control: PublicationControl,
    /// The one-shot authorization this allocation would consume, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_authorization: Option<PublicationRetryAuthorizationDigest>,
}

/// The key one Publication's control record is stored under.
fn control_key(publication: &PublicationId) -> String {
    publication.to_string()
}

/// Per-Publication control records.
#[derive(Debug, Clone)]
pub struct PublicationControlStore {
    records: RevisionedRecordStore<PublicationControl>,
}

impl PublicationControlStore {
    /// Open the store over `publication/control/`.
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            records: RevisionedRecordStore::new(directory)
                .with_order(LockOrder::PublicationControlStore)
                .counting_conflicts_as(Counter::PublicationControlCasConflicts),
        }
    }

    /// The stable sidecar this Publication's control lock is held on.
    pub fn lock_path(&self, publication: &PublicationId) -> std::path::PathBuf {
        self.records.lock_path(&control_key(publication))
    }

    /// Read without locking.
    ///
    /// For display and read models. A value read this way must never be used
    /// as the expected state of a mutation.
    pub fn read_unlocked(
        &self,
        publication: &PublicationId,
    ) -> DraftResult<Option<PublicationControl>> {
        self.records.read_unlocked(&control_key(publication))
    }

    /// Every publication control record, for read models that fold status
    /// across a project's targets.
    pub fn list(&self) -> DraftResult<Vec<PublicationControl>> {
        let mut controls = Vec::new();
        for key in self.records.keys()? {
            if let Some(control) = self.records.read_unlocked(&key)? {
                controls.push(control);
            }
        }
        controls.sort_by(|left, right| left.publication.as_str().cmp(right.publication.as_str()));
        Ok(controls)
    }

    /// Create the initial control record for a Publication.
    pub fn initialize(&self, initial: &PublicationControl) -> DraftResult<()> {
        self.records.compare_exchange(
            &control_key(&initial.publication),
            &ExpectedRecordState::Absent,
            initial,
        )
    }

    /// Run `body` with this Publication's control record held for exactly one
    /// acquisition.
    pub fn with_locked_control<R>(
        &self,
        publication: &PublicationId,
        body: impl FnOnce(&mut PublicationControlGuard<'_, '_>) -> DraftResult<R>,
    ) -> DraftResult<R> {
        self.records.with_locked_record(
            &control_key(publication),
            DEFAULT_LOCK_TIMEOUT,
            |record_guard| {
                let mut guard = PublicationControlGuard {
                    inner: record_guard,
                };
                body(&mut guard)
            },
        )
    }
}

/// A live, exclusive hold on one Publication's control record.
pub struct PublicationControlGuard<'a, 'b> {
    inner: &'b mut RecordGuard<'a, PublicationControl>,
}

impl PublicationControlGuard<'_, '_> {
    /// The authoritative current value.
    pub fn current(&self) -> DraftResult<Option<PublicationControl>> {
        self.inner.current()
    }

    /// Plan an allocation for `attempt`, spending `retry_authorization` if one
    /// is supplied.
    ///
    /// Runs the whole gate — record exists, no attempt in flight, the
    /// authorization not already consumed — inside this acquisition, and
    /// returns the exact expected and planned values the commit will use. The
    /// caller writes those into the attempt journal *before* calling
    /// [`Self::commit_allocation`], so a crash between the two is classifiable.
    ///
    /// # Why refusing a busy Publication is not a retry hint
    ///
    /// `in_flight_attempt == Some(..)` means an earlier attempt is unresolved.
    /// Allocating alongside it would be a second external effect against a
    /// Publication Draft cannot yet describe, so this refuses rather than
    /// waits: whether the earlier attempt can be resolved is the bookkeeping
    /// barrier's question, answered before allocation is reached at all.
    pub fn plan_allocation(
        &self,
        attempt: PublicationAttemptId,
        retry_authorization: Option<PublicationRetryAuthorizationDigest>,
    ) -> DraftResult<PlannedAllocation> {
        let current = self.inner.current()?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                "this Publication has no control record, so nothing can be allocated against it",
            )
        })?;

        if let Some(in_flight) = &current.in_flight_attempt {
            crate::support::telemetry::Counter::PublicationAllocationCasFailures.increment();
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "publication '{}' already has attempt '{in_flight}' in flight",
                    current.publication
                ),
            ));
        }

        // The definitive check. Everything earlier is a read-only pre-check
        // that could have gone stale; this one is inside the lock that commits.
        if let Some(authorization) = &retry_authorization {
            if current.has_consumed(authorization) {
                crate::support::telemetry::Counter::PublicationAllocationCasFailures.increment();
                return Err(DraftError::new(
                    DraftErrorKind::ConflictDetected,
                    format!(
                        "the retry authorization {authorization} was already consumed by an \
                         earlier committed allocation"
                    ),
                ));
            }
        }

        let candidate_attempt_number = current.next_attempt_number;
        let planned_control = current.advanced(|next| {
            next.in_flight_attempt = Some(attempt.clone());
            next.next_attempt_number = candidate_attempt_number.saturating_add(1);
            if let Some(authorization) = &retry_authorization {
                next.consumed_retry_authorizations
                    .insert(authorization.clone());
            }
        });

        Ok(PlannedAllocation {
            attempt,
            candidate_attempt_number,
            expected_control: current,
            planned_control,
            retry_authorization,
        })
    }

    /// Commit an allocation planned under this same guard.
    ///
    /// The allocation commit point: only after this returns is the attempt
    /// number authoritative, the retry authorization consumed, and the
    /// immutable `PublicationAttempt` allowed to be written.
    pub fn commit_allocation(&mut self, planned: &PlannedAllocation) -> DraftResult<()> {
        self.commit_exact(&planned.expected_control, &planned.planned_control)
    }

    /// Commit an exact whole-value transition.
    ///
    /// Used for the clear at the end of a completion or an abandonment, where
    /// the expected and planned values were frozen into the journal long
    /// before this runs.
    pub fn commit_exact(
        &mut self,
        expected: &PublicationControl,
        planned: &PublicationControl,
    ) -> DraftResult<()> {
        let expected_state = ExpectedRecordState::of(expected)?;
        self.inner.compare_exchange_locked(&expected_state, planned)
    }
}

/// How the current control value compares to the two the journal recorded.
///
/// The comparison is by whole value, never by inspecting `in_flight_attempt`:
/// a record that was cleared and a record that never moved differ in
/// generation and in the consumed-authorization set, and only the whole value
/// carries that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlMatch {
    /// Exactly the value the transaction expected: it did not commit.
    Expected,
    /// Exactly the value the transaction planned: it committed.
    Planned,
    /// Neither. Something else moved it.
    Neither,
}

impl ControlMatch {
    /// Classify `current` against the exact pair a journal recorded.
    ///
    /// `current` is `None` when the record is absent, which matches neither:
    /// a control record that has gone missing beneath a live journal is not a
    /// state any legal sequence produces.
    pub fn classify(
        current: Option<&PublicationControl>,
        expected: &PublicationControl,
        planned: &PublicationControl,
    ) -> Self {
        match current {
            Some(value) if value == expected => Self::Expected,
            Some(value) if value == planned => Self::Planned,
            _ => Self::Neither,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::Digest;

    fn publication() -> PublicationId {
        PublicationId::parse("pub_000000000001").unwrap()
    }

    fn attempt(suffix: &str) -> PublicationAttemptId {
        PublicationAttemptId::parse(format!("pat_{suffix}")).unwrap()
    }

    fn authorization(bytes: &[u8]) -> PublicationRetryAuthorizationDigest {
        PublicationRetryAuthorizationDigest::new(Digest::of_bytes(bytes))
    }

    fn store(directory: &tempfile::TempDir) -> PublicationControlStore {
        let store = PublicationControlStore::new(directory.path());
        store
            .initialize(&PublicationControl::initial(publication()))
            .unwrap();
        store
    }

    #[test]
    fn an_allocation_claims_the_next_number_and_marks_the_attempt_in_flight() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);

        let planned = store
            .with_locked_control(&publication(), |guard| {
                let planned = guard.plan_allocation(attempt("000000000001"), None)?;
                guard.commit_allocation(&planned)?;
                Ok(planned)
            })
            .unwrap();

        assert_eq!(planned.candidate_attempt_number, 1);
        let committed = store.read_unlocked(&publication()).unwrap().unwrap();
        assert_eq!(committed, planned.planned_control);
        assert_eq!(committed.in_flight_attempt, Some(attempt("000000000001")));
        assert_eq!(committed.next_attempt_number, 2);
    }

    #[test]
    fn a_publication_with_an_attempt_in_flight_refuses_a_second_allocation() {
        // The refusal that stops two external effects racing against one
        // Publication. Whether the in-flight attempt can be resolved is the
        // barrier's question, asked before allocation is reached.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store
            .with_locked_control(&publication(), |guard| {
                let planned = guard.plan_allocation(attempt("000000000001"), None)?;
                guard.commit_allocation(&planned)
            })
            .unwrap();

        let error = store
            .with_locked_control(&publication(), |guard| {
                guard.plan_allocation(attempt("000000000002"), None)
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    }

    #[test]
    fn a_retry_authorization_is_consumed_exactly_once() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let one_shot = authorization(b"pra-1");

        // Allocate, then clear, so the Publication is idle again and the only
        // thing standing between the second allocation and success is the
        // consumed-authorization set.
        let first = store
            .with_locked_control(&publication(), |guard| {
                let planned =
                    guard.plan_allocation(attempt("000000000001"), Some(one_shot.clone()))?;
                guard.commit_allocation(&planned)?;
                Ok(planned)
            })
            .unwrap();

        let cleared = first.planned_control.advanced(|next| {
            next.in_flight_attempt = None;
        });
        store
            .with_locked_control(&publication(), |guard| {
                guard.commit_exact(&first.planned_control, &cleared)
            })
            .unwrap();

        let error = store
            .with_locked_control(&publication(), |guard| {
                guard.plan_allocation(attempt("000000000002"), Some(one_shot.clone()))
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);

        // A different authorization is still spendable, so the refusal is
        // about this one-shot fact and not about retries in general.
        store
            .with_locked_control(&publication(), |guard| {
                let planned = guard
                    .plan_allocation(attempt("000000000002"), Some(authorization(b"pra-2")))?;
                guard.commit_allocation(&planned)
            })
            .unwrap();
    }

    #[test]
    fn an_uncommitted_candidate_number_is_not_consumed() {
        // pat_A planned N and never committed. pat_B legitimately receives N:
        // only a committed allocation mutation spends a number.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);

        let abandoned = store
            .with_locked_control(&publication(), |guard| {
                guard.plan_allocation(attempt("00000000000a"), None)
            })
            .unwrap();
        assert_eq!(abandoned.candidate_attempt_number, 1);

        let committed = store
            .with_locked_control(&publication(), |guard| {
                let planned = guard.plan_allocation(attempt("00000000000b"), None)?;
                guard.commit_allocation(&planned)?;
                Ok(planned)
            })
            .unwrap();
        assert_eq!(
            committed.candidate_attempt_number, 1,
            "an uncommitted candidate must not have burned the number"
        );
    }

    #[test]
    fn a_committed_allocation_consumes_its_number_even_if_dispatch_is_later_refused() {
        // The legal gap: pat_A commits N, dispatch is refused, the control is
        // cleared. pat_B gets N+1 and N is never reused.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);

        let first = store
            .with_locked_control(&publication(), |guard| {
                let planned = guard.plan_allocation(attempt("00000000000a"), None)?;
                guard.commit_allocation(&planned)?;
                Ok(planned)
            })
            .unwrap();
        let cleared = first.planned_control.advanced(|next| {
            next.in_flight_attempt = None;
        });
        store
            .with_locked_control(&publication(), |guard| {
                guard.commit_exact(&first.planned_control, &cleared)
            })
            .unwrap();

        let second = store
            .with_locked_control(&publication(), |guard| {
                guard.plan_allocation(attempt("00000000000b"), None)
            })
            .unwrap();
        assert_eq!(second.candidate_attempt_number, 2);
    }

    #[test]
    fn a_stale_expected_value_cannot_commit() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);

        let stale = store
            .with_locked_control(&publication(), |guard| {
                guard.plan_allocation(attempt("00000000000a"), None)
            })
            .unwrap();

        // Somebody else moved the record between the plan and the commit.
        store
            .with_locked_control(&publication(), |guard| {
                let planned = guard.plan_allocation(attempt("00000000000b"), None)?;
                guard.commit_allocation(&planned)
            })
            .unwrap();

        let error = store
            .with_locked_control(&publication(), |guard| guard.commit_allocation(&stale))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    }

    #[test]
    fn control_is_classified_by_whole_value_not_by_the_in_flight_field() {
        // A record that never moved and a record that was cleared both have
        // `in_flight_attempt == None`. Only the whole value separates them,
        // which is why recovery never reads that field alone.
        let expected = PublicationControl::initial(publication());
        let planned = expected.advanced(|next| {
            next.in_flight_attempt = Some(attempt("000000000001"));
            next.next_attempt_number = 2;
        });
        let cleared = planned.advanced(|next| {
            next.in_flight_attempt = None;
        });

        assert_eq!(
            ControlMatch::classify(Some(&expected), &expected, &planned),
            ControlMatch::Expected
        );
        assert_eq!(
            ControlMatch::classify(Some(&planned), &expected, &planned),
            ControlMatch::Planned
        );
        assert_eq!(
            ControlMatch::classify(Some(&cleared), &expected, &planned),
            ControlMatch::Neither,
            "a cleared record must not read as the record that never moved"
        );
        assert_eq!(
            ControlMatch::classify(None, &expected, &planned),
            ControlMatch::Neither
        );
    }
}
