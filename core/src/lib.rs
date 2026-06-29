pub mod models;
pub mod errors;
pub mod storage;
pub mod git_adapter;
pub mod repo_detector;
pub mod identity_manager;
pub mod conflict_engine;
pub mod diff_analyzer;
pub mod change_grouper;
pub mod risk_engine;
pub mod checkpoint_engine;
pub mod verification_engine;
pub mod commit_engine;
pub mod receipt_engine;

use std::io::Write;
use std::path::{Path, PathBuf};
use chrono::Utc;
use serde::{Serialize, Deserialize};
use uuid::Uuid;

use crate::models::{ChangeGroup, Checkpoint, CommitReceipt, GitOid, Identity, RepoContext, RiskAssessment, VerificationEvidence};
use crate::errors::DraftError;
use crate::storage::DraftStorage;
use crate::repo_detector::RepoDetector;
use crate::git_adapter::{GitAdapter, GitCliAdapter, DiffOptions};
use crate::diff_analyzer::DiffAnalyzer;
use crate::change_grouper::ChangeGrouper;
use crate::risk_engine::RiskEngine;
use crate::checkpoint_engine::CheckpointEngine;
use crate::verification_engine::VerificationEngine;
use crate::conflict_engine::ConflictEngine;
use crate::commit_engine::CommitEngine;
use crate::receipt_engine::ReceiptEngine;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartResult {
    pub repo_root: PathBuf,
    pub branch: Option<String>,
    pub head: GitOid,
    pub identity: Option<Identity>,
    pub is_new: bool,
    pub draft_tracked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResult {
    pub repo_root: PathBuf,
    pub branch: Option<String>,
    pub head: GitOid,
    pub changed_files: usize,
    pub risk_summary: RiskAssessment,
    pub verification: Option<VerificationEvidence>,
    pub last_checkpoint: Option<Checkpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    pub repo_context: RepoContext,
    pub groups: Vec<ChangeGroup>,
    pub risk_summary: RiskAssessment,
    pub verification: Option<VerificationEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRequest {
    pub message: String,
    pub groups: Vec<ChangeGroup>,
    pub no_verify: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResult {
    pub commit_hash: GitOid,
    pub receipt: CommitReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoPlan {
    pub checkpoint_id: String,
    pub files_to_restore: Vec<PathBuf>,
    pub files_to_delete: Vec<PathBuf>,
}

pub fn start_repo(path: &Path) -> Result<StartResult, DraftError> {
    let ctx = RepoDetector::detect(path)?;
    RepoDetector::validate_supported(&ctx)?;

    let storage = DraftStorage::init(&ctx.repo_root)?;
    
    // Manage configuration files
    let config_path = Path::new("config.toml");
    let mut is_new = false;
    
    let repo_id = if let Ok(existing_config) = storage.read_toml::<serde_json::Value>(config_path) {
        existing_config.get("repo_id")
            .and_then(|v| v.as_str())
            .unwrap_or(&Uuid::new_v4().to_string())
            .to_string()
    } else {
        is_new = true;
        Uuid::new_v4().to_string()
    };

    if is_new {
        let config_data = serde_json::json!({
            "version": 1,
            "repo_id": repo_id,
            "default_verify_command": "",
            "created_at": Utc::now().to_rfc3339(),
        });
        storage.write_toml(config_path, &config_data)?;

        let repo_metadata = serde_json::json!({
            "repo_root": ctx.repo_root.to_string_lossy().into_owned(),
            "git_dir": ctx.git_dir.to_string_lossy().into_owned(),
            "initial_head": ctx.head.clone(),
            "created_at": Utc::now().to_rfc3339(),
        });
        storage.write_toml(Path::new("repo.toml"), &repo_metadata)?;
    }

    // Exclude .draft/ from git tracking
    let exclude_path = ctx.git_dir.join("info/exclude");
    if let Some(parent) = exclude_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_path)
    {
        if let Ok(content) = std::fs::read_to_string(&exclude_path) {
            if !content.lines().any(|l| l.trim() == ".draft/") {
                let _ = writeln!(file, ".draft/");
            }
        }
    }

    storage.append_log("Draft session initialized or resumed.")?;

    // FR-009: warn if .draft/ is accidentally tracked by Git
    let draft_tracked = std::process::Command::new("git")
        .args(&["ls-files", "--error-unmatch", ".draft/"])
        .current_dir(&ctx.repo_root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    Ok(StartResult {
        repo_root: ctx.repo_root,
        branch: ctx.branch,
        head: ctx.head,
        identity: ctx.identity,
        is_new,
        draft_tracked,
    })
}

pub fn get_status(path: &Path) -> Result<StatusResult, DraftError> {
    let ctx = RepoDetector::detect(path)?;
    let storage = DraftStorage::open(&ctx.repo_root)?;

    let git = GitCliAdapter::new(ctx.repo_root.clone());
    let diff_text = git.diff(DiffOptions { binary: true, paths: Vec::new() })?;
    let status_text = git.status_porcelain()?;
    let changes = DiffAnalyzer::analyze(&ctx.repo_root, &diff_text, &status_text)?;
    let changed_files = changes.len();

    let groups = ChangeGrouper::group(&changes)?;
    let assessed_groups = RiskEngine::assess(&groups, &ctx)?;
    let risk_summary = RiskEngine::summarize(&assessed_groups);

    let last_checkpoint = CheckpointEngine::latest(&storage)?;
    let verification = latest_verification_evidence(&storage)?;

    Ok(StatusResult {
        repo_root: ctx.repo_root,
        branch: ctx.branch,
        head: ctx.head,
        changed_files,
        risk_summary,
        verification,
        last_checkpoint,
    })
}

pub fn review_repo(path: &Path) -> Result<ReviewResult, DraftError> {
    let ctx = RepoDetector::detect(path)?;
    let storage = DraftStorage::open(&ctx.repo_root)?;

    let git = GitCliAdapter::new(ctx.repo_root.clone());
    let diff_text = git.diff(DiffOptions { binary: true, paths: Vec::new() })?;
    let status_text = git.status_porcelain()?;
    let changes = DiffAnalyzer::analyze(&ctx.repo_root, &diff_text, &status_text)?;

    let groups = ChangeGrouper::group(&changes)?;
    let assessed_groups = RiskEngine::assess(&groups, &ctx)?;
    let risk_summary = RiskEngine::summarize(&assessed_groups);

    let verification = latest_verification_evidence(&storage)?;

    Ok(ReviewResult {
        repo_context: ctx,
        groups: assessed_groups,
        risk_summary,
        verification,
    })
}

pub fn run_verification(path: &Path, command: Option<String>) -> Result<VerificationEvidence, DraftError> {
    let ctx = RepoDetector::detect(path)?;
    let storage = DraftStorage::open(&ctx.repo_root)?;

    let command_to_run = match command {
        Some(cmd) => cmd,
        None => match VerificationEngine::infer_command(&ctx.repo_root) {
            Some(inferred) => inferred,
            None => {
                return Err(DraftError::VerificationFailed(
                    "No verification command provided and none could be inferred.".to_string(),
                ));
            }
        },
    };

    let evidence = VerificationEngine::run(&ctx.repo_root, &command_to_run, &storage)?;
    Ok(evidence)
}

pub fn create_commit(path: &Path, request: CommitRequest) -> Result<CommitResult, DraftError> {
    let ctx = RepoDetector::detect(path)?;
    RepoDetector::validate_supported(&ctx)?;

    let storage = DraftStorage::open(&ctx.repo_root)?;
    let git = GitCliAdapter::new(ctx.repo_root.clone());

    // 1. Unstage all changes to get a clean index
    git.unstage_all()?;

    // 2. Diff analyzer to get current raw changes
    let diff_text = git.diff(DiffOptions { binary: true, paths: Vec::new() })?;
    let status_text = git.status_porcelain()?;
    let changes = DiffAnalyzer::analyze(&ctx.repo_root, &diff_text, &status_text)?;

    // 3. Scan for conflicts
    let conflict_report = ConflictEngine::detect(&ctx, &changes)?;
    if conflict_report.has_conflicts {
        return Err(DraftError::CommitBlocked(format!(
            "Cannot commit: unresolved merge conflicts exist. Reasons:\n{}",
            conflict_report.reasons.join("\n")
        )));
    }

    // 4. Retrieve latest verification evidence if not skipping
    let verification = if request.no_verify {
        None
    } else {
        latest_verification_evidence(&storage)?
    };

    // 5. Build CommitPlan
    let commit_plan = CommitEngine::prepare(&ctx, &request.groups, request.message.clone(), verification.clone())?;

    // 6. Create Checkpoint snapshot first
    let checkpoint = CheckpointEngine::create(&ctx, &storage, &format!("Pre-commit: {}", request.message))?;

    // 7. Execute commit
    let commit_hash = CommitEngine::execute(&git, &commit_plan)?;

    // 8. Generate Commit Receipt
    let config_path = Path::new("config.toml");
    let config_val = storage.read_toml::<serde_json::Value>(config_path)?;
    let repo_id = config_val.get("repo_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let receipt = CommitReceipt {
        receipt_id: Uuid::new_v4().to_string(),
        draft_version: "0.1.0".to_string(),
        repo_id,
        session_id: checkpoint.session_id.clone(),
        commit_hash: commit_hash.clone(),
        commit_message: request.message.clone(),
        branch: ctx.branch,
        head_before: commit_plan.head_before,
        head_after: commit_hash.clone(),
        included_files: commit_plan.included_paths,
        excluded_files: commit_plan.excluded_paths,
        risk_summary: commit_plan.risk_summary,
        verification,
        checkpoint_id: checkpoint.checkpoint_id,
        identity: ctx.identity,
        coauthors: commit_plan.coauthors,
        created_at: Utc::now(),
    };

    ReceiptEngine::create(&storage, receipt.clone())?;

    Ok(CommitResult {
        commit_hash,
        receipt,
    })
}

pub fn undo_last(path: &Path) -> Result<UndoPlan, DraftError> {
    let ctx = RepoDetector::detect(path)?;
    let storage = DraftStorage::open(&ctx.repo_root)?;

    let checkpoint = CheckpointEngine::latest(&storage)?
        .ok_or(DraftError::CheckpointMissing)?;

    let plan = CheckpointEngine::restore(&ctx, &storage, &checkpoint.checkpoint_id)?;
    CheckpointEngine::apply_restore(&ctx, &storage, plan.clone())?;

    Ok(UndoPlan {
        checkpoint_id: plan.checkpoint_id,
        files_to_restore: plan.files_to_restore,
        files_to_delete: plan.files_to_delete,
    })
}

fn latest_verification_evidence(storage: &DraftStorage) -> Result<Option<VerificationEvidence>, DraftError> {
    let dir = storage.root.join("verification");
    if !dir.exists() {
        return Ok(None);
    }

    let mut latest: Option<VerificationEvidence> = None;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
            let rel_path = path.strip_prefix(&storage.root)
                .map_err(|e| DraftError::StorageError(e.to_string()))?;

            if let Ok(evidence) = storage.read_json::<VerificationEvidence>(Path::new(rel_path)) {
                match &latest {
                    None => latest = Some(evidence),
                    Some(current) => {
                        if evidence.started_at > current.started_at {
                            latest = Some(evidence);
                        }
                    }
                }
            }
        }
    }
    Ok(latest)
}
