use std::path::{Path, PathBuf};
use crate::errors::DraftError;
use crate::models::RepoContext;
use crate::git_adapter::{GitAdapter, GitCliAdapter};
use crate::identity_manager::IdentityManager;

pub struct RepoDetector;

impl RepoDetector {
    pub fn detect(start_path: &Path) -> Result<RepoContext, DraftError> {
        let output = std::process::Command::new("git")
            .args(&["rev-parse", "--show-toplevel"])
            .current_dir(start_path)
            .output();

        let output = match output {
            Ok(out) => out,
            Err(_) => return Err(DraftError::NotGitRepo),
        };

        if !output.status.success() {
            return Err(DraftError::NotGitRepo);
        }

        let repo_root_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let repo_root = PathBuf::from(repo_root_str);

        let git = GitCliAdapter::new(repo_root.clone());
        let git_dir = git.git_dir()?;
        let branch = git.branch_name()?;
        let head = git.current_head()?;
        
        let status_output = git.status_porcelain()?;
        let is_dirty = !status_output.is_empty();

        // Check if detached HEAD: branch is None but HEAD is not zeroed
        let is_detached_head = branch.is_none() && head != "0000000000000000000000000000000000000000";

        // Check for unmerged conflicts in porcelain status
        let mut has_unmerged_conflicts = false;
        for line in status_output.lines() {
            if line.len() >= 2 {
                let code = &line[0..2];
                // Porcelain conflict codes: DD, AU, UD, UA, DU, AA, UU
                if code == "DD" || code == "AU" || code == "UD" || code == "UA" || code == "DU" || code == "AA" || code == "UU" {
                    has_unmerged_conflicts = true;
                    break;
                }
            }
        }

        let mut ctx = RepoContext {
            repo_root,
            git_dir,
            branch,
            head,
            is_dirty,
            is_detached_head,
            has_unmerged_conflicts,
            identity: None,
        };

        // Populate identity
        ctx.identity = IdentityManager::current(&ctx).ok().flatten();

        Ok(ctx)
    }

    pub fn validate_supported(ctx: &RepoContext) -> Result<(), DraftError> {
        // Detached HEAD checks
        if ctx.is_detached_head {
            return Err(DraftError::UnsupportedRepoState(
                "Repository is in a detached HEAD state. Please checkout a branch first.".to_string(),
            ));
        }

        // Unmerged conflicts checks
        if ctx.has_unmerged_conflicts {
            return Err(DraftError::UnsupportedRepoState(
                "Repository contains unmerged conflicts. Please resolve them first.".to_string(),
            ));
        }

        // Check if bare repo by verifying show-toplevel output doesn't match bare repo error
        // But since we successfully resolved repo_root, it is not bare.

        Ok(())
    }
}
