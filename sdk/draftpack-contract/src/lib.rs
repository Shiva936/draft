//! The portable DraftPack interchange format.
//!
//! A DraftPack carries an accepted Baseline — its state, the provenance that
//! establishes it, its coverage evidence and its receipts — out of one Draft
//! installation so another can **verify** it. This crate is the format
//! contract: what a valid pack declares, how its members are named and
//! digested, what limits apply, and how the whole thing is signed and checked.
//!
//! # What lives here, and what does not
//!
//! This crate is pure format. It contains no tar reader, no filesystem access,
//! no extraction, and no import policy — those are Draft's, in `core::draftpack`.
//! The split matters because import is a **security boundary**: keeping the
//! grammar and the limits in a portable crate means an exporter validates
//! against exactly the rules an importer will apply, so an archive cannot be
//! produced that the recipient must refuse.
//!
//! # The verification a recipient can actually perform
//!
//! 1. Check the [`DraftpackEnvelope`]'s signature over the manifest and the
//!    signer binding together.
//! 2. Check every member's bytes against the manifest's per-entry digests, and
//!    detect members that were added or removed rather than merely altered.
//! 3. Recompute the Baseline's roots from the carried DCG facts, using
//!    `draft-dcg-contract` alone.
//!
//! What a recipient cannot conclude from any of that is that the pack should be
//! **trusted**. A valid signature says these bytes came from that key
//! unmodified; whether that key is trusted here, and whether the Baseline
//! should be adopted, are the importer's decisions.

#![forbid(unsafe_code)]

pub mod envelope;
pub mod limits;
pub mod manifest;
pub mod path;

pub use envelope::{DraftpackEnvelope, DraftpackSigningMessage, DRAFTPACK_SIGNATURE_DOMAIN};
pub use limits::{MAX_ENTRIES, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES};
pub use manifest::{
    ArchiveEntryMetadata, DraftpackManifest, DraftpackManifestDigest, VerificationReport,
    DRAFTPACK_MEDIA_TYPE, MANIFEST_ENTRY_PATH,
};
pub use path::{SafeEntryPath, MAX_ENTRY_PATH_LENGTH};

/// The DraftPack format revision this crate implements.
pub const DRAFTPACK_FORMAT_REVISION: u32 = 1;

/// Why a DraftPack document or archive is not acceptable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// Identity, naming or format-marker rules were violated.
    Identity(String),
    /// A member name is not a safe relative path.
    ///
    /// Kept distinct from [`FormatError::Limit`] because Draft treats an
    /// attempt to escape the extraction root as a protected-access refusal,
    /// while an oversized archive is a resource decision.
    UnsafePath(String),
    /// A declared format limit was exceeded.
    Limit(String),
    /// A document could not be encoded or decoded.
    Encoding(String),
    /// Signature or key-material verification failed.
    Signature(String),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identity(detail) => write!(formatter, "identity: {detail}"),
            Self::UnsafePath(detail) => write!(formatter, "unsafe path: {detail}"),
            Self::Limit(detail) => write!(formatter, "limit: {detail}"),
            Self::Encoding(detail) => write!(formatter, "encoding: {detail}"),
            Self::Signature(detail) => write!(formatter, "signature: {detail}"),
        }
    }
}

impl std::error::Error for FormatError {}

/// Map a portable DCG contract failure onto this crate's taxonomy.
///
/// Only the reused primitives can raise one here, so the mapping is narrow:
/// identity and encoding mean the same in both crates, and both an integrity
/// mismatch and a signature failure mean the bytes are not what something
/// attested them to be.
impl From<draft_dcg_contract::FormatError> for FormatError {
    fn from(error: draft_dcg_contract::FormatError) -> Self {
        use draft_dcg_contract::FormatError as Portable;
        match error {
            Portable::Identity(detail) | Portable::Consistency(detail) => Self::Identity(detail),
            Portable::Encoding(detail) => Self::Encoding(detail),
            Portable::Integrity(detail) | Portable::Signature(detail) => Self::Signature(detail),
        }
    }
}

pub type FormatResult<T> = Result<T, FormatError>;
