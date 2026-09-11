//! The append-only Activity log.
//!
//! # The append protocol
//!
//! ```text
//! append(event_id, payload):
//!     acquire events/events.lock                 (a correctness lock, not a lease)
//!     read the authoritative tail
//!     event_id present, payload identical  -> idempotent success, no second record
//!     event_id present, payload differs    -> conflict; the ledger is not rewritten
//!     event_id absent                      -> link to the tail hash, frame, fsync, index
//!     release
//! ```
//!
//! Serialization is not optional here. Two appenders that both read the same
//! tail hash would produce two records claiming the same predecessor — a forked
//! chain, which no later verification could repair because both records are
//! individually well formed.
//!
//! # Why idempotence is keyed on the event id
//!
//! Audit facts are drained by recovery, which may run any number of times. The
//! event id is preallocated before the mutation it describes commits, so a
//! replay presents the *same* id — and the ledger recognises it and does
//! nothing. Without that, every crash between commit and drain would add a
//! duplicate event, and Activity would stop being a record of what happened.
//!
//! Presenting the same id with a *different* payload is the opposite case: two
//! irreconcilable claims about one event. That is a conflict, and the ledger
//! refuses rather than picking one.
//!
//! # Recovery
//!
//! `events/events.log` is the only authoritative file. `events/events.index` is
//! derived and rebuilt from it. Recovery truncates **only** a physically
//! incomplete final frame; a complete frame that fails verification is refused,
//! because deleting a committed record to make the file parse is exactly the
//! outcome a ledger exists to prevent.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::activity::frame::{encode_frame, scan, FrameScan};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::{canonical_json, domain_hash};
use crate::support::lock_order::LockOrder;
use crate::support::process_lock::ProcessFileLock;
use crate::support::telemetry::Counter;

/// The frozen domain separator for the ledger's genesis hash.
pub const GENESIS_DOMAIN: &str = "draft.activity.genesis/v1";
/// The frozen domain separator for a record's chain hash.
pub const RECORD_DOMAIN: &str = "draft.activity.record/v1";
/// How long an appender waits for the ledger lock.
pub const APPEND_TIMEOUT: Duration = Duration::from_secs(30);

/// One logical Activity record.
///
/// The chain hash and event identity are computed over this, never over the
/// frame that stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRecord {
    pub event_id: String,
    pub previous_hash: String,
    pub record_hash: String,
    pub payload: Value,
}

impl crate::contracts::VersionedContract for LedgerRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::ActivityRecord;
}

impl LedgerRecord {
    /// The hash this record's contents imply.
    pub fn recompute_hash(&self) -> String {
        domain_hash(
            RECORD_DOMAIN,
            [
                self.previous_hash.as_bytes(),
                self.event_id.as_bytes(),
                canonical_json(&self.payload).as_bytes(),
            ],
        )
    }
}

/// What an append did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendOutcome {
    /// A new record was linked onto the chain.
    Appended(LedgerRecord),
    /// This exact event was already recorded. Nothing was written.
    AlreadyPresent(LedgerRecord),
}

impl AppendOutcome {
    pub fn record(&self) -> &LedgerRecord {
        match self {
            Self::Appended(record) | Self::AlreadyPresent(record) => record,
        }
    }
}

/// What recovery found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Records readable after recovery.
    pub records: usize,
    /// Bytes discarded from an unfinished final frame, if any.
    pub truncated_bytes: u64,
    /// Why the tail was truncated, when it was.
    pub reason: Option<String>,
}

/// The append-only Activity log for one ledger.
#[derive(Debug, Clone)]
pub struct ActivityLog {
    log_path: PathBuf,
    lock_path: PathBuf,
    index_path: PathBuf,
    ledger_id: String,
}

