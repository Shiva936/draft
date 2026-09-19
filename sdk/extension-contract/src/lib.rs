//! The portable Draft extension contract.
//!
//! This crate is the public boundary between Draft and the extensions it
//! installs. It describes how a declarative extension package and a signed
//! extension catalog are shaped, canonicalized, signed and verified — and
//! nothing else. Extension *authors* and *publishers* need exactly this crate;
//! they never need Draft's runtime, services, stores or daemon.
//!
//! The membership rule is deliberate: if the tooling that authors, validates,
//! packages, signs, verifies or publishes an extension artifact does not need a
//! type, that type does not belong here. Installed state, provenance,
//! authorization grants and capability resolution are Draft-operational
//! concepts and live in `draft-core` instead.
//!
//! Nothing here executes anything. A package is data; Draft decides what, if
//! anything, to do with it — and even a declared command is inert until Draft
//! holds an authorization for the exact installed artifact.
//!
//! The vocabulary is domain-neutral. Nothing in this crate names a file
//! extension, a programming language, a toolchain or a diff: a domain
//! contributes namespaced identifiers, predicates over intrinsic resource facts,
//! and schema-bound operations, and Draft stores and compares those without
//! interpreting them.

/// Canonical JSON, re-exported from the portable DCG contract.
///
/// Kept as a module path so publisher tooling that already imports
/// `draft_extension_contract::canonical` keeps working, while the one
/// implementation lives in `draft-dcg-contract`. Catalog signatures and DCG
/// digests are taken over the same bytes precisely because there is only one
/// canonicalizer.
pub mod canonical {
    pub use draft_dcg_contract::canonical::{canonical_bytes, canonical_json};
}
pub mod catalog;
pub mod compatibility;
pub mod contribution;
pub mod envelope;
/// The identifier grammar, re-exported from the portable DCG contract.
///
/// Extension publishers and the DCG share one set of naming rules, so a
/// namespaced identifier minted in a package manifest and one recorded in a
/// canonical DCG fact are the same value with the same canonical form.
pub mod identifier {
    pub use draft_dcg_contract::identifier::{
        validate_segment, IdentifierClass, NamespacedId, ScopedId, MAX_QUALIFIED_LENGTH,
        MAX_SEGMENT_LENGTH,
    };
}
pub mod identity;
pub mod manifest;
pub mod package;
pub mod schema;

pub use canonical::canonical_json;
pub use catalog::{
    CatalogKey, CatalogTarget, Delegation, MetadataDescriptor, RoleSpec, RootMetadata,
    SignatureRecord, SignedEnvelope, SnapshotMetadata, TargetsMetadata, TimestampMetadata,
};
pub use compatibility::draft_api_compatible;
pub use contribution::{
    AdapterCapabilities, AttributeMatch, AttributeValue, CandidateExecution, CandidateLimits,
    CandidatePreset, ChangeAspectName, ChangeMetricBudget, CheckRequirement, CheckSelection,
    ClassAttribute, ComparisonContribution, ContributionPayload, ControlPolicy,
    ElementExtractionContribution, ElementPredicate, EngineId, Executor, ExtensionPermission,
    IntentDeclaration, IntentVocabulary, MechanismOperation, ObservationConsistency, OutputScope,
    PolicyPreset, PresentationBinding, PresentationContribution, PresentationEngineId,
    PresentationSurface, RawResourcePredicate, RecoveryContribution, RelationDirection,
    ResourceAdapterContribution, ResourceClassificationRule, ResourceForm, ResourcePredicate,
    ResourceRule, ReviewabilityBudget, RiskCondition, RiskRule, RiskRuleSet, RiskThresholds,
    StructuredCommand, TaskTemplateContribution, TaskTemplateStep, ToolActionContribution,
    ToolEffect, VerificationCheck, VerificationContribution, VerificationEscalation,
    VerificationStateName, ViewRulePolicy,
};
pub use draft_dcg_contract::verify_signature;
pub use envelope::{ContributionEnvelope, CONTRIBUTION_DIGEST_DOMAIN};
pub use identifier::{IdentifierClass, NamespacedId, ScopedId};
pub use identity::PackageIdentity;
pub use manifest::{
    ExtensionContribution, ExtensionContributionKind, ExtensionId, ExtensionManifest,
};
pub use schema::{PackageSchema, SchemaRef};

/// The package and catalog format revision this crate implements.
///
/// Draft owns schema versioning through its closed contract registry; this
/// constant exists so authoring tooling outside the Draft repository can write
/// a correct `schema_version` without importing that registry. `draft-core`
/// asserts the two agree, so they cannot drift.
pub const FORMAT_REVISION: u32 = 1;

/// Why a package, manifest or catalog document is not acceptable.
///
/// Draft maps each variant onto its own error taxonomy; keeping the mapping
/// explicit is what lets this crate stay free of Draft's error type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// Identity, naming or schema-marker rules were violated.
    Identity(String),
    /// The package does not accept the Draft API version offered to it.
    Compatibility(String),
    /// A declared path is not a safe package-relative path at all.
    ///
    /// Kept distinct from [`FormatError::Path`] because Draft treats an attempt
    /// to escape the package as a protected-access refusal, while a merely
    /// misplaced or wrongly typed file is a configuration mistake.
    UnsafePath(String),
    /// A declared path is outside its permitted namespace or has an
    /// unsupported static file type.
    Path(String),
    /// Signature, threshold or key-material verification failed.
    Signature(String),
    /// A document could not be encoded or decoded.
    Encoding(String),
    /// A declared size limit was exceeded.
    Limit(String),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identity(detail)
            | Self::Compatibility(detail)
            | Self::UnsafePath(detail)
            | Self::Path(detail)
            | Self::Signature(detail)
            | Self::Encoding(detail)
            | Self::Limit(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for FormatError {}

/// Map a portable DCG contract failure onto this crate's taxonomy.
///
/// Only the reused primitives — the identifier grammar, canonical JSON and
/// Ed25519 verification — can raise one of these here, so the mapping is
/// narrow and deliberate rather than a catch-all:
///
/// * `Identity` and `Encoding` mean the same thing in both crates.
/// * `Signature` is verification failure in both.
/// * `Consistency` means a document's own fields disagree with one another,
///   which in package terms is an identity failure.
/// * `Integrity` means a recomputed digest did not match the one carried
///   alongside it, which is the same class of failure as a bad signature: the
///   bytes are not what something attested them to be.
impl From<draft_dcg_contract::FormatError> for FormatError {
    fn from(error: draft_dcg_contract::FormatError) -> Self {
        use draft_dcg_contract::FormatError as Portable;
        match error {
            Portable::Identity(detail) => Self::Identity(detail),
            Portable::Encoding(detail) => Self::Encoding(detail),
            Portable::Consistency(detail) => Self::Identity(detail),
            Portable::Integrity(detail) | Portable::Signature(detail) => Self::Signature(detail),
        }
    }
}

pub type FormatResult<T> = Result<T, FormatError>;
