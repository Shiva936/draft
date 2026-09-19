//! `draft uninstall` — forward-only, with a surviving executor.
//!
//! ```text
//! Resolved → HelperStaged → DaemonStopped → PathIntegrationRemoved →
//! DraftBinaryRemoved → DraftDaemonBinaryRemoved → [GlobalStorePurged] →
//! ReceiptRemoved → Committed → CleanupPending → Finalized
//! ```
//!
//! No phase is rollbackable and no backup is kept: once `Resolved` exists,
//! recovery drives forward. Before the first deletion an identity-verified
//! copy of `draft` is staged at `staging/<operation_id>/draft[.exe]` together
//! with `bootstrap.recovery`; that helper — never the binaries it deletes —
//! finishes the tail, and after a reboot the official installer relaunches it.
//! The helper takes only installation and operation ids, never a path, and
//! re-derives every slot from its own canonical location.
//!
//! Default uninstall removes only the receipt-owned PATH exposure, both
//! identity-validated binaries, operation-owned staging/rollback and the
//! receipt. It preserves every project (no `.draft/` is ever scanned), the
//! global user store (unless `--purge` proves its ownership), and the
//! permanent inert `<install_root>/.draft-install/lifecycle.lock`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use super::bootstrap::BootstrapRecord;
use super::layout::{self, Executable, InstallLayout};
use super::operation::{
    self, InstallationOperation, Journal, OperationPayload, Phase, UninstallPayload,
    UninstallSnapshot,
};
use super::path::{unix, windows};
use super::receipt::{InstallationReceipt, WindowsPathProvenance};
use super::terminal::TerminalRecord;
use super::{
    fail, is_injected_crash, Faults, Identity, InstallPlatform, InstallationFailure,
    InstallationId, InstallationOperationId, LifecycleHost, LIFECYCLE_LOCK_TIMEOUT,
};
use crate::project::home::DraftGlobalStore;
use crate::support::common::GlobalStoreId;
use crate::support::error::{DraftError, DraftResult};

/// How long a helper waits for its parent before giving up.
pub const PARENT_EXIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Everything an uninstall needs from its surroundings.
pub struct Context<'a> {
    pub layout: &'a InstallLayout,
    pub host: &'a dyn LifecycleHost,
    /// The Windows User PATH; `None` on Unix.
    pub registry: Option<&'a dyn windows::UserPathRegistry>,
    pub faults: &'a dyn Faults,
}

/// A global store whose ownership was proven before the journal existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenStore {
    pub root: PathBuf,
    pub store_id: GlobalStoreId,
}

fn unsafe_purge(why: String) -> DraftError {
    fail(InstallationFailure::GlobalStorePurgeUnsafe, why)
}

/// I51: canonicalize, reject every dangerous root, and require a valid
/// `home.json` marker. `--yes` waives only the prompt, never any of this.
pub fn prove_global_store(root: &Path, install: &InstallLayout) -> DraftResult<ProvenStore> {
    if !root.exists() {
        return Err(fail(
            InstallationFailure::GlobalStoreOwnershipInvalid,
            format!("there is no Draft global store at {}", root.display()),
        ));
    }
    let canonical = layout::canonicalize(root)?;
    let install_root =
        layout::canonicalize(install.root()).unwrap_or_else(|_| install.root().to_path_buf());
    if layout::is_dangerous_root(&canonical) {
        return Err(unsafe_purge(format!(
            "{} can never be purged",
            canonical.display()
        )));
    }
    if canonical.starts_with(&install_root) || install_root.starts_with(&canonical) {
        return Err(unsafe_purge(format!(
            "{} overlaps the installation root {}",
            canonical.display(),
            install_root.display()
        )));
    }
    // A project root, a project `.draft/` or a repository root is never a store.
    let project_markers = [".draft", ".git", "project", "events", "graph"];
    if let Some(marker) = project_markers
        .iter()
        .find(|marker| canonical.join(marker).exists())
    {
        return Err(unsafe_purge(format!(
            "{} looks like a project or repository ({marker} is present)",
            canonical.display()
        )));
    }
    let marker = DraftGlobalStore::at(&canonical)
        .read_home_marker()
        .map_err(|error| {
            fail(
                InstallationFailure::GlobalStoreOwnershipInvalid,
                format!("{}: {}", canonical.display(), error.message),
            )
        })?
        .ok_or_else(|| {
            fail(
                InstallationFailure::GlobalStoreOwnershipInvalid,
                format!(
                    "{} carries no Draft store marker (home.json), so it is not proven to be a \
                     Draft-managed store and will not be purged",
                    canonical.display()
                ),
            )
            .with_suggestion(
                "Uninstall without --purge, then remove the directory yourself if it is yours.",
            )
        })?;
    Ok(ProvenStore {
        root: canonical,
        store_id: marker.store_id,
    })
}

/// The PATH part of an uninstall plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "platform", rename_all = "snake_case")]
pub enum PathPlan {
    Unix {
        remove_links: Vec<String>,
    },
    Windows {
        provenance: WindowsPathProvenance,
        segment: String,
        /// The human line: preserved, or exactly what is removed.
        summary: String,
        removes_segment: bool,
        deletes_value: bool,
    },
}

/// The plan `--dry-run` prints and the real uninstall executes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UninstallPlan {
    pub install_root: String,
    pub installation_id: String,
    pub remove_executables: Vec<String>,
    pub path: PathPlan,
    pub remove_lifecycle_state: Vec<String>,
    pub remains: String,
    pub preserved: Vec<String>,
    pub purge: Option<String>,
}

