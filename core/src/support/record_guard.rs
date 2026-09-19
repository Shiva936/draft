//! The guarded revisioned-record Store.
//!
//! Two mechanisms, both necessary, doing different jobs:
//!
//! * the [`ProcessFileLock`] **serializes** the read/compare/write critical
//!   section, so no two processes interleave inside it;
//! * the expected-state comparison **rejects stale intent** within that
//!   serialized boundary, so a caller that read the record before the lock was
//!   available cannot commit against a value that has since moved.
//!
//! Serialization alone would let a caller commit a decision made from a stale
//! read. Comparison alone would leave the classic lost update, because two
//! processes can both observe the same value before either writes.
//!
//! # Why the API is a guard rather than a method
//!
//! [`ProcessFileLock`] is not reentrant. A `compare_exchange` that acquired the
//! lock would deadlock against a caller that already held it — which is exactly
//! what a promotion or a publication allocation does, since it must hold the
//! record locked across several steps. So the mutation that runs inside an
//! existing critical section is reachable **only** through a live
//! [`RecordGuard`], and cannot be called without one.
//!
//! [`RevisionedRecordStore::compare_exchange`] remains available for the simple
//! case, and acquires the lock itself exactly once.
//!
//! # What is locked
//!
//! A stable `<key>.lock` sidecar, never `<key>.json`. Records are replaced by
//! atomic rename, so a lock on the record's own inode would stop protecting
//! anything the moment a write landed.

use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use crate::support::lock_order::LockOrder;
use crate::support::process_lock::ProcessFileLock;
use crate::support::telemetry::Counter;

/// How long a caller waits for a record lock before reporting contention.
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// A record whose identity includes a monotonic generation.
pub trait RevisionedRecord: Serialize + DeserializeOwned + Clone {
    /// The generation this value was written at.
    fn generation(&self) -> u64;
}

/// What a caller believes the record's state to be.
///
/// `Absent` is a first-class expectation, not a missing case: it is what makes
/// audited *creation* use the same protocol as update, so `ChangePackCreated` and
/// `TaskCreated` need no separate creation path with its own failure modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedRecordState {
    /// No record exists under this key.
    Absent,
    /// A record exists with exactly this generation and canonical digest.
    Present {
        generation: u64,
        value_digest: String,
    },
}

impl ExpectedRecordState {
    /// The state of an existing record.
    pub fn of<T: RevisionedRecord>(record: &T) -> DraftResult<Self> {
        Ok(Self::Present {
            generation: record.generation(),
            value_digest: try_canonical_hash(record)?,
        })
    }

    fn describe(&self) -> String {
        match self {
            Self::Absent => "absent".into(),
            Self::Present {
                generation,
                value_digest,
            } => format!("generation {generation} ({value_digest})"),
        }
    }
}

/// A revisioned-record Store over one directory.
#[derive(Debug, Clone)]
pub struct RevisionedRecordStore<T: RevisionedRecord> {
    directory: PathBuf,
    /// Where this Store's record locks sit in the frozen partial order.
    ///
    /// Optional because a Store that has not yet been classified must not be
    /// forced to guess a rank — an invented rank is worse than none, since it
    /// would make the checker confirm an ordering nobody chose.
    order: Option<LockOrder>,
    /// The counter a lost compare-exchange on this Store increments.
    ///
    /// Per-Store rather than one shared tally: §2.57 names a separate counter
    /// for each record family, and an operator reading "some record lost a
    /// CAS" learns nothing about which invariant is under pressure. `None`
    /// means the Store has no counter in the frozen vocabulary, which is a
    /// real answer — inventing one would be worse than leaving it unmeasured.
    conflicts: Option<Counter>,
    marker: PhantomData<T>,
}

