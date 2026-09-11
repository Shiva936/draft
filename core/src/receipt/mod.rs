//! Signed receipts.
//!
//! Receipts live outside `trust` on purpose. `trust` owns cryptographic
//! mechanism — identities, signing, the registry fence — while a receipt is a
//! *domain fact*: an attestation that one exact thing happened. Keeping them
//! together made it easy to read "the signature verifies" as "the receipt is
//! trustworthy", which are different claims answered by different layers.
//!
//! # What v1 receipts attest
//!
//! Exactly three things, named by [`draft_dcg_contract::receipt::ReceiptKind`]:
//! a Promotion that accepted a Baseline, a Publication attempt's primary
//! outcome, and an authorized Resolution of one. Nothing else. A local action
//! that is already an immutable fact in its own store — a checkpoint, a
//! decision, a sealed revision — is not receipted, because two records of one
//! act with no rule for which is authoritative is worse than one.
//!
//! # The three verification levels are reported separately
//!
//! A signature proves the bytes are unchanged and that whoever held the key
//! produced them. It does not prove the key was trusted at issuance, or that
//! it is trusted now. Those are three questions, and collapsing them into one
//! yes/no is how "signature valid, key since revoked" becomes invisible. What
//! cannot be determined reads `unknown` — never `valid`.

pub mod envelope;
pub mod signer;

pub use envelope::ReceiptEnvelopeStore;

use draft_dcg_contract::receipt::{ReceiptEnvelope, ReceiptKind};

use crate::project::home::DraftGlobalStore;
use crate::project::layout::DraftLayout;
use crate::support::error::DraftResult;
use serde::{Deserialize, Serialize};

/// A signed receipt envelope is a registered contract boundary.
///
/// Its version is `DCG_FORMAT_REVISION` rather than a per-artifact field:
/// the envelope is a portable canonical value, and a version stamped inside
/// it would mean the same receipt serialized differently depending on which
/// side wrote it.
impl crate::contracts::VersionedContract for draft_dcg_contract::receipt::ReceiptEnvelope {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::Receipt;
}

/// The answer to one verification question.
///
/// Three-valued on purpose: an unresolvable signing key makes the signature
/// question unanswerable, and answering it `false` would say the receipt
/// failed when what actually happened is that Draft could not tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Valid,
    Invalid,
    Unknown,
}

impl CheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::Unknown => "unknown",
        }
    }
}

/// One question asked of one receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

fn chk(name: &str, status: CheckStatus, detail: impl Into<String>) -> ReceiptCheck {
    ReceiptCheck {
        name: name.to_string(),
        status,
        detail: detail.into(),
    }
}

/// What verifying one receipt established.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptVerification {
    pub receipt_id: String,
    /// What this receipt attests: `promotion`, `publication-outcome`, or
    /// `publication-resolution`.
    pub subject: String,
    /// True only when every question was answered `valid`. An `unknown`
    /// never counts as a pass.
    pub ok: bool,
    pub checks: Vec<ReceiptCheck>,
}

fn subject_name(kind: &ReceiptKind) -> &'static str {
    match kind {
        ReceiptKind::Promotion { .. } => "promotion",
        ReceiptKind::PublicationOutcome { .. } => "publication-outcome",
        ReceiptKind::PublicationResolution { .. } => "publication-resolution",
    }
}

/// Issue a receipt, store it, and enter it in the transparency chain.
///
/// One call rather than three, because the three are one act: a receipt that
/// exists but was never entered in the chain is not publicly accountable, and
/// a chain entry with no stored receipt names nothing. The store is
/// create-once, so replaying this after a crash converges.
pub fn issue_and_store(
    layout: &DraftLayout,
    payload: draft_dcg_contract::ReceiptPayload,
    signer: draft_dcg_contract::receipt::ReceiptSignerBinding,
    keypair: &crate::trust::signing::Keypair,
) -> DraftResult<ReceiptEnvelope> {
    let envelope = signer::issue(payload, signer, keypair)?;
    ReceiptEnvelopeStore::for_layout(layout).put(&envelope)?;

    // The chain entry names the receipt by the digest of exactly the bytes
    // that were signed, so an entry can never be read as covering a different
    // receipt than the one it was made for.
    let signed = envelope
        .signing_message()
        .signing_bytes()
        .map_err(|error| {
            crate::support::error::DraftError::new(
                crate::support::error::DraftErrorKind::Validation,
                error.to_string(),
            )
        })?;
    let receipt_id = envelope.payload.receipt_id.to_string();
    let chain = crate::trust::transparency::TransparencyLog::new(layout.clone());
    let already = chain
        .read_all()?
        .into_iter()
        .any(|entry| entry.receipt_id == receipt_id);
    if !already {
        chain.append(
            &receipt_id,
            draft_dcg_contract::Digest::of_bytes(&signed).as_str(),
            envelope.payload.issued_by.as_str(),
            &envelope.signer.signing_key_id,
            keypair,
        )?;
    }
    Ok(envelope)
}

