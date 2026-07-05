//! Local transparency log (PRD §9.10, TDD §16).
//!
//! An append-only, hash-chained, signed log of receipts at
//! `.draft/transparency/chain.log`. Each entry links a receipt into a chain
//! (`previous_entry_hash` → `entry_hash`) and is signed by the acting key, so
//! removing or reordering receipts is detectable independently of the event log.

use crate::error::{DraftError, DraftResult};
use crate::fsutil;
use crate::hashing;
use crate::layout::ProjectPaths;
use crate::signing::Keypair;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Genesis previous-entry hash for the first transparency entry.
pub const GENESIS_ENTRY_HASH: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// One entry in the transparency chain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransparencyEntry {
    pub entry_index: u64,
    pub receipt_id: String,
    pub event_hash: String,
    pub previous_entry_hash: String,
    pub entry_hash: String,
    pub timestamp: String,
    pub actor_id: String,
    pub signature: String,
}

impl TransparencyEntry {
    /// Canonical bytes (excludes `entry_hash` and `signature`) used to derive
    /// the entry hash and the signature.
    fn signable(&self) -> String {
        let content = json!({
            "entry_index": self.entry_index,
            "receipt_id": self.receipt_id,
            "event_hash": self.event_hash,
            "previous_entry_hash": self.previous_entry_hash,
            "timestamp": self.timestamp,
            "actor_id": self.actor_id,
        });
        hashing::canonical_json(&content)
    }

    pub fn recompute_entry_hash(&self) -> String {
        hashing::sha256_hex(self.signable().as_bytes())
    }
}

/// The transparency log bound to a project store.
pub struct TransparencyLog {
    paths: ProjectPaths,
}

impl TransparencyLog {
    pub fn new(paths: ProjectPaths) -> Self {
        TransparencyLog { paths }
    }

    pub fn read_all(&self) -> DraftResult<Vec<TransparencyEntry>> {
        let path = self.paths.transparency_chain();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| DraftError::storage(format!("read chain.log: {e}")))?;
        let mut out = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: TransparencyEntry = serde_json::from_str(line).map_err(|e| {
                DraftError::new(
                    crate::error::DraftErrorKind::OperationLogCorrupt,
                    format!("chain.log line {} corrupt: {e}", i + 1),
                )
            })?;
            out.push(entry);
        }
        Ok(out)
    }

    /// Append a receipt into the chain, signing the new entry with `keypair`.
    pub fn append(
        &self,
        receipt_id: &str,
        event_hash: &str,
        actor_id: &str,
        keypair: &Keypair,
    ) -> DraftResult<TransparencyEntry> {
        fsutil::ensure_dir(&self.paths.transparency_dir())?;
        let all = self.read_all()?;
        let previous_entry_hash = all
            .last()
            .map(|e| e.entry_hash.clone())
            .unwrap_or_else(|| GENESIS_ENTRY_HASH.to_string());
        let mut entry = TransparencyEntry {
            entry_index: all.len() as u64,
            receipt_id: receipt_id.to_string(),
            event_hash: event_hash.to_string(),
            previous_entry_hash,
            entry_hash: String::new(),
            timestamp: crate::common::now().to_rfc3339(),
            actor_id: actor_id.to_string(),
            signature: String::new(),
        };
        entry.entry_hash = entry.recompute_entry_hash();
        entry.signature = keypair.sign_b64(entry.entry_hash.as_bytes());

        let line = serde_json::to_string(&entry)
            .map_err(|e| DraftError::storage(format!("serialize entry: {e}")))?;
        let mut buf = if self.paths.transparency_chain().exists() {
            std::fs::read(self.paths.transparency_chain())
                .map_err(|e| DraftError::storage(e.to_string()))?
        } else {
            Vec::new()
        };
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        fsutil::write_atomic(&self.paths.transparency_chain(), &buf)?;
        Ok(entry)
    }

    /// Verify the chain structure and each entry's hash. Signature verification
    /// requires a resolver from `public_key_id`/`actor_id` to a public key; when
    /// `resolve_key` returns `Some`, the signature is checked too.
    pub fn verify<F>(&self, resolve_key: F) -> DraftResult<usize>
    where
        F: Fn(&str) -> Option<String>,
    {
        let all = self.read_all()?;
        let mut prev = GENESIS_ENTRY_HASH.to_string();
        for (i, entry) in all.iter().enumerate() {
            if entry.entry_index != i as u64 {
                return Err(corrupt(i, "entry index out of order"));
            }
            if entry.previous_entry_hash != prev {
                return Err(corrupt(i, "previous entry hash mismatch"));
            }
            if entry.recompute_entry_hash() != entry.entry_hash {
                return Err(corrupt(i, "tampered entry hash"));
            }
            if let Some(pk) = resolve_key(&entry.actor_id) {
                let sig_ok =
                    crate::signing::verify_b64(&pk, entry.entry_hash.as_bytes(), &entry.signature)
                        .unwrap_or(false);
                if !sig_ok {
                    return Err(corrupt(i, "invalid entry signature"));
                }
            }
            prev = entry.entry_hash.clone();
        }
        Ok(all.len())
    }
}

fn corrupt(i: usize, msg: &str) -> DraftError {
    DraftError::new(
        crate::error::DraftErrorKind::OperationLogCorrupt,
        format!("transparency entry {}: {msg}", i + 1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_verify_chain() {
        let tmp = tempfile::tempdir().unwrap();
        let log = TransparencyLog::new(ProjectPaths::for_root(tmp.path()));
        let kp = Keypair::generate();
        let pk = kp.public_key_b64();
        log.append("rcp_1", "sha256:a", "act_1", &kp).unwrap();
        log.append("rcp_2", "sha256:b", "act_1", &kp).unwrap();
        let n = log.verify(|_| Some(pk.clone())).unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn tampering_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let log = TransparencyLog::new(ProjectPaths::for_root(tmp.path()));
        let kp = Keypair::generate();
        log.append("rcp_1", "sha256:a", "act_1", &kp).unwrap();
        let path = log.paths.transparency_chain();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("rcp_1", "rcp_x")).unwrap();
        assert!(log.verify(|_| Some(kp.public_key_b64())).is_err());
    }
}