pub fn plan(
    layout: &InstallLayout,
    receipt: &InstallationReceipt,
    registry: Option<&dyn windows::UserPathRegistry>,
    purge: Option<&ProvenStore>,
    global_store: &Path,
) -> DraftResult<UninstallPlan> {
    let path = match layout.platform() {
        InstallPlatform::Unix => PathPlan::Unix {
            remove_links: receipt
                .path_links
                .iter()
                .map(|link| link.link_path.clone())
                .collect(),
        },
        InstallPlatform::Windows => {
            let entry = receipt.windows_path.clone().ok_or_else(|| {
                fail(
                    InstallationFailure::UninstallPlanInvalid,
                    "no windows_path decision",
                )
            })?;
            let (summary, removes, deletes) = match entry.provenance {
                WindowsPathProvenance::NotManaged => {
                    ("PATH: not managed (preserved)".to_string(), false, false)
                }
                WindowsPathProvenance::PreExisting => {
                    ("PATH: pre-existing (preserved)".to_string(), false, false)
                }
                WindowsPathProvenance::AddedByDraft => {
                    let registry = registry.ok_or_else(|| {
                        fail(
                            InstallationFailure::UninstallPlanInvalid,
                            "the User PATH is unavailable",
                        )
                    })?;
                    match windows::undo_case(
                        &registry.read()?,
                        &entry.segment,
                        entry.value_pre_install,
                    ) {
                        windows::UndoCase::DeleteValue => (
                            format!(
                                "PATH: remove {} (the User Path value itself is deleted)",
                                entry.segment
                            ),
                            true,
                            true,
                        ),
                        windows::UndoCase::RemoveToken => {
                            (format!("PATH: remove {}", entry.segment), true, false)
                        }
                        windows::UndoCase::Absent | windows::UndoCase::AlreadyUndone => (
                            "PATH: Draft's entry is already gone".to_string(),
                            false,
                            false,
                        ),
                        windows::UndoCase::Ambiguous => {
                            return Err(fail(
                                InstallationFailure::UninstallPlanInvalid,
                                "the User PATH holds a duplicate or ambiguous copy of Draft's \
                                 segment; nothing will be removed",
                            ))
                        }
                    }
                }
            };
            PathPlan::Windows {
                provenance: entry.provenance,
                segment: entry.segment,
                summary,
                removes_segment: removes,
                deletes_value: deletes,
            }
        }
    };
    let mut preserved =
        vec!["every project (.draft/ directories are never scanned or deleted)".to_string()];
    if purge.is_none() {
        preserved.push(format!(
            "the global user store at {}",
            global_store.display()
        ));
    }
    Ok(UninstallPlan {
        install_root: layout.root().display().to_string(),
        installation_id: receipt.installation_id.to_string(),
        remove_executables: vec![
            layout.executable(Executable::Draft).display().to_string(),
            layout.executable(Executable::Draftd).display().to_string(),
        ],
        path,
        remove_lifecycle_state: vec![
            layout.receipt().display().to_string(),
            "the operation journal, helper and terminal records of this uninstall".into(),
        ],
        remains: layout.lock().display().to_string(),
        preserved,
        purge: purge.map(|store| store.root.display().to_string()),
    })
}

fn payload(journal: &Journal<'_>) -> UninstallPayload {
    match &journal.op.payload {
        OperationPayload::Uninstall(payload) => payload.clone(),
        _ => unreachable!("the uninstall engine runs Uninstall journals only"),
    }
}

/// Start an uninstall: validate everything, journal `Resolved` with the
/// expected helper identity, stage the helper and its bootstrap record, and
/// hand off. Returns the operation id; the helper finishes the rest.
pub fn begin(
    ctx: &Context<'_>,
    receipt: &InstallationReceipt,
    purge: Option<ProvenStore>,
) -> DraftResult<InstallationOperationId> {
    let layout = ctx.layout;
    if !receipt
        .draft_executable
        .identity()
        .matches(&layout.executable(Executable::Draft))
    {
        return Err(fail(
            InstallationFailure::InstallationEntryIdentityMismatch,
            "the installed draft does not match its receipt; nothing was changed",
        ));
    }
    let operation_id = InstallationOperationId::generate();
    let op = InstallationOperation::new(
        receipt.installation_id.clone(),
        operation_id.clone(),
        OperationPayload::Uninstall(UninstallPayload {
            purge_requested: purge.is_some(),
            snapshot: UninstallSnapshot {
                draft_identity: receipt.draft_executable.identity(),
                draftd_identity: receipt.draftd_executable.identity(),
                path_links: receipt.path_links.clone(),
                windows_path: receipt.windows_path.clone(),
            },
            helper_path: layout.helper(&operation_id).display().to_string(),
            helper_identity: receipt.draft_executable.identity(),
            daemon_was_running: ctx.host.daemon_running(),
            global_store_root: purge.as_ref().map(|store| store.root.display().to_string()),
            global_store_id: purge.map(|store| store.store_id),
        }),
    );
    let mut journal = Journal::create(layout, op, ctx.faults)?;
    stage_helper(&mut journal, &layout.executable(Executable::Draft))?;
    ctx.host.launch_helper(
        &layout.helper(&operation_id),
        &receipt.installation_id,
        &operation_id,
    )?;
    Ok(operation_id)
}

