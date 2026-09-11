//! The single converter, and the only orchestrator of a normal Activity append.
//!
//! # Why appending is centralised
//!
//! Lower layers persist **domain audit facts** carrying a preallocated event
//! id. They do not construct Activity payloads and they do not call the ledger.
//! Exactly one place converts a fact into an event and appends it.
//!
//! The alternative — every module appending its own events — fails in a
//! specific way rather than merely being untidy. An append that happens
//! *before* its mutation commits records something that may not have happened;
//! one that happens after, outside a transaction, is lost on the crash in
//! between. Centralising the conversion is what lets the append be the drain
//! step of a durable transaction rather than a side effect somebody remembered.
//!
//! # The audited mutation, end to end
//!
//! ```text
//!  1. acquire the record's stable-sidecar lock
//!  2. resolve any unresolved journal for that key            <- the barrier
//!  3. re-read the authoritative value
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
//! Steps 1–9 run under the record's lock; the drain does not, because the
//! ledger is order 10 and holding a record lock across it would serialize
//! unrelated work behind the ledger for no benefit. What makes that safe is
//! that the drain is idempotent on the preallocated id: whether it runs now,
//! after a crash, or twice, the ledger ends with exactly one event.

use serde_json::Value;

use crate::activity::log::event_id_for;
use crate::activity::{ActivityLog, AppendOutcome, EventKind};
use crate::support::error::DraftResult;
use crate::support::mutation_journal::{
    self, AuditFactEnvelope, MutationJournal, MutationJournalState, MutationJournalStore,
};
use crate::support::record_guard::{
    ExpectedRecordState, RevisionedRecord, RevisionedRecordStore, DEFAULT_LOCK_TIMEOUT,
};

/// What an audited mutation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditedMutation {
    /// The event this mutation appended, or recognised as already appended.
    pub activity_event_id: String,
    /// Whether an earlier transaction's event was drained on the way in.
    ///
    /// Surfaced rather than hidden: it means a previous run of this mutation
    /// committed and then crashed before recording it, which is worth being
    /// able to see.
    pub recovered_earlier_event: Option<String>,
}

/// The three stores an audited mutation needs, which always travel together.
///
/// Named rather than passed loose because they are one collaboration: the
/// record is what changes, the journal is what makes the change decidable after
/// a crash, and the ledger is where it is finally said to have happened. A
/// caller holding two of the three has no useful operation.
#[derive(Debug, Clone, Copy)]
pub struct AuditedStores<'a, T: RevisionedRecord> {
    pub records: &'a RevisionedRecordStore<T>,
    pub journals: &'a MutationJournalStore,
    pub ledger: &'a ActivityLog,
}

/// Perform one audited record mutation.
///
/// `expected` is checked inside the record's critical section, so a caller that
/// read the record before the lock was available cannot commit against a value
/// that has since moved.
pub fn commit_audited_mutation<T: RevisionedRecord>(
    stores: AuditedStores<'_, T>,
    record_key: &str,
    transaction_id: &str,
    expected: &ExpectedRecordState,
    replacement: &T,
    audit_fact: AuditFactEnvelope,
) -> DraftResult<AuditedMutation> {
    let AuditedStores {
        records,
        journals,
        ledger,
    } = stores;
    commit_audited_mutation_draining(
        records,
        journals,
        record_key,
        transaction_id,
        expected,
        replacement,
        audit_fact,
        |fact| drain(ledger, fact).map(|_| ()),
    )
}

