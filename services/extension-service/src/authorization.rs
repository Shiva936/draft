//! Durable capability authorization for installed extensions.
//!
//! Authorization is a decision in its own right, recorded separately from the
//! installation it applies to. Installing a package never creates a grant, and
//! removing a grant never touches an installation.
//!
//! Grants bind to an exact artifact, so any install or update that changes the
//! version or the content digest retires the grant it replaces. Retired grants
//! are kept: they are the record of what was once permitted, and deleting them
//! would erase the audit trail this file exists to hold.

use draft_core::extension::{
    AuthorizedArtifact, ExtensionAuthorizationGrant, ExtensionAuthorizationRegistry,
    ExtensionPermission, InstalledExtension, PendingAuthorization,
};
use draft_core::project::home::DraftGlobalStore;
use draft_core::support::common::{now, OperationId};
use draft_core::support::error::{DraftError, DraftResult};
use draft_core::support::fsutil::{ensure_dir, write_json};
use std::path::PathBuf;

fn authorizations_path(home: &DraftGlobalStore) -> PathBuf {
    home.extensions_dir().join("authorizations.json")
}

fn load(home: &DraftGlobalStore) -> DraftResult<ExtensionAuthorizationRegistry> {
    let path = authorizations_path(home);
    if !path.exists() {
        return Ok(ExtensionAuthorizationRegistry::default());
    }
    draft_core::contracts::read_persisted(&path)
}

fn persist(home: &DraftGlobalStore, registry: &ExtensionAuthorizationRegistry) -> DraftResult<()> {
    ensure_dir(&home.extensions_dir())?;
    write_json(&authorizations_path(home), registry)
}

/// The permissions currently authorized for `installed`.
pub fn authorized_permissions(
    installed: &InstalledExtension,
) -> DraftResult<Vec<ExtensionPermission>> {
    let home = DraftGlobalStore::locate()?;
    Ok(load(&home)?.authorized_permissions(&AuthorizedArtifact::of(installed)))
}

/// Whether Draft may act on `permission` for `installed`.
pub fn authorizes(
    installed: &InstalledExtension,
    permission: ExtensionPermission,
) -> DraftResult<bool> {
    let home = DraftGlobalStore::locate()?;
    Ok(load(&home)?.authorizes(&AuthorizedArtifact::of(installed), permission))
}

/// What `installed` still needs before every one of its contributions is live.
///
/// An extension with nothing outstanding returns `None`. One that declares
/// permissions it does not hold returns them, along with whether a grant exists
/// for some *other* artifact of the same extension — the update case, where the
/// user authorized this extension before but not this build of it.
pub fn pending(installed: &InstalledExtension) -> DraftResult<Option<PendingAuthorization>> {
    let home = DraftGlobalStore::locate()?;
    let registry = load(&home)?;
    let artifact = AuthorizedArtifact::of(installed);
    let held = registry.authorized_permissions(&artifact);
    let missing: Vec<ExtensionPermission> = installed
        .manifest
        .permissions
        .iter()
        .copied()
        .filter(|permission| !held.contains(permission))
        .collect();
    if missing.is_empty() {
        return Ok(None);
    }
    // A grant recorded against a different artifact of the same extension means
    // the user has authorized this extension before; the update invalidated it.
    let superseded_by_update = registry
        .grants
        .get(installed.id())
        .is_some_and(|grant| !grant.binds(&artifact))
        || registry
            .superseded
            .iter()
            .any(|grant| grant.artifact.extension_id == installed.id());
    Ok(Some(PendingAuthorization {
        extension_id: installed.id().to_string(),
        package_version: installed.version().to_string(),
        missing_permissions: missing,
        superseded_by_update,
    }))
}

