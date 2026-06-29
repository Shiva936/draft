//! Provider-neutral value types exchanged between `core` and providers.
//!
//! Core treats all provider-native identifiers (`ProviderRevisionId`,
//! `ProviderObjectId`, ...) as **opaque strings**. Core must never parse them
//! (e.g. it must never assume a Git SHA layout).

use serde::{Deserialize, Serialize};

use crate::common::{Timestamp, WorkspacePath};
use crate::id_newtype;

// ---------------------------------------------------------------------------
// Opaque provider-native identifiers
// ---------------------------------------------------------------------------

id_newtype!(
    /// Identifies a provider implementation, e.g. `git`, `fs`, `jj`.
    ProviderId, "");
id_newtype!(
    /// A provider-native revision identifier (e.g. a Git commit SHA). Opaque.
    ProviderRevisionId, "");
id_newtype!(
    /// A provider-native ref name (e.g. a Git ref). Opaque.
    ProviderRef, "");
id_newtype!(
    /// A provider-native object identifier. Opaque.
    ProviderObjectId, "");
id_newtype!(
    /// A provider-native checkpoint identifier. Opaque.
    ProviderCheckpointId, "");

/// A reference to a provider-native object, with a human label describing what
/// kind of object it is (e.g. "commit"). Stored in receipts (DR-004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderObjectRef {
    pub provider_id: ProviderId,
    pub object_id: ProviderObjectId,
    /// Provider-defined kind, e.g. "commit", "tree", "snapshot".
    pub kind: String,
    /// Optional short human label, e.g. an abbreviated revision.
    pub label: Option<String>,
}

// ---------------------------------------------------------------------------
// Views and status
// ---------------------------------------------------------------------------

/// A snapshot of the provider's current logical position (what the working tree
/// is based on). Everything here is opaque/provider-defined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderView {
    pub provider_id: ProviderId,
    /// The current base revision, if the provider has one.
    pub revision: Option<ProviderRevisionId>,
    /// The current named position (branch/bookmark/channel), if any.
    pub reference: Option<ProviderRef>,
    /// Whether the working area has uncommitted/unfinalized changes.
    pub is_dirty: bool,
    /// Provider-specific human-readable description of the view.
    pub description: String,
}

/// Provider-neutral status of the working area.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub entries: Vec<StatusEntry>,
    /// True if the provider reports it has a staging area distinct from the
    /// working tree (capability detail surfaced for display only).
    pub has_staged_changes: bool,
    /// True if the provider reports unresolved conflicts.
    pub has_conflicts: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusEntry {
    pub path: WorkspacePath,
    pub status: FileStatus,
    /// Original path for renames/copies.
    pub old_path: Option<WorkspacePath>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

impl FileStatus {
    pub fn label(&self) -> &'static str {
        match self {
            FileStatus::Added => "added",
            FileStatus::Modified => "modified",
            FileStatus::Deleted => "deleted",
            FileStatus::Renamed => "renamed",
            FileStatus::Copied => "copied",
            FileStatus::TypeChanged => "type-changed",
            FileStatus::Untracked => "untracked",
            FileStatus::Conflicted => "conflicted",
        }
    }
}

// ---------------------------------------------------------------------------
// Diff model
// ---------------------------------------------------------------------------

/// What a diff should be computed against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DiffInput {
    /// Diff the working area against the current base view.
    #[default]
    WorkingTree,
    /// Diff only the given paths against the current base view.
    Paths(Vec<WorkspacePath>),
}