impl ActivityLog {
    /// Open the log in `events_dir`.
    ///
    /// The authoritative file is `events.log`, not `events.jsonl`: a frame is
    /// not a JSON line, and a suffix that promised otherwise would mislead
    /// every tool that met the file.
    pub fn new(events_dir: impl AsRef<Path>, ledger_id: impl Into<String>) -> Self {
        let directory = events_dir.as_ref();
        Self {
            log_path: directory.join("events.log"),
            lock_path: directory.join("events.lock"),
            index_path: directory.join("events.index"),
            ledger_id: ledger_id.into(),
        }
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    pub fn index_path(&self) -> &Path {
        &self.index_path
    }

    /// The hash an empty chain starts from.
    ///
    /// Bound to the ledger's identity, so a record from one ledger cannot be
    /// replayed into another and still verify.
    pub fn genesis_hash(&self) -> String {
        domain_hash(GENESIS_DOMAIN, [self.ledger_id.as_bytes()])
    }

    /// Every record, refusing to return anything if the log is corrupt.
    ///
    /// A torn tail is reported rather than silently ignored: reading is not
    /// where history gets rewritten, so the caller must ask for recovery
    /// explicitly.
    pub fn read_all(&self) -> DraftResult<Vec<LedgerRecord>> {
        let bytes = self.read_bytes()?;
        match scan(&bytes) {
            FrameScan::Complete { records } => self.decode_and_verify(&records),
            FrameScan::TornTail { reason, .. } => Err(DraftError::new(
                DraftErrorKind::OperationLogCorrupt,
                format!("the activity log has an unfinished final frame: {reason}"),
            )
            .with_suggestion("Run recovery to discard the incomplete tail.")),
            FrameScan::HardCorruption { offset, reason } => Err(DraftError::new(
                DraftErrorKind::OperationLogCorrupt,
                format!("the activity log is damaged at byte {offset}: {reason}"),
            )
            .with_suggestion("Run `draft doctor`; a committed record is never truncated away.")),
        }
    }

    /// The tail hash a new record links onto.
    pub fn tail_hash(&self) -> DraftResult<String> {
        Ok(self
            .read_all()?
            .last()
            .map(|record| record.record_hash.clone())
            .unwrap_or_else(|| self.genesis_hash()))
    }

    /// Append `payload` under `event_id`, or recognise it as already recorded.
    pub fn append(&self, event_id: &str, payload: &Value) -> DraftResult<AppendOutcome> {
        if let Some(parent) = self.log_path.parent() {
            crate::support::fsutil::ensure_dir(parent)?;
        }
        // Timed around the acquisition rather than counted as a boolean:
        // "somebody waited" is true on every busy system, and how long they
        // waited is the number that tells contention from a stuck holder.
        let waiting = std::time::Instant::now();
        let _lock = ProcessFileLock::acquire_exclusive_ordered(
            &self.lock_path,
            APPEND_TIMEOUT,
            LockOrder::ActivityLedger,
        )?;
        Counter::ActivityAppendContention.add(waiting.elapsed().as_micros() as u64);

        // Authoritative read, inside the critical section. A tail read before
        // the lock could already be stale.
        let existing = self.read_all()?;

        if let Some(recorded) = existing.iter().find(|record| record.event_id == event_id) {
            if canonical_json(&recorded.payload) == canonical_json(payload) {
                return Ok(AppendOutcome::AlreadyPresent(recorded.clone()));
            }
            return Err(DraftError::new(
                DraftErrorKind::OperationLogCorrupt,
                format!(
                    "activity event '{event_id}' is already recorded with a different payload; \
                     the ledger is never rewritten to resolve this"
                ),
            )
            .with_suggestion("Run `draft doctor`; two claims about one event need recovery."));
        }

        let previous_hash = existing
            .last()
            .map(|record| record.record_hash.clone())
            .unwrap_or_else(|| self.genesis_hash());
        let mut record = LedgerRecord {
            event_id: event_id.to_string(),
            previous_hash,
            record_hash: String::new(),
            payload: payload.clone(),
        };
        record.record_hash = record.recompute_hash();

        let canonical = canonical_json(&serde_json::to_value(&record).map_err(|error| {
            DraftError::storage(format!("cannot encode activity record: {error}"))
        })?);
        let framed = encode_frame(canonical.as_bytes())?;

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .map_err(|error| {
                DraftError::storage(format!("cannot open the activity log: {error}"))
            })?;
        file.write_all(&framed).map_err(|error| {
            DraftError::storage(format!("cannot append to the activity log: {error}"))
        })?;
        file.sync_all().map_err(|error| {
            DraftError::storage(format!("cannot sync the activity log: {error}"))
        })?;

        self.write_index(existing.len() + 1)?;
        Ok(AppendOutcome::Appended(record))
    }

    /// Discard an unfinished final frame, if there is one.
    ///
    /// Refuses to touch a log whose damage is not a torn write.
    pub fn recover(&self) -> DraftResult<RecoveryReport> {
        let _lock = ProcessFileLock::acquire_exclusive_ordered(
            &self.lock_path,
            APPEND_TIMEOUT,
            LockOrder::ActivityLedger,
        )?;
        let bytes = self.read_bytes()?;
        match scan(&bytes) {
            FrameScan::Complete { records } => {
                let decoded = self.decode_and_verify(&records)?;
                self.write_index(decoded.len())?;
                Ok(RecoveryReport {
                    records: decoded.len(),
                    truncated_bytes: 0,
                    reason: None,
                })
            }
            FrameScan::TornTail {
                records,
                valid_length,
                reason,
            } => {
                let decoded = self.decode_and_verify(&records)?;
                let discarded = bytes.len() as u64 - valid_length;
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&self.log_path)
                    .map_err(|error| {
                        DraftError::storage(format!("cannot open the activity log: {error}"))
                    })?;
                file.set_len(valid_length).map_err(|error| {
                    DraftError::storage(format!("cannot truncate the activity log: {error}"))
                })?;
                file.sync_all().map_err(|error| {
                    DraftError::storage(format!("cannot sync the activity log: {error}"))
                })?;
                self.write_index(decoded.len())?;
                Counter::ActivityTornTailTruncations.increment();
                Ok(RecoveryReport {
                    records: decoded.len(),
                    truncated_bytes: discarded,
                    reason: Some(reason),
                })
            }
            FrameScan::HardCorruption { offset, reason } => {
                Counter::ActivityHardCorruptions.increment();
                Err(DraftError::new(
                    DraftErrorKind::OperationLogCorrupt,
                    format!(
                        "the activity log is damaged at byte {offset}: {reason}. A committed \
                         record is never truncated away to make the log parse."
                    ),
                )
                .with_suggestion("Run `draft doctor`."))
            }
        }
    }

