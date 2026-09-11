//! The machine-scoped audit chain.
//!
//! Distinct from a project's Activity Ledger, and deliberately so. Activity is
//! project history: what happened to one project's Change Graph, recorded in
//! that project's `events/events.log` under the frozen v1 vocabulary. This
//! chain records the acts that belong to the *installation* rather than to any
//! project — configuring a catalog source, trusting a root, installing a
//! package — which no single project owns and which must survive every project
//! being removed.
//!
//! It reuses the Activity Ledger's storage: the same framed, correctness-locked,
//! hash-chained append. What differs is the vocabulary, because the questions
//! are different ones.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::activity::log::{event_id_for, ActivityLog, LedgerRecord};
use crate::project::home::DraftGlobalStore;
use crate::support::error::DraftResult;

const GLOBAL_AUDIT_LEDGER: &str = "draft-global-audit";

/// What the installation-scoped chain can record.
///
/// Closed, like the Activity vocabulary, so a new global act has to be named
/// here rather than smuggled in as a free-form string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum GlobalAuditEvent {
    ExtensionSourceConfigured,
    ExtensionSourceBuiltinConfigured,
    ExtensionSourceEnabled,
    ExtensionSourceDisabled,
    ExtensionSourceRefreshed,
    ExtensionSourceRemoved,
    ExtensionTrustBootstrapped,
    ExtensionTrustReset,
    ExtensionInstalled,
    ExtensionUpdated,
    ExtensionUninstalled,
    ExtensionEnabled,
    ExtensionDisabled,
    ExtensionAuthorized,
    ExtensionAuthorizationRevoked,
    UserProfileUpdated,
}

impl GlobalAuditEvent {
    pub fn as_str(self) -> &'static str {
        match serde_json::to_value(self) {
            Ok(Value::String(name)) => Box::leak(name.into_boxed_str()),
            _ => unreachable!("GlobalAuditEvent serializes as a string"),
        }
    }
}

impl std::fmt::Display for GlobalAuditEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One entry in the machine-scoped chain, as a reader sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalAuditEntry {
    pub event_id: String,
    pub kind: String,
    pub actor_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    pub payload: Value,
    pub record_hash: String,
    pub previous_hash: String,
}

impl GlobalAuditEntry {
    fn from_record(record: &LedgerRecord) -> Self {
        let field = |name: &str| {
            record
                .payload
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        Self {
            event_id: record.event_id.clone(),
            kind: field("kind").unwrap_or_default(),
            actor_id: field("actor").unwrap_or_default(),
            subject_id: field("subject"),
            operation_id: field("operation_id"),
            payload: record
                .payload
                .get("payload")
                .cloned()
                .unwrap_or(Value::Null),
            record_hash: record.record_hash.clone(),
            previous_hash: record.previous_hash.clone(),
        }
    }
}

pub struct GlobalAuditLog {
    log: ActivityLog,
}

impl GlobalAuditLog {
    pub fn global() -> DraftResult<Self> {
        let store = DraftGlobalStore::locate()?;
        store.create_all()?;
        Ok(Self::at(store.audit_dir()))
    }

    pub fn at(directory: impl AsRef<std::path::Path>) -> Self {
        Self {
            log: ActivityLog::new(directory, GLOBAL_AUDIT_LEDGER),
        }
    }

    pub fn append(
        &self,
        event: GlobalAuditEvent,
        actor_id: Option<String>,
        subject_id: Option<String>,
        operation_id: Option<String>,
        payload: Value,
    ) -> DraftResult<GlobalAuditEntry> {
        let mut record = serde_json::Map::new();
        record.insert("kind".into(), Value::String(event.as_str().to_string()));
        record.insert(
            "actor".into(),
            Value::String(actor_id.unwrap_or_else(|| "system".into())),
        );
        if let Some(subject) = subject_id {
            record.insert("subject".into(), Value::String(subject));
        }
        if let Some(operation) = operation_id {
            record.insert("operation_id".into(), Value::String(operation));
        }
        record.insert(
            "payload".into(),
            crate::support::redaction::redact_value(payload),
        );
        let payload = Value::Object(record);

        let tail = self.log.tail_hash()?;
        let event_id = event_id_for(&format!(
            "{tail}|{}|{}",
            event.as_str(),
            crate::support::hashing::canonical_json(&payload)
        ));
        let outcome = self.log.append(&event_id, &payload)?;
        Ok(GlobalAuditEntry::from_record(outcome.record()))
    }

    pub fn read_all(&self) -> DraftResult<Vec<GlobalAuditEntry>> {
        Ok(self
            .log
            .read_all()?
            .iter()
            .map(GlobalAuditEntry::from_record)
            .collect())
    }

    pub fn verify(&self) -> DraftResult<usize> {
        self.log.verify_chain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::error::DraftErrorKind;

    #[test]
    fn the_global_chain_detects_tampering() {
        let temp = tempfile::tempdir().unwrap();
        let log = GlobalAuditLog::at(temp.path());
        log.append(
            GlobalAuditEvent::ExtensionInstalled,
            None,
            None,
            Some("op_1".into()),
            Value::Null,
        )
        .unwrap();
        log.append(
            GlobalAuditEvent::ExtensionEnabled,
            None,
            None,
            Some("op_2".into()),
            Value::Null,
        )
        .unwrap();
        assert_eq!(log.verify().unwrap(), 2);

        // Same length, so the physical frame stays well-formed and what is
        // detected is the record's own integrity rather than a torn tail.
        let path = temp.path().join("events.log");
        let bytes = std::fs::read(&path).unwrap();
        let text =
            String::from_utf8_lossy(&bytes).replacen("ExtensionInstalled", "TamperedEventKind1", 1);
        std::fs::write(&path, text.as_bytes()).unwrap();
        assert_eq!(
            log.verify().unwrap_err().kind,
            DraftErrorKind::OperationLogCorrupt
        );
    }

    #[test]
    fn a_global_ledger_is_scoped_to_its_own_identity() {
        // So an entry cannot be lifted from one ledger into another and still
        // verify.
        let temp = tempfile::tempdir().unwrap();
        let mine = ActivityLog::new(temp.path(), GLOBAL_AUDIT_LEDGER);
        let theirs = ActivityLog::new(temp.path(), "somebody-else");
        assert_ne!(mine.genesis_hash(), theirs.genesis_hash());
    }
}
