//! Canonical ledger-scoped event chain for workspace and system audit events.

use crate::support::common::WorkspaceId;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil;
use crate::support::hashing::{canonical_json, domain_hash};
use crate::support::lock::FileGuard;
use crate::support::redaction::redact_value;
use crate::trust::identity::resolve_actor;
use crate::workspace::layout::DraftLayout;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

const EVENT_CHAIN_DOMAIN: &str = "draft-event-chain";
const EVENT_GENESIS_DOMAIN: &str = "draft-event-chain-genesis";

crate::id_newtype!(EventId, "evt_");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashChainStatus {
    pub ok: bool,
    pub events: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventReplayReport {
    pub workspace_id: String,
    pub events: usize,
    pub by_type: BTreeMap<String, usize>,
    pub chain_ok: bool,
    pub error: Option<String>,
}

/// Workspace-scoped access to the canonical event ledger.
pub(crate) struct WorkspaceEventLog {
    layout: DraftLayout,
    log: EventLog,
}

impl WorkspaceEventLog {
    pub(crate) fn new(layout: DraftLayout, workspace_id: WorkspaceId) -> Self {
        let log = EventLog::workspace(layout.clone(), workspace_id.to_string());
        Self { layout, log }
    }

    pub(crate) fn append(
        &self,
        event_type: &str,
        subject_id: Option<String>,
        metadata: Value,
    ) -> DraftResult<EventId> {
        let actor = resolve_actor(&self.layout.draft_dir)?;
        let event = self.log.append_named(
            event_type,
            subject_id,
            actor.id.to_string(),
            None,
            None,
            redact_value(metadata),
        )?;
        Ok(EventId::new(event.event_id))
    }

    pub(crate) fn read_all(&self) -> DraftResult<Vec<EventRecord>> {
        self.log.read_all()
    }

    pub(crate) fn read_page(
        &self,
        top: bool,
        bottom: bool,
        page: Option<usize>,
        limit: Option<usize>,
        filter: Option<&str>,
    ) -> DraftResult<Vec<EventRecord>> {
        let mut events = self.log.read_all()?;
        if let Some(filter) = filter {
            events.retain(|event| {
                event.event_type.contains(filter)
                    || event
                        .subject_id
                        .as_deref()
                        .is_some_and(|subject| subject.contains(filter))
            });
        }
        if bottom || !top {
            events.reverse();
        }
        let limit = limit.unwrap_or(5);
        let skip = page.unwrap_or(1).saturating_sub(1).saturating_mul(limit);
        Ok(events.into_iter().skip(skip).take(limit).collect())
    }

    pub(crate) fn verify_chain(&self) -> DraftResult<HashChainStatus> {
        let events = self.log.verify_chain()?;
        Ok(HashChainStatus {
            ok: true,
            events,
            error: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LedgerIdentity {
    Workspace { workspace_id: String },
    System { ledger_id: String },
}

impl LedgerIdentity {
    pub fn workspace_id(&self) -> Option<&str> {
        match self {
            Self::Workspace { workspace_id } => Some(workspace_id),
            Self::System { .. } => None,
        }
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        canonical_json(&serde_json::to_value(self).expect("ledger identity")).into_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    InitStarted,
    InitialStableBaseCreated,
    CheckpointCreated,
    PackCreated,
    PackVerified,
    PackAccepted,
    PackApproved,
    PackRejected,
    PackReopened,
    PackSubmitted,
    CompositionCreated,
    CompositionVerified,
    CompositionFailed,
    SubmitStarted,
    SubmitHookStarted,
    SubmitHookCompleted,
    SubmitHookFailed,
    ProjectStateVerificationStarted,
    ProjectStateVerified,
    ProjectStateVerificationFailed,
    StableHeadAdvanced,
    SubmitFinalized,
    PackDisposed,
    PackDisposalFailed,
    PackExported,
    PackImported,
    PackComposed,
    PackDispersed,
    RiskAssessed,
    ReviewCompleted,
    SubmitCompleted,
    RollbackPerformed,
    CloseStarted,
    CloseCompleted,
    CloseFailed,
    GcStarted,
    GcCompleted,
    GcFailed,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InitStarted => "InitStarted",
            Self::InitialStableBaseCreated => "InitialStableBaseCreated",
            Self::CheckpointCreated => "CheckpointCreated",
            Self::PackCreated => "PackCreated",
            Self::PackVerified => "PackVerified",
            Self::PackAccepted => "PackAccepted",
            Self::PackApproved => "PackApproved",
            Self::PackRejected => "PackRejected",
            Self::PackReopened => "PackReopened",
            Self::PackSubmitted => "PackSubmitted",
            Self::CompositionCreated => "CompositionCreated",
            Self::CompositionVerified => "CompositionVerified",
            Self::CompositionFailed => "CompositionFailed",
            Self::SubmitStarted => "SubmitStarted",
            Self::SubmitHookStarted => "SubmitHookStarted",
            Self::SubmitHookCompleted => "SubmitHookCompleted",
            Self::SubmitHookFailed => "SubmitHookFailed",
            Self::ProjectStateVerificationStarted => "ProjectStateVerificationStarted",
            Self::ProjectStateVerified => "ProjectStateVerified",
            Self::ProjectStateVerificationFailed => "ProjectStateVerificationFailed",
            Self::StableHeadAdvanced => "StableHeadAdvanced",
            Self::SubmitFinalized => "SubmitFinalized",
            Self::PackDisposed => "PackDisposed",
            Self::PackDisposalFailed => "PackDisposalFailed",
            Self::PackExported => "PackExported",
            Self::PackImported => "PackImported",
            Self::PackComposed => "PackComposed",
            Self::PackDispersed => "PackDispersed",
            Self::RiskAssessed => "RiskAssessed",
            Self::ReviewCompleted => "ReviewCompleted",
            Self::SubmitCompleted => "SubmitCompleted",
            Self::RollbackPerformed => "RollbackPerformed",
            Self::CloseStarted => "CloseStarted",
            Self::CloseCompleted => "CloseCompleted",
            Self::CloseFailed => "CloseFailed",
            Self::GcStarted => "GcStarted",
            Self::GcCompleted => "GcCompleted",
            Self::GcFailed => "GcFailed",
        }
    }
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EventRecord {
    pub schema_version: u32,
    pub ledger: LedgerIdentity,
    pub event_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub time: String,
    pub subject_id: Option<String>,
    pub actor_id: String,
    pub candidate_id: Option<String>,
    pub previous_event_hash: String,
    pub event_hash: String,
    pub receipt_id: Option<String>,
    pub metadata: Value,
}

impl crate::contracts::VersionedContract for EventRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::EventRecord;
}

impl EventRecord {
    fn content_bytes(&self) -> Vec<u8> {
        canonical_json(&serde_json::json!({
            "schema_version": self.schema_version,
            "event_id": self.event_id,
            "type": self.event_type,
            "time": self.time,
            "subject_id": self.subject_id,
            "actor_id": self.actor_id,
            "candidate_id": self.candidate_id,
            "receipt_id": self.receipt_id,
            "metadata": self.metadata,
        }))
        .into_bytes()
    }

    pub fn recompute_hash(&self) -> String {
        let ledger = self.ledger.canonical_bytes();
        let content = self.content_bytes();
        domain_hash(
            EVENT_CHAIN_DOMAIN,
            [
                ledger.as_slice(),
                self.previous_event_hash.as_bytes(),
                content.as_slice(),
            ],
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventIndexEntry {
    pub event_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub subject_id: Option<String>,
    pub receipt_id: Option<String>,
    pub time: String,
    pub event_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EventIndex {
    schema_version: u32,
    ledger: LedgerIdentity,
    entries: Vec<EventIndexEntry>,
}

impl crate::contracts::VersionedContract for EventIndex {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::EventIndex;
}

pub struct EventLog {
    log_path: PathBuf,
    index_path: PathBuf,
    lock_path: PathBuf,
    ledger: LedgerIdentity,
}

pub struct NewEvent {
    pub kind: EventKind,
    pub subject_id: Option<String>,
    pub actor_id: String,
    pub candidate_id: Option<String>,
    pub receipt_id: Option<String>,
    pub metadata: Value,
}

impl EventLog {
    pub fn workspace(paths: DraftLayout, workspace_id: impl Into<String>) -> Self {
        Self {
            log_path: paths.event_log(),
            index_path: paths.event_index(),
            lock_path: paths.events_dir().join("event.lock"),
            ledger: LedgerIdentity::Workspace {
                workspace_id: workspace_id.into(),
            },
        }
    }

    pub fn system(path: impl Into<PathBuf>, ledger_id: impl Into<String>) -> Self {
        let log_path = path.into();
        Self {
            index_path: log_path.with_extension("index.json"),
            lock_path: log_path.with_extension("lock"),
            log_path,
            ledger: LedgerIdentity::System {
                ledger_id: ledger_id.into(),
            },
        }
    }

    pub fn ledger(&self) -> &LedgerIdentity {
        &self.ledger
    }

    pub fn genesis_hash(&self) -> String {
        let ledger = self.ledger.canonical_bytes();
        domain_hash(EVENT_GENESIS_DOMAIN, [ledger.as_slice()])
    }

    pub fn read_all(&self) -> DraftResult<Vec<EventRecord>> {
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&self.log_path)
            .map_err(|error| DraftError::storage(format!("read event ledger: {error}")))?;
        text.lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(index, line)| {
                let record: EventRecord = crate::contracts::decode_persisted(line.as_bytes())
                    .map_err(|error| {
                        error.with_context(format!("event ledger line {}", index + 1))
                    })?;
                if record.ledger != self.ledger {
                    return Err(corrupt(format!(
                        "event {} belongs to a different ledger",
                        index + 1
                    )));
                }
                Ok(record)
            })
            .collect()
    }

    pub fn last_hash(&self) -> DraftResult<String> {
        Ok(self
            .read_all()?
            .last()
            .map(|event| event.event_hash.clone())
            .unwrap_or_else(|| self.genesis_hash()))
    }

    pub fn append(&self, event: NewEvent) -> DraftResult<EventRecord> {
        self.append_named(
            event.kind.as_str(),
            event.subject_id,
            event.actor_id,
            event.candidate_id,
            event.receipt_id,
            event.metadata,
        )
    }

    pub fn append_named(
        &self,
        event_type: impl Into<String>,
        subject_id: Option<String>,
        actor_id: impl Into<String>,
        candidate_id: Option<String>,
        receipt_id: Option<String>,
        metadata: Value,
    ) -> DraftResult<EventRecord> {
        if let Some(parent) = self.log_path.parent() {
            fsutil::ensure_dir(parent)?;
        }
        let _guard = FileGuard::acquire(&self.lock_path, Duration::from_secs(5))?;
        let previous_event_hash = self.last_hash()?;
        let mut record = EventRecord {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::EventRecord,
            ),
            ledger: self.ledger.clone(),
            event_id: format!("evt_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]),
            event_type: event_type.into(),
            time: crate::support::common::now().to_rfc3339(),
            subject_id,
            actor_id: actor_id.into(),
            candidate_id,
            previous_event_hash,
            event_hash: String::new(),
            receipt_id,
            metadata,
        };
        record.event_hash = record.recompute_hash();
        self.write_line(&record)?;
        self.reindex()?;
        Ok(record)
    }

    pub fn verify_chain(&self) -> DraftResult<usize> {
        let records = self.read_all()?;
        let mut previous = self.genesis_hash();
        for (index, record) in records.iter().enumerate() {
            if record.ledger != self.ledger {
                return Err(corrupt(format!(
                    "event {} belongs to a different ledger",
                    index + 1
                )));
            }
            if record.previous_event_hash != previous {
                return Err(corrupt(format!(
                    "event {} breaks the ledger chain",
                    index + 1
                )));
            }
            if record.recompute_hash() != record.event_hash {
                return Err(corrupt(format!("event {} has a tampered hash", index + 1)));
            }
            previous = record.event_hash.clone();
        }
        Ok(records.len())
    }

    pub fn index(&self) -> DraftResult<Vec<EventIndexEntry>> {
        if !self.index_path.exists() {
            self.reindex()?;
        }
        let bytes = std::fs::read(&self.index_path)?;
        let index: EventIndex = crate::contracts::decode_persisted(&bytes)?;
        if index.ledger != self.ledger {
            return Err(corrupt("event index belongs to a different ledger"));
        }
        Ok(index.entries)
    }

    fn write_line(&self, record: &EventRecord) -> DraftResult<()> {
        let line = serde_json::to_string(record)
            .map_err(|error| DraftError::storage(format!("serialize event: {error}")))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        Ok(())
    }

    fn reindex(&self) -> DraftResult<()> {
        let entries = self
            .read_all()?
            .into_iter()
            .map(|event| EventIndexEntry {
                event_id: event.event_id,
                event_type: event.event_type,
                subject_id: event.subject_id,
                receipt_id: event.receipt_id,
                time: event.time,
                event_hash: event.event_hash,
            })
            .collect();
        fsutil::write_json(
            &self.index_path,
            &EventIndex {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::EventIndex,
                ),
                ledger: self.ledger.clone(),
                entries,
            },
        )
    }
}

fn corrupt(message: impl Into<String>) -> DraftError {
    DraftError::new(DraftErrorKind::CorruptData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(kind: EventKind, subject: &str) -> NewEvent {
        NewEvent {
            kind,
            subject_id: Some(subject.into()),
            actor_id: "act_test".into(),
            candidate_id: None,
            receipt_id: None,
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn chain_is_scoped_to_its_workspace_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = DraftLayout::for_root(tmp.path());
        let source = EventLog::workspace(paths.clone(), "ws_source");
        let first = source
            .append(sample(EventKind::PackCreated, "pck_a"))
            .unwrap();
        assert_eq!(first.previous_event_hash, source.genesis_hash());
        assert_eq!(source.verify_chain().unwrap(), 1);

        let destination = EventLog::workspace(paths, "ws_destination");
        assert_eq!(
            destination.verify_chain().unwrap_err().kind,
            DraftErrorKind::CorruptData
        );
    }

    #[test]
    fn chain_is_scoped_between_system_ledgers() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("audit.jsonl");
        let source = EventLog::system(&path, "system-a");
        source
            .append_named("system.started", None, "system", None, None, Value::Null)
            .unwrap();
        assert_eq!(source.verify_chain().unwrap(), 1);
        let destination = EventLog::system(path, "system-b");
        assert_eq!(
            destination.verify_chain().unwrap_err().kind,
            DraftErrorKind::CorruptData
        );
    }
}
