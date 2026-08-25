//! Workspace scan and snapshot records.

use crate::support::actor::ActorRef;
use crate::support::common::{SnapshotId, WorkspaceId, WorkspacePath};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStatus {
    pub workspace_id: WorkspaceId,
    pub root_path: String,
    pub scanned_at: DateTime<Utc>,
    pub changes: Vec<FileChange>,
    pub ignored_count: usize,
    pub has_draft_dir_violation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: WorkspacePath,
    pub change_kind: FileChangeKind,
    pub file_kind: FileKind,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
    pub size_bytes: Option<u64>,
    pub executable: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed { from: WorkspacePath },
    TypeChanged,
    PermissionChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Text,
    Binary,
    Symlink,
    Directory,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub id: SnapshotId,
    pub workspace_id: WorkspaceId,
    pub manifest_hash: String,
    pub files: Vec<FileManifestEntry>,
    pub content_object_refs: Vec<String>,
    pub ignored_patterns_hash: String,
    pub created_at: DateTime<Utc>,
    pub created_by: ActorRef,
}

impl crate::contracts::VersionedContract for Snapshot {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::WorkspaceSnapshot;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileManifestEntry {
    pub path: WorkspacePath,
    pub file_kind: FileKind,
    pub content_hash: Option<String>,
    pub size_bytes: u64,
    pub modified_time: Option<DateTime<Utc>>,
    pub executable: Option<bool>,
}
