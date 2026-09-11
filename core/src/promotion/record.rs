//! The prepared promotion journal, and the immutable record it becomes.

use draft_dcg_contract::ids::{ChangeId, ChangeRevisionId, PromotionId, ReceiptId};
use draft_dcg_contract::receipt::ReceiptSignerBinding;
use draft_dcg_contract::BaselineId;
use serde::{Deserialize, Serialize};

use crate::promotion::journal::PromotionJournalState;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use crate::support::immutable_store::ImmutableFactStore;
use draft_dcg_contract::Digest;

/// Everything a promotion intends to do, written down before anything moves.
///
/// # Why the ids are preallocated
///
/// The receipt id and the Activity event ids are chosen *before* the commit,
/// not after. A crash between committing and issuing the receipt would
/// otherwise mean recovery had to mint new ids — and a replayed recovery would
/// mint different ones again, so the same promotion could accumulate several
/// receipts each claiming to be the one.
///
/// Preallocating makes finalization idempotent: replaying it writes the same
/// receipt under the same id and appends the same event, so "did this already
/// happen?" has an answer that does not depend on when you ask.
///
/// # Why the planned Change completion is in here
///
/// Recovery needs to know what `Completed` was *going to* look like, not just
/// that completion was intended. Without the planned value it could not tell
/// a Change this promotion completed from one completed by something else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionJournal {
    pub promotion: PromotionId,
    /// The revision being promoted.
    pub revision: ChangeRevisionId,
    /// The Change that revision belongs to.
    pub change: ChangeId,
    /// The Baseline this promotion accepts.
    pub baseline: BaselineId,
    /// The receipt this promotion will issue, chosen in advance.
    pub receipt: ReceiptId,
    /// Who will sign it.
    pub signer: ReceiptSignerBinding,
    /// The Activity events this promotion will append, chosen in advance.
    pub activity_event_ids: Vec<String>,
    /// When this promotion was prepared.
    ///
    /// Frozen here rather than read at finalization, so a recovery that
    /// replays finalization writes byte-identical events and the idempotent
    /// append converges instead of refusing a changed payload.
    pub prepared_at: draft_dcg_contract::value::Timestamp,
    /// The control state digest expected before the commit.
    pub expected_control: Digest,
    /// The control state digest the commit will produce.
    pub planned_control: Digest,
    /// The Change value the completion will produce.
    pub planned_change: Digest,
    pub state: PromotionJournalState,
}

impl PromotionJournal {
    pub fn validate(&self) -> DraftResult<()> {
        if self.expected_control == self.planned_control {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "a promotion whose planned control state equals the expected one accepts \
                 nothing; recovery could never tell whether it committed",
            ));
        }
        if self.activity_event_ids.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "a promotion must preallocate the events it will append; minting them after the \
                 commit would let a replayed recovery append a second set",
            ));
        }
        Ok(())
    }
}

/// The immutable historical fact that a promotion happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionRecord {
    pub promotion: PromotionId,
    pub revision: ChangeRevisionId,
    pub change: ChangeId,
    pub baseline: BaselineId,
    pub receipt: ReceiptId,
}

impl PromotionRecord {
    pub fn digest(&self) -> DraftResult<Digest> {
        Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
    }
}

/// Create-once storage for promotion records.
pub struct PromotionRecordStore {
    facts: ImmutableFactStore<PromotionRecord>,
}

impl PromotionRecordStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            facts: ImmutableFactStore::new(directory),
        }
    }

    pub fn put(&self, record: &PromotionRecord) -> DraftResult<()> {
        self.facts.put(record.promotion.as_str(), record)?;
        Ok(())
    }

    pub fn get(&self, id: &PromotionId) -> DraftResult<Option<PromotionRecord>> {
        self.facts.get(id.as_str())
    }
}
