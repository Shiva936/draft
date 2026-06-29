//! Draft change model and grouping (FR-CHG-001/002/003).
//!
//! A `DraftChange` is a reviewable unit of work *before* finalization. It is not
//! a provider commit — it is grouped from a provider-neutral [`ProviderDelta`].

pub mod grouping;
pub mod store;

use serde::{Deserialize, Serialize};

use crate::common::{now, DraftChangeId, Timestamp, WorkspaceId, WorkspacePath};
use crate::review::ReviewState;
use crate::risk::RiskSummary;
use crate::vcs::types::FileStatus;
use crate::verification::VerificationSummary;

pub use grouping::group_delta;
pub use store::{load_change, load_changes, save_change, save_group_index};

/// How a change grouping was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupingSource {
    Automatic,
    Manual,
    ProviderSuggested,
    AgentSuggested,
}

/// A reference to one file's participation in a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChangeRef {
    pub path: WorkspacePath,
    pub old_path: Option<WorkspacePath>,
    pub status: FileStatus,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftChange {
    pub id: DraftChangeId,
    pub workspace_id: WorkspaceId,
    pub title: Option<String>,
    pub description: Option<String>,
    pub file_changes: Vec<FileChangeRef>,
    pub grouping_source: GroupingSource,
    pub review_state: ReviewState,
    pub risk_summary: Option<RiskSummary>,
    pub verification_summary: Option<VerificationSummary>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl DraftChange {
    pub fn new(
        workspace_id: WorkspaceId,
        title: Option<String>,
        file_changes: Vec<FileChangeRef>,
        grouping_source: GroupingSource,
    ) -> Self {
        let ts = now();
        DraftChange {
            id: DraftChangeId::generate(),
            workspace_id,
            title,
            description: None,
            file_changes,
            grouping_source,
            review_state: ReviewState::Pending,
            risk_summary: None,
            verification_summary: None,
            created_at: ts,
            updated_at: ts,
        }
    }

    /// All paths touched by this change (new paths; renames use the new path).
    pub fn paths(&self) -> Vec<WorkspacePath> {
        self.file_changes.iter().map(|f| f.path.clone()).collect()
    }
}