fn bootstrap_of(journal: &Journal<'_>) -> BootstrapRecord {
    BootstrapRecord {
        installation_id: journal.op.installation_id.clone(),
        operation_id: journal.op.operation_id.clone(),
        helper: payload(journal).helper_identity,
    }
}

/// Whether the helper copy and a matching bootstrap record both exist.
fn helper_staged(journal: &Journal<'_>) -> DraftResult<bool> {
    let layout = journal.layout;
    let p = payload(journal);
    let helper = layout.helper(&journal.op.operation_id);
    let helper_ok = p.helper_identity.matches(&helper);
    let record = match std::fs::read(layout.bootstrap_record()) {
        Ok(bytes) => Some(BootstrapRecord::parse(&bytes).map_err(|_| {
            fail(
                InstallationFailure::UninstallRecoveryFailed,
                "bootstrap.recovery is malformed",
            )
        })?),
        Err(_) => None,
    };
    if let Some(record) = &record {
        if *record != bootstrap_of(journal) {
            return Err(fail(
                InstallationFailure::UninstallRecoveryFailed,
                "bootstrap.recovery disagrees with operation.json",
            ));
        }
    }
    if helper.exists() && !helper_ok {
        return Err(fail(
            InstallationFailure::UninstallRecoveryFailed,
            "the staged helper does not match its expected identity",
        ));
    }
    Ok(helper_ok && record.is_some())
}

/// `HelperStaged`: copy the helper from `source`, verify it against the
/// expected identity journalled at `Resolved`, write and verify the bootstrap
/// record. Idempotent, so a partial earlier attempt is simply completed.
pub(crate) fn stage_helper(journal: &mut Journal<'_>, source: &Path) -> DraftResult<()> {
    if journal.phase() != Phase::Resolved {
        return Ok(());
    }
    if helper_staged(journal)? {
        return journal.advance(Phase::HelperStaged);
    }
    journal.step(Phase::HelperStaged, |journal| {
        let layout = journal.layout;
        let p = payload(journal);
        let helper = layout.helper(&journal.op.operation_id);
        if !p.helper_identity.matches(&helper) {
            if !p.helper_identity.matches(source) {
                return Err(fail(
                    InstallationFailure::UninstallRecoveryExecutorUnavailable,
                    "no identity-valid draft remains to stage the uninstall helper from",
                ));
            }
            crate::support::fsutil::ensure_dir(&layout.staging(&journal.op.operation_id))?;
            let _ = crate::support::hidden::restrict_dir(&layout.staging_root(), 0o700);
            super::remove_file_if_present(&helper)?;
            std::fs::copy(source, &helper)
                .map_err(|error| DraftError::storage(format!("stage helper: {error}")))?;
            std::fs::File::open(&helper)
                .and_then(|file| file.sync_all())
                .map_err(|error| DraftError::storage(format!("sync helper: {error}")))?;
            if !p.helper_identity.matches(&helper) {
                return Err(fail(
                    InstallationFailure::UninstallRecoveryFailed,
                    "the staged helper copy does not match its expected identity",
                ));
            }
        }
        let record = bootstrap_of(journal);
        super::write_private_bytes(&layout.bootstrap_record(), record.render().as_bytes())?;
        let written = BootstrapRecord::parse(&std::fs::read(layout.bootstrap_record())?)?;
        if written != record {
            return Err(fail(
                InstallationFailure::UninstallRecoveryFailed,
                "bootstrap.recovery did not read back as written",
            ));
        }
        Ok(())
    })
}

/// How the helper was started (I59). The authority checks are identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperMode {
    /// Launched by a live `draft`; wait for that exact parent to exit.
    ParentExit { parent_pid: u32 },
    /// Launched by the installer after a reboot; wait for nobody.
    BootstrapRecovery,
}

/// Derive `<install_root>` from the helper's own canonical location:
/// `<root>/.draft-install/staging/<operation_id>/draft[.exe]`.
pub fn layout_of_helper(
    executable: &Path,
    operation: &InstallationOperationId,
    platform: InstallPlatform,
) -> DraftResult<InstallLayout> {
    let canonical = layout::canonicalize(executable)?;
    let refused = || {
        fail(
            InstallationFailure::UninstallRecoveryFailed,
            format!(
                "{} is not a staged lifecycle helper for {operation}",
                canonical.display()
            ),
        )
    };
    let op_dir = canonical.parent().ok_or_else(refused)?;
    let staging = op_dir.parent().ok_or_else(refused)?;
    let lifecycle = staging.parent().ok_or_else(refused)?;
    let root = lifecycle.parent().ok_or_else(refused)?;
    let named = |path: &Path, name: &str| {
        path.file_name()
            .is_some_and(|n| n.to_string_lossy() == name)
    };
    if !named(op_dir, operation.as_str())
        || !named(staging, "staging")
        || !named(lifecycle, ".draft-install")
        || !named(&canonical, &format!("draft{}", platform.exe_suffix()))
    {
        return Err(refused());
    }
    Ok(InstallLayout::new(root, platform))
}

