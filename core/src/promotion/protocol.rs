//! The promotion protocol: journal, barrier, coverage, commit, finalize.
//!
//! This is the state machine. `app/` decides *whether* a promotion is
//! authorized and supplies the inputs; everything about *how* it executes
//! durably — what is written before what, what a restart concludes, when the
//! Baseline may be created — lives here.
//!
//! ```text
//! classify restart  ──▶ resume, or start fresh
//!         ↓
//! barrier            an earlier promotion may still owe its completion
//!         ↓
//! coverage           absence must be proved before it is accepted
//!         ↓
//! journal: Prepared  the intent, durable, with preallocated ids
//!         ↓
//! COMMIT             the Baseline is created — the authority point
//!         ↓
//! journal: Committed
//!         ↓
//! finalize           receipt, events, ChangePack completion
//!         ↓
//! journal: Finalized
//! ```
//!
//! # Why the journal is written before the Baseline
//!
//! A crash between them leaves a `Prepared` journal and no Baseline, which
//! recovery reads as "did not commit" and can safely abandon or retry. The
//! reverse order would leave an accepted Baseline that nothing describes: no
//! record of which revision it accepted, which receipt it owes, or which
//! events it still has to append.
//!
//! # Why the commit point is the Baseline, not the journal mark
//!
//! `Committed` is a *description* of what happened, written after the fact.
//! The moment the project's authority actually changes is when the Baseline
//! becomes what the control record names. So recovery never trusts the mark
//! alone — it compares the control state against the journal's exact expected
//! and planned values, which is what [`crate::promotion::journal::resolve`]
//! decides.

use draft_dcg_contract::baseline::BaselineId;
use draft_dcg_contract::coverage::CoverageEvidence;
use draft_dcg_contract::ids::PromotionId;
use draft_dcg_contract::Digest;

use crate::promotion::barrier::{enforce, BarrierOutcome};
use crate::promotion::coverage::shortfalls;
use crate::promotion::journal::{
    resolve, ChangePackMatch, ControlMatch, PromotionJournalState, PromotionResolution,
};
use crate::promotion::record::{PromotionJournal, PromotionRecord, PromotionRecordStore};
use crate::promotion::store::PromotionJournalStore;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// What the protocol needs from its caller at each durable step.
///
/// A trait rather than concrete stores, so the protocol owns the ordering and
/// the application owns what each step means. A caller cannot reorder the
/// steps — it only supplies their content.
pub trait PromotionEffects {
    /// The digest of the project's control state as it is now.
    fn control_digest(&self) -> DraftResult<Digest>;

    /// Whether the ChangePack is still in the state the journal expected.
    fn change_match(&self, journal: &PromotionJournal) -> DraftResult<ChangePackMatch>;

    /// Create the Baseline. **The authority point.**
    ///
    /// Called exactly once per promotion, between `Prepared` and `Committed`.
    fn commit_baseline(&self, journal: &PromotionJournal) -> DraftResult<BaselineId>;

    /// Complete the ChangePack this promotion accepted.
    fn complete_change(&self, journal: &PromotionJournal) -> DraftResult<()>;

    /// Issue the receipt and append the preallocated events. Idempotent.
    fn finalize(&self, journal: &PromotionJournal, baseline: &BaselineId) -> DraftResult<()>;
}

/// What the protocol did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionProgress {
    /// The protocol ran to completion in this call.
    Promoted {
        promotion: PromotionId,
        baseline: BaselineId,
    },
    /// A previous run had already committed and finalized this promotion.
    AlreadyFinalized {
        promotion: PromotionId,
        baseline: BaselineId,
    },
    /// A previous run committed; this call finished what it owed.
    ResumedAndFinalized {
        promotion: PromotionId,
        baseline: BaselineId,
    },
}

impl PromotionProgress {
    pub fn baseline(&self) -> &BaselineId {
        match self {
            Self::Promoted { baseline, .. }
            | Self::AlreadyFinalized { baseline, .. }
            | Self::ResumedAndFinalized { baseline, .. } => baseline,
        }
    }

