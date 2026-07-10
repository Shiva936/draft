//! Stable base and `stable_head` storage for Draft v0.3.3.

use crate::error::{DraftError, DraftErrorKind, DraftResult};
use crate::fsutil;
use crate::hashing;
use crate::layout::ProjectPaths;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveMode {
    #[default]
    MergeAndDispose,
    DisposeOnly,
}

impl SaveMode {
    pub fn parse(raw: &str) -> DraftResult<Self> {
        match raw {
            "merge_and_dispose" => Ok(Self::MergeAndDispose),
            "dispose_only" => Ok(Self::DisposeOnly),
            other => Err(DraftError::invalid_config(format!(
                "invalid save.pack_disposal '{other}' (expected merge_and_dispose or dispose_only)"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MergeAndDispose => "merge_and_dispose",
            Self::DisposeOnly => "dispose_only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Verified,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSummary {
    pub pack_id: String,
    pub name: Option<String>,
    pub affected_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StableHead {
    pub schema_version: String,
    pub id: String,
    pub receipt_id: String,
    pub workspace_hash: String,
    pub base_hash: String,
    pub previous_stable_head: Option<String>,
    pub finalized_pack_digest: Option<String>,
    pub finalized_pack_summary: Option<PackSummary>,
    pub verification_result: VerificationStatus,
    pub save_mode: Option<SaveMode>,
    pub timestamp: DateTime<Utc>,
    pub stable_head_hash: String,
}

impl StableHead {
    pub fn canonical_hash_without_self(&self) -> String {
        let mut clone = self.clone();
        clone.stable_head_hash.clear();
        hashing::canonical_hash(&clone)
    }

    pub fn verify_integrity(&self) -> DraftResult<()> {
        let expected = self.canonical_hash_without_self();
        if self.stable_head_hash != expected {
            return Err(DraftError::new(
                DraftErrorKind::Storage,
                format!(
                    "stable_head integrity check failed: expected {expected}, found {}",
                    self.stable_head_hash
                ),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StableHeadReport {
    pub stable_head_id: String,
    pub receipt_id: String,
    pub workspace_hash: String,
    pub base_hash: String,
    pub previous_stable_head: Option<String>,
}

pub struct StableHeadStore {
    paths: ProjectPaths,
}

impl StableHeadStore {
    pub fn new(paths: ProjectPaths) -> Self {
        StableHeadStore { paths }
    }

    pub fn exists(&self) -> bool {
        self.paths.stable_head_file().exists()
    }

    pub fn read(&self) -> DraftResult<StableHead> {
        let head: StableHead = fsutil::read_json(&self.paths.stable_head_file())?;
        head.verify_integrity()?;
        Ok(head)
    }

    pub fn write(&self, head: &StableHead) -> DraftResult<()> {
        fsutil::ensure_dir(&self.paths.stable_head_dir())?;
        let mut h = head.clone();
        h.stable_head_hash = h.canonical_hash_without_self();
        fsutil::write_json(&self.paths.stable_head_file(), &h)
    }

    pub fn initialize(
        &self,
        root: &std::path::Path,
        receipt_id: String,
    ) -> DraftResult<StableHead> {
        let workspace_hash = hashing::workspace_hash(root)?;
        let mut head = StableHead {
            schema_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            id: receipt_id.clone(),
            receipt_id,
            workspace_hash: workspace_hash.clone(),
            base_hash: workspace_hash,
            previous_stable_head: None,
            finalized_pack_digest: None,
            finalized_pack_summary: None,
            verification_result: VerificationStatus::Verified,
            save_mode: None,
            timestamp: crate::common::now(),
            stable_head_hash: String::new(),
        };
        head.stable_head_hash = head.canonical_hash_without_self();
        self.write(&head)?;
        Ok(head)
    }

    pub fn advance(
        &self,
        root: &std::path::Path,
        receipt_id: String,
        previous: Option<StableHead>,
        pack_digest: Option<String>,
        pack_summary: Option<PackSummary>,
        save_mode: SaveMode,
    ) -> DraftResult<StableHead> {
        let workspace_hash = hashing::workspace_hash(root)?;
        let mut head = StableHead {
            schema_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            id: receipt_id.clone(),
            receipt_id,
            workspace_hash: workspace_hash.clone(),
            base_hash: workspace_hash,
            previous_stable_head: previous.map(|h| h.id),
            finalized_pack_digest: pack_digest,
            finalized_pack_summary: pack_summary,
            verification_result: VerificationStatus::Verified,
            save_mode: Some(save_mode),
            timestamp: crate::common::now(),
            stable_head_hash: String::new(),
        };
        head.stable_head_hash = head.canonical_hash_without_self();
        self.write(&head)?;
        Ok(head)
    }

    pub fn report(&self) -> DraftResult<StableHeadReport> {
        let h = self.read()?;
        Ok(StableHeadReport {
            stable_head_id: h.id,
            receipt_id: h.receipt_id,
            workspace_hash: h.workspace_hash,
            base_hash: h.base_hash,
            previous_stable_head: h.previous_stable_head,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_head_hash_detects_tampering() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("file.txt"), "hello").unwrap();
        let paths = ProjectPaths::for_root(tmp.path());
        paths.create_all().unwrap();
        let store = StableHeadStore::new(paths);
        let mut head = store
            .initialize(tmp.path(), "rcp_teststablehead".to_string())
            .unwrap();
        head.workspace_hash = hashing::sha256_hex(b"tampered");
        assert!(head.verify_integrity().is_err());
    }

    #[test]
    fn save_mode_rejects_unknown_values() {
        assert_eq!(
            SaveMode::parse("merge_and_dispose").unwrap(),
            SaveMode::MergeAndDispose
        );
        assert_eq!(
            SaveMode::parse("dispose_only").unwrap(),
            SaveMode::DisposeOnly
        );
        assert!(SaveMode::parse("keep_everything").is_err());
    }
}
