//! Durable submit and rollback operation records.

use crate::support::common::{
    ActorId, PackId, ReceiptId, RollbackPlanId, SnapshotId, Timestamp, WorkspacePath,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitRecord {
    pub schema_version: u32,
    pub id: ReceiptId,
    pub pack_id: PackId,
    pub actor_id: ActorId,
    pub native_submit_status: NativeSubmitStatus,
    pub hook_status: HookStatus,
    pub overall_status: SubmitOverallStatus,
    pub message_ref: String,
    pub hook_results: Vec<HookResult>,
    pub hook_receipt_refs: Vec<String>,
    pub object_refs: Vec<String>,
    pub event_refs: Vec<String>,
    pub risk_level: String,
    pub risk_receipt_id: Option<String>,
    pub started_at: Timestamp,
    pub ended_at: Timestamp,
    pub record_digest: String,
    pub failure_reason: Option<String>,
}

impl crate::contracts::VersionedContract for SubmitRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::SubmitRecord;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeSubmitStatus {
    Submitted,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookStatus {
    NotConfigured,
    Skipped,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmitOverallStatus {
    Submitted,
    Failed,
    SubmittedWithHookFailure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookResult {
    pub hook_name: String,
    pub hook_phase: String,
    pub shell: String,
    pub working_dir: String,
    pub command_hash: String,
    pub exit_code: i32,
    pub stdout_ref: String,
    pub stderr_ref: String,
    pub started_at: Timestamp,
    pub ended_at: Timestamp,
    pub env_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackPlan {
    pub schema_version: u32,
    pub id: RollbackPlanId,
    pub rollback_snapshot_id: SnapshotId,
    pub affected_files: Vec<WorkspacePath>,
    pub destructive: bool,
    pub warnings: Vec<String>,
}

impl crate::contracts::VersionedContract for RollbackPlan {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::RollbackPlan;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackRecord {
    pub schema_version: u32,
    pub id: ReceiptId,
    pub rollback_plan_id: RollbackPlanId,
    pub actor_id: ActorId,
    pub status: String,
    pub started_at: Timestamp,
    pub ended_at: Timestamp,
    pub record_digest: String,
}

impl crate::contracts::VersionedContract for RollbackRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::RollbackRecord;
}
