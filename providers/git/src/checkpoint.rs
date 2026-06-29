//! Git checkpoint strategy.
//!
//! v0.2.0 uses **non-destructive stash snapshots**: `git stash create` builds a
//! commit object capturing the working tree + index *without* modifying either
//! and without pushing onto the stash stack. Restore applies that object.
//! If there are no local changes, the checkpoint references `HEAD` instead.

use draft_core::common::now;
use draft_core::vcs::errors::{ProviderError, ProviderErrorKind};
use draft_core::vcs::types::{
    CheckpointInput, ProviderCheckpoint, ProviderCheckpointKind, ProviderCheckpointRef,
    ProviderRef, ProviderRestoreResult, ProviderRevisionId,
};

use crate::command::{GitCommand, ZERO_OID};

pub fn create_checkpoint(
    git: &GitCommand,
    input: CheckpointInput,
) -> Result<ProviderCheckpoint, ProviderError> {
    let desc = input
        .description
        .unwrap_or_else(|| "draft checkpoint".to_string());
    let stash_oid = git.run(&["stash", "create", &desc]).unwrap_or_default();

    if stash_oid.trim().is_empty() {
        // No local changes: checkpoint the current revision.
        let head = git.current_head()?;
        let revisions = if head == ZERO_OID {
            vec![]
        } else {
            vec![ProviderRevisionId::new(head)]
        };
        Ok(ProviderCheckpoint {
            id: draft_core::vcs::types::ProviderCheckpointId::new(format!(
                "rev-{}",
                revisions
                    .first()
                    .map(|r| r.as_str().to_string())
                    .unwrap_or_else(|| "empty".to_string())
            )),
            kind: ProviderCheckpointKind::Revision,
            provider_refs: branch_ref(git),
            provider_revisions: revisions,
            created_at: now(),
            restore_token: None,
        })
    } else {
        let oid = stash_oid.trim().to_string();
        Ok(ProviderCheckpoint {
            id: draft_core::vcs::types::ProviderCheckpointId::new(format!("stash-{oid}")),
            kind: ProviderCheckpointKind::WorkingSnapshot,
            provider_refs: branch_ref(git),
            provider_revisions: vec![ProviderRevisionId::new(oid.clone())],
            created_at: now(),
            restore_token: Some(oid),
        })
    }
}

fn branch_ref(git: &GitCommand) -> Vec<ProviderRef> {
    git.branch_name()
        .ok()
        .flatten()
        .map(|b| vec![ProviderRef::new(b)])
        .unwrap_or_default()
}

pub fn restore_checkpoint(
    git: &GitCommand,
    checkpoint: ProviderCheckpointRef,
) -> Result<ProviderRestoreResult, ProviderError> {
    match (checkpoint.kind, checkpoint.restore_token) {
        (ProviderCheckpointKind::WorkingSnapshot, Some(oid)) => {
            // Applying a stash snapshot onto the working tree. May fail if the
            // tree has diverged; surface that as a safety error.
            let out = git.run_raw(&["stash", "apply", &oid])?;
            if out.success {
                Ok(ProviderRestoreResult {
                    restored: true,
                    restored_paths: vec![],
                    removed_paths: vec![],
                    message: format!("applied working snapshot {oid}"),
                })
            } else {
                Err(ProviderError::new(
                    ProviderErrorKind::InvalidState,
                    "could not apply checkpoint snapshot onto the current working tree",
                )
                .with_context(out.stderr.trim().to_string())
                .with_suggestion("Resolve or stash current changes, then retry the restore."))
            }
        }
        (ProviderCheckpointKind::Revision, _) => Err(ProviderError::new(
            ProviderErrorKind::UnsupportedOperation,
            "restoring to a bare revision checkpoint would rewrite working state",
        )
        .with_suggestion("Use `git checkout` manually if you intend to discard changes.")),
        _ => Err(ProviderError::unsupported(
            "checkpoint restore for this checkpoint kind",
        )),
    }
}
