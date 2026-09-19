//! The guarded promotion journal.
//!
//! `PromotionJournal` says what a promotion intends to do. This is where that
//! intent becomes durable, and the only way its state moves.
//!
//! # Why the journal needs a lock at all
//!
//! Several local actors legitimately touch one promotion: the worker that
//! started it, and recovery on the next command after a crash. Without
//! serialization both could read `Prepared`, both could conclude the commit
//! had not happened, and both could commit it — accepting the same work into
//! two different Baselines.
//!
//! So every read that decides anything, and every transition, happens under
//! this lock, and a stale expected state is refused rather than overwritten.
//!
//! # Why the transition set is closed
//!
//! ```text
//! Prepared → Committed → Finalized
//! ```
//!
//! and nothing else. A promotion cannot go back to `Prepared` after
//! committing, because the project has already accepted the Baseline; and it
//! cannot skip from `Prepared` to `Finalized`, because that would claim
//! finalization of a commit that never happened. Both are refused by the
//! transition set rather than by a rule somebody has to remember.

use serde::{Deserialize, Serialize};

use draft_dcg_contract::ids::PromotionId;

use crate::promotion::journal::PromotionJournalState;
use crate::promotion::record::PromotionJournal;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::lock_order::LockOrder;
use crate::support::record_guard::{
    ExpectedRecordState, RecordGuard, RevisionedRecord, RevisionedRecordStore, DEFAULT_LOCK_TIMEOUT,
};

/// One promotion's journal, as a revisioned record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionJournalRecord {
    /// Advances on every transition, covering every field below.
    pub generation: u64,
    pub journal: PromotionJournal,
}

impl RevisionedRecord for PromotionJournalRecord {
    fn generation(&self) -> u64 {
        self.generation
    }
}

impl PromotionJournalRecord {
    pub fn state(&self) -> PromotionJournalState {
        self.journal.state
    }
}

/// Whether `next` is a legal successor of `current`.
fn permits(current: PromotionJournalState, next: PromotionJournalState) -> bool {
    use PromotionJournalState as S;
    matches!(
        (current, next),
        (S::Prepared, S::Committed) | (S::Committed, S::Finalized)
    )
}

/// Per-promotion journals, on `promotions/journal/<pro_>.lock`.
#[derive(Debug, Clone)]
pub struct PromotionJournalStore {
    records: RevisionedRecordStore<PromotionJournalRecord>,
}

impl PromotionJournalStore {
    /// Open the store over `promotions/journal/`.
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            records: RevisionedRecordStore::new(directory)
                // The promotion journal is a per-record domain store: it sits
                // at the same rank as the ChangePack lock it will later complete,
                // and above the project control record it commits against.
                .with_order(LockOrder::DomainRecordStore),
        }
    }

    /// Read without locking.
    ///
    /// For display only. Every read that decides anything goes through
    /// [`Self::with_locked`], because a decision made from an unlocked read
    /// can be invalidated before it is acted on.
    pub fn read_unlocked(
        &self,
        promotion: &PromotionId,
    ) -> DraftResult<Option<PromotionJournalRecord>> {
        self.records.read_unlocked(promotion.as_str())
    }

    /// Every promotion that has a journal.
    ///
    /// Reads the record directory rather than a separate index: an index would
    /// be a second place the set of promotions lives, and a promotion missing
    /// from it would be invisible to the barrier that must not miss one.
    pub fn list(&self) -> DraftResult<Vec<PromotionId>> {
        let directory = self.records.record_path("x");
        let Some(parent) = directory.parent() else {
            return Ok(Vec::new());
        };
        let entries = match std::fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(DraftError::storage(format!(
                    "cannot list promotion journals in {}: {error}",
                    parent.display()
                )))
            }
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                    if let Ok(promotion) = PromotionId::parse(stem) {
                        found.push(promotion);
                    }
                }
            }
        }
        found.sort();
        Ok(found)
    }

    /// Run `body` with this promotion's journal held for one acquisition.
    pub fn with_locked<R>(
        &self,
        promotion: &PromotionId,
        body: impl FnOnce(&mut PromotionJournalGuard<'_, '_>) -> DraftResult<R>,
    ) -> DraftResult<R> {
        self.records
            .with_locked_record(promotion.as_str(), DEFAULT_LOCK_TIMEOUT, |record_guard| {
                let mut guard = PromotionJournalGuard {
                    inner: record_guard,
                };
                body(&mut guard)
            })
    }
}