/// The same protocol, draining the audit fact wherever the caller says.
///
/// One implementation of the twelve steps, not two. The drain target is a
/// parameter because the ledger a subsystem records into is a migration
/// question, and duplicating the journal protocol to vary it would mean two
/// places that must stay correct about crash recovery.
#[allow(clippy::too_many_arguments)]
pub fn commit_audited_mutation_draining<T: RevisionedRecord>(
    records: &RevisionedRecordStore<T>,
    journals: &MutationJournalStore,
    record_key: &str,
    transaction_id: &str,
    expected: &ExpectedRecordState,
    replacement: &T,
    audit_fact: AuditFactEnvelope,
    mut drain_fact: impl FnMut(&AuditFactEnvelope) -> DraftResult<()>,
) -> DraftResult<AuditedMutation> {
    // Steps 1-9, all inside one acquisition.
    let (recovered, journal) = records.with_locked_record(
        record_key,
        DEFAULT_LOCK_TIMEOUT,
        |guard| -> DraftResult<(Option<AuditFactEnvelope>, MutationJournal)> {
            // 2. The barrier. A mutation that began while an earlier
            //    transaction was unresolved would move the record past that
            //    journal's replacement state, leaving its outcome permanently
            //    undecidable.
            let recovered = mutation_journal::enforce_barrier(journals, guard)?;

            // 3-4. Authoritative read and expected-state check.
            let current = guard.current_state()?;

            // 5-7. The intent becomes durable before the record moves, so a
            //      crash after this point is decidable rather than a guess.
            let journal = MutationJournal {
                transaction_id: transaction_id.to_string(),
                record_key: record_key.to_string(),
                expected: (&current).into(),
                replacement: (&ExpectedRecordState::of(replacement)?).into(),
                audit_fact,
                state: MutationJournalState::Prepared,
            };
            journals.write(&journal)?;

            // 8-9.
            guard.compare_exchange_locked(expected, replacement)?;
            let committed = journals.transition(&journal, MutationJournalState::Committed)?;
            Ok((recovered, committed))
        },
    )?;

    // 11. Outside the record lock: the ledger is innermost in the order, and
    //     holding a record across it would serialize unrelated work behind it.
    if let Some(earlier) = &recovered {
        drain_fact(earlier)?;
    }
    drain_fact(&journal.audit_fact)?;

    // 12.
    mutation_journal::finalize(journals, record_key)?;

    Ok(AuditedMutation {
        activity_event_id: journal.audit_fact.activity_event_id.clone(),
        recovered_earlier_event: recovered.map(|fact| fact.activity_event_id),
    })
}

/// Drain one audit fact into the ledger.
///
/// The only conversion from a domain fact to an Activity event. Idempotent on
/// the preallocated id, so a replay after a crash appends nothing.
pub fn drain(ledger: &ActivityLog, fact: &AuditFactEnvelope) -> DraftResult<AppendOutcome> {
    ledger.append(&fact.activity_event_id, &fact.payload)
}

/// One domain audit fact, before it is an Activity event.
///
/// Lower layers describe *what happened* in these terms; only this module
/// turns one into the payload the ledger stores. Keeping the payload shape in
/// exactly one place is what stops two subsystems recording the same kind of
/// fact under two different shapes, which a reader of the ledger could never
/// reconcile afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainAuditFact {
    /// Which frozen v1 event this fact becomes.
    pub kind: EventKind,
    /// The graph object the event is about, when there is one.
    pub subject: Option<String>,
    /// Who caused it.
    pub actor: String,
    /// When the fact this event records became durable.
    ///
    /// Supplied by the fact rather than read at append time, because the
    /// append is idempotent on the payload: a recovery replaying a drain has
    /// to produce byte-identical bytes, and `now()` would not. A transaction
    /// with a journal freezes this when it prepares.
    pub recorded_at: draft_dcg_contract::value::Timestamp,
    /// Everything else the event needs to be understood without the store
    /// that produced it.
    pub metadata: Value,
}

impl DomainAuditFact {
    pub fn new(
        kind: EventKind,
        actor: impl Into<String>,
        recorded_at: draft_dcg_contract::value::Timestamp,
    ) -> Self {
        Self {
            kind,
            subject: None,
            actor: actor.into(),
            recorded_at,
            metadata: Value::Object(serde_json::Map::new()),
        }
    }

    pub fn about(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    pub fn with(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }
}

/// The one converter: a domain fact becomes exactly this payload.
///
/// Frozen shape. `kind` is the v1 vocabulary name, so a reader never has to
/// know which subsystem wrote the record in order to interpret it.
pub fn payload_of(fact: &DomainAuditFact) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("kind".into(), Value::String(fact.kind.as_str().to_string()));
    if let Some(subject) = &fact.subject {
        payload.insert("subject".into(), Value::String(subject.clone()));
    }
    payload.insert("actor".into(), Value::String(fact.actor.clone()));
    payload.insert(
        "recorded_at".into(),
        Value::Number(fact.recorded_at.as_unix_nanos().into()),
    );
    payload.insert("metadata".into(), fact.metadata.clone());
    Value::Object(payload)
}