    pub fn promotion(&self) -> &PromotionId {
        match self {
            Self::Promoted { promotion, .. }
            | Self::AlreadyFinalized { promotion, .. }
            | Self::ResumedAndFinalized { promotion, .. } => promotion,
        }
    }
}

/// The stores the protocol drives.
pub struct PromotionStores {
    pub journals: PromotionJournalStore,
    pub records: PromotionRecordStore,
}

impl PromotionStores {
    pub fn for_layout(layout: &crate::project::layout::DraftLayout) -> Self {
        Self {
            journals: PromotionJournalStore::new(layout.promotion_journals_dir()),
            records: PromotionRecordStore::new(layout.promotion_records_dir()),
        }
    }
}

/// Refuse the promotion unless every required coverage domain is established.
///
/// Run before the journal is written, because a promotion that cannot prove
/// what it did not observe should never become a durable intent at all.
pub fn require_coverage(
    evidence: &[CoverageEvidence],
    required: &std::collections::BTreeSet<String>,
) -> DraftResult<()> {
    let missing = shortfalls(evidence, required);
    if missing.is_empty() {
        return Ok(());
    }
    let described: Vec<String> = missing
        .iter()
        .map(|shortfall| format!("{} ({shortfall:?})", shortfall.domain()))
        .collect();
    Err(DraftError::new(
        DraftErrorKind::CoverageIncomplete,
        format!(
            "promotion requires coverage of {}, which was not established; absence has to be \
             proved, not assumed",
            described.join(", ")
        ),
    )
    .with_suggestion(
        "observe the missing domains, or fix the binding that failed to enumerate them",
    ))
}

/// Run the promotion protocol, starting or resuming as the records dictate.
pub fn execute(
    stores: &PromotionStores,
    intent: &PromotionJournal,
    effects: &dyn PromotionEffects,
) -> DraftResult<PromotionProgress> {
    let promotion = intent.promotion.clone();

    // What, if anything, a previous run left behind. Classified from the two
    // authoritative records rather than from the journal mark alone.
    let existing = stores.journals.read_unlocked(&promotion)?;
    if let Some(record) = existing {
        return resume(stores, &record.journal, effects);
    }

    // A promotion that cannot be described durably must not begin.
    intent.validate()?;

    // The barrier: an earlier promotion that committed but never completed its
    // ChangePack still owes that work, and starting a new one over the top would
    // let the same work be accepted twice.
    require_barrier_clear(stores, effects)?;

    stores
        .journals
        .with_locked(&promotion, |guard| guard.open(intent))?;

    commit_and_finalize(
        stores,
        intent,
        effects,
        PromotionProgress::Promoted {
            promotion: promotion.clone(),
            baseline: intent.baseline.clone(),
        },
    )
}

/// Resume a promotion a previous run left unfinished.
fn resume(
    stores: &PromotionStores,
    journal: &PromotionJournal,
    effects: &dyn PromotionEffects,
) -> DraftResult<PromotionProgress> {
    let control = classify_control(journal, effects)?;
    let change = effects.change_match(journal)?;

    match resolve(journal.state, control, change) {
        // The commit never landed. The journal is an intent nothing acted on,
        // so the protocol may run it from the commit point.
        PromotionResolution::DidNotCommit => commit_and_finalize(
            stores,
            journal,
            effects,
            PromotionProgress::Promoted {
                promotion: journal.promotion.clone(),
                baseline: journal.baseline.clone(),
            },
        ),
        // Committed, ChangePack still open: completing it is mandatory. The
        // project already accepted this work; leaving the ChangePack open would
        // let it be revised and promoted again.
        PromotionResolution::CompleteChangePackThenFinalize => {
            effects.complete_change(journal)?;
            finalize_from_committed(stores, journal, effects)
        }
        // Committed and completed; only idempotent finalization remains.
        PromotionResolution::ContinueFinalization => {
            finalize_from_committed(stores, journal, effects)
        }
        PromotionResolution::AlreadyFinalized => Ok(PromotionProgress::AlreadyFinalized {
            promotion: journal.promotion.clone(),
            baseline: journal.baseline.clone(),
        }),
        PromotionResolution::Inconsistent { detail } => Err(DraftError::new(
            DraftErrorKind::CorruptData,
            format!(
                "promotion '{}' cannot be resolved: {detail}",
                journal.promotion
            ),
        )
        .with_suggestion(
            "Run `draft doctor`; Draft will not guess whether a promotion committed.",
        )),
    }
}

