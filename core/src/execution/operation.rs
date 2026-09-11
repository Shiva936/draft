//! Durable operation state, recovery details, fenced leases, jobs, and notifications.

use crate::project::home::DraftGlobalStore;
use crate::project::layout::DraftLayout;
use crate::support::common::{now, OperationId, Timestamp};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::{ensure_dir, list_with_extension, write_json};
use crate::support::hashing::canonical_hash;
use crate::support::process_lock::ProcessFileLock;
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationPhase {
    #[default]
    Prepared,
    Running,
    Finalizing,
    Completed,
    FailedBeforeFinalization,
    CancelledBeforeFinalization,
    SafelyRetryable,
    ReconciliationRequired,
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
    #[serde(default)]
    pub phase: OperationPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finalization_started_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_result_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_guidance: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub result: Option<Value>,
    pub error: Option<Value>,
}

/// What a completed Operation permanently did.
///
/// Deliberately smaller than [`OperationRecord`]: only the fields whose
/// substitution would change what the operation *means* to a later reader.
/// Timestamps and phase are lifecycle bookkeeping and are not sealed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedOperation {
    pub operation_id: OperationId,
    pub method: String,
    pub request_hash: String,
    pub result: Option<Value>,
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
        if self.finalization_started_at.is_some()
            && !matches!(
                self.phase,
                OperationPhase::Finalizing
                    | OperationPhase::Completed
                    | OperationPhase::ReconciliationRequired
            )
        {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "an irreversible boundary is recorded for an incompatible operation phase",
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
        let _guard = ProcessFileLock::acquire_exclusive(&self.lock_path(), Duration::from_secs(5))?;
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
            phase: OperationPhase::Prepared,
            finalization_started_at: None,
            target_identity: None,
            known_result_ids: Vec::new(),
            recovery_guidance: None,
            created_at: at,
            updated_at: at,
            result: None,
            error: None,
        };
        self.save(&record)?;
        Ok(BeginOperation::New(record))
    }

    /// The immutable outcome of a completed Operation.
    ///
    /// The in-flight [`OperationRecord`] is mutable by design: status, phase
    /// and timestamps move as the operation runs. What must never move is what
    /// a *completed* operation did — anything that later says "operation X
    /// produced Y" is relying on Y not having been swapped underneath the id.
    ///
    /// So completion seals a separate immutable fact under §2.45's create-once
    /// binding, rather than trying to make the whole lifecycle record
    /// immutable. Replaying the identical completion is idempotent; completing
    /// the same operation id with a *different* outcome is an integrity
    /// violation, not a last-writer-wins update.
    fn outcomes(&self) -> crate::support::immutable_store::ImmutableFactStore<SealedOperation> {
        crate::support::immutable_store::ImmutableFactStore::new(self.root.join("outcomes"))
    }

    pub fn sealed_outcome(
        &self,
        operation_id: &OperationId,
    ) -> DraftResult<Option<SealedOperation>> {
        self.outcomes().get(operation_id.as_str())
    }

    pub fn complete(
        &self,
        mut record: OperationRecord,
        result: Value,
    ) -> DraftResult<OperationRecord> {
        record.status = OperationStatus::Completed;
        record.phase = OperationPhase::Completed;
        record.updated_at = now();
        record.result = Some(result);
        record.error = None;
        // Seal before saving: if the outcome contradicts one already recorded,
        // the operation must not appear completed at all.
        self.outcomes().put(
            record.operation_id.as_str(),
            &SealedOperation {
                operation_id: record.operation_id.clone(),
                method: record.method.clone(),
                request_hash: record.request_hash.clone(),
                result: record.result.clone(),
            },
        )?;
        self.save(&record)?;
        Ok(record)
    }

    pub fn fail(&self, mut record: OperationRecord, error: Value) -> DraftResult<OperationRecord> {
        record.status = OperationStatus::Failed;
        record.phase = if record.finalization_started_at.is_some() {
            OperationPhase::ReconciliationRequired
        } else {
            OperationPhase::FailedBeforeFinalization
        };
        record.updated_at = now();
        record.error = Some(error);
        if record.phase == OperationPhase::ReconciliationRequired {
            record.recovery_guidance = Some(
                "Inspect canonical state and external effects before choosing an explicit recovery action"
                    .into(),
            );
        }
        self.save(&record)?;
        Ok(record)
    }

    pub fn mark_running(&self, mut record: OperationRecord) -> DraftResult<OperationRecord> {
        record.status = OperationStatus::Running;
        record.phase = OperationPhase::Running;
        record.updated_at = now();
        self.save(&record)?;
        Ok(record)
    }

    pub fn begin_finalization(
        &self,
        mut record: OperationRecord,
        target_identity: Option<String>,
    ) -> DraftResult<OperationRecord> {
        record.status = OperationStatus::Running;
        record.phase = OperationPhase::Finalizing;
        record.finalization_started_at = Some(now());
        record.target_identity = target_identity;
        record.updated_at = now();
        self.save(&record)?;
        Ok(record)
    }

    pub fn cancel(&self, id: &str) -> DraftResult<OperationRecord> {
        let _guard = ProcessFileLock::acquire_exclusive(&self.lock_path(), Duration::from_secs(5))?;
        let mut record = self
            .load(id)?
            .ok_or_else(|| DraftError::not_found(format!("operation '{id}' was not found")))?;
        if record.finalization_started_at.is_some()
            || matches!(
                record.phase,
                OperationPhase::Finalizing
                    | OperationPhase::Completed
                    | OperationPhase::ReconciliationRequired
            )
        {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "operation '{}' cannot be cancelled in phase {:?}",
                    id, record.phase
                ),
            )
            .with_suggestion(record.recovery_guidance.clone().unwrap_or_else(|| {
                "wait for finalization to finish, then inspect operation status".into()
            })));
        }
        record.status = OperationStatus::Cancelled;
        record.phase = OperationPhase::CancelledBeforeFinalization;
        record.updated_at = now();
        record.error = None;
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
    fn cancellation_is_allowed_before_but_refused_after_finalization() {
        let tmp = tempfile::tempdir().unwrap();
        let store = OperationStore::at(tmp.path());
        let cancellable = match store
            .begin(
                OperationId::new("op_cancel"),
                "verify.run",
                "hash-cancel",
                None,
            )
            .unwrap()
        {
            BeginOperation::New(record) => store.mark_running(record).unwrap(),
            BeginOperation::Replay(_) => unreachable!(),
        };
        assert_eq!(cancellable.phase, OperationPhase::Running);
        let cancelled = store.cancel("op_cancel").unwrap();
        assert_eq!(cancelled.phase, OperationPhase::CancelledBeforeFinalization);

        let finalizing = match store
            .begin(
                OperationId::new("op_finalize"),
                "promotion.run",
                "hash-finalize",
                None,
            )
            .unwrap()
        {
            BeginOperation::New(record) => store
                .begin_finalization(record, Some("chg_target".into()))
                .unwrap(),
            BeginOperation::Replay(_) => unreachable!(),
        };
        assert!(finalizing.finalization_started_at.is_some());
        assert_eq!(
            store.cancel("op_finalize").unwrap_err().kind,
            DraftErrorKind::ConflictDetected
        );
    }

    #[test]
    fn fencing_tokens_increase_after_release() {
        let tmp = tempfile::tempdir().unwrap();
        let store = crate::execution::lease::LeaseStore::at(tmp.path());
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
