//! Structured error model for `draft-core`.
//!
//! Every error carries a machine `code`, a human `message`, optional `context`,
//! and an optional `suggestion` for what to do next.

use serde::{Deserialize, Serialize};

pub type DraftResult<T> = Result<T, DraftError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftError {
    pub kind: DraftErrorKind,
    pub message: String,
    pub context: Option<String>,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DraftErrorKind {
    WorkspaceNotFound,
    OperationLogCorrupt,
    OperationLogLocked,
    VerificationFailed,
    RiskPolicyBlocked,
    ReviewRequired,
    ConflictDetected,
    /// A configured hook exited non-zero, or could not be run.
    HookFailed,
    ProjectScopeRequired,
    TaskDefinitionConflict,
    CandidateNotConfigured,
    ExecutionLimitExceeded,
    ProtectedResourceAccess,
    /// An installed, trusted artifact has no grant permitting this operation.
    ///
    /// Distinct from a missing capability: the knowledge is present and only the
    /// permission is absent, so the fix is to authorize rather than to install.
    CapabilityNotAuthorized,
    /// Nothing installed can perform this operation at all.
    ///
    /// The other half of the pair above, and kept separate for the same reason
    /// `unavailable` and `not_applicable` are: "nobody can do this" and "someone
    /// can but you have not allowed it" have different fixes, and collapsing
    /// them would send the reader to the wrong one.
    CapabilityUnavailable,
    /// Work was derived under observation semantics no longer in force.
    ///
    /// Not a corruption and not a conflict: the work is intact and still
    /// readable. What is missing is a shared frame in which to compare it to the
    /// project, so it must be re-derived before it can change anything.
    ContextSuperseded,
    /// An authorization was assembled from facts about a different revision.
    ///
    /// Deliberately distinct from [`ContextSuperseded`]. That one says the work
    /// itself no longer has a frame; this one says the facts are intact and
    /// simply do not describe the revision being promoted. A Decision judged
    /// one exact revision and a Gate evaluated one exact revision, and a
    /// promotion citing either about a different one is not authorized by it.
    StaleRevision,
    /// The Baseline a promotion was decided against is no longer the accepted
    /// one.
    ///
    /// The work and the judgement are both intact; the project moved underneath
    /// them. Draft refuses rather than rebasing, because an authorization given
    /// against one Baseline says nothing about another.
    StaleBaseline,
    EvidenceStale,
    ApprovalInvalidated,
    RegistryStale,
    LockActive,
    /// The Gate a promotion needs is not satisfied.
    GateUnsatisfied,
    /// Coverage a promotion requires was never established.
    ///
    /// Absence has to be proved rather than assumed, so an unestablished
    /// domain is a refusal and not an empty result.
    CoverageIncomplete,
    DirtyWorkspace,
    UnsupportedSchema,
    CorruptData,
    Validation,
    ReceiptWriteFailed,
    ServiceUnavailable,
    IpcError,
    LockTimeout,
    InvalidConfig,
    Storage,
    NotFound,
    Internal,
}

impl DraftErrorKind {
    /// Stable SCREAMING_SNAKE code for machine-readable output / IPC.
    pub fn code(&self) -> &'static str {
        match self {
            DraftErrorKind::WorkspaceNotFound => "WORKSPACE_NOT_FOUND",
            DraftErrorKind::OperationLogCorrupt => "OPERATION_LOG_CORRUPT",
            DraftErrorKind::OperationLogLocked => "OPERATION_LOG_LOCKED",
            DraftErrorKind::VerificationFailed => "VERIFICATION_FAILED",
            DraftErrorKind::RiskPolicyBlocked => "RISK_POLICY_BLOCKED",
            DraftErrorKind::ReviewRequired => "REVIEW_REQUIRED",
            DraftErrorKind::ConflictDetected => "CONFLICT_DETECTED",
            DraftErrorKind::HookFailed => "HOOK_FAILED",
            DraftErrorKind::ProjectScopeRequired => "PROJECT_SCOPE_REQUIRED",
            DraftErrorKind::TaskDefinitionConflict => "TASK_DEFINITION_CONFLICT",
            DraftErrorKind::CandidateNotConfigured => "CANDIDATE_NOT_CONFIGURED",
            DraftErrorKind::ExecutionLimitExceeded => "EXECUTION_LIMIT_EXCEEDED",
            DraftErrorKind::ProtectedResourceAccess => "PROTECTED_RESOURCE_ACCESS",
            DraftErrorKind::CapabilityNotAuthorized => "CAPABILITY_NOT_AUTHORIZED",
            DraftErrorKind::CapabilityUnavailable => "CAPABILITY_UNAVAILABLE",
            DraftErrorKind::ContextSuperseded => "CONTEXT_SUPERSEDED",
            DraftErrorKind::StaleRevision => "STALE_REVISION",
            DraftErrorKind::StaleBaseline => "STALE_BASELINE",
            DraftErrorKind::EvidenceStale => "EVIDENCE_STALE",
            DraftErrorKind::ApprovalInvalidated => "APPROVAL_INVALIDATED",
            DraftErrorKind::RegistryStale => "REGISTRY_STALE",
            DraftErrorKind::LockActive => "LOCK_ACTIVE",
            DraftErrorKind::GateUnsatisfied => "GATE_UNSATISFIED",
            DraftErrorKind::CoverageIncomplete => "COVERAGE_INCOMPLETE",
            DraftErrorKind::DirtyWorkspace => "DIRTY_WORKSPACE",
            DraftErrorKind::UnsupportedSchema => "UNSUPPORTED_SCHEMA",
            DraftErrorKind::CorruptData => "CORRUPT_DATA",
            DraftErrorKind::Validation => "VALIDATION_ERROR",
            DraftErrorKind::ReceiptWriteFailed => "RECEIPT_WRITE_FAILED",
            DraftErrorKind::ServiceUnavailable => "SERVICE_UNAVAILABLE",
            DraftErrorKind::IpcError => "IPC_ERROR",
            DraftErrorKind::LockTimeout => "LOCK_TIMEOUT",
            DraftErrorKind::InvalidConfig => "INVALID_CONFIG",
            DraftErrorKind::Storage => "STORAGE_ERROR",
            DraftErrorKind::NotFound => "NOT_FOUND",
            DraftErrorKind::Internal => "INTERNAL_ERROR",
        }
    }
}

impl DraftError {
    pub fn new(kind: DraftErrorKind, message: impl Into<String>) -> Self {
        DraftError {
            kind,
            message: message.into(),
            context: None,
            suggestion: None,
        }
    }

    pub fn with_context(mut self, ctx: impl Into<String>) -> Self {
        self.context = Some(ctx.into());
        self
    }

    pub fn with_suggestion(mut self, s: impl Into<String>) -> Self {
        self.suggestion = Some(s.into());
        self
    }

    pub fn storage(message: impl Into<String>) -> Self {
        DraftError::new(DraftErrorKind::Storage, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        DraftError::new(DraftErrorKind::NotFound, message)
    }

    pub fn invalid_config(message: impl Into<String>) -> Self {
        DraftError::new(DraftErrorKind::InvalidConfig, message)
    }

    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
}

impl std::fmt::Display for DraftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code(), self.message)?;
        if let Some(ctx) = &self.context {
            write!(f, "\n  context: {ctx}")?;
        }
        if let Some(s) = &self.suggestion {
            write!(f, "\n  try: {s}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DraftError {}

impl From<std::io::Error> for DraftError {
    fn from(e: std::io::Error) -> Self {
        DraftError::new(DraftErrorKind::Storage, e.to_string())
    }
}