/// The verification result for a receipt that could not be read at all.
///
/// Reported as an explicit failure rather than an absence: a receipt whose
/// stored bytes no longer match the digest they were bound to is a fact about
/// this project somebody has to see, and dropping it from the list would make
/// damage look like nothing having happened.
pub fn unreadable(receipt_id: &str, detail: &str) -> ReceiptVerification {
    ReceiptVerification {
        receipt_id: receipt_id.to_string(),
        subject: "unknown".to_string(),
        ok: false,
        checks: vec![chk("structure", CheckStatus::Invalid, detail.to_string())],
    }
}

/// Verify one envelope.
///
/// `public_key` is the base64 key resolved for the envelope's signing key id,
/// `None` when it could not be resolved; `revoked` is the current revoked-key
/// set. Both are passed in rather than looked up here, so this stays a pure
/// function of the bytes and the trust state it was told about.
pub fn verify(
    envelope: &ReceiptEnvelope,
    public_key: Option<&str>,
    revoked: &[String],
) -> ReceiptVerification {
    let mut checks = Vec::new();

    // Structure: the envelope parsed and its payload names itself.
    checks.push(chk(
        "structure",
        CheckStatus::Valid,
        format!("subject = {}", subject_name(&envelope.payload.subject)),
    ));

    // Canonical form: the signed message re-derives from the stored payload
    // and binding. A payload that cannot be canonicalized cannot have been
    // signed as these bytes.
    let canonical = envelope.signing_message().signing_bytes();
    checks.push(match &canonical {
        Ok(_) => chk(
            "canonical-form",
            CheckStatus::Valid,
            "the signed message re-derives from the stored envelope",
        ),
        Err(error) => chk("canonical-form", CheckStatus::Invalid, error.to_string()),
    });

    // Algorithm.
    let algorithm_ok =
        envelope.signer.signature_algorithm == draft_dcg_contract::signature::SIGNATURE_ALGORITHM;
    checks.push(chk(
        "algorithm",
        if algorithm_ok {
            CheckStatus::Valid
        } else {
            CheckStatus::Invalid
        },
        format!("algorithm = {}", envelope.signer.signature_algorithm),
    ));

    // Signature. Unresolvable key means unanswerable, not failed.
    let signature = match (canonical.is_ok(), public_key) {
        (false, _) => chk(
            "signature",
            CheckStatus::Invalid,
            "the signed bytes could not be re-derived",
        ),
        (true, None) => chk(
            "signature",
            CheckStatus::Unknown,
            format!(
                "signing key '{}' could not be resolved",
                envelope.signer.signing_key_id
            ),
        ),
        (true, Some(key)) => match signer::is_authentic(envelope, key) {
            Ok(true) => chk(
                "signature",
                CheckStatus::Valid,
                format!("valid for key {}", envelope.signer.signing_key_id),
            ),
            Ok(false) => chk(
                "signature",
                CheckStatus::Invalid,
                "the signature does not cover this payload and binding",
            ),
            Err(error) => chk("signature", CheckStatus::Unknown, error.message),
        },
    };
    checks.push(signature);

    // Historical trust: whether the key was accepted when the receipt was
    // issued. Draft does not yet retain issuance-time registry revisions for
    // every receipt, so this is reported honestly as unknown rather than
    // inferred from the current registry.
    checks.push(chk(
        "historical-trust",
        CheckStatus::Unknown,
        "issuance-time trust state is not retained for this receipt",
    ));

    // Current trust: whether the key is still accepted now.
    let is_revoked = revoked.contains(&envelope.signer.signing_key_id);
    checks.push(chk(
        "current-trust",
        if is_revoked {
            CheckStatus::Invalid
        } else if public_key.is_some() {
            CheckStatus::Valid
        } else {
            CheckStatus::Unknown
        },
        if is_revoked {
            format!("key {} is revoked", envelope.signer.signing_key_id)
        } else if public_key.is_some() {
            "the signing key is currently accepted".to_string()
        } else {
            "the signing key is not published locally".to_string()
        },
    ));

    ReceiptVerification {
        receipt_id: envelope.payload.receipt_id.to_string(),
        subject: subject_name(&envelope.payload.subject).to_string(),
        ok: checks
            .iter()
            // `historical-trust` is reported, not required: a receipt whose
            // issuance-time registry revision was never retained is not
            // thereby invalid, and failing it would make every v1 receipt
            // fail for a reason that is about Draft rather than the receipt.
            .filter(|check| check.name != "historical-trust")
            .all(|check| check.status == CheckStatus::Valid),
        checks,
    }
}

