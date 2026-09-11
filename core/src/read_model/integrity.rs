//! Does the durable history still hold together?
//!
//! Three independent structures answer that, and this folds them into one
//! report without collapsing them into one answer: the Activity chain, the
//! signed receipts, and the transparency chain the receipts are entered in.
//! A reader has to be able to see *which* of the three failed, because they
//! fail for different reasons and are repaired in different ways.
//!
//! This lives in `read_model` rather than in `receipt` because it reads across
//! ownership boundaries — Activity, receipts and trust — and none of those
//! owns the combined view. It only reads: Doctor calls it, and Doctor never
//! rewrites history.

use serde::{Deserialize, Serialize};

use crate::activity::ActivityLog;
use crate::project::home::DraftGlobalStore;
use crate::project::layout::DraftLayout;
use crate::receipt::{ReceiptEnvelopeStore, ReceiptVerification};
use crate::support::error::DraftResult;

/// What verifying a whole project's durable history established.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerVerification {
    pub activity_chain_ok: bool,
    pub activity_count: usize,
    pub transparency_ok: bool,
    pub transparency_count: usize,
    pub receipts: Vec<ReceiptVerification>,
    pub all_ok: bool,
}

/// Verify the Activity chain, every stored receipt, and the transparency chain.
pub fn verify_all(
    layout: &DraftLayout,
    project: &draft_dcg_contract::ids::ProjectId,
) -> DraftResult<LedgerVerification> {
    let log = ActivityLog::new(layout.events_dir(), project.to_string());
    let (activity_chain_ok, activity_count) = match log.verify_chain() {
        Ok(count) => (true, count),
        Err(_) => (false, log.read_all().map(|records| records.len())?),
    };

    let home = DraftGlobalStore::locate()?;
    let revoked = crate::receipt::revoked_keys(&home)?;
    let mut receipts = Vec::new();
    let (intact, damaged) = ReceiptEnvelopeStore::for_layout(layout).read_all_reporting()?;
    for envelope in intact {
        let key = crate::trust::identity::global::resolve_public_key(
            &home,
            &envelope.signer.signing_key_id,
        )
        .ok()
        .flatten();
        receipts.push(crate::receipt::verify(&envelope, key.as_deref(), &revoked));
    }
    // A receipt whose stored bytes no longer match the digest they were bound
    // to is reported by name rather than dropped from the count. Silently
    // verifying fewer receipts than the project holds would make damage look
    // like absence.
    for (id, error) in damaged {
        receipts.push(crate::receipt::unreadable(&id, &error.message));
    }

    let chain = crate::trust::transparency::TransparencyLog::new(layout.clone());
    let (transparency_ok, transparency_count) = match chain.verify(|entry| {
        entry.public_key_id.as_deref().and_then(|key_id| {
            crate::trust::identity::global::resolve_public_key(&home, key_id)
                .ok()
                .flatten()
        })
    }) {
        Ok(count) => (true, count),
        Err(_) => (false, chain.read_all().map(|entries| entries.len())?),
    };

    let all_ok = activity_chain_ok && transparency_ok && receipts.iter().all(|receipt| receipt.ok);
    Ok(LedgerVerification {
        activity_chain_ok,
        activity_count,
        transparency_ok,
        transparency_count,
        receipts,
        all_ok,
    })
}
