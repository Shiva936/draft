use std::path::{Path, PathBuf};

use crate::errors::DraftError;
use crate::models::{CommitReceipt, GitOid};
use crate::storage::DraftStorage;

pub struct ReceiptEngine;

impl ReceiptEngine {
    pub fn create(
        storage: &DraftStorage,
        receipt: CommitReceipt,
    ) -> Result<(), DraftError> {
        let rel_path = PathBuf::from("receipts").join(format!("{}.json", receipt.commit_hash));
        storage.write_json(&rel_path, &receipt)?;
        storage.append_log(&format!("Commit receipt stored: {}", receipt.commit_hash))?;
        Ok(())
    }

    pub fn read(
        storage: &DraftStorage,
        commit_hash: &GitOid,
    ) -> Result<CommitReceipt, DraftError> {
        let rel_path = PathBuf::from("receipts").join(format!("{}.json", commit_hash));
        storage.read_json(&rel_path)
    }

    pub fn latest(storage: &DraftStorage) -> Result<Option<CommitReceipt>, DraftError> {
        let receipts_dir = storage.root.join("receipts");
        if !receipts_dir.exists() {
            return Ok(None);
        }

        let mut latest_receipt: Option<CommitReceipt> = None;

        for entry in std::fs::read_dir(receipts_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
                let rel_path = path.strip_prefix(&storage.root)
                    .map_err(|e| DraftError::StorageError(e.to_string()))?;

                if let Ok(receipt) = storage.read_json::<CommitReceipt>(Path::new(rel_path)) {
                    match &latest_receipt {
                        None => latest_receipt = Some(receipt),
                        Some(current) => {
                            if receipt.created_at > current.created_at {
                                latest_receipt = Some(receipt);
                            }
                        }
                    }
                }
            }
        }

        Ok(latest_receipt)
    }
}
