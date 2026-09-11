//! Restoring a project to a state it previously held.
//!
//! A rollback plan is drawn before anything moves and says exactly what would
//! be restored, what would be *removed*, and — separately — what it already
//! knows it cannot achieve. Rollback deletes, so the removals are surfaced
//! first and by name: a plan that only listed what it would put back would
//! quietly take things away.
//!
//! The record afterwards says what the rollback actually achieved rather than
//! an unconditional "completed". Applying restoration successfully is not the
//! same as having proved the target state, and the two must not read alike.

use serde::{Deserialize, Serialize};

use crate::support::common::{ActorId, ReceiptId, RollbackPlanId, SnapshotId, Timestamp};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackPlan {
    pub schema_version: u32,
    pub id: RollbackPlanId,
    pub rollback_snapshot_id: SnapshotId,
    /// The authoritative identity of the state this plan restores toward.
    pub target_snapshot_digest: String,
    /// Resources the target says must exist, and that an anchor can restore.
    pub restored_resources: Vec<crate::dcg::resource::ResourceId>,
    /// Resources the target *proves* were absent, which the plan will remove.
    ///
    /// Surfaced separately and before anything runs: rollback deletes, and a
    /// person must be able to see what would go.
    pub removed_resources: Vec<crate::dcg::resource::ResourceId>,
    /// Locators the plan touches, for display.
    pub affected_locators: Vec<crate::dcg::resource::ResourceLocator>,
    /// What this plan already knows it cannot achieve.
    pub known_uncertainties: Vec<crate::dcg::anchor::RollbackUncertainty>,
    /// How completely the target can be restored at all.
    pub recovery_status: crate::dcg::anchor::SnapshotRecoveryStatus,
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
    /// What the rollback actually achieved, with its typed causes.
    ///
    /// Replaces an unconditional "completed": applying restoration successfully
    /// is not the same as having proved the target state, and the two must not
    /// read alike.
    pub outcome: crate::dcg::anchor::RollbackOutcome,
    pub status: String,
    pub started_at: Timestamp,
    pub ended_at: Timestamp,
    pub record_digest: String,
}

impl crate::contracts::VersionedContract for RollbackRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::RollbackRecord;
}
