//! Leases, and the fence that makes a stale holder harmless.
//!
//! # What a lease is and is not
//!
//! A lease is a *product* concept: it coordinates who is working on what, it
//! is visible, and it expires. It is not a correctness lock. The frozen lock
//! order governs `ProcessFileLock` acquisitions; a lease sits above them and
//! may be held across work no lock could span.
//!
//! # Why expiry alone is not safety
//!
//! A holder can be paused between checking its lease and acting on it — long
//! enough for the lease to expire, be reacquired by somebody else, and for
//! both to believe they hold it. Nothing the first holder can observe about
//! its own lease rules this out, because from the inside a pause is
//! indistinguishable from being fast.
//!
//! So every acquisition allocates a strictly greater fence, and a mutation
//! carries the fence it was authorized under. A write from the superseded
//! holder arrives with a lower fence and is refused. The stale actor never has
//! to notice it is stale: the fence makes its writes inert.
//!
//! The fence *value* is portable ([`draft_dcg_contract::LeaseFence`]) because
//! a `PublicationAttempt` records it and an independent verifier must compare
//! it without linking Draft. Allocation, lifecycle and enforcement stay here
//! and in `services/locks` — moving a value down does not move its subsystem
//! down.

use crate::dcg::source_view::WorkspaceRevision;
use crate::project::home::DraftGlobalStore;
use crate::support::common::{now, OperationId, Timestamp};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::write_json;
use crate::support::process_lock::ProcessFileLock;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FencedLease {
    pub schema_version: u32,
    pub lease_id: String,
    pub scope: String,
    pub operation_id: OperationId,
    pub fencing_token: u64,
    pub acquired_at: Timestamp,
    pub expires_at: Timestamp,
}

impl crate::contracts::VersionedContract for FencedLease {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::FencedLease;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeaseState {
    schema_version: u32,
    last_fencing_token: u64,
    active: Option<FencedLease>,
}

impl crate::contracts::VersionedContract for LeaseState {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::LeaseState;
}

impl Default for LeaseState {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::LeaseState,
            ),
            last_fencing_token: 0,
            active: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationPrecondition {
    pub workspace_id: String,
    pub expected_workspace_revision: WorkspaceRevision,
    pub operation_id: OperationId,
    pub lease_id: String,
    pub fencing_token: u64,
    pub policy_revision: Option<String>,
}

pub struct LeaseStore {
    root: PathBuf,
}

impl LeaseStore {
    pub fn global() -> DraftResult<Self> {
        Ok(Self::at(DraftGlobalStore::locate()?.root().join("leases")))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn acquire(
        &self,
        scope: &str,
        operation_id: OperationId,
        ttl: chrono::Duration,
    ) -> DraftResult<FencedLease> {
        let _guard =
            ProcessFileLock::acquire_exclusive(&self.lock_path(scope), Duration::from_secs(5))?;
        let mut state = self.load_state(scope)?;
        if let Some(active) = &state.active {
            if active.expires_at > now() && active.operation_id != operation_id {
                return Err(DraftError::new(
                    DraftErrorKind::LockTimeout,
                    format!("mutation lease '{}' is held by another operation", scope),
                ));
            }
            if active.expires_at > now() && active.operation_id == operation_id {
                return Ok(active.clone());
            }
        }
        state.last_fencing_token = state.last_fencing_token.saturating_add(1);
        let acquired_at = now();
        let lease = FencedLease {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::FencedLease,
            ),
            lease_id: format!("lease_{}", uuid::Uuid::new_v4().simple()),
            scope: scope.into(),
            operation_id,
            fencing_token: state.last_fencing_token,
            acquired_at,
            expires_at: acquired_at + ttl,
        };
        state.active = Some(lease.clone());
        self.save_state(scope, &state)?;
        Ok(lease)
    }

    pub fn release(&self, lease: &FencedLease) -> DraftResult<()> {
        let _guard = ProcessFileLock::acquire_exclusive(
            &self.lock_path(&lease.scope),
            Duration::from_secs(5),
        )?;
        let mut state = self.load_state(&lease.scope)?;
        if state.active.as_ref().is_some_and(|active| {
            active.lease_id == lease.lease_id && active.fencing_token == lease.fencing_token
        }) {
            state.active = None;
            self.save_state(&lease.scope, &state)?;
        }
        Ok(())
    }

    pub fn validate(&self, root: &Path, precondition: &MutationPrecondition) -> DraftResult<()> {
        let current = WorkspaceRevision::derive(root)?;
        if current != precondition.expected_workspace_revision
            || current.workspace_id.as_str() != precondition.workspace_id
        {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "workspace revision changed before mutation commit",
            ));
        }
        let state = self.load_state(&format!("workspace-{}", precondition.workspace_id))?;
        let valid = state.active.as_ref().is_some_and(|lease| {
            lease.lease_id == precondition.lease_id
                && lease.fencing_token == precondition.fencing_token
                && lease.operation_id == precondition.operation_id
                && lease.expires_at > now()
        });
        if !valid {
            // The fence doing its job: a superseded holder's write is refused
            // rather than applied. The stale actor never has to notice.
            crate::support::telemetry::Counter::StaleLeaseFenceRejections.increment();
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "stale or mismatched mutation lease",
            ));
        }
        Ok(())
    }

    fn load_state(&self, scope: &str) -> DraftResult<LeaseState> {
        let path = self.state_path(scope);
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            crate::contracts::decode_persisted(&bytes)
                .map_err(|error| error.with_context(path.display().to_string()))
        } else {
            Ok(LeaseState::default())
        }
    }

    fn save_state(&self, scope: &str, state: &LeaseState) -> DraftResult<()> {
        write_json(&self.state_path(scope), state)
    }

    fn state_path(&self, scope: &str) -> PathBuf {
        self.root.join(format!("{}.json", safe_scope(scope)))
    }

    fn lock_path(&self, scope: &str) -> PathBuf {
        self.root.join(format!("{}.lock", safe_scope(scope)))
    }
}

