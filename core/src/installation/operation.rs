//! `InstallationOperation` — `<install_root>/.draft-install/operation.json`.
//!
//! A common header plus a closed payload per kind. `phase` is always the
//! *last durably completed* action, never an intent: each step performs its
//! action, validates the result, flushes it, and only then persists the new
//! phase. No arbitrary deletion path is ever journalled — every path is
//! re-derived from the validated roots, the fixed layout and the operation id.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::layout::InstallLayout;
use super::receipt::{ReleaseChannel, WindowsPathProvenance, WindowsUserPathValueOrigin};
use super::{
    fail, FaultPoint, Faults, Identity, InstallationFailure, InstallationId,
    InstallationOperationId,
};
use crate::support::common::Timestamp;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::process_lock::ProcessFileLock;

pub const OPERATION_SCHEMA_VERSION: u32 = 1;

/// The four operation kinds — one lifecycle machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    FreshInstall,
    BinaryUpdate,
    ChannelOnly,
    Uninstall,
}

/// Every phase any kind can reach. A kind's legal path is a subsequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Resolved,
    // BinaryUpdate
    Downloaded,
    Verified,
    Extracted,
    Staged,
    StagedBinariesValidated,
    DaemonStopped,
    BackupDraftCreated,
    BackupDraftdCreated,
    DraftReplaced,
    DraftdReplaced,
    InstalledBinariesValidated,
    DaemonRestarted,
    ReceiptCommitted,
    // FreshInstall
    RootBinariesInstalled,
    BinariesReceiptCommitted,
    LegacyPathStaged,
    PathIntegrationApplied,
    PathIntegrationReceiptCommitted,
    // Uninstall
    HelperStaged,
    PathIntegrationRemoved,
    DraftBinaryRemoved,
    DraftDaemonBinaryRemoved,
    GlobalStorePurged,
    ReceiptRemoved,
    // Shared tail
    Committed,
    CleanupPending,
    Finalized,
}

/// A legacy / managed PATH slot as the pre-install classifier found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotPreInstall {
    Missing,
    ExpectedManagedSymlink,
    AuthorizedLegacyFile,
}

/// One Unix PATH slot's durable provenance, fixed at `Resolved`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnixSlot {
    pub pre_install: SlotPreInstall,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_identity: Option<Identity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_staging_slot: Option<String>,
    /// This operation creates the managed symlink at the slot.
    pub created_by_operation: bool,
    /// This operation moves an authorized legacy file aside before creating it.
    pub moved_aside: bool,
}

/// Whether the canonical segment was already exposed before install (I75's
/// one-time classification). Immutable; never re-derived from current PATH.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentPreInstall {
    Absent,
    Present,
}

/// The platform-tagged pre-mutation state a `FreshInstall` journals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "snake_case", deny_unknown_fields)]
pub enum PathState {
    Unix {
        path_bin: String,
        draft_slot: UnixSlot,
        draftd_slot: UnixSlot,
        legacy_migration_authorized: bool,
    },
    Windows {
        path_update_requested: bool,
        value_pre_install: WindowsUserPathValueOrigin,
        segment_pre_install: SegmentPreInstall,
        provenance: WindowsPathProvenance,
    },
}

