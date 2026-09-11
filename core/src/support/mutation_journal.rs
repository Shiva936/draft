//! `MutationJournal` — one audited record mutation, made restartable.
//!
//! An audited mutation is two durable effects that must agree: the record
//! changes, and an Activity event says it changed. A crash between them leaves
//! a question no amount of later inspection can answer — *did it commit?* — so
//! the intent is written down first, and recovery reads the answer off the
//! authoritative record rather than guessing.
//!
//! ```text
//!  1. acquire the record's stable-sidecar ProcessFileLock
//!  2. RESOLVE any unresolved journal for that key            <- the barrier
//!  3. re-read the authoritative value (or observe its absence)
//!  4. verify the caller's expected state
//!  5. preallocate the ActivityEventId
//!  6. construct the exact replacement
//!  7. persist the journal as Prepared; fsync
//!  8. write the replacement through the guard; fsync
//!  9. journal -> Committed; fsync
//! 10. release the record lock
//! 11. drain the AuditFact idempotently
//! 12. journal -> Finalized
//! ```
//!
//! # The barrier
//!
//! > No audited mutation may begin while an unresolved journal exists for the
//! > same record.
//!
//! Without it, a second mutation could advance the record past the state the
//! first journal recorded, and the first transaction's outcome would become
//! permanently undecidable — its expected and replacement states would both
//! fail to match, and there would be no way to tell "it never committed" from
//! "it committed and something else happened after".
//!
//! # Recovery is a comparison, never a guess
//!
//! | Authoritative record equals | Resolution |
//! |---|---|
//! | the exact **expected** state (including `Absent`) | did not commit; the AuditFact is **never** emitted |
//! | the exact **replacement** state | committed; drain the AuditFact exactly once |
//! | neither | impossible under the barrier: hard inconsistency |
//!
//! The middle row is why the audit event is preallocated rather than minted
//! during the drain: recovery must be able to emit *that exact event*, once,
//! however many times recovery runs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::record_guard::{ExpectedRecordState, RecordGuard, RevisionedRecord};

/// Where a transaction has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationJournalState {
    /// The intent is durable; the record may or may not have been written.
    /// The only genuinely ambiguous state, and the one recovery resolves.
    Prepared,
    /// The record was written. The AuditFact still has to be drained.
    Committed,
    /// The AuditFact was drained. Nothing remains.
    Finalized,
    /// The mutation provably did not commit. No AuditFact is ever emitted.
    Abandoned,
}

/// The record state a journal refers to.
///
/// Mirrors [`ExpectedRecordState`] in a form that serializes, so a journal
/// written before a crash can be compared byte-for-byte afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JournalRecordState {
    Absent,
    Present {
        generation: u64,
        value_digest: String,
    },
}

impl From<&ExpectedRecordState> for JournalRecordState {
    fn from(state: &ExpectedRecordState) -> Self {
        match state {
            ExpectedRecordState::Absent => Self::Absent,
            ExpectedRecordState::Present {
                generation,
                value_digest,
            } => Self::Present {
                generation: *generation,
                value_digest: value_digest.clone(),
            },
        }
    }
}

impl From<&JournalRecordState> for ExpectedRecordState {
    fn from(state: &JournalRecordState) -> Self {
        match state {
            JournalRecordState::Absent => Self::Absent,
            JournalRecordState::Present {
                generation,
                value_digest,
            } => Self::Present {
                generation: *generation,
                value_digest: value_digest.clone(),
            },
        }
    }
}

/// The Activity event this mutation will emit, decided before the commit.
///
/// The id is preallocated so that however many times recovery runs, it emits
/// the same event — which is what makes the drain idempotent rather than
/// merely retried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditFactEnvelope {
    /// The preallocated `evt_` identity.
    pub activity_event_id: String,
    /// The domain fact, canonical and opaque to this module.
    pub payload: serde_json::Value,
}

/// One audited record mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationJournal {
    pub transaction_id: String,
    pub record_key: String,
    pub expected: JournalRecordState,
    pub replacement: JournalRecordState,
    pub audit_fact: AuditFactEnvelope,
    pub state: MutationJournalState,
}

impl MutationJournal {
    /// Whether this transaction still requires resolution before another
    /// mutation may touch the same record.
    pub fn is_unresolved(&self) -> bool {
        matches!(
            self.state,
            MutationJournalState::Prepared | MutationJournalState::Committed
        )
    }
}

