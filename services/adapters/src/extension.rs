//! Protocol-neutral Draft extension manifests, registry, and installation.

use draft_core::error::{DraftError, DraftErrorKind, DraftResult};
use draft_core::fsutil::{ensure_dir, read_json, write_json};
use draft_core::hashing::sha256_hex;
use draft_core::home::GlobalHome;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub schema_version: String,
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub draft_api: String,
    pub entrypoint: ExtensionEntrypoint,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtensionEntrypoint {
    Executable { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledExtension {
    pub manifest: ExtensionManifest,
    pub content_hash: String,
    pub enabled: bool,
    pub installed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Registry {
    schema_version: String,
    extensions: BTreeMap<String, InstalledExtension>,
}

fn root(home: &GlobalHome) -> PathBuf {
    home.root().join("extensions")
}
fn packages(home: &GlobalHome) -> PathBuf {
    root(home).join("packages")
}
fn registry_path(home: &GlobalHome) -> PathBuf {
    root(home).join("registry.json")
}

fn load(home: &GlobalHome) -> DraftResult<Registry> {
    let registry = if registry_path(home).exists() {
        read_json(&registry_path(home))?
    } else {
        Registry {
            schema_version: draft_core::DRAFT_SCHEMA_VERSION.into(),
            ..Default::default()
        }
    };
    Ok(registry)
}

fn persist(home: &GlobalHome, r: Registry) -> DraftResult<()> {
    ensure_dir(&root(home))?;
    write_json(&registry_path(home), &r)
}

pub fn list() -> DraftResult<Vec<InstalledExtension>> {
    let home = GlobalHome::locate()?;
    let mut out: Vec<_> = load(&home)?.extensions.into_values().collect();
    out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    Ok(out)
}

pub fn show(id: &str) -> DraftResult<InstalledExtension> {
    let home = GlobalHome::locate()?;
    load(&home)?
        .extensions
        .remove(id)
        .ok_or_else(|| DraftError::not_found(format!("extension '{id}' is not installed")))
}

pub fn set_enabled(id: &str, enabled: bool) -> DraftResult<InstalledExtension> {
    let home = GlobalHome::locate()?;
    let mut r = load(&home)?;
    let ext = r
        .extensions
        .get_mut(id)
        .ok_or_else(|| DraftError::not_found(format!("extension '{id}' is not installed")))?;
    ext.enabled = enabled;
    let result = ext.clone();
    persist(&home, r)?;
    Ok(result)
}

pub fn install(source: &Path) -> DraftResult<InstalledExtension> {
    let source = source
        .canonicalize()
        .map_err(|e| DraftError::not_found(format!("cannot open extension package: {e}")))?;
    if !source.is_dir() {
        return Err(DraftError::invalid_config(
            "extension source must be a directory",
        ));
    }
    let manifest: ExtensionManifest = read_json(&source.join("extension.json"))?;
    validate_manifest(&manifest)?;
    let home = GlobalHome::locate()?;
    let mut r = load(&home)?;
    if r.extensions.contains_key(&manifest.id) {
        return Err(DraftError::new(
            DraftErrorKind::TaskDefinitionConflict,
            format!("extension '{}' is already installed", manifest.id),
        ));
    }
    let bytes = package_bytes(&source)?;
    let hash = sha256_hex(&bytes);
    let destination = packages(&home).join(&manifest.id);
    ensure_dir(&packages(&home))?;
    let staging = packages(&home).join(format!(".install-{}-{}", manifest.id, std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    copy_package(&source, &staging)?;
    fs::rename(&staging, &destination).map_err(|e| {
        let _ = fs::remove_dir_all(&staging);
        DraftError::storage(format!("failed to promote extension package: {e}"))
    })?;
    let installed = InstalledExtension {
        manifest: manifest.clone(),
        content_hash: hash,
        enabled: true,
        installed_at: draft_core::common::now().to_rfc3339(),
    };
    r.extensions.insert(manifest.id.clone(), installed.clone());
    if let Err(e) = persist(&home, r) {
        let _ = fs::remove_dir_all(&destination);
        return Err(e);
    }
    Ok(installed)
}

pub fn uninstall(id: &str) -> DraftResult<InstalledExtension> {
    let home = GlobalHome::locate()?;
    let mut r = load(&home)?;
    let ext = r
        .extensions
        .get(id)
        .cloned()
        .ok_or_else(|| DraftError::not_found(format!("extension '{id}' is not installed")))?;
    r.extensions.remove(id);
    persist(&home, r)?;
    let path = packages(&home).join(id);
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    Ok(ext)
}

fn validate_manifest(m: &ExtensionManifest) -> DraftResult<()> {
    let id_ok = !m.id.is_empty()
        && m.id.len() <= 64
        && m.id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.');
    if !id_ok || m.schema_version != "0.3.4" || m.draft_api != "^0.3.4" {
        return Err(DraftError::invalid_config(
            "extension manifest id/schema/API compatibility is invalid",
        ));
    }
    let ExtensionEntrypoint::Executable { path } = &m.entrypoint;
    let p = Path::new(path);
    if p.is_absolute()
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(DraftError::invalid_config(
            "extension entrypoint must be package-relative",
        ));
    }
    Ok(())
}

fn package_bytes(source: &Path) -> DraftResult<Vec<u8>> {
    let mut files = Vec::new();
    collect_files(source, source, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Vec::new();
    for (rel, path) in files {
        out.extend_from_slice(rel.as_bytes());
        out.push(0);
        out.extend_from_slice(&fs::read(path)?);
        out.push(0);
    }
    Ok(out)
}
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> DraftResult<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let ty = e.file_type()?;
        if ty.is_symlink() {
            return Err(DraftError::new(
                DraftErrorKind::ProtectedFileAccess,
                "extension packages may not contain symlinks",
            ));
        }
        if ty.is_dir() {
            collect_files(root, &e.path(), out)?;
        } else if ty.is_file() {
            out.push((
                e.path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                e.path(),
            ));
        }
    }
    Ok(())
}
fn copy_package(source: &Path, dest: &Path) -> DraftResult<()> {
    if dest.exists() {
        return Err(DraftError::invalid_config(
            "extension destination already exists",
        ));
    }
    ensure_dir(dest)?;
    for e in fs::read_dir(source)? {
        let e = e?;
        let ty = e.file_type()?;
        let target = dest.join(e.file_name());
        if ty.is_symlink() {
            return Err(DraftError::new(
                DraftErrorKind::ProtectedFileAccess,
                "extension packages may not contain symlinks",
            ));
        }
        if ty.is_dir() {
            copy_package(&e.path(), &target)?;
        } else {
            fs::copy(e.path(), target)?;
        }
    }
    Ok(())
}
