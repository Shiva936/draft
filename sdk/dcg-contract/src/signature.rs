//! The dependency-light Ed25519 verification primitive.
//!
//! Verification only. This crate never signs and never holds private key
//! material: a portable verifier needs to check a signature, and nothing that
//! only checks signatures should be able to produce them.
//!
//! `draft-extension-contract` reuses this primitive for catalog role
//! signatures, so Draft has exactly one Ed25519 verification path rather than
//! two that could disagree.

use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::{FormatError, FormatResult};

const BASE64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// The one signature algorithm v1 uses.
pub const SIGNATURE_ALGORITHM: &str = "ed25519";

/// Verify one detached base64 Ed25519 signature over `message`.
///
/// Returns `Ok(false)` for a well-formed signature that does not verify, and
/// `Err` only when the key or signature material is malformed. The two are
/// kept apart because they mean different things: a bad signature is a failed
/// attestation, while unusable key material is a broken document.
pub fn verify_signature(
    public_key_b64: &str,
    message: &[u8],
    signature_b64: &str,
) -> FormatResult<bool> {
    let key_bytes = BASE64
        .decode(public_key_b64.trim())
        .map_err(|_| FormatError::Signature("public key is not valid base64".into()))?;
    let key_bytes: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| FormatError::Signature("public key must be 32 bytes".into()))?;
    let verifying = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|_| FormatError::Signature("invalid Ed25519 public key".into()))?;

    let signature_bytes = BASE64
        .decode(signature_b64.trim())
        .map_err(|_| FormatError::Signature("signature is not valid base64".into()))?;
    let signature_bytes: [u8; 64] = signature_bytes
        .as_slice()
        .try_into()
        .map_err(|_| FormatError::Signature("signature must be 64 bytes".into()))?;

    Ok(verifying
        .verify(message, &Signature::from_bytes(&signature_bytes))
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn public(key: &SigningKey) -> String {
        BASE64.encode(key.verifying_key().to_bytes())
    }

    #[test]
    fn a_genuine_signature_verifies() {
        let key = key();
        let signature = BASE64.encode(key.sign(b"payload").to_bytes());
        assert!(verify_signature(&public(&key), b"payload", &signature).unwrap());
    }

    #[test]
    fn a_signature_over_other_bytes_does_not_verify() {
        let key = key();
        let signature = BASE64.encode(key.sign(b"payload").to_bytes());
        assert!(!verify_signature(&public(&key), b"other payload", &signature).unwrap());
    }

    #[test]
    fn another_keys_signature_does_not_verify() {
        let signer = key();
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let signature = BASE64.encode(other.sign(b"payload").to_bytes());
        assert!(!verify_signature(&public(&signer), b"payload", &signature).unwrap());
    }

    #[test]
    fn malformed_material_is_an_error_not_a_quiet_false() {
        // The distinction matters: "this did not verify" and "this document is
        // unreadable" call for different handling upstream.
        assert!(verify_signature("not base64!", b"x", "also not base64!").is_err());
        assert!(
            verify_signature(&BASE64.encode([1u8; 8]), b"x", &BASE64.encode([2u8; 64])).is_err()
        );
        let key = key();
        assert!(verify_signature(&public(&key), b"x", &BASE64.encode([2u8; 8])).is_err());
    }
}
