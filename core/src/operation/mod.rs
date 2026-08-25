//! Durable operation state, recovery details, fenced leases, jobs, and notifications.

pub mod editor;
pub mod notification;
pub mod records;

use crate::support::common::{now, OperationId, Timestamp};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::{ensure_dir, list_with_extension, write_json};
use crate::support::hashing::canonical_hash;
use crate::support::lock::FileGuard;
use crate::workspace::home::DraftGlobalStore;
use crate::workspace::layout::DraftLayout;
use crate::workspace::source_view::WorkspaceRevision;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationRecord {
    pub schema_version: u32,
    pub operation_id: OperationId,
    pub request_hash: String,
    pub method: String,
    pub workspace_id: Option<String>,
    pub status: OperationStatus,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub result: Option<Value>,
    pub error: Option<Value>,
}

impl crate::contracts::VersionedContract for OperationRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::OperationRecord;
}

impl OperationRecord {
    fn validate(&self) -> DraftResult<()> {
        if !crate::contracts::supports_version(
            crate::contracts::ContractId::OperationRecord,
            self.schema_version,
        ) {
            return Err(DraftError::new(
                DraftErrorKind::UnsupportedSchema,
                format!("operation schema {} is unsupported", self.schema_version),
            ));
        }
        if self.operation_id.as_str().is_empty()
            || self.request_hash.is_empty()
            || self.method.is_empty()
        {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "operation identity, request hash, and method are mandatory",
            ));
        }
        let valid_payload = match self.status {
            OperationStatus::Completed => self.result.is_some() && self.error.is_none(),
            OperationStatus::Failed => self.result.is_none() && self.error.is_some(),
            OperationStatus::Pending | OperationStatus::Running | OperationStatus::Cancelled => {
                self.result.is_none()
            }
        };
        if !valid_payload {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "operation status/result/error invariant is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum BeginOperation {
    New(OperationRecord),
    Replay(OperationRecord),
}

pub struct OperationStore {
    root: PathBuf,
}

impl OperationStore {
    pub fn global() -> DraftResult<Self> {
        Ok(Self::at(DraftGlobalStore::locate()?.operations_dir()))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn request_hash<T: Serialize>(request: &T) -> String {
        canonical_hash(request)
    }

    pub fn begin(
        &self,
        operation_id: OperationId,
        method: impl Into<String>,
        request_hash: impl Into<String>,
        workspace_id: Option<String>,
    ) -> DraftResult<BeginOperation> {
        let method = method.into();
        let request_hash = request_hash.into();
        let _guard = FileGuard::acquire(&self.lock_path(), Duration::from_secs(5))?;
        if let Some(existing) = self.load(operation_id.as_str())? {
            if existing.request_hash != request_hash || existing.method != method {
                return Err(DraftError::new(
                    DraftErrorKind::ConflictDetected,
                    format!(
                        "operation id '{}' was already used with different parameters",
                        operation_id
                    ),
                ));
            }
            return Ok(BeginOperation::Replay(existing));
        }
        let at = now();
        let record = OperationRecord {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::OperationRecord,
            ),
            operation_id,
            request_hash,
            method,
            workspace_id,
            status: OperationStatus::Pending,
            created_at: at,
            updated_at: at,
            result: None,
            error: None,
        };
        self.save(&record)?;
        Ok(BeginOperation::New(record))
    }

    pub fn complete(
        &self,
        mut record: OperationRecord,
        result: Value,
    ) -> DraftResult<OperationRecord> {
        record.status = OperationStatus::Completed;
        record.updated_at = now();
        record.result = Some(result);
        record.error = None;
        self.save(&record)?;
        Ok(record)
    }

    pub fn fail(&self, mut record: OperationRecord, error: Value) -> DraftResult<OperationRecord> {
        record.status = OperationStatus::Failed;
        record.updated_at = now();
        record.error = Some(error);
        self.save(&record)?;
        Ok(record)
    }

    pub fn load(&self, id: &str) -> DraftResult<Option<OperationRecord>> {
        let path = self.path(id);
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            let record: OperationRecord = crate::contracts::decode_persisted(&bytes)
                .map_err(|error| error.with_context(path.display().to_string()))?;
            record.validate()?;
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }

    fn save(&self, record: &OperationRecord) -> DraftResult<()> {
        record.validate()?;
        write_json(&self.path(record.operation_id.as_str()), record)
    }

