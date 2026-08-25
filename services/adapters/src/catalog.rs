//! Signed extension catalogs and durable trust state.
//!
//! Catalogs use a compact TUF-style chain: an explicitly bootstrapped root
//! authorizes timestamp, snapshot, and targets roles. Snapshot metadata may
//! authenticate one level of namespace-restricted delegated targets. Catalog
//! configuration, trust, and current usability are deliberately independent.

use crate::extension::{
    self, ExtensionTrustProvenance, InstalledExtension, InstalledExtensionProvenance,
};
use chrono::{DateTime, Utc};
use draft_core::support::error::{DraftError, DraftErrorKind, DraftResult};
use draft_core::support::fsutil::{ensure_dir, write_atomic, write_json};
use draft_core::support::hashing::{canonical_json, sha256_hex};
use draft_core::trust::signing::verify_b64;
use draft_core::workspace::home::DraftGlobalStore;
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_METADATA_BYTES: usize = 2 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogLocation {
    LocalDirectory { path: String },
    Https { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSource {
    pub schema_version: u32,
    pub id: String,
    pub location: CatalogLocation,
    pub configured_at: String,
    pub catalog_id: Option<String>,
    pub last_refreshed_at: Option<String>,
    pub last_error: Option<String>,
}

impl draft_core::contracts::VersionedContract for CatalogSource {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::CatalogSource;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRegistry {
    schema_version: u32,
    sources: BTreeMap<String, CatalogSource>,
}

impl draft_core::contracts::VersionedContract for SourceRegistry {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::CatalogSourceRegistry;
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self {
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::CatalogSourceRegistry,
            ),
            sources: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogUsability {
    Untrusted,
    Usable,
    Expired,
    Invalid,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSourceStatus {
    pub source: CatalogSource,
    pub configured: bool,
    pub trusted: bool,
    pub usability: CatalogUsability,
    #[serde(default)]
    pub root_version: Option<u64>,
    #[serde(default)]
    pub cached_package_count: usize,
    #[serde(default)]
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureRecord {
    pub key_id: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedEnvelope<T> {
    pub signed: T,
    pub signatures: Vec<SignatureRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogKey {
    pub algorithm: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSpec {
    pub key_ids: Vec<String>,
    pub threshold: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootMetadata {
    pub schema_version: u32,
    pub catalog_id: String,
    pub role: String,
    pub version: u64,
    pub expires_at: String,
    pub keys: BTreeMap<String, CatalogKey>,
    pub roles: BTreeMap<String, RoleSpec>,
    pub revoked_key_ids: Vec<String>,
    pub revoked_packages: Vec<String>,
}

impl draft_core::contracts::VersionedContract for RootMetadata {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::CatalogRoot;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataDescriptor {
    pub version: u64,
    pub length: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimestampMetadata {
    pub schema_version: u32,
    pub catalog_id: String,
    pub role: String,
    pub version: u64,
    pub expires_at: String,
    pub snapshot: MetadataDescriptor,
}

impl draft_core::contracts::VersionedContract for TimestampMetadata {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::CatalogTimestamp;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotMetadata {
    pub schema_version: u32,
    pub catalog_id: String,
    pub role: String,
    pub version: u64,
    pub expires_at: String,
    pub roles: BTreeMap<String, MetadataDescriptor>,
}

impl draft_core::contracts::VersionedContract for SnapshotMetadata {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::CatalogSnapshot;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delegation {
    pub role: String,
    pub keys: BTreeMap<String, CatalogKey>,
    pub key_ids: Vec<String>,
    pub threshold: u32,
    pub path_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogTarget {
    pub id: String,
    pub version: String,
    pub publisher: String,
    pub draft_api: String,
    pub artifact_path: String,
    pub length: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetsMetadata {
    pub schema_version: u32,
    pub catalog_id: String,
    pub role: String,
    pub version: u64,
    pub expires_at: String,
    pub packages: Vec<CatalogTarget>,
    pub delegations: Vec<Delegation>,
}

impl draft_core::contracts::VersionedContract for TargetsMetadata {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::CatalogTargets;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleFloor {
    version: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogTrust {
    schema_version: u32,
    catalog_id: String,
    trusted_root: SignedEnvelope<RootMetadata>,
    trusted_root_digest: String,
    trusted_at: String,
    reset_count: u64,
    floors: BTreeMap<String, RoleFloor>,
}

impl draft_core::contracts::VersionedContract for CatalogTrust {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::CatalogTrust;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedCatalog {
    schema_version: u32,
    catalog_id: String,
    root_version: u64,
    expires_at: BTreeMap<String, String>,
    metadata_versions: BTreeMap<String, u64>,
    packages: Vec<CatalogTarget>,
    refreshed_at: String,
}

impl draft_core::contracts::VersionedContract for CachedCatalog {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::CachedCatalog;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredExtension {
    pub source_id: String,
    pub catalog_id: String,
    pub freshness: CatalogUsability,
    pub target: CatalogTarget,
}

pub fn source_add(id: &str, location: &str) -> DraftResult<CatalogSourceStatus> {
    validate_source_id(id)?;
    let home = DraftGlobalStore::locate()?;
    let mut registry = load_sources(&home)?;
    let parsed = parse_location(location)?;
    if let Some(existing) = registry.sources.get(id) {
        if existing.location == parsed {
            return status_for(&home, existing.clone());
        }
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            format!("extension source '{id}' is already configured differently"),
        ));
    }
    let source = CatalogSource {
        schema_version: draft_core::contracts::current_version(
            draft_core::contracts::ContractId::CatalogSource,
        ),
        id: id.into(),
        location: parsed,
        configured_at: Utc::now().to_rfc3339(),
        catalog_id: None,
        last_refreshed_at: None,
        last_error: None,
    };
    registry.sources.insert(id.into(), source.clone());
    persist_sources(&home, &registry)?;
    audit_catalog(
        "extension.source.configured",
        id,
        serde_json::json!({ "location": source.location }),
    )?;
    status_for(&home, source)
}

pub fn source_list() -> DraftResult<Vec<CatalogSourceStatus>> {
    let home = DraftGlobalStore::locate()?;
    let registry = load_sources(&home)?;
    registry
        .sources
        .into_values()
        .map(|source| status_for(&home, source))
        .collect()
}

pub fn source_remove(id: &str) -> DraftResult<CatalogSource> {
    let home = DraftGlobalStore::locate()?;
    let mut registry = load_sources(&home)?;
    let source = registry.sources.remove(id).ok_or_else(|| {
        DraftError::not_found(format!("extension source '{id}' is not configured"))
    })?;
    persist_sources(&home, &registry)?;
    audit_catalog(
        "extension.source.removed",
        id,
        serde_json::json!({
            "catalog_id": source.catalog_id,
            "trust_and_install_provenance_preserved": true,
        }),
    )?;
    Ok(source)
}

pub fn trust_source(
    id: &str,
    root_path: &Path,
    expected_fingerprint: &str,
    reset: bool,
) -> DraftResult<CatalogSourceStatus> {
    let bytes = read_bounded(root_path, MAX_METADATA_BYTES)?;
    trust_source_bytes(id, &bytes, expected_fingerprint, reset)
}

pub fn trust_source_bytes(
    id: &str,
    bytes: &[u8],
    expected_fingerprint: &str,
    reset: bool,
) -> DraftResult<CatalogSourceStatus> {
    if bytes.len() > MAX_METADATA_BYTES {
        return Err(invalid("root metadata exceeds its size limit"));
    }
    let home = DraftGlobalStore::locate()?;
    let mut registry = load_sources(&home)?;
    let source = registry.sources.get_mut(id).ok_or_else(|| {
        DraftError::not_found(format!("extension source '{id}' is not configured"))
    })?;
    let digest = digest(bytes);
    if digest != expected_fingerprint {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            format!("root fingerprint mismatch: expected {expected_fingerprint}, got {digest}"),
        ));
    }
    let root: SignedEnvelope<RootMetadata> = parse_json(bytes, "root metadata")?;
    validate_root_shape(&root.signed)?;
    verify_role(
        &root,
        root.signed
            .roles
            .get("root")
            .ok_or_else(|| invalid("root metadata does not define the root role"))?,
        &root.signed.keys,
        &root.signed.revoked_key_ids,
    )?;
    require_unexpired("root", &root.signed.expires_at)?;
    let existing = load_trust(&home, id)?;
    if existing.is_some() && !reset {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "source is already trusted; use signed root rotation or an explicit --reset",
        ));
    }
    let reset_count = existing.map_or(0, |state| state.reset_count.saturating_add(1));
    let mut floors = BTreeMap::new();
    floors.insert(
        "root".into(),
        RoleFloor {
            version: root.signed.version,
            sha256: digest.clone(),
        },
    );
    let trust = CatalogTrust {
        schema_version: draft_core::contracts::current_version(
            draft_core::contracts::ContractId::CatalogTrust,
        ),
        catalog_id: root.signed.catalog_id.clone(),
        trusted_root: root,
        trusted_root_digest: digest.clone(),
        trusted_at: Utc::now().to_rfc3339(),
        reset_count,
        floors,
    };
    persist_trust(&home, id, &trust)?;
    source.catalog_id = Some(trust.catalog_id.clone());
    source.last_error = None;
    let result = source.clone();
    persist_sources(&home, &registry)?;
    audit_catalog(
        if reset {
            "extension.trust.reset"
        } else {
            "extension.trust.bootstrapped"
        },
        id,
        serde_json::json!({
            "catalog_id": trust.catalog_id,
            "root_version": trust.trusted_root.signed.version,
            "fingerprint": digest,
            "out_of_band": true,
        }),
    )?;
    status_for(&home, result)
}

pub fn source_refresh(id: &str) -> DraftResult<CatalogSourceStatus> {
    let home = DraftGlobalStore::locate()?;
    let mut registry = load_sources(&home)?;
    let mut source = registry.sources.get(id).cloned().ok_or_else(|| {
        DraftError::not_found(format!("extension source '{id}' is not configured"))
    })?;
    let result = refresh_inner(&home, &source);
    match result {
        Ok((trust, cache, metadata)) => {
            commit_cache(&home, id, &cache, &metadata)?;
            persist_trust(&home, id, &trust)?;
            source.catalog_id = Some(trust.catalog_id.clone());
            source.last_refreshed_at = Some(cache.refreshed_at.clone());
            source.last_error = None;
            registry.sources.insert(id.into(), source.clone());
            persist_sources(&home, &registry)?;
            audit_catalog(
                "extension.source.refreshed",
                id,
                serde_json::json!({
                    "catalog_id": cache.catalog_id,
                    "root_version": cache.root_version,
                    "metadata_versions": cache.metadata_versions,
                    "package_count": cache.packages.len(),
                }),
            )?;
            status_for(&home, source)
        }
        Err(error) => {
            source.last_error = Some(error.to_string());
            registry.sources.insert(id.into(), source);
            persist_sources(&home, &registry)?;
            Err(error)
        }
    }
}

pub fn discover(query: Option<&str>) -> DraftResult<Vec<DiscoveredExtension>> {
    let home = DraftGlobalStore::locate()?;
    let query = query.unwrap_or_default().to_ascii_lowercase();
    let mut results = Vec::new();
    for source in load_sources(&home)?.sources.into_values() {
        let cache_path = catalog_cache_root(&home, &source.id).join("catalog.json");
        if !cache_path.exists() {
            continue;
        }
        let cache = load_cache(&home, &source.id)?;
        let freshness = cache_usability(&cache);
        for target in cache.packages {
            if query.is_empty()
                || target.id.to_ascii_lowercase().contains(&query)
                || target.publisher.to_ascii_lowercase().contains(&query)
            {
                results.push(DiscoveredExtension {
                    source_id: source.id.clone(),
                    catalog_id: cache.catalog_id.clone(),
                    freshness: freshness.clone(),
                    target,
                });
            }
        }
    }
    results.sort_by(|left, right| {
        left.target
            .id
            .cmp(&right.target.id)
            .then_with(|| compare_versions(&right.target.version, &left.target.version))
            .then_with(|| left.source_id.cmp(&right.source_id))
    });
    Ok(results)
}

pub fn install_from_source(
    source_id: &str,
    package_id: &str,
    version: Option<&str>,
) -> DraftResult<InstalledExtension> {
    install_or_update(source_id, package_id, version, false)
}

pub fn update_from_source(
    source_id: &str,
    package_id: &str,
    version: Option<&str>,
) -> DraftResult<InstalledExtension> {
    install_or_update(source_id, package_id, version, true)
}

pub fn update_all() -> DraftResult<Vec<InstalledExtension>> {
    let installed = extension::list()?;
    let available = discover(None)?;
    let mut updated = Vec::new();
    for current in installed {
        let Some(source_id) = current.provenance.catalog_source_id().map(str::to_owned) else {
            continue;
        };
        let latest = available
            .iter()
            .filter(|candidate| {
                candidate.source_id == source_id && candidate.target.id == current.manifest.id
            })
            .max_by(|left, right| compare_versions(&left.target.version, &right.target.version));
        if latest.is_some_and(|candidate| {
            compare_versions(&candidate.target.version, &current.manifest.version).is_gt()
        }) {
            updated.push(update_from_source(&source_id, &current.manifest.id, None)?);
        }
    }
    Ok(updated)
}

fn install_or_update(
    source_id: &str,
    package_id: &str,
    version: Option<&str>,
    update: bool,
) -> DraftResult<InstalledExtension> {
    let home = DraftGlobalStore::locate()?;
    let source = load_sources(&home)?
        .sources
        .remove(source_id)
        .ok_or_else(|| {
            DraftError::not_found(format!("extension source '{source_id}' is not configured"))
        })?;
    let trust = load_trust(&home, source_id)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::ReviewRequired,
            "extension source is configured but has no accepted trust root",
        )
    })?;
    let cache = load_cache(&home, source_id)?;
    if cache_usability(&cache) != CatalogUsability::Usable {
        return Err(DraftError::new(
            DraftErrorKind::EvidenceStale,
            "catalog metadata is expired or incomplete; install and update are blocked",
        ));
    }
    if cache.catalog_id != trust.catalog_id {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "cached catalog identity does not match the trusted root",
        ));
    }
    let target = choose_target(&cache.packages, package_id, version)?;
    if !extension::draft_api_compatible(&target.draft_api) {
        return Err(invalid(format!(
            "catalog target is incompatible with Draft API {}",
            draft_core::DRAFT_API_VERSION
        )));
    }
    let bytes = fetch_or_cached_artifact(&home, &source, &target)?;
    verify_descriptor_bytes(
        &bytes,
        &MetadataDescriptor {
            version: 0,
            length: target.length,
            sha256: target.sha256.clone(),
        },
        "extension artifact",
    )?;
    let extracted = tempfile::tempdir()
        .map_err(|error| DraftError::storage(format!("create extension staging dir: {error}")))?;
    extract_archive(&bytes, extracted.path())?;
    let manifest = extension::read_package_manifest(&extracted.path().join("extension.json"))?;
    extension::validate_manifest(extracted.path(), &manifest)?;
    if manifest.id != target.id
        || manifest.version != target.version
        || manifest.publisher != target.publisher
        || manifest.draft_api != target.draft_api
    {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "extension artifact manifest does not match signed targets metadata",
        ));
    }
    let operation_id = draft_core::support::common::OperationId::generate().to_string();
    let installed_at = Utc::now().to_rfc3339();
    let trust_provenance = match &source.location {
        CatalogLocation::Https { .. } => ExtensionTrustProvenance::HttpsCatalog {
            source_id: source_id.to_string(),
            catalog_id: cache.catalog_id.clone(),
            trusted_root_fingerprint: trust.trusted_root_digest.clone(),
            signed_metadata_counters: cache.metadata_versions.clone(),
            target_digest: target.sha256.clone(),
            verified_at: installed_at.clone(),
        },
        CatalogLocation::LocalDirectory { path } => ExtensionTrustProvenance::TrustedLocalCatalog {
            source_id: source_id.to_string(),
            catalog_id: cache.catalog_id.clone(),
            local_source: path.clone(),
            trust_root_id: trust.trusted_root_digest.clone(),
            trust_decision_id: format!("trust:{}", trust.trusted_root_digest),
            signed_metadata_counters: cache.metadata_versions.clone(),
            target_digest: target.sha256.clone(),
            verified_at: installed_at.clone(),
        },
    };
    let provenance = InstalledExtensionProvenance {
        schema_version: draft_core::contracts::current_version(
            draft_core::contracts::ContractId::InstalledExtensionProvenance,
        ),
        package_id: target.id.clone(),
        package_version: target.version.clone(),
        operation_id,
        trust: trust_provenance,
    };
    extension::install_catalog_package(extracted.path(), provenance, update)
}

fn refresh_inner(
    home: &DraftGlobalStore,
    source: &CatalogSource,
) -> DraftResult<(CatalogTrust, CachedCatalog, BTreeMap<String, Vec<u8>>)> {
    let mut trust = load_trust(home, &source.id)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::ReviewRequired,
            "catalog is configured but untrusted; bootstrap an out-of-band root first",
        )
    })?;
    if !draft_core::contracts::supports_version(
        draft_core::contracts::ContractId::CatalogTrust,
        trust.schema_version,
    ) {
        return Err(invalid("unsupported extension trust schema"));
    }
    let root_bytes = fetch_source_file(source, "root.json", MAX_METADATA_BYTES)?;
    let root: SignedEnvelope<RootMetadata> = parse_json(&root_bytes, "root metadata")?;
    validate_root_shape(&root.signed)?;
    if root.signed.catalog_id != trust.catalog_id {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "catalog identity changed across root metadata",
        ));
    }
    let root_digest = digest(&root_bytes);
    let current = &trust.trusted_root;
    if root.signed.version < current.signed.version {
        return Err(rollback(
            "root",
            root.signed.version,
            current.signed.version,
        ));
    }
    if root.signed.version == current.signed.version {
        if root_digest != trust.trusted_root_digest {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "root metadata reused a version with different content",
            ));
        }
    } else {
        require_unexpired("trusted root", &current.signed.expires_at)?;
        if root.signed.version != current.signed.version.saturating_add(1) {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "root rotations must be applied one sequential version at a time",
            ));
        }
        verify_role(
            &root,
            current
                .signed
                .roles
                .get("root")
                .ok_or_else(|| invalid("trusted root role is missing"))?,
            &current.signed.keys,
            &current.signed.revoked_key_ids,
        )?;
        verify_role(
            &root,
            root.signed
                .roles
                .get("root")
                .ok_or_else(|| invalid("new root role is missing"))?,
            &root.signed.keys,
            &root.signed.revoked_key_ids,
        )?;
        trust.trusted_root = root.clone();
        trust.trusted_root_digest = root_digest.clone();
    }
    require_unexpired("root", &root.signed.expires_at)?;
    check_and_raise_floor(&mut trust, "root", root.signed.version, &root_digest)?;
    // Root rotation and revocation are security-critical independently of
    // catalog availability. Persist them before fetching lower roles so a
    // timestamp outage cannot revive revoked authority or packages.
    persist_trust(home, &source.id, &trust)?;

    let timestamp_bytes = fetch_source_file(source, "timestamp.json", MAX_METADATA_BYTES)?;
    let timestamp: SignedEnvelope<TimestampMetadata> =
        parse_json(&timestamp_bytes, "timestamp metadata")?;
    validate_role_header(
        draft_core::contracts::ContractId::CatalogTimestamp,
        &timestamp.signed.schema_version,
        &timestamp.signed.catalog_id,
        &timestamp.signed.role,
        &trust.catalog_id,
        "timestamp",
    )?;
    verify_root_role(&root.signed, "timestamp", &timestamp)?;
    require_unexpired("timestamp", &timestamp.signed.expires_at)?;
    check_and_raise_floor(
        &mut trust,
        "timestamp",
        timestamp.signed.version,
        &digest(&timestamp_bytes),
    )?;

    let snapshot_bytes = fetch_source_file(source, "snapshot.json", MAX_METADATA_BYTES)?;
    verify_descriptor_bytes(&snapshot_bytes, &timestamp.signed.snapshot, "snapshot")?;
    let snapshot: SignedEnvelope<SnapshotMetadata> =
        parse_json(&snapshot_bytes, "snapshot metadata")?;
    validate_role_header(
        draft_core::contracts::ContractId::CatalogSnapshot,
        &snapshot.signed.schema_version,
        &snapshot.signed.catalog_id,
        &snapshot.signed.role,
        &trust.catalog_id,
        "snapshot",
    )?;
    if snapshot.signed.version != timestamp.signed.snapshot.version {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "snapshot version does not match timestamp metadata",
        ));
    }
    verify_root_role(&root.signed, "snapshot", &snapshot)?;
    require_unexpired("snapshot", &snapshot.signed.expires_at)?;
    check_and_raise_floor(
        &mut trust,
        "snapshot",
        snapshot.signed.version,
        &digest(&snapshot_bytes),
    )?;

    let targets_descriptor = snapshot
        .signed
        .roles
        .get("targets")
        .ok_or_else(|| invalid("snapshot metadata does not authenticate targets"))?;
    let targets_bytes = fetch_source_file(source, "targets.json", MAX_METADATA_BYTES)?;
    verify_descriptor_bytes(&targets_bytes, targets_descriptor, "targets")?;
    let targets: SignedEnvelope<TargetsMetadata> = parse_json(&targets_bytes, "targets metadata")?;
    validate_role_header(
        draft_core::contracts::ContractId::CatalogTargets,
        &targets.signed.schema_version,
        &targets.signed.catalog_id,
        &targets.signed.role,
        &trust.catalog_id,
        "targets",
    )?;
    if targets.signed.version != targets_descriptor.version {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "targets version does not match snapshot metadata",
        ));
    }
    verify_root_role(&root.signed, "targets", &targets)?;
    require_unexpired("targets", &targets.signed.expires_at)?;
    check_and_raise_floor(
        &mut trust,
        "targets",
        targets.signed.version,
        &digest(&targets_bytes),
    )?;
    let mut packages = targets.signed.packages.clone();
    validate_targets(&packages)?;

    let mut metadata = BTreeMap::from([
        ("root.json".into(), root_bytes),
        ("timestamp.json".into(), timestamp_bytes),
        ("snapshot.json".into(), snapshot_bytes),
        ("targets.json".into(), targets_bytes),
    ]);
    let mut expirations = BTreeMap::from([
        ("root".into(), root.signed.expires_at.clone()),
        ("timestamp".into(), timestamp.signed.expires_at.clone()),
        ("snapshot".into(), snapshot.signed.expires_at.clone()),
        ("targets".into(), targets.signed.expires_at.clone()),
    ]);
    let mut versions = BTreeMap::from([
        ("root".into(), root.signed.version),
        ("timestamp".into(), timestamp.signed.version),
        ("snapshot".into(), snapshot.signed.version),
        ("targets".into(), targets.signed.version),
    ]);
    for delegation in &targets.signed.delegations {
        validate_delegation(delegation)?;
        let descriptor = snapshot.signed.roles.get(&delegation.role).ok_or_else(|| {
            invalid(format!(
                "snapshot does not authenticate delegated role '{}'",
                delegation.role
            ))
        })?;
        let filename = format!("{}.json", delegation.role);
        let bytes = fetch_source_file(source, &filename, MAX_METADATA_BYTES)?;
        verify_descriptor_bytes(&bytes, descriptor, &delegation.role)?;
        let delegated: SignedEnvelope<TargetsMetadata> =
            parse_json(&bytes, "delegated targets metadata")?;
        validate_role_header(
            draft_core::contracts::ContractId::CatalogTargets,
            &delegated.signed.schema_version,
            &delegated.signed.catalog_id,
            &delegated.signed.role,
            &trust.catalog_id,
            &delegation.role,
        )?;
        if delegated.signed.version != descriptor.version {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "delegated role version does not match snapshot metadata",
            ));
        }
        let spec = RoleSpec {
            key_ids: delegation.key_ids.clone(),
            threshold: delegation.threshold,
        };
        verify_role(&delegated, &spec, &delegation.keys, &[])?;
        require_unexpired(&delegation.role, &delegated.signed.expires_at)?;
        if !delegated.signed.delegations.is_empty() {
            return Err(invalid(
                "nested delegations are rejected because they could expand parent authority",
            ));
        }
        for package in &delegated.signed.packages {
            if !delegation
                .path_prefixes
                .iter()
                .any(|prefix| package.id.starts_with(prefix))
            {
                return Err(DraftError::new(
                    DraftErrorKind::ProtectedFileAccess,
                    format!(
                        "delegated role '{}' attempted to publish outside its namespace",
                        delegation.role
                    ),
                ));
            }
        }
        validate_targets(&delegated.signed.packages)?;
        packages.extend(delegated.signed.packages);
        let role_digest = digest(&bytes);
        check_and_raise_floor(
            &mut trust,
            &delegation.role,
            delegated.signed.version,
            &role_digest,
        )?;
        metadata.insert(filename, bytes);
        expirations.insert(delegation.role.clone(), delegated.signed.expires_at);
        versions.insert(delegation.role.clone(), delegated.signed.version);
    }
    reject_target_mix_and_match(&packages)?;
    packages.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| compare_versions(&left.version, &right.version))
    });
    let cache = CachedCatalog {
        schema_version: draft_core::contracts::current_version(
            draft_core::contracts::ContractId::CachedCatalog,
        ),
        catalog_id: trust.catalog_id.clone(),
        root_version: root.signed.version,
        expires_at: expirations,
        metadata_versions: versions,
        packages,
        refreshed_at: Utc::now().to_rfc3339(),
    };
    Ok((trust, cache, metadata))
}

