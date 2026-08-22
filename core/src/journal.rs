//! Durable operation journal for maintenance and destructive operations.

use crate::common::{now, Timestamp};
use crate::error::{DraftError, DraftResult};
use crate::fsutil::{ensure_dir, list_with_extension, read_json, write_json};
use crate::layout::ProjectPaths;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

crate::id_newtype!(JournalId, "op_");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalStatus {
    Started,
    InProgress,
    Completed,
    Failed,
    RolledBack,
    NeedsRecovery,
}

impl JournalStatus {
    pub fn is_recoverable(self) -> bool {
        matches!(
            self,
            JournalStatus::Started | JournalStatus::InProgress | JournalStatus::NeedsRecovery
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub schema_version: String,
    pub id: JournalId,
    pub operation: String,
    pub subject_id: Option<String>,
    pub status: JournalStatus,
    pub started_at: Timestamp,
    pub updated_at: Timestamp,
    pub backup_path: Option<String>,
    pub detail: Value,
    pub error: Option<String>,
}

pub struct JournalStore {
    paths: ProjectPaths,
}

impl JournalStore {
    pub fn for_root(root: &Path) -> Self {
        Self {
            paths: ProjectPaths::for_root(root),
        }
    }

    pub fn start(
        &self,
        operation: impl Into<String>,
        subject_id: Option<String>,
        detail: Value,
    ) -> DraftResult<JournalEntry> {
        let at = now();
        let entry = JournalEntry {
            schema_version: crate::DRAFT_SCHEMA_VERSION.into(),
            id: JournalId::generate(),
            operation: operation.into(),
            subject_id,
            status: JournalStatus::Started,
            started_at: at,
            updated_at: at,
            backup_path: None,
            detail,
            error: None,
        };
        self.write(&entry)?;
        Ok(entry)
    }

    pub fn write(&self, entry: &JournalEntry) -> DraftResult<()> {
        ensure_dir(&self.paths.journal_dir())?;
        write_json(
            &self.paths.journal_dir().join(format!("{}.json", entry.id)),
            entry,
        )
    }

    pub fn update(
        &self,
        mut entry: JournalEntry,
        status: JournalStatus,
        detail: Option<Value>,
        error: Option<String>,
    ) -> DraftResult<JournalEntry> {
        entry.status = status;
        entry.updated_at = now();
        if let Some(detail) = detail {
            entry.detail = detail;
        }
        entry.error = error;
        self.write(&entry)?;
        Ok(entry)
    }

    pub fn mark_in_progress(
        &self,
        entry: JournalEntry,
        backup_path: Option<String>,
    ) -> DraftResult<JournalEntry> {
        let mut entry = entry;
        entry.backup_path = backup_path;
        self.update(entry, JournalStatus::InProgress, None, None)
    }

    pub fn complete(&self, entry: JournalEntry, detail: Value) -> DraftResult<JournalEntry> {
        self.update(entry, JournalStatus::Completed, Some(detail), None)
    }

    pub fn fail(&self, entry: JournalEntry, error: impl Into<String>) -> DraftResult<JournalEntry> {
        self.update(entry, JournalStatus::Failed, None, Some(error.into()))
    }

    pub fn list(&self) -> DraftResult<Vec<JournalEntry>> {
        let mut entries: Vec<JournalEntry> = Vec::new();
        for path in list_with_extension(&self.paths.journal_dir(), "json")? {
            entries.push(read_json(&path)?);
        }
        entries.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(entries)
    }

    pub fn recoverable(&self) -> DraftResult<Vec<JournalEntry>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|entry| entry.status.is_recoverable())
            .collect())
    }
}

pub fn journal_error(operation: &str, err: &DraftError) -> Value {
    serde_json::json!({
        "operation": operation,
        "error_kind": err.kind.code(),
        "message": err.message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recoverable_tracks_interrupted_operations() {
        let tmp = tempfile::tempdir().unwrap();
        let store = JournalStore::for_root(tmp.path());
        let entry = store
            .start("doctor.migrate", None, serde_json::json!({"from":"0.3.3"}))
            .unwrap();
        assert_eq!(store.recoverable().unwrap().len(), 1);
        store
            .complete(entry, serde_json::json!({"to":"0.3.4"}))
            .unwrap();
        assert!(store.recoverable().unwrap().is_empty());
    }
}
