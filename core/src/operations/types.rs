//! Operation-log record types (FR-OP-002/003).

use serde::{Deserialize, Serialize};

use crate::common::{OperationId, ReceiptId, Timestamp, WorkspaceId};
use crate::identity::ActorRef;
use crate::risk::RiskSummary;
use crate::vcs::types::{ProviderId, ProviderView};
use crate::verification::VerificationSummary;

/// The kinds of meaningful Draft actions recorded in the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationKind {
    WorkspaceDetected,
    WorkspaceInitialized,
    ProviderSelected,
    ChangeScanned,
    ChangeGrouped,
    ReviewStarted,
    ReviewDecisionRecorded,
    RiskEvaluated,
    VerificationStarted,
    VerificationCompleted,
    CheckpointCreated,
    CheckpointRestored,
    FinalizationPlanned,
    FinalizationCompleted,
    ProviderPublished,
    UndoPlanned,
    UndoApplied,
    ServiceStarted,
    ServiceStopped,
    WorkspaceMigrated,
}

impl OperationKind {
    pub fn label(&self) -> &'static str {
        // Debug names are stable enough; provide explicit labels for output.
        match self {
            OperationKind::WorkspaceDetected => "workspace-detected",
            OperationKind::WorkspaceInitialized => "workspace-initialized",
            OperationKind::ProviderSelected => "provider-selected",
            OperationKind::ChangeScanned => "change-scanned",
            OperationKind::ChangeGrouped => "change-grouped",
            OperationKind::ReviewStarted => "review-started",
            OperationKind::ReviewDecisionRecorded => "review-decision-recorded",
            OperationKind::RiskEvaluated => "risk-evaluated",
            OperationKind::VerificationStarted => "verification-started",
            OperationKind::VerificationCompleted => "verification-completed",
            OperationKind::CheckpointCreated => "checkpoint-created",
            OperationKind::CheckpointRestored => "checkpoint-restored",
            OperationKind::FinalizationPlanned => "finalization-planned",
            OperationKind::FinalizationCompleted => "finalization-completed",
            OperationKind::ProviderPublished => "provider-published",
            OperationKind::UndoPlanned => "undo-planned",
            OperationKind::UndoApplied => "undo-applied",
            OperationKind::ServiceStarted => "service-started",
            OperationKind::ServiceStopped => "service-stopped",
            OperationKind::WorkspaceMigrated => "workspace-migrated",
        }
    }
}

/// A typed reference to another Draft or provider object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRef {
    pub kind: ObjectKind,
    pub id: String,
    #[serde(default)]
    pub provider_id: Option<ProviderId>,
}

impl ObjectRef {
    pub fn new(kind: ObjectKind, id: impl Into<String>) -> Self {
        ObjectRef {
            kind,
            id: id.into(),
            provider_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectKind {
    Change,
    Checkpoint,
    Receipt,
    Review,
    Verification,
    Finalization,
    ProviderObject,
    Workspace,
}

/// Integrity metadata; sha256 over the canonical body (FR-OP-002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationIntegrity {
    pub algorithm: String,
    pub content_sha256: String,
}

/// A single append-only operation-log record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftOperation {
    pub id: OperationId,
    /// Monotonic sequence number (also the file name).
    pub seq: u64,
    pub workspace_id: WorkspaceId,
    pub parent_ids: Vec<OperationId>,
    pub actor: ActorRef,
    pub provider_id: ProviderId,
    #[serde(default)]
    pub observed_provider_view: Option<ProviderView>,
    pub timestamp: Timestamp,
    pub kind: OperationKind,
    #[serde(default)]
    pub input_refs: Vec<ObjectRef>,
    #[serde(default)]
    pub output_refs: Vec<ObjectRef>,
    #[serde(default)]
    pub risk_summary: Option<RiskSummary>,
    #[serde(default)]
    pub verification_summary: Option<VerificationSummary>,
    #[serde(default)]
    pub receipt_refs: Vec<ReceiptId>,
    #[serde(default)]
    pub message: Option<String>,
    pub integrity: OperationIntegrity,
}
