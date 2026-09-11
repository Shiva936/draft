//! The signed DraftPack envelope.
//!
//! An exporter signs the manifest, and the signature covers the signer binding
//! alongside it — the same rule as receipts, for the same reason: a signature
//! over the payload alone would leave the binding as unauthenticated metadata
//! anyone could rewrite, so a pack could be re-attributed to a different
//! exporter and still verify.
//!
//! Because the manifest names every member with its exact digest, signing the
//! manifest transitively attests the whole archive. A recipient checks the
//! signature once, then checks members against the manifest.
//!
//! # What a valid signature does not mean
//!
//! It means these bytes came from the holder of that key, unmodified. It does
//! **not** mean the key is trusted here, that the Baseline should be adopted,
//! or that anything in the pack is authorized locally. Trust is the importer's
//! decision, and this crate deliberately cannot make it.

use serde::{Deserialize, Serialize};

use draft_dcg_contract::digest::domain_hash;
use draft_dcg_contract::{canonical_bytes, verify_signature, ReceiptSignerBinding};

use crate::manifest::DraftpackManifest;
use crate::{FormatError, FormatResult};

/// The frozen domain separator a DraftPack signature is taken under.
///
/// Distinct from the receipt signature domain, so a receipt signature can never
/// be replayed as a pack signature or the reverse.
pub const DRAFTPACK_SIGNATURE_DOMAIN: &str = "draft.draftpack.signature/v1";

/// Exactly the bytes a DraftPack signature is computed over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftpackSigningMessage {
    pub manifest: DraftpackManifest,
    pub signer: ReceiptSignerBinding,
}

impl DraftpackSigningMessage {
    /// The exact message bytes to sign or verify.
    pub fn signing_bytes(&self) -> FormatResult<Vec<u8>> {
        self.manifest.validate()?;
        let canonical = canonical_bytes(self)?;
        Ok(
            domain_hash(DRAFTPACK_SIGNATURE_DOMAIN, [canonical.as_slice()])
                .as_str()
                .as_bytes()
                .to_vec(),
        )
    }
}

/// A DraftPack manifest with its exporter's signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftpackEnvelope {
    pub manifest: DraftpackManifest,
    pub signer: ReceiptSignerBinding,
    /// The detached base64 signature over [`DraftpackSigningMessage`].
    pub signature: String,
}

impl DraftpackEnvelope {
    /// The signing message this envelope's signature should cover.
    pub fn signing_message(&self) -> DraftpackSigningMessage {
        DraftpackSigningMessage {
            manifest: self.manifest.clone(),
            signer: self.signer.clone(),
        }
    }

    /// Verification level 1: the envelope is structurally sound.
    pub fn verify_structure(&self) -> FormatResult<()> {
        self.manifest.validate()?;
        if self.signature.is_empty() {
            return Err(FormatError::Signature(
                "DraftPack envelope carries no signature".into(),
            ));
        }
        Ok(())
    }

    /// Verification level 2: the signature verifies under `public_key_b64`.
    ///
    /// Says nothing about whether that key is trusted — level 3 is the
    /// importer's, and reporting it as `valid` from here would be a lie.
    pub fn verify_signature_with(&self, public_key_b64: &str) -> FormatResult<bool> {
        self.verify_structure()?;
        let message = self.signing_message().signing_bytes()?;
        verify_signature(public_key_b64, &message, &self.signature).map_err(FormatError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    use std::collections::BTreeSet;

    use draft_dcg_contract::identifier::NamespacedId;
    use draft_dcg_contract::ids::{ActorId, ProjectId};
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::{BaselineId, Digest, ProducerIdentity};

    use crate::manifest::ArchiveEntryMetadata;
    use crate::path::SafeEntryPath;
    use crate::DRAFTPACK_FORMAT_REVISION;

    const BASE64: base64::engine::general_purpose::GeneralPurpose =
        base64::engine::general_purpose::STANDARD;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[23u8; 32])
    }

    fn public(key: &SigningKey) -> String {
        BASE64.encode(key.verifying_key().to_bytes())
    }

    fn signer() -> ReceiptSignerBinding {
        ReceiptSignerBinding::new(
            ActorId::parse("act_exporter").unwrap(),
            "export-key-1",
            "ed25519",
        )
        .unwrap()
    }

