//! Persisted workspace identity metadata (`.draft/workspace.json`).

use serde::{Deserialize, Serialize};

use crate::common::{Timestamp, WorkspaceId};
use crate::vcs::types::ProviderId;

/// The portable identity of a workspace. Path fields are stored relative to the
/// workspace root so the directory can be moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMetadata {
    pub id: WorkspaceId,
    pub draft_version: String,
    pub provider_id: ProviderId,
    /// Provider root relative to the workspace root (usually ".").
    pub provider_root_rel: String,
    pub created_at: Timestamp,
    /// Set when this workspace was migrated from a previous Draft version.
    #[serde(default)]
    pub migrated_from: Option<String>,
}
