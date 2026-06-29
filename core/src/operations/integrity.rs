//! Operation integrity hashing (structured for future signing).

use sha2::{Digest, Sha256};

use super::types::{DraftOperation, OperationIntegrity};

pub const ALGORITHM: &str = "sha256";

/// Compute the integrity hash of an operation, excluding the integrity field
/// itself (the field is zeroed before hashing so the result is reproducible).
pub fn compute(op: &DraftOperation) -> OperationIntegrity {
    let mut clone = op.clone();
    clone.integrity = OperationIntegrity {
        algorithm: ALGORITHM.to_string(),
        content_sha256: String::new(),
    };
    let canonical = serde_json::to_vec(&clone).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(&canonical);
    OperationIntegrity {
        algorithm: ALGORITHM.to_string(),
        content_sha256: format!("{:x}", hasher.finalize()),
    }
}

/// Verify an operation's recorded integrity hash.
pub fn verify(op: &DraftOperation) -> bool {
    compute(op).content_sha256 == op.integrity.content_sha256
}