    fn manifest() -> DraftpackManifest {
        DraftpackManifest {
            format_revision: DRAFTPACK_FORMAT_REVISION,
            project: ProjectId::parse("prj_a1").unwrap(),
            baseline: BaselineId::new(Digest::of_bytes(b"baseline")),
            entries: BTreeSet::from([ArchiveEntryMetadata::describe(
                SafeEntryPath::parse("objects/blake3/aa").unwrap(),
                b"payload",
            )
            .unwrap()]),
            receipts: Vec::new(),
            exported_by: ActorId::parse("act_a1").unwrap(),
            exported_at: Timestamp::from_unix_nanos(1_000),
            producer: ProducerIdentity::new(
                NamespacedId::parse("draft.core/draftpack").unwrap(),
                "0.3.4",
            )
            .unwrap(),
        }
    }

    fn sign(manifest: DraftpackManifest, signer: ReceiptSignerBinding) -> DraftpackEnvelope {
        let message = DraftpackSigningMessage {
            manifest: manifest.clone(),
            signer: signer.clone(),
        };
        let signature = BASE64.encode(key().sign(&message.signing_bytes().unwrap()).to_bytes());
        DraftpackEnvelope {
            manifest,
            signer,
            signature,
        }
    }

    #[test]
    fn a_genuine_envelope_verifies() {
        let envelope = sign(manifest(), signer());
        envelope.verify_structure().unwrap();
        assert!(envelope.verify_signature_with(&public(&key())).unwrap());
    }

    #[test]
    fn tampering_with_any_member_breaks_the_signature() {
        // The manifest names each member's digest, so re-signing is the only
        // way to change a member — which is the whole point.
        let mut tampered = sign(manifest(), signer());
        tampered.manifest.entries = BTreeSet::from([ArchiveEntryMetadata::describe(
            SafeEntryPath::parse("objects/blake3/aa").unwrap(),
            b"substituted",
        )
        .unwrap()]);
        assert!(!tampered.verify_signature_with(&public(&key())).unwrap());
    }

    #[test]
    fn the_exporter_binding_cannot_be_rewritten() {
        let mut reattributed = sign(manifest(), signer());
        reattributed.signer.signer_identity = ActorId::parse("act_impostor").unwrap();
        assert!(!reattributed.verify_signature_with(&public(&key())).unwrap());
    }

    #[test]
    fn redirecting_the_baseline_breaks_the_signature() {
        let mut redirected = sign(manifest(), signer());
        redirected.manifest.baseline = BaselineId::new(Digest::of_bytes(b"other-baseline"));
        assert!(!redirected.verify_signature_with(&public(&key())).unwrap());
    }

    #[test]
    fn another_key_does_not_verify_a_pack() {
        let envelope = sign(manifest(), signer());
        let other = SigningKey::from_bytes(&[24u8; 32]);
        assert!(!envelope.verify_signature_with(&public(&other)).unwrap());
    }

    #[test]
    fn a_pack_signature_lives_in_its_own_domain() {
        // A receipt signature must not be replayable as a pack signature.
        let message = DraftpackSigningMessage {
            manifest: manifest(),
            signer: signer(),
        };
        let bytes = message.signing_bytes().unwrap();
        let receipt_domain = domain_hash(
            draft_dcg_contract::RECEIPT_SIGNATURE_DOMAIN,
            [canonical_bytes(&message).unwrap().as_slice()],
        );
        assert_ne!(bytes, receipt_domain.as_str().as_bytes().to_vec());
    }

    #[test]
    fn an_unsigned_or_invalid_envelope_fails_structurally() {
        let mut unsigned = sign(manifest(), signer());
        unsigned.signature = String::new();
        assert!(unsigned.verify_structure().is_err());

        let mut future = sign(manifest(), signer());
        future.manifest.format_revision = 2;
        assert!(future.verify_structure().is_err());
    }

    #[test]
    fn the_wire_form_round_trips() {
        let envelope = sign(manifest(), signer());
        let encoded = serde_json::to_string(&envelope).unwrap();
        assert_eq!(
            serde_json::from_str::<DraftpackEnvelope>(&encoded).unwrap(),
            envelope
        );
    }
}