fn verify_root_role<T: Serialize>(
    root: &RootMetadata,
    role: &str,
    envelope: &SignedEnvelope<T>,
) -> DraftResult<()> {
    let spec = root
        .roles
        .get(role)
        .ok_or_else(|| invalid(format!("root metadata does not define role '{role}'")))?;
    verify_role(envelope, spec, &root.keys, &root.revoked_key_ids)
}

fn verify_role<T: Serialize>(
    envelope: &SignedEnvelope<T>,
    role: &RoleSpec,
    keys: &BTreeMap<String, CatalogKey>,
    revoked: &[String],
) -> DraftResult<()> {
    validate_role_spec(role)?;
    let authorized: BTreeSet<_> = role.key_ids.iter().cloned().collect();
    let revoked: BTreeSet<_> = revoked.iter().cloned().collect();
    let message = canonical_json(
        &serde_json::to_value(&envelope.signed)
            .map_err(|error| DraftError::storage(format!("serialize signed metadata: {error}")))?,
    );
    let mut valid = BTreeSet::new();
    for signature in &envelope.signatures {
        if valid.contains(&signature.key_id)
            || !authorized.contains(&signature.key_id)
            || revoked.contains(&signature.key_id)
        {
            continue;
        }
        let Some(key) = keys.get(&signature.key_id) else {
            continue;
        };
        if key.algorithm != "ed25519" {
            continue;
        }
        if verify_b64(&key.public_key, message.as_bytes(), &signature.signature)? {
            valid.insert(signature.key_id.clone());
        }
    }
    if valid.len() < role.threshold as usize {
        return Err(DraftError::new(
            DraftErrorKind::ReviewRequired,
            format!(
                "signed metadata has {} unique authorized signatures but threshold is {}",
                valid.len(),
                role.threshold
            ),
        ));
    }
    Ok(())
}