/// Commit the Baseline, then finish everything the promotion owes.
fn commit_and_finalize(
    stores: &PromotionStores,
    journal: &PromotionJournal,
    effects: &dyn PromotionEffects,
    progress: PromotionProgress,
) -> DraftResult<PromotionProgress> {
    // The authority point.
    let baseline = effects.commit_baseline(journal)?;

    // The journal now records what was accepted, not what was intended: a
    // resumed promotion reads its result from here.
    let committed = stores
        .journals
        .with_locked(&journal.promotion, |guard| guard.commit(&baseline))?;

    effects.complete_change(&committed.journal)?;
    finish(stores, &committed.journal, effects, &baseline)?;

    Ok(match progress {
        PromotionProgress::Promoted { promotion, .. } => PromotionProgress::Promoted {
            promotion,
            baseline,
        },
        other => other,
    })
}

fn finalize_from_committed(
    stores: &PromotionStores,
    journal: &PromotionJournal,
    effects: &dyn PromotionEffects,
) -> DraftResult<PromotionProgress> {
    finish(stores, journal, effects, &journal.baseline)?;
    Ok(PromotionProgress::ResumedAndFinalized {
        promotion: journal.promotion.clone(),
        baseline: journal.baseline.clone(),
    })
}

/// Receipt, events, immutable record, then the terminal journal mark.
///
/// The record is written before `Finalized` so a crash between them leaves a
/// promotion that recovery finalizes idempotently, rather than a `Finalized`
/// journal with no historical record of what it did.
fn finish(
    stores: &PromotionStores,
    journal: &PromotionJournal,
    effects: &dyn PromotionEffects,
    baseline: &BaselineId,
) -> DraftResult<()> {
    effects.finalize(journal, baseline)?;
    stores.records.put(&PromotionRecord {
        promotion: journal.promotion.clone(),
        revision_pack: journal.revision_pack.clone(),
        change_pack: journal.change_pack.clone(),
        baseline: baseline.clone(),
        receipt: journal.receipt.clone(),
    })?;
    stores.journals.with_locked(&journal.promotion, |guard| {
        guard.advance(
            PromotionJournalState::Committed,
            PromotionJournalState::Finalized,
        )
    })?;
    Ok(())
}

/// How the current control state compares to what the journal recorded.
fn classify_control(
    journal: &PromotionJournal,
    effects: &dyn PromotionEffects,
) -> DraftResult<ControlMatch> {
    let current = effects.control_digest()?;
    Ok(ControlMatch::classify(
        &current,
        &journal.expected_control,
        &journal.planned_control,
    ))
}

/// Refuse a new promotion while an earlier one still owes its completion.
fn require_barrier_clear(
    stores: &PromotionStores,
    effects: &dyn PromotionEffects,
) -> DraftResult<()> {
    let Some(outstanding) = most_recent_unfinished(stores)? else {
        return Ok(());
    };
    let control = classify_control(&outstanding, effects)?;
    let change = effects.change_match(&outstanding)?;
    match enforce(outstanding.state, control, change) {
        BarrierOutcome::Clear => Ok(()),
        other => other.require_clear(),
    }
}

/// The most recent promotion journal that has not reached `Finalized`.
fn most_recent_unfinished(stores: &PromotionStores) -> DraftResult<Option<PromotionJournal>> {
    for promotion in stores.journals.list()? {
        if let Some(record) = stores.journals.read_unlocked(&promotion)? {
            if record.journal.state != PromotionJournalState::Finalized {
                return Ok(Some(record.journal));
            }
        }
    }
    Ok(None)
}
