//! The generic representation engines Draft ships.
//!
//! An engine is Draft's own code doing something domain-neutral with data an
//! extension configured. That is the whole distinction: `sequence_alignment`
//! knows tokens, not lines; byte delimiters, not text; alignment, not diffs.
//! The extension that says "split on `0x0A` and call the coordinate space
//! `draft.text.document/line`" owns every one of those decisions, and Core never
//! compares a coordinate space or a key space against a literal.
//!
//! Each engine carries its own revision, so a change to how an engine aligns
//! or projects is visible in the identity of every artifact it produced.

pub mod alignment;
pub mod attribute;
pub mod keyed;
pub mod whole;

use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use draft_extension_contract::EngineId;

/// The implementation revision of each engine.
///
/// A contribution authored against an older revision is refused rather than run
/// under semantics it did not agree to.
pub const fn engine_revision(engine: EngineId) -> u32 {
    match engine {
        EngineId::WholeResource => whole::REVISION,
        EngineId::SequenceAlignment => alignment::REVISION,
        EngineId::KeyedRecordSet => keyed::REVISION,
        EngineId::AttributeProjection => attribute::REVISION,
        EngineId::ResourceEnumeration => ENUMERATION_REVISION,
    }
}

/// Resource enumeration is performed by an adapter rather than by a shared
/// engine body, so its revision lives here alongside the others.
pub const ENUMERATION_REVISION: u32 = 1;

/// What an engine was given to work on.
///
/// Bytes are optional because most engines need none: whole-resource comparison
/// works on digests alone, and attribute projection works on typed attributes.
#[derive(Debug, Clone, Default)]
pub struct EngineInput {
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}

/// One aligned or keyed unit an engine produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineUnit {
    /// Stable within the representation, minted by the engine.
    pub unit_id: String,
    /// The claim this unit makes on the resource.
    pub scope: crate::dcg::representation::ConflictScope,
    /// A short, contributed-space description. Never interpreted by Core.
    pub label: String,
}

/// What an engine produced for one resource.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineOutput {
    pub units: Vec<EngineUnit>,
    pub claims: Vec<crate::dcg::representation::ConflictClaim>,
    /// Contributed metric keys. Core makes none of them mean anything.
    pub metrics: std::collections::BTreeMap<String, i64>,
}

/// Refuse an engine invocation whose contribution was authored against a
/// different implementation revision.
pub fn check_revision(engine: EngineId, declared: u32) -> DraftResult<()> {
    let current = engine_revision(engine);
    if declared == current {
        return Ok(());
    }
    Err(DraftError::new(
        DraftErrorKind::UnsupportedSchema,
        format!(
            "engine '{}' is at revision {current}; this contribution declares {declared}",
            engine.as_str()
        ),
    ))
}