fn validate_root_shape(root: &RootMetadata) -> DraftResult<()> {
    validate_role_header(
        draft_core::contracts::ContractId::CatalogRoot,
        &root.schema_version,
        &root.catalog_id,
        &root.role,
        &root.catalog_id,
        "root",
    )?;
    if root.version == 0 || root.keys.is_empty() {
        return Err(invalid(
            "root metadata has an invalid version or empty key set",
        ));
    }
    for role in ["root", "timestamp", "snapshot", "targets"] {
        let spec = root
            .roles
            .get(role)
            .ok_or_else(|| invalid(format!("root metadata is missing role '{role}'")))?;
        validate_role_spec(spec)?;
        if spec.key_ids.iter().any(|id| !root.keys.contains_key(id)) {
            return Err(invalid(format!("role '{role}' references an unknown key")));
        }
    }
    for package_id in &root.revoked_packages {
        validate_source_id(package_id)?;
    }
    Ok(())
}

fn validate_role_spec(role: &RoleSpec) -> DraftResult<()> {
    let unique: BTreeSet<_> = role.key_ids.iter().collect();
    if role.threshold == 0
        || role.threshold as usize > unique.len()
        || unique.len() != role.key_ids.len()
    {
        return Err(invalid("role key ids/threshold are invalid"));
    }
    Ok(())
}