impl<T: RevisionedRecord> RevisionedRecordStore<T> {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            order: None,
            conflicts: None,
            marker: PhantomData,
        }
    }

    /// Declare which frozen counter a lost compare-exchange here increments.
    pub fn counting_conflicts_as(mut self, counter: Counter) -> Self {
        self.conflicts = Some(counter);
        self
    }

    /// Declare where this Store's record locks sit in the partial order.
    pub fn with_order(mut self, order: LockOrder) -> Self {
        self.order = Some(order);
        self
    }

    /// Where the authoritative record lives.
    pub fn record_path(&self, key: &str) -> PathBuf {
        self.directory.join(format!("{key}.json"))
    }

    /// The stable sidecar the record's lock is held on.
    ///
    /// Deliberately a different path from the record: see the module docs.
    pub fn lock_path(&self, key: &str) -> PathBuf {
        self.directory.join(format!("{key}.lock"))
    }

    /// Every key this store holds a record for.
    ///
    /// Reads the record directory rather than an index, for the same reason
    /// the immutable-fact store does: a record invisible to an enumerating
    /// reader is a record the project has and cannot show.
    pub fn keys(&self) -> DraftResult<Vec<String>> {
        let path = self.record_path("x");
        let Some(parent) = path.parent() else {
            return Ok(Vec::new());
        };
        let entries = match std::fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(DraftError::storage(format!(
                    "cannot list records in {}: {error}",
                    parent.display()
                )))
            }
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|value| value == "json") {
                if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                    found.push(stem.to_string());
                }
            }
        }
        found.sort();
        Ok(found)
    }

    /// Read the record without locking.
    ///
    /// For display and read models only. A value read this way must never be
    /// used as the `expected` state of a mutation — that is what
    /// [`RecordGuard::current`] is for, because only a read taken *inside* the
    /// critical section is still true when the write happens.
    pub fn read_unlocked(&self, key: &str) -> DraftResult<Option<T>> {
        load(&self.record_path(key))
    }

    /// Run `body` with the record's lock held for exactly one acquisition.
    pub fn with_locked_record<R>(
        &self,
        key: &str,
        timeout: Duration,
        body: impl FnOnce(&mut RecordGuard<'_, T>) -> DraftResult<R>,
    ) -> DraftResult<R> {
        self.with_locked_record_ordered(key, timeout, self.order, body)
    }

    /// As above, declaring where this Store sits in the frozen partial order.
    pub fn with_locked_record_ordered<R>(
        &self,
        key: &str,
        timeout: Duration,
        order: Option<LockOrder>,
        body: impl FnOnce(&mut RecordGuard<'_, T>) -> DraftResult<R>,
    ) -> DraftResult<R> {
        let lock = match order {
            Some(order) => {
                ProcessFileLock::acquire_exclusive_ordered(&self.lock_path(key), timeout, order)?
            }
            None => ProcessFileLock::acquire_exclusive(&self.lock_path(key), timeout)?,
        };
        let mut guard = RecordGuard {
            store: self,
            key: key.to_string(),
            lock,
        };
        body(&mut guard)
    }

    /// Compare and exchange, acquiring the lock itself.
    ///
    /// For the simple case where the caller holds nothing yet. A caller already
    /// inside a critical section must use [`RecordGuard::compare_exchange_locked`]
    /// instead — calling this from there would deadlock on the non-reentrant
    /// lock.
    pub fn compare_exchange(
        &self,
        key: &str,
        expected: &ExpectedRecordState,
        replacement: &T,
    ) -> DraftResult<()> {
        self.with_locked_record(key, DEFAULT_LOCK_TIMEOUT, |guard| {
            guard.compare_exchange_locked(expected, replacement)
        })
    }
}

/// A live, exclusive hold on one record.
#[derive(Debug)]
pub struct RecordGuard<'a, T: RevisionedRecord> {
    store: &'a RevisionedRecordStore<T>,
    key: String,
    // Held for its side effect. Dropping the guard releases the lock.
    #[allow(dead_code)]
    lock: ProcessFileLock,
}