/// A provider-neutral delta describing what changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDelta {
    pub base: Option<ProviderRevisionId>,
    pub files: Vec<FileDelta>,
    pub stats: DiffStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDelta {
    pub path: WorkspacePath,
    pub old_path: Option<WorkspacePath>,
    pub status: FileStatus,
    pub hunks: Vec<DiffHunk>,
    pub binary: bool,
    /// Set when hunks were intentionally omitted (e.g. huge or binary file),
    /// so callers know the delta is summarized rather than empty.
    pub summarized: bool,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStats {
    pub files_changed: usize,
    pub additions: usize,
    pub deletions: usize,
    pub binary_files: usize,
}

// ---------------------------------------------------------------------------
// Ignore rules
// ---------------------------------------------------------------------------

/// Provider ignore information relevant to Draft (used to keep `.draft/` out of
/// provider history and to avoid scanning ignored files).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoreRules {
    /// Whether the provider already excludes `.draft/` from its history.
    pub draft_dir_excluded: bool,
    /// Human description of how exclusion is/should be achieved.
    pub note: String,
}

// ---------------------------------------------------------------------------
// Conflicts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictSet {
    pub conflicts: Vec<Conflict>,
}

impl ConflictSet {
    pub fn is_empty(&self) -> bool {
        self.conflicts.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub path: WorkspacePath,
    pub kind: ConflictKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictKind {
    /// Provider-reported content/merge conflict.
    Provider,
    /// Draft-detected metadata conflict.
    DraftMetadata,
}

// ---------------------------------------------------------------------------
// Checkpoints
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointInput {
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCheckpoint {
    pub id: ProviderCheckpointId,
    pub kind: ProviderCheckpointKind,
    pub provider_refs: Vec<ProviderRef>,
    pub provider_revisions: Vec<ProviderRevisionId>,
    pub created_at: Timestamp,
    /// Provider-defined opaque payload needed to restore (e.g. stash ref).
    pub restore_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderCheckpointKind {
    /// A snapshot of the working area (e.g. Git stash/temp tree).
    WorkingSnapshot,
    /// A reference to an existing provider revision.
    Revision,
    /// A filesystem-level snapshot (fs provider).
    FileSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCheckpointRef {
    pub id: ProviderCheckpointId,
    pub kind: ProviderCheckpointKind,
    pub restore_token: Option<String>,
}

impl From<&ProviderCheckpoint> for ProviderCheckpointRef {
    fn from(c: &ProviderCheckpoint) -> Self {
        ProviderCheckpointRef {
            id: c.id.clone(),
            kind: c.kind,
            restore_token: c.restore_token.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRestoreResult {
    pub restored: bool,
    /// Files that were restored to their checkpoint content.
    pub restored_paths: Vec<WorkspacePath>,
    /// Files that were removed to match the checkpoint.
    pub removed_paths: Vec<WorkspacePath>,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Finalization
// ---------------------------------------------------------------------------

/// Everything a provider needs to prepare a finalization (commit) without any
/// knowledge of Draft's review/risk policy (those are gated in core first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFinalizationInput {
    /// Paths to include in the finalized object.
    pub include_paths: Vec<WorkspacePath>,
    /// The finalization message (e.g. commit message body).
    pub message: String,
    /// Free-form metadata trailers (e.g. co-authors). Provider may use or ignore.
    pub trailers: Vec<String>,
}

/// A provider's prepared, not-yet-executed finalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFinalizationPlan {
    pub provider_id: ProviderId,
    pub base_revision: Option<ProviderRevisionId>,
    pub include_paths: Vec<WorkspacePath>,
    pub message: String,
    pub trailers: Vec<String>,
    /// Human description of what finalization will do.
    pub summary: String,
}

/// The result of executing a finalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFinalizationResult {
    pub provider_id: ProviderId,
    /// The created provider object (e.g. the new commit).
    pub object: ProviderObjectRef,
    pub base_revision: Option<ProviderRevisionId>,
    pub new_revision: Option<ProviderRevisionId>,
    pub reference: Option<ProviderRef>,
}

// ---------------------------------------------------------------------------
// Undo
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUndoInput {
    /// The provider object to undo (e.g. the last finalized commit).
    pub object: Option<ProviderObjectRef>,
    /// A checkpoint to restore to, if available.
    pub checkpoint: Option<ProviderCheckpointRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUndoResult {
    pub undone: bool,
    pub message: String,
    /// Whether provider history was modified by the undo.
    pub provider_history_changed: bool,
}
