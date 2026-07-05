//! Ed25519 signing and verification for receipts and the transparency log
//! (PRD §9.8, TDD §11.3, NFRD §4.6).
//!
//! The private key exists only as `~/.draft/keys/signing.key` (mode 0600); the
//! public half is published as a base64 string plus a stable `public_key_id`
//! that receipts reference. Verification resolves an actor's public key from the
//! project/global trust material and checks the detached signature over the
//! canonical serialized payload.

use crate::error::{DraftError, DraftResult};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use std::path::Path;

/// The signing algorithm identifier stamped into receipts.
pub const SIGNATURE_ALGORITHM: &str = "Ed25519";

/// A loaded keypair capable of producing signatures.
pub struct Keypair {
    inner: SigningKey,
}

impl Keypair {
    /// Generate a fresh random keypair.
    pub fn generate() -> Self {
        let mut rng = rand::rngs::OsRng;
        Keypair {
            inner: SigningKey::generate(&mut rng),
        }
    }

    /// The base64-encoded 32-byte public key.
    pub fn public_key_b64(&self) -> String {
        B64.encode(self.inner.verifying_key().to_bytes())
    }

    /// A stable identifier derived from the public key.
    pub fn public_key_id(&self) -> String {
        public_key_id_for(&self.inner.verifying_key().to_bytes())
    }

    /// Sign `message`, returning a base64 detached signature.
    pub fn sign_b64(&self, message: &[u8]) -> String {
        let sig: Signature = self.inner.sign(message);
        B64.encode(sig.to_bytes())
    }

    /// Persist the private key to `path` with restrictive permissions (0600).
    pub fn save(&self, path: &Path) -> DraftResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DraftError::storage(format!("create key dir: {e}")))?;
            let _ = crate::hidden::restrict_dir(parent, 0o700);
        }
        let encoded = B64.encode(self.inner.to_bytes());
        crate::fsutil::write_atomic(path, encoded.as_bytes())
            .map_err(|e| DraftError::storage(format!("write signing key: {e}")))?;
        crate::hidden::restrict_file(path, 0o600)
            .map_err(|e| DraftError::storage(format!("chmod signing key: {e}")))?;
        Ok(())
    }

    /// Load a private key previously written by [`Keypair::save`].
    pub fn load(path: &Path) -> DraftResult<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| DraftError::storage(format!("read signing key: {e}")))?;
        let bytes = B64
            .decode(raw.trim())
            .map_err(|_| DraftError::invalid_config("signing key is not valid base64"))?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| DraftError::invalid_config("signing key must be 32 bytes"))?;
        Ok(Keypair {
            inner: SigningKey::from_bytes(&arr),
        })
    }
}

/// Derive the stable `public_key_id` for a raw 32-byte public key.
pub fn public_key_id_for(public_key: &[u8]) -> String {
    let digest = Sha256::digest(public_key);
    format!("key_ed25519_{}", hex12(&digest))
}

/// Verify a base64 detached `signature` over `message` using a base64 public key.
pub fn verify_b64(public_key_b64: &str, message: &[u8], signature_b64: &str) -> DraftResult<bool> {
    let pk_bytes = B64
        .decode(public_key_b64.trim())
        .map_err(|_| DraftError::invalid_config("public key is not valid base64"))?;
    let pk_arr: [u8; 32] = pk_bytes
        .as_slice()
        .try_into()
        .map_err(|_| DraftError::invalid_config("public key must be 32 bytes"))?;
    let verifying = VerifyingKey::from_bytes(&pk_arr)
        .map_err(|_| DraftError::invalid_config("invalid Ed25519 public key"))?;
    let sig_bytes = B64
        .decode(signature_b64.trim())
        .map_err(|_| DraftError::invalid_config("signature is not valid base64"))?;
    let sig_arr: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| DraftError::invalid_config("signature must be 64 bytes"))?;
    let sig = Signature::from_bytes(&sig_arr);
    Ok(verifying.verify(message, &sig).is_ok())
}

fn hex12(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(12);
    for b in bytes.iter().take(6) {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_roundtrip() {
        let kp = Keypair::generate();
        let msg = b"canonical-receipt-bytes";
        let sig = kp.sign_b64(msg);
        assert!(verify_b64(&kp.public_key_b64(), msg, &sig).unwrap());
        // Tampered message fails.
        assert!(!verify_b64(&kp.public_key_b64(), b"other", &sig).unwrap());
    }

    #[test]
    fn key_id_is_stable_and_prefixed() {
        let kp = Keypair::generate();
        let id = kp.public_key_id();
        assert!(id.starts_with("key_ed25519_"));
        assert_eq!(id, kp.public_key_id());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys/signing.key");
        let kp = Keypair::generate();
        kp.save(&path).unwrap();
        let loaded = Keypair::load(&path).unwrap();
        assert_eq!(kp.public_key_b64(), loaded.public_key_b64());
        let msg = b"x";
        let sig = kp.sign_b64(msg);
        assert!(verify_b64(&loaded.public_key_b64(), msg, &sig).unwrap());
    }
}