/// A live, exclusive hold on one promotion's journal.
pub struct PromotionJournalGuard<'a, 'b> {
    inner: &'b mut RecordGuard<'a, PromotionJournalRecord>,
}

impl PromotionJournalGuard<'_, '_> {
    /// The authoritative current record.
    pub fn current(&self) -> DraftResult<Option<PromotionJournalRecord>> {
        self.inner.current()
    }

    /// Write the opening `Prepared` journal.
    ///
    /// Requires the journal to be absent, so a second worker cannot re-open a
    /// promotion that already has history.
    pub fn open(&mut self, journal: &PromotionJournal) -> DraftResult<PromotionJournalRecord> {
        journal.validate()?;
        if journal.state != PromotionJournalState::Prepared {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "a promotion journal opens at Prepared, not at {:?}",
                    journal.state
                ),
            ));
        }
        let record = PromotionJournalRecord {
            generation: 0,
            journal: journal.clone(),
        };
        self.inner
            .compare_exchange_locked(&ExpectedRecordState::Absent, &record)?;
        Ok(record)
    }

    /// Record the Baseline this promotion accepted, and mark it committed.
    ///
    /// One transition, because they are one fact: the journal's `baseline`
    /// before the commit is what the promotion *intended*, and after it is
    /// what the project actually accepted. Advancing the state without
    /// recording the Baseline would leave a `Committed` journal naming
    /// something that was never accepted, and a resumed promotion would report
    /// that placeholder as its result.
    pub fn commit(
        &mut self,
        baseline: &draft_dcg_contract::BaselineId,
    ) -> DraftResult<PromotionJournalRecord> {
        let current = self.require_current()?;
        if current.journal.state != PromotionJournalState::Prepared {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "promotion '{}' is at {:?}, so it cannot commit",
                    current.journal.promotion, current.journal.state
                ),
            ));
        }
        let mut committed = current.clone();
        committed.generation += 1;
        committed.journal.state = PromotionJournalState::Committed;
        committed.journal.baseline = baseline.clone();
        let expected_state = ExpectedRecordState::of(&current)?;
        self.inner
            .compare_exchange_locked(&expected_state, &committed)?;
        Ok(committed)
    }

    fn require_current(&self) -> DraftResult<PromotionJournalRecord> {
        self.current()?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                "the promotion journal does not exist, so it cannot be advanced",
            )
        })
    }

    /// Advance the journal's state, requiring it to be exactly `expected`.
    ///
    /// Refuses a stale caller and an illegal move separately, because they
    /// mean different things: one is a worker acting on a state another actor
    /// has moved past, the other is a caller inventing a shortcut through the
    /// protocol.
    pub fn advance(
        &mut self,
        expected: PromotionJournalState,
        next: PromotionJournalState,
    ) -> DraftResult<PromotionJournalRecord> {
        let current = self.require_current()?;

        if current.journal.state != expected {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "promotion '{}' is at {:?} but the caller expected {expected:?}",
                    current.journal.promotion, current.journal.state
                ),
            )
            .with_suggestion("Re-read the authoritative journal state and reclassify from it."));
        }
        if !permits(current.journal.state, next) {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "{:?} → {next:?} is not a legal promotion transition",
                    current.journal.state
                ),
            ));
        }

        let mut advanced = current.clone();
        advanced.generation += 1;
        advanced.journal.state = next;
        let expected_state = ExpectedRecordState::of(&current)?;
        self.inner
            .compare_exchange_locked(&expected_state, &advanced)?;
        Ok(advanced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::ids::{ChangePackId, ReceiptId, RevisionPackId};
    use draft_dcg_contract::receipt::ReceiptSignerBinding;
    use draft_dcg_contract::{BaselineId, Digest};

    fn promotion() -> PromotionId {
        PromotionId::parse("pro_000000000001").unwrap()
    }

    fn journal(state: PromotionJournalState) -> PromotionJournal {
        PromotionJournal {
            prepared_at: draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
            promotion: promotion(),
            revision_pack: RevisionPackId::parse("rpk_000000000001").unwrap(),
            change_pack: ChangePackId::parse("cpk_000000000001").unwrap(),
            baseline: BaselineId::new(Digest::of_bytes(b"baseline")),
            receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
            signer: ReceiptSignerBinding::new(
                draft_dcg_contract::ids::ActorId::parse("act_000000000001").unwrap(),
                "key-1",
                "ed25519",
            )
            .unwrap(),
            activity_event_ids: vec!["evt_000000000001".into()],
            expected_control: Digest::of_bytes(b"before"),
            planned_control: Digest::of_bytes(b"after"),
            planned_change: Digest::of_bytes(b"completed"),
            state,
        }
    }

    fn store(directory: &tempfile::TempDir) -> PromotionJournalStore {
        PromotionJournalStore::new(directory.path())
    }

    #[test]
    fn a_journal_opens_prepared_and_advances_through_the_protocol() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);

        store
            .with_locked(&promotion(), |guard| {
                guard.open(&journal(PromotionJournalState::Prepared))?;
                guard.advance(
                    PromotionJournalState::Prepared,
                    PromotionJournalState::Committed,
                )?;
                guard.advance(
                    PromotionJournalState::Committed,
                    PromotionJournalState::Finalized,
                )
            })
            .unwrap();

        let record = store.read_unlocked(&promotion()).unwrap().unwrap();
        assert_eq!(record.state(), PromotionJournalState::Finalized);
        assert_eq!(record.generation, 2);
    }

    #[test]
    fn a_journal_cannot_be_opened_twice() {
        // The second worker must observe the existing history rather than
        // starting a fresh one over the top of it.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store
            .with_locked(&promotion(), |guard| {
                guard.open(&journal(PromotionJournalState::Prepared))
            })
            .unwrap();

        let error = store
            .with_locked(&promotion(), |guard| {
                guard.open(&journal(PromotionJournalState::Prepared))
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    }

    #[test]
    fn a_stale_expected_state_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store
            .with_locked(&promotion(), |guard| {
                guard.open(&journal(PromotionJournalState::Prepared))?;
                guard.advance(
                    PromotionJournalState::Prepared,
                    PromotionJournalState::Committed,
                )
            })
            .unwrap();

        // A worker that read `Prepared` before recovery moved the journal must
        // not be able to act on what it read.
        let error = store
            .with_locked(&promotion(), |guard| {
                guard.advance(
                    PromotionJournalState::Prepared,
                    PromotionJournalState::Committed,
                )
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    }

    #[test]
    fn skipping_the_commit_is_refused() {
        // Prepared → Finalized would claim finalization of a commit that never
        // happened, leaving a receipt describing a Baseline nobody accepted.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let error = store
            .with_locked(&promotion(), |guard| {
                guard.open(&journal(PromotionJournalState::Prepared))?;
                guard.advance(
                    PromotionJournalState::Prepared,
                    PromotionJournalState::Finalized,
                )
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);
    }

    #[test]
    fn a_committed_promotion_cannot_go_back() {
        // The project has already accepted the Baseline; reopening would let a
        // second commit accept the same work again.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let error = store
            .with_locked(&promotion(), |guard| {
                guard.open(&journal(PromotionJournalState::Prepared))?;
                guard.advance(
                    PromotionJournalState::Prepared,
                    PromotionJournalState::Committed,
                )?;
                guard.advance(
                    PromotionJournalState::Committed,
                    PromotionJournalState::Prepared,
                )
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);
    }

    #[test]
    fn a_journal_that_accepts_nothing_is_refused_at_open() {
        // Equal expected and planned control states means recovery could never
        // tell whether the commit landed.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let mut degenerate = journal(PromotionJournalState::Prepared);
        degenerate.planned_control = degenerate.expected_control.clone();

        let error = store
            .with_locked(&promotion(), |guard| guard.open(&degenerate))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);
    }
}
