//! Installing declarative Draft extension packages.
//!
//! Extension *semantics* — identity, manifests, contributions and
//! compatibility — belong to `draft_core::extension`, which re-exports the
//! portable format every extension author consumes. This module owns the
//! infrastructure around them: validating a candidate package on this
//! filesystem, copying it into place atomically, and durably recording what is
//! installed.
//!
//! Extensions contribute validated declarative records only. No source type,
//! manifest field, asset, or installed file can introduce executable code.

use draft_core::extension::in_draft;
use draft_core::extension::package as format_package;
use draft_core::project::home::DraftGlobalStore;
use draft_core::support::error::{DraftError, DraftErrorKind, DraftResult};
use draft_core::support::fsutil::{ensure_dir, write_json};
use draft_core::support::hashing::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

// Re-exported so callers keep one import path for the extension model even
// though its semantics are owned by Draft Core.
pub use draft_core::extension::{
    ExtensionContribution, ExtensionContributionKind, ExtensionId, ExtensionManifest,
    ExtensionTrustProvenance, InstalledExtension, InstalledExtensionProvenance,
};

const MAX_FILE_BYTES: u64 = format_package::MAX_FILE_BYTES;

pub(crate) fn draft_api_compatible(requirement: &str) -> bool {
    draft_core::extension::draft_api_compatible(requirement)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    schema_version: u32,
    extensions: BTreeMap<String, InstalledExtension>,
}

impl draft_core::contracts::VersionedContract for Registry {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::ExtensionRegistry;
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::ExtensionRegistry,
            ),
            extensions: BTreeMap::new(),
        }
    }
}

fn packages(home: &DraftGlobalStore) -> PathBuf {
    home.extensions_dir().join("packages")
}

fn registry_path(home: &DraftGlobalStore) -> PathBuf {
    home.extensions_dir().join("registry.json")
}

fn load(home: &DraftGlobalStore) -> DraftResult<Registry> {
    let registry = if registry_path(home).exists() {
        draft_core::contracts::read_persisted(&registry_path(home))?
    } else {
        Registry::default()
    };
    for (id, installed) in &registry.extensions {
        if !draft_core::contracts::supports_version(
            draft_core::contracts::ContractId::ExtensionManifest,
            installed.manifest.schema_version,
        ) || !draft_core::contracts::supports_version(
            draft_core::contracts::ContractId::InstalledExtensionProvenance,
            installed.provenance.schema_version,
        ) {
            return Err(DraftError::new(
                DraftErrorKind::UnsupportedSchema,
                format!("installed extension '{id}' contains an unsupported schema"),
            ));
        }
        if installed.manifest.id != *id
            || installed.provenance.package_id != installed.manifest.id
            || installed.provenance.package_version != installed.manifest.version
        {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("installed extension '{id}' has inconsistent identity or provenance"),
            ));
        }
        let package_root = packages(home).join(id);
        let actual_content_hash = sha256_hex(&package_bytes(&package_root)?);
        if installed.content_hash != actual_content_hash {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("installed extension '{id}' content digest mismatch"),
            ));
        }
    }
    Ok(registry)
}

pub(crate) fn read_package_manifest(path: &Path) -> DraftResult<ExtensionManifest> {
    let bytes = fs::read(path)?;
    draft_core::contracts::decode_wire(&bytes)
        .map_err(|error| error.with_context(path.display().to_string()))
}

fn persist(home: &DraftGlobalStore, registry: Registry) -> DraftResult<()> {
    ensure_dir(&home.extensions_dir())?;
    write_json(&registry_path(home), &registry)
}

pub fn list() -> DraftResult<Vec<InstalledExtension>> {
    let home = DraftGlobalStore::locate()?;
    let mut extensions: Vec<_> = load(&home)?.extensions.into_values().collect();
    extensions.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
    Ok(extensions)
}

pub fn show(id: &str) -> DraftResult<InstalledExtension> {
    let home = DraftGlobalStore::locate()?;
    load(&home)?
        .extensions
        .remove(id)
        .ok_or_else(|| DraftError::not_found(format!("extension '{id}' is not installed")))
}

