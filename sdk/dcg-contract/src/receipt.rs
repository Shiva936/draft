//! Receipts: signed attestations of exactly one immutable fact.
//!
//! # The signer binding is authenticated
//!
//! The signature covers a [`ReceiptSigningMessage`], which is the payload
//! **and** the signer binding together. Signing the payload alone would leave
//! the binding — signer identity, key id, algorithm — as unauthenticated
//! metadata anyone could rewrite, so a receipt could be re-attributed to a
//! different signer while still verifying.
//!
//! # Three verification levels, never one word "valid"
//!
//! 1. **Structural / canonical** — the payload parses, is canonical, and its
//!    self-declared `receipt_id` matches the envelope. Answered here.
//! 2. **Cryptographic** — the signature verifies over the signing message under
//!    the declared key. Answered here.
//! 3. **Trust and policy** — was that key trusted at issuance, and is it
//!    trusted now? Core's question, not this crate's.
//!
//! A verifier that can only answer 1 and 2 must report level 3 as `unknown`,
//! never as `valid`.
//!
//! # Signed once, never re-signed
//!
//! The signer is frozen before the crash-sensitive boundary it attests, and a
//! receipt is signed exactly once. Existing receipts are never re-signed after
//! key rotation: a receipt attests what was true when it was issued, and
//! re-signing would silently restate history under new authority.

use serde::{Deserialize, Serialize};

use crate::baseline::BaselineId;
use crate::canonical::canonical_bytes;
use crate::digest::domain_hash;
use crate::ids::{ActorId, PromotionId, ReceiptId};
use crate::publication::{PublicationOutcomeDigest, PublicationResolutionDigest};
use crate::signature::{verify_signature, SIGNATURE_ALGORITHM};
use crate::value::Timestamp;
use crate::{FormatError, FormatResult};

/// The frozen domain separator a receipt signature is taken under.
pub const RECEIPT_SIGNATURE_DOMAIN: &str = "draft.dcg.receipt-signature/v1";

/// Exactly which immutable fact a receipt attests.
///
/// One receipt, one fact. A receipt that could attest a set of things would be
/// unable to say which of them it actually witnessed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReceiptKind {
    /// A Promotion committed, accepting a Baseline.
    Promotion {
        promotion: PromotionId,
        baseline: BaselineId,
    },
    /// A publication attempt concluded with a primary outcome.
    PublicationOutcome { outcome: PublicationOutcomeDigest },
    /// An authorized interpretation of an outcome became authoritative.
    PublicationResolution {
        resolution: PublicationResolutionDigest,
    },
}

/// What a receipt says, including its own identity.
///
/// The `receipt_id` is inside the payload, so the signature covers it: a signed
/// payload cannot be re-filed under a different receipt id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptPayload {
    pub receipt_id: ReceiptId,
    pub subject: ReceiptKind,
    pub issued_by: ActorId,
    pub issued_at: Timestamp,
}

/// Who signed, with what, and how.
///
/// Non-secret throughout: an identity, a key identifier and an algorithm name.
/// No private key material is ever represented in this crate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptSignerBinding {
    pub signer_identity: ActorId,
    pub signing_key_id: String,
    pub signature_algorithm: String,
}

impl ReceiptSignerBinding {
    pub fn new(
        signer_identity: ActorId,
        signing_key_id: impl Into<String>,
        signature_algorithm: impl Into<String>,
    ) -> FormatResult<Self> {
        let binding = Self {
            signer_identity,
            signing_key_id: signing_key_id.into(),
            signature_algorithm: signature_algorithm.into(),
        };
        if binding.signing_key_id.is_empty() {
            return Err(FormatError::Identity(
                "receipt signer binding must name the signing key".into(),
            ));
        }
        if binding.signature_algorithm != SIGNATURE_ALGORITHM {
            return Err(FormatError::Signature(format!(
                "unsupported receipt signature algorithm '{}'; v1 signs with {SIGNATURE_ALGORITHM}",
                binding.signature_algorithm
            )));
        }
        Ok(binding)
    }
}

/// Exactly the bytes a receipt signature is computed over.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptSigningMessage {
    pub payload: ReceiptPayload,
    pub signer: ReceiptSignerBinding,
}

impl ReceiptSigningMessage {
    /// The exact message bytes: the frozen domain separator framed with the
    /// canonical signing message.
    pub fn signing_bytes(&self) -> FormatResult<Vec<u8>> {
        let canonical = canonical_bytes(self)?;
        // Reuse the framed domain construction so the signed bytes cannot be
        // confused with any other domain-separated hash input.
        Ok(
            domain_hash(RECEIPT_SIGNATURE_DOMAIN, [canonical.as_slice()])
                .as_str()
                .as_bytes()
                .to_vec(),
        )
    }
}

/// An immutable, signed receipt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptEnvelope {
    pub payload: ReceiptPayload,
    pub signer: ReceiptSignerBinding,
    /// The detached base64 signature over [`ReceiptSigningMessage`].
    pub signature: String,
}

impl ReceiptEnvelope {
    /// The signing message this envelope's signature should cover.
    pub fn signing_message(&self) -> ReceiptSigningMessage {
        ReceiptSigningMessage {
            payload: self.payload.clone(),
            signer: self.signer.clone(),
        }
    }

