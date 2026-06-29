//! Shared provider-neutral primitives used across all of `draft-core`.
//!
//! These types deliberately contain no provider-specific (e.g. Git) concepts.

use serde::{Deserialize, Serialize};

/// Wall-clock timestamp used throughout Draft metadata.
pub type Timestamp = chrono::DateTime<chrono::Utc>;

/// Returns the current timestamp. Centralized so tests can reason about it.
pub fn now() -> Timestamp {
    chrono::Utc::now()
}

/// A workspace-relative path, always stored using forward slashes so metadata
/// is portable across platforms.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkspacePath(pub String);

impl WorkspacePath {
    pub fn new(s: impl Into<String>) -> Self {
        WorkspacePath(s.into().replace('\\', "/"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Build a workspace path from a filesystem path relative to `root`.
    pub fn from_relative(path: &std::path::Path) -> Self {
        WorkspacePath::new(path.to_string_lossy().into_owned())
    }
}

impl std::fmt::Display for WorkspacePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for WorkspacePath {
    fn from(s: &str) -> Self {
        WorkspacePath::new(s)
    }
}

/// Declares a string-newtype identifier with the common impls Draft relies on
/// (serde, Display, From<String>/&str, `new()`, `as_str()`, random `generate()`).
#[macro_export]
macro_rules! id_newtype {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, ::serde::Serialize, ::serde::Deserialize)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                $name(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Generate a fresh, prefixed, random identifier.
            pub fn generate() -> Self {
                let raw = ::uuid::Uuid::new_v4().simple().to_string();
                $name(format!("{}{}", $prefix, &raw[..12]))
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                $name(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                $name(s.to_string())
            }
        }
    };
}

// Draft-owned stable IDs (DR-001). Provider-native IDs live in `vcs::types`.
id_newtype!(
    /// Identifies a Draft workspace.
    WorkspaceId, "ws_");
id_newtype!(
    /// Identifies a Draft change (a reviewable unit before finalization).
    DraftChangeId, "chg_");
id_newtype!(
    /// Identifies a Draft operation-log entry by ULID-like opaque id (the
    /// monotonic sequence number is the file name; this is a stable handle).
    OperationId, "op_");
id_newtype!(
    /// Identifies a review session.
    ReviewSessionId, "rev_");
id_newtype!(
    /// Identifies a Draft checkpoint.
    CheckpointId, "ckpt_");
id_newtype!(
    /// Identifies a verification plan.
    VerificationPlanId, "vplan_");
id_newtype!(
    /// Identifies a verification result.
    VerificationResultId, "vres_");
id_newtype!(
    /// Identifies a finalization plan.
    FinalizationPlanId, "fplan_");
id_newtype!(
    /// Identifies a finalization result.
    FinalizationResultId, "fres_");
id_newtype!(
    /// Identifies a durable receipt.
    ReceiptId, "rcpt_");
id_newtype!(
    /// Identifies an actor (human/agent/service).
    ActorId, "act_");
