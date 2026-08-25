//! Uniform, non-executing Draft extension packages.
//!
//! Extensions contribute validated declarative records only. No source type,
//! manifest field, asset, or installed file can introduce executable code.

use draft_core::support::error::{DraftError, DraftErrorKind, DraftResult};
use draft_core::support::fsutil::{ensure_dir, write_json};
use draft_core::support::hashing::sha256_hex;
use draft_core::workspace::home::DraftGlobalStore;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_PACKAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn draft_api_compatible(requirement: &str) -> bool {
    let Ok(requirement) = semver::VersionReq::parse(requirement) else {
        return false;
    };
    semver::Version::parse(draft_core::DRAFT_API_VERSION)
        .is_ok_and(|version| requirement.matches(&version))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub draft_api: String,
    pub contributions: Vec<ExtensionContribution>,
    pub documentation: Vec<String>,
    pub licenses: Vec<String>,
    pub assets: Vec<String>,
}

impl draft_core::contracts::VersionedContract for ExtensionManifest {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::ExtensionManifest;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionContribution {
    pub id: String,
    pub kind: ExtensionContributionKind,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionContributionKind {
    TaskTemplate,
    PolicyPreset,
    RiskRule,
    FileAssociation,
    Documentation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledExtensionProvenance {
    pub schema_version: u32,
    pub package_id: String,
    pub package_version: String,
    pub operation_id: String,
    pub trust: ExtensionTrustProvenance,
}

impl draft_core::contracts::VersionedContract for InstalledExtensionProvenance {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::InstalledExtensionProvenance;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source_kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtensionTrustProvenance {
    HttpsCatalog {
        source_id: String,
        catalog_id: String,
        trusted_root_fingerprint: String,
        signed_metadata_counters: BTreeMap<String, u64>,
        target_digest: String,
        verified_at: String,
    },
    TrustedLocalCatalog {
        source_id: String,
        catalog_id: String,
        local_source: String,
        trust_root_id: String,
        trust_decision_id: String,
        signed_metadata_counters: BTreeMap<String, u64>,
        target_digest: String,
        verified_at: String,
    },
    DirectLocal {
        decision_id: String,
        actor_id: String,
        decided_at: String,
        source_description: String,
        artifact_digest: String,
    },
}

impl InstalledExtensionProvenance {
    pub fn artifact_digest(&self) -> &str {
        match &self.trust {
            ExtensionTrustProvenance::HttpsCatalog { target_digest, .. }
            | ExtensionTrustProvenance::TrustedLocalCatalog { target_digest, .. } => target_digest,
            ExtensionTrustProvenance::DirectLocal {
                artifact_digest, ..
            } => artifact_digest,
        }
    }

    pub fn catalog_source_id(&self) -> Option<&str> {
        match &self.trust {
            ExtensionTrustProvenance::HttpsCatalog { source_id, .. }
            | ExtensionTrustProvenance::TrustedLocalCatalog { source_id, .. } => Some(source_id),
            ExtensionTrustProvenance::DirectLocal { .. } => None,
        }
    }

    pub fn catalog_id(&self) -> Option<&str> {
        match &self.trust {
            ExtensionTrustProvenance::HttpsCatalog { catalog_id, .. }
            | ExtensionTrustProvenance::TrustedLocalCatalog { catalog_id, .. } => Some(catalog_id),
            ExtensionTrustProvenance::DirectLocal { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledExtension {
    pub manifest: ExtensionManifest,
    pub content_hash: String,
    pub enabled: bool,
    pub installed_at: String,
    pub provenance: InstalledExtensionProvenance,
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

pub fn active_contributions() -> DraftResult<Vec<(String, ExtensionContribution)>> {
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
    audit_extension(
        if enabled {
            "extension.enabled"
        } else {
            "extension.disabled"
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
        package_id: manifest.id.clone(),
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
    let previous = registry.extensions.get(&manifest.id).cloned();
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

    let destination = packages(&home).join(&manifest.id);
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
        .insert(manifest.id.clone(), installed.clone());
    if let Err(error) = persist(&home, registry) {
        let _ = fs::remove_dir_all(&destination);
        if let Some(backup) = &backup {
            let _ = fs::rename(backup, &destination);
        }
        return Err(error);
    }
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
    audit_extension(
        if previous.is_some() {
            "extension.updated"
        } else {
            "extension.installed"
        },
        &manifest.id,
        &provenance.operation_id,
        serde_json::json!({
            "version": manifest.version,
            "artifact_digest": installed.provenance.artifact_digest(),
            "trust_provenance": installed.provenance.trust.clone(),
            "previous_version": previous.map(|installed| installed.manifest.version),
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
    audit_extension(
        "extension.uninstalled",
        id,
        &draft_core::support::common::OperationId::generate().to_string(),
        serde_json::json!({
            "version": extension.manifest.version.clone(),
            "artifact_digest": extension.content_hash.clone(),
            "provenance_preserved": true,
        }),
    )?;
    Ok(extension)
}

fn audit_extension(
    event_type: &str,
    extension_id: &str,
    operation_id: &str,
    payload: serde_json::Value,
) -> DraftResult<()> {
    let home = DraftGlobalStore::locate()?;
    let actor_id =
        draft_core::trust::identity::global::load_actor(&home)?.map(|actor| actor.actor_id);
    draft_core::trust::audit::GlobalAuditLog::global()?.append(
        event_type,
        actor_id,
        Some(extension_id.into()),
        Some(operation_id.into()),
        payload,
    )?;
    Ok(())
}

pub(crate) fn validate_manifest(root: &Path, manifest: &ExtensionManifest) -> DraftResult<()> {
    let id_valid = !manifest.id.is_empty()
        && manifest.id.len() <= 64
        && manifest.id.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '.')
        });
    if !id_valid
        || !draft_core::contracts::supports_version(
            draft_core::contracts::ContractId::ExtensionManifest,
            manifest.schema_version,
        )
        || !draft_api_compatible(&manifest.draft_api)
        || manifest.name.trim().is_empty()
        || manifest.publisher.trim().is_empty()
    {
        return Err(DraftError::invalid_config(
            "extension manifest identity/schema/API compatibility is invalid",
        ));
    }
    let mut contribution_ids = BTreeSet::new();
    for contribution in &manifest.contributions {
        if contribution.id.trim().is_empty() || !contribution_ids.insert(&contribution.id) {
            return Err(DraftError::invalid_config(
                "extension contribution ids must be non-empty and unique",
            ));
        }
        validate_declared_path(root, &contribution.path, "contributions", &["json", "toml"])?;
    }
    for path in &manifest.documentation {
        validate_declared_path(root, path, "docs", &["md", "txt"])?;
    }
    for path in &manifest.licenses {
        validate_declared_path(root, path, "", &["md", "txt"])?;
        let name = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !name.starts_with("license") && !name.starts_with("notice") {
            return Err(DraftError::invalid_config(
                "license paths must name LICENSE or NOTICE files",
            ));
        }
    }
    for path in &manifest.assets {
        validate_declared_path(root, path, "assets", &["png", "jpg", "jpeg", "webp"])?;
    }
    Ok(())
}

fn validate_declared_path(
    root: &Path,
    path: &str,
    required_prefix: &str,
    allowed_extensions: &[&str],
) -> DraftResult<()> {
    let normalized = draft_core::support::pathguard::check_relative(path).map_err(|error| {
        DraftError::new(
            DraftErrorKind::ProtectedFileAccess,
            format!("unsafe extension path '{path}': {error}"),
        )
    })?;
    if !required_prefix.is_empty()
        && normalized
            .split('/')
            .next()
            .is_none_or(|prefix| prefix != required_prefix)
    {
        return Err(DraftError::invalid_config(format!(
            "extension path '{path}' must be under {required_prefix}/"
        )));
    }
    let target = draft_core::support::pathguard::safe_join(root, &normalized)
        .map_err(|error| DraftError::new(DraftErrorKind::ProtectedFileAccess, error.to_string()))?;
    let extension = target
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !target.is_file() || !allowed_extensions.contains(&extension) {
        return Err(DraftError::invalid_config(format!(
            "extension path '{path}' has an unsupported static file type"
        )));
    }
    Ok(())
}

fn package_bytes(source: &Path) -> DraftResult<Vec<u8>> {
    let mut files = Vec::new();
    collect_files(source, source, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut output = Vec::new();
    for (relative, path) in files {
        output.extend_from_slice(relative.as_bytes());
        output.push(0);
        output.extend_from_slice(&fs::read(path)?);
        output.push(0);
        if output.len() as u64 > MAX_PACKAGE_BYTES {
            return Err(DraftError::invalid_config(
                "extension package exceeds size limit",
            ));
        }
    }
    Ok(output)
}

fn collect_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(String, PathBuf)>,
) -> DraftResult<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(DraftError::new(
                DraftErrorKind::ProtectedFileAccess,
                "extension packages may not contain symlinks",
            ));
        }
        if file_type.is_dir() {
            collect_files(root, &entry.path(), output)?;
        } else if file_type.is_file() {
            validate_package_file(root, &entry.path())?;
            output.push((
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|error| DraftError::storage(error.to_string()))?
                    .to_string_lossy()
                    .replace('\\', "/"),
                entry.path(),
            ));
        } else {
            return Err(DraftError::new(
                DraftErrorKind::ProtectedFileAccess,
                "extension packages may contain only regular files and directories",
            ));
        }
    }
    Ok(())
}

fn validate_package_file(root: &Path, path: &Path) -> DraftResult<()> {
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
                DraftErrorKind::ProtectedFileAccess,
                "extension packages reject executable permissions and hardlinks",
            ));
        }
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|error| DraftError::storage(error.to_string()))?
        .to_string_lossy()
        .replace('\\', "/");
    let allowed = relative == "extension.json"
        || relative.starts_with("contributions/")
            && matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("json" | "toml")
            )
        || relative.starts_with("docs/")
            && matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("md" | "txt")
            )
        || relative.starts_with("assets/")
            && matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("png" | "jpg" | "jpeg" | "webp")
            )
        || relative.rsplit('/').next().is_some_and(|name| {
            name.to_ascii_lowercase().starts_with("license")
                || name.to_ascii_lowercase().starts_with("notice")
        }) && matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("md" | "txt")
        );
    if !allowed {
        return Err(DraftError::new(
            DraftErrorKind::ProtectedFileAccess,
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
                    DraftErrorKind::ProtectedFileAccess,
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
            id: "example.docs".into(),
            name: "Example docs".into(),
            version: "1.0.0".into(),
            publisher: "example".into(),
            draft_api: format!("^{}", draft_core::DRAFT_API_VERSION),
            contributions: vec![ExtensionContribution {
                id: "task-review".into(),
                kind: ExtensionContributionKind::TaskTemplate,
                path: "contributions/task.json".into(),
            }],
            documentation: vec!["docs/readme.md".into()],
            licenses: vec!["LICENSE.txt".into()],
            assets: vec![],
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
        assert_eq!(error.kind, DraftErrorKind::ProtectedFileAccess);
    }
}
