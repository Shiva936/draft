//! Draft's extension domain.
//!
//! The portable half of the extension model — identity, manifests,
//! contributions, compatibility and the signed catalog format — lives in
//! `draft-extension-contract`, the crate an extension author or publisher
//! consumes. This module re-exports it and adds the half only Draft can own:
//! how a package's schema markers bind to Draft's closed contract registry,
//! what Draft durably records about an installed artifact, and where it came
//! from.
//!
//! Nothing here knows the name of any particular extension, and nothing here
//! executes anything.

pub mod algebra;
pub mod authorization;
pub mod capability;
pub mod lifecycle;
pub mod provenance;

pub use algebra::{composition_of, Composition, KeyedEntry};
pub use authorization::{
    AuthorizationDecision, AuthorizationResult, AuthorizationSubject, AuthorizedArtifact,
    ExtensionAuthorizationGrant, ExtensionAuthorizationRegistry, PendingAuthorization,
    AUTHORIZATION_EVALUATOR_REVISION,
};
pub use capability::{
    matches, matches_raw, supports_contribution, ActiveContributions, CapabilityGap,
    ClassCollision, ClassificationOutcome, Contributed, ContributedCheck, ExtensionCapabilityKind,
    ExtensionContributionSource, NoExtensions, Resolution, ResourceView, WithheldCapability,
    FILE_SCHEME, SUPPORTED_CONTRIBUTION_KINDS,
};
pub use draft_extension_contract::{
    canonical_json, catalog, compatibility, contribution, identifier, manifest, package, schema,
    AdapterCapabilities, AttributeMatch, AttributeValue, CandidateExecution, CandidateLimits,
    CandidatePreset, CatalogKey, CatalogTarget, ChangeAspectName, ChangeMetricBudget,
    CheckRequirement, CheckSelection, ClassAttribute, ComparisonContribution, ContributionPayload,
    ControlPolicy, Delegation, ElementExtractionContribution, ElementPredicate, EngineId, Executor,
    ExtensionContribution, ExtensionContributionKind, ExtensionId, ExtensionManifest,
    ExtensionPermission, FormatError, FormatResult, IntentDeclaration, IntentVocabulary,
    MechanismOperation, MetadataDescriptor, NamespacedId, ObservationConsistency, OutputScope,
    PackageSchema, PolicyPreset, PresentationBinding, PresentationContribution,
    PresentationEngineId, PresentationSurface, RawResourcePredicate, RecoveryContribution,
    RelationDirection, ResourceAdapterContribution, ResourceClassificationRule, ResourceForm,
    ResourcePredicate, ResourceRule, ReviewabilityBudget, RiskCondition, RiskRule, RiskRuleSet,
    RiskThresholds, RoleSpec, RootMetadata, SchemaRef, ScopedId, SignatureRecord, SignedEnvelope,
    SnapshotMetadata, StructuredCommand, TargetsMetadata, TaskTemplateContribution,
    TaskTemplateStep, TimestampMetadata, ToolActionContribution, ToolEffect, VerificationCheck,
    VerificationContribution, VerificationStateName, ViewRulePolicy, FORMAT_REVISION,
};
pub use lifecycle::InstalledExtension;
pub use provenance::{
    ArtifactAttestation, ArtifactVerification, ExtensionTrustProvenance,
    InstalledExtensionProvenance, ProducerRef,
};

use crate::contracts::{ContractId, VersionedContract};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// Whether `requirement` accepts the Draft API version this build offers.
pub fn draft_api_compatible(requirement: &str) -> bool {
    draft_extension_contract::draft_api_compatible(requirement, crate::DRAFT_API_VERSION)
}

/// Translate a format-level refusal into Draft's error taxonomy.
///
/// The mapping is deliberate and observable: an attempt to escape the package
/// is a protected-access refusal, a signature shortfall is a review
/// requirement, and everything else is a configuration or storage fault.
pub fn from_format_error(error: FormatError) -> DraftError {
    match error {
        FormatError::Identity(detail)
        | FormatError::Compatibility(detail)
        | FormatError::Path(detail)
        | FormatError::Limit(detail) => DraftError::invalid_config(detail),
        FormatError::UnsafePath(detail) => {
            DraftError::new(DraftErrorKind::ProtectedResourceAccess, detail)
        }
        FormatError::Signature(detail) => DraftError::new(DraftErrorKind::ReviewRequired, detail),
        FormatError::Encoding(detail) => DraftError::storage(detail),
    }
}

