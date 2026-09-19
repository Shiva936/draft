//! Canonical, daemon-owned notification and Inbox state.

use crate::project::home::DraftGlobalStore;
use crate::support::common::{now, Timestamp};
use crate::support::error::{DraftError, DraftResult};
use crate::support::fsutil::write_json;
use crate::support::process_lock::ProcessFileLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationSeverity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextSafeAction {
    pub kind: String,
    pub label: String,
    pub workspace_id: Option<String>,
    pub subject_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationOccurrence {
    pub occurred_at: Timestamp,
    pub correlation_id: Option<String>,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationRecord {
    pub schema_version: u32,
    pub id: String,
    pub deduplication_key: String,
    pub kind: String,
    pub title: String,
    pub message: String,
    pub severity: NotificationSeverity,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub resolved_at: Option<Timestamp>,
    pub read_at: Option<Timestamp>,
    pub dismissed_at: Option<Timestamp>,
    pub workspace_id: Option<String>,
    pub context: Value,
    pub correlation_id: Option<String>,
    pub next_safe_action: Option<NextSafeAction>,
    pub occurrences: Vec<NotificationOccurrence>,
}

impl crate::contracts::VersionedContract for NotificationRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::NotificationRecord;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotificationEnvelope {
    schema_version: u32,
    revision: u64,
    notifications: Vec<NotificationRecord>,
}

impl crate::contracts::VersionedContract for NotificationEnvelope {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::NotificationStore;
}

impl Default for NotificationEnvelope {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::NotificationStore,
            ),
            revision: 0,
            notifications: Vec::new(),
        }
    }
}

pub struct NotificationStore {
    root: PathBuf,
}

impl NotificationStore {
    pub fn global() -> DraftResult<Self> {
        Ok(Self::at(DraftGlobalStore::locate()?.notifications_dir()))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn list(&self, include_resolved: bool) -> DraftResult<Vec<NotificationRecord>> {
        let mut records = self.load()?.notifications;
        if !include_resolved {
            records.retain(|record| record.resolved_at.is_none() && record.dismissed_at.is_none());
        }
        records.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(records)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_condition(
        &self,
        deduplication_key: &str,
        kind: &str,
        title: &str,
        message: &str,
        severity: NotificationSeverity,
        workspace_id: Option<String>,
        correlation_id: Option<String>,
        context: Value,
        next_safe_action: Option<NextSafeAction>,
    ) -> DraftResult<NotificationRecord> {
        if deduplication_key.trim().is_empty() {
            return Err(DraftError::invalid_config(
                "notification deduplication key is required",
            ));
        }
        let _guard = ProcessFileLock::acquire_exclusive(
            &self.root.join("notifications.lock"),
            Duration::from_secs(5),
        )?;
        let mut envelope = self.load()?;
        let at = now();
        let occurrence = NotificationOccurrence {
            occurred_at: at,
            correlation_id: correlation_id.clone(),
            details: context.clone(),
        };
        let record = if let Some(record) = envelope.notifications.iter_mut().find(|record| {
            record.deduplication_key == deduplication_key && record.resolved_at.is_none()
        }) {
            record.kind = kind.into();
            record.title = title.into();
            record.message = message.into();
            record.severity = severity;
            record.updated_at = at;
            record.workspace_id = workspace_id;
            record.correlation_id = correlation_id;
            record.context = context;
            record.next_safe_action = next_safe_action;
            record.dismissed_at = None;
            record.occurrences.push(occurrence);
            record.clone()
        } else {
            let record = NotificationRecord {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::NotificationRecord,
                ),
                id: format!("ntf_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]),
                deduplication_key: deduplication_key.into(),
                kind: kind.into(),
                title: title.into(),
                message: message.into(),
                severity,
                created_at: at,
                updated_at: at,
                resolved_at: None,
                read_at: None,
                dismissed_at: None,
                workspace_id,
                context,
                correlation_id,
                next_safe_action,
                occurrences: vec![occurrence],
            };
            envelope.notifications.push(record.clone());
            record
        };
        self.save(&mut envelope)?;
        Ok(record)
    }

    pub fn mark_read(&self, id: &str, read: bool) -> DraftResult<NotificationRecord> {
        self.update(id, |record| record.read_at = read.then(now))
    }

    pub fn dismiss(&self, id: &str) -> DraftResult<NotificationRecord> {
        self.update(id, |record| record.dismissed_at = Some(now()))
    }

    pub fn resolve(&self, id: &str) -> DraftResult<NotificationRecord> {
        self.update(id, |record| record.resolved_at = Some(now()))
    }

    fn update(
        &self,
        id: &str,
        update: impl FnOnce(&mut NotificationRecord),
    ) -> DraftResult<NotificationRecord> {
        let _guard = ProcessFileLock::acquire_exclusive(
            &self.root.join("notifications.lock"),
            Duration::from_secs(5),
        )?;
        let mut envelope = self.load()?;
        let record = envelope
            .notifications
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or_else(|| DraftError::not_found(format!("notification '{id}' was not found")))?;
        update(record);
        record.updated_at = now();
        let result = record.clone();
        self.save(&mut envelope)?;
        Ok(result)
    }

    fn load(&self) -> DraftResult<NotificationEnvelope> {
        let path = self.root.join("notifications.json");
        if path.exists() {
            crate::contracts::read_persisted(&path)
        } else {
            Ok(NotificationEnvelope::default())
        }
    }

    fn save(&self, envelope: &mut NotificationEnvelope) -> DraftResult<()> {
        envelope.schema_version =
            crate::contracts::current_version(crate::contracts::ContractId::NotificationStore);
        envelope.revision = envelope.revision.saturating_add(1);
        write_json(&self.root.join("notifications.json"), envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_conditions_deduplicate_and_retain_occurrences() {
        let tmp = tempfile::tempdir().unwrap();
        let store = NotificationStore::at(tmp.path());
        for _ in 0..2 {
            store
                .upsert_condition(
                    "workspace:missing",
                    "registry",
                    "Missing",
                    "Path missing",
                    NotificationSeverity::High,
                    Some("prj_a".into()),
                    None,
                    Value::Null,
                    None,
                )
                .unwrap();
        }
        let records = store.list(false).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].occurrences.len(), 2);
        store.mark_read(&records[0].id, true).unwrap();
        assert!(store.list(false).unwrap()[0].read_at.is_some());
    }
}