/// The lifecycle helper (I31): the same authority checks in both modes, then
/// only the legal remaining phases of the recorded `Uninstall`.
pub fn run_helper(
    ctx: &Context<'_>,
    helper_executable: &Path,
    installation: &InstallationId,
    operation: &InstallationOperationId,
    mode: HelperMode,
) -> DraftResult<()> {
    let layout = ctx.layout;
    if let HelperMode::ParentExit { parent_pid } = mode {
        ctx.host.wait_for_exit(parent_pid, PARENT_EXIT_TIMEOUT)?;
    }
    let lock = operation::lock(layout, LIFECYCLE_LOCK_TIMEOUT)?;
    let refuse = |why: &str| {
        fail(
            InstallationFailure::UninstallRecoveryFailed,
            why.to_string(),
        )
    };
    let op = operation::read(layout)?.ok_or_else(|| refuse("there is no operation to continue"))?;
    op.validate(layout)?;
    if op.kind != operation::OperationKind::Uninstall {
        return Err(refuse("the recorded operation is not an uninstall"));
    }
    if &op.installation_id != installation || &op.operation_id != operation {
        return Err(refuse(
            "the operation belongs to another installation or run",
        ));
    }
    let OperationPayload::Uninstall(p) = &op.payload else {
        unreachable!("validated as Uninstall")
    };
    if layout::canonicalize(helper_executable)? != layout::canonicalize(&layout.helper(operation))?
        || !p.helper_identity.matches(helper_executable)
    {
        return Err(refuse(
            "this helper is not the identity-verified executor the operation authorized",
        ));
    }
    if mode == HelperMode::BootstrapRecovery {
        let record = std::fs::read(layout.bootstrap_record())
            .map_err(|_| {
                fail(
                    InstallationFailure::UninstallRecoveryBootstrapFailed,
                    "bootstrap.recovery is missing",
                )
            })
            .and_then(|bytes| BootstrapRecord::parse(&bytes))?;
        if record.installation_id != op.installation_id
            || record.operation_id != op.operation_id
            || record.helper != p.helper_identity
        {
            return Err(fail(
                InstallationFailure::UninstallRecoveryBootstrapFailed,
                "bootstrap.recovery disagrees with operation.json",
            ));
        }
    }
    let journal = Journal::resume(layout, op, ctx.faults);
    let result = drive(ctx, journal);
    drop(lock);
    // Zero filesystem mutation after the lock is released.
    result
}

/// Run every remaining phase, advancing past actions that already completed.
pub(crate) fn drive(ctx: &Context<'_>, mut journal: Journal<'_>) -> DraftResult<()> {
    let layout = ctx.layout;
    let operation = journal.op.operation_id.clone();
    let p = payload(&journal);
    if journal.phase() == Phase::Resolved {
        stage_helper(&mut journal, &layout.executable(Executable::Draft))?;
    }
    while let Some(next) = journal.op.next_phase() {
        if probe(ctx, &journal, next)? {
            // The action already completed: only the phase write remains, and
            // the same crash windows apply to it.
            ctx.faults.at(super::FaultPoint::BeforeAction(next))?;
            ctx.faults
                .at(super::FaultPoint::AfterActionBeforePhase(next))?;
            journal.advance(next)?;
            continue;
        }
        journal.step(next, |journal| act(ctx, journal, next, &p))?;
    }
    // `Finalized` is durable: no semantic decision remains.
    let record = TerminalRecord {
        installation_id: journal.op.installation_id.clone(),
        operation_id: operation.clone(),
        mode: layout.platform(),
    };
    journal.delete()?;
    ctx.faults.at(super::FaultPoint::AfterJournalDeleted)?;
    terminal_tail(layout, &record);
    Ok(())
}

fn owned_links(p: &UninstallPayload) -> Vec<(PathBuf, PathBuf)> {
    p.snapshot
        .path_links
        .iter()
        .map(|link| {
            (
                PathBuf::from(&link.link_path),
                PathBuf::from(&link.expected_target),
            )
        })
        .collect()
}

fn registry<'a>(ctx: &Context<'a>) -> DraftResult<&'a dyn windows::UserPathRegistry> {
    ctx.registry.ok_or_else(|| {
        fail(
            InstallationFailure::UninstallPlanInvalid,
            "the User PATH is unavailable",
        )
    })
}

/// The liminal probe for `next` (I44). `Err` means ambiguous: fail closed.
fn probe(ctx: &Context<'_>, journal: &Journal<'_>, next: Phase) -> DraftResult<bool> {
    let layout = ctx.layout;
    let p = payload(journal);
    let failed = |why: String| fail(InstallationFailure::UninstallRecoveryFailed, why);
    match next {
        Phase::HelperStaged => helper_staged(journal),
        Phase::DaemonStopped => Ok(!ctx.host.daemon_running()),
        Phase::PathIntegrationRemoved => match layout.platform() {
            InstallPlatform::Unix => {
                let mut all_absent = true;
                for (link, target) in owned_links(&p) {
                    match unix::observe(&link, &target) {
                        unix::SlotObservation::Missing => {}
                        unix::SlotObservation::ExpectedSymlink => all_absent = false,
                        _ => {
                            return Err(failed(format!(
                                "{} is no longer Draft's link",
                                link.display()
                            )))
                        }
                    }
                }
                Ok(all_absent)
            }
            InstallPlatform::Windows => {
                let entry = p
                    .snapshot
                    .windows_path
                    .clone()
                    .ok_or_else(|| failed("no windows_path".into()))?;
                if entry.provenance != WindowsPathProvenance::AddedByDraft {
                    return Ok(true);
                }
                match windows::undo_case(
                    &registry(ctx)?.read()?,
                    &entry.segment,
                    entry.value_pre_install,
                ) {
                    windows::UndoCase::Absent | windows::UndoCase::AlreadyUndone => Ok(true),
                    windows::UndoCase::DeleteValue | windows::UndoCase::RemoveToken => Ok(false),
                    windows::UndoCase::Ambiguous => Err(fail(
                        InstallationFailure::UninstallPlanInvalid,
                        "the User PATH holds a duplicate or ambiguous copy of Draft's segment",
                    )),
                }
            }
        },
        Phase::DraftBinaryRemoved | Phase::DraftDaemonBinaryRemoved => {
            let (executable, identity) = if next == Phase::DraftBinaryRemoved {
                (Executable::Draft, &p.snapshot.draft_identity)
            } else {
                (Executable::Draftd, &p.snapshot.draftd_identity)
            };
            let slot = layout.executable(executable);
            if std::fs::symlink_metadata(&slot).is_err() {
                Ok(true)
            } else if identity.matches(&slot) {
                Ok(false)
            } else {
                Err(failed(format!(
                    "{} is not the binary this installation owns; nothing was deleted",
                    slot.display()
                )))
            }
        }
        Phase::GlobalStorePurged => Ok(p
            .global_store_root
            .as_ref()
            .is_none_or(|root| !Path::new(root).exists())),
        Phase::ReceiptRemoved => Ok(!layout.receipt().exists()),
        Phase::Finalized => match std::fs::read(layout.terminal_record()) {
            Err(_) => Ok(false),
            Ok(bytes) => {
                let record = TerminalRecord::parse(&bytes)
                    .map_err(|_| failed("terminal-cleanup is malformed".into()))?;
                if record.installation_id == journal.op.installation_id
                    && record.operation_id == journal.op.operation_id
                    && record.mode == layout.platform()
                {
                    Ok(true)
                } else {
                    Err(failed("terminal-cleanup names another operation".into()))
                }
            }
        },
        _ => Ok(false),
    }
}