impl<T: RevisionedRecord> RecordGuard<'_, T> {
    /// The record this guard holds.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The authoritative current value, read inside the critical section.
    pub fn current(&self) -> DraftResult<Option<T>> {
        load(&self.store.record_path(&self.key))
    }

    /// The authoritative current state, in the form a mutation compares against.
    pub fn current_state(&self) -> DraftResult<ExpectedRecordState> {
        match self.current()? {
            None => Ok(ExpectedRecordState::Absent),
            Some(record) => ExpectedRecordState::of(&record),
        }
    }

    /// Replace the record, requiring it to be exactly `expected` first.
    ///
    /// Does **not** reacquire the lock: this guard already holds it, and the
    /// lock is not reentrant.
    ///
    /// A mismatch is a `Conflict`. Under a correctly held lock that means a
    /// stale caller or a direct-write bypass, not ordinary concurrency — two
    /// well-behaved callers cannot both be inside this critical section.
    pub fn compare_exchange_locked(
        &mut self,
        expected: &ExpectedRecordState,
        replacement: &T,
    ) -> DraftResult<()> {
        let current = self.current_state()?;
        if &current != expected {
            if let Some(counter) = self.store.conflicts {
                counter.increment();
            }
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "record '{}' is {} but the caller expected {}",
                    self.key,
                    current.describe(),
                    expected.describe()
                ),
            )
            .with_suggestion("Re-read the record and retry against its current state."));
        }

        // A generation that did not advance would make two distinct values
        // indistinguishable to every later expected-state comparison.
        let required = match expected {
            ExpectedRecordState::Absent => 0,
            ExpectedRecordState::Present { generation, .. } => generation + 1,
        };
        if replacement.generation() != required {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "record '{}' must advance to generation {required}, but the replacement is at \
                     generation {}",
                    self.key,
                    replacement.generation()
                ),
            ));
        }

        let encoded = serde_json::to_vec_pretty(replacement).map_err(|error| {
            DraftError::storage(format!("cannot encode record '{}': {error}", self.key))
        })?;
        crate::support::fsutil::write_atomic(&self.store.record_path(&self.key), &encoded)
    }
}

