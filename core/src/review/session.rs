//! Canonical review comments, decisions, and read models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::pack::lifecycle::PackLifecycle;
use crate::review::workflow::{DecisionId, ReviewCommentId};
use crate::support::actor::ActorRef;
use crate::support::common::{PackId, WorkspacePath};

/// Authoritative review state persisted for one pack.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewFile {
    pub(crate) schema_version: u32,
    pub(crate) comments: Vec<ReviewComment>,
    pub(crate) decisions: Vec<Decision>,
}

impl crate::contracts::VersionedContract for ReviewFile {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::ReviewFile;
}

impl Default for ReviewFile {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ReviewFile,
            ),
            comments: Vec::new(),
            decisions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: ReviewCommentId,
    pub pack_id: PackId,
    pub path: Option<WorkspacePath>,
    pub hunk_id: Option<String>,
    pub actor: ActorRef,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Approve,
    Reject,
    NeedsChanges,
    AcceptFile,
    RejectFile,
    AcceptCandidate,
}

impl DecisionKind {
    pub fn label(self) -> &'static str {
        match self {
            DecisionKind::Approve => "approve",
            DecisionKind::Reject => "reject",
            DecisionKind::NeedsChanges => "needs_changes",
            DecisionKind::AcceptFile => "accept_file",
            DecisionKind::RejectFile => "reject_file",
            DecisionKind::AcceptCandidate => "accept_candidate",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: DecisionId,
    pub pack_id: PackId,
    pub actor: ActorRef,
    pub kind: DecisionKind,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewReport {
    pub pack_id: String,
    #[serde(default)]
    pub review_receipt_id: Option<String>,
    pub comments: usize,
    pub decisions: usize,
    pub status: PackLifecycle,
    #[serde(default)]
    pub review_units: Vec<ReviewUnit>,
    #[serde(default)]
    pub risk_receipt_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewUnit {
    pub id: String,
    pub path: WorkspacePath,
    pub change_kind: String,
    pub risk_contribution: u32,
    pub evidence_refs: Vec<String>,
    pub provenance_refs: Vec<String>,
    pub status: String,
}
