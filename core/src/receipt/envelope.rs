//! Create-once storage for signed receipt envelopes.
//!
//! A receipt is an immutable fact: it attests that one exact thing happened,
//! under one exact identity, at one exact moment. So it is stored the way
//! every other immutable fact is — bound create-once to the digest of its own
//! canonical bytes, and refused rather than rewritten if a second write
//! carries different content under the same `rcp_` (§2.45).
//!
//! # Why the id is preallocated by the caller
//!
//! Promotion and Publication both choose the receipt id *before* the commit
//! they will attest. That is what makes finalization idempotent: a recovery
//! that replays finalization writes the same envelope under the same id and
//! converges, instead of minting a second receipt claiming to be the one.

use draft_dcg_contract::ids::ReceiptId;
use draft_dcg_contract::receipt::ReceiptEnvelope;

use crate::support::error::DraftResult;
use crate::support::immutable_store::{ImmutableFactStore, StoreOutcome};

/// The intact envelopes, and every one that could not be read.
///
/// Named rather than returned as a bare tuple: the second half is the whole
/// point of the call, and a caller who ignored it would silently report fewer
/// receipts than the project holds.
pub type ReadableReceipts = (
    Vec<ReceiptEnvelope>,
    Vec<(String, crate::support::error::DraftError)>,
);

/// Signed receipt envelopes, one per `rcp_` id.
#[derive(Debug, Clone)]
pub struct ReceiptEnvelopeStore {
    facts: ImmutableFactStore<ReceiptEnvelope>,
}

impl ReceiptEnvelopeStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            facts: ImmutableFactStore::new(directory),
        }
    }

    pub fn for_layout(layout: &crate::project::layout::DraftLayout) -> Self {
        Self::new(layout.receipt_envelopes_dir())
    }

    /// Store `envelope` under its own payload's receipt id.
    ///
    /// The id is taken from the signed payload rather than from a parameter,
    /// so an envelope can never be filed under an id its signature does not
    /// cover.
    pub fn put(&self, envelope: &ReceiptEnvelope) -> DraftResult<StoreOutcome> {
        self.facts
            .put(envelope.payload.receipt_id.as_str(), envelope)
    }

    pub fn get(&self, receipt: &ReceiptId) -> DraftResult<Option<ReceiptEnvelope>> {
        self.facts.get(receipt.as_str())
    }

    pub fn list_ids(&self) -> DraftResult<Vec<String>> {
        self.facts.list_ids()
    }

    /// Every stored envelope, in id order.
    ///
    /// Hard-fails on the first receipt whose stored bytes no longer match the
    /// digest they were bound to. Anything *consuming* a receipt must, because
    /// a substituted attestation is not a weaker attestation.
    pub fn read_all(&self) -> DraftResult<Vec<ReceiptEnvelope>> {
        let (envelopes, damaged) = self.read_all_reporting()?;
        if let Some((id, error)) = damaged.into_iter().next() {
            return Err(error.with_context(format!("receipt '{id}'")));
        }
        Ok(envelopes)
    }

    /// Every stored envelope, and every one that could not be read.
    ///
    /// What Doctor calls. A diagnostic that aborted on the first damaged
    /// receipt would be unable to say how many others are intact, which is
    /// exactly the question somebody asks after finding one — so the damage is
    /// *reported*, not swallowed, and never repaired.
    pub fn read_all_reporting(&self) -> DraftResult<ReadableReceipts> {
        let mut envelopes = Vec::new();
        let mut damaged = Vec::new();
        for id in self.facts.list_ids()? {
            match self.facts.get(&id) {
                Ok(Some(envelope)) => envelopes.push(envelope),
                Ok(None) => {}
                Err(error) => damaged.push((id, error)),
            }
        }
        Ok((envelopes, damaged))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::signing::Keypair;
    use draft_dcg_contract::ids::{ActorId, PromotionId};
    use draft_dcg_contract::receipt::{ReceiptKind, ReceiptSignerBinding};
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::{BaselineId, Digest, ReceiptPayload};

    fn payload(receipt: &str, baseline: &[u8]) -> ReceiptPayload {
        ReceiptPayload {
            receipt_id: draft_dcg_contract::ids::ReceiptId::parse(receipt).unwrap(),
            subject: ReceiptKind::Promotion {
                promotion: PromotionId::parse("pro_000000000001").unwrap(),
                baseline: BaselineId::new(Digest::of_bytes(baseline)),
            },
            issued_by: ActorId::parse("act_000000000001").unwrap(),
            issued_at: Timestamp::from_unix_nanos(0),
        }
    }

    fn binding() -> ReceiptSignerBinding {
        ReceiptSignerBinding::new(
            ActorId::parse("act_000000000001").unwrap(),
            "key-a",
            "ed25519",
        )
        .unwrap()
    }

    #[test]
    fn an_identical_reissue_converges() {
        // The crash case: finalization committed, then died before recording
        // that it had. Replaying it must be safe.
        let directory = tempfile::tempdir().unwrap();
        let store = ReceiptEnvelopeStore::new(directory.path());
        let keypair = Keypair::generate();
        let envelope = crate::receipt::signer::issue(
            payload("rcp_000000000001", b"baseline"),
            binding(),
            &keypair,
        )
        .unwrap();

        assert_eq!(store.put(&envelope).unwrap(), StoreOutcome::Created);
        assert_eq!(
            store.put(&envelope).unwrap(),
            StoreOutcome::AlreadyIdentical
        );
        let loaded = store
            .get(&draft_dcg_contract::ids::ReceiptId::parse("rcp_000000000001").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(loaded, envelope);
    }

    #[test]
    fn a_second_receipt_under_the_same_id_is_refused() {
        // Two different attestations cannot both be "the" receipt for one id;
        // that is exactly the ambiguity preallocation exists to remove.
        let directory = tempfile::tempdir().unwrap();
        let store = ReceiptEnvelopeStore::new(directory.path());
        let keypair = Keypair::generate();
        store
            .put(
                &crate::receipt::signer::issue(
                    payload("rcp_000000000001", b"baseline"),
                    binding(),
                    &keypair,
                )
                .unwrap(),
            )
            .unwrap();

        let conflicting = crate::receipt::signer::issue(
            payload("rcp_000000000001", b"another baseline"),
            binding(),
            &keypair,
        )
        .unwrap();
        assert!(store.put(&conflicting).is_err());
    }
}
