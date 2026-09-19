//! `ChangePack` — a unit of proposed work, and its lifecycle.
//!
//! ```text
//! ChangePackLifecycle = Active | Completed | Abandoned
//! ```
//!
//! | State | New revisions? | Promotion? | Entered by |
//! |---|---|---|---|
//! | `Active` | yes | yes | creation, or reopen |
//! | `Completed` | no | no | Promotion finalization |
//! | `Abandoned` | no | no | an explicit decision to stop |
//!
//! # Neither terminal state deletes anything
//!
//! Abandoning a ChangePack is a statement about the *future*: no more revisions, no
//! promotion. It says nothing about the past, so every definition, revision,
//! piece of evidence and decision stays exactly where it was.
//!
//! This is why there is no `draft pack delete`. Deletion would destroy the
//! record of work that was genuinely done and genuinely decided against — and
//! "we tried this and stopped" is frequently the most useful thing in a
//! project's history. Reopening is available precisely because the history was
//! never thrown away.
//!
//! # Completion is not a decision
//!
//! `Completed` is reached only by Promotion's deterministic finalization, under
//! the same lock as the Baseline commit. It is not something a person sets, and
//! not a second commit point: a ChangePack is complete because its work was
//! promoted, so any other route into that state would let the two disagree.

use draft_dcg_contract::ids::{ChangePackId, ProjectId};
use draft_dcg_contract::Digest;
use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::lock_order::LockOrder;
use crate::support::record_guard::{
    ExpectedRecordState, RecordGuard, RevisionedRecord, RevisionedRecordStore, DEFAULT_LOCK_TIMEOUT,
};
use crate::support::telemetry::Counter;

/// Whether a ChangePack still accepts work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangePackLifecycle {
    Active,
    /// Its work was promoted. Set only by Promotion's finalization.
    Completed,
    /// Stopped deliberately. History is retained in full.
    Abandoned,
}

impl ChangePackLifecycle {
    /// Whether new revisions may be sealed.
    pub fn accepts_work(self) -> bool {
        self == Self::Active
    }

    /// Whether this state may be reopened.
    ///
    /// An abandoned ChangePack may resume. A completed one may not: its work is
    /// already in an accepted Baseline, and reopening would make the ChangePack and
    /// the history it produced disagree about whether it is finished.
    pub fn is_reopenable(self) -> bool {
        self == Self::Abandoned
    }
}

/// A unit of proposed work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangePack {
    pub generation: u64,
    pub id: ChangePackId,
    pub project: ProjectId,
    /// The exact definition currently in force.
    pub current_definition: Digest,
    pub lifecycle: ChangePackLifecycle,
}

impl RevisionedRecord for ChangePack {
    fn generation(&self) -> u64 {
        self.generation
    }
}

impl ChangePack {
    /// This ChangePack one generation on, with `mutate` applied.
    pub fn advanced(&self, mutate: impl FnOnce(&mut Self)) -> Self {
        let mut next = self.clone();
        next.generation += 1;
        mutate(&mut next);
        next
    }

    /// Move to `lifecycle`, refusing transitions that are not defined.
    pub fn transition(&self, lifecycle: ChangePackLifecycle) -> DraftResult<Self> {
        // The complete set of defined transitions. Anything absent is refused
        // rather than assumed harmless: a Completed change reopening, or an
        // Abandoned one completing, would each make the ChangePack disagree with
        // the history it produced.
        let permitted = matches!(
            (self.lifecycle, lifecycle),
            (ChangePackLifecycle::Active, ChangePackLifecycle::Completed)
                | (ChangePackLifecycle::Active, ChangePackLifecycle::Abandoned)
                | (ChangePackLifecycle::Abandoned, ChangePackLifecycle::Active)
        );
        if !permitted {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "a ChangePack cannot move from {:?} to {lifecycle:?}",
                    self.lifecycle
                ),
            ));
        }
        Ok(self.advanced(|next| next.lifecycle = lifecycle))
    }

    /// Amend which definition is in force.
    ///
    /// Only while Active: amending a finished ChangePack would change what its
    /// already-promoted or already-abandoned work claimed to be.
    pub fn amend_definition(&self, definition: Digest) -> DraftResult<Self> {
        if !self.lifecycle.accepts_work() {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "ChangePack '{}' is {:?} and no longer accepts amendments",
                    self.id, self.lifecycle
                ),
            ));
        }
        Ok(self.advanced(|next| next.current_definition = definition))
    }
}

/// The project's ChangePacks.
#[derive(Debug, Clone)]
pub struct ChangePackStore {
    records: RevisionedRecordStore<ChangePack>,
}

/// A live, exclusive hold on one ChangePack.
pub type ChangePackGuard<'a> = RecordGuard<'a, ChangePack>;

