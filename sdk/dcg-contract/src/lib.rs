//! The portable Draft Change Graph (DCG) contract.
//!
//! This crate is the canonical, dependency-closed representation of every DCG
//! value whose bytes participate in a portable root, a portable interchange
//! proof, or an independently verifiable historical fact. An external verifier
//! that holds only this crate can parse a Draft baseline, recompute its roots,
//! walk a publication's cryptographic chain and check a receipt signature —
//! without Draft's runtime, stores, services, policy or daemon.
//!
//! # What lives here, and what deliberately does not
//!
//! The rule is *portable canonical value*, never *runtime subsystem*. Moving a
//! value newtype down does not move its behaviour down:
//!
//! | Concept | Portable value owner | Runtime owner |
//! |---|---|---|
//! | [`LeaseId`], [`LeaseFence`] | this crate | `core::execution` + `services/locks` |
//! | [`ProjectSecurityStateDigest`] | this crate | `core::project` |
//! | [`PolicyDigest`] | this crate | `core::project` |
//! | [`RegistryId`], [`RegistryRevision`] | this crate | `core::trust` |
//! | [`ProjectControlGeneration`] | this crate | `core::project` |
//! | [`ProviderBindingGeneration`] | this crate | `core::project` |
//! | [`CredentialAuthorityClass`] | this crate | `core::project` |
//! | [`AuthorityDecision`] | this crate | `core::authority` |
//!
//! So this crate owns none of: repository state, installation, trust policy,
//! signing policy, project trust state, authorization logic, provider
//! execution, storage, stores, guards, journals, resolution algorithms, daemon
//! or console behaviour, lifecycle orchestration or extension package concepts.
//!
//! # Providers: describe a route, never execute one
//!
//! Generic portable provider identity and provenance values are deliberately
//! owned here, because canonical historical facts contain them —
//! [`ProviderKindId`], [`ProviderBindingId`], [`ProviderProvenanceRef`],
//! [`ProviderRouteRef`], [`ProviderSemanticDefinitionDigest`] and
//! [`ProviderOperationalProfileDigest`]. Provider clients, HTTP/Git/cloud SDKs,
//! credential secrets, credential handle references, runtime sessions, mutable
//! binding state and dispatch code are forbidden here.
//!
//! # Dependency closure
//!
//! This crate depends on **no** Draft crate. Every field of every canonical
//! type defined here is either another type defined here or a dependency-light
//! `std`/third-party type. `scripts/check-portable-contract-closure.sh` proves
//! that mechanically by building a scratch crate that depends only on this one.

#![forbid(unsafe_code)]

pub mod attribute;
pub mod authority;
pub mod baseline;
pub mod canonical;
pub mod capability;
pub mod coverage;
pub mod digest;
pub mod identifier;
pub mod ids;
pub mod kinds;
pub mod merkle;
pub mod observation;
pub mod producer;
pub mod provider;
pub mod publication;
pub mod receipt;
pub mod relation;
pub mod roots;
pub mod schema;
pub mod security;
pub mod semantics;
pub mod signature;
pub mod state;
pub mod value;

