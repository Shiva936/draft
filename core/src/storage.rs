use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Sha256, Digest};
use chrono::Utc;

use crate::errors::DraftError;
use crate::models::ObjectHash;

#[derive(Debug, Clone)]
pub struct DraftStorage {
    pub root: PathBuf,
}

impl DraftStorage {
    pub fn init(repo_root: &Path) -> Result<Self, DraftError> {
        let root = repo_root.join(".draft");
        
        let dirs = [
            root.clone(),
            root.join("sessions"),
            root.join("objects/blobs"),
            root.join("checkpoints"),
            root.join("verification"),
            root.join("receipts"),
            root.join("logs"),
        ];

        for dir in &dirs {
            if !dir.exists() {
                fs::create_dir_all(dir).map_err(|e| {
                    DraftError::StorageError(format!("Failed to create directory {:?}: {}", dir, e))
                })?;
            }
        }

        Ok(Self { root })
    }

    pub fn open(repo_root: &Path) -> Result<Self, DraftError> {
        let root = repo_root.join(".draft");
        if !root.exists() || !root.is_dir() {
            return Err(DraftError::StorageError(
                "Draft has not been initialized in this repository. Run 'draft start' first.".to_string(),
            ));
        }
        Ok(Self { root })
    }

    pub fn write_json<T: Serialize>(&self, rel: &Path, value: &T) -> Result<(), DraftError> {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let serialized = serde_json::to_string_pretty(value)
            .map_err(|e| DraftError::StorageError(format!("JSON serialization failed: {}", e)))?;
        
        // Atomic write via temp file
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, serialized.as_bytes())?;
        fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    pub fn read_json<T: DeserializeOwned>(&self, rel: &Path) -> Result<T, DraftError> {
        let path = self.root.join(rel);
        if !path.exists() {
            return Err(DraftError::StorageError(format!("JSON file not found: {:?}", path)));
        }
        let content = fs::read_to_string(&path)?;
        let value = serde_json::from_str(&content)
            .map_err(|e| DraftError::StorageError(format!("JSON deserialization failed: {}", e)))?;
        Ok(value)
    }

    pub fn write_toml<T: Serialize>(&self, rel: &Path, value: &T) -> Result<(), DraftError> {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let serialized = toml::to_string_pretty(value)
            .map_err(|e| DraftError::StorageError(format!("TOML serialization failed: {}", e)))?;
        
        // Atomic write via temp file
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, serialized.as_bytes())?;
        fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    pub fn read_toml<T: DeserializeOwned>(&self, rel: &Path) -> Result<T, DraftError> {
        let path = self.root.join(rel);
        if !path.exists() {
            return Err(DraftError::StorageError(format!("TOML file not found: {:?}", path)));
        }
        let content = fs::read_to_string(&path)?;
        let value = toml::from_str(&content)
            .map_err(|e| DraftError::StorageError(format!("TOML deserialization failed: {}", e)))?;
        Ok(value)
    }

    pub fn write_blob(&self, bytes: &[u8]) -> Result<ObjectHash, DraftError> {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash = format!("{:x}", hasher.finalize());

        let blob_dir = self.root.join("objects").join("blobs");
        let blob_path = blob_dir.join(&hash);

        if !blob_path.exists() {
            fs::write(&blob_path, bytes)?;
        }

        Ok(hash)
    }

    pub fn read_blob(&self, hash: &ObjectHash) -> Result<Vec<u8>, DraftError> {
        let blob_path = self.root.join("objects").join("blobs").join(hash);
        if !blob_path.exists() {
            return Err(DraftError::StorageError(format!("Blob {} not found", hash)));
        }
        let mut file = File::open(&blob_path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub fn append_log(&self, message: &str) -> Result<(), DraftError> {
        let log_path = self.root.join("logs").join("draft.log");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;

        let timestamp = Utc::now().to_rfc3339();
        writeln!(file, "[{}] {}", timestamp, message)?;
        Ok(())
    }
}