pub fn active_contributions() -> DraftResult<Vec<(ExtensionId, ExtensionContribution)>> {
    let mut active = Vec::new();
    for extension in list()?.into_iter().filter(|extension| extension.enabled) {
        if crate::catalog::package_is_revoked(&extension)? {
            continue;
        }
        for contribution in extension.manifest.contributions {
            active.push((extension.manifest.id.clone(), contribution));
        }
    }
    active.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.id.cmp(&right.1.id))
    });
    Ok(active)
}

pub fn set_enabled(id: &str, enabled: bool) -> DraftResult<InstalledExtension> {
    let home = DraftGlobalStore::locate()?;
    let mut registry = load(&home)?;
    let extension = registry
        .extensions
        .get_mut(id)
        .ok_or_else(|| DraftError::not_found(format!("extension '{id}' is not installed")))?;
    if enabled && crate::catalog::package_is_revoked(extension)? {
        return Err(DraftError::new(
            DraftErrorKind::ReviewRequired,
            format!("extension '{id}' is revoked by its authoritative trust root"),
        ));
    }
    extension.enabled = enabled;
    let result = extension.clone();
    persist(&home, registry)?;
    audit(
        if enabled {
            draft_core::activity::GlobalAuditEvent::ExtensionEnabled
        } else {
            draft_core::activity::GlobalAuditEvent::ExtensionDisabled
        },
        id,
        &draft_core::support::common::OperationId::generate().to_string(),
        serde_json::json!({ "enabled": enabled }),
    )?;
    Ok(result)
}

/// Install an explicitly selected local directory as an out-of-band local
/// trust decision. Catalog configuration alone never reaches this operation.
pub fn install(source: &Path) -> DraftResult<InstalledExtension> {
    let source = source.canonicalize().map_err(|error| {
        DraftError::not_found(format!("cannot open extension package: {error}"))
    })?;
    if !source.is_dir() {
        return Err(DraftError::invalid_config(
            "extension source must be a directory",
        ));
    }
    let manifest = read_package_manifest(&source.join("extension.json"))?;
    validate_manifest(&source, &manifest)?;
    let bytes = package_bytes(&source)?;
    let artifact_digest = sha256_hex(&bytes);
    let operation_id = draft_core::support::common::OperationId::generate().to_string();
    let installed_at = draft_core::support::common::now().to_rfc3339();
    let home = DraftGlobalStore::locate()?;
    let actor_id = draft_core::trust::identity::global::load_actor(&home)?
        .map(|actor| actor.actor_id)
        .unwrap_or_else(|| "act_unknown".to_string());
    let provenance = InstalledExtensionProvenance {
        schema_version: draft_core::contracts::current_version(
            draft_core::contracts::ContractId::InstalledExtensionProvenance,
        ),
        package_id: manifest.id.to_string(),
        package_version: manifest.version.clone(),
        operation_id: operation_id.clone(),
        trust: ExtensionTrustProvenance::DirectLocal {
            decision_id: operation_id,
            actor_id,
            decided_at: installed_at,
            source_description: format!("local-directory:{}", source.display()),
            artifact_digest,
        },
    };
    install_validated(&source, provenance, false)
}

pub(crate) fn install_catalog_package(
    source: &Path,
    provenance: InstalledExtensionProvenance,
    allow_update: bool,
) -> DraftResult<InstalledExtension> {
    install_validated(source, provenance, allow_update)
}