fn validate_role_header(
    contract: draft_core::contracts::ContractId,
    schema: &u32,
    catalog_id: &str,
    actual_role: &str,
    expected_catalog: &str,
    expected_role: &str,
) -> DraftResult<()> {
    if !draft_core::contracts::supports_version(contract, *schema)
        || catalog_id != expected_catalog
        || actual_role != expected_role
        || catalog_id.trim().is_empty()
    {
        return Err(invalid(format!(
            "invalid {expected_role} metadata schema, catalog identity, or role"
        )));
    }
    Ok(())
}

fn validate_targets(targets: &[CatalogTarget]) -> DraftResult<()> {
    for target in targets {
        validate_source_id(&target.id)?;
        if target.version.trim().is_empty()
            || target.publisher.trim().is_empty()
            || target.draft_api.trim().is_empty()
            || target.length == 0
            || target.length > MAX_ARTIFACT_BYTES as u64
            || !valid_digest(&target.sha256)
        {
            return Err(invalid(
                "catalog target identity, compatibility, hash, or length is invalid",
            ));
        }
        let normalized = draft_core::support::pathguard::check_relative(&target.artifact_path)
            .map_err(|error| invalid(format!("unsafe target artifact path: {error}")))?;
        if normalized != target.artifact_path.replace('\\', "/") || !normalized.ends_with(".tar") {
            return Err(invalid(
                "catalog artifacts must be normalized relative .tar paths",
            ));
        }
    }
    Ok(())
}

fn reject_target_mix_and_match(targets: &[CatalogTarget]) -> DraftResult<()> {
    let mut seen = BTreeMap::<(String, String), (&str, u64)>::new();
    for target in targets {
        let key = (target.id.clone(), target.version.clone());
        if let Some((digest, length)) = seen.insert(key, (&target.sha256, target.length)) {
            if digest != target.sha256 || length != target.length {
                return Err(DraftError::new(
                    DraftErrorKind::ConflictDetected,
                    "catalog published duplicate package/version with different content",
                ));
            }
        }
    }
    Ok(())
}