impl PathState {
    /// `LegacyPathStaged` is in the legal path only on Unix, with the opt-in,
    /// and when the validated pair actually requires migration.
    pub fn migrates_legacy(&self) -> bool {
        matches!(
            self,
            Self::Unix { legacy_migration_authorized: true, draft_slot, draftd_slot, .. }
                if draft_slot.moved_aside || draftd_slot.moved_aside
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshInstallPayload {
    pub install_root: String,
    pub platform_target: String,
    pub target_version: String,
    pub target_release_channel: ReleaseChannel,
    pub target_draft_identity: Identity,
    pub target_draftd_identity: Identity,
    pub initial_install_generation: u64,
    pub path_state: PathState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryUpdatePayload {
    pub previous_version: String,
    pub target_version: String,
    pub previous_generation: u64,
    pub target_generation: u64,
    pub previous_release_channel: ReleaseChannel,
    /// Present only when this update persistently changes the track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_release_channel: Option<ReleaseChannel>,
    pub previous_draft_identity: Identity,
    pub previous_draftd_identity: Identity,
    /// Known once the verified package is extracted; journalled with `Staged`
    /// and required before any replacement.
    pub target_draft_identity: Option<Identity>,
    pub target_draftd_identity: Option<Identity>,
    /// The release tag and artifact this operation authorized.
    pub target_tag: String,
    pub artifact_name: String,
    pub artifact: Identity,
    pub daemon_was_running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelOnlyPayload {
    pub previous_release_channel: ReleaseChannel,
    pub target_release_channel: ReleaseChannel,
    pub previous_generation: u64,
    pub target_generation: u64,
}

/// The validated path ownership an uninstall acts on, snapshotted from the
/// receipt at `Resolved` so the tail still knows it after `ReceiptRemoved`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UninstallSnapshot {
    pub draft_identity: Identity,
    pub draftd_identity: Identity,
    pub path_links: Vec<super::receipt::PathSymlinkEntry>,
    pub windows_path: Option<super::receipt::WindowsPathEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UninstallPayload {
    pub purge_requested: bool,
    pub snapshot: UninstallSnapshot,
    /// `staging/<operation_id>/draft[.exe]`, derived and re-checked.
    pub helper_path: String,
    /// The *expected* helper identity: the validated
    /// `receipt.draft_executable`, journalled at `Resolved` and never
    /// re-derived from the copy.
    pub helper_identity: Identity,
    pub daemon_was_running: bool,
    /// Present only when purge ownership was proven before `Resolved`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_store_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_store_id: Option<crate::support::common::GlobalStoreId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationPayload {
    FreshInstall(FreshInstallPayload),
    BinaryUpdate(BinaryUpdatePayload),
    ChannelOnly(ChannelOnlyPayload),
    Uninstall(UninstallPayload),
}

impl OperationPayload {
    pub fn kind(&self) -> OperationKind {
        match self {
            Self::FreshInstall(_) => OperationKind::FreshInstall,
            Self::BinaryUpdate(_) => OperationKind::BinaryUpdate,
            Self::ChannelOnly(_) => OperationKind::ChannelOnly,
            Self::Uninstall(_) => OperationKind::Uninstall,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallationOperation {
    pub schema_version: u32,
    pub operation_id: InstallationOperationId,
    pub installation_id: InstallationId,
    pub kind: OperationKind,
    pub phase: Phase,
    pub started_at: Timestamp,
    pub payload: OperationPayload,
}

impl InstallationOperation {
    pub fn new(
        installation_id: InstallationId,
        operation_id: InstallationOperationId,
        payload: OperationPayload,
    ) -> Self {
        Self {
            schema_version: OPERATION_SCHEMA_VERSION,
            operation_id,
            installation_id,
            kind: payload.kind(),
            phase: Phase::Resolved,
            started_at: crate::support::common::now(),
            payload,
        }
    }

    /// The kind's legal phase path, with its conditional phases resolved.
    pub fn legal_path(&self) -> Vec<Phase> {
        use Phase::*;
        match &self.payload {
            OperationPayload::BinaryUpdate(_) => vec![
                Resolved,
                Downloaded,
                Verified,
                Extracted,
                Staged,
                StagedBinariesValidated,
                DaemonStopped,
                BackupDraftCreated,
                BackupDraftdCreated,
                DraftReplaced,
                DraftdReplaced,
                InstalledBinariesValidated,
                DaemonRestarted,
                ReceiptCommitted,
                Committed,
                CleanupPending,
                Finalized,
            ],
            OperationPayload::ChannelOnly(_) => {
                vec![
                    Resolved,
                    ReceiptCommitted,
                    Committed,
                    CleanupPending,
                    Finalized,
                ]
            }
            OperationPayload::FreshInstall(payload) => {
                let mut path = vec![Resolved, RootBinariesInstalled, BinariesReceiptCommitted];
                if payload.path_state.migrates_legacy() {
                    path.push(LegacyPathStaged);
                }
                path.extend([
                    PathIntegrationApplied,
                    PathIntegrationReceiptCommitted,
                    InstalledBinariesValidated,
                    Committed,
                    CleanupPending,
                    Finalized,
                ]);
                path
            }
            OperationPayload::Uninstall(payload) => {
                let mut path = vec![
                    Resolved,
                    HelperStaged,
                    DaemonStopped,
                    PathIntegrationRemoved,
                    DraftBinaryRemoved,
                    DraftDaemonBinaryRemoved,
                ];
                if payload.purge_requested {
                    path.push(GlobalStorePurged);
                }
                path.extend([ReceiptRemoved, Committed, CleanupPending, Finalized]);
                path
            }
        }
    }

    pub fn is_on_path(&self, phase: Phase) -> bool {
        self.legal_path().contains(&phase)
    }

    /// The phase after the current one, if any.
    pub fn next_phase(&self) -> Option<Phase> {
        let path = self.legal_path();
        let index = path.iter().position(|phase| *phase == self.phase)?;
        path.get(index + 1).copied()
    }

    /// Whether the current phase is at or beyond `phase` on this kind's path.
    pub fn reached(&self, phase: Phase) -> bool {
        let path = self.legal_path();
        match (
            path.iter().position(|p| *p == self.phase),
            path.iter().position(|p| *p == phase),
        ) {
            (Some(current), Some(target)) => current >= target,
            _ => false,
        }
    }

    /// Authoritative structural validation of a loaded journal.
    pub fn validate(&self, layout: &InstallLayout) -> DraftResult<()> {
        let invalid = |why: &str| {
            fail(
                InstallationFailure::RecoveryFailed,
                format!("operation.json is invalid: {why}"),
            )
        };
        if self.schema_version != OPERATION_SCHEMA_VERSION {
            return Err(fail(
                InstallationFailure::InstallationRecoveryIncompatible,
                format!(
                    "operation.json schema {} is not one this Draft supports",
                    self.schema_version
                ),
            ));
        }
        if !InstallationId::is_well_formed(self.installation_id.as_str())
            || !InstallationOperationId::is_well_formed(self.operation_id.as_str())
        {
            return Err(invalid("malformed identifiers"));
        }
        if self.kind != self.payload.kind() {
            return Err(invalid("kind disagrees with payload"));
        }
        if !self.is_on_path(self.phase) {
            return Err(invalid("phase is not on this kind's legal path"));
        }
        match &self.payload {
            OperationPayload::FreshInstall(payload) => {
                if std::path::Path::new(&payload.install_root) != layout.root()
                    || payload.initial_install_generation != 1
                {
                    return Err(invalid("FreshInstall root or generation"));
                }
            }
            OperationPayload::Uninstall(payload) => {
                if std::path::Path::new(&payload.helper_path) != layout.helper(&self.operation_id) {
                    return Err(invalid("helper_path is not the derived helper slot"));
                }
                if payload.purge_requested != payload.global_store_root.is_some()
                    || payload.global_store_root.is_some() != payload.global_store_id.is_some()
                {
                    return Err(invalid("purge target"));
                }
            }
            OperationPayload::BinaryUpdate(payload) => {
                if payload.target_generation != payload.previous_generation + 1 {
                    return Err(invalid("generation"));
                }
            }
            OperationPayload::ChannelOnly(payload) => {
                if payload.target_generation != payload.previous_generation + 1 {
                    return Err(invalid("generation"));
                }
            }
        }
        Ok(())
    }
}

/// Read the journal if present. Unparseable is `InstallationRecoveryIncompatible`
/// when its schema is foreign, `RecoveryFailed` otherwise.
pub fn read(layout: &InstallLayout) -> DraftResult<Option<InstallationOperation>> {
    let bytes = match std::fs::read(layout.operation()) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DraftError::storage(format!("read operation.json: {error}"))),
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        fail(
            InstallationFailure::RecoveryFailed,
            format!("operation.json is not JSON: {error}"),
        )
    })?;
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(OPERATION_SCHEMA_VERSION))
    {
        return Err(fail(
            InstallationFailure::InstallationRecoveryIncompatible,
            "operation.json uses a schema this Draft does not support",
        ));
    }
    let known_kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|kind| {
            [
                "fresh_install",
                "binary_update",
                "channel_only",
                "uninstall",
            ]
            .contains(&kind)
        });
    if !known_kind {
        return Err(fail(
            InstallationFailure::InstallationRecoveryIncompatible,
            "operation.json names an operation kind this Draft does not know",
        ));
    }
    serde_json::from_value(value).map(Some).map_err(|error| {
        fail(
            InstallationFailure::RecoveryFailed,
            format!("operation.json is malformed: {error}"),
        )
    })
}

/// Acquire the permanent lifecycle lock. Busy is `InstallationBusy`.
pub fn lock(layout: &InstallLayout, timeout: Duration) -> DraftResult<ProcessFileLock> {
    layout.ensure_skeleton()?;
    ProcessFileLock::acquire_exclusive(&layout.lock(), timeout).map_err(|error| {
        if error.kind == DraftErrorKind::LockTimeout {
            fail(
                InstallationFailure::InstallationBusy,
                "another Draft lifecycle operation holds this installation's lock",
            )
            .with_suggestion("Wait for it to finish, then run the command again.")
        } else {
            error
        }
    })
}

/// The live journal of one operation, written under the held lock.
pub struct Journal<'a> {
    pub layout: &'a InstallLayout,
    pub op: InstallationOperation,
    pub faults: &'a dyn Faults,
}

impl<'a> Journal<'a> {
    /// Durably create the journal at `Resolved`.
    pub fn create(
        layout: &'a InstallLayout,
        op: InstallationOperation,
        faults: &'a dyn Faults,
    ) -> DraftResult<Self> {
        faults.at(FaultPoint::BeforeAction(Phase::Resolved))?;
        super::write_private_json(&layout.operation(), &op)?;
        faults.at(FaultPoint::AfterPhase(Phase::Resolved))?;
        Ok(Self { layout, op, faults })
    }

    pub fn resume(
        layout: &'a InstallLayout,
        op: InstallationOperation,
        faults: &'a dyn Faults,
    ) -> Self {
        Self { layout, op, faults }
    }

    pub fn phase(&self) -> Phase {
        self.op.phase
    }

    /// Run `action` as the step into `phase`: fault points before, after the
    /// action, and after the durable phase write.
    pub fn step(
        &mut self,
        phase: Phase,
        action: impl FnOnce(&mut Self) -> DraftResult<()>,
    ) -> DraftResult<()> {
        self.faults.at(FaultPoint::BeforeAction(phase))?;
        action(self)?;
        self.faults.at(FaultPoint::AfterActionBeforePhase(phase))?;
        self.advance(phase)
    }

    /// Persist `phase` as the last durably completed action. Must be the next
    /// phase on the kind's legal path.
    pub fn advance(&mut self, phase: Phase) -> DraftResult<()> {
        self.advance_with(phase, |_| {})
    }

    /// Persist `phase` together with facts the completed action established
    /// (for example the staged target identities).
    pub fn advance_with(
        &mut self,
        phase: Phase,
        record: impl FnOnce(&mut OperationPayload),
    ) -> DraftResult<()> {
        if self.op.next_phase() != Some(phase) {
            return Err(fail(
                InstallationFailure::RecoveryFailed,
                format!(
                    "{:?} does not follow {:?} for this operation",
                    phase, self.op.phase
                ),
            ));
        }
        let mut next = self.op.clone();
        next.phase = phase;
        record(&mut next.payload);
        super::write_private_json(&self.layout.operation(), &next)?;
        self.op = next;
        self.faults.at(FaultPoint::AfterPhase(phase))
    }

    /// Remove `operation.json`: no semantic decision remains.
    pub fn delete(self) -> DraftResult<()> {
        super::remove_file_if_present(&self.layout.operation())?;
        crate::support::fsutil::sync_directory(&self.layout.lifecycle_dir())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installation::receipt::fixtures::identity;
    use crate::installation::InstallPlatform;

    pub(crate) fn uninstall(purge: bool, layout: &InstallLayout) -> InstallationOperation {
        let op = InstallationOperationId::new("ilo_0123456789ab");
        InstallationOperation::new(
            InstallationId::new("ins_0123456789ab"),
            op.clone(),
            OperationPayload::Uninstall(UninstallPayload {
                purge_requested: purge,
                snapshot: UninstallSnapshot {
                    draft_identity: identity("draft"),
                    draftd_identity: identity("draftd"),
                    path_links: vec![],
                    windows_path: None,
                },
                helper_path: layout.helper(&op).display().to_string(),
                helper_identity: identity("draft"),
                daemon_was_running: false,
                global_store_root: purge.then(|| "/home/ada/.draft".into()),
                global_store_id: purge
                    .then(|| crate::support::common::GlobalStoreId::new("gst_0123456789ab")),
            }),
        )
    }

    #[test]
    fn conditional_phases_are_legal_only_when_their_condition_holds() {
        let layout = InstallLayout::new("/opt/root", InstallPlatform::Unix);
        assert!(!uninstall(false, &layout).is_on_path(Phase::GlobalStorePurged));
        assert!(uninstall(true, &layout).is_on_path(Phase::GlobalStorePurged));
        let mut op = uninstall(false, &layout);
        op.phase = Phase::DraftDaemonBinaryRemoved;
        assert_eq!(op.next_phase(), Some(Phase::ReceiptRemoved));
        let mut op = uninstall(true, &layout);
        op.phase = Phase::DraftDaemonBinaryRemoved;
        assert_eq!(op.next_phase(), Some(Phase::GlobalStorePurged));
        op.validate(&layout).unwrap();
    }

    #[test]
    fn a_journal_advances_only_along_its_legal_path() {
        let dir = tempfile::tempdir().unwrap();
        let layout = InstallLayout::new(dir.path(), InstallPlatform::Unix);
        layout.ensure_skeleton().unwrap();
        let mut journal = Journal::create(
            &layout,
            uninstall(false, &layout),
            &crate::installation::NoFaults,
        )
        .unwrap();
        assert!(journal.advance(Phase::DaemonStopped).is_err());
        journal.advance(Phase::HelperStaged).unwrap();
        let loaded = read(&layout).unwrap().unwrap();
        assert_eq!(loaded.phase, Phase::HelperStaged);
        loaded.validate(&layout).unwrap();
    }

    #[test]
    fn a_foreign_schema_or_kind_is_incompatible_and_garbage_is_a_recovery_failure() {
        let dir = tempfile::tempdir().unwrap();
        let layout = InstallLayout::new(dir.path(), InstallPlatform::Unix);
        layout.ensure_skeleton().unwrap();
        let mut value = serde_json::to_value(uninstall(false, &layout)).unwrap();
        value["schema_version"] = serde_json::json!(2);
        std::fs::write(layout.operation(), value.to_string()).unwrap();
        assert_eq!(
            crate::installation::failure_of(&read(&layout).unwrap_err()),
            Some(InstallationFailure::InstallationRecoveryIncompatible)
        );
        value["schema_version"] = serde_json::json!(1);
        value["kind"] = serde_json::json!("self_destruct");
        std::fs::write(layout.operation(), value.to_string()).unwrap();
        assert_eq!(
            crate::installation::failure_of(&read(&layout).unwrap_err()),
            Some(InstallationFailure::InstallationRecoveryIncompatible)
        );
        std::fs::write(layout.operation(), b"{").unwrap();
        assert_eq!(
            crate::installation::failure_of(&read(&layout).unwrap_err()),
            Some(InstallationFailure::RecoveryFailed)
        );
    }

    #[test]
    fn a_second_holder_is_busy_and_the_lock_path_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let layout = InstallLayout::new(dir.path(), InstallPlatform::Unix);
        let held = lock(&layout, Duration::from_millis(50)).unwrap();
        let busy = std::thread::scope(|scope| {
            scope
                .spawn(|| lock(&layout, Duration::from_millis(50)).map(|_| ()))
                .join()
                .unwrap()
        });
        assert_eq!(
            crate::installation::failure_of(&busy.unwrap_err()),
            Some(InstallationFailure::InstallationBusy)
        );
        drop(held);
        assert!(layout.lock().exists(), "the lock sidecar is never removed");
    }
}