/// Append `fact` under the id its transaction preallocated.
///
/// The normal path for a journalled mutation: the id was chosen before the
/// commit, so replaying the drain converges on the same single record.
pub fn record_with_id(
    ledger: &ActivityLog,
    event_id: &str,
    fact: &DomainAuditFact,
) -> DraftResult<AppendOutcome> {
    ledger.append(event_id, &payload_of(fact))
}

/// The Activity Ledger for one project, as the application layer uses it.
///
/// Wraps the log with the two things an app-layer append always needs: the
/// acting identity, resolved from the project, and redaction, so a metadata
/// value that happens to carry a secret never reaches durable history.
///
/// Reading goes through [`crate::read_model::activity`]; this type exists for
/// the write side and the handle a reader needs.
#[derive(Debug, Clone)]
pub struct ProjectActivity {
    layout: crate::project::layout::DraftLayout,
    log: ActivityLog,
}

impl ProjectActivity {
    pub fn new(
        layout: crate::project::layout::DraftLayout,
        project: &draft_dcg_contract::ids::ProjectId,
    ) -> Self {
        let log = ActivityLog::new(layout.events_dir(), project.to_string());
        Self { layout, log }
    }

    /// The underlying log, for the paths that preallocate their own ids.
    pub fn log(&self) -> &ActivityLog {
        &self.log
    }

    /// Every event, oldest first.
    pub fn read_all(&self) -> DraftResult<Vec<crate::read_model::ActivityEntry>> {
        crate::read_model::activity::entries(&self.log)
    }

    /// Verify the whole chain, returning how many records held.
    pub fn verify_chain(&self) -> DraftResult<usize> {
        self.log.verify_chain()
    }

    /// Append one event, minting its id from the record it will become.
    ///
    /// The id is derived from the ledger position and the fact, so replaying
    /// an interrupted operation that reaches the same point appends the same
    /// event rather than a duplicate. A transaction with a durable journal
    /// preallocates through that instead and calls [`record_with_id`].
    pub fn append(
        &self,
        kind: EventKind,
        subject: Option<String>,
        metadata: Value,
    ) -> DraftResult<String> {
        let actor = crate::trust::identity::resolve_actor(&self.layout.draft_dir)?;
        let now = crate::support::clock::Clock::now(&crate::support::clock::SystemClock);
        let mut fact = DomainAuditFact::new(kind, actor.id.to_string(), now)
            .with(crate::support::redaction::redact_value(metadata));
        fact.subject = subject;

        let tail = self.log.tail_hash()?;
        let event_id = event_id_for(&format!(
            "{tail}|{}|{}",
            kind.as_str(),
            crate::support::hashing::canonical_json(&payload_of(&fact))
        ));
        record_with_id(&self.log, &event_id, &fact)?;
        Ok(event_id)
    }
}

