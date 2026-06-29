use std::path::PathBuf;
use serde::{Serialize, Deserialize};

pub type GitOid = String;
pub type RepoId = String;
pub type SessionId = String;
pub type CheckpointId = String;
pub type VerificationId = String;
pub type ReceiptId = String;
pub type ChangeGroupId = String;
pub type ObjectHash = String;
pub type Timestamp = chrono::DateTime<chrono::Utc>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoContext {
    pub repo_root: PathBuf,
    pub git_dir: PathBuf,
    pub branch: Option<String>,
    pub head: GitOid,
    pub is_dirty: bool,
    pub is_detached_head: bool,
    pub has_unmerged_conflicts: bool,
    pub identity: Option<Identity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
    pub source: IdentitySource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IdentitySource {
    GitConfig,
    DraftConfig,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftSession {
    pub session_id: SessionId,
    pub repo_id: RepoId,
    pub repo_root: PathBuf,
    pub base_head: GitOid,
    pub current_head: GitOid,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub last_checkpoint: Option<CheckpointId>,
    pub last_verification: Option<VerificationId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    pub status: FileStatus,
    pub additions: usize,
    pub deletions: usize,
    pub is_binary: bool,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed { old_path: PathBuf },
    Copied,
    Untracked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HunkRef {
    pub file_path: PathBuf,
    pub hunk_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangeGroup {
    pub group_id: ChangeGroupId,
    pub title: String,
    pub description: Option<String>,
    pub files: Vec<PathBuf>,
    pub hunks: Vec<HunkRef>,
    pub risk: RiskAssessment,
    pub group_kind: ChangeGroupKind,
    pub included: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ChangeGroupKind {
    SourceChange,
    TestChange,
    ConfigChange,
    DependencyChange,
    MigrationChange,
    GeneratedChange,
    BinaryChange,
    RefactorLikeChange,
    DebugOrLoggingChange,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    pub reasons: Vec<RiskReason>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskReason {
    pub code: String,
    pub message: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationEvidence {
    pub verification_id: VerificationId,
    pub command: String,
    pub exit_code: Option<i32>,
    pub status: VerificationStatus,
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub duration_ms: u64,
    pub stdout_summary: String,
    pub stderr_summary: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VerificationStatus {
    Passed,
    Failed,
    Skipped,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub checkpoint_id: CheckpointId,
    pub session_id: SessionId,
    pub repo_head: GitOid,
    pub message: String,
    pub created_at: Timestamp,
    pub files: Vec<CheckpointFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointFile {
    pub path: PathBuf,
    pub content_hash: ObjectHash,
    pub file_status: FileStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitPlan {
    pub message: String,
    pub included_paths: Vec<PathBuf>,
    pub excluded_paths: Vec<PathBuf>,
    pub head_before: GitOid,
    pub risk_summary: RiskAssessment,
    pub verification: Option<VerificationEvidence>,
    pub coauthors: Vec<Identity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitReceipt {
    pub receipt_id: ReceiptId,
    pub draft_version: String,
    pub repo_id: RepoId,
    pub session_id: SessionId,
    pub commit_hash: GitOid,
    pub commit_message: String,
    pub branch: Option<String>,
    pub head_before: GitOid,
    pub head_after: GitOid,
    pub included_files: Vec<PathBuf>,
    pub excluded_files: Vec<PathBuf>,
    pub risk_summary: RiskAssessment,
    pub verification: Option<VerificationEvidence>,
    pub checkpoint_id: CheckpointId,
    pub identity: Option<Identity>,
    pub coauthors: Vec<Identity>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictReport {
    pub has_conflicts: bool,
    pub files: Vec<PathBuf>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestorePlan {
    pub checkpoint_id: CheckpointId,
    pub files_to_restore: Vec<PathBuf>,
    pub files_to_delete: Vec<PathBuf>,
}
