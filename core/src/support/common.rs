//! Shared Draft-native primitives used across all of `draft-core`.

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

/// Who a change is on behalf of.
///
/// A small tagged identifier and nothing else. It lives here because two layers
/// need it and neither may depend on the other: the layer that owns resource
/// mutation carries it in a plan, and the layer that stages edits records it on
/// a session.
#[derive(Debug, Clone, PartialEq, Eq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditAttribution {
    Task { id: String },
    ChangePack { id: String },
    CandidateExecution { id: String },
    Review { id: String },
}

impl EditAttribution {
    pub fn id(&self) -> &str {
        match self {
            Self::Task { id }
            | Self::ChangePack { id }
            | Self::CandidateExecution { id }
            | Self::Review { id } => id,
        }
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

// Draft-owned stable IDs (DR-001).
// The project identity is `draft_dcg_contract::ids::ProjectId`. It is not
// redefined here: one canonical value has exactly one Rust type, and a second
// definition could drift from the one that travels in canonical facts.
// `project::mint_project_id` mints one — Core mints, the contract validates.
id_newtype!(
    /// Identifies a Draft operation-log entry by ULID-like opaque id (the
    /// monotonic sequence number is the file name; this is a stable handle).
    OperationId, "op_");
id_newtype!(
    /// Identifies one Draft global user store, minted once when Draft itself
    /// establishes the store root and never rotated (`home.json`).
    GlobalStoreId, "gst_");
id_newtype!(
    /// Identifies a stored project-state snapshot file.
    ///
    /// Still `chk_`, and deliberately **not** an `obs_` Observation. What this
    /// names is the artifact the rollback machinery reads back:
    /// `snapshots/<chk_>.json`, plus an optional handle on
    /// `ObservationRunProvenance` that sits *beside* its `runs` rather than
    /// standing in for them.
    ///
    /// The DCG acceptance path proves the distinction. It derives its own
    /// `run_` id from what was observed and never reads this one, so a
    /// snapshot and an observation run are two identities over the same act,
    /// and renaming one into the other would assert an equivalence the code
    /// contradicts.
    ///
    /// The real remedy is not a prefix change: returning to a prior state is
    /// what Baseline lineage does in the DCG, and this family retires with the
    /// checkpoint/rollback mechanism it serves.
    SnapshotId, "chk_");
id_newtype!(
    /// Identifies an immutable rollback plan.
    RollbackPlanId, "rbp_");
id_newtype!(
    /// Identifies a verification plan.
    VerificationPlanId, "vplan_");
id_newtype!(
    /// Identifies a verification result.
    VerificationResultId, "vres_");
id_newtype!(
    /// Identifies a durable receipt.
    ReceiptId, "rcp_");
id_newtype!(
    /// Identifies an actor (human/agent/service).
    ActorId, "act_");
id_newtype!(
    /// Identifies a piece of review or execution evidence.
    EvidenceId, "evd_");
id_newtype!(
    /// Identifies a Draft task.
    TaskId, "tsk_");
id_newtype!(
    /// Identifies one task execution.
    ExecutionId, "exe_");