/// Resolve any unresolved journal for `record_key` without mutating the record.
///
/// What startup recovery calls. A transaction that committed but never drained
/// gets its exact preallocated event; one that never committed is abandoned and
/// emits nothing, because Activity records what happened.
pub fn recover_record<T: RevisionedRecord>(
    stores: AuditedStores<'_, T>,
    record_key: &str,
) -> DraftResult<Option<String>> {
    let AuditedStores {
        records,
        journals,
        ledger,
    } = stores;

    let owed = records.with_locked_record(record_key, DEFAULT_LOCK_TIMEOUT, |guard| {
        mutation_journal::enforce_barrier(journals, guard)
    })?;

    let Some(fact) = owed else {
        return Ok(None);
    };
    drain(ledger, &fact)?;
    mutation_journal::finalize(journals, record_key)?;
    Ok(Some(fact.activity_event_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        ledger: ActivityLog,
    }

    fn harness() -> Harness {
        let directory = tempfile::tempdir().unwrap();
        Harness {
            records: RevisionedRecordStore::new(directory.path().join("changes")),
            journals: MutationJournalStore::new(directory.path().join("journals")),
            ledger: ActivityLog::new(directory.path().join("events"), "prj_000000000001"),
            _directory: directory,
        }
    }

    impl Harness {
        fn stores(&self) -> AuditedStores<'_, Change> {
            AuditedStores {
                records: &self.records,
                journals: &self.journals,
                ledger: &self.ledger,
            }
        }
    }

    fn change(generation: u64, lifecycle: &str) -> Change {
        Change {
            generation,
            lifecycle: lifecycle.to_string(),
        }
    }

    fn fact(event_id: &str, kind: &str) -> AuditFactEnvelope {
        AuditFactEnvelope {
            activity_event_id: event_id.to_string(),
            payload: json!({ "kind": kind, "change": "chg_a1" }),
        }
    }

    fn commit(
        harness: &Harness,
        transaction: &str,
        expected: &ExpectedRecordState,
        replacement: &Change,
        fact: AuditFactEnvelope,
    ) -> DraftResult<AuditedMutation> {
        commit_audited_mutation(
            harness.stores(),
            "chg_a1",
            transaction,
            expected,
            replacement,
            fact,
        )
    }

    #[test]
    fn an_audited_creation_moves_the_record_and_records_one_event() {
        let harness = harness();
        let outcome = commit(
            &harness,
            "txn_1",
            &ExpectedRecordState::Absent,
            &change(0, "active"),
            fact("evt_1", "ChangeCreated"),
        )
        .unwrap();

        assert_eq!(outcome.activity_event_id, "evt_1");
        assert!(outcome.recovered_earlier_event.is_none());
        assert_eq!(
            harness.records.read_unlocked("chg_a1").unwrap().unwrap(),
            change(0, "active")
        );
        let records = harness.ledger.read_all().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].event_id, "evt_1");
        // Nothing is left behind to block the next mutation.
        assert!(harness.journals.load("chg_a1").unwrap().is_none());
    }

    #[test]
    fn a_rejected_mutation_records_nothing() {
        // The expected-state check fires before the record moves, so a stale
        // caller leaves no trace in Activity.
        let harness = harness();
        commit(
            &harness,
            "txn_1",
            &ExpectedRecordState::Absent,
            &change(0, "active"),
            fact("evt_1", "ChangeCreated"),
        )
        .unwrap();

        let stale = ExpectedRecordState::Absent;
        assert!(commit(
            &harness,
            "txn_2",
            &stale,
            &change(0, "conflicting"),
            fact("evt_2", "ChangeAbandoned"),
        )
        .is_err());

        let records = harness.ledger.read_all().unwrap();
        assert_eq!(records.len(), 1, "a refused mutation appended nothing");
        assert_eq!(records[0].event_id, "evt_1");
    }

    #[test]
    fn a_crash_after_the_commit_drains_the_exact_event_on_recovery() {
        // The window between step 9 and step 11. The record moved, so the
        // event is owed — and it must be the id chosen before the commit.
        let harness = harness();
        harness
            .records
            .with_locked_record("chg_a1", DEFAULT_LOCK_TIMEOUT, |guard| {
                let journal = MutationJournal {
                    transaction_id: "txn_1".into(),
                    record_key: "chg_a1".into(),
                    expected: (&ExpectedRecordState::Absent).into(),
                    replacement: (&ExpectedRecordState::of(&change(0, "active"))?).into(),
                    audit_fact: fact("evt_1", "ChangeCreated"),
                    state: MutationJournalState::Prepared,
                };
                harness.journals.write(&journal)?;
                guard.compare_exchange_locked(&ExpectedRecordState::Absent, &change(0, "active"))
            })
            .unwrap();

        // Nothing has been appended yet.
        assert!(harness.ledger.read_all().unwrap().is_empty());

        let recovered = recover_record(harness.stores(), "chg_a1").unwrap();
        assert_eq!(recovered.as_deref(), Some("evt_1"));
        assert_eq!(harness.ledger.read_all().unwrap().len(), 1);
    }

    #[test]
    fn a_crash_before_the_commit_emits_no_event_at_all() {
        // The window between step 7 and step 8. The record never moved, so
        // Activity must stay silent.
        let harness = harness();
        let journal = MutationJournal {
            transaction_id: "txn_1".into(),
            record_key: "chg_a1".into(),
            expected: (&ExpectedRecordState::Absent).into(),
            replacement: (&ExpectedRecordState::of(&change(0, "active")).unwrap()).into(),
            audit_fact: fact("evt_1", "ChangeCreated"),
            state: MutationJournalState::Prepared,
        };
        harness.journals.write(&journal).unwrap();

        let recovered = recover_record(harness.stores(), "chg_a1").unwrap();
        assert!(recovered.is_none());
        assert!(
            harness.ledger.read_all().unwrap().is_empty(),
            "a mutation that never committed must not appear in Activity"
        );
    }

    #[test]
    fn recovery_is_idempotent_however_many_times_it_runs() {
        let harness = harness();
        harness
            .records
            .with_locked_record("chg_a1", DEFAULT_LOCK_TIMEOUT, |guard| {
                let journal = MutationJournal {
                    transaction_id: "txn_1".into(),
                    record_key: "chg_a1".into(),
                    expected: (&ExpectedRecordState::Absent).into(),
                    replacement: (&ExpectedRecordState::of(&change(0, "active"))?).into(),
                    audit_fact: fact("evt_1", "ChangeCreated"),
                    state: MutationJournalState::Prepared,
                };
                harness.journals.write(&journal)?;
                guard.compare_exchange_locked(&ExpectedRecordState::Absent, &change(0, "active"))
            })
            .unwrap();

        for round in 0..4 {
            let recovered = recover_record(harness.stores(), "chg_a1").unwrap();
            if round == 0 {
                assert_eq!(recovered.as_deref(), Some("evt_1"));
            } else {
                assert!(recovered.is_none(), "the journal is finished after round 0");
            }
        }
        assert_eq!(harness.ledger.read_all().unwrap().len(), 1);
    }

    #[test]
    fn a_later_mutation_drains_an_earlier_transaction_before_proceeding() {
        // The barrier, exercised through the real orchestrator: the next
        // mutation cannot begin until the earlier one's outcome is settled,
        // and it surfaces that it settled it.
        let harness = harness();
        harness
            .records
            .with_locked_record("chg_a1", DEFAULT_LOCK_TIMEOUT, |guard| {
                let journal = MutationJournal {
                    transaction_id: "txn_1".into(),
                    record_key: "chg_a1".into(),
                    expected: (&ExpectedRecordState::Absent).into(),
                    replacement: (&ExpectedRecordState::of(&change(0, "active"))?).into(),
                    audit_fact: fact("evt_1", "ChangeCreated"),
                    state: MutationJournalState::Prepared,
                };
                harness.journals.write(&journal)?;
                guard.compare_exchange_locked(&ExpectedRecordState::Absent, &change(0, "active"))
            })
            .unwrap();

        let expected = ExpectedRecordState::of(&change(0, "active")).unwrap();
        let outcome = commit(
            &harness,
            "txn_2",
            &expected,
            &change(1, "completed"),
            fact("evt_2", "ChangeCompleted"),
        )
        .unwrap();

        assert_eq!(outcome.recovered_earlier_event.as_deref(), Some("evt_1"));
        let events: Vec<String> = harness
            .ledger
            .read_all()
            .unwrap()
            .into_iter()
            .map(|record| record.event_id)
            .collect();
        assert_eq!(
            events,
            vec!["evt_1", "evt_2"],
            "in order, exactly once each"
        );
    }

    #[test]
    fn a_sequence_of_mutations_chains_its_events() {
        let harness = harness();
        commit(
            &harness,
            "txn_1",
            &ExpectedRecordState::Absent,
            &change(0, "active"),
            fact("evt_1", "ChangeCreated"),
        )
        .unwrap();
        let expected = ExpectedRecordState::of(&change(0, "active")).unwrap();
        commit(
            &harness,
            "txn_2",
            &expected,
            &change(1, "completed"),
            fact("evt_2", "ChangeCompleted"),
        )
        .unwrap();

        assert_eq!(harness.ledger.verify_chain().unwrap(), 2);
    }

    #[test]
    fn the_drain_does_not_hold_the_record_lock() {
        // The ledger is innermost in the lock order, and a record held across
        // it would serialize unrelated work behind the append. Committing while
        // holding nothing else proves the drain runs outside the guard.
        use crate::support::lock_order;

        let harness = harness();
        commit(
            &harness,
            "txn_1",
            &ExpectedRecordState::Absent,
            &change(0, "active"),
            fact("evt_1", "ChangeCreated"),
        )
        .unwrap();
        assert!(lock_order::currently_held().is_empty());
    }
}