/// The action that establishes `next`.
fn act(
    ctx: &Context<'_>,
    journal: &Journal<'_>,
    next: Phase,
    p: &UninstallPayload,
) -> DraftResult<()> {
    let layout = ctx.layout;
    let operation = &journal.op.operation_id;
    match next {
        Phase::DaemonStopped => {
            if ctx.host.daemon_running() {
                ctx.host
                    .stop_daemon()
                    .map_err(|error| fail(InstallationFailure::DaemonStopFailed, error.message))?;
            }
            Ok(())
        }
        Phase::PathIntegrationRemoved => match layout.platform() {
            InstallPlatform::Unix => {
                for (link, target) in owned_links(p) {
                    if target != layout.executable(Executable::Draft)
                        && target != layout.executable(Executable::Draftd)
                    {
                        return Err(fail(
                            InstallationFailure::UninstallPlanInvalid,
                            "a link targets no owned slot",
                        ));
                    }
                    unix::remove(&link, &target)?;
                }
                Ok(())
            }
            InstallPlatform::Windows => {
                let entry = p.snapshot.windows_path.clone().ok_or_else(|| {
                    fail(InstallationFailure::UninstallPlanInvalid, "no windows_path")
                })?;
                if entry.provenance == WindowsPathProvenance::AddedByDraft {
                    if entry.segment != windows::canonical_segment(layout) {
                        return Err(fail(
                            InstallationFailure::UninstallPlanInvalid,
                            "segment mismatch",
                        ));
                    }
                    windows::undo(registry(ctx)?, &entry.segment, entry.value_pre_install)?;
                }
                Ok(())
            }
        },
        Phase::DraftBinaryRemoved => {
            super::remove_file_if_present(&layout.executable(Executable::Draft))
        }
        Phase::DraftDaemonBinaryRemoved => {
            super::remove_file_if_present(&layout.executable(Executable::Draftd))
        }
        Phase::GlobalStorePurged => {
            let (Some(root), Some(expected)) = (&p.global_store_root, &p.global_store_id) else {
                return Ok(());
            };
            let root = Path::new(root);
            if !root.exists() {
                return Ok(());
            }
            // Revalidate every constraint immediately before the recursive
            // delete, including the same store identity.
            let proven = prove_global_store(root, layout).map_err(|error| {
                fail(InstallationFailure::UninstallRecoveryFailed, error.message)
            })?;
            if &proven.store_id != expected || proven.root != root {
                return Err(fail(
                    InstallationFailure::UninstallRecoveryFailed,
                    "the global store changed identity since it was validated; nothing was deleted",
                ));
            }
            std::fs::remove_dir_all(root)
                .map_err(|error| DraftError::storage(format!("purge {}: {error}", root.display())))
        }
        Phase::ReceiptRemoved => {
            super::remove_file_if_present(&layout.receipt())?;
            crate::support::fsutil::sync_directory(&layout.lifecycle_dir())
        }
        Phase::Committed => Ok(()),
        Phase::CleanupPending => {
            let _ = std::fs::remove_dir_all(layout.rollback(operation));
            super::remove_dir_if_empty(&layout.rollback_root());
            super::remove_dir_if_empty(&layout.bin_dir());
            Ok(())
        }
        Phase::Finalized => {
            // The semantic→structural handoff commit. Only now can READY exist.
            let record = TerminalRecord {
                installation_id: journal.op.installation_id.clone(),
                operation_id: operation.clone(),
                mode: layout.platform(),
            };
            super::write_private_bytes(&layout.terminal_record(), record.render().as_bytes())
        }
        other => Err(fail(
            InstallationFailure::UninstallRecoveryFailed,
            format!("{other:?} is not an uninstall action"),
        )),
    }
}