fn install_validated(
    source: &Path,
    provenance: InstalledExtensionProvenance,
    allow_update: bool,
) -> DraftResult<InstalledExtension> {
    let source = source.canonicalize().map_err(|error| {
        DraftError::not_found(format!("cannot open extension package: {error}"))
    })?;
    let manifest = read_package_manifest(&source.join("extension.json"))?;
    validate_manifest(&source, &manifest)?;
    if manifest.id != provenance.package_id || manifest.version != provenance.package_version {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "verified catalog target identity does not match its extension manifest",
        ));
    }
    let content_hash = sha256_hex(&package_bytes(&source)?);
    let home = DraftGlobalStore::locate()?;
    let mut registry = load(&home)?;
    let previous = registry.extensions.get(manifest.id.as_str()).cloned();
    if previous.is_some() && !allow_update {
        return Err(DraftError::new(
            DraftErrorKind::TaskDefinitionConflict,
            format!("extension '{}' is already installed", manifest.id),
        ));
    }
    if previous.as_ref().is_some_and(|installed| {
        installed.manifest.version == manifest.version
            && installed.provenance.artifact_digest() == provenance.artifact_digest()
    }) {
        return Ok(previous.expect("checked above"));
    }

    let destination = packages(&home).join(manifest.id.as_str());
    ensure_dir(&packages(&home))?;
    let staging = packages(&home).join(format!(
        ".install-{}-{}-{}",
        manifest.id,
        std::process::id(),
        provenance.operation_id.trim_start_matches("op_")
    ));
    copy_package(&source, &staging)?;
    let backup = previous.as_ref().map(|_| {
        packages(&home).join(format!(
            ".previous-{}-{}",
            manifest.id,
            provenance.operation_id.trim_start_matches("op_")
        ))
    });
    if let Some(backup) = &backup {
        fs::rename(&destination, backup).map_err(|error| {
            let _ = fs::remove_dir_all(&staging);
            DraftError::storage(format!("failed to stage prior extension version: {error}"))
        })?;
    } else if destination.exists() {
        let _ = fs::remove_dir_all(&staging);
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "extension package destination already exists without registry provenance",
        ));
    }
    if let Err(error) = fs::rename(&staging, &destination) {
        if let Some(backup) = &backup {
            let _ = fs::rename(backup, &destination);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(DraftError::storage(format!(
            "failed to promote extension package: {error}"
        )));
    }
    let installed = InstalledExtension {
        manifest: manifest.clone(),
        content_hash,
        enabled: previous.as_ref().is_none_or(|installed| installed.enabled),
        installed_at: match &provenance.trust {
            ExtensionTrustProvenance::HttpsCatalog { verified_at, .. }
            | ExtensionTrustProvenance::TrustedLocalCatalog { verified_at, .. } => {
                verified_at.clone()
            }
            ExtensionTrustProvenance::DirectLocal { decided_at, .. } => decided_at.clone(),
        },
        provenance: provenance.clone(),
    };
    registry
        .extensions
        .insert(manifest.id.to_string(), installed.clone());
    if let Err(error) = persist(&home, registry) {
        let _ = fs::remove_dir_all(&destination);
        if let Some(backup) = &backup {
            let _ = fs::rename(backup, &destination);
        }
        return Err(error);
    }
    // The installed artifact just changed, so any authorization the user gave
    // the previous one no longer binds. It is retired rather than deleted, and
    // the new artifact starts unauthorized until the user says otherwise.
    let superseded_grant = crate::authorization::supersede_for(manifest.id.as_str())?;
    if let Some(backup) = &backup {
        let preserved = home.extensions_dir().join("removed").join(format!(
            "{}-{}-{}",
            manifest.id,
            previous
                .as_ref()
                .map(|installed| installed.manifest.version.as_str())
                .unwrap_or("unknown"),
            draft_core::support::common::now().timestamp_millis()
        ));
        ensure_dir(preserved.parent().unwrap_or(&home.extensions_dir()))?;
        fs::rename(backup, preserved).map_err(|error| {
            DraftError::storage(format!(
                "failed to preserve prior extension version: {error}"
            ))
        })?;
    }
    audit(
        if previous.is_some() {
            draft_core::activity::GlobalAuditEvent::ExtensionUpdated
        } else {
            draft_core::activity::GlobalAuditEvent::ExtensionInstalled
        },
        &manifest.id,
        &provenance.operation_id,
        serde_json::json!({
            "version": manifest.version,
            "artifact_digest": installed.provenance.artifact_digest(),
            "trust_provenance": installed.provenance.trust.clone(),
            "previous_version": previous.map(|installed| installed.manifest.version),
            "superseded_authorization": superseded_grant
                .map(|grant| serde_json::json!({
                    "package_version": grant.artifact.package_version,
                    "permissions": grant.permissions,
                })),
        }),
    )?;
    Ok(installed)
}