/// Verify one receipt against the project's stored envelopes and global trust.
pub fn verify_one(layout: &DraftLayout, receipt_id: &str) -> DraftResult<ReceiptVerification> {
    let receipt = draft_dcg_contract::ids::ReceiptId::parse(receipt_id).map_err(|error| {
        crate::support::error::DraftError::new(
            crate::support::error::DraftErrorKind::Validation,
            error.to_string(),
        )
    })?;
    let envelope = ReceiptEnvelopeStore::for_layout(layout)
        .get(&receipt)?
        .ok_or_else(|| {
            crate::support::error::DraftError::not_found(format!("no receipt '{receipt_id}'"))
        })?;
    let home = DraftGlobalStore::locate()?;
    let key =
        crate::trust::identity::global::resolve_public_key(&home, &envelope.signer.signing_key_id)
            .ok()
            .flatten();
    Ok(verify(&envelope, key.as_deref(), &revoked_keys(&home)?))
}

#[derive(Debug, Deserialize)]
struct RevokedKeyRegistry {
    #[serde(rename = "schema_version")]
    _schema_version: u32,
    public_key_ids: Vec<String>,
}

impl crate::contracts::VersionedContract for RevokedKeyRegistry {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::RevokedKeyRegistry;
}

pub(crate) fn revoked_keys(home: &DraftGlobalStore) -> DraftResult<Vec<String>> {
    let path = home.revoked_keys_json();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let registry: RevokedKeyRegistry = crate::contracts::read_persisted(&path)?;
    Ok(registry.public_key_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::signing::Keypair;
    use draft_dcg_contract::ids::{ActorId, PromotionId, ReceiptId};
    use draft_dcg_contract::receipt::ReceiptSignerBinding;
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::{BaselineId, Digest, ReceiptPayload};

    fn envelope(keypair: &Keypair) -> ReceiptEnvelope {
        signer::issue(
            ReceiptPayload {
                receipt_id: ReceiptId::parse("rcp_000000000001").unwrap(),
                subject: ReceiptKind::Promotion {
                    promotion: PromotionId::parse("pro_000000000001").unwrap(),
                    baseline: BaselineId::new(Digest::of_bytes(b"baseline")),
                },
                issued_by: ActorId::parse("act_000000000001").unwrap(),
                issued_at: Timestamp::from_unix_nanos(0),
            },
            ReceiptSignerBinding::new(
                ActorId::parse("act_000000000001").unwrap(),
                keypair.public_key_id(),
                "ed25519",
            )
            .unwrap(),
            keypair,
        )
        .unwrap()
    }

    fn status(verification: &ReceiptVerification, name: &str) -> CheckStatus {
        verification
            .checks
            .iter()
            .find(|check| check.name == name)
            .unwrap()
            .status
    }

    #[test]
    fn a_receipt_signed_by_a_resolvable_key_verifies() {
        let keypair = Keypair::generate();
        let envelope = envelope(&keypair);
        let verification = verify(&envelope, Some(&keypair.public_key_b64()), &[]);
        assert_eq!(status(&verification, "signature"), CheckStatus::Valid);
        assert_eq!(status(&verification, "current-trust"), CheckStatus::Valid);
        assert!(verification.ok);
    }

    #[test]
    fn an_unresolvable_key_makes_the_signature_unknown_not_invalid() {
        // "Draft cannot tell" and "the receipt is forged" are different
        // answers, and reporting the first as the second would make an
        // offline verifier look like a tamper detection.
        let keypair = Keypair::generate();
        let verification = verify(&envelope(&keypair), None, &[]);
        assert_eq!(status(&verification, "signature"), CheckStatus::Unknown);
        assert_eq!(status(&verification, "current-trust"), CheckStatus::Unknown);
        assert!(!verification.ok);
    }

    #[test]
    fn a_revoked_key_fails_current_trust_while_the_signature_still_verifies() {
        // The distinction §8.2 exists to preserve: the bytes are intact and
        // the key made them; the key is simply no longer accepted.
        let keypair = Keypair::generate();
        let envelope = envelope(&keypair);
        let verification = verify(
            &envelope,
            Some(&keypair.public_key_b64()),
            &[keypair.public_key_id()],
        );
        assert_eq!(status(&verification, "signature"), CheckStatus::Valid);
        assert_eq!(status(&verification, "current-trust"), CheckStatus::Invalid);
        assert!(!verification.ok);
    }

    #[test]
    fn a_rewritten_payload_fails_the_signature() {
        let keypair = Keypair::generate();
        let mut tampered = envelope(&keypair);
        tampered.payload.subject = ReceiptKind::Promotion {
            promotion: PromotionId::parse("pro_000000000002").unwrap(),
            baseline: BaselineId::new(Digest::of_bytes(b"another baseline")),
        };
        let verification = verify(&tampered, Some(&keypair.public_key_b64()), &[]);
        assert_eq!(status(&verification, "signature"), CheckStatus::Invalid);
        assert!(!verification.ok);
    }

    #[test]
    fn historical_trust_is_reported_unknown_rather_than_assumed() {
        let keypair = Keypair::generate();
        let verification = verify(&envelope(&keypair), Some(&keypair.public_key_b64()), &[]);
        assert_eq!(
            status(&verification, "historical-trust"),
            CheckStatus::Unknown
        );
    }
}