/// A lease as the runtime refers to it.
///
/// Runtime-only by design: canonical facts record a `LeaseId` and a
/// `LeaseFence`, never this. Keeping the runtime shape out of the portable
/// contract is what stops lease lifecycle leaking into what an independent
/// verifier must understand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseRef {
    pub lease_id: draft_dcg_contract::LeaseId,
    pub owner: OperationId,
    pub scope: String,
    pub fence: draft_dcg_contract::LeaseFence,
    pub expires_at: Timestamp,
}

impl LeaseRef {
    /// The portable view of a held lease.
    pub fn of(lease: &FencedLease) -> DraftResult<Self> {
        Ok(Self {
            lease_id: draft_dcg_contract::LeaseId::parse(&lease.lease_id)
                .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
            owner: lease.operation_id.clone(),
            scope: lease.scope.clone(),
            fence: draft_dcg_contract::LeaseFence::new(lease.fencing_token),
            expires_at: lease.expires_at,
        })
    }

    /// Whether a write carrying `fence` may still act under this lease.
    ///
    /// Greater-or-equal, not equality: the holder's own fence is valid, and
    /// anything lower belongs to a lease that has since been superseded.
    ///
    /// Expiry is checked separately because the two failures mean different
    /// things to whoever hit them — "you took too long" and "somebody else
    /// already holds this" call for different responses.
    pub fn admits(&self, fence: draft_dcg_contract::LeaseFence) -> bool {
        fence >= self.fence
    }

    pub fn is_expired_at(&self, instant: Timestamp) -> bool {
        self.expires_at <= instant
    }
}

fn safe_scope(scope: &str) -> String {
    scope
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::LeaseFence;

    fn store(directory: &tempfile::TempDir) -> LeaseStore {
        LeaseStore::at(directory.path())
    }

    #[test]
    fn each_acquisition_allocates_a_strictly_greater_fence() {
        // The whole guarantee rests on this: if two acquisitions could share a
        // fence, a superseded holder's writes would be indistinguishable from
        // the current holder's.
        let directory = tempfile::tempdir().unwrap();
        let leases = store(&directory);

        let first = leases
            .acquire(
                "scope",
                OperationId::new("op_first"),
                chrono::Duration::seconds(-1),
            )
            .unwrap();
        let second = leases
            .acquire(
                "scope",
                OperationId::new("op_second"),
                chrono::Duration::minutes(5),
            )
            .unwrap();

        assert!(
            second.fencing_token > first.fencing_token,
            "a reacquisition must allocate a greater fence"
        );
    }

    #[test]
    fn a_superseded_holder_is_fenced_out_without_having_to_notice() {
        // The failure this prevents: a holder paused long enough for its lease
        // to expire and be taken by someone else. From the inside a pause is
        // indistinguishable from being fast, so the old holder cannot detect
        // its own staleness — the fence has to make its writes inert instead.
        let directory = tempfile::tempdir().unwrap();
        let leases = store(&directory);

        let old = leases
            .acquire(
                "scope",
                OperationId::new("op_old"),
                chrono::Duration::seconds(-1),
            )
            .unwrap();
        let new = leases
            .acquire(
                "scope",
                OperationId::new("op_new"),
                chrono::Duration::minutes(5),
            )
            .unwrap();

        let current = LeaseRef::of(&new).unwrap();
        assert!(
            current.admits(LeaseFence::new(new.fencing_token)),
            "the current holder's own fence is admitted"
        );
        assert!(
            !current.admits(LeaseFence::new(old.fencing_token)),
            "the superseded holder's fence is refused"
        );
    }

    #[test]
    fn expiry_and_fencing_are_different_answers() {
        // Both stop a write, and conflating them would tell the caller the
        // wrong thing: "you took too long" invites a retry, "somebody else
        // holds this" does not.
        let directory = tempfile::tempdir().unwrap();
        let leases = store(&directory);
        let lease = leases
            .acquire(
                "scope",
                OperationId::new("op_a"),
                chrono::Duration::minutes(5),
            )
            .unwrap();
        let held = LeaseRef::of(&lease).unwrap();

        assert!(!held.is_expired_at(now()), "a fresh lease has not expired");
        assert!(held.admits(held.fence), "and it admits its own fence");

        let expired = LeaseRef {
            expires_at: now() - chrono::Duration::seconds(1),
            ..held.clone()
        };
        assert!(expired.is_expired_at(now()));
        // Still admits its own fence: expiry is a separate question from
        // whether a newer holder has superseded it.
        assert!(expired.admits(expired.fence));
    }
}