fn load<T: DeserializeOwned>(path: &Path) -> DraftResult<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(path)
        .map_err(|error| DraftError::storage(format!("cannot read {}: {error}", path.display())))?;
    let value = serde_json::from_slice(&bytes).map_err(|error| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!("cannot decode {}: {error}", path.display()),
        )
    })?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Control {
        generation: u64,
        accepted: String,
    }

    impl RevisionedRecord for Control {
        fn generation(&self) -> u64 {
            self.generation
        }
    }

    fn store(directory: &tempfile::TempDir) -> RevisionedRecordStore<Control> {
        RevisionedRecordStore::new(directory.path())
    }

    fn control(generation: u64, accepted: &str) -> Control {
        Control {
            generation,
            accepted: accepted.to_string(),
        }
    }

    #[test]
    fn creation_uses_the_same_protocol_as_update() {
        // `Absent` being a first-class expectation is what removes the need for
        // a separate creation path with its own failure modes.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store
            .compare_exchange("control", &ExpectedRecordState::Absent, &control(0, "one"))
            .unwrap();
        assert_eq!(
            store.read_unlocked("control").unwrap(),
            Some(control(0, "one"))
        );

        // Creating again against `Absent` now conflicts, because it exists.
        let error = store
            .compare_exchange("control", &ExpectedRecordState::Absent, &control(0, "two"))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    }

    #[test]
    fn a_stale_expected_state_is_rejected() {
        // The lost-update regression. A caller that read generation 0, then
        // lost the race, must not be able to commit over the winner.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store
            .compare_exchange("control", &ExpectedRecordState::Absent, &control(0, "one"))
            .unwrap();

        let stale = ExpectedRecordState::of(&control(0, "one")).unwrap();
        store
            .compare_exchange("control", &stale, &control(1, "winner"))
            .unwrap();

        let error = store
            .compare_exchange("control", &stale, &control(1, "loser"))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
        assert_eq!(
            store.read_unlocked("control").unwrap().unwrap().accepted,
            "winner",
            "the loser must not overwrite the committed value"
        );
    }

    #[test]
    fn a_matching_generation_with_different_content_is_still_rejected() {
        // The digest is part of the expectation, so two values that happen to
        // share a generation are not interchangeable.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store
            .compare_exchange(
                "control",
                &ExpectedRecordState::Absent,
                &control(0, "actual"),
            )
            .unwrap();

        let wrong_content = ExpectedRecordState::of(&control(0, "imagined")).unwrap();
        let error = store
            .compare_exchange("control", &wrong_content, &control(1, "next"))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    }

    #[test]
    fn the_generation_must_advance_by_exactly_one() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store
            .compare_exchange("control", &ExpectedRecordState::Absent, &control(0, "one"))
            .unwrap();
        let expected = ExpectedRecordState::of(&control(0, "one")).unwrap();

        // Not advancing would make two distinct values indistinguishable to
        // every later comparison.
        assert!(store
            .compare_exchange("control", &expected, &control(0, "same-generation"))
            .is_err());
        // Skipping ahead would leave a generation nothing ever occupied.
        assert!(store
            .compare_exchange("control", &expected, &control(5, "skipped"))
            .is_err());
        store
            .compare_exchange("control", &expected, &control(1, "correct"))
            .unwrap();
    }

    #[test]
    fn a_guarded_mutation_does_not_reacquire_the_lock() {
        // Scenario DC. The proof is structural, not timed: the lock is not
        // reentrant, so if `compare_exchange_locked` acquired anything, the
        // second call below could not return at all. That it succeeds twice
        // inside one acquisition is the assertion.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store
            .with_locked_record("control", Duration::from_millis(250), |guard| {
                let current = guard.current_state()?;
                assert_eq!(current, ExpectedRecordState::Absent);
                guard.compare_exchange_locked(&current, &control(0, "one"))?;

                let next = guard.current_state()?;
                guard.compare_exchange_locked(&next, &control(1, "two"))
            })
            .expect("two guarded mutations must fit in one acquisition");
        assert_eq!(
            store.read_unlocked("control").unwrap().unwrap().accepted,
            "two"
        );
    }

    #[test]
    fn the_lock_is_not_reentrant_which_is_why_the_guard_exists() {
        // The failure the guarded API is shaped to prevent. Calling the
        // self-locking `compare_exchange` from inside a critical section — the
        // obvious thing to reach for — would block against the caller's own
        // lock until the timeout, not merely be inefficient.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let error = store
            .with_locked_record("control", Duration::from_millis(250), |_guard| {
                store.with_locked_record("control", Duration::from_millis(250), |inner| {
                    inner.current_state()
                })
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::LockTimeout);
    }

    #[test]
    fn the_lock_targets_a_stable_sidecar_not_the_record() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        assert_ne!(store.lock_path("control"), store.record_path("control"));
        assert!(store.lock_path("control").ends_with("control.lock"));

        // The record's inode changes on every write; the sidecar's does not.
        store
            .compare_exchange("control", &ExpectedRecordState::Absent, &control(0, "one"))
            .unwrap();
        let sidecar_before = std::fs::metadata(store.lock_path("control")).unwrap();
        let expected = ExpectedRecordState::of(&control(0, "one")).unwrap();
        store
            .compare_exchange("control", &expected, &control(1, "two"))
            .unwrap();
        let sidecar_after = std::fs::metadata(store.lock_path("control")).unwrap();
        assert_eq!(sidecar_before.len(), sidecar_after.len());
    }

    #[test]
    fn concurrent_writers_serialize_and_exactly_one_wins_each_generation() {
        // Eight threads racing the same record. Serialization plus the
        // expected-state check means every successful write advanced the
        // generation by one and nothing was lost.
        let directory = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(store(&directory));
        store
            .compare_exchange("control", &ExpectedRecordState::Absent, &control(0, "seed"))
            .unwrap();

        let mut handles = Vec::new();
        for worker in 0..8 {
            let store = std::sync::Arc::clone(&store);
            handles.push(std::thread::spawn(move || {
                // Retry against the authoritative state, as a real caller does.
                for _ in 0..20 {
                    let outcome =
                        store.with_locked_record("control", Duration::from_secs(10), |guard| {
                            let current = guard.current_state()?;
                            let generation = match &current {
                                ExpectedRecordState::Absent => 0,
                                ExpectedRecordState::Present { generation, .. } => generation + 1,
                            };
                            guard.compare_exchange_locked(
                                &current,
                                &control(generation, &format!("worker-{worker}")),
                            )
                        });
                    if outcome.is_ok() {
                        return;
                    }
                }
                panic!("worker {worker} never committed");
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        // Eight commits on top of the seed, each advancing exactly one.
        assert_eq!(
            store.read_unlocked("control").unwrap().unwrap().generation,
            8
        );
    }

    #[test]
    fn a_corrupt_record_is_reported_rather_than_treated_as_absent() {
        // Treating unreadable bytes as "no record" would let a mutation
        // expecting `Absent` succeed and silently destroy the real value.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        std::fs::write(store.record_path("control"), b"{not json").unwrap();
        let error = store.read_unlocked("control").unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }
}