/// What recovery concluded about a prepared transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalResolution {
    /// The record still equals the expected state: the write never landed.
    DidNotCommit,
    /// The record equals the replacement state: the write landed, and the
    /// AuditFact must be drained exactly once.
    Committed,
}

/// Journals for one family of records.
#[derive(Debug, Clone)]
pub struct MutationJournalStore {
    directory: PathBuf,
}

impl MutationJournalStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn journal_path(&self, record_key: &str) -> PathBuf {
        self.directory.join(format!("{record_key}.journal.json"))
    }

    /// The journal for `record_key`, if one exists.
    pub fn load(&self, record_key: &str) -> DraftResult<Option<MutationJournal>> {
        let path = self.journal_path(record_key);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(|error| {
            DraftError::storage(format!("cannot read journal {}: {error}", path.display()))
        })?;
        serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!("cannot decode journal {}: {error}", path.display()),
            )
        })
    }

    /// Persist a journal durably.
    pub fn write(&self, journal: &MutationJournal) -> DraftResult<()> {
        let encoded = serde_json::to_vec_pretty(journal)
            .map_err(|error| DraftError::storage(format!("cannot encode journal: {error}")))?;
        crate::support::fsutil::write_atomic(&self.journal_path(&journal.record_key), &encoded)
    }

    /// Advance a journal to `state`.
    pub fn transition(
        &self,
        journal: &MutationJournal,
        state: MutationJournalState,
    ) -> DraftResult<MutationJournal> {
        let advanced = MutationJournal {
            state,
            ..journal.clone()
        };
        self.write(&advanced)?;
        Ok(advanced)
    }

    /// Remove a finished journal.
    ///
    /// Only ever called for a terminal state; removing an unresolved journal
    /// would destroy the only record of what was in flight.
    pub fn clear(&self, record_key: &str) -> DraftResult<()> {
        let journal = self.journal_path(record_key);
        if !journal.exists() {
            return Ok(());
        }
        std::fs::remove_file(&journal).map_err(|error| {
            DraftError::storage(format!(
                "cannot remove journal {}: {error}",
                journal.display()
            ))
        })?;
        if let Some(parent) = journal.parent() {
            crate::support::fsutil::sync_directory(parent)?;
        }
        Ok(())
    }
}

/// Decide the outcome of an unresolved transaction from authoritative state.
///
/// Must be called with the record's lock held — the comparison is only
/// meaningful inside the critical section that the mutation itself ran in.
pub fn resolve<T: RevisionedRecord>(
    guard: &RecordGuard<'_, T>,
    journal: &MutationJournal,
) -> DraftResult<JournalResolution> {
    let current = JournalRecordState::from(&guard.current_state()?);
    if current == journal.expected {
        return Ok(JournalResolution::DidNotCommit);
    }
    if current == journal.replacement {
        return Ok(JournalResolution::Committed);
    }
    Err(DraftError::new(
        DraftErrorKind::CorruptData,
        format!(
            "record '{}' matches neither the prepared transaction's expected state nor its \
             replacement, which the mutation barrier makes impossible without a direct-write \
             bypass",
            journal.record_key
        ),
    )
    .with_suggestion("Run `draft doctor`; this needs recovery rather than a retry."))
}

/// Resolve any unresolved journal for `record_key` before a new mutation.
///
/// This is step 2, the barrier. It returns the drain obligation an already
/// committed transaction left behind, so the caller emits that exact
/// preallocated event before proceeding.
pub fn enforce_barrier<T: RevisionedRecord>(
    journals: &MutationJournalStore,
    guard: &RecordGuard<'_, T>,
) -> DraftResult<Option<AuditFactEnvelope>> {
    let Some(journal) = journals.load(guard.key())? else {
        return Ok(None);
    };
    if !journal.is_unresolved() {
        return Ok(None);
    }
    match resolve(guard, &journal)? {
        JournalResolution::DidNotCommit => {
            // The AuditFact is never emitted: nothing happened to describe.
            journals.transition(&journal, MutationJournalState::Abandoned)?;
            journals.clear(guard.key())?;
            crate::support::telemetry::Counter::MutationJournalAbandoned.increment();
            Ok(None)
        }
        JournalResolution::Committed => {
            let committed = journals.transition(&journal, MutationJournalState::Committed)?;
            crate::support::telemetry::Counter::MutationJournalRecovered.increment();
            Ok(Some(committed.audit_fact))
        }
    }
}