    /// Verify the hash chain end to end.
    pub fn verify_chain(&self) -> DraftResult<usize> {
        let records = self.read_all()?;
        let mut expected = self.genesis_hash();
        for (position, record) in records.iter().enumerate() {
            if record.previous_hash != expected {
                Counter::ActivityChainVerifyFailures.increment();
                return Err(DraftError::new(
                    DraftErrorKind::OperationLogCorrupt,
                    format!(
                        "activity record {} does not link to its predecessor",
                        position + 1
                    ),
                ));
            }
            expected = record.record_hash.clone();
        }
        Ok(records.len())
    }

    fn read_bytes(&self) -> DraftResult<Vec<u8>> {
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }
        std::fs::read(&self.log_path)
            .map_err(|error| DraftError::storage(format!("cannot read the activity log: {error}")))
    }

    fn decode_and_verify(&self, records: &[Vec<u8>]) -> DraftResult<Vec<LedgerRecord>> {
        let mut decoded = Vec::with_capacity(records.len());
        for (position, bytes) in records.iter().enumerate() {
            let record: LedgerRecord = serde_json::from_slice(bytes).map_err(|error| {
                DraftError::new(
                    DraftErrorKind::OperationLogCorrupt,
                    format!("activity record {} is unreadable: {error}", position + 1),
                )
            })?;
            // The frame checksum catches physical damage; this catches a
            // record whose contents no longer imply the hash it carries.
            if record.recompute_hash() != record.record_hash {
                return Err(DraftError::new(
                    DraftErrorKind::OperationLogCorrupt,
                    format!(
                        "activity record {} carries a hash its contents do not produce",
                        position + 1
                    ),
                ));
            }
            decoded.push(record);
        }
        Ok(decoded)
    }

    fn write_index(&self, count: usize) -> DraftResult<()> {
        // Derived state, rebuildable from the log at any time. It is written
        // after the log so it can never describe a record the log does not have.
        let index = serde_json::json!({
            "ledger": self.ledger_id,
            "records": count,
        });
        crate::support::fsutil::write_atomic(
            &self.index_path,
            serde_json::to_string_pretty(&index)
                .map_err(|error| DraftError::storage(format!("cannot encode index: {error}")))?
                .as_bytes(),
        )
    }
}