    /// Verification level 1: structural and canonical.
    pub fn verify_structure(&self) -> FormatResult<()> {
        ReceiptSignerBinding::new(
            self.signer.signer_identity.clone(),
            self.signer.signing_key_id.clone(),
            self.signer.signature_algorithm.clone(),
        )?;
        if self.signature.is_empty() {
            return Err(FormatError::Signature(
                "receipt envelope carries no signature".into(),
            ));
        }
        Ok(())
    }

    /// Verification level 2: cryptographic, over payload **and** signer.
    ///
    /// Returns `Ok(false)` for a well-formed signature that does not verify.
    /// Says nothing about whether `public_key_b64` is trusted — that is level
    /// 3, and it is Core's to answer.
    pub fn verify_signature_with(&self, public_key_b64: &str) -> FormatResult<bool> {
        self.verify_structure()?;
        let message = self.signing_message().signing_bytes()?;
        verify_signature(public_key_b64, &message, &self.signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};

    const BASE64: base64::engine::general_purpose::GeneralPurpose =
        base64::engine::general_purpose::STANDARD;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[11u8; 32])
    }

    fn public(key: &SigningKey) -> String {
        BASE64.encode(key.verifying_key().to_bytes())
    }

    fn signer() -> ReceiptSignerBinding {
        ReceiptSignerBinding::new(
            ActorId::parse("act_signer").unwrap(),
            "key-1",
            SIGNATURE_ALGORITHM,
        )
        .unwrap()
    }

    fn payload() -> ReceiptPayload {
        ReceiptPayload {
            receipt_id: ReceiptId::parse("rcp_a1b2c3").unwrap(),
            subject: ReceiptKind::Promotion {
                promotion: PromotionId::parse("pro_a1b2c3").unwrap(),
                baseline: BaselineId::new(crate::digest::Digest::of_bytes(b"baseline")),
            },
            issued_by: ActorId::parse("act_issuer").unwrap(),
            issued_at: Timestamp::from_unix_nanos(1_000),
        }
    }

    fn issue(payload: ReceiptPayload, signer: ReceiptSignerBinding) -> ReceiptEnvelope {
        let message = ReceiptSigningMessage {
            payload: payload.clone(),
            signer: signer.clone(),
        };
        let signature = BASE64.encode(key().sign(&message.signing_bytes().unwrap()).to_bytes());
        ReceiptEnvelope {
            payload,
            signer,
            signature,
        }
    }

    #[test]
    fn a_genuine_receipt_passes_both_local_levels() {
        let envelope = issue(payload(), signer());
        envelope.verify_structure().unwrap();
        assert!(envelope.verify_signature_with(&public(&key())).unwrap());
    }

    #[test]
    fn the_signer_binding_cannot_be_rewritten() {
        // The property this design exists for: re-attributing a receipt to a
        // different signer must break the signature, not merely look odd.
        let mut tampered = issue(payload(), signer());
        tampered.signer.signer_identity = ActorId::parse("act_impostor").unwrap();
        assert!(!tampered.verify_signature_with(&public(&key())).unwrap());

        let mut rekeyed = issue(payload(), signer());
        rekeyed.signer.signing_key_id = "key-2".into();
        assert!(!rekeyed.verify_signature_with(&public(&key())).unwrap());
    }

    #[test]
    fn the_receipt_id_is_signed_so_it_cannot_be_refiled() {
        let mut refiled = issue(payload(), signer());
        refiled.payload.receipt_id = ReceiptId::parse("rcp_999999").unwrap();
        assert!(!refiled.verify_signature_with(&public(&key())).unwrap());
    }

    #[test]
    fn changing_the_attested_fact_breaks_the_signature() {
        let mut restated = issue(payload(), signer());
        restated.payload.subject = ReceiptKind::Promotion {
            promotion: PromotionId::parse("pro_999999").unwrap(),
            baseline: BaselineId::new(crate::digest::Digest::of_bytes(b"baseline")),
        };
        assert!(!restated.verify_signature_with(&public(&key())).unwrap());
    }

    #[test]
    fn another_key_does_not_verify_a_receipt() {
        let envelope = issue(payload(), signer());
        let other = SigningKey::from_bytes(&[12u8; 32]);
        assert!(!envelope.verify_signature_with(&public(&other)).unwrap());
    }

    #[test]
    fn cryptographic_validity_says_nothing_about_trust() {
        // Level 2 passes here. Whether `key-1` was ever trusted is level 3, and
        // this crate deliberately cannot answer it.
        let envelope = issue(payload(), signer());
        assert!(envelope.verify_signature_with(&public(&key())).unwrap());
    }

    #[test]
    fn an_unsupported_algorithm_is_refused_at_binding_time() {
        assert!(matches!(
            ReceiptSignerBinding::new(ActorId::parse("act_x").unwrap(), "key-1", "rsa"),
            Err(FormatError::Signature(_))
        ));
        assert!(ReceiptSignerBinding::new(
            ActorId::parse("act_x").unwrap(),
            "",
            SIGNATURE_ALGORITHM
        )
        .is_err());
    }

    #[test]
    fn an_unsigned_envelope_fails_structural_verification() {
        let mut empty = issue(payload(), signer());
        empty.signature = String::new();
        assert!(empty.verify_structure().is_err());
    }

    #[test]
    fn the_wire_form_round_trips() {
        let envelope = issue(payload(), signer());
        let encoded = serde_json::to_string(&envelope).unwrap();
        assert_eq!(
            serde_json::from_str::<ReceiptEnvelope>(&encoded).unwrap(),
            envelope
        );
    }
}
