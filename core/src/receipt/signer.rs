//! Signing and authenticating receipts.
//!
//! # The signed bytes are a canonical message, not the record
//!
//! A signature over "the receipt" would be a signature over whatever the
//! serializer happened to emit. Field order, added optional fields and
//! formatting would all change the bytes without changing the meaning, so a
//! verifier and a signer built at different times could disagree about what
//! was signed while both behaving correctly.
//!
//! [`ReceiptSigningMessage`] fixes exactly what is covered: the payload and
//! the signer binding, canonicalized and framed under a domain separator. The
//! separator is what stops the signed bytes being reusable as any other
//! domain-separated hash input in Draft.
//!
//! # Why the signer binding is inside the signed bytes
//!
//! If the binding sat outside, an attacker could keep a valid signature and
//! rewrite who it claims to be from. Covering it means the signature attests
//! "*this signer* issued *this payload*" as one statement, and swapping either
//! half invalidates it.
//!
//! # What verification does and does not prove
//!
//! A valid signature proves the bytes are unchanged and that whoever held the
//! key produced them. It does not prove the key was trusted at issuance, or is
//! trusted now — those are separate questions answered by the trust registry,
//! and reported separately so that "signature valid, key since revoked" is
//! never collapsed into a single yes or no.

use draft_dcg_contract::receipt::{ReceiptEnvelope, ReceiptSignerBinding, ReceiptSigningMessage};
use draft_dcg_contract::ReceiptPayload;

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::trust::signing::{verify_b64, Keypair};

fn contract_error(error: draft_dcg_contract::FormatError) -> DraftError {
    DraftError::new(DraftErrorKind::Validation, error.to_string())
}

/// Sign a payload, producing the immutable envelope.
pub fn issue(
    payload: ReceiptPayload,
    signer: ReceiptSignerBinding,
    keypair: &Keypair,
) -> DraftResult<ReceiptEnvelope> {
    let message = ReceiptSigningMessage {
        payload: payload.clone(),
        signer: signer.clone(),
    };
    let signature = keypair.sign_b64(&message.signing_bytes().map_err(contract_error)?);
    Ok(ReceiptEnvelope {
        payload,
        signer,
        signature,
    })
}

/// Whether an envelope's signature covers exactly its own payload and binding.
///
/// Deliberately narrow: this is the cryptographic question alone. It says
/// nothing about whether the key was authorized, which is why the trust checks
/// live elsewhere and are reported as their own results.
pub fn is_authentic(envelope: &ReceiptEnvelope, public_key_b64: &str) -> DraftResult<bool> {
    let message = envelope.signing_message();
    verify_b64(
        public_key_b64,
        &message.signing_bytes().map_err(contract_error)?,
        &envelope.signature,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::ids::{ActorId, ReceiptId};
    use draft_dcg_contract::receipt::ReceiptKind;
    use draft_dcg_contract::value::Timestamp;

    fn actor() -> ActorId {
        ActorId::parse("act_000000000001").unwrap()
    }

    fn payload(receipt: &str) -> ReceiptPayload {
        ReceiptPayload {
            receipt_id: ReceiptId::parse(receipt).unwrap(),
            subject: ReceiptKind::Promotion {
                promotion: draft_dcg_contract::ids::PromotionId::parse("pro_000000000001").unwrap(),
                baseline: draft_dcg_contract::BaselineId::new(
                    draft_dcg_contract::Digest::of_bytes(b"baseline"),
                ),
            },
            issued_by: actor(),
            issued_at: Timestamp::from_unix_nanos(0),
        }
    }

    fn binding(key_id: &str) -> ReceiptSignerBinding {
        ReceiptSignerBinding::new(actor(), key_id, "ed25519").unwrap()
    }

    #[test]
    fn a_receipt_verifies_against_the_key_that_signed_it() {
        let keypair = Keypair::generate();
        let envelope = issue(payload("rcp_000000000001"), binding("key-a"), &keypair).unwrap();
        assert!(is_authentic(&envelope, &keypair.public_key_b64()).unwrap());
    }

    #[test]
    fn rewriting_the_payload_invalidates_the_signature() {
        let keypair = Keypair::generate();
        let mut envelope = issue(payload("rcp_000000000001"), binding("key-a"), &keypair).unwrap();
        envelope.payload = payload("rcp_999999999999");
        assert!(!is_authentic(&envelope, &keypair.public_key_b64()).unwrap());
    }

    #[test]
    fn rewriting_the_signer_binding_invalidates_the_signature() {
        // The attack this closes: keep a valid signature, change who it claims
        // to be from. The binding is inside the signed bytes, so the signature
        // attests "this signer issued this payload" as one statement.
        let keypair = Keypair::generate();
        let mut envelope = issue(payload("rcp_000000000001"), binding("key-a"), &keypair).unwrap();
        envelope.signer = binding("key-b");
        assert!(!is_authentic(&envelope, &keypair.public_key_b64()).unwrap());
    }

    #[test]
    fn another_key_does_not_authenticate_it() {
        let keypair = Keypair::generate();
        let other = Keypair::generate();
        let envelope = issue(payload("rcp_000000000001"), binding("key-a"), &keypair).unwrap();
        assert!(!is_authentic(&envelope, &other.public_key_b64()).unwrap());
    }

    #[test]
    fn the_signed_bytes_are_domain_separated() {
        // The framed domain separator is what stops these bytes doubling as
        // any other hash input in Draft: a signature over a receipt must not
        // be replayable as a signature over something else that happened to
        // canonicalize identically.
        let message = ReceiptSigningMessage {
            payload: payload("rcp_000000000001"),
            signer: binding("key-a"),
        };
        let bytes = message.signing_bytes().unwrap();
        let canonical =
            crate::support::hashing::canonical_json(&serde_json::to_value(&message).unwrap());
        assert_ne!(
            bytes,
            canonical.as_bytes(),
            "the signed bytes must be the framed digest, not the bare canonical form"
        );
    }
}