/// The structural tail after `operation.json` is gone, under the still-held
/// lock: helper image and slot, `bootstrap.recovery`, empty `staging/` and
/// `rollback/`, and READY last — but only once nothing else remains. Where a
/// running image cannot remove itself (Windows) READY stays for the next lock
/// owner. `lifecycle.lock`, `.draft-install/` and `<install_root>` are never
/// removed.
pub(crate) fn terminal_tail(layout: &InstallLayout, record: &TerminalRecord) -> bool {
    let op = &record.operation_id;
    let _ = std::fs::remove_file(layout.helper(op));
    let _ = std::fs::remove_file(layout.bootstrap_record());
    let _ = std::fs::remove_dir_all(layout.staging(op));
    super::remove_dir_if_empty(&layout.staging_root());
    super::remove_dir_if_empty(&layout.rollback_root());
    let residue = layout.staging(op).exists() || layout.bootstrap_record().exists();
    if !residue {
        let _ = std::fs::remove_file(layout.terminal_record());
        let _ = crate::support::fsutil::sync_directory(&layout.lifecycle_dir());
    }
    !residue
}

/// Continue an interrupted uninstall from an installed `draft` (generic
/// recovery, I68): re-stage the helper if the executor is gone but an
/// identity-valid `draft` still exists, then hand off. With no executor left,
/// fail closed and preserve every lifecycle record.
pub fn resume_from_installed(ctx: &Context<'_>, op: InstallationOperation) -> DraftResult<()> {
    op.validate(ctx.layout)?;
    let installation = op.installation_id.clone();
    let operation = op.operation_id.clone();
    let OperationPayload::Uninstall(p) = op.payload.clone() else {
        return Err(fail(
            InstallationFailure::RecoveryFailed,
            "not an uninstall",
        ));
    };
    let helper = ctx.layout.helper(&operation);
    let mut journal = Journal::resume(ctx.layout, op, ctx.faults);
    if journal.phase() == Phase::Resolved {
        stage_helper(&mut journal, &ctx.layout.executable(Executable::Draft))?;
    } else if !p.helper_identity.matches(&helper) {
        let source = ctx.layout.executable(Executable::Draft);
        if !p.helper_identity.matches(&source) {
            return Err(executor_unavailable(
                ctx.layout,
                &installation,
                &operation,
                &p.helper_identity,
                helper.exists(),
            ));
        }
        std::fs::copy(&source, &helper)
            .map_err(|error| DraftError::storage(format!("re-stage helper: {error}")))?;
        if !p.helper_identity.matches(&helper) {
            return Err(executor_unavailable(
                ctx.layout,
                &installation,
                &operation,
                &p.helper_identity,
                true,
            ));
        }
        let record = bootstrap_of(&journal);
        super::write_private_bytes(&ctx.layout.bootstrap_record(), record.render().as_bytes())?;
    }
    ctx.host.launch_helper(&helper, &installation, &operation)
}

/// `UninstallRecoveryExecutorUnavailable`: only safely known facts, and never
/// any advice to delete lifecycle state.
pub fn executor_unavailable(
    layout: &InstallLayout,
    installation: &InstallationId,
    operation: &InstallationOperationId,
    expected: &Identity,
    helper_present: bool,
) -> DraftError {
    fail(
        InstallationFailure::UninstallRecoveryExecutorUnavailable,
        format!(
            "an uninstall of installation {installation} (operation {operation}) at {} cannot \
             continue: its staged helper {} is {} (expected sha256 {} size {}) and no \
             identity-valid draft remains. This lifecycle state requires manual lifecycle \
             repair/support because the operation's previously authorized executor is \
             unavailable.",
            layout.root().display(),
            layout.helper(operation).display(),
            if helper_present {
                "digest-mismatched"
            } else {
                "missing"
            },
            expected.sha256,
            expected.size,
        ),
    )
    .with_suggestion(
        "Do NOT delete lifecycle.lock; do NOT delete operation.json; do NOT recursively remove \
         .draft-install/; do NOT delete the installation root to retry installation.",
    )
}

