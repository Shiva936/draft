//! `ProjectControlState` — the single authoritative pointer to what a project
//! currently accepts.
//!
//! One record, mutated only through a compare-exchange inside one acquisition
//! of the project's correctness lock. Everything a decision must be taken
//! against lives here together, and that is deliberate: if the accepted
//! Baseline, the policy in force and the security state could each move
//! independently, a Promotion could validate against three different moments
//! and commit as though they were one.
//!
//! ```text
//! ProjectControlState {
//!     generation, project, accepted_baseline, current_policy_digest,
//!     project_security_state, project_lifecycle
//! }
//! ```
//!
//! # There is no separate security generation
//!
//! The record's own `generation` covers every field, so "the security state
//! changed" and "the control state changed" are the same event. A second
//! counter would let a caller observe a matching security generation while the
//! record it belongs to had already moved, which is the ambiguity the single
//! generation removes.
//!
//! # Store serialization is correctness; the lease is coordination
//!
//! A caller that ignores the product lease entirely is still serialized here
//! and still fails its compare-exchange if it is stale. The lease exists to
//! stop work being wasted and to scope ownership and audit — not to protect
//! this record.

use serde::{Deserialize, Serialize};

use draft_dcg_contract::ids::ProjectId;
use draft_dcg_contract::{BaselineId, PolicyDigest, ProjectSecurityStateDigest};

use crate::support::error::DraftResult;
use crate::support::lock_order::LockOrder;
use crate::support::record_guard::{
    ExpectedRecordState, RecordGuard, RevisionedRecord, RevisionedRecordStore, DEFAULT_LOCK_TIMEOUT,
};
use crate::support::telemetry::Counter;

/// The key the single control record is stored under.
pub const CONTROL_RECORD_KEY: &str = "control";

/// Whether a project accepts new work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectLifecycle {
    /// Ordinary operation.
    Active,
    /// Closed to new work. History remains readable and verifiable — closing a
    /// project is a lifecycle transition, never a deletion.
    Closed,
}

/// What a project currently accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectControlState {
    /// Advances on every mutation, covering every field below.
    pub generation: u64,
    pub project: ProjectId,
    /// The Baseline this project has accepted.
    pub accepted_baseline: BaselineId,
    /// The policy in force.
    pub current_policy_digest: PolicyDigest,
    /// The security state in force.
    pub project_security_state: ProjectSecurityStateDigest,
    pub project_lifecycle: ProjectLifecycle,
}

impl RevisionedRecord for ProjectControlState {
    fn generation(&self) -> u64 {
        self.generation
    }
}

impl ProjectControlState {
    /// Whether the project currently accepts new work.
    pub fn is_active(&self) -> bool {
        self.project_lifecycle == ProjectLifecycle::Active
    }

    /// The same state one generation on, with `mutate` applied.
    ///
    /// The only way to build a replacement, so a caller cannot forget to
    /// advance the generation and quietly produce a value that compares equal
    /// to a different one.
    pub fn advanced(&self, mutate: impl FnOnce(&mut Self)) -> Self {
        let mut next = self.clone();
        next.generation += 1;
        mutate(&mut next);
        next
    }
}

/// The project's control record.
#[derive(Debug, Clone)]
pub struct ProjectControlStore {
    records: RevisionedRecordStore<ProjectControlState>,
}

/// A live, exclusive hold on the control record.
pub type ProjectControlGuard<'a> = RecordGuard<'a, ProjectControlState>;