pub use attribute::{AttributeValue, ResourceForm, ResourceLocator};
pub use authority::{AuthorityDecision, AuthorityDecisionOutcome, AuthorityScopeClaim};
pub use baseline::{BaselineId, BaselineManifest};
pub use canonical::{canonical_bytes, canonical_json};
pub use capability::{CapabilityId, ContributionSchemaId};
pub use coverage::{CoverageDomainRef, CoverageEvidence, CoverageStatus, ObservationGapRef};
pub use digest::{domain_hash, Digest};
pub use identifier::{IdentifierClass, NamespacedId, ScopedId};
pub use ids::{
    ActorId, BaselineIdentifier, ChangePackId, DecisionId, EvidenceId, ExecutionId, ObservationId,
    ObservationRunId, ProjectId, PromotionId, ProviderBindingId, PublicationAttemptId,
    PublicationId, ReceiptId, ResourceId, RevisionPackId, TaskId,
};
pub use kinds::{
    AssessmentKindId, EvidenceKindId, OperationKindId, ProviderKindId, RelationTypeId,
    RepresentationKindId, ResourceKindId,
};
pub use observation::{
    Observation, ObservationDigest, ObservationRef, ObservationRun, ObservationRunDigest,
    ObservationRunRef, ObservationStability, ObservationTerminalStatus,
};
pub use producer::ProducerIdentity;
pub use provider::{
    ProviderOperationalProfileDigest, ProviderProvenanceRef, ProviderRouteRef,
    ProviderSemanticDefinitionDigest,
};
pub use publication::{
    DeliverySemantics, Publication, PublicationAttempt, PublicationAttemptDigest,
    PublicationAttemptRef, PublicationDigest, PublicationIdempotencyKey, PublicationOutcome,
    PublicationOutcomeDigest, PublicationOutcomeKind, PublicationPurposeId, PublicationRef,
    PublicationRequestKey, PublicationResolution, PublicationResolutionDigest,
    PublicationResolutionKind, PublicationRetryAuthorization, PublicationRetryAuthorizationDigest,
    RepublishIntentId,
};
pub use receipt::{
    ReceiptEnvelope, ReceiptKind, ReceiptPayload, ReceiptSignerBinding, ReceiptSigningMessage,
    RECEIPT_SIGNATURE_DOMAIN,
};
pub use relation::{
    RelationInstanceKey, RelationProvenance, RelationRecord, RelationRecordDigest, RelationRole,
    RelationState, RelationStateDigest, StateBearingDeclaration, StateBearingDeclarationDigest,
};
pub use roots::{
    BaselineStateEvidenceEntry, CoverageEvidenceRoot, CoverageEvidenceRootBuilder, EvidenceSubject,
    ProjectRelationStateEntry, ProjectResourceStateEntry, ProjectStateRoot,
    ProjectStateRootBuilder, RelationStateEvidenceRef, StateEvidenceRoot, StateEvidenceRootBuilder,
};
pub use schema::SchemaRef;
pub use security::{
    CredentialAuthorityClass, PolicyDigest, ProjectSecurityStateDigest, SecurityControlKindId,
    SecurityFactRef,
};
pub use semantics::{
    LocatorStateRole, ResourceStateSemanticsContract, ResourceStateSemanticsContractDigest,
    ResourceStateSemanticsId, ResourceStateSemanticsRef,
};
pub use signature::verify_signature;
pub use state::{ResourceState, ResourceStateDigest};
pub use value::{
    LeaseFence, LeaseId, ProjectControlGeneration, ProviderBindingGeneration, RegistryId,
    RegistryRevision, RegistryRevisions, Timestamp,
};

/// The DCG format revision this crate implements.
///
/// There is no revision 2 and no migration path: Draft is pre-release, so the
/// v1 canonical forms frozen here are final for v1.
pub const DCG_FORMAT_REVISION: u32 = 1;

/// Why a portable DCG document is not acceptable.
///
/// Draft maps each variant onto its own error taxonomy; keeping the mapping
/// explicit is what lets this crate stay free of Draft's error type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// Identity, naming or format-marker rules were violated.
    Identity(String),
    /// Canonical serialization or parsing failed.
    Encoding(String),
    /// A canonical structure is internally inconsistent: a derived field
    /// disagrees with the canonical inputs it is derived from, a cardinality
    /// rule is broken, or two canonical entries conflict.
    ///
    /// This is deliberately distinct from [`FormatError::Integrity`]: this
    /// variant means the *value* is invalid, not that stored bytes were
    /// substituted.
    Consistency(String),
    /// A recomputed digest did not equal the digest carried by the reference.
    ///
    /// Draft treats this as `CorruptData` / `IntegrityViolation` — never a
    /// warning, and never something to repair in place.
    Integrity(String),
    /// Signature, key material or signer-binding verification failed.
    Signature(String),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identity(message) => write!(formatter, "identity: {message}"),
            Self::Encoding(message) => write!(formatter, "encoding: {message}"),
            Self::Consistency(message) => write!(formatter, "consistency: {message}"),
            Self::Integrity(message) => write!(formatter, "integrity: {message}"),
            Self::Signature(message) => write!(formatter, "signature: {message}"),
        }
    }
}

impl std::error::Error for FormatError {}

pub type FormatResult<T> = Result<T, FormatError>;