pub fn uninstall(id: &str) -> DraftResult<InstalledExtension> {
    let home = DraftGlobalStore::locate()?;
    let mut registry = load(&home)?;
    let extension = registry
        .extensions
        .get(id)
        .cloned()
        .ok_or_else(|| DraftError::not_found(format!("extension '{id}' is not installed")))?;
    registry.extensions.remove(id);
    persist(&home, registry)?;
    // Provenance is preserved on uninstall, but authorization is not: there is
    // no longer an artifact for a grant to bind to.
    let superseded_grant = crate::authorization::supersede_for(id)?;
    let installed = packages(&home).join(id);
    if installed.exists() {
        let trash = home.extensions_dir().join("removed").join(format!(
            "{}-{}",
            id,
            draft_core::support::common::now().timestamp_millis()
        ));
        ensure_dir(trash.parent().unwrap_or(&home.extensions_dir()))?;
        fs::rename(&installed, &trash).map_err(|error| {
            DraftError::storage(format!("failed to preserve removed extension: {error}"))
        })?;
    }
    audit(
        draft_core::activity::GlobalAuditEvent::ExtensionUninstalled,
        id,
        &draft_core::support::common::OperationId::generate().to_string(),
        serde_json::json!({
            "version": extension.manifest.version.clone(),
            "artifact_digest": extension.content_hash.clone(),
            "provenance_preserved": true,
            "superseded_authorization": superseded_grant
                .map(|grant| serde_json::json!({ "permissions": grant.permissions })),
        }),
    )?;
    Ok(extension)
}

pub(crate) fn audit(
    event_type: draft_core::activity::GlobalAuditEvent,
    extension_id: &str,
    operation_id: &str,
    payload: serde_json::Value,
) -> DraftResult<()> {
    let home = DraftGlobalStore::locate()?;
    let actor_id =
        draft_core::trust::identity::global::load_actor(&home)?.map(|actor| actor.actor_id);
    draft_core::activity::GlobalAuditLog::global()?.append(
        event_type,
        actor_id,
        Some(extension_id.into()),
        Some(operation_id.into()),
        payload,
    )?;
    Ok(())
}

pub(crate) fn validate_manifest(root: &Path, manifest: &ExtensionManifest) -> DraftResult<()> {
    // The format decides what a manifest may declare; Draft decides whether
    // this filesystem actually holds it. Both run, in that order.
    in_draft(manifest.validate_shape(draft_core::DRAFT_API_VERSION))?;
    if !draft_core::contracts::supports_version(
        draft_core::contracts::ContractId::ExtensionManifest,
        manifest.schema_version,
    ) {
        return Err(DraftError::invalid_config(
            "extension manifest declares an unsupported schema version",
        ));
    }
    // A kind this build has no subsystem for is refused here, at the Draft
    // compatibility layer — not in the portable format, which must keep
    // accepting the whole published vocabulary so an author can target a Draft
    // newer than this one. Refusing is atomic: it happens before anything is
    // installed, so no package is ever recorded as active while one of its
    // declared contributions does nothing.
    for contribution in &manifest.contributions {
        if !draft_core::extension::supports_contribution(contribution.kind) {
            return Err(DraftError::invalid_config(format!(
                "this Draft build has no subsystem for the '{}' contribution \
                 declared by '{}'; nothing would consume it",
                serde_json::to_value(contribution.kind)
                    .ok()
                    .and_then(|value| value.as_str().map(ToOwned::to_owned))
                    .unwrap_or_else(|| "unknown".into()),
                contribution.id,
            )));
        }
    }
    for path in manifest.declared_paths() {
        resolve_declared_file(root, path)?;
    }
    Ok(())
}

/// Resolve one manifest-declared path against the real package directory.
///
/// The format has already ruled on the path's namespace and file type; this is
/// the filesystem half — Draft's path guard, then confirmation that a regular
/// file is really there.
fn resolve_declared_file(root: &Path, path: &str) -> DraftResult<PathBuf> {
    let normalized = draft_core::support::pathguard::check_relative(path).map_err(|error| {
        DraftError::new(
            DraftErrorKind::ProtectedResourceAccess,
            format!("unsafe extension path '{path}': {error}"),
        )
    })?;
    let target = draft_core::support::pathguard::safe_join(root, &normalized).map_err(|error| {
        DraftError::new(DraftErrorKind::ProtectedResourceAccess, error.to_string())
    })?;
    if !target.is_file() {
        return Err(DraftError::invalid_config(format!(
            "extension path '{path}' does not name a file in the package"
        )));
    }
    Ok(target)
}

/// The canonical byte stream this package's content hash is taken over.
///
/// The walk is Draft's — it is the party with a filesystem to defend — but the
/// stream's shape is the format's, so a publisher computing a digest outside
/// this repository gets the same answer.
fn package_bytes(source: &Path) -> DraftResult<Vec<u8>> {
    let mut entries = Vec::new();
    collect_files(source, source, &mut entries)?;
    in_draft(format_package::content_stream(&mut entries))
}

