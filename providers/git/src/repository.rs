//! `VcsRepository` implementation for Git.

use std::io::Write;
use std::path::PathBuf;

use draft_core::vcs::errors::ProviderError;
use draft_core::vcs::traits::VcsRepository;
use draft_core::vcs::types::*;

use crate::command::{GitCommand, ZERO_OID};
use crate::provider_id;
use crate::{checkpoint, conflicts, diff, finalization, status};

pub struct GitRepository {
    git: GitCommand,
}

impl GitRepository {
    pub fn new(provider_root: PathBuf) -> Self {
        GitRepository {
            git: GitCommand::new(provider_root),
        }
    }
}

impl VcsRepository for GitRepository {
    fn provider_id(&self) -> ProviderId {
        provider_id()
    }

    fn current_view(&self) -> Result<ProviderView, ProviderError> {
        let head = self.git.current_head()?;
        let branch = self.git.branch_name()?;
        let is_dirty = !self.git.status_porcelain()?.is_empty();
        let revision = if head == ZERO_OID {
            None
        } else {
            Some(ProviderRevisionId::new(head.clone()))
        };
        let description = match &branch {
            Some(b) => format!("on branch {b} at {}", &head[..head.len().min(8)]),
            None if head == ZERO_OID => "unborn branch (no commits yet)".to_string(),
            None => format!("detached HEAD at {}", &head[..head.len().min(8)]),
        };
        Ok(ProviderView {
            provider_id: provider_id(),
            revision,
            reference: branch.map(ProviderRef::new),
            is_dirty,
            description,
        })
    }

    fn status(&self) -> Result<ProviderStatus, ProviderError> {
        status::status(&self.git)
    }

    fn diff(&self, input: DiffInput) -> Result<ProviderDelta, ProviderError> {
        diff::diff(&self.git, input)
    }

    fn ignore_rules(&self) -> Result<IgnoreRules, ProviderError> {
        // Prefer .git/info/exclude over editing the project .gitignore (§8.5).
        let git_dir = self.git.git_dir()?;
        let exclude = git_dir.join("info").join("exclude");
        let mut excluded = false;
        if let Ok(content) = std::fs::read_to_string(&exclude) {
            excluded = content.lines().any(|l| l.trim() == ".draft/");
        }
        if !excluded {
            if let Some(parent) = exclude.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&exclude)
            {
                let _ = writeln!(f, ".draft/");
                excluded = true;
            }
        }
        Ok(IgnoreRules {
            draft_dir_excluded: excluded,
            note: ".draft/ excluded via .git/info/exclude".to_string(),
        })
    }

    fn conflicts(&self) -> Result<ConflictSet, ProviderError> {
        conflicts::conflicts(&self.git)
    }

    fn create_checkpoint(
        &self,
        input: CheckpointInput,
    ) -> Result<ProviderCheckpoint, ProviderError> {
        checkpoint::create_checkpoint(&self.git, input)
    }

    fn restore_checkpoint(
        &self,
        checkpoint: ProviderCheckpointRef,
    ) -> Result<ProviderRestoreResult, ProviderError> {
        checkpoint::restore_checkpoint(&self.git, checkpoint)
    }

    fn prepare_finalization(
        &self,
        input: ProviderFinalizationInput,
    ) -> Result<ProviderFinalizationPlan, ProviderError> {
        finalization::prepare_finalization(&self.git, input)
    }

    fn finalize(
        &self,
        plan: ProviderFinalizationPlan,
    ) -> Result<ProviderFinalizationResult, ProviderError> {
        finalization::finalize(&self.git, plan)
    }

    fn undo_provider_action(
        &self,
        input: ProviderUndoInput,
    ) -> Result<ProviderUndoResult, ProviderError> {
        finalization::undo_provider_action(&self.git, input)
    }
}
