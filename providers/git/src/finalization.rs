//! Git finalization: turn reviewed Draft changes into a Git commit.
//!
//! Policy gating (review/risk/verification) happens in `core` *before* this is
//! called. This module only performs the provider-native mechanics.

use draft_core::vcs::errors::{ProviderError, ProviderErrorKind};
use draft_core::vcs::types::{
    ProviderFinalizationInput, ProviderFinalizationPlan, ProviderFinalizationResult,
    ProviderObjectId, ProviderObjectRef, ProviderRef, ProviderRevisionId, ProviderUndoInput,
    ProviderUndoResult,
};

use crate::command::{GitCommand, ZERO_OID};
use crate::provider_id;

pub fn prepare_finalization(
    git: &GitCommand,
    input: ProviderFinalizationInput,
) -> Result<ProviderFinalizationPlan, ProviderError> {
    if input.message.trim().is_empty() {
        return Err(ProviderError::new(
            ProviderErrorKind::InvalidState,
            "finalization message cannot be empty",
        ));
    }
    let head = git.current_head()?;
    let base = if head == ZERO_OID {
        None
    } else {
        Some(ProviderRevisionId::new(head))
    };
    let summary = format!(
        "Create a Git commit from {} path(s) on {}.",
        input.include_paths.len(),
        git.branch_name()
            .ok()
            .flatten()
            .unwrap_or_else(|| "current branch".to_string())
    );
    Ok(ProviderFinalizationPlan {
        provider_id: provider_id(),
        base_revision: base,
        include_paths: input.include_paths,
        message: input.message,
        trailers: input.trailers,
        summary,
    })
}

pub fn finalize(
    git: &GitCommand,
    plan: ProviderFinalizationPlan,
) -> Result<ProviderFinalizationResult, ProviderError> {
    if plan.include_paths.is_empty() {
        return Err(ProviderError::new(
            ProviderErrorKind::InvalidState,
            "no paths to finalize",
        ));
    }

    // Start from a clean index, then stage exactly the included paths so that
    // excluded changes are not finalized.
    git.run(&["reset", "--quiet"]).ok();

    let mut add_args: Vec<String> = vec!["add".to_string(), "--".to_string()];
    for p in &plan.include_paths {
        add_args.push(p.as_str().to_string());
    }
    let add_refs: Vec<&str> = add_args.iter().map(|s| s.as_str()).collect();
    git.run(&add_refs)?;

    let mut message = plan.message.clone();
    if !plan.trailers.is_empty() {
        message.push_str("\n\n");
        message.push_str(&plan.trailers.join("\n"));
    }

    git.run(&["commit", "-m", &message])?;
    let new_head = git.current_head()?;
    let short = new_head.chars().take(8).collect::<String>();

    Ok(ProviderFinalizationResult {
        provider_id: provider_id(),
        object: ProviderObjectRef {
            provider_id: provider_id(),
            object_id: ProviderObjectId::new(new_head.clone()),
            kind: "commit".to_string(),
            label: Some(short),
        },
        base_revision: plan.base_revision,
        new_revision: Some(ProviderRevisionId::new(new_head)),
        reference: git.branch_name().ok().flatten().map(ProviderRef::new),
    })
}

pub fn undo_provider_action(
    git: &GitCommand,
    input: ProviderUndoInput,
) -> Result<ProviderUndoResult, ProviderError> {
    // We only support undoing the most recent commit, and only if it is still
    // HEAD (safe, non-destructive: a soft reset keeps file content).
    let Some(object) = input.object else {
        return Err(ProviderError::new(
            ProviderErrorKind::InvalidState,
            "no provider object given to undo",
        ));
    };
    if object.kind != "commit" {
        return Err(ProviderError::unsupported("undo of non-commit objects"));
    }
    let head = git.current_head()?;
    if head != object.object_id.as_str() {
        return Err(ProviderError::new(
            ProviderErrorKind::InvalidState,
            "the commit to undo is no longer HEAD; refusing to rewrite history",
        )
        .with_suggestion("Use `git revert` to safely undo an older commit."));
    }
    // Ensure there is a parent.
    if git.run(&["rev-parse", "--verify", "HEAD^"]).is_err() {
        return Err(ProviderError::new(
            ProviderErrorKind::InvalidState,
            "cannot undo the initial commit via soft reset",
        ));
    }
    git.run(&["reset", "--soft", "HEAD^"])?;
    Ok(ProviderUndoResult {
        undone: true,
        message: "soft-reset HEAD to its parent; your changes are preserved and unstaged"
            .to_string(),
        provider_history_changed: true,
    })
}