fn collect_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(String, Vec<u8>)>,
) -> DraftResult<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(DraftError::new(
                DraftErrorKind::ProtectedResourceAccess,
                "extension packages may not contain symlinks",
            ));
        }
        if file_type.is_dir() {
            collect_files(root, &entry.path(), output)?;
        } else if file_type.is_file() {
            let relative = relative_package_path(root, &entry.path())?;
            validate_package_file(&entry.path(), &relative)?;
            output.push((relative, fs::read(entry.path())?));
        } else {
            return Err(DraftError::new(
                DraftErrorKind::ProtectedResourceAccess,
                "extension packages may contain only regular files and directories",
            ));
        }
    }
    Ok(())
}

fn relative_package_path(root: &Path, path: &Path) -> DraftResult<String> {
    Ok(path
        .strip_prefix(root)
        .map_err(|error| DraftError::storage(error.to_string()))?
        .to_string_lossy()
        .replace('\\', "/"))
}

/// Refuse anything a declarative package may not contain.
///
/// Two independent rules apply. The filesystem rules — size, executable
/// permission bits, hardlinks — are Draft's, because only Draft is looking at a
/// real inode. Which relative paths a package may contain at all is the
/// format's rule, so the same answer holds for a publisher validating a package
/// before it is ever installed.
fn validate_package_file(path: &Path, relative: &str) -> DraftResult<()> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(DraftError::invalid_config(
            "extension file exceeds size limit",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o111 != 0 || metadata.nlink() > 1 {
            return Err(DraftError::new(
                DraftErrorKind::ProtectedResourceAccess,
                "extension packages reject executable permissions and hardlinks",
            ));
        }
    }
    if !format_package::is_admissible_package_path(relative) {
        return Err(DraftError::new(
            DraftErrorKind::ProtectedResourceAccess,
            format!("extension package rejects executable or unknown file '{relative}'"),
        ));
    }
    Ok(())
}

fn copy_package(source: &Path, destination: &Path) -> DraftResult<()> {
    if destination.exists() {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "extension staging destination already exists",
        ));
    }
    ensure_dir(destination)?;
    let result = (|| -> DraftResult<()> {
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let target = destination.join(entry.file_name());
            if file_type.is_dir() {
                copy_package(&entry.path(), &target)?;
            } else if file_type.is_file() {
                fs::copy(entry.path(), target)?;
            } else {
                return Err(DraftError::new(
                    DraftErrorKind::ProtectedResourceAccess,
                    "extension packages contain an unsafe filesystem entry",
                ));
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ExtensionManifest {
        ExtensionManifest {
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::ExtensionManifest,
            ),
            id: ExtensionId::parse("example.docs").unwrap(),
            name: "Example docs".into(),
            version: "1.0.0".into(),
            publisher: "example".into(),
            draft_api: format!("^{}", draft_core::DRAFT_API_VERSION),
            contributions: vec![ExtensionContribution {
                id: "classification".into(),
                kind: ExtensionContributionKind::ResourceClassification,
                path: "contributions/task.json".into(),
            }],
            permissions: vec![],
            description: None,
            keywords: vec![],
            documentation: vec!["docs/readme.md".into()],
            licenses: vec!["LICENSE.txt".into()],
            assets: vec![],
            schemas: vec![],
        }
    }

    #[test]
    fn declarative_package_is_accepted() {
        let temp = tempfile::tempdir().unwrap();
        ensure_dir(&temp.path().join("contributions")).unwrap();
        ensure_dir(&temp.path().join("docs")).unwrap();
        write_json(&temp.path().join("extension.json"), &manifest()).unwrap();
        fs::write(temp.path().join("contributions/task.json"), "{}").unwrap();
        fs::write(temp.path().join("docs/readme.md"), "docs").unwrap();
        fs::write(temp.path().join("LICENSE.txt"), "license").unwrap();
        validate_manifest(temp.path(), &manifest()).unwrap();
        assert!(!package_bytes(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn executable_and_unknown_content_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        write_json(&temp.path().join("extension.json"), &manifest()).unwrap();
        fs::write(temp.path().join("entrypoint.sh"), "#!/bin/sh").unwrap();
        let error = package_bytes(temp.path()).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ProtectedResourceAccess);
    }
}