fn validate_delegation(delegation: &Delegation) -> DraftResult<()> {
    if delegation.role.is_empty()
        || matches!(
            delegation.role.as_str(),
            "root" | "timestamp" | "snapshot" | "targets"
        )
        || delegation.path_prefixes.is_empty()
    {
        return Err(invalid("delegated role name or namespace is invalid"));
    }
    validate_role_spec(&RoleSpec {
        key_ids: delegation.key_ids.clone(),
        threshold: delegation.threshold,
    })?;
    if delegation
        .key_ids
        .iter()
        .any(|id| !delegation.keys.contains_key(id))
        || delegation
            .path_prefixes
            .iter()
            .any(|prefix| prefix.is_empty() || prefix.contains('/') || prefix.contains(".."))
    {
        return Err(invalid("delegation keys or namespace prefixes are invalid"));
    }
    Ok(())
}

fn verify_descriptor_bytes(
    bytes: &[u8],
    descriptor: &MetadataDescriptor,
    label: &str,
) -> DraftResult<()> {
    if bytes.len() as u64 != descriptor.length || digest(bytes) != descriptor.sha256 {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            format!("{label} length or digest does not match trusted metadata"),
        ));
    }
    Ok(())
}

fn check_and_raise_floor(
    trust: &mut CatalogTrust,
    role: &str,
    version: u64,
    digest: &str,
) -> DraftResult<()> {
    if version == 0 || !valid_digest(digest) {
        return Err(invalid("metadata version floor input is invalid"));
    }
    if let Some(floor) = trust.floors.get(role) {
        if version < floor.version {
            return Err(rollback(role, version, floor.version));
        }
        if version == floor.version && digest != floor.sha256 {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!("{role} metadata reused version {version} with different content"),
            ));
        }
    }
    trust.floors.insert(
        role.into(),
        RoleFloor {
            version,
            sha256: digest.into(),
        },
    );
    Ok(())
}

fn require_unexpired(role: &str, expiry: &str) -> DraftResult<()> {
    let expiry = parse_time(expiry)?;
    if expiry <= Utc::now() {
        return Err(DraftError::new(
            DraftErrorKind::EvidenceStale,
            format!("{role} metadata expired at {}", expiry.to_rfc3339()),
        ));
    }
    Ok(())
}

fn parse_time(value: &str) -> DraftResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| invalid("metadata expiry must be RFC 3339"))
}

fn choose_target(
    targets: &[CatalogTarget],
    id: &str,
    version: Option<&str>,
) -> DraftResult<CatalogTarget> {
    targets
        .iter()
        .filter(|target| target.id == id && version.is_none_or(|version| target.version == version))
        .max_by(|left, right| compare_versions(&left.version, &right.version))
        .cloned()
        .ok_or_else(|| {
            DraftError::not_found(format!(
                "extension '{id}'{} is not present in the trusted catalog",
                version.map_or(String::new(), |version| format!(" version {version}"))
            ))
        })
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let parse = |value: &str| {
        value
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    parse(left).cmp(&parse(right)).then_with(|| left.cmp(right))
}

fn cache_usability(cache: &CachedCatalog) -> CatalogUsability {
    if !draft_core::contracts::supports_version(
        draft_core::contracts::ContractId::CachedCatalog,
        cache.schema_version,
    ) || cache.catalog_id.is_empty()
        || cache.expires_at.is_empty()
    {
        return CatalogUsability::Invalid;
    }
    if cache.expires_at.values().any(|expiry| {
        parse_time(expiry).is_err() || parse_time(expiry).is_ok_and(|time| time <= Utc::now())
    }) {
        CatalogUsability::Expired
    } else {
        CatalogUsability::Usable
    }
}

fn status_for(home: &DraftGlobalStore, source: CatalogSource) -> DraftResult<CatalogSourceStatus> {
    let trust = load_trust(home, &source.id)?;
    let cache_path = catalog_cache_root(home, &source.id).join("catalog.json");
    let cache = if cache_path.exists() {
        Some(load_cache(home, &source.id)?)
    } else {
        None
    };
    let usability = if trust.is_none() {
        CatalogUsability::Untrusted
    } else if let Some(cache) = &cache {
        cache_usability(cache)
    } else if source.last_error.is_some() {
        CatalogUsability::Invalid
    } else {
        CatalogUsability::Unavailable
    };
    Ok(CatalogSourceStatus {
        configured: true,
        trusted: trust.is_some(),
        root_version: trust
            .as_ref()
            .map(|trust| trust.trusted_root.signed.version),
        cached_package_count: cache.as_ref().map_or(0, |cache| cache.packages.len()),
        diagnostic: source.last_error.clone(),
        source,
        usability,
    })
}

fn parse_location(location: &str) -> DraftResult<CatalogLocation> {
    if location.starts_with("https://") {
        let mut url = reqwest::Url::parse(location)
            .map_err(|error| invalid(format!("invalid HTTPS catalog URL: {error}")))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid(
                "catalog URL must be an authenticated HTTPS origin/path",
            ));
        }
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        Ok(CatalogLocation::Https {
            url: url.to_string(),
        })
    } else {
        let path = Path::new(location)
            .canonicalize()
            .map_err(|error| DraftError::not_found(format!("cannot open catalog: {error}")))?;
        if !path.is_dir() {
            return Err(invalid("local catalog source must be a directory"));
        }
        Ok(CatalogLocation::LocalDirectory {
            path: path.to_string_lossy().into_owned(),
        })
    }
}

fn fetch_source_file(source: &CatalogSource, relative: &str, limit: usize) -> DraftResult<Vec<u8>> {
    let normalized = draft_core::support::pathguard::check_relative(relative)
        .map_err(|error| invalid(format!("unsafe catalog path: {error}")))?;
    if normalized != relative.replace('\\', "/") {
        return Err(invalid("catalog path is not normalized"));
    }
    match &source.location {
        CatalogLocation::LocalDirectory { path } => {
            let root = Path::new(path);
            let target = draft_core::support::pathguard::safe_join(root, &normalized)
                .map_err(|error| invalid(format!("unsafe catalog path: {error}")))?;
            read_bounded(&target, limit)
        }
        CatalogLocation::Https { url } => {
            let base = reqwest::Url::parse(url)
                .map_err(|error| invalid(format!("invalid catalog URL: {error}")))?;
            let target = base
                .join(&normalized)
                .map_err(|error| invalid(format!("invalid catalog target URL: {error}")))?;
            if target.scheme() != "https" || target.host_str() != base.host_str() {
                return Err(DraftError::new(
                    DraftErrorKind::ProtectedFileAccess,
                    "catalog target escaped the configured HTTPS host",
                ));
            }
            let expected_host = base.host_str().unwrap_or_default().to_string();
            let client = Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(20))
                .redirect(Policy::custom(move |attempt| {
                    if attempt.previous().len() < 3
                        && attempt.url().scheme() == "https"
                        && attempt.url().host_str() == Some(expected_host.as_str())
                    {
                        attempt.follow()
                    } else {
                        attempt.stop()
                    }
                }))
                .build()
                .map_err(|error| DraftError::storage(format!("build HTTPS client: {error}")))?;
            let response = client
                .get(target)
                .send()
                .and_then(|response| response.error_for_status())
                .map_err(|error| {
                    DraftError::new(
                        DraftErrorKind::ServiceUnavailable,
                        format!("catalog HTTPS request failed: {error}"),
                    )
                })?;
            if response
                .content_length()
                .is_some_and(|length| length > limit as u64)
            {
                return Err(invalid("catalog response exceeds its size limit"));
            }
            let mut bytes = Vec::new();
            response
                .take(limit as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| DraftError::storage(format!("read catalog response: {error}")))?;
            if bytes.len() > limit {
                return Err(invalid("catalog response exceeds its size limit"));
            }
            Ok(bytes)
        }
    }
}