/// Run a format operation, surfacing its refusal as a [`DraftError`].
pub fn in_draft<T>(result: FormatResult<T>) -> DraftResult<T> {
    result.map_err(from_format_error)
}

impl VersionedContract for ExtensionManifest {
    const CONTRACT: ContractId = ContractId::ExtensionManifest;
}

impl VersionedContract for RootMetadata {
    const CONTRACT: ContractId = ContractId::CatalogRoot;
}

impl VersionedContract for TimestampMetadata {
    const CONTRACT: ContractId = ContractId::CatalogTimestamp;
}

impl VersionedContract for SnapshotMetadata {
    const CONTRACT: ContractId = ContractId::CatalogSnapshot;
}

impl VersionedContract for TargetsMetadata {
    const CONTRACT: ContractId = ContractId::CatalogTargets;
}

/// Every contract whose document shape the portable format crate defines.
///
/// The format crate publishes a single `FORMAT_REVISION` so tooling outside
/// this repository can write a correct `schema_version` without importing
/// Draft's registry. These are the registry entries that revision has to agree
/// with.
pub const FORMAT_DEFINED_CONTRACTS: &[ContractId] = &[
    ContractId::ExtensionManifest,
    ContractId::CatalogRoot,
    ContractId::CatalogTimestamp,
    ContractId::CatalogSnapshot,
    ContractId::CatalogTargets,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_format_revision_agrees_with_the_contract_registry() {
        for contract in FORMAT_DEFINED_CONTRACTS {
            assert_eq!(
                contract.current().get(),
                FORMAT_REVISION,
                "{} drifted from the portable format revision",
                contract.stable_id()
            );
        }
    }

    #[test]
    fn portable_canonical_json_agrees_with_draft_byte_for_byte() {
        // Catalog signatures are taken over canonical JSON, so an independent
        // implementation in the portable crate is only safe while it agrees
        // with Draft's own. Cover the cases that distinguish encoders.
        let corpus = [
            serde_json::json!(null),
            serde_json::json!(true),
            serde_json::json!(0),
            serde_json::json!(-17),
            serde_json::json!(1.5),
            serde_json::json!(""),
            serde_json::json!("plain"),
            serde_json::json!("quote\" backslash\\ newline\n tab\t return\r"),
            serde_json::json!("unicode \u{00e9}\u{4e2d}\u{1f600}"),
            serde_json::json!("control \u{1}\u{1f}"),
            serde_json::json!([]),
            serde_json::json!([3, 1, 2]),
            serde_json::json!({}),
            serde_json::json!({"b": 1, "a": 2}),
            serde_json::json!({"z": {"y": [{"b": 1, "a": 2}]}, "a": null}),
            serde_json::json!({"": "empty key", "0": 0}),
        ];
        for value in corpus {
            assert_eq!(
                draft_extension_contract::canonical_json(&value),
                crate::support::hashing::canonical_json(&value),
                "canonical encodings diverged for {value}"
            );
        }
    }

    #[test]
    fn format_refusals_keep_their_draft_error_kinds() {
        assert_eq!(
            from_format_error(FormatError::UnsafePath("escape".into())).kind,
            DraftErrorKind::ProtectedResourceAccess
        );
        assert_eq!(
            from_format_error(FormatError::Signature("threshold".into())).kind,
            DraftErrorKind::ReviewRequired
        );
        assert_eq!(
            from_format_error(FormatError::Path("namespace".into())).kind,
            DraftErrorKind::InvalidConfig
        );
        assert_eq!(
            from_format_error(FormatError::Identity("id".into())).kind,
            DraftErrorKind::InvalidConfig
        );
        assert_eq!(
            from_format_error(FormatError::Compatibility("api".into())).kind,
            DraftErrorKind::InvalidConfig
        );
        assert_eq!(
            from_format_error(FormatError::Limit("size".into())).kind,
            DraftErrorKind::InvalidConfig
        );
        assert_eq!(
            from_format_error(FormatError::Encoding("json".into())).kind,
            DraftErrorKind::Storage
        );
    }
}