/// Preallocate a deterministic event id for `seed`.
///
/// Deterministic rather than random so a recovery that recomputes the same
/// transaction recomputes the same id, and the idempotent append converges
/// instead of writing a second record. A caller with a durable journal
/// preallocates through that instead.
pub fn event_id_for(seed: &str) -> String {
    let digest = draft_dcg_contract::Digest::of_bytes(format!("activity|{seed}").as_bytes());
    let hex: String = digest
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect();
    format!("evt_{hex}")
}

/// Read-only access to Activity.
///
/// Read models and Doctor take this rather than an [`ActivityLog`], so nothing
/// outside the single append orchestrator can write an event by accident.
pub trait ActivityReader {
    fn records(&self) -> DraftResult<Vec<LedgerRecord>>;
}

impl ActivityReader for ActivityLog {
    fn records(&self) -> DraftResult<Vec<LedgerRecord>> {
        self.read_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn log(directory: &tempfile::TempDir) -> ActivityLog {
        ActivityLog::new(directory.path(), "prj_000000000001")
    }

    fn payload(kind: &str) -> Value {
        json!({ "kind": kind })
    }

    #[test]
    fn an_empty_ledger_starts_from_its_genesis_hash() {
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        assert!(log.read_all().unwrap().is_empty());
        assert_eq!(log.tail_hash().unwrap(), log.genesis_hash());
    }

    #[test]
    fn a_genesis_hash_is_bound_to_its_ledger() {
        // So a record cannot be lifted from one project's ledger into another
        // and still verify.
        let directory = tempfile::tempdir().unwrap();
        let mine = ActivityLog::new(directory.path(), "prj_mine");
        let theirs = ActivityLog::new(directory.path(), "prj_theirs");
        assert_ne!(mine.genesis_hash(), theirs.genesis_hash());
    }

    #[test]
    fn appended_records_form_a_chain() {
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        log.append("evt_1", &payload("ProjectCreated")).unwrap();
        log.append("evt_2", &payload("ChangeCreated")).unwrap();
        log.append("evt_3", &payload("ChangeCompleted")).unwrap();

        assert_eq!(log.verify_chain().unwrap(), 3);
        let records = log.read_all().unwrap();
        assert_eq!(records[0].previous_hash, log.genesis_hash());
        assert_eq!(records[1].previous_hash, records[0].record_hash);
        assert_eq!(records[2].previous_hash, records[1].record_hash);
    }

    #[test]
    fn replaying_the_same_event_appends_nothing() {
        // Recovery drains an audit fact however many times it runs; the
        // preallocated id is what makes the second drain a no-op.
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        let first = log.append("evt_1", &payload("ChangeCreated")).unwrap();
        assert!(matches!(first, AppendOutcome::Appended(_)));

        for _ in 0..3 {
            let replay = log.append("evt_1", &payload("ChangeCreated")).unwrap();
            assert!(matches!(replay, AppendOutcome::AlreadyPresent(_)));
            assert_eq!(replay.record(), first.record());
        }
        assert_eq!(log.read_all().unwrap().len(), 1);
    }

    #[test]
    fn payload_identity_ignores_field_order() {
        // Idempotence must survive a serializer that orders keys differently,
        // or a replay would look like a conflicting claim.
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        log.append("evt_1", &json!({"a": 1, "b": 2})).unwrap();
        let replay = log.append("evt_1", &json!({"b": 2, "a": 1})).unwrap();
        assert!(matches!(replay, AppendOutcome::AlreadyPresent(_)));
    }

    #[test]
    fn the_same_id_with_a_different_payload_is_refused() {
        // Two irreconcilable claims about one event. Picking either would make
        // the ledger a summary rather than a record.
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        log.append("evt_1", &payload("ChangeCreated")).unwrap();
        let error = log
            .append("evt_1", &payload("ChangeAbandoned"))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::OperationLogCorrupt);
        assert_eq!(log.read_all().unwrap().len(), 1, "nothing was rewritten");
    }

