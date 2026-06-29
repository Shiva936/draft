//! Structured provider error model.

use serde::{Deserialize, Serialize};

/// Error returned by provider operations. Providers must convert their native
/// failures (e.g. a failed `git` invocation) into one of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
    /// Optional additional context (command output, path, etc.).
    pub context: Option<String>,
    /// Optional actionable suggestion for the user.
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderErrorKind {
    NotDetected,
    Ambiguous,
    UnsupportedOperation,
    CommandFailed,
    InvalidState,
    Conflict,
    Io,
}

impl ProviderError {
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        ProviderError {
            kind,
            message: message.into(),
            context: None,
            suggestion: None,
        }
    }

    pub fn unsupported(op: &str) -> Self {
        ProviderError {
            kind: ProviderErrorKind::UnsupportedOperation,
            message: format!("This provider does not support: {op}"),
            context: None,
            suggestion: Some("This is expected for experimental or limited providers.".to_string()),
        }
    }

    pub fn command_failed(message: impl Into<String>) -> Self {
        ProviderError::new(ProviderErrorKind::CommandFailed, message)
    }

    pub fn with_context(mut self, ctx: impl Into<String>) -> Self {
        self.context = Some(ctx.into());
        self
    }

    pub fn with_suggestion(mut self, s: impl Into<String>) -> Self {
        self.suggestion = Some(s.into());
        self
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)?;
        if let Some(ctx) = &self.context {
            write!(f, " ({ctx})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ProviderError {}

impl From<std::io::Error> for ProviderError {
    fn from(e: std::io::Error) -> Self {
        ProviderError::new(ProviderErrorKind::Io, e.to_string())
    }
}
