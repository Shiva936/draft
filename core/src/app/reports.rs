//! Serializable result types returned by [`super::App`]. These are the shapes
//! rendered by the CLI/TUI and carried over IPC, so they are provider-neutral.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectReport {
    pub provider_id: String,
    pub provider_name: String,
    pub experimental: bool,
    pub root: String,
    pub confidence: String,
    pub reason: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitReport {
    pub workspace_id: String,
    pub provider_id: String,
    pub root: String,
    pub created: bool,
    pub draft_excluded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeGroupSummary {
    pub id: String,
    pub title: String,
    pub files: usize,
    pub review_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    pub workspace_id: String,
    pub provider_id: String,
    pub provider_view: String,
    pub changed_files: usize,
    pub additions: usize,
    pub deletions: usize,
    pub change_groups: Vec<ChangeGroupSummary>,
    pub risk_level: String,
    pub risk_findings: usize,
    pub verification_status: Option<String>,
    pub conflicts: usize,
    pub last_receipt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewReport {
    pub session_id: String,
    pub change_groups: Vec<ChangeGroupSummary>,
    pub decisions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSummary {
    pub command: String,
    pub status: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    pub result_id: String,
    pub status: String,
    pub commands: Vec<CommandSummary>,
}

#[derive(Debug, Clone)]
pub struct FinalizeOptions {
    pub message: String,
    pub trailers: Vec<String>,
    pub no_verify: bool,
    pub confirm_high_risk: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalizeReport {
    pub change_count: usize,
    pub provider_object: String,
    pub provider_object_label: Option<String>,
    pub provider_object_kind: String,
    pub receipt_id: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoReport {
    pub undone: bool,
    pub message: String,
    pub provider_history_changed: bool,
    pub receipt_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointReport {
    pub checkpoint_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderListItem {
    pub id: String,
    pub name: String,
    pub experimental: bool,
    pub description: String,
    pub capabilities: Vec<String>,
}