    #[test]
    fn concurrent_appenders_do_not_fork_the_chain() {
        // The reason the append is serialized. Two appenders reading the same
        // tail hash would each produce a well-formed record claiming the same
        // predecessor, and no later check could repair it.
        //
        // What is asserted is chain integrity, not that every thread wins its
        // race within a deadline. Acquisition is not FIFO-fair, so under heavy
        // load a waiter can legitimately time out — that is contention, not a
        // fork, and treating it as a failure would make this test measure the
        // machine rather than the invariant.
        let directory = tempfile::tempdir().unwrap();
        let log = std::sync::Arc::new(log(&directory));
        let mut handles = Vec::new();
        for worker in 0..6 {
            let log = std::sync::Arc::clone(&log);
            handles.push(std::thread::spawn(move || {
                match log.append(&format!("evt_{worker}"), &payload("Concurrent")) {
                    Ok(_) => true,
                    Err(error) if error.kind == DraftErrorKind::LockTimeout => false,
                    Err(error) => panic!("unexpected append failure: {error}"),
                }
            }));
        }
        let appended = handles
            .into_iter()
            .filter(|_| true)
            .map(|handle| handle.join().unwrap())
            .filter(|committed| *committed)
            .count();

        // However many got through, the chain is intact and holds exactly
        // those records — never a fork, and never a partial write.
        assert_eq!(log.verify_chain().unwrap(), appended);
        assert!(appended > 0, "no appender made progress at all");
    }

    #[test]
    fn an_unfinished_final_frame_is_recovered_and_earlier_records_survive() {
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        log.append("evt_1", &payload("ChangeCreated")).unwrap();
        log.append("evt_2", &payload("ChangeCompleted")).unwrap();

        // Simulate a crash mid-append.
        let mut bytes = std::fs::read(log.log_path()).unwrap();
        bytes.extend_from_slice(b"\x44\x52\x46\x54");
        std::fs::write(log.log_path(), &bytes).unwrap();

        assert!(log.read_all().is_err(), "reading must not hide a torn tail");

        let report = log.recover().unwrap();
        assert_eq!(report.records, 2);
        assert_eq!(report.truncated_bytes, 4);
        assert!(report.reason.is_some());
        assert_eq!(log.verify_chain().unwrap(), 2);

        // And the log is writable again.
        log.append("evt_3", &payload("Resumed")).unwrap();
        assert_eq!(log.verify_chain().unwrap(), 3);
    }

    #[test]
    fn a_damaged_committed_record_is_never_truncated_away() {
        // The case the classification exists for. Recovery must refuse rather
        // than delete history to make the file parse.
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        log.append("evt_1", &payload("ChangeCreated")).unwrap();
        log.append("evt_2", &payload("ChangeCompleted")).unwrap();

        let mut bytes = std::fs::read(log.log_path()).unwrap();
        let midpoint = bytes.len() - 20;
        bytes[midpoint] ^= 0xff;
        std::fs::write(log.log_path(), &bytes).unwrap();

        let error = log.recover().unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::OperationLogCorrupt);
        assert_eq!(
            std::fs::read(log.log_path()).unwrap().len(),
            bytes.len(),
            "a refused recovery must not have altered the log"
        );
    }

    #[test]
    fn a_record_whose_contents_no_longer_imply_its_hash_is_refused() {
        // Physical damage is the frame's job; this catches a record that was
        // re-framed correctly around altered contents.
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        log.append("evt_1", &payload("ChangeCreated")).unwrap();

        let mut record = log.read_all().unwrap().remove(0);
        record.payload = payload("ChangeAbandoned");
        let canonical = canonical_json(&serde_json::to_value(&record).unwrap());
        std::fs::write(log.log_path(), encode_frame(canonical.as_bytes()).unwrap()).unwrap();

        let error = log.read_all().unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::OperationLogCorrupt);
    }

    #[test]
    fn the_index_is_derived_and_rebuilt_from_the_log() {
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        log.append("evt_1", &payload("ChangeCreated")).unwrap();
        assert!(log.index_path().exists());

        std::fs::remove_file(log.index_path()).unwrap();
        // Losing the index loses nothing: the log is authoritative.
        assert_eq!(log.read_all().unwrap().len(), 1);
        log.recover().unwrap();
        assert!(log.index_path().exists());
    }

    #[test]
    fn the_authoritative_file_is_events_log() {
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        log.append("evt_1", &payload("ChangeCreated")).unwrap();
        assert!(log.log_path().ends_with("events.log"));
        // retired-architecture-ok: proving the old name is absent must name it.
        assert!(!directory.path().join("events.jsonl").exists());
    }

    #[test]
    fn a_reader_sees_records_without_being_able_to_append() {
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        log.append("evt_1", &payload("ChangeCreated")).unwrap();
        let reader: &dyn ActivityReader = &log;
        assert_eq!(reader.records().unwrap().len(), 1);
    }
}
