use std::path::{Path, PathBuf};
use chrono::Utc;
use uuid::Uuid;

use crate::errors::DraftError;
use crate::models::{Checkpoint, CheckpointFile, FileStatus, RestorePlan};
use crate::storage::DraftStorage;
use crate::git_adapter::{GitAdapter, GitCliAdapter, DiffOptions};
use crate::diff_analyzer::DiffAnalyzer;

pub struct CheckpointEngine;

impl CheckpointEngine {
    pub fn create(
        ctx: &crate::models::RepoContext,
        storage: &DraftStorage,
        message: &str,
    ) -> Result<Checkpoint, DraftError> {
        let git = GitCliAdapter::new(ctx.repo_root.clone());
        let diff_text = git.diff(DiffOptions { binary: true, paths: Vec::new() })?;
        let status_text = git.status_porcelain()?;
        let changes = DiffAnalyzer::analyze(&ctx.repo_root, &diff_text, &status_text)?;

        let mut files = Vec::new();

        for change in changes {
            let bytes = match change.status {
                FileStatus::Deleted => {
                    if ctx.head == "0000000000000000000000000000000000000000" {
                        Vec::new()
                    } else {
                        // Retrieve file contents prior to deletion from HEAD
                        git.show_file("HEAD", &change.path).unwrap_or_default()
                    }
                }
                _ => {
                    let full_path = ctx.repo_root.join(&change.path);
                    std::fs::read(&full_path).unwrap_or_default()
                }
            };

            let content_hash = storage.write_blob(&bytes)?;
            files.push(CheckpointFile {
                path: change.path,
                content_hash,
                file_status: change.status,
            });
        }

        let checkpoint = Checkpoint {
            checkpoint_id: Uuid::new_v4().to_string(),
            session_id: Uuid::new_v4().to_string(),
            repo_head: ctx.head.clone(),
            message: message.to_string(),
            created_at: Utc::now(),
            files,
        };

        let rel_path = PathBuf::from("checkpoints").join(format!("{}.json", checkpoint.checkpoint_id));
        storage.write_json(&rel_path, &checkpoint)?;
        
        storage.append_log(&format!("Checkpoint created: {} ({})", checkpoint.checkpoint_id, message))?;

        Ok(checkpoint)
    }

    pub fn restore(
        _ctx: &crate::models::RepoContext,
        storage: &DraftStorage,
        checkpoint_id: &crate::models::CheckpointId,
    ) -> Result<RestorePlan, DraftError> {
        let rel_path = PathBuf::from("checkpoints").join(format!("{}.json", checkpoint_id));
        let checkpoint: Checkpoint = storage.read_json(&rel_path)?;

        let mut files_to_restore = Vec::new();
        let mut files_to_delete = Vec::new();

        for file in &checkpoint.files {
            match file.file_status {
                FileStatus::Deleted => {
                    files_to_delete.push(file.path.clone());
                }
                _ => {
                    files_to_restore.push(file.path.clone());
                }
            }
        }

        Ok(RestorePlan {
            checkpoint_id: checkpoint_id.clone(),
            files_to_restore,
            files_to_delete,
        })
    }

    pub fn apply_restore(
        ctx: &crate::models::RepoContext,
        storage: &DraftStorage,
        plan: RestorePlan,
    ) -> Result<(), DraftError> {
        let rel_path = PathBuf::from("checkpoints").join(format!("{}.json", plan.checkpoint_id));
        let checkpoint: Checkpoint = storage.read_json(&rel_path)?;

        // Restore files
        for file in &checkpoint.files {
            let full_path = ctx.repo_root.join(&file.path);
            
            if plan.files_to_delete.contains(&file.path) {
                if full_path.exists() {
                    std::fs::remove_file(&full_path)?;
                }
            } else if plan.files_to_restore.contains(&file.path) {
                let bytes = storage.read_blob(&file.content_hash)?;
                if let Some(parent) = full_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&full_path, bytes)?;
            }
        }

        storage.append_log(&format!("Checkpoint restored: {}", plan.checkpoint_id))?;
        Ok(())
    }

    pub fn latest(storage: &DraftStorage) -> Result<Option<Checkpoint>, DraftError> {
        let checkpoint_dir = storage.root.join("checkpoints");
        if !checkpoint_dir.exists() {
            return Ok(None);
        }

        let mut latest_checkpoint: Option<Checkpoint> = None;

        for entry in std::fs::read_dir(checkpoint_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
                let rel_path = path.strip_prefix(&storage.root)
                    .map_err(|e| DraftError::StorageError(e.to_string()))?;
                
                if let Ok(checkpoint) = storage.read_json::<Checkpoint>(Path::new(rel_path)) {
                    match &latest_checkpoint {
                        None => latest_checkpoint = Some(checkpoint),
                        Some(current) => {
                            if checkpoint.created_at > current.created_at {
                                latest_checkpoint = Some(checkpoint);
                            }
                        }
                    }
                }
            }
        }

        Ok(latest_checkpoint)
    }
}
