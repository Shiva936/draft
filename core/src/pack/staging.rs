//! Mutable pack staging state used while deriving immutable pack revisions.
//!
//! Lifecycle, verification, review, quarantine, and rollback state are not
//! stored here; each is owned by its canonical domain record.

use crate::pack::PatchSetId;
use crate::support::common::{
    now, EvidenceId, ExecutionId, PackId, SnapshotId, TaskId, WorkspaceId, WorkspacePath,
};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::workspace::state::FileChangeKind;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackWorkspace {
    pub schema_version: u32,
    pub id: PackId,
    pub name: Option<String>,
    pub task_id: Option<TaskId>,
    pub execution_id: Option<ExecutionId>,
    pub workspace_id: WorkspaceId,
    pub base_snapshot_id: SnapshotId,
    pub result_snapshot_id: SnapshotId,
    pub patch_refs: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub verification_refs: Vec<String>,
    pub review_refs: Vec<String>,
    pub decision_refs: Vec<String>,
    pub receipt_refs: Vec<String>,
    pub source_pack_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub manifest_hash: String,
}

impl crate::contracts::VersionedContract for PackWorkspace {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::PackWorkspace;
}

impl PackWorkspace {
    pub(crate) fn new(
        workspace_id: WorkspaceId,
        task_id: Option<TaskId>,
        execution_id: Option<ExecutionId>,
        base_snapshot_id: SnapshotId,
        result_snapshot_id: SnapshotId,
        name: Option<String>,
    ) -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::PackWorkspace,
            ),
            id: PackId::generate(),
            name,
            task_id,
            execution_id,
            workspace_id,
            base_snapshot_id,
            result_snapshot_id,
            patch_refs: vec![],
            evidence_refs: vec![],
            verification_refs: vec![],
            review_refs: vec![],
            decision_refs: vec![],
            receipt_refs: vec![],
            source_pack_ids: vec![],
            created_at: now(),
            updated_at: now(),
            manifest_hash: String::new(),
        }
    }

    pub(crate) fn validate(&self) -> DraftResult<()> {
        let mut canonical = self.clone();
        canonical.manifest_hash.clear();
        if self.manifest_hash != crate::support::hashing::try_canonical_hash(&canonical)? {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("pack workspace {} staging digest mismatch", self.id),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchSet {
    pub schema_version: u32,
    pub id: PatchSetId,
    pub base_snapshot_id: SnapshotId,
    pub result_snapshot_id: SnapshotId,
    pub files: Vec<FilePatch>,
    pub patch_graph_hash: String,
}

impl crate::contracts::VersionedContract for PatchSet {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::PatchSet;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePatch {
    pub path: WorkspacePath,
    pub old_path: Option<WorkspacePath>,
    pub change_kind: FileChangeKind,
    pub hunks: Vec<PatchHunk>,
    pub binary: bool,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchHunk {
    pub id: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub content_ref: String,
    pub old_content_hash: Option<String>,
    pub new_content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HunkOverlap {
    pub path: WorkspacePath,
    pub left_hunk_id: String,
    pub right_hunk_id: String,
    pub old_start: u32,
    pub old_end: u32,
    pub new_start: u32,
    pub new_end: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub schema_version: u32,
    pub id: EvidenceId,
    pub pack_id: PackId,
    pub command_logs: Vec<String>,
    pub files_touched: Vec<WorkspacePath>,
    pub generated_diff_ref: Option<String>,
    pub test_results: Vec<String>,
    pub lint_results: Vec<String>,
    pub risk_summary_ref: Option<String>,
    pub agent_plan_ref: Option<String>,
    pub agent_transcript_ref: Option<String>,
    pub warnings: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl crate::contracts::VersionedContract for Evidence {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::PackEvidence;
}
