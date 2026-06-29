//! Crash-safe filesystem helpers shared by all storage layers.
//!
//! All structured writes go through atomic write-then-rename (DR-002, NFR-005)
//! so a partial write is never observed as a valid record.

use std::fs;
use std::io::Write;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{DraftError, DraftResult};

/// Ensure a directory (and parents) exists.
pub fn ensure_dir(dir: &Path) -> DraftResult<()> {
    fs::create_dir_all(dir).map_err(|e| {
        DraftError::storage(format!("failed to create directory {}: {e}", dir.display()))
    })
}

/// Atomically write bytes to `path` (temp file + fsync + rename).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> DraftResult<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    // Unique temp name to avoid concurrent writers clobbering each other.
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    {
        let mut f = fs::File::create(&tmp).map_err(|e| {
            DraftError::storage(format!("failed to create temp file {}: {e}", tmp.display()))
        })?;
        f.write_all(bytes)
            .map_err(|e| DraftError::storage(format!("failed to write {}: {e}", tmp.display())))?;
        f.flush().ok();
        // Best-effort durability before the rename.
        f.sync_all().ok();
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        DraftError::storage(format!("failed to rename into {}: {e}", path.display()))
    })?;
    Ok(())
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> DraftResult<()> {
    let s = serde_json::to_string_pretty(value)
        .map_err(|e| DraftError::storage(format!("JSON serialize failed: {e}")))?;
    write_atomic(path, s.as_bytes())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> DraftResult<T> {
    let content = fs::read_to_string(path)
        .map_err(|e| DraftError::not_found(format!("cannot read {}: {e}", path.display())))?;
    serde_json::from_str(&content)
        .map_err(|e| DraftError::storage(format!("JSON parse failed for {}: {e}", path.display())))
}

pub fn write_toml<T: Serialize>(path: &Path, value: &T) -> DraftResult<()> {
    let s = toml::to_string_pretty(value)
        .map_err(|e| DraftError::storage(format!("TOML serialize failed: {e}")))?;
    write_atomic(path, s.as_bytes())
}

pub fn read_toml<T: DeserializeOwned>(path: &Path) -> DraftResult<T> {
    let content = fs::read_to_string(path)
        .map_err(|e| DraftError::not_found(format!("cannot read {}: {e}", path.display())))?;
    toml::from_str(&content).map_err(|e| {
        DraftError::invalid_config(format!("TOML parse failed for {}: {e}", path.display()))
    })
}

/// Read a directory's immediate entries with the given extension (no dot),
/// returning full paths. Returns empty if the directory does not exist.
pub fn list_with_extension(dir: &Path, ext: &str) -> DraftResult<Vec<std::path::PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)
        .map_err(|e| DraftError::storage(format!("cannot read dir {}: {e}", dir.display())))?
    {
        let entry = entry.map_err(|e| DraftError::storage(e.to_string()))?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some(ext) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}
