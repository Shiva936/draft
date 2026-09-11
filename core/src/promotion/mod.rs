//! Promotion: accepting a Change's work into the project's Baseline.
//!
//! # Why this is journalled
//!
//! A promotion is several durable effects that must agree: the control state
//! advances to a new accepted Baseline, the Change becomes `Completed`, a
//! receipt is issued, and Activity records it. A crash anywhere in the middle
//! leaves a question that the surviving state alone cannot answer — and unlike
//! most such questions, guessing wrong is unrecoverable in one direction.
//!
//! **The accepted Baseline is never rolled back.** If a promotion committed,
//! finishing it is the only correct resolution; abandoning it would discard an
//! acceptance the project already made. If it did not commit, completing it
//! would accept work nobody approved. So the journal records the intent *and
//! the planned Change completion* before anything moves, and recovery reads
//! the answer off the two authoritative records rather than inferring it.
//!
//! # Why two records are consulted
//!
//! The commit point advances `ProjectControlState`; the Change completion
//! follows under the Change's own lock. Between them a crash leaves the
//! control state moved and the Change still `Active` — a real, legal,
//! recoverable state. Reading only one record could not distinguish it from a
//! promotion that never committed at all.
//!
//! [`resolve`] is the whole of §2.34's restart table, written as one total
//! function so every combination has a decided answer rather than a default.

pub mod barrier;
pub mod coverage;
pub mod journal;
pub mod protocol;
pub mod record;
pub mod store;

pub use barrier::{enforce, BarrierOutcome};
pub use coverage::{satisfies, shortfalls, CoverageShortfall};
pub use protocol::{
    execute, require_coverage, PromotionEffects, PromotionProgress, PromotionStores,
};
pub use record::{PromotionJournal, PromotionRecord, PromotionRecordStore};
pub use store::{PromotionJournalGuard, PromotionJournalRecord, PromotionJournalStore};

pub use journal::{
    classify, resolve, ChangeMatch, ControlMatch, PromotionJournalState, PromotionResolution,
};
