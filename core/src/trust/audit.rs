//! Genuine system/global audit state backed by the canonical event chain.

use crate::support::error::DraftResult;
use crate::trust::event::{EventLog, EventRecord};
use crate::workspace::home::DraftGlobalStore;
use serde_json::Value;
use std::path::PathBuf;

const GLOBAL_AUDIT_LEDGER: &str = "draft-global-audit";

pub struct GlobalAuditLog {
    events: EventLog,
}

impl GlobalAuditLog {
    pub fn global() -> DraftResult<Self> {
        let store = DraftGlobalStore::locate()?;
        store.create_all()?;
        Ok(Self::at(store.audit_dir().join("audit.jsonl")))
    }

    pub fn at(path: PathBuf) -> Self {
        Self {
            events: EventLog::system(path, GLOBAL_AUDIT_LEDGER),
        }
    }

    pub fn append(
        &self,
        event_type: impl Into<String>,
        actor_id: Option<String>,
        subject_id: Option<String>,
        operation_id: Option<String>,
        payload: Value,
    ) -> DraftResult<EventRecord> {
        self.events.append_named(
            event_type,
            subject_id,
            actor_id.unwrap_or_else(|| "system".into()),
            None,
            None,
            serde_json::json!({
                "operation_id": operation_id,
                "payload": payload,
            }),
        )
    }

    pub fn read_all(&self) -> DraftResult<Vec<EventRecord>> {
        self.events.read_all()
    }

    pub fn verify(&self) -> DraftResult<usize> {
        self.events.verify_chain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::error::DraftErrorKind;

    #[test]
    fn audit_uses_the_canonical_scoped_event_chain() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("audit.jsonl");
        let log = GlobalAuditLog::at(path.clone());
        log.append("first", None, None, Some("op_1".into()), Value::Null)
            .unwrap();
        log.append("second", None, None, Some("op_2".into()), Value::Null)
            .unwrap();
        assert_eq!(log.verify().unwrap(), 2);
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replacen("first", "tampered", 1);
        std::fs::write(&path, text).unwrap();
        assert_eq!(log.verify().unwrap_err().kind, DraftErrorKind::CorruptData);
    }
}