fn fetch_or_cached_artifact(
    home: &DraftGlobalStore,
    source: &CatalogSource,
    target: &CatalogTarget,
) -> DraftResult<Vec<u8>> {
    let cache = artifact_cache_path(home, &source.id, &target.sha256);
    if cache.exists() {
        let bytes = read_bounded(&cache, MAX_ARTIFACT_BYTES)?;
        if bytes.len() as u64 == target.length && digest(&bytes) == target.sha256 {
            return Ok(bytes);
        }
    }
    let bytes = fetch_source_file(source, &target.artifact_path, MAX_ARTIFACT_BYTES)?;
    if bytes.len() as u64 != target.length || digest(&bytes) != target.sha256 {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "downloaded extension artifact failed integrity verification",
        ));
    }
    if let Some(parent) = cache.parent() {
        ensure_dir(parent)?;
    }
    write_atomic(&cache, &bytes)?;
    Ok(bytes)
}

fn extract_archive(bytes: &[u8], destination: &Path) -> DraftResult<()> {
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    for entry in archive
        .entries()
        .map_err(|error| invalid(format!("invalid extension archive: {error}")))?
    {
        let mut entry =
            entry.map_err(|error| invalid(format!("invalid archive entry: {error}")))?;
        let entry_type = entry.header().entry_type();
        let path = entry
            .path()
            .map_err(|error| invalid(format!("invalid archive path: {error}")))?
            .to_string_lossy()
            .replace('\\', "/");
        let normalized = draft_core::support::pathguard::check_relative(&path)
            .map_err(|error| invalid(format!("unsafe extension archive path: {error}")))?;
        let target = draft_core::support::pathguard::safe_join(destination, &normalized)
            .map_err(|error| invalid(format!("unsafe extension extraction path: {error}")))?;
        if entry_type.is_dir() {
            ensure_dir(&target)?;
            continue;
        }
        if !entry_type.is_file() {
            return Err(DraftError::new(
                DraftErrorKind::ProtectedFileAccess,
                "extension archives reject links, devices, and non-regular entries",
            ));
        }
        if entry.header().mode().unwrap_or(0) & 0o111 != 0 {
            return Err(DraftError::new(
                DraftErrorKind::ProtectedFileAccess,
                "extension archives reject executable permissions",
            ));
        }
        let size = entry.header().size().unwrap_or(u64::MAX);
        if size > 8 * 1024 * 1024 {
            return Err(invalid("extension archive entry exceeds its size limit"));
        }
        if let Some(parent) = target.parent() {
            ensure_dir(parent)?;
        }
        let mut content = Vec::with_capacity(size as usize);
        entry
            .read_to_end(&mut content)
            .map_err(|error| invalid(format!("read extension archive entry: {error}")))?;
        write_atomic(&target, &content)?;
    }
    Ok(())
}

fn commit_cache(
    home: &DraftGlobalStore,
    source_id: &str,
    cache: &CachedCatalog,
    metadata: &BTreeMap<String, Vec<u8>>,
) -> DraftResult<()> {
    let root = catalog_cache_root(home, source_id);
    let parent = root
        .parent()
        .ok_or_else(|| DraftError::storage("catalog cache path has no parent"))?;
    ensure_dir(parent)?;
    let staging = parent.join(format!(
        ".refresh-{source_id}-{}",
        draft_core::support::common::OperationId::generate()
    ));
    ensure_dir(&staging)?;
    let result = (|| -> DraftResult<()> {
        write_json(&staging.join("catalog.json"), cache)?;
        ensure_dir(&staging.join("metadata"))?;
        for (name, bytes) in metadata {
            write_atomic(&staging.join("metadata").join(name), bytes)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let prior = parent.join(format!(".prior-{source_id}"));
    if prior.exists() {
        fs::remove_dir_all(&prior)?;
    }
    if root.exists() {
        fs::rename(&root, &prior)?;
    }
    if let Err(error) = fs::rename(&staging, &root) {
        if prior.exists() {
            let _ = fs::rename(&prior, &root);
        }
        return Err(DraftError::storage(format!(
            "promote extension catalog cache: {error}"
        )));
    }
    if prior.exists() {
        fs::remove_dir_all(prior)?;
    }
    Ok(())
}

fn load_sources(home: &DraftGlobalStore) -> DraftResult<SourceRegistry> {
    let path = sources_path(home);
    let registry = if path.exists() {
        draft_core::contracts::read_persisted(&path)?
    } else {
        SourceRegistry::default()
    };
    for source in registry.sources.values() {
        if !draft_core::contracts::supports_version(
            draft_core::contracts::ContractId::CatalogSource,
            source.schema_version,
        ) {
            return Err(DraftError::new(
                DraftErrorKind::UnsupportedSchema,
                format!(
                    "extension source '{}' schema {} is unsupported",
                    source.id, source.schema_version
                ),
            ));
        }
    }
    Ok(registry)
}

fn persist_sources(home: &DraftGlobalStore, registry: &SourceRegistry) -> DraftResult<()> {
    ensure_dir(&home.extensions_dir())?;
    write_json(&sources_path(home), registry)
}

fn load_trust(home: &DraftGlobalStore, id: &str) -> DraftResult<Option<CatalogTrust>> {
    let path = trust_path(home, id);
    if path.exists() {
        draft_core::contracts::read_persisted(&path).map(Some)
    } else {
        Ok(None)
    }
}

fn persist_trust(home: &DraftGlobalStore, id: &str, trust: &CatalogTrust) -> DraftResult<()> {
    let path = trust_path(home, id);
    ensure_dir(path.parent().unwrap_or(&home.trust_dir()))?;
    write_json(&path, trust)
}

fn load_cache(home: &DraftGlobalStore, id: &str) -> DraftResult<CachedCatalog> {
    draft_core::contracts::read_persisted(&catalog_cache_root(home, id).join("catalog.json"))
}

fn sources_path(home: &DraftGlobalStore) -> PathBuf {
    home.extensions_dir().join("sources.json")
}

fn trust_path(home: &DraftGlobalStore, id: &str) -> PathBuf {
    home.trust_dir()
        .join("extension-catalogs")
        .join(format!("{id}.json"))
}

fn catalog_cache_root(home: &DraftGlobalStore, id: &str) -> PathBuf {
    home.cache_dir().join("extension-catalogs").join(id)
}

fn artifact_cache_path(home: &DraftGlobalStore, id: &str, digest: &str) -> PathBuf {
    home.cache_dir()
        .join("extension-artifacts")
        .join(id)
        .join(digest.trim_start_matches("sha256:"))
        .with_extension("tar")
}

fn read_bounded(path: &Path, limit: usize) -> DraftResult<Vec<u8>> {
    let metadata = fs::metadata(path).map_err(|error| {
        DraftError::not_found(format!("cannot read {}: {error}", path.display()))
    })?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(invalid(format!(
            "{} is not a bounded regular file",
            path.display()
        )));
    }
    fs::read(path).map_err(Into::into)
}

fn parse_json<T: DeserializeOwned>(bytes: &[u8], label: &str) -> DraftResult<T> {
    serde_json::from_slice(bytes).map_err(|error| invalid(format!("invalid {label}: {error}")))
}

fn digest(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
}

fn validate_source_id(id: &str) -> DraftResult<()> {
    if id.is_empty()
        || id.len() > 64
        || !id.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '.' | '_')
        })
    {
        return Err(invalid("extension/catalog id is invalid"));
    }
    Ok(())
}

