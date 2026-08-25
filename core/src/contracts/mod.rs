//! Typed metadata for every independently persisted or transmitted Draft contract.
//!
//! Registry membership is deliberately closed at compile time. Persisted and
//! wire values remain self-describing through their own `schema_version`; this
//! module validates that marker against the statically expected Rust contract
//! and never uses registry metadata to reinterpret bytes.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

pub const BINARY_METADATA_PART: &str = "metadata";
pub const BINARY_CONTENT_PART: &str = "content";
pub const BINARY_METADATA_MEDIA_TYPE: &str = "application/json";

/// A schema version declared by one contract boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SchemaVersion(u32);

impl SchemaVersion {
    /// Version supported by every contract shipped in Draft v0.3.4.
    pub const V1: Self = Self(1);

    pub const fn new(version: u32) -> Self {
        Self(version)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractBoundary {
    Persisted,
    Wire,
    PersistedAndWire,
}

#[derive(Debug, Clone, Copy)]
pub struct ContractMetadata {
    pub id: ContractId,
    pub stable_id: &'static str,
    pub current: SchemaVersion,
    pub supported: &'static [SchemaVersion],
    pub boundary: ContractBoundary,
    pub schema: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionPolicy {
    pub current: SchemaVersion,
    pub supported: &'static [SchemaVersion],
}

impl VersionPolicy {
    pub const V1_ONLY: Self = Self {
        current: SchemaVersion::V1,
        supported: &[SchemaVersion::V1],
    };

    pub const fn new(current: SchemaVersion, supported: &'static [SchemaVersion]) -> Self {
        Self { current, supported }
    }
}

macro_rules! contract_registry {
    ($( $variant:ident => ($stable:literal, $boundary:ident, $schema:expr, $policy:expr) ),+ $(,)?) => {
        /// Closed set of contracts understood by this binary.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum ContractId { $( $variant, )+ }

        impl ContractId {
            pub const ALL: &'static [Self] = &[ $( Self::$variant, )+ ];

            pub const fn metadata(self) -> ContractMetadata {
                match self {
                    $( Self::$variant => {
                        let policy: VersionPolicy = $policy;
                        ContractMetadata {
                            id: Self::$variant,
                            stable_id: $stable,
                            current: policy.current,
                            supported: policy.supported,
                            boundary: ContractBoundary::$boundary,
                            schema: $schema,
                        }
                    }, )+
                }
            }

            pub const fn current(self) -> SchemaVersion {
                self.metadata().current
            }

            pub const fn stable_id(self) -> &'static str {
                self.metadata().stable_id
            }
        }
    };
}

contract_registry! {
    BinaryTransferMetadata => ("binary-transfer-metadata", Wire, None, VersionPolicy::V1_ONLY),
    WorkspaceMetadata => ("workspace-metadata", Persisted, None, VersionPolicy::V1_ONLY),
    DraftConfig => ("draft-config", Persisted, Some("config.schema.json"), VersionPolicy::V1_ONLY),
    ProjectRegistry => ("project-registry", Persisted, Some("registry.schema.json"), VersionPolicy::V1_ONLY),
    ProjectRegistryEntry => ("project-registry-entry", Wire, None, VersionPolicy::V1_ONLY),
    AdoptionReceipt => ("adoption-receipt", Persisted, None, VersionPolicy::V1_ONLY),
    StableHead => ("stable-head", Persisted, Some("stable-head.schema.json"), VersionPolicy::V1_ONLY),
    StableGraphIndex => ("stable-graph-index", Persisted, None, VersionPolicy::V1_ONLY),
    WorkspaceSnapshot => ("workspace-snapshot", Persisted, None, VersionPolicy::V1_ONLY),
    CanonicalSourcePolicy => ("canonical-source-policy", Persisted, None, VersionPolicy::V1_ONLY),
    CanonicalSourceView => ("canonical-source-view", PersistedAndWire, Some("canonical-source-view.schema.json"), VersionPolicy::V1_ONLY),
    WorkspaceRevision => ("workspace-revision", PersistedAndWire, Some("workspace-revision.schema.json"), VersionPolicy::V1_ONLY),
    ObjectPackIndex => ("object-pack-index", Persisted, None, VersionPolicy::V1_ONLY),
    ObjectPack => ("object-pack", Persisted, None, VersionPolicy::V1_ONLY),
    WorkspaceIndex => ("workspace-index", Persisted, None, VersionPolicy::V1_ONLY),
    EventRecord => ("event", PersistedAndWire, Some("event.schema.json"), VersionPolicy::V1_ONLY),
    EventIndex => ("event-index", Persisted, None, VersionPolicy::V1_ONLY),
    Receipt => ("receipt", PersistedAndWire, Some("receipt.schema.json"), VersionPolicy::V1_ONLY),
    TransparencyEntry => ("transparency-entry", Persisted, None, VersionPolicy::V1_ONLY),
    RevokedKeyRegistry => ("revoked-key-registry", Persisted, None, VersionPolicy::V1_ONLY),
    SigningKeyRecord => ("signing-key-record", Persisted, None, VersionPolicy::V1_ONLY),
    SecurityActor => ("security-actor", Persisted, None, VersionPolicy::V1_ONLY),
    PublicKeyRecord => ("public-key-record", Persisted, None, VersionPolicy::V1_ONLY),
    CandidateRecord => ("candidate-record", Wire, None, VersionPolicy::V1_ONLY),
    CandidateRegistry => ("candidate-registry", Persisted, None, VersionPolicy::V1_ONLY),
    OperationRecord => ("operation", Persisted, Some("operation.schema.json"), VersionPolicy::V1_ONLY),
    FencedLease => ("fenced-lease", Wire, None, VersionPolicy::V1_ONLY),
    LeaseState => ("lease-state", Persisted, Some("lease.schema.json"), VersionPolicy::V1_ONLY),
    RecoveryRecord => ("recovery-record", Persisted, None, VersionPolicy::V1_ONLY),
    EditorSession => ("editor-session", Persisted, Some("editor-session.schema.json"), VersionPolicy::V1_ONLY),
    EditorCommitResult => ("editor-commit-result", Wire, None, VersionPolicy::V1_ONLY),
    NotificationStore => ("notification-store", Persisted, Some("notification.schema.json"), VersionPolicy::V1_ONLY),
    NotificationRecord => ("notification", Wire, Some("notification.schema.json"), VersionPolicy::V1_ONLY),
    SubmitRecord => ("submit-record", Persisted, Some("submit-record.schema.json"), VersionPolicy::V1_ONLY),
    RollbackPlan => ("rollback-plan", PersistedAndWire, Some("rollback-plan.schema.json"), VersionPolicy::V1_ONLY),
    RollbackRecord => ("rollback-record", Persisted, Some("rollback-record.schema.json"), VersionPolicy::V1_ONLY),
    VerificationConfig => ("verification-config", Persisted, None, VersionPolicy::V1_ONLY),
    VerificationEvidence => ("verification-evidence", PersistedAndWire, Some("verification.schema.json"), VersionPolicy::V1_ONLY),
    RiskConfig => ("risk-config", Persisted, None, VersionPolicy::V1_ONLY),
    RiskReport => ("risk-report", PersistedAndWire, Some("risk.schema.json"), VersionPolicy::V1_ONLY),
    WorkflowEvidence => ("workflow-evidence", PersistedAndWire, Some("evidence-state.schema.json"), VersionPolicy::V1_ONLY),
    DecisionRecord => ("decision-record", Persisted, Some("decision-record.schema.json"), VersionPolicy::V1_ONLY),
    Waiver => ("waiver", Persisted, Some("waiver.schema.json"), VersionPolicy::V1_ONLY),
    InboxItem => ("inbox-item", Persisted, Some("inbox-item.schema.json"), VersionPolicy::V1_ONLY),
    Policy => ("policy", Persisted, None, VersionPolicy::V1_ONLY),
    AffectedPathIndex => ("affected-path-index", Persisted, None, VersionPolicy::V1_ONLY),
    VerificationCache => ("verification-cache", Persisted, None, VersionPolicy::V1_ONLY),
    ReviewFile => ("review-file", Persisted, None, VersionPolicy::V1_ONLY),
    Composition => ("composition", PersistedAndWire, Some("composition.schema.json"), VersionPolicy::V1_ONLY),
    ProjectStateReport => ("project-state-report", Wire, Some("project-state.schema.json"), VersionPolicy::V1_ONLY),
    PackManifest => ("pack", PersistedAndWire, Some("pack.schema.json"), VersionPolicy::V1_ONLY),
    PackRevision => ("pack-revision", PersistedAndWire, Some("pack-revision.schema.json"), VersionPolicy::V1_ONLY),
    PackQuarantine => ("pack-quarantine", Persisted, Some("pack-quarantine.schema.json"), VersionPolicy::V1_ONLY),
    PackLock => ("pack-lock", PersistedAndWire, Some("pack-lock.schema.json"), VersionPolicy::V1_ONLY),
    PackWorkspace => ("pack-workspace", Persisted, None, VersionPolicy::V1_ONLY),
    PatchSet => ("patch-set", PersistedAndWire, None, VersionPolicy::V1_ONLY),
    PackEvidence => ("pack-evidence", Persisted, None, VersionPolicy::V1_ONLY),
    PackLifecycle => ("pack-lifecycle", PersistedAndWire, Some("pack-lifecycle.schema.json"), VersionPolicy::V1_ONLY),
    LifecycleEvidence => ("lifecycle-evidence", PersistedAndWire, Some("evidence-dependency.schema.json"), VersionPolicy::V1_ONLY),
    Draftpack => ("draftpack", Wire, Some("draftpack.schema.json"), VersionPolicy::V1_ONLY),
    DraftpackProvenance => ("draftpack-provenance", Wire, Some("draftpack-provenance.schema.json"), VersionPolicy::V1_ONLY),
    TaskDefinition => ("task", PersistedAndWire, Some("task.schema.json"), VersionPolicy::V1_ONLY),
    TaskTemplate => ("task-template", PersistedAndWire, Some("task-template.schema.json"), VersionPolicy::V1_ONLY),
    Execution => ("execution", PersistedAndWire, Some("execution.schema.json"), VersionPolicy::V1_ONLY),
    TaskIndex => ("task-index", Persisted, None, VersionPolicy::V1_ONLY),
    CandidateProfile => ("candidate-profile", PersistedAndWire, Some("candidate-profile.schema.json"), VersionPolicy::V1_ONLY),
    CandidatePreset => ("candidate-preset", Persisted, None, VersionPolicy::V1_ONLY),
    ExtensionManifest => ("extension-package", PersistedAndWire, Some("extension-package.schema.json"), VersionPolicy::V1_ONLY),
    InstalledExtensionProvenance => ("installed-extension-provenance", Persisted, None, VersionPolicy::V1_ONLY),
    ExtensionRegistry => ("extension-registry", Persisted, None, VersionPolicy::V1_ONLY),
    CatalogSourceRegistry => ("catalog-source-registry", Persisted, Some("extension-catalog.schema.json"), VersionPolicy::V1_ONLY),
    CatalogSource => ("catalog-source", PersistedAndWire, Some("extension-catalog.schema.json"), VersionPolicy::V1_ONLY),
    CatalogRoot => ("catalog-root", PersistedAndWire, Some("extension-catalog.schema.json"), VersionPolicy::V1_ONLY),
    CatalogTimestamp => ("catalog-timestamp", PersistedAndWire, Some("extension-catalog.schema.json"), VersionPolicy::V1_ONLY),
    CatalogSnapshot => ("catalog-snapshot", PersistedAndWire, Some("extension-catalog.schema.json"), VersionPolicy::V1_ONLY),
    CatalogTargets => ("catalog-targets", PersistedAndWire, Some("extension-catalog.schema.json"), VersionPolicy::V1_ONLY),
    CatalogTrust => ("catalog-trust", Persisted, None, VersionPolicy::V1_ONLY),
    CachedCatalog => ("cached-catalog", Persisted, None, VersionPolicy::V1_ONLY),
    IpcRequest => ("ipc-request", Wire, Some("ipc.schema.json"), VersionPolicy::V1_ONLY),
    IpcResponse => ("ipc-response", Wire, Some("ipc.schema.json"), VersionPolicy::V1_ONLY),
    IpcHandshakeRequest => ("ipc-handshake-request", Wire, Some("ipc.schema.json"), VersionPolicy::V1_ONLY),
    IpcHandshakeResponse => ("ipc-handshake-response", Wire, Some("ipc.schema.json"), VersionPolicy::V1_ONLY),
    IpcProgressEvent => ("ipc-progress-event", Wire, Some("ipc.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleSession => ("console-session", Wire, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleWorkspaceRevision => ("console-workspace-revision", Wire, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleRegistryProject => ("console-registry-project", Wire, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleTaskDefinition => ("console-task-definition", Wire, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleCatalogSource => ("console-catalog-source", Wire, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleServiceJob => ("console-service-job", Wire, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleApiFailure => ("console-api-failure", Wire, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleApiEnvelope => ("console-api-envelope", Wire, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleMutationRequest => ("console-mutation-request", Wire, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleJobsEvent => ("console-jobs-event", Wire, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleDaemonEvent => ("console-daemon-event", Wire, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleOverview => ("console-overview", Wire, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ConsoleContractsSchema => ("console-contracts-schema", Persisted, Some("console-http.schema.json"), VersionPolicy::V1_ONLY),
    ServiceJob => ("service-job", PersistedAndWire, None, VersionPolicy::V1_ONLY),
}

/// Statically associates one Rust type with one registered contract.
pub trait VersionedContract {
    const CONTRACT: ContractId;

    fn schema_version() -> u32 {
        Self::CONTRACT.current().get()
    }
}

/// Obtain the current write version for one statically selected contract.
pub const fn current_version(contract: ContractId) -> u32 {
    contract.current().get()
}

/// Test an artifact's declared version against one statically selected
/// contract's supported decoder policy.
pub fn supports_version(contract: ContractId, declared: u32) -> bool {
    contract
        .metadata()
        .supported
        .iter()
        .any(|supported| supported.get() == declared)
}

/// Canonical control metadata for transfers that combine JSON and opaque bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryTransferMetadata {
    pub schema_version: u32,
    pub media_type: String,
    pub length: u64,
    pub digest: String,
    pub correlation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

impl VersionedContract for BinaryTransferMetadata {
    const CONTRACT: ContractId = ContractId::BinaryTransferMetadata;
}

impl BinaryTransferMetadata {
    pub fn for_bytes(
        media_type: impl Into<String>,
        bytes: &[u8],
        correlation_id: impl Into<String>,
        operation_id: Option<String>,
    ) -> Self {
        Self {
            schema_version: Self::schema_version(),
            media_type: media_type.into(),
            length: bytes.len() as u64,
            digest: crate::support::hashing::sha256_hex(bytes),
            correlation_id: correlation_id.into(),
            operation_id,
        }
    }

    pub fn validate_bytes(&self, bytes: &[u8]) -> DraftResult<()> {
        validate_declared_version(self.schema_version, Self::CONTRACT, ContractContext::Wire)?;
        if self.length != bytes.len() as u64
            || self.digest != crate::support::hashing::sha256_hex(bytes)
        {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "binary transfer length or digest mismatch",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractContext {
    Persisted,
    Wire,
}

impl ContractContext {
    fn invalid_kind(self) -> DraftErrorKind {
        match self {
            Self::Persisted => DraftErrorKind::CorruptData,
            Self::Wire => DraftErrorKind::Validation,
        }
    }
}

pub fn validate_declared_version(
    declared: u32,
    contract: ContractId,
    context: ContractContext,
) -> DraftResult<()> {
    let metadata = contract.metadata();
    if !metadata
        .supported
        .iter()
        .any(|supported| supported.get() == declared)
    {
        let supported = metadata
            .supported
            .iter()
            .map(|version| version.get().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(DraftError::new(
            DraftErrorKind::UnsupportedSchema,
            format!(
                "{} schema version {declared} is unsupported; supported: {supported}",
                metadata.stable_id
            ),
        ));
    }
    let _ = context;
    Ok(())
}

/// Validate an artifact marker against the statically expected contract.
pub fn validate_schema_version(
    value: &Value,
    contract: ContractId,
    context: ContractContext,
) -> DraftResult<()> {
    let Some(version) = value.get("schema_version") else {
        return Err(DraftError::new(
            context.invalid_kind(),
            format!(
                "{} requires numeric field 'schema_version'",
                contract.stable_id()
            ),
        ));
    };
    let Value::Number(number) = version else {
        return Err(DraftError::new(
            context.invalid_kind(),
            format!(
                "{} field 'schema_version' must be an unsigned integer",
                contract.stable_id()
            ),
        ));
    };
    let Some(version) = number.as_u64().and_then(|value| u32::try_from(value).ok()) else {
        return Err(DraftError::new(
            context.invalid_kind(),
            format!(
                "{} field 'schema_version' must be an unsigned 32-bit integer",
                contract.stable_id()
            ),
        ));
    };
    validate_declared_version(version, contract, context)
}

pub fn decode<T: DeserializeOwned + VersionedContract>(
    bytes: &[u8],
    context: ContractContext,
) -> DraftResult<T> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| {
        DraftError::new(
            context.invalid_kind(),
            format!("{} is not valid JSON: {error}", T::CONTRACT.stable_id()),
        )
    })?;
    validate_schema_version(&value, T::CONTRACT, context)?;
    serde_json::from_value(value).map_err(|error| {
        DraftError::new(
            context.invalid_kind(),
            format!("{} validation failed: {error}", T::CONTRACT.stable_id()),
        )
    })
}

pub fn decode_persisted<T: DeserializeOwned + VersionedContract>(bytes: &[u8]) -> DraftResult<T> {
    decode(bytes, ContractContext::Persisted)
}

pub fn read_persisted<T: DeserializeOwned + VersionedContract>(path: &Path) -> DraftResult<T> {
    let bytes = std::fs::read(path)?;
    decode_persisted(&bytes).map_err(|error| error.with_context(path.display().to_string()))
}

pub fn decode_wire<T: DeserializeOwned + VersionedContract>(bytes: &[u8]) -> DraftResult<T> {
    decode(bytes, ContractContext::Wire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::collections::BTreeSet;

    #[derive(Debug, Deserialize)]
    struct Example {
        schema_version: u32,
        value: String,
    }

    impl VersionedContract for Example {
        const CONTRACT: ContractId = ContractId::BinaryTransferMetadata;
    }

    #[test]
    fn registry_is_closed_unique_and_v1_for_this_release() {
        let ids = ContractId::ALL
            .iter()
            .map(|id| id.stable_id())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), ContractId::ALL.len());
        for id in ContractId::ALL {
            assert_eq!(id.metadata().current, SchemaVersion::V1);
            assert_eq!(id.metadata().supported, VersionPolicy::V1_ONLY.supported);
        }
    }

    #[test]
    fn schema_failures_are_distinct_and_strict() {
        let unsupported =
            decode_persisted::<Example>(br#"{"schema_version":2,"value":"x"}"#).unwrap_err();
        assert_eq!(unsupported.kind, DraftErrorKind::UnsupportedSchema);
        for invalid in [
            br#"{"schema_version":-1,"value":"x"}"#.as_slice(),
            br#"{"schema_version":1.5,"value":"x"}"#.as_slice(),
            br#"{"value":"x"}"#.as_slice(),
            br#"{"schema_version":"1","value":"x"}"#.as_slice(),
            br#"{"schema_version":1,"value":2}"#.as_slice(),
            b"not json".as_slice(),
        ] {
            assert_eq!(
                decode_persisted::<Example>(invalid).unwrap_err().kind,
                DraftErrorKind::CorruptData
            );
            assert_eq!(
                decode_wire::<Example>(invalid).unwrap_err().kind,
                DraftErrorKind::Validation
            );
        }

        let decoded = decode_persisted::<Example>(br#"{"schema_version":1,"value":"x"}"#).unwrap();
        assert_eq!(decoded.schema_version, Example::schema_version());
        assert_eq!(decoded.value, "x");
    }

    #[test]
    fn binary_metadata_authenticates_the_opaque_part() {
        let metadata = BinaryTransferMetadata::for_bytes(
            "application/octet-stream",
            b"payload",
            "corr_1",
            Some("op_1".into()),
        );
        metadata.validate_bytes(b"payload").unwrap();
        assert_eq!(
            metadata.validate_bytes(b"tampered").unwrap_err().kind,
            DraftErrorKind::Validation
        );
    }
}
