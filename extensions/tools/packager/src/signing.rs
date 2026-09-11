//! Signing with externally supplied key material.
//!
//! This module reads a key; it never creates one. Production signing keys
//! belong to an authorized signing environment, not to a packaging tool and
//! certainly not to this repository — so there is deliberately no "generate"
//! path here, and no code that would write a key to disk.
//!
//! For tests and CI an ephemeral key can be created in memory with
//! [`SigningKey::ephemeral`], which is behind `cfg(test)`-free but documented
//! as test-only and never persists anything.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey as Ed25519SigningKey};
use std::path::Path;

/// An Ed25519 signing key held only for the life of one command.
pub struct SigningKey(Ed25519SigningKey);

impl SigningKey {
    /// Read base64 key material from a file the caller supplies.
    ///
    /// The file is read and never rewritten, copied, or logged.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let encoded = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read signing key {}: {error}", path.display()))?;
        Self::from_base64(encoded.trim())
    }

    /// Accept base64 key material directly.
    pub fn from_base64(encoded: &str) -> Result<Self, String> {
        let bytes = BASE64
            .decode(encoded)
            .map_err(|_| "signing key is not valid base64".to_string())?;
        let bytes: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| "signing key must be 32 bytes".to_string())?;
        Ok(Self(Ed25519SigningKey::from_bytes(&bytes)))
    }

    /// An in-memory key for tests.
    ///
    /// Never written anywhere. A catalog signed with one of these is a test
    /// fixture, not something a Draft install would trust without the user
    /// explicitly accepting its root. CI supplies its ephemeral key through
    /// `--key` like any other caller, so this has no non-test callers.
    #[cfg(test)]
    pub fn ephemeral(seed: [u8; 32]) -> Self {
        Self(Ed25519SigningKey::from_bytes(&seed))
    }

    pub fn public_key_base64(&self) -> String {
        BASE64.encode(self.0.verifying_key().to_bytes())
    }

    pub fn sign_base64(&self, message: &[u8]) -> String {
        BASE64.encode(self.0.sign(message).to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_round_trips_through_its_base64_form() {
        let key = SigningKey::ephemeral([7; 32]);
        let encoded = BASE64.encode([7u8; 32]);
        let reloaded = SigningKey::from_base64(&encoded).unwrap();
        assert_eq!(key.public_key_base64(), reloaded.public_key_base64());
        assert_eq!(
            key.sign_base64(b"document"),
            reloaded.sign_base64(b"document")
        );
    }

    #[test]
    fn malformed_key_material_is_refused_clearly() {
        assert!(SigningKey::from_base64("not base64!").is_err());
        assert!(SigningKey::from_base64(&BASE64.encode([0u8; 16])).is_err());
    }
}