pub(crate) fn package_is_revoked(installed: &InstalledExtension) -> DraftResult<bool> {
    let Some(source_id) = installed.provenance.catalog_source_id() else {
        return Ok(false);
    };
    let home = DraftGlobalStore::locate()?;
    let Some(trust) = load_trust(&home, source_id)? else {
        return Ok(false);
    };
    if installed.provenance.catalog_id() != Some(trust.catalog_id.as_str()) {
        return Ok(false);
    }
    Ok(trust
        .trusted_root
        .signed
        .revoked_packages
        .iter()
        .any(|id| id == &installed.manifest.id))
}

fn rollback(role: &str, actual: u64, floor: u64) -> DraftError {
    DraftError::new(
        DraftErrorKind::ConflictDetected,
        format!("{role} metadata version {actual} is below durable floor {floor}"),
    )
}

fn invalid(message: impl Into<String>) -> DraftError {
    DraftError::invalid_config(message)
}

fn audit_catalog(event: &str, source_id: &str, payload: serde_json::Value) -> DraftResult<()> {
    let home = DraftGlobalStore::locate()?;
    let actor_id =
        draft_core::trust::identity::global::load_actor(&home)?.map(|actor| actor.actor_id);
    draft_core::trust::audit::GlobalAuditLog::global()?.append(
        event,
        actor_id,
        Some(source_id.into()),
        Some(draft_core::support::common::OperationId::generate().to_string()),
        payload,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_core::trust::signing::Keypair;

    fn root_with(keys: &[(&str, &Keypair)], threshold: u32) -> RootMetadata {
        let key_map = keys
            .iter()
            .map(|(id, key)| {
                (
                    (*id).to_string(),
                    CatalogKey {
                        algorithm: "ed25519".into(),
                        public_key: key.public_key_b64(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let role = RoleSpec {
            key_ids: keys.iter().map(|(id, _)| (*id).to_string()).collect(),
            threshold,
        };
        RootMetadata {
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::CatalogRoot,
            ),
            catalog_id: "catalog.example".into(),
            role: "root".into(),
            version: 1,
            expires_at: "2099-01-01T00:00:00Z".into(),
            keys: key_map,
            roles: BTreeMap::from([
                ("root".into(), role.clone()),
                ("timestamp".into(), role.clone()),
                ("snapshot".into(), role.clone()),
                ("targets".into(), role),
            ]),
            revoked_key_ids: vec![],
            revoked_packages: vec![],
        }
    }

    fn sign<T: Serialize + Clone>(signed: T, keys: &[(&str, &Keypair)]) -> SignedEnvelope<T> {
        let message = canonical_json(&serde_json::to_value(&signed).unwrap());
        SignedEnvelope {
            signed,
            signatures: keys
                .iter()
                .map(|(id, key)| SignatureRecord {
                    key_id: (*id).into(),
                    signature: key.sign_b64(message.as_bytes()),
                })
                .collect(),
        }
    }

    #[test]
    fn threshold_counts_only_unique_authorized_signatures() {
        let first = Keypair::generate();
        let second = Keypair::generate();
        let root = root_with(&[("first", &first), ("second", &second)], 2);
        let mut envelope = sign(root.clone(), &[("first", &first)]);
        envelope.signatures.push(envelope.signatures[0].clone());
        let error =
            verify_role(&envelope, root.roles.get("root").unwrap(), &root.keys, &[]).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ReviewRequired);
        let envelope = sign(root.clone(), &[("first", &first), ("second", &second)]);
        verify_role(&envelope, root.roles.get("root").unwrap(), &root.keys, &[]).unwrap();
    }

    #[test]
    fn revoked_keys_do_not_count_toward_threshold() {
        let first = Keypair::generate();
        let root = root_with(&[("first", &first)], 1);
        let envelope = sign(root.clone(), &[("first", &first)]);
        assert!(verify_role(
            &envelope,
            root.roles.get("root").unwrap(),
            &root.keys,
            &["first".into()],
        )
        .is_err());
    }

    #[test]
    fn metadata_floors_reject_rollback_and_same_version_replay() {
        let key = Keypair::generate();
        let root = root_with(&[("root", &key)], 1);
        let envelope = sign(root, &[("root", &key)]);
        let mut trust = CatalogTrust {
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::CatalogTrust,
            ),
            catalog_id: "catalog.example".into(),
            trusted_root: envelope,
            trusted_root_digest: format!("sha256:{}", "0".repeat(64)),
            trusted_at: Utc::now().to_rfc3339(),
            reset_count: 0,
            floors: BTreeMap::new(),
        };
        let one = format!("sha256:{}", "1".repeat(64));
        let two = format!("sha256:{}", "2".repeat(64));
        check_and_raise_floor(&mut trust, "targets", 3, &one).unwrap();
        assert!(check_and_raise_floor(&mut trust, "targets", 2, &one).is_err());
        assert!(check_and_raise_floor(&mut trust, "targets", 3, &two).is_err());
    }

    #[test]
    fn delegation_cannot_escape_parent_namespace() {
        let delegation = Delegation {
            role: "example".into(),
            keys: BTreeMap::new(),
            key_ids: vec!["key".into()],
            threshold: 1,
            path_prefixes: vec!["example.".into()],
        };
        assert!(validate_delegation(&delegation).is_err());
        let target = CatalogTarget {
            id: "other.package".into(),
            version: "1.0.0".into(),
            publisher: "other".into(),
            draft_api: format!("^{}", draft_core::DRAFT_API_VERSION),
            artifact_path: "artifacts/other.tar".into(),
            length: 1,
            sha256: format!("sha256:{}", "0".repeat(64)),
        };
        assert!(!delegation
            .path_prefixes
            .iter()
            .any(|prefix| target.id.starts_with(prefix)));
    }

    #[test]
    fn expired_cache_remains_discoverable_but_is_not_usable() {
        let cache = CachedCatalog {
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::CachedCatalog,
            ),
            catalog_id: "catalog.example".into(),
            root_version: 1,
            expires_at: BTreeMap::from([("targets".into(), "2000-01-01T00:00:00Z".into())]),
            metadata_versions: BTreeMap::new(),
            packages: vec![],
            refreshed_at: Utc::now().to_rfc3339(),
        };
        assert_eq!(cache_usability(&cache), CatalogUsability::Expired);
    }

    #[test]
    fn archive_extraction_rejects_symlinks() {
        let mut bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut bytes);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "entry", Cursor::new(Vec::<u8>::new()))
                .unwrap();
            builder.finish().unwrap();
        }
        let temp = tempfile::tempdir().unwrap();
        let error = extract_archive(&bytes, temp.path()).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ProtectedFileAccess);
    }

    #[test]
    fn root_rotation_requires_old_and_new_thresholds() {
        let old_key = Keypair::generate();
        let new_key = Keypair::generate();
        let old_root = root_with(&[("old", &old_key)], 1);
        let mut new_root = root_with(&[("new", &new_key)], 1);
        new_root.version = 2;
        let signed_only_by_new = sign(new_root.clone(), &[("new", &new_key)]);
        verify_role(
            &signed_only_by_new,
            new_root.roles.get("root").unwrap(),
            &new_root.keys,
            &[],
        )
        .unwrap();
        assert!(verify_role(
            &signed_only_by_new,
            old_root.roles.get("root").unwrap(),
            &old_root.keys,
            &[],
        )
        .is_err());
        let message = canonical_json(&serde_json::to_value(&new_root).unwrap());
        let dual = SignedEnvelope {
            signed: new_root.clone(),
            signatures: vec![
                SignatureRecord {
                    key_id: "old".into(),
                    signature: old_key.sign_b64(message.as_bytes()),
                },
                SignatureRecord {
                    key_id: "new".into(),
                    signature: new_key.sign_b64(message.as_bytes()),
                },
            ],
        };
        verify_role(
            &dual,
            old_root.roles.get("root").unwrap(),
            &old_root.keys,
            &[],
        )
        .unwrap();
        verify_role(
            &dual,
            new_root.roles.get("root").unwrap(),
            &new_root.keys,
            &[],
        )
        .unwrap();
    }

    struct EnvRestore(Option<std::ffi::OsString>);

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            if let Some(value) = &self.0 {
                std::env::set_var("DRAFT_GLOBAL_HOME", value);
            } else {
                std::env::remove_var("DRAFT_GLOBAL_HOME");
            }
        }
    }

    fn write_package_archive(catalog: &Path, version: &str) -> Vec<u8> {
        let package = tempfile::tempdir().unwrap();
        ensure_dir(&package.path().join("contributions")).unwrap();
        ensure_dir(&package.path().join("docs")).unwrap();
        let manifest = extension::ExtensionManifest {
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::ExtensionManifest,
            ),
            id: "example.docs".into(),
            name: "Example docs".into(),
            version: version.into(),
            publisher: "example".into(),
            draft_api: format!("^{}", draft_core::DRAFT_API_VERSION),
            contributions: vec![extension::ExtensionContribution {
                id: "task-review".into(),
                kind: extension::ExtensionContributionKind::TaskTemplate,
                path: "contributions/task.json".into(),
            }],
            documentation: vec!["docs/readme.md".into()],
            licenses: vec!["LICENSE.txt".into()],
            assets: vec![],
        };
        write_json(&package.path().join("extension.json"), &manifest).unwrap();
        fs::write(package.path().join("contributions/task.json"), "{}").unwrap();
        fs::write(package.path().join("docs/readme.md"), "docs").unwrap();
        fs::write(package.path().join("LICENSE.txt"), "license").unwrap();
        let mut bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut bytes);
            for relative in [
                "extension.json",
                "contributions/task.json",
                "docs/readme.md",
                "LICENSE.txt",
            ] {
                builder
                    .append_path_with_name(package.path().join(relative), relative)
                    .unwrap();
            }
            builder.finish().unwrap();
        }
        ensure_dir(&catalog.join("artifacts")).unwrap();
        fs::write(catalog.join("artifacts/example.docs.tar"), &bytes).unwrap();
        bytes
    }

    fn write_catalog_generation(
        catalog: &Path,
        key: &Keypair,
        root: &SignedEnvelope<RootMetadata>,
        metadata_version: u64,
        package_version: &str,
    ) -> Vec<u8> {
        let root_bytes = serde_json::to_vec(root).unwrap();
        fs::write(catalog.join("root.json"), &root_bytes).unwrap();
        let artifact = write_package_archive(catalog, package_version);
        let target = CatalogTarget {
            id: "example.docs".into(),
            version: package_version.into(),
            publisher: "example".into(),
            draft_api: format!("^{}", draft_core::DRAFT_API_VERSION),
            artifact_path: "artifacts/example.docs.tar".into(),
            length: artifact.len() as u64,
            sha256: digest(&artifact),
        };
        let targets = sign(
            TargetsMetadata {
                schema_version: draft_core::contracts::current_version(
                    draft_core::contracts::ContractId::CatalogTargets,
                ),
                catalog_id: "catalog.example".into(),
                role: "targets".into(),
                version: metadata_version,
                expires_at: "2099-01-01T00:00:00Z".into(),
                packages: vec![target],
                delegations: vec![],
            },
            &[("root", key)],
        );
        let targets_bytes = serde_json::to_vec(&targets).unwrap();
        fs::write(catalog.join("targets.json"), &targets_bytes).unwrap();
        let snapshot = sign(
            SnapshotMetadata {
                schema_version: draft_core::contracts::current_version(
                    draft_core::contracts::ContractId::CatalogSnapshot,
                ),
                catalog_id: "catalog.example".into(),
                role: "snapshot".into(),
                version: metadata_version,
                expires_at: "2099-01-01T00:00:00Z".into(),
                roles: BTreeMap::from([(
                    "targets".into(),
                    MetadataDescriptor {
                        version: metadata_version,
                        length: targets_bytes.len() as u64,
                        sha256: digest(&targets_bytes),
                    },
                )]),
            },
            &[("root", key)],
        );
        let snapshot_bytes = serde_json::to_vec(&snapshot).unwrap();
        fs::write(catalog.join("snapshot.json"), &snapshot_bytes).unwrap();
        let timestamp = sign(
            TimestampMetadata {
                schema_version: draft_core::contracts::current_version(
                    draft_core::contracts::ContractId::CatalogTimestamp,
                ),
                catalog_id: "catalog.example".into(),
                role: "timestamp".into(),
                version: metadata_version,
                expires_at: "2099-01-01T00:00:00Z".into(),
                snapshot: MetadataDescriptor {
                    version: metadata_version,
                    length: snapshot_bytes.len() as u64,
                    sha256: digest(&snapshot_bytes),
                },
            },
            &[("root", key)],
        );
        fs::write(
            catalog.join("timestamp.json"),
            serde_json::to_vec(&timestamp).unwrap(),
        )
        .unwrap();
        root_bytes
    }

    #[test]
    fn local_catalog_lifecycle_preserves_provenance_and_rejects_tampering() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join("global");
        let catalog = temp.path().join("catalog");
        ensure_dir(&catalog).unwrap();
        let _restore = EnvRestore(std::env::var_os("DRAFT_GLOBAL_HOME"));
        std::env::set_var("DRAFT_GLOBAL_HOME", &global);

        let key = Keypair::generate();
        let root = sign(root_with(&[("root", &key)], 1), &[("root", &key)]);
        let root_bytes = write_catalog_generation(&catalog, &key, &root, 1, "1.0.0");
        source_add("local", catalog.to_str().unwrap()).unwrap();
        let untrusted = source_list().unwrap();
        assert_eq!(untrusted[0].usability, CatalogUsability::Untrusted);
        trust_source_bytes("local", &root_bytes, &digest(&root_bytes), false).unwrap();
        source_refresh("local").unwrap();
        assert_eq!(discover(None).unwrap().len(), 1);

        let artifact_path = catalog.join("artifacts/example.docs.tar");
        let original = fs::read(&artifact_path).unwrap();
        fs::write(&artifact_path, b"tampered").unwrap();
        assert!(install_from_source("local", "example.docs", None).is_err());
        fs::write(&artifact_path, original).unwrap();
        let installed = install_from_source("local", "example.docs", None).unwrap();
        assert_eq!(installed.manifest.version, "1.0.0");
        assert_eq!(installed.provenance.catalog_id(), Some("catalog.example"));
        extension::set_enabled("example.docs", false).unwrap();
        assert!(extension::active_contributions().unwrap().is_empty());

        write_catalog_generation(&catalog, &key, &root, 2, "2.0.0");
        source_refresh("local").unwrap();
        let updated = update_from_source("local", "example.docs", None).unwrap();
        assert_eq!(updated.manifest.version, "2.0.0");
        assert!(
            !updated.enabled,
            "updates preserve explicit enablement state"
        );
        extension::set_enabled("example.docs", true).unwrap();
        assert_eq!(extension::active_contributions().unwrap().len(), 1);
        let mut revoked_root = root.signed.clone();
        revoked_root.version = 2;
        revoked_root.revoked_packages = vec!["example.docs".into()];
        let revoked_root = sign(revoked_root, &[("root", &key)]);
        write_catalog_generation(&catalog, &key, &revoked_root, 3, "2.0.0");
        source_refresh("local").unwrap();
        assert!(extension::active_contributions().unwrap().is_empty());
        assert!(extension::set_enabled("example.docs", true).is_err());
        source_remove("local").unwrap();
        assert_eq!(
            extension::show("example.docs").unwrap().manifest.version,
            "2.0.0"
        );
        assert_eq!(discover(None).unwrap().len(), 0);
    }
}
