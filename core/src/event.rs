//! Canonical v0.3.2 event engine (PRD §9.9, TDD §13).
//!
//! The event log is the append-only, hash-chained, indexed, tamper-detectable
//! record of trust-relevant actions, stored at `.draft/events/event.log`
//! (JSON-lines) with a companion `.draft/events/event.index` for fast lookup by
//! id/type/subject/receipt/time. Each entry's `event_hash` covers its immutable
//! content (everything except `event_hash`) chained to `previous_event_hash`, so
//! any edit to history is detectable.

use crate::error::{DraftError, DraftResult};
use crate::fsutil;
use crate::hashing;
use crate::layout::ProjectPaths;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Write;

/// The genesis previous-hash for the first event in a chain.
pub const GENESIS_HASH: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// The ten trust-relevant event types that also produce signed receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    CheckpointCreated,
    PackCreated,
    PackVerified,
    PackApproved,
    PackRejected,
    PackSaved,
    PackExported,
    PackImported,
    PackComposed,
    RollbackPerformed,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::CheckpointCreated => "CheckpointCreated",
            EventKind::PackCreated => "PackCreated",
            EventKind::PackVerified => "PackVerified",
            EventKind::PackApproved => "PackApproved",
            EventKind::PackRejected => "PackRejected",
            EventKind::PackSaved => "PackSaved",
            EventKind::PackExported => "PackExported",
            EventKind::PackImported => "PackImported",
            EventKind::PackComposed => "PackComposed",
            EventKind::RollbackPerformed => "RollbackPerformed",
        }
    }
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A canonical event record as persisted to `event.log`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventRecord {
    pub event_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub time: String,
    pub subject_id: Option<String>,
    pub actor_id: String,
    pub candidate_id: Option<String>,
    pub workspace_id: String,
    pub previous_event_hash: String,
    pub event_hash: String,
    pub receipt_id: Option<String>,
    pub metadata: serde_json::Value,
}

impl EventRecord {
    /// Recompute the content hash of this event (excludes only `event_hash`).
    pub fn recompute_hash(&self) -> String {
        let content = json!({
            "event_id": self.event_id,
            "type": self.event_type,
            "time": self.time,
            "subject_id": self.subject_id,
            "actor_id": self.actor_id,
            "candidate_id": self.candidate_id,
            "workspace_id": self.workspace_id,
            "previous_event_hash": self.previous_event_hash,
            "receipt_id": self.receipt_id,
            "metadata": self.metadata,
        });
        hashing::sha256_hex(hashing::canonical_json(&content).as_bytes())
    }
}

/// A compact index entry for fast lookups without parsing the whole log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub event_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub subject_id: Option<String>,
    pub receipt_id: Option<String>,
    pub time: String,
    pub event_hash: String,
}

/// Append-only event log bound to a project store.
pub struct EventLog {
    paths: ProjectPaths,
}

/// Inputs for appending a new event (hash + id are computed by the log).
pub struct NewEvent {
    pub kind: EventKind,
    pub subject_id: Option<String>,
    pub actor_id: String,
    pub candidate_id: Option<String>,
    pub workspace_id: String,
    pub receipt_id: Option<String>,
    pub metadata: serde_json::Value,
}

impl EventLog {
    pub fn new(paths: ProjectPaths) -> Self {
        EventLog { paths }
    }