/// Whether a failure was an injected crash (tests) rather than a refusal.
pub fn crashed(error: &DraftError) -> bool {
    is_injected_crash(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installation::layout::canonicalize;
    use crate::installation::receipt::{self, fixtures};
    use crate::installation::update::testing::FakeHost;
    use crate::installation::{FaultPoint, NoFaults, INJECTED_CRASH};
    use crate::support::error::DraftErrorKind;

    struct World {
        dir: tempfile::TempDir,
        layout: InstallLayout,
        host: FakeHost,
        path_bin: PathBuf,
    }

    fn make_world() -> World {
        let dir = tempfile::tempdir().unwrap();
        let base = canonicalize(dir.path()).unwrap();
        let layout = InstallLayout::new(base.join("root"), InstallPlatform::Unix);
        let path_bin = base.join("pathbin");
        std::fs::create_dir_all(layout.bin_dir()).unwrap();
        std::fs::create_dir_all(&path_bin).unwrap();
        std::fs::write(layout.executable(Executable::Draft), b"draft").unwrap();
        std::fs::write(layout.executable(Executable::Draftd), b"draftd").unwrap();
        // An unrelated sibling in the shared PATH directory.
        std::fs::write(path_bin.join("python3"), b"not draft").unwrap();
        #[cfg(unix)]
        for executable in [Executable::Draft, Executable::Draftd] {
            std::os::unix::fs::symlink(
                layout.executable(executable),
                path_bin.join(executable.stem()),
            )
            .unwrap();
        }
        layout.ensure_skeleton().unwrap();
        receipt::write(&layout, &fixtures::unix(&layout, &path_bin)).unwrap();
        World {
            dir,
            layout,
            host: FakeHost::default(),
            path_bin,
        }
    }

    fn ctx<'a>(world: &'a World, faults: &'a dyn Faults) -> Context<'a> {
        Context {
            layout: &world.layout,
            host: &world.host,
            registry: None,
            faults,
        }
    }

    struct CrashAt(FaultPoint);
    impl Faults for CrashAt {
        fn at(&self, point: FaultPoint) -> DraftResult<()> {
            if point == self.0 {
                Err(DraftError::new(DraftErrorKind::Internal, INJECTED_CRASH))
            } else {
                Ok(())
            }
        }
    }

    fn start(
        world: &World,
        faults: &dyn Faults,
        purge: Option<ProvenStore>,
    ) -> DraftResult<InstallationOperationId> {
        let receipt = receipt::read_final(&world.layout).unwrap();
        let _lock = operation::lock(&world.layout, LIFECYCLE_LOCK_TIMEOUT)?;
        begin(&ctx(world, faults), &receipt, purge)
    }

    fn helper(
        world: &World,
        operation: &InstallationOperationId,
        faults: &dyn Faults,
        mode: HelperMode,
    ) -> DraftResult<()> {
        let installation = InstallationId::new("ins_0123456789ab");
        run_helper(
            &ctx(world, faults),
            &world.layout.helper(operation),
            &installation,
            operation,
            mode,
        )
    }

    fn assert_clean(world: &World) {
        let layout = &world.layout;
        for gone in [
            layout.executable(Executable::Draft),
            layout.executable(Executable::Draftd),
            layout.receipt(),
            layout.operation(),
            layout.bootstrap_record(),
            layout.terminal_record(),
            layout.staging_root(),
            world.path_bin.join("draft"),
            world.path_bin.join("draftd"),
        ] {
            assert!(
                std::fs::symlink_metadata(&gone).is_err(),
                "{} survived",
                gone.display()
            );
        }
        // The permanent inert skeleton, and nothing else.
        assert!(layout.lock().exists());
        let remaining: Vec<_> = std::fs::read_dir(layout.lifecycle_dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(remaining, ["lifecycle.lock"]);
        assert!(layout.root().exists(), "the root is never removed");
        assert_eq!(
            std::fs::read(world.path_bin.join("python3")).unwrap(),
            b"not draft"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_normal_uninstall_leaves_only_the_lock_skeleton() {
        let world = make_world();
        world.host.running.set(true);
        let operation = start(&world, &NoFaults, None).unwrap();
        // The helper was handed ids only; it was staged with the bootstrap record.
        assert_eq!(world.host.launched.borrow().len(), 1);
        assert!(world.layout.bootstrap_record().exists());
        let record =
            BootstrapRecord::parse(&std::fs::read(world.layout.bootstrap_record()).unwrap())
                .unwrap();
        let op = operation::read(&world.layout).unwrap().unwrap();
        if let OperationPayload::Uninstall(p) = &op.payload {
            assert_eq!(
                record.helper, p.helper_identity,
                "expected identity journalled at Resolved"
            );
        }
        helper(
            &world,
            &operation,
            &NoFaults,
            HelperMode::ParentExit { parent_pid: 1 },
        )
        .unwrap();
        assert_clean(&world);
        assert!(!world.host.running.get());
    }

    #[cfg(unix)]
    #[test]
    fn every_uninstall_crash_window_converges_forward() {
        let phases = [
            Phase::DaemonStopped,
            Phase::PathIntegrationRemoved,
            Phase::DraftBinaryRemoved,
            Phase::DraftDaemonBinaryRemoved,
            Phase::ReceiptRemoved,
            Phase::Committed,
            Phase::CleanupPending,
            Phase::Finalized,
        ];
        for phase in phases {
            for point in [
                FaultPoint::BeforeAction(phase),
                FaultPoint::AfterActionBeforePhase(phase),
                FaultPoint::AfterPhase(phase),
            ] {
                let world = make_world();
                let operation = start(&world, &NoFaults, None).unwrap();
                let crashed = helper(
                    &world,
                    &operation,
                    &CrashAt(point),
                    HelperMode::ParentExit { parent_pid: 1 },
                );
                assert!(crashed.is_err(), "{point:?}");
                // After a reboot the installer relaunches the helper.
                helper(&world, &operation, &NoFaults, HelperMode::BootstrapRecovery)
                    .unwrap_or_else(|error| panic!("{point:?}: {error:?}"));
                assert_clean(&world);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_helper_refuses_forged_or_foreign_requests_and_changes_nothing() {
        let world = make_world();
        let operation = start(&world, &NoFaults, None).unwrap();
        let before = operation::read(&world.layout).unwrap();
        let foreign = InstallationId::new("ins_ffffffffffff");
        assert!(run_helper(
            &ctx(&world, &NoFaults),
            &world.layout.helper(&operation),
            &foreign,
            &operation,
            HelperMode::BootstrapRecovery
        )
        .is_err());
        let forged = InstallationOperationId::new("ilo_ffffffffffff");
        assert!(run_helper(
            &ctx(&world, &NoFaults),
            &world.layout.helper(&operation),
            &InstallationId::new("ins_0123456789ab"),
            &forged,
            HelperMode::BootstrapRecovery
        )
        .is_err());
        // A helper whose bytes do not match is never trusted.
        std::fs::write(world.layout.helper(&operation), b"tampered").unwrap();
        assert!(helper(&world, &operation, &NoFaults, HelperMode::BootstrapRecovery).is_err());
        assert_eq!(operation::read(&world.layout).unwrap(), before);
        assert!(world.layout.executable(Executable::Draft).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_lost_executor_fails_closed_and_preserves_every_record() {
        let world = make_world();
        let operation = start(&world, &NoFaults, None).unwrap();
        // Crash after both binaries are gone, then lose the helper.
        let _ = helper(
            &world,
            &operation,
            &CrashAt(FaultPoint::AfterPhase(Phase::DraftDaemonBinaryRemoved)),
            HelperMode::ParentExit { parent_pid: 1 },
        );
        std::fs::remove_file(world.layout.helper(&operation)).unwrap();
        let op = operation::read(&world.layout).unwrap().unwrap();
        let error = resume_from_installed(&ctx(&world, &NoFaults), op).unwrap_err();
        assert_eq!(
            super::super::failure_of(&error),
            Some(InstallationFailure::UninstallRecoveryExecutorUnavailable)
        );
        assert!(!error.suggestion.clone().unwrap().contains("rm "));
        for kept in [
            world.layout.operation(),
            world.layout.lock(),
            world.layout.bootstrap_record(),
            world.layout.receipt(),
        ] {
            assert!(kept.exists(), "{} was not preserved", kept.display());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_retargeted_link_or_foreign_binary_stops_the_uninstall_before_deletion() {
        let world = make_world();
        std::fs::remove_file(world.path_bin.join("draft")).unwrap();
        std::os::unix::fs::symlink(world.path_bin.join("python3"), world.path_bin.join("draft"))
            .unwrap();
        let operation = start(&world, &NoFaults, None).unwrap();
        assert!(helper(
            &world,
            &operation,
            &NoFaults,
            HelperMode::ParentExit { parent_pid: 1 }
        )
        .is_err());
        assert!(world.layout.executable(Executable::Draft).exists());
        assert!(world.layout.executable(Executable::Draftd).exists());
        assert!(world.path_bin.join("python3").exists());
    }

    #[cfg(unix)]
    #[test]
    fn purge_is_proven_before_the_journal_and_revalidated_before_deletion() {
        let world = make_world();
        let base = canonicalize(world.dir.path()).unwrap();
        // An unrelated populated directory is never purge-authorized.
        let documents = base.join("Documents");
        std::fs::create_dir_all(&documents).unwrap();
        std::fs::write(documents.join("thesis.txt"), b"mine").unwrap();
        DraftGlobalStore::at(&documents).create_all().unwrap();
        assert!(prove_global_store(&documents, &world.layout).is_err());
        // Nor is the root, the install root, a project or a symlink elsewhere.
        assert!(prove_global_store(Path::new("/"), &world.layout).is_err());
        assert!(prove_global_store(world.layout.root(), &world.layout).is_err());
        assert!(prove_global_store(&world.layout.lifecycle_dir(), &world.layout).is_err());
        let project = base.join("project");
        std::fs::create_dir_all(project.join(".draft")).unwrap();
        assert!(prove_global_store(&project, &world.layout).is_err());

        let store = base.join("home/.draft");
        DraftGlobalStore::at(&store).create_all().unwrap();
        let proven = prove_global_store(&store, &world.layout).unwrap();
        assert!(
            operation::read(&world.layout).unwrap().is_none(),
            "no journal before proof"
        );
        let operation = start(&world, &NoFaults, Some(proven)).unwrap();
        helper(
            &world,
            &operation,
            &NoFaults,
            HelperMode::ParentExit { parent_pid: 1 },
        )
        .unwrap();
        assert!(!store.exists());
        assert_eq!(
            std::fs::read(documents.join("thesis.txt")).unwrap(),
            b"mine"
        );
        assert_clean(&world);
    }

    #[cfg(unix)]
    #[test]
    fn a_swapped_store_is_refused_rather_than_deleted() {
        let world = make_world();
        let base = canonicalize(world.dir.path()).unwrap();
        let store = base.join("home/.draft");
        DraftGlobalStore::at(&store).create_all().unwrap();
        let proven = prove_global_store(&store, &world.layout).unwrap();
        let operation = start(&world, &NoFaults, Some(proven)).unwrap();
        // Replace the store with a different Draft store under the same path.
        std::fs::remove_dir_all(&store).unwrap();
        DraftGlobalStore::at(&store).create_all().unwrap();
        assert!(helper(
            &world,
            &operation,
            &NoFaults,
            HelperMode::ParentExit { parent_pid: 1 }
        )
        .is_err());
        assert!(store.exists(), "never deleted on an identity mismatch");
    }

    #[cfg(unix)]
    #[test]
    fn dry_run_is_the_same_plan_and_mutates_nothing() {
        let world = make_world();
        let receipt = receipt::read_final(&world.layout).unwrap();
        let plan = plan(
            &world.layout,
            &receipt,
            None,
            None,
            Path::new("/home/ada/.draft"),
        )
        .unwrap();
        assert_eq!(plan.remove_executables.len(), 2);
        assert!(matches!(&plan.path, PathPlan::Unix { remove_links } if remove_links.len() == 2));
        assert!(plan
            .preserved
            .iter()
            .any(|line| line.contains("/home/ada/.draft")));
        assert!(plan.remains.ends_with("lifecycle.lock"));
        assert!(operation::read(&world.layout).unwrap().is_none());
    }
}