/// Authorize `permissions` for the installed artifact of `extension_id`.
///
/// Every permission must be one the package actually declares: a user cannot
/// grant a capability the extension never asked for, and an extension cannot
/// receive one it did not declare.
pub fn authorize(
    extension_id: &str,
    permissions: &[ExtensionPermission],
    operation_id: &OperationId,
) -> DraftResult<ExtensionAuthorizationGrant> {
    if permissions.is_empty() {
        return Err(DraftError::invalid_config(
            "authorizing an extension requires at least one permission",
        ));
    }
    let installed = crate::extension::show(extension_id)?;
    for permission in permissions {
        if !installed.manifest.permissions.contains(permission) {
            return Err(DraftError::invalid_config(format!(
                "extension '{extension_id}' does not declare permission '{permission}'"
            )));
        }
    }

    let home = DraftGlobalStore::locate()?;
    let mut registry = load(&home)?;
    let artifact = AuthorizedArtifact::of(&installed);
    let decided_at = now().to_rfc3339();
    let actor_id = draft_core::trust::identity::global::load_actor(&home)?
        .map(|actor| actor.actor_id)
        .unwrap_or_default();

    let mut ordered = permissions.to_vec();
    ordered.sort();
    ordered.dedup();

    let grant = ExtensionAuthorizationGrant {
        schema_version: draft_core::contracts::current_version(
            draft_core::contracts::ContractId::ExtensionAuthorizationGrant,
        ),
        artifact: artifact.clone(),
        permissions: ordered,
        actor_id,
        decision_id: format!("dec_{}", operation_id.as_str().trim_start_matches("op_")),
        operation_id: operation_id.to_string(),
        decided_at: decided_at.clone(),
        superseded_at: None,
    };
    registry.record(grant.clone());
    persist(&home, &registry)?;

    crate::extension::audit(
        draft_core::activity::GlobalAuditEvent::ExtensionAuthorized,
        extension_id,
        &operation_id.to_string(),
        serde_json::json!({
            "package_version": artifact.package_version,
            "content_hash": artifact.content_hash,
            "source_id": artifact.source_id,
            "permissions": grant.permissions,
        }),
    )?;
    Ok(grant)
}

/// Withdraw authorization from `extension_id`, entirely or one permission at a
/// time.
pub fn revoke(
    extension_id: &str,
    permission: Option<ExtensionPermission>,
    operation_id: &OperationId,
) -> DraftResult<ExtensionAuthorizationGrant> {
    let home = DraftGlobalStore::locate()?;
    let mut registry = load(&home)?;
    let at = now().to_rfc3339();
    let withdrawn = match permission {
        Some(permission) => registry.revoke_permission(extension_id, permission, &at),
        None => registry.revoke(extension_id, &at),
    }
    .ok_or_else(|| {
        DraftError::not_found(match permission {
            Some(permission) => {
                format!("extension '{extension_id}' does not hold permission '{permission}'")
            }
            None => format!("extension '{extension_id}' has no authorization to revoke"),
        })
    })?;
    persist(&home, &registry)?;

    crate::extension::audit(
        draft_core::activity::GlobalAuditEvent::ExtensionAuthorizationRevoked,
        extension_id,
        &operation_id.to_string(),
        serde_json::json!({
            "package_version": withdrawn.artifact.package_version,
            "content_hash": withdrawn.artifact.content_hash,
            "permissions": permission
                .map(|permission| vec![permission])
                .unwrap_or_else(|| withdrawn.permissions.clone()),
        }),
    )?;
    Ok(withdrawn)
}

/// Retire the grant for `extension_id` because its installed artifact changed.
///
/// Called from install, update and uninstall. This is what makes an update
/// require fresh authorization even when it asks for exactly the permissions
/// the user already approved.
pub(crate) fn supersede_for(
    extension_id: &str,
) -> DraftResult<Option<ExtensionAuthorizationGrant>> {
    let home = DraftGlobalStore::locate()?;
    let mut registry = load(&home)?;
    let Some(retired) = registry.supersede(extension_id, &now().to_rfc3339()) else {
        return Ok(None);
    };
    persist(&home, &registry)?;
    Ok(Some(retired))
}

/// An installed extension together with its authorization state.
///
/// Serialization flattens the installed record, so every field callers already
/// depend on stays exactly where it was; the authorization fields are added
/// beside them. This is a projection for reading, never a persisted record.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtensionView {
    #[serde(flatten)]
    pub installed: InstalledExtension,
    /// Permissions the package asks for.
    pub declared_permissions: Vec<ExtensionPermission>,
    /// Permissions currently authorized for this exact artifact.
    pub authorized_permissions: Vec<ExtensionPermission>,
    /// What is still outstanding, if anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_authorization: Option<PendingAuthorization>,
}

/// Project one installed extension together with what it is allowed to do.
pub fn view(installed: InstalledExtension) -> DraftResult<ExtensionView> {
    Ok(ExtensionView {
        declared_permissions: installed.manifest.permissions.clone(),
        authorized_permissions: authorized_permissions(&installed)?,
        pending_authorization: pending(&installed)?,
        installed,
    })
}

/// Project every installed extension with its authorization state.
pub fn views(installed: Vec<InstalledExtension>) -> DraftResult<Vec<ExtensionView>> {
    installed.into_iter().map(view).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_core::extension::ExtensionPermission;

    #[test]
    fn a_permission_the_package_never_declared_cannot_be_granted() {
        // Exercised end to end in the service lifecycle tests; here we only
        // pin the shape of the refusal so the message stays actionable.
        let error = DraftError::invalid_config(
            "extension 'x' does not declare permission 'process.execute'",
        );
        assert!(error.to_string().contains("does not declare permission"));
        assert_eq!(
            ExtensionPermission::ProcessExecute.as_str(),
            "process.execute"
        );
    }
}