/// Mark a drained transaction finalized and clear it.
pub fn finalize(journals: &MutationJournalStore, record_key: &str) -> DraftResult<()> {
    if let Some(journal) = journals.load(record_key)? {
        journals.transition(&journal, MutationJournalState::Finalized)?;
    }
    journals.clear(record_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::record_guard::{RevisionedRecordStore, DEFAULT_LOCK_TIMEOUT};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
    struct Change {
        generation: u64,
        lifecycle: String,
    }

    impl RevisionedRecord for Change {
        fn generation(&self) -> u64 {
            self.generation
        }
    }

    struct Harness {
        _directory: tempfile::TempDir,
        records: RevisionedRecordStore<Change>,
        journals: MutationJournalStore,
    }

    fn harness() -> Harness {
        let directory = tempfile::tempdir().unwrap();
        let records = RevisionedRecordStore::new(directory.path().join("changes"));
        let journals = MutationJournalStore::new(directory.path().join("journals"));
        Harness {
            _directory: directory,
            records,
            journals,
        }
    }

    fn change(generation: u64, lifecycle: &str) -> Change {
        Change {
            generation,
            lifecycle: lifecycle.to_string(),
        }
    }

    fn journal(
        expected: &ExpectedRecordState,
        replacement: &Change,
        state: MutationJournalState,
    ) -> MutationJournal {
        MutationJournal {
            transaction_id: "txn_1".into(),
            record_key: "chg_a1".into(),
            expected: JournalRecordState::from(expected),
            replacement: JournalRecordState::from(&ExpectedRecordState::of(replacement).unwrap()),
            audit_fact: AuditFactEnvelope {
                activity_event_id: "evt_0000000000000001".into(),
                payload: serde_json::json!({"kind": "ChangeCreated"}),
            },
            state,
        }
    }

    #[test]
    fn a_creation_and_an_update_use_the_same_transaction_shape() {
        let harness = harness();
        // `Absent` as an expected state is what removes the ad-hoc creation
        // path; ChangeCreated needs no special case.
        let created = journal(
            &ExpectedRecordState::Absent,
            &change(0, "active"),
            MutationJournalState::Prepared,
        );
        assert_eq!(created.expected, JournalRecordState::Absent);
        harness.journals.write(&created).unwrap();
        assert_eq!(harness.journals.load("chg_a1").unwrap().unwrap(), created);
    }

    #[test]
    fn recovery_of_a_write_that_never_landed_emits_no_event() {
        // The crash between step 7 and step 8. The record is untouched, so the
        // event must never appear: Activity records what happened.
        let harness = harness();
        let prepared = journal(
            &ExpectedRecordState::Absent,
            &change(0, "active"),
            MutationJournalState::Prepared,
        );
        harness.journals.write(&prepared).unwrap();

        let drain = harness
            .records
            .with_locked_record("chg_a1", DEFAULT_LOCK_TIMEOUT, |guard| {
                assert_eq!(resolve(guard, &prepared)?, JournalResolution::DidNotCommit);
                enforce_barrier(&harness.journals, guard)
            })
            .unwrap();
        assert!(
            drain.is_none(),
            "no event may be emitted for a write that never landed"
        );
        assert!(harness.journals.load("chg_a1").unwrap().is_none());
    }

    #[test]
    fn recovery_of_a_committed_write_returns_the_exact_preallocated_event() {
        // The crash between step 8 and step 9. The record moved, so the event
        // is owed — and it must be the id chosen before the commit, not a new
        // one, or a second recovery would emit a second event.
        let harness = harness();
        let prepared = journal(
            &ExpectedRecordState::Absent,
            &change(0, "active"),
            MutationJournalState::Prepared,
        );
        harness.journals.write(&prepared).unwrap();
        harness
            .records
            .compare_exchange("chg_a1", &ExpectedRecordState::Absent, &change(0, "active"))
            .unwrap();

        let drain = harness
            .records
            .with_locked_record("chg_a1", DEFAULT_LOCK_TIMEOUT, |guard| {
                enforce_barrier(&harness.journals, guard)
            })
            .unwrap()
            .expect("a committed transaction owes its event");
        assert_eq!(drain.activity_event_id, "evt_0000000000000001");

        finalize(&harness.journals, "chg_a1").unwrap();
        assert!(harness.journals.load("chg_a1").unwrap().is_none());
    }

    #[test]
    fn draining_twice_still_emits_exactly_one_event() {
        // Recovery may run any number of times. Each run yields the same
        // preallocated id, so the append is idempotent at the ledger.
        let harness = harness();
        let prepared = journal(
            &ExpectedRecordState::Absent,
            &change(0, "active"),
            MutationJournalState::Prepared,
        );
        harness.journals.write(&prepared).unwrap();
        harness
            .records
            .compare_exchange("chg_a1", &ExpectedRecordState::Absent, &change(0, "active"))
            .unwrap();

        let mut ids = Vec::new();
        for _ in 0..3 {
            let drain = harness
                .records
                .with_locked_record("chg_a1", DEFAULT_LOCK_TIMEOUT, |guard| {
                    enforce_barrier(&harness.journals, guard)
                })
                .unwrap();
            if let Some(fact) = drain {
                ids.push(fact.activity_event_id);
            }
        }
        assert!(ids.iter().all(|id| id == "evt_0000000000000001"));
    }

    #[test]
    fn a_record_matching_neither_state_is_a_hard_inconsistency() {
        // Only reachable by bypassing the barrier or writing the record
        // directly. Guessing here would be worse than refusing.
        let harness = harness();
        let prepared = journal(
            &ExpectedRecordState::Absent,
            &change(0, "active"),
            MutationJournalState::Prepared,
        );
        harness.journals.write(&prepared).unwrap();
        harness
            .records
            .compare_exchange(
                "chg_a1",
                &ExpectedRecordState::Absent,
                &change(0, "something-else-entirely"),
            )
            .unwrap();

        let error = harness
            .records
            .with_locked_record("chg_a1", DEFAULT_LOCK_TIMEOUT, |guard| {
                resolve(guard, &prepared)
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn the_barrier_reports_unresolved_transactions_and_ignores_terminal_ones() {
        let harness = harness();
        for terminal in [
            MutationJournalState::Finalized,
            MutationJournalState::Abandoned,
        ] {
            let done = journal(&ExpectedRecordState::Absent, &change(0, "active"), terminal);
            assert!(!done.is_unresolved(), "{terminal:?} still blocks mutation");
        }
        for unresolved in [
            MutationJournalState::Prepared,
            MutationJournalState::Committed,
        ] {
            let pending = journal(
                &ExpectedRecordState::Absent,
                &change(0, "active"),
                unresolved,
            );
            assert!(
                pending.is_unresolved(),
                "{unresolved:?} must block mutation"
            );
        }
        let _ = &harness;
    }

    #[test]
    fn a_committed_journal_is_resolved_before_a_later_mutation_proceeds() {
        // The barrier's purpose. If the second mutation ran first, the record
        // would move past the first journal's replacement state and that
        // transaction's outcome would become permanently undecidable.
        let harness = harness();
        let prepared = journal(
            &ExpectedRecordState::Absent,
            &change(0, "active"),
            MutationJournalState::Prepared,
        );
        harness.journals.write(&prepared).unwrap();
        harness
            .records
            .compare_exchange("chg_a1", &ExpectedRecordState::Absent, &change(0, "active"))
            .unwrap();

        let owed = harness
            .records
            .with_locked_record("chg_a1", DEFAULT_LOCK_TIMEOUT, |guard| {
                let owed = enforce_barrier(&harness.journals, guard)?;
                // Only now may the next mutation be prepared.
                let current = guard.current_state()?;
                guard.compare_exchange_locked(&current, &change(1, "completed"))?;
                Ok(owed)
            })
            .unwrap();
        assert!(owed.is_some(), "the earlier transaction's event was owed");
        assert_eq!(
            harness
                .records
                .read_unlocked("chg_a1")
                .unwrap()
                .unwrap()
                .lifecycle,
            "completed"
        );
    }

    #[test]
    fn a_corrupt_journal_is_reported_rather_than_ignored() {
        // Ignoring it would silently drop the barrier for that record.
        let harness = harness();
        std::fs::create_dir_all(harness.journals.journal_path("chg_a1").parent().unwrap()).unwrap();
        std::fs::write(harness.journals.journal_path("chg_a1"), b"{not json").unwrap();
        let error = harness.journals.load("chg_a1").unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn the_journal_wire_form_round_trips() {
        let prepared = journal(
            &ExpectedRecordState::Present {
                generation: 3,
                value_digest: "sha256:abc".into(),
            },
            &change(4, "completed"),
            MutationJournalState::Prepared,
        );
        let encoded = serde_json::to_string(&prepared).unwrap();
        assert_eq!(
            MutationJournal::deserialize(&mut serde_json::Deserializer::from_str(&encoded))
                .unwrap(),
            prepared
        );
    }
}
