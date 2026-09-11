//! Shared, test-only fixtures for building a real signed extension catalog.
//!
//! Included by more than one integration suite through `#[path]` rather than
//! published from a crate: this is test scaffolding, and putting it in a
//! production crate — or making a lower-level service crate depend upward on
//! the daemon — would invert the dependency direction the platform relies on.
//!
//! Everything here drives the real packager over the real `/extensions/`
//! sources. Nothing is a stand-in for the shipping artifacts.
#![allow(dead_code)]

use draft_extension_service::catalog;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub struct GlobalHome {
    _directory: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}

impl Default for GlobalHome {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalHome {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("DRAFT_GLOBAL_HOME");
        std::env::set_var("DRAFT_GLOBAL_HOME", directory.path().join(".draft"));
        Self {
            _directory: directory,
            previous,
        }
    }
}

impl Drop for GlobalHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(previous) => std::env::set_var("DRAFT_GLOBAL_HOME", previous),
            None => std::env::remove_var("DRAFT_GLOBAL_HOME"),
        }
    }
}

pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("services/extension-service sits two levels below the repository root")
        .to_path_buf()
}

pub fn packager(arguments: &[&str]) -> Result<String, String> {
    let root = repository_root();
    let output = std::process::Command::new(env!("CARGO"))
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(root.join("extensions/Cargo.toml"))
        .arg("--bin")
        .arg("draft-extension-packager")
        .arg("--")
        .args(arguments)
        .current_dir(&root)
        .output()
        .map_err(|error| format!("cannot run the packager: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Build and sign the official catalog exactly as a publisher would.
///
/// Returns the catalog directory and its root fingerprint, which is the trust
/// anchor a user would be given out of band.
pub fn build_official_catalog(into: &Path) -> Result<String, String> {
    let root = repository_root();
    let packages = root.join("extensions/packages");
    let catalog = into.to_string_lossy().into_owned();

    packager(&["validate", &packages.to_string_lossy()])?;
    packager(&[
        "build",
        &packages.to_string_lossy(),
        &catalog,
        "--catalog-id",
        "draft-official",
    ])?;

    // An ephemeral key, created here and never persisted beyond this temporary
    // directory. The packager itself never generates key material.
    let key_path = into.join("ephemeral.key");
    let encoded = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode([11u8; 32])
    };
    std::fs::write(&key_path, encoded).map_err(|error| error.to_string())?;

    packager(&[
        "sign",
        &catalog,
        "--key",
        &key_path.to_string_lossy(),
        "--key-id",
        "official-test",
    ])?;
    packager(&["verify", &catalog])?;

    // The trust anchor is the digest of the signed root document.
    let root_bytes = std::fs::read(into.join("root.json")).map_err(|error| error.to_string())?;
    std::fs::remove_file(&key_path).map_err(|error| error.to_string())?;
    Ok(draft_core::extension::package::digest(&root_bytes))
}

/// Configure and trust the built catalog, then refresh it.
pub fn trust_catalog(source_id: &str, catalog: &Path, fingerprint: &str) {
    catalog::source_add(source_id, catalog.to_str().unwrap()).unwrap();
    let root_bytes = std::fs::read(catalog.join("root.json")).unwrap();
    catalog::trust_source_bytes(source_id, &root_bytes, fingerprint, false).unwrap();
    catalog::source_refresh(source_id).unwrap();
}