    /// Read every event in chain order.
    pub fn read_all(&self) -> DraftResult<Vec<EventRecord>> {
        let path = self.paths.event_log();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| DraftError::storage(format!("read event.log: {e}")))?;
        let mut out = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let rec: EventRecord = serde_json::from_str(line).map_err(|e| {
                DraftError::new(
                    crate::error::DraftErrorKind::OperationLogCorrupt,
                    format!("event.log line {} is corrupt: {e}", i + 1),
                )
            })?;
            out.push(rec);
        }
        Ok(out)
    }

    /// The hash of the most recent event, or the genesis hash if empty.
    pub fn last_hash(&self) -> DraftResult<String> {
        Ok(self
            .read_all()?
            .last()
            .map(|e| e.event_hash.clone())
            .unwrap_or_else(|| GENESIS_HASH.to_string()))
    }

    /// Append a new event, returning the persisted record. Trust-ledger callers
    /// pass a preallocated receipt id so the event line is final when written.
    pub fn append(&self, new: NewEvent) -> DraftResult<EventRecord> {
        fsutil::ensure_dir(&self.paths.events_dir())?;
        let previous_event_hash = self.last_hash()?;
        let event_id = format!("evt_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        let time = crate::common::now().to_rfc3339();
        let mut rec = EventRecord {
            event_id,
            event_type: new.kind.as_str().to_string(),
            time,
            subject_id: new.subject_id,
            actor_id: new.actor_id,
            candidate_id: new.candidate_id,
            workspace_id: new.workspace_id,
            previous_event_hash,
            event_hash: String::new(),
            receipt_id: new.receipt_id,
            metadata: new.metadata,
        };
        rec.event_hash = rec.recompute_hash();
        self.write_line(&rec)?;
        self.reindex()?;
        Ok(rec)
    }

    /// Verify the full hash chain: every recomputed hash matches, and each links
    /// to its predecessor. Returns the number of verified events.
    pub fn verify_chain(&self) -> DraftResult<usize> {
        let all = self.read_all()?;
        let mut prev = GENESIS_HASH.to_string();
        for (i, rec) in all.iter().enumerate() {
            if rec.previous_event_hash != prev {
                return Err(DraftError::new(
                    crate::error::DraftErrorKind::OperationLogCorrupt,
                    format!("event {} breaks the chain (bad previous hash)", i + 1),
                ));
            }
            if rec.recompute_hash() != rec.event_hash {
                return Err(DraftError::new(
                    crate::error::DraftErrorKind::OperationLogCorrupt,
                    format!("event {} has a tampered hash", i + 1),
                ));
            }
            prev = rec.event_hash.clone();
        }
        Ok(all.len())
    }

    /// Load the persisted index (rebuilding lazily if absent).
    pub fn index(&self) -> DraftResult<Vec<IndexEntry>> {
        let path = self.paths.event_index();
        if !path.exists() {
            return self.reindex().and_then(|_| self.index());
        }
        fsutil::read_json::<Vec<IndexEntry>>(&path)
    }

    fn write_line(&self, rec: &EventRecord) -> DraftResult<()> {
        let path = self.paths.event_log();
        let line = serde_json::to_string(rec)
            .map_err(|e| DraftError::storage(format!("serialize event: {e}")))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| DraftError::storage(format!("open event.log for append: {e}")))?;
        file.write_all(line.as_bytes())
            .map_err(|e| DraftError::storage(format!("append event.log: {e}")))?;
        file.write_all(b"\n")
            .map_err(|e| DraftError::storage(format!("append event.log newline: {e}")))?;
        file.sync_data()
            .map_err(|e| DraftError::storage(format!("sync event.log: {e}")))
    }

    fn reindex(&self) -> DraftResult<()> {
        let all = self.read_all()?;
        let entries: Vec<IndexEntry> = all
            .into_iter()
            .map(|e| IndexEntry {
                event_id: e.event_id,
                event_type: e.event_type,
                subject_id: e.subject_id,
                receipt_id: e.receipt_id,
                time: e.time,
                event_hash: e.event_hash,
            })
            .collect();
        fsutil::write_json(&self.paths.event_index(), &entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(root: &std::path::Path) -> EventLog {
        EventLog::new(ProjectPaths::for_root(root))
    }

    fn sample(kind: EventKind, subject: &str) -> NewEvent {
        NewEvent {
            kind,
            subject_id: Some(subject.to_string()),
            actor_id: "act_test".to_string(),
            candidate_id: None,
            workspace_id: "ws_test".to_string(),
            receipt_id: None,
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn append_chains_and_verifies() {
        let tmp = tempfile::tempdir().unwrap();
        let l = log(tmp.path());
        let e1 = l.append(sample(EventKind::PackCreated, "pck_a")).unwrap();
        assert_eq!(e1.previous_event_hash, GENESIS_HASH);
        let e2 = l.append(sample(EventKind::PackVerified, "pck_a")).unwrap();
        assert_eq!(e2.previous_event_hash, e1.event_hash);
        assert_eq!(l.verify_chain().unwrap(), 2);
        assert_eq!(l.index().unwrap().len(), 2);
    }

    #[test]
    fn tampering_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let l = log(tmp.path());
        l.append(sample(EventKind::PackCreated, "pck_a")).unwrap();
        l.append(sample(EventKind::PackSaved, "pck_a")).unwrap();
        // Corrupt the first line's metadata directly.
        let path = l.paths.event_log();
        let text = std::fs::read_to_string(&path).unwrap();
        let tampered = text.replacen("\"metadata\":{}", "\"metadata\":{\"x\":1}", 1);
        std::fs::write(&path, tampered).unwrap();
        assert!(l.verify_chain().is_err());
    }

    #[test]
    fn receipt_id_is_part_of_the_appended_event_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let l = log(tmp.path());
        let mut new = sample(EventKind::PackApproved, "pck_a");
        new.receipt_id = Some("rcp_123".to_string());
        let e = l.append(new).unwrap();
        assert_eq!(l.verify_chain().unwrap(), 1);
        assert_eq!(e.receipt_id.as_deref(), Some("rcp_123"));
        assert_eq!(
            l.read_all().unwrap()[0].receipt_id.as_deref(),
            Some("rcp_123")
        );
        let mut tampered = l.read_all().unwrap()[0].clone();
        tampered.receipt_id = Some("rcp_other".to_string());
        assert_ne!(tampered.recompute_hash(), e.event_hash);
    }
}