    fn path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    fn lock_path(&self) -> PathBuf {
        self.root.join("operations.lock")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FencedLease {
    pub schema_version: u32,
    pub lease_id: String,
    pub scope: String,
    pub operation_id: OperationId,
    pub fencing_token: u64,
    pub acquired_at: Timestamp,
    pub expires_at: Timestamp,
}

impl crate::contracts::VersionedContract for FencedLease {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::FencedLease;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeaseState {
    schema_version: u32,
    last_fencing_token: u64,
    active: Option<FencedLease>,
}

impl crate::contracts::VersionedContract for LeaseState {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::LeaseState;
}

impl Default for LeaseState {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::LeaseState,
            ),
            last_fencing_token: 0,
            active: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationPrecondition {
    pub workspace_id: String,
    pub expected_workspace_revision: WorkspaceRevision,
    pub operation_id: OperationId,
    pub lease_id: String,
    pub fencing_token: u64,
    pub policy_revision: Option<String>,
}

pub struct LeaseStore {
    root: PathBuf,
}

impl LeaseStore {
    pub fn global() -> DraftResult<Self> {
        Ok(Self::at(DraftGlobalStore::locate()?.root().join("leases")))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn acquire(
        &self,
        scope: &str,
        operation_id: OperationId,
        ttl: chrono::Duration,
    ) -> DraftResult<FencedLease> {
        let _guard = FileGuard::acquire(&self.lock_path(scope), Duration::from_secs(5))?;
        let mut state = self.load_state(scope)?;
        if let Some(active) = &state.active {
            if active.expires_at > now() && active.operation_id != operation_id {
                return Err(DraftError::new(
                    DraftErrorKind::LockTimeout,
                    format!("mutation lease '{}' is held by another operation", scope),
                ));
            }
            if active.expires_at > now() && active.operation_id == operation_id {
                return Ok(active.clone());
            }
        }
        state.last_fencing_token = state.last_fencing_token.saturating_add(1);
        let acquired_at = now();
        let lease = FencedLease {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::FencedLease,
            ),
            lease_id: format!("lease_{}", uuid::Uuid::new_v4().simple()),
            scope: scope.into(),
            operation_id,
            fencing_token: state.last_fencing_token,
            acquired_at,
            expires_at: acquired_at + ttl,
        };
        state.active = Some(lease.clone());
        self.save_state(scope, &state)?;
        Ok(lease)
    }

    pub fn release(&self, lease: &FencedLease) -> DraftResult<()> {
        let _guard = FileGuard::acquire(&self.lock_path(&lease.scope), Duration::from_secs(5))?;
        let mut state = self.load_state(&lease.scope)?;
        if state.active.as_ref().is_some_and(|active| {
            active.lease_id == lease.lease_id && active.fencing_token == lease.fencing_token
        }) {
            state.active = None;
            self.save_state(&lease.scope, &state)?;
        }
        Ok(())
    }

    pub fn validate(&self, root: &Path, precondition: &MutationPrecondition) -> DraftResult<()> {
        let current = WorkspaceRevision::derive(root)?;
        if current != precondition.expected_workspace_revision
            || current.workspace_id.as_str() != precondition.workspace_id
        {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "workspace revision changed before mutation commit",
            ));
        }
        let state = self.load_state(&format!("workspace-{}", precondition.workspace_id))?;
        let valid = state.active.as_ref().is_some_and(|lease| {
            lease.lease_id == precondition.lease_id
                && lease.fencing_token == precondition.fencing_token
                && lease.operation_id == precondition.operation_id
                && lease.expires_at > now()
        });
        if !valid {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "stale or mismatched mutation lease",
            ));
        }
        Ok(())
    }

    fn load_state(&self, scope: &str) -> DraftResult<LeaseState> {
        let path = self.state_path(scope);
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            crate::contracts::decode_persisted(&bytes)
                .map_err(|error| error.with_context(path.display().to_string()))
        } else {
            Ok(LeaseState::default())
        }
    }

    fn save_state(&self, scope: &str, state: &LeaseState) -> DraftResult<()> {
        write_json(&self.state_path(scope), state)
    }

    fn state_path(&self, scope: &str) -> PathBuf {
        self.root.join(format!("{}.json", safe_scope(scope)))
    }

    fn lock_path(&self, scope: &str) -> PathBuf {
        self.root.join(format!("{}.lock", safe_scope(scope)))
    }
}

crate::id_newtype!(RecoveryId, "rcv_");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    Started,
    InProgress,
    Completed,
    Failed,
    RolledBack,
    NeedsIntervention,
}

impl RecoveryStatus {
    pub fn requires_recovery(self) -> bool {
        matches!(
            self,
            Self::Started | Self::InProgress | Self::NeedsIntervention
        )
    }
}

/// Recovery-only detail for one durable operation. Identity, idempotency,
/// result, and error remain owned by [`OperationRecord`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryRecord {
    pub schema_version: u32,
    pub recovery_id: RecoveryId,
    pub operation_id: OperationId,
    pub operation: String,
    pub subject_id: Option<String>,
    pub status: RecoveryStatus,
    pub started_at: Timestamp,
    pub updated_at: Timestamp,
    pub backup_paths: Vec<String>,
    pub checkpoint: Option<Value>,
    pub rollback_detail: Option<Value>,
    pub intervention_required: bool,
    pub detail: Value,
    pub error: Option<String>,
}