impl ProjectControlStore {
    /// Open the store over the project's directory.
    pub fn new(project_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            records: RevisionedRecordStore::new(project_dir)
                .with_order(LockOrder::ProjectControlStore)
                .counting_conflicts_as(Counter::ProjectControlCasConflicts),
        }
    }

    /// The stable sidecar the control lock is held on.
    pub fn lock_path(&self) -> std::path::PathBuf {
        self.records.lock_path(CONTROL_RECORD_KEY)
    }

    /// Read the control state without locking.
    ///
    /// For display and read models. A value read this way must never be used
    /// as the expected state of a mutation.
    pub fn read_unlocked(&self) -> DraftResult<Option<ProjectControlState>> {
        self.records.read_unlocked(CONTROL_RECORD_KEY)
    }

    /// Run `body` with the control record held for exactly one acquisition.
    ///
    /// This is how Promotion commits: the guard stays live across re-reading
    /// registry revisions, re-resolving security facts and verifying gates, so
    /// nothing it validated can move before the compare-exchange lands.
    pub fn with_locked_control<R>(
        &self,
        body: impl FnOnce(&mut ProjectControlGuard<'_>) -> DraftResult<R>,
    ) -> DraftResult<R> {
        self.records
            .with_locked_record(CONTROL_RECORD_KEY, DEFAULT_LOCK_TIMEOUT, body)
    }

    /// Compare and exchange, acquiring the lock itself.
    ///
    /// For a caller holding nothing yet. A caller already inside the critical
    /// section uses the guard's `compare_exchange_locked`, because the lock is
    /// not reentrant.
    pub fn compare_exchange(
        &self,
        expected: &ExpectedRecordState,
        replacement: &ProjectControlState,
    ) -> DraftResult<()> {
        self.records
            .compare_exchange(CONTROL_RECORD_KEY, expected, replacement)
    }

    /// Create the initial control record.
    ///
    /// Uses the ordinary transaction against `Absent`, so initialization is not
    /// a special path with its own failure modes.
    pub fn initialize(&self, initial: &ProjectControlState) -> DraftResult<()> {
        self.compare_exchange(&ExpectedRecordState::Absent, initial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::error::DraftErrorKind;
    use draft_dcg_contract::Digest;

    fn state(generation: u64, lifecycle: ProjectLifecycle) -> ProjectControlState {
        ProjectControlState {
            generation,
            project: ProjectId::parse("prj_000000000001").unwrap(),
            accepted_baseline: BaselineId::new(Digest::of_bytes(b"baseline-1")),
            current_policy_digest: PolicyDigest::new(Digest::of_bytes(b"policy-1")),
            project_security_state: ProjectSecurityStateDigest::new(Digest::of_bytes(
                b"security-1",
            )),
            project_lifecycle: lifecycle,
        }
    }

    fn store(directory: &tempfile::TempDir) -> ProjectControlStore {
        ProjectControlStore::new(directory.path())
    }

    #[test]
    fn initialization_uses_the_ordinary_transaction() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store
            .initialize(&state(0, ProjectLifecycle::Active))
            .unwrap();
        assert_eq!(
            store.read_unlocked().unwrap().unwrap(),
            state(0, ProjectLifecycle::Active)
        );

        // Initializing twice conflicts, because the record now exists.
        let error = store
            .initialize(&state(0, ProjectLifecycle::Active))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    }

    #[test]
    fn a_stale_promoter_cannot_overwrite_a_committed_baseline() {
        // Two promotions racing. The loser must be rejected rather than
        // silently replacing the accepted Baseline.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let initial = state(0, ProjectLifecycle::Active);
        store.initialize(&initial).unwrap();
        let expected = ExpectedRecordState::of(&initial).unwrap();

        let winner = initial.advanced(|next| {
            next.accepted_baseline = BaselineId::new(Digest::of_bytes(b"baseline-winner"));
        });
        store.compare_exchange(&expected, &winner).unwrap();

        let loser = initial.advanced(|next| {
            next.accepted_baseline = BaselineId::new(Digest::of_bytes(b"baseline-loser"));
        });
        let error = store.compare_exchange(&expected, &loser).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
        assert_eq!(
            store.read_unlocked().unwrap().unwrap().accepted_baseline,
            BaselineId::new(Digest::of_bytes(b"baseline-winner")),
            "the accepted Baseline is never rolled back"
        );
    }

    #[test]
    fn one_generation_covers_every_field() {
        // Security, policy and Baseline move together under a single counter,
        // so no caller can observe one as current while another has moved.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let initial = state(0, ProjectLifecycle::Active);
        store.initialize(&initial).unwrap();
        let expected = ExpectedRecordState::of(&initial).unwrap();

        let security_only = initial.advanced(|next| {
            next.project_security_state =
                ProjectSecurityStateDigest::new(Digest::of_bytes(b"security-2"));
        });
        store.compare_exchange(&expected, &security_only).unwrap();

        // A caller holding the pre-change expectation now fails, even though
        // it only cared about policy.
        let policy_only = initial.advanced(|next| {
            next.current_policy_digest = PolicyDigest::new(Digest::of_bytes(b"policy-2"));
        });
        assert!(store.compare_exchange(&expected, &policy_only).is_err());
    }

    #[test]
    fn a_promotion_shaped_commit_holds_one_acquisition_throughout() {
        // The shape Promotion needs: validate everything and commit without
        // ever releasing, so nothing validated can move before the write.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store
            .initialize(&state(0, ProjectLifecycle::Active))
            .unwrap();

        store
            .with_locked_control(|guard| {
                let current = guard.current()?.expect("the control record exists");
                assert!(current.is_active());

                // Stand-ins for the checks Promotion performs under this guard.
                assert_eq!(
                    current.current_policy_digest,
                    PolicyDigest::new(Digest::of_bytes(b"policy-1"))
                );

                let expected = ExpectedRecordState::of(&current)?;
                let promoted = current.advanced(|next| {
                    next.accepted_baseline = BaselineId::new(Digest::of_bytes(b"baseline-2"));
                });
                guard.compare_exchange_locked(&expected, &promoted)
            })
            .unwrap();

        assert_eq!(
            store.read_unlocked().unwrap().unwrap().accepted_baseline,
            BaselineId::new(Digest::of_bytes(b"baseline-2"))
        );
    }

    #[test]
    fn closing_a_project_is_a_transition_that_deletes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let initial = state(0, ProjectLifecycle::Active);
        store.initialize(&initial).unwrap();

        let closed = initial.advanced(|next| next.project_lifecycle = ProjectLifecycle::Closed);
        store
            .compare_exchange(&ExpectedRecordState::of(&initial).unwrap(), &closed)
            .unwrap();

        let current = store.read_unlocked().unwrap().unwrap();
        assert!(!current.is_active());
        // The accepted Baseline and its lineage are untouched.
        assert_eq!(current.accepted_baseline, initial.accepted_baseline);
    }

    #[test]
    fn advancing_always_moves_the_generation_by_one() {
        let initial = state(7, ProjectLifecycle::Active);
        assert_eq!(initial.advanced(|_| {}).generation, 8);
    }

    #[test]
    fn holding_the_control_record_refuses_a_reverse_acquisition() {
        // The check runs through the real lock, not just the order registry:
        // holding order 4, a higher order is fine and anything below is refused
        // before the lock is even attempted.
        //
        // The drain that actually reaches the ledger is exercised in
        // `app::activity`, the layer permitted to see both. `project`
        // deliberately cannot, which is why this asserts the ordering rather
        // than performing the append.
        use crate::support::lock_order::{self, LockOrder};

        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store
            .initialize(&state(0, ProjectLifecycle::Active))
            .unwrap();

        store
            .with_locked_control(|_guard| {
                assert_eq!(
                    lock_order::currently_held(),
                    vec![LockOrder::ProjectControlStore]
                );

                // Order 10 from order 4: where the audit drain sits.
                drop(lock_order::enter(LockOrder::ActivityLedger)?);

                // Order 1 from order 4 would be a reverse acquisition.
                let error = lock_order::enter(LockOrder::TrustReadFence).unwrap_err();
                assert!(
                    error.message.contains("reverse lock acquisition"),
                    "{}",
                    error.message
                );
                Ok(())
            })
            .unwrap();

        // And the held set is empty again once the guard is dropped.
        assert!(lock_order::currently_held().is_empty());
    }

    #[test]
    fn the_control_lock_is_a_stable_sidecar() {
        let directory = tempfile::tempdir().unwrap();
        assert!(store(&directory).lock_path().ends_with("control.lock"));
    }

    #[test]
    fn concurrent_promoters_serialize_and_none_is_lost() {
        let directory = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(store(&directory));
        store
            .initialize(&state(0, ProjectLifecycle::Active))
            .unwrap();

        let mut handles = Vec::new();
        for worker in 0..6 {
            let store = std::sync::Arc::clone(&store);
            handles.push(std::thread::spawn(move || {
                for _ in 0..20 {
                    let attempt = store.with_locked_control(|guard| {
                        let current = guard.current()?.expect("initialized");
                        let expected = ExpectedRecordState::of(&current)?;
                        let next = current.advanced(|next| {
                            next.accepted_baseline = BaselineId::new(Digest::of_bytes(
                                format!("baseline-{worker}").as_bytes(),
                            ));
                        });
                        guard.compare_exchange_locked(&expected, &next)
                    });
                    if attempt.is_ok() {
                        return;
                    }
                }
                panic!("worker {worker} never committed");
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        // Six commits on top of the initial state, each advancing exactly one.
        assert_eq!(store.read_unlocked().unwrap().unwrap().generation, 6);
    }
}