impl ChangePackStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            records: RevisionedRecordStore::new(directory)
                .with_order(LockOrder::DomainRecordStore)
                .counting_conflicts_as(Counter::ChangePackDefinitionCasConflicts),
        }
    }

    pub fn lock_path(&self, id: &ChangePackId) -> std::path::PathBuf {
        self.records.lock_path(id.as_str())
    }

    pub fn read_unlocked(&self, id: &ChangePackId) -> DraftResult<Option<ChangePack>> {
        self.records.read_unlocked(id.as_str())
    }

    /// Run `body` with the ChangePack's lock held for exactly one acquisition.
    ///
    /// Promotion holds this across its Baseline commit, so `Active → Completed`
    /// is part of the same critical section rather than a follow-up that could
    /// fail on its own.
    pub fn with_locked_record<R>(
        &self,
        id: &ChangePackId,
        body: impl FnOnce(&mut ChangePackGuard<'_>) -> DraftResult<R>,
    ) -> DraftResult<R> {
        self.records
            .with_locked_record(id.as_str(), DEFAULT_LOCK_TIMEOUT, body)
    }

    /// Create a ChangePack.
    pub fn create(&self, change: &ChangePack) -> DraftResult<()> {
        self.records
            .compare_exchange(change.id.as_str(), &ExpectedRecordState::Absent, change)
    }

    /// The record store, for callers that commit through the audited path.
    ///
    /// Exposed so the layer that owns the Activity ledger can hand this Store
    /// to the audited-mutation protocol. `dcg` deliberately does not name that
    /// layer: it sits above this one, and a ChangePack store that reached up to
    /// append its own events would invert the dependency the whole module tree
    /// is arranged to keep pointing one way.
    /// Every ChangePack this project holds.
    pub fn list(&self) -> DraftResult<Vec<ChangePack>> {
        let mut found = Vec::new();
        for key in self.records.keys()? {
            if let Ok(id) = ChangePackId::parse(&key) {
                if let Some(change) = self.read_unlocked(&id)? {
                    found.push(change);
                }
            }
        }
        Ok(found)
    }

    pub fn records(&self) -> &RevisionedRecordStore<ChangePack> {
        &self.records
    }

    /// Apply `change` to the ChangePack under its lock, against its current value.
    ///
    /// # When this is not enough
    ///
    /// This commits the record and nothing else. A lifecycle transition that
    /// must also be on the Activity record is two durable effects, and a crash
    /// between them leaves "did it commit?" unanswerable — so those go through
    /// the audited path in `app::activity`, which journals the intent first.
    /// This remains for mutations that are not themselves audited facts.
    pub fn mutate(
        &self,
        id: &ChangePackId,
        change: impl FnOnce(&ChangePack) -> DraftResult<ChangePack>,
    ) -> DraftResult<ChangePack> {
        self.with_locked_record(id, |guard| {
            let Some(current) = guard.current()? else {
                return Err(DraftError::new(
                    DraftErrorKind::NotFound,
                    format!("ChangePack '{id}' does not exist"),
                ));
            };
            let expected = ExpectedRecordState::of(&current)?;
            let next = change(&current)?;
            guard.compare_exchange_locked(&expected, &next)?;
            Ok(next)
        })
    }

    /// Complete a ChangePack whose work a promotion accepted.
    ///
    /// Idempotent on purpose. Promotion's finalization is replayed after an
    /// interruption, and a second call must converge rather than refuse — a
    /// `Completed → Completed` transition is not a defined move, but arriving
    /// at a state you were trying to reach is not a conflict.
    ///
    /// Only promotion calls this. A ChangePack becomes `Completed` because its
    /// work is in an accepted Baseline, never because somebody asked.
    pub fn complete(&self, id: &ChangePackId) -> DraftResult<ChangePack> {
        self.with_locked_record(id, |guard| {
            let Some(current) = guard.current()? else {
                return Err(DraftError::new(
                    DraftErrorKind::NotFound,
                    format!("ChangePack '{id}' does not exist"),
                ));
            };
            if current.lifecycle == ChangePackLifecycle::Completed {
                return Ok(current);
            }
            let expected = ExpectedRecordState::of(&current)?;
            let completed = current.transition(ChangePackLifecycle::Completed)?;
            guard.compare_exchange_locked(&expected, &completed)?;
            Ok(completed)
        })
    }

    pub fn abandon(&self, id: &ChangePackId) -> DraftResult<ChangePack> {
        self.mutate(id, |current| {
            current.transition(ChangePackLifecycle::Abandoned)
        })
    }

    pub fn reopen(&self, id: &ChangePackId) -> DraftResult<ChangePack> {
        self.mutate(id, |current| {
            if !current.lifecycle.is_reopenable() {
                return Err(DraftError::new(
                    DraftErrorKind::ConflictDetected,
                    format!(
                        "ChangePack '{}' is {:?} and cannot be reopened; its work is already in an \
                         accepted Baseline",
                        current.id, current.lifecycle
                    ),
                ));
            }
            current.transition(ChangePackLifecycle::Active)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change_pack_id() -> ChangePackId {
        ChangePackId::parse("cpk_000000000001").unwrap()
    }

    fn change(lifecycle: ChangePackLifecycle) -> ChangePack {
        ChangePack {
            generation: 0,
            id: change_pack_id(),
            project: ProjectId::parse("prj_000000000001").unwrap(),
            current_definition: Digest::of_bytes(b"definition-1"),
            lifecycle,
        }
    }

    fn store(directory: &tempfile::TempDir) -> ChangePackStore {
        ChangePackStore::new(directory.path())
    }

    #[test]
    fn an_active_change_accepts_work_and_a_finished_one_does_not() {
        assert!(ChangePackLifecycle::Active.accepts_work());
        assert!(!ChangePackLifecycle::Completed.accepts_work());
        assert!(!ChangePackLifecycle::Abandoned.accepts_work());
    }

    #[test]
    fn abandoning_retains_everything_and_can_be_reopened() {
        // Scenario BU. "We tried this and stopped" is frequently the most
        // useful thing in a project's history.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.create(&change(ChangePackLifecycle::Active)).unwrap();

        let abandoned = store.abandon(&change_pack_id()).unwrap();
        assert_eq!(abandoned.lifecycle, ChangePackLifecycle::Abandoned);
        // The definition it was working to is untouched.
        assert_eq!(
            abandoned.current_definition,
            Digest::of_bytes(b"definition-1")
        );

        let reopened = store.reopen(&change_pack_id()).unwrap();
        assert!(reopened.lifecycle.accepts_work());
        assert_eq!(reopened.generation, 2);
    }

    #[test]
    fn a_completed_change_cannot_be_reopened() {
        // Its work is already in an accepted Baseline; reopening would make the
        // ChangePack and the history it produced disagree about whether it is done.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.create(&change(ChangePackLifecycle::Active)).unwrap();
        store
            .mutate(&change_pack_id(), |current| {
                current.transition(ChangePackLifecycle::Completed)
            })
            .unwrap();

        let error = store.reopen(&change_pack_id()).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
        assert!(
            error.message.contains("accepted Baseline"),
            "{}",
            error.message
        );
    }

    #[test]
    fn undefined_transitions_are_refused() {
        assert!(change(ChangePackLifecycle::Completed)
            .transition(ChangePackLifecycle::Abandoned)
            .is_err());
        assert!(change(ChangePackLifecycle::Abandoned)
            .transition(ChangePackLifecycle::Completed)
            .is_err());
        assert!(change(ChangePackLifecycle::Active)
            .transition(ChangePackLifecycle::Active)
            .is_err());
    }

    #[test]
    fn a_finished_change_refuses_an_amendment() {
        // Amending would change what already-promoted or already-abandoned work
        // claimed to be.
        for finished in [
            ChangePackLifecycle::Completed,
            ChangePackLifecycle::Abandoned,
        ] {
            assert!(change(finished)
                .amend_definition(Digest::of_bytes(b"definition-2"))
                .is_err());
        }
        change(ChangePackLifecycle::Active)
            .amend_definition(Digest::of_bytes(b"definition-2"))
            .unwrap();
    }

    #[test]
    fn a_stale_amendment_loses_to_the_committed_one() {
        // Scenario BC. Two amendments racing: the loser must be rejected rather
        // than overwriting the definition somebody else committed.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.create(&change(ChangePackLifecycle::Active)).unwrap();

        let initial = change(ChangePackLifecycle::Active);
        let expected = ExpectedRecordState::of(&initial).unwrap();
        let winner = initial
            .amend_definition(Digest::of_bytes(b"winner"))
            .unwrap();
        store
            .with_locked_record(&change_pack_id(), |guard| {
                guard.compare_exchange_locked(&expected, &winner)
            })
            .unwrap();

        let loser = initial
            .amend_definition(Digest::of_bytes(b"loser"))
            .unwrap();
        let error = store
            .with_locked_record(&change_pack_id(), |guard| {
                guard.compare_exchange_locked(&expected, &loser)
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
        assert_eq!(
            store
                .read_unlocked(&change_pack_id())
                .unwrap()
                .unwrap()
                .current_definition,
            Digest::of_bytes(b"winner")
        );
    }

    #[test]
    fn completion_happens_inside_one_acquisition() {
        // Promotion holds this lock across its Baseline commit, so completion
        // is part of that critical section rather than a follow-up that could
        // fail on its own.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.create(&change(ChangePackLifecycle::Active)).unwrap();

        store
            .with_locked_record(&change_pack_id(), |guard| {
                let current = guard.current()?.unwrap();
                assert!(current.lifecycle.accepts_work());
                let expected = ExpectedRecordState::of(&current)?;
                let completed = current.transition(ChangePackLifecycle::Completed)?;
                guard.compare_exchange_locked(&expected, &completed)
            })
            .unwrap();

        assert_eq!(
            store
                .read_unlocked(&change_pack_id())
                .unwrap()
                .unwrap()
                .lifecycle,
            ChangePackLifecycle::Completed
        );
    }

    #[test]
    fn the_change_lock_is_a_domain_record_lock() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.create(&change(ChangePackLifecycle::Active)).unwrap();
        store
            .with_locked_record(&change_pack_id(), |_guard| {
                assert_eq!(
                    crate::support::lock_order::currently_held(),
                    vec![LockOrder::DomainRecordStore]
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn mutating_an_absent_change_is_reported_rather_than_creating_one() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            store(&directory)
                .abandon(&change_pack_id())
                .unwrap_err()
                .kind,
            DraftErrorKind::NotFound
        );
    }
}
