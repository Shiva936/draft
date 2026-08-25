//! Stable actor resolution and fail-closed retired profile detection.

use std::path::{Path, PathBuf};

use crate::support::actor::{ActorKind, ActorRef};
use crate::support::common::ActorId;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::workspace::home::DraftGlobalStore;

fn retired_profile_error(location: &Path) -> DraftError {
    DraftError::new(
        DraftErrorKind::UnsupportedSchema,
        format!(
            "unsupported pre-release profile state exists at {}",
            location.display()
        ),
    )
    .with_suggestion(
        "remove the retired profile state without reusing its values; configure display metadata through `draft config set user.name ...` and optional `user.email`",
    )
}

fn user_config_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(value) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        roots.push(PathBuf::from(value));
    }
    if let Some(value) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        let root = PathBuf::from(value).join(".config");
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// Reject retired profile stores by location alone. Their contents are never
/// opened, parsed, normalized, migrated, aliased, or applied.
pub fn reject_retired_profile_state(draft_dir: Option<&Path>) -> DraftResult<()> {
    if let Some(draft_dir) = draft_dir {
        let retired = draft_dir.join("identity.json");
        if retired.exists() {
            return Err(retired_profile_error(&retired));
        }
    }
    for root in user_config_roots() {
        let retired = root.join("draft").join("identity.toml");
        if retired.exists() {
            return Err(retired_profile_error(&retired));
        }
    }
    for key in ["DRAFT_IDENTITY_USERNAME", "DRAFT_IDENTITY_EMAIL"] {
        if std::env::var_os(key).is_some() {
            return Err(DraftError::new(
                DraftErrorKind::UnsupportedSchema,
                format!("unsupported pre-release profile environment key {key} is set"),
            )
            .with_suggestion(
                "unset the retired environment key; use canonical user.* configuration",
            ));
        }
    }
    Ok(())
}

/// Resolve the stable security actor. Profile/display configuration is not
/// read here and therefore cannot affect attribution, signatures, or hashes.
pub fn resolve_actor(draft_dir: &Path) -> DraftResult<ActorRef> {
    reject_retired_profile_state(Some(draft_dir))?;
    let home = DraftGlobalStore::locate()?;
    let actor = super::global::load_actor(&home)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            "stable security actor state is missing",
        )
        .with_suggestion(
            "restore the actor/key state or initialize a new isolated Draft global store",
        )
    })?;
    Ok(ActorRef {
        id: ActorId::new(actor.actor_id.clone()),
        kind: ActorKind::Human,
        display_name: actor.actor_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_workspace_profile_is_rejected_without_reading_it() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("identity.json");
        std::fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();
        let error = reject_retired_profile_state(Some(temp.path())).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::UnsupportedSchema);
        assert!(error.message.contains(&path.display().to_string()));
    }
}
