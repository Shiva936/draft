use crate::errors::DraftError;
use crate::models::{ConflictReport, FileChange, FileStatus, RepoContext};

pub struct ConflictEngine;

impl ConflictEngine {
    pub fn detect(
        ctx: &RepoContext,
        changes: &[FileChange],
    ) -> Result<ConflictReport, DraftError> {
        let mut files = Vec::new();
        let mut reasons = Vec::new();
        let mut has_conflicts = false;

        // 1. Check Git index unmerged state
        if ctx.has_unmerged_conflicts {
            has_conflicts = true;
            reasons.push("Git index indicates an unmerged conflict state.".to_string());
        }

        // 2. Scan text contents of modified files for conflict markers
        for change in changes {
            if change.status == FileStatus::Deleted || change.is_binary {
                continue;
            }

            let full_path = ctx.repo_root.join(&change.path);
            if full_path.exists() && full_path.is_file() {
                if let Ok(content) = std::fs::read_to_string(&full_path) {
                    if content.contains("<<<<<<<") && content.contains("=======") && content.contains(">>>>>>>") {
                        has_conflicts = true;
                        if !files.contains(&change.path) {
                            files.push(change.path.clone());
                        }
                        reasons.push(format!("Conflict markers found in: {}", change.path.display()));
                    }
                }
            }
        }

        Ok(ConflictReport {
            has_conflicts,
            files,
            reasons,
        })
    }
}
