//! Shared serializable view models used across CLI, TUI, and AG-UI.

use crate::status::StatusDisplay;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionView {
    pub execution_id: String,
    pub candidate: String,
    pub status: String,
    pub produced_pack: Option<String>,
    pub error: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskView {
    pub task: crate::task::TaskDefinition,
    pub health: String,
    pub health_status: StatusDisplay,
    pub latest_execution: Option<ExecutionView>,
    pub review_status: String,
    pub review_status_display: StatusDisplay,
    pub recommended_action: String,
    pub execution_count: usize,
    pub evidence_count: usize,
    pub produced_packs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackView {
    pub pack_id: String,
    pub name: Option<String>,
    pub status: StatusDisplay,
    pub next_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceView {
    pub evidence_id: String,
    pub kind: String,
    pub status: StatusDisplay,
    pub receipt_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxView {
    pub items: Vec<crate::workflow::InboxItem>,
    pub status: StatusDisplay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectView {
    pub root: String,
    pub status: StatusDisplay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitReadinessView {
    pub pack_id: String,
    pub status: StatusDisplay,
    pub blockers: Vec<String>,
}

#[deprecated(note = "use SubmitReadinessView")]
pub type SaveReadinessView = SubmitReadinessView;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorWorkspaceView {
    pub mode: String,
    pub workspace_hash: String,
    pub pending_edits: usize,
    pub status: StatusDisplay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityView {
    pub events: Vec<String>,
    pub status: StatusDisplay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorView {
    pub healthy: bool,
    pub status: StatusDisplay,
    pub checks: Vec<String>,
}
