use std::collections::HashSet;
use std::path::PathBuf;

use crate::errors::DraftError;
use crate::models::{ChangeGroup, CommitPlan, GitOid, RepoContext, VerificationEvidence};
use crate::git_adapter::GitAdapter;
use crate::risk_engine::RiskEngine;

pub struct CommitEngine;

impl CommitEngine {
    pub fn prepare(
        ctx: &RepoContext,
        groups: &[ChangeGroup],
        message: String,
        verification: Option<VerificationEvidence>,
    ) -> Result<CommitPlan, DraftError> {
        if ctx.has_unmerged_conflicts {
            return Err(DraftError::CommitBlocked(
                "Unresolved conflicts detected. Resolve them first before committing.".to_string()
            ));
        }

        if message.trim().is_empty() {
            return Err(DraftError::CommitBlocked("Commit message cannot be empty.".to_string()));
        }

        let mut included_set = HashSet::new();
        let mut excluded_set = HashSet::new();

        for group in groups {
            if group.included {
                for file in &group.files {
                    included_set.insert(file.clone());
                }
            } else {
                for file in &group.files {
                    excluded_set.insert(file.clone());
                }
            }
        }

        // If a file is marked both included and excluded, inclusion takes precedence
        for file in &included_set {
            excluded_set.remove(file);
        }

        let included_paths: Vec<PathBuf> = included_set.into_iter().collect();
        let excluded_paths: Vec<PathBuf> = excluded_set.into_iter().collect();

        if included_paths.is_empty() {
            return Err(DraftError::CommitBlocked(
                "No changes are included in this commit plan.".to_string()
            ));
        }

        let risk_summary = RiskEngine::summarize(groups);

        Ok(CommitPlan {
            message,
            included_paths,
            excluded_paths,
            head_before: ctx.head.clone(),
            risk_summary,
            verification,
            coauthors: Vec::new(),
        })
    }

    pub fn execute(
        git: &dyn GitAdapter,
        plan: &CommitPlan,
    ) -> Result<GitOid, DraftError> {
        // 1. Verify HEAD stability
        let current_head = git.current_head()?;
        if current_head != plan.head_before {
            return Err(DraftError::CommitBlocked(
                "Git HEAD has moved since review. Re-run draft review.".to_string()
            ));
        }

        // 2. Unstage all currently staged changes
        git.unstage_all()?;

        // 3. Stage only included paths
        git.stage_paths(&plan.included_paths)?;

        // 4. Build commit message (with co-author trailers if any)
        let mut full_message = plan.message.clone();
        if !plan.coauthors.is_empty() {
            full_message.push_str("\n\n");
            for coauthor in &plan.coauthors {
                full_message.push_str(&crate::identity_manager::IdentityManager::coauthor_trailer(coauthor));
                full_message.push_str("\n");
            }
        }

        // 5. Execute git commit
        let new_head = git.commit(&full_message)?;
        Ok(new_head)
    }
}
