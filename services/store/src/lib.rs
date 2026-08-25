//! Rebuildable daemon state in the canonical global Draft store.
//!
//! Holds daemon runtime caches and resumable job records. This is **not** the
//! source of portable project truth — that lives in each workspace's `.draft/`
//! (FR-SVC-007). The canonical cross-project registry is owned by
//! `draft_core::workspace::registry`.

use std::path::{Path, PathBuf};

use draft_core::support::error::{DraftError, DraftErrorKind, DraftResult};
use serde::{Deserialize, Serialize};

pub fn state_dir() -> DraftResult<PathBuf> {
    Ok(draft_core::workspace::home::DraftGlobalStore::locate()?.runtime_dir())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceJobRecord {
    pub schema_version: u32,
    pub id: String,
    pub kind: String,
    pub workspace_path: String,
    pub status: ServiceJobStatus,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub operation_id: Option<String>,
    pub workspace_id: Option<String>,
    pub phase: String,
    pub progress_completed: u64,
    pub progress_total: Option<u64>,
    pub cancellation_requested: bool,
    /// Complete, validated IPC parameters needed to resume this job after a
    /// daemon restart. Secrets must never be accepted as job parameters.
    pub params: serde_json::Value,
    pub correlation_id: String,
    pub attempt: u64,
    pub recovered_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl draft_core::contracts::VersionedContract for ServiceJobRecord {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::ServiceJob;
}

impl ServiceJobRecord {
    fn validate(&self, kind: DraftErrorKind) -> DraftResult<()> {
        if !draft_core::contracts::supports_version(
            draft_core::contracts::ContractId::ServiceJob,
            self.schema_version,
        ) {
            return Err(DraftError::new(
                DraftErrorKind::UnsupportedSchema,
                format!("service job schema {} is unsupported", self.schema_version),
            ));
        }
        if !valid_job_id(&self.id)
            || self.kind.trim().is_empty()
            || self.phase.trim().is_empty()
            || self.correlation_id.trim().is_empty()
        {
            return Err(DraftError::new(
                kind,
                "service job identity, kind, phase, or correlation id is invalid",
            ));
        }
        if self
            .progress_total
            .is_some_and(|total| self.progress_completed > total)
        {
            return Err(DraftError::new(kind, "service job progress is impossible"));
        }
        let state_valid = match self.status {
            ServiceJobStatus::Queued => {
                self.started_at.is_none()
                    && self.ended_at.is_none()
                    && self.result.is_none()
                    && self.error.is_none()
            }
            ServiceJobStatus::Running => {
                self.started_at.is_some()
                    && self.ended_at.is_none()
                    && self.result.is_none()
                    && self.error.is_none()
            }
            ServiceJobStatus::Completed => {
                self.started_at.is_some()
                    && self.ended_at.is_some()
                    && self.result.is_some()
                    && self.error.is_none()
            }
            ServiceJobStatus::Failed => {
                self.ended_at.is_some() && self.result.is_none() && self.error.is_some()
            }
            ServiceJobStatus::Cancelled => self.ended_at.is_some() && self.result.is_none(),
        };
        if !state_valid {
            return Err(DraftError::new(
                kind,
                "service job status/result/timestamp invariant is invalid",
            ));
        }
        Ok(())
    }
}

fn valid_job_id(id: &str) -> bool {
    id.strip_prefix("job_").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

/// The service store rooted in `DraftGlobalStore::runtime_dir()` (or a custom
/// root for tests). Runtime indexes are rebuildable; durable operations and
/// jobs live in the canonical operations namespace.
#[derive(Clone)]
pub struct ServiceStore {
    root: PathBuf,
    operations_root: PathBuf,
    jobs_root: PathBuf,
}

impl ServiceStore {
    pub fn open_default() -> DraftResult<Self> {
        let global = draft_core::workspace::home::DraftGlobalStore::locate()?;
        Self::open_with_namespaces(global.runtime_dir(), global.operations_dir())
    }

    pub fn open(root: PathBuf) -> DraftResult<Self> {
        let operations_root = root.join("operations");
        Self::open_with_namespaces(root, operations_root)
    }

    fn open_with_namespaces(root: PathBuf, operations_root: PathBuf) -> DraftResult<Self> {
        let jobs_root = operations_root.join("jobs");
        std::fs::create_dir_all(root.join("logs"))?;
        std::fs::create_dir_all(&jobs_root)?;
        Ok(ServiceStore {
            root,
            operations_root,
            jobs_root,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn operations_root(&self) -> &Path {
        &self.operations_root
    }

    pub fn save_job(&self, job: &ServiceJobRecord) -> DraftResult<()> {
        job.validate(DraftErrorKind::Validation)?;
        draft_core::support::fsutil::write_atomic(
            &self.jobs_root.join(format!("{}.json", job.id)),
            serde_json::to_vec_pretty(job)?.as_slice(),
        )?;
        Ok(())
    }

    pub fn load_job(&self, id: &str) -> DraftResult<Option<ServiceJobRecord>> {
        if !valid_job_id(id) {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "service job id is invalid",
            ));
        }
        let path = self.jobs_root.join(format!("{id}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let job: ServiceJobRecord = draft_core::contracts::read_persisted(&path)?;
        job.validate(DraftErrorKind::CorruptData)?;
        Ok(Some(job))
    }

    pub fn list_jobs(&self) -> DraftResult<Vec<ServiceJobRecord>> {
        let mut jobs: Vec<ServiceJobRecord> = Vec::new();
        if !self.jobs_root.exists() {
            return Ok(jobs);
        }
        for path in draft_core::support::fsutil::list_with_extension(&self.jobs_root, "json")? {
            let job: ServiceJobRecord = draft_core::contracts::read_persisted(&path)?;
            job.validate(DraftErrorKind::CorruptData)?;
            let stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::CorruptData,
                        "service job filename is invalid",
                    )
                })?;
            if stem != job.id {
                return Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    "service job filename does not match its embedded id",
                ));
            }
            jobs.push(job);
        }
        jobs.sort_by_key(|j| j.submitted_at);
        Ok(jobs)
    }

    /// Append a line to the service log.
    pub fn log(&self, line: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("logs").join("draftd.log"))
        {
            let _ = writeln!(f, "[{}] {}", chrono::Utc::now().to_rfc3339(), line);
        }
    }
}
