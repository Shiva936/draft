pub mod config;
pub mod home;
pub mod layout;
pub(crate) mod object_store;
pub mod ownership;
pub mod protected;
pub mod registry;
pub(crate) mod snapshot;
pub mod source_view;
pub mod stable;
pub mod state;

pub use layout::DraftLayout;
pub use source_view::{CanonicalSourceView, WorkspaceRevision};

use crate::support::common::WorkspaceId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Workspace {
    pub workspace_id: WorkspaceId,
    pub root: PathBuf,
    pub layout: DraftLayout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMetadata {
    pub schema_version: u32,
    pub workspace_id: WorkspaceId,
    pub draft_version: String,
    pub created_at: DateTime<Utc>,
}

impl crate::contracts::VersionedContract for WorkspaceMetadata {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::WorkspaceMetadata;
}