impl crate::contracts::VersionedContract for RecoveryRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::RecoveryRecord;
}

pub struct RecoveryStore {
    paths: DraftLayout,
}

impl RecoveryStore {
    pub fn for_root(root: &Path) -> Self {
        Self {
            paths: DraftLayout::for_root(root),
        }
    }

    pub fn start(
        &self,
        operation: impl Into<String>,
        subject_id: Option<String>,
        detail: Value,
    ) -> DraftResult<RecoveryRecord> {
        let recovery_id = RecoveryId::generate();
        let at = now();
        let record = RecoveryRecord {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::RecoveryRecord,
            ),
            operation_id: OperationId::new(recovery_id.as_str().replace("rcv_", "op_")),
            recovery_id,
            operation: operation.into(),
            subject_id,
            status: RecoveryStatus::Started,
            started_at: at,
            updated_at: at,
            backup_paths: Vec::new(),
            checkpoint: None,
            rollback_detail: None,
            intervention_required: false,
            detail,
            error: None,
        };
        self.write(&record)?;
        Ok(record)
    }

    pub fn write(&self, record: &RecoveryRecord) -> DraftResult<()> {
        ensure_dir(&self.paths.recovery_dir())?;
        write_json(
            &self
                .paths
                .recovery_dir()
                .join(format!("{}.json", record.recovery_id)),
            record,
        )
    }

    pub fn update(
        &self,
        mut record: RecoveryRecord,
        status: RecoveryStatus,
        detail: Option<Value>,
        error: Option<String>,
    ) -> DraftResult<RecoveryRecord> {
        record.status = status;
        record.updated_at = now();
        if let Some(detail) = detail {
            record.detail = detail;
        }
        record.error = error;
        record.intervention_required = status == RecoveryStatus::NeedsIntervention;
        self.write(&record)?;
        Ok(record)
    }

    pub fn mark_in_progress(
        &self,
        mut record: RecoveryRecord,
        backup_path: Option<String>,
    ) -> DraftResult<RecoveryRecord> {
        if let Some(path) = backup_path {
            record.backup_paths.push(path);
        }
        self.update(record, RecoveryStatus::InProgress, None, None)
    }

    pub fn complete(&self, record: RecoveryRecord, detail: Value) -> DraftResult<RecoveryRecord> {
        self.update(record, RecoveryStatus::Completed, Some(detail), None)
    }

    pub fn fail(
        &self,
        record: RecoveryRecord,
        error: impl Into<String>,
    ) -> DraftResult<RecoveryRecord> {
        self.update(record, RecoveryStatus::Failed, None, Some(error.into()))
    }

    pub fn list(&self) -> DraftResult<Vec<RecoveryRecord>> {
        let mut records = Vec::new();
        for path in list_with_extension(&self.paths.recovery_dir(), "json")? {
            let bytes = std::fs::read(&path)?;
            records.push(
                crate::contracts::decode_persisted(&bytes)
                    .map_err(|error| error.with_context(path.display().to_string()))?,
            );
        }
        records.sort_by(|a: &RecoveryRecord, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.recovery_id.cmp(&b.recovery_id))
        });
        Ok(records)
    }

    pub fn recoverable(&self) -> DraftResult<Vec<RecoveryRecord>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|record| record.status.requires_recovery())
            .collect())
    }
}

pub fn recovery_error(operation: &str, error: &DraftError) -> Value {
    serde_json::json!({
        "operation": operation,
        "error_kind": error.kind.code(),
        "message": error.message,
    })
}

fn safe_scope(scope: &str) -> String {
    scope
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_ids_replay_only_identical_requests() {
        let tmp = tempfile::tempdir().unwrap();
        let store = OperationStore::at(tmp.path());
        let id = OperationId::new("op_same");
        assert!(matches!(
            store
                .begin(id.clone(), "task.update", "hash-a", None)
                .unwrap(),
            BeginOperation::New(_)
        ));
        assert!(matches!(
            store
                .begin(id.clone(), "task.update", "hash-a", None)
                .unwrap(),
            BeginOperation::Replay(_)
        ));
        assert_eq!(
            store
                .begin(id, "task.update", "hash-b", None)
                .unwrap_err()
                .kind,
            DraftErrorKind::ConflictDetected
        );
    }

    #[test]
    fn fencing_tokens_increase_after_release() {
        let tmp = tempfile::tempdir().unwrap();
        let store = LeaseStore::at(tmp.path());
        let first = store
            .acquire(
                "workspace-ws",
                OperationId::new("op_1"),
                chrono::Duration::minutes(1),
            )
            .unwrap();
        store.release(&first).unwrap();
        let second = store
            .acquire(
                "workspace-ws",
                OperationId::new("op_2"),
                chrono::Duration::minutes(1),
            )
            .unwrap();
        assert!(second.fencing_token > first.fencing_token);
    }
}
