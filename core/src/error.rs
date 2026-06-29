//! Provider-neutral structured error model for `draft-core` (TDD §12, ERR-001).
//!
//! Every error carries a machine `code`, a human `message`, optional `context`,
//! and an optional `suggestion` for what to do next.

use serde::{Deserialize, Serialize};

use crate::vcs::errors::{ProviderError, ProviderErrorKind};

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
    ProviderNotDetected,
    ProviderAmbiguous,
    ProviderUnsupportedOperation,
    ProviderCommandFailed,
    OperationLogCorrupt,
    OperationLogLocked,
    VerificationFailed,
    RiskPolicyBlocked,
    ReviewRequired,
    ConflictDetected,
    FinalizationFailed,
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
            DraftErrorKind::ProviderNotDetected => "PROVIDER_NOT_DETECTED",
            DraftErrorKind::ProviderAmbiguous => "PROVIDER_AMBIGUOUS",
            DraftErrorKind::ProviderUnsupportedOperation => "PROVIDER_UNSUPPORTED_OPERATION",
            DraftErrorKind::ProviderCommandFailed => "PROVIDER_COMMAND_FAILED",
            DraftErrorKind::OperationLogCorrupt => "OPERATION_LOG_CORRUPT",
            DraftErrorKind::OperationLogLocked => "OPERATION_LOG_LOCKED",
            DraftErrorKind::VerificationFailed => "VERIFICATION_FAILED",
            DraftErrorKind::RiskPolicyBlocked => "RISK_POLICY_BLOCKED",
            DraftErrorKind::ReviewRequired => "REVIEW_REQUIRED",
            DraftErrorKind::ConflictDetected => "CONFLICT_DETECTED",
            DraftErrorKind::FinalizationFailed => "FINALIZATION_FAILED",
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

impl From<ProviderError> for DraftError {
    fn from(e: ProviderError) -> Self {
        let kind = match e.kind {
            ProviderErrorKind::NotDetected => DraftErrorKind::ProviderNotDetected,
            ProviderErrorKind::Ambiguous => DraftErrorKind::ProviderAmbiguous,
            ProviderErrorKind::UnsupportedOperation => DraftErrorKind::ProviderUnsupportedOperation,
            ProviderErrorKind::CommandFailed => DraftErrorKind::ProviderCommandFailed,
            ProviderErrorKind::Conflict => DraftErrorKind::ConflictDetected,
            ProviderErrorKind::InvalidState => DraftErrorKind::FinalizationFailed,
            ProviderErrorKind::Io => DraftErrorKind::Storage,
        };
        DraftError {
            kind,
            message: e.message,
            context: e.context,
            suggestion: e.suggestion,
        }
    }
}
