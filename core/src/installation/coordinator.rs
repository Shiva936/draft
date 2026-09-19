//! The installer lifecycle coordinator and generic installation recovery.
//!
//! `install.sh` / `install.ps1` verify the download against `SHA256SUMS`
//! (stage-1 trust), extract it to a temporary directory and launch the
//! extracted `draft` in this hidden mode. From then on only Rust mutates the
//! canonical root, and only while holding `lifecycle.lock`:
//!
//! 1. create the minimal authority skeleton (`<root>/`, `.draft-install/`,
//!    the lock) and acquire the lock;
//! 2. **reclassify under the lock** — never act on the shell's pre-lock view;
//! 3. mint the operation id and the new `InstallationId`, and durably journal
//!    `FreshInstall` `Resolved` with every identity, root and the
//!    platform-tagged `path_state` *before* any managed payload changes;
//! 4. run the whole transaction in-process — there is no separately invokable
//!    finalizer — and roll back exactly what this operation created if it
//!    fails before the installation is provably complete.
//!
//! ```text
//! Resolved → RootBinariesInstalled → BinariesReceiptCommitted →
//! [LegacyPathStaged] → PathIntegrationApplied → PathIntegrationReceiptCommitted →
//! InstalledBinariesValidated → Committed → CleanupPending → Finalized
//! ```
//!
//! Recovery ownership is by executor availability: an installed target
//! `draft` recovers all four kinds through [`recover_installed`]; with none,
//! the freshly extracted coordinator may continue only a schema-compatible
//! `FreshInstall` whose target pair it exactly is ([`recover_as_coordinator`]).

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::layout::{self, Executable, InstallLayout};
use super::operation::{
    self, FreshInstallPayload, InstallationOperation, Journal, OperationKind, OperationPayload,
    PathState, Phase,
};
use super::path::{unix, windows};
use super::receipt::{
    self, DraftBinaryEntry, DraftDaemonBinaryEntry, InstallationMethod, InstallationReceipt,
    IntermediateExpectation, PathSymlinkEntry, ReceiptContext, ReleaseChannel, WindowsPathEntry,
    WindowsPathProvenance,
};
use super::terminal::TerminalRecord;
use super::{
    fail, is_injected_crash, Faults, Identity, InstallPlatform, InstallationFailure,
    InstallationId, InstallationOperationId, LifecycleHost, LIFECYCLE_LOCK_TIMEOUT,
};
use crate::support::error::{DraftError, DraftResult};

/// The bounded install configuration the installer passes: roots and scalar
/// modes only. Every mutable slot is derived from these by Rust.
#[derive(Debug, Clone)]
pub struct InstallConfig {
    pub install_root: PathBuf,
    /// `<path_bin>` for the Unix PATH symlinks (`DRAFT_INSTALL_DIR`).
    pub path_bin: Option<PathBuf>,
    /// Windows: `-UpdatePath` / `DRAFT_UPDATE_PATH=1`.
    pub update_path_requested: bool,
    /// Unix: `DRAFT_MIGRATE_LEGACY_PATH=1`.
    pub migrate_legacy: bool,
    /// Windows: the installer's process-effective PATH, for the exposure rule.
    pub process_path: Option<String>,
}

/// The release this coordinator *is*: its own extracted `draft` and the
/// sibling `draftd` from the same verified package.
#[derive(Debug, Clone)]
pub struct Package {
    pub draft: PathBuf,
    pub draftd: PathBuf,
    pub version: String,
    pub platform_target: String,
}

impl Package {
    fn identities(&self) -> DraftResult<(Identity, Identity)> {
        Ok((
            Identity::of_file(&self.draft)?,
            Identity::of_file(&self.draftd)?,
        ))
    }
}

pub struct Context<'a> {
    pub host: &'a dyn LifecycleHost,
    pub registry: Option<&'a dyn windows::UserPathRegistry>,
    pub faults: &'a dyn Faults,
    pub platform: InstallPlatform,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum InstallOutcome {
    Installed {
        install_root: String,
        version: String,
        installation_id: String,
    },
    AlreadyInstalled {
        install_root: String,
        version: String,
    },
}

/// The fixed-slot classification both installers perform (I64), before any
/// lock and without parsing `operation.json`. The Rust coordinator repeats it
/// under the lock; this pre-lock answer only routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Route {
    /// Journal + a valid bootstrap record: relaunch the staged uninstall helper.
    A,
    /// Journal, no bootstrap, an installed `draft`: generic recovery on it.
    B,
    /// Journal, no bootstrap, no installed `draft`: the extracted coordinator,
    /// recovery-only, for a compatible `FreshInstall` only.
    C,
    /// No journal, a valid READY: terminal cleanup, then install.
    D,
    /// No journal, malformed or orphan residue: fail closed.
    E,
    /// Nothing, or only the inert lock skeleton: install.
    F,
    /// A live managed installation (a receipt, no journal).
    Managed,
}

pub fn classify(layout: &InstallLayout) -> Route {
    let journal = layout.operation().exists();
    let bootstrap = std::fs::read(layout.bootstrap_record())
        .ok()
        .map(|bytes| super::bootstrap::BootstrapRecord::parse(&bytes).is_ok());
    let ready = std::fs::read(layout.terminal_record())
        .ok()
        .map(|bytes| TerminalRecord::parse(&bytes).is_ok());
    let installed = layout.executable(Executable::Draft).exists();
    match (journal, bootstrap, ready) {
        (true, Some(true), _) => Route::A,
        (true, Some(false), _) => Route::E,
        (true, None, _) if installed => Route::B,
        (true, None, _) => Route::C,
        (false, _, Some(true)) => Route::D,
        (false, _, Some(false)) => Route::E,
        (false, Some(_), None) => Route::E,
        (false, None, None) => {
            let residue =
                dir_has_entries(&layout.staging_root()) || dir_has_entries(&layout.rollback_root());
            if residue {
                Route::E
            } else if layout.receipt().exists() {
                Route::Managed
            } else {
                Route::F
            }
        }
    }
}

fn dir_has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// Case D: consume a finished uninstall's structural residue as the same lock
/// owner, removing READY last. Malformed state is never deletion authority.
pub fn consume_terminal(layout: &InstallLayout) -> DraftResult<()> {
    let bytes = std::fs::read(layout.terminal_record())?;
    let record = TerminalRecord::parse(&bytes)?;
    if layout.operation().exists() {
        return Err(fail(
            InstallationFailure::InstallerLifecycleStateChanged,
            "an operation journal reappeared beside terminal-cleanup",
        ));
    }
    if record.mode != layout.platform() {
        return Err(fail(
            InstallationFailure::TerminalCleanupRecordInvalid,
            "terminal-cleanup names another platform",
        ));
    }
    if let Ok(bytes) = std::fs::read(layout.bootstrap_record()) {
        let bootstrap = super::bootstrap::BootstrapRecord::parse(&bytes).map_err(|_| {
            fail(
                InstallationFailure::TerminalCleanupRecordInvalid,
                "bootstrap residue is malformed",
            )
        })?;
        if bootstrap.operation_id != record.operation_id
            || bootstrap.installation_id != record.installation_id
        {
            return Err(fail(
                InstallationFailure::TerminalCleanupRecordInvalid,
                "bootstrap residue belongs to another operation",
            ));
        }
    }
    let other_staging = std::fs::read_dir(layout.staging_root())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.file_name() != record.operation_id.as_str())
        })
        .unwrap_or(false);
    if other_staging || dir_has_entries(&layout.bin_dir()) || layout.receipt().exists() {
        return Err(fail(
            InstallationFailure::TerminalCleanupRecordInvalid,
            "the terminal residue holds more than its own operation's slots",
        ));
    }
    if !super::uninstall::terminal_tail(layout, &record) {
        return Err(fail(
            InstallationFailure::TerminalCleanupPending,
            "the finished uninstall's helper still holds its residue",
        ));
    }
    Ok(())
}

/// Whether the root holds only the permitted inert skeleton.
fn is_inert_skeleton(layout: &InstallLayout) -> bool {
    let only = |dir: &Path, allowed: &[&str]| {
        std::fs::read_dir(dir).map_or(true, |entries| {
            entries
                .filter_map(Result::ok)
                .all(|entry| allowed.contains(&entry.file_name().to_string_lossy().as_ref()))
        })
    };
    only(layout.root(), &[".draft-install", "bin"])
        && !dir_has_entries(&layout.bin_dir())
        && only(
            &layout.lifecycle_dir(),
            &["lifecycle.lock", "staging", "rollback"],
        )
        && !dir_has_entries(&layout.staging_root())
        && !dir_has_entries(&layout.rollback_root())
}

/// `install.sh` / `install.ps1`'s coordinator entry: a fresh install, or a
/// no-mutation report when this exact installation already exists.
pub fn install(
    ctx: &Context<'_>,
    config: &InstallConfig,
    package: &Package,
) -> DraftResult<InstallOutcome> {
    if !super::SUPPORTED_TARGETS.contains(&package.platform_target.as_str()) {
        return Err(fail(
            InstallationFailure::UnsupportedPlatform,
            "unsupported target",
        ));
    }
    let root = layout::validate_selected_root(&config.install_root)?;
    let layout = InstallLayout::new(root, ctx.platform);
    let path_bin = match ctx.platform {
        InstallPlatform::Unix => {
            let bin = config.path_bin.clone().ok_or_else(|| {
                fail(
                    InstallationFailure::PermissionDenied,
                    "the Unix installer needs a PATH directory",
                )
            })?;
            if !bin.is_absolute() {
                return Err(fail(
                    InstallationFailure::PermissionDenied,
                    "the PATH directory must be absolute",
                ));
            }
            Some(bin)
        }
        InstallPlatform::Windows => None,
    };
    // The only mutation before `Resolved`: the authority skeleton.
    layout.ensure_skeleton()?;
    let _lock = operation::lock(&layout, LIFECYCLE_LOCK_TIMEOUT)?;
    // Reclassify under the lock; the installer's preflight is only advisory.
    if operation::read(&layout)?.is_some() {
        return Err(fail(
            InstallationFailure::InstallerLifecycleStateChanged,
            "an unfinished lifecycle operation exists for this root; it must be recovered first",
        )
        .with_suggestion(
            "Re-run the official installer: it resumes the operation before installing.",
        ));
    }
    match classify(&layout) {
        Route::D => consume_terminal(&layout)?,
        Route::E => {
            return Err(fail(
                InstallationFailure::TerminalCleanupRecordInvalid,
                format!(
                    "{} holds lifecycle residue that is malformed or has no terminal record; \
                     nothing was changed",
                    layout.lifecycle_dir().display()
                ),
            ))
        }
        Route::Managed => {
            let existing = receipt::read_final(&layout)?;
            let (draft, draftd) = package.identities()?;
            if existing.installed_version == package.version
                && existing.draft_executable.identity() == draft
                && existing.draftd_executable.identity() == draftd
            {
                return Ok(InstallOutcome::AlreadyInstalled {
                    install_root: layout.root().display().to_string(),
                    version: existing.installed_version,
                });
            }
            return Err(fail(
                InstallationFailure::InstallerLifecycleStateChanged,
                format!(
                    "Draft {} is already installed at {}; nothing was changed",
                    existing.installed_version,
                    layout.root().display()
                ),
            )
            .with_suggestion("Use `draft update` to change the installed version."));
        }
        _ => {}
    }
    if !is_inert_skeleton(&layout) {
        return Err(fail(
            InstallationFailure::LegacyInstallationConflict,
            format!(
                "{} already holds content Draft did not create; nothing was changed",
                layout.root().display()
            ),
        ));
    }
    fresh_install(ctx, &layout, config, package, path_bin.as_deref())
}

fn fresh_install(
    ctx: &Context<'_>,
    layout: &InstallLayout,
    config: &InstallConfig,
    package: &Package,
    path_bin: Option<&Path>,
) -> DraftResult<InstallOutcome> {
    let (draft_identity, draftd_identity) = package.identities()?;
    let operation_id = InstallationOperationId::generate();
    let installation_id = InstallationId::generate();
    let path_state = match layout.platform() {
        InstallPlatform::Unix => {
            let path_bin = path_bin.expect("validated above");
            let (draft_slot, draftd_slot, _) = unix::classify_pair(
                layout,
                path_bin,
                config.migrate_legacy,
                &operation_id,
                ctx.host,
            )?;
            PathState::Unix {
                path_bin: path_bin.display().to_string(),
                draft_slot,
                draftd_slot,
                legacy_migration_authorized: config.migrate_legacy,
            }
        }
        InstallPlatform::Windows => {
            let registry = ctx.registry.ok_or_else(|| {
                fail(
                    InstallationFailure::WindowsPathStateInvalid,
                    "the User PATH is unavailable",
                )
            })?;
            let (value_pre_install, segment_pre_install, provenance) =
                windows::classify_pre_install(
                    registry,
                    &windows::canonical_segment(layout),
                    config.process_path.as_deref(),
                    config.update_path_requested,
                )?;
            PathState::Windows {
                path_update_requested: config.update_path_requested,
                value_pre_install,
                segment_pre_install,
                provenance,
            }
        }
    };
    let channel = if semver::Version::parse(&package.version).is_ok_and(|v| !v.pre.is_empty()) {
        ReleaseChannel::Prerelease
    } else {
        ReleaseChannel::Stable
    };
    let op = InstallationOperation::new(
        installation_id.clone(),
        operation_id,
        OperationPayload::FreshInstall(FreshInstallPayload {
            install_root: layout.root().display().to_string(),
            platform_target: package.platform_target.clone(),
            target_version: package.version.clone(),
            target_release_channel: channel,
            target_draft_identity: draft_identity,
            target_draftd_identity: draftd_identity,
            initial_install_generation: 1,
            path_state,
        }),
    );
    // `Resolved` is durable before any managed payload mutates.
    let journal = Journal::create(layout, op, ctx.faults)?;
    drive_fresh(ctx, journal, package)?;
    Ok(InstallOutcome::Installed {
        install_root: layout.root().display().to_string(),
        version: package.version.clone(),
        installation_id: installation_id.to_string(),
    })
}

fn fresh(journal: &Journal<'_>) -> FreshInstallPayload {
    match &journal.op.payload {
        OperationPayload::FreshInstall(payload) => payload.clone(),
        _ => unreachable!("the FreshInstall engine runs FreshInstall journals only"),
    }
}

fn expectation(journal: &Journal<'_>) -> IntermediateExpectation {
    let p = fresh(journal);
    IntermediateExpectation {
        installation_id: journal.op.installation_id.clone(),
        target_version: p.target_version,
        target_release_channel: p.target_release_channel,
        target_draft_identity: p.target_draft_identity,
        target_draftd_identity: p.target_draftd_identity,
    }
}

fn target_of(p: &FreshInstallPayload, executable: Executable) -> &Identity {
    match executable {
        Executable::Draft => &p.target_draft_identity,
        Executable::Draftd => &p.target_draftd_identity,
    }
}

fn unix_slots(p: &FreshInstallPayload) -> Vec<(Executable, operation::UnixSlot, PathBuf)> {
    match &p.path_state {
        PathState::Unix {
            path_bin,
            draft_slot,
            draftd_slot,
            ..
        } => vec![
            (
                Executable::Draft,
                draft_slot.clone(),
                Path::new(path_bin).join("draft"),
            ),
            (
                Executable::Draftd,
                draftd_slot.clone(),
                Path::new(path_bin).join("draftd"),
            ),
        ],
        PathState::Windows { .. } => Vec::new(),
    }
}

fn build_receipt(
    layout: &InstallLayout,
    journal: &Journal<'_>,
    final_shape: bool,
) -> InstallationReceipt {
    let p = fresh(journal);
    let (path_links, windows_path) = if !final_shape {
        (Vec::new(), None)
    } else {
        match &p.path_state {
            PathState::Unix { .. } => (
                unix_slots(&p)
                    .into_iter()
                    .map(|(executable, _, link)| PathSymlinkEntry {
                        link_path: link.display().to_string(),
                        expected_target: layout.executable(executable).display().to_string(),
                    })
                    .collect(),
                None,
            ),
            PathState::Windows {
                value_pre_install,
                provenance,
                ..
            } => (
                Vec::new(),
                Some(WindowsPathEntry {
                    segment: windows::canonical_segment(layout),
                    provenance: *provenance,
                    value_pre_install: *value_pre_install,
                }),
            ),
        }
    };
    InstallationReceipt {
        schema_version: receipt::RECEIPT_SCHEMA_VERSION,
        installation_id: journal.op.installation_id.clone(),
        installed_version: p.target_version.clone(),
        install_generation: 1,
        release_channel: p.target_release_channel,
        installation_method: InstallationMethod::OfficialStandalone,
        platform_target: p.platform_target.clone(),
        install_root: layout.root().display().to_string(),
        draft_executable: DraftBinaryEntry {
            relative_path: layout.relative(Executable::Draft),
            sha256: p.target_draft_identity.sha256.clone(),
            size: p.target_draft_identity.size,
        },
        draftd_executable: DraftDaemonBinaryEntry {
            relative_path: layout.relative(Executable::Draftd),
            sha256: p.target_draftd_identity.sha256.clone(),
            size: p.target_draftd_identity.size,
        },
        path_links,
        windows_path,
        installed_at: crate::support::common::now(),
    }
}

fn incompatible(why: impl Into<String>) -> DraftError {
    fail(InstallationFailure::InstallationRecoveryIncompatible, why)
}

/// The FreshInstall liminal probes (I71 table).
fn probe_fresh(ctx: &Context<'_>, journal: &Journal<'_>, next: Phase) -> DraftResult<bool> {
    let layout = journal.layout;
    let p = fresh(journal);
    match next {
        Phase::RootBinariesInstalled => {
            let mut complete = true;
            for executable in [Executable::Draft, Executable::Draftd] {
                let slot = layout.executable(executable);
                if !slot.exists() {
                    complete = false;
                } else if !target_of(&p, executable).matches(&slot) {
                    return Err(incompatible(format!(
                        "{} holds an unexpected identity",
                        slot.display()
                    )));
                }
            }
            Ok(complete)
        }
        Phase::BinariesReceiptCommitted => match receipt::read(layout)? {
            None => Ok(false),
            Some(existing) => {
                existing
                    .validate(
                        layout,
                        &ReceiptContext::FreshInstallIntermediate(expectation(journal)),
                    )
                    .map_err(|error| incompatible(error.message))?;
                if existing.is_intermediate() {
                    Ok(true)
                } else {
                    Err(incompatible(
                        "a final receipt exists before its path integration",
                    ))
                }
            }
        },
        Phase::LegacyPathStaged => Ok(unix_slots(&p).iter().all(|(executable, slot, link)| {
            !slot.moved_aside
                || (std::fs::symlink_metadata(link).is_err()
                    && slot.legacy_identity.as_ref().is_some_and(|identity| {
                        identity.matches(&layout.legacy_slot(&journal.op.operation_id, *executable))
                    }))
        })),
        Phase::PathIntegrationApplied => match &p.path_state {
            PathState::Unix { .. } => {
                let mut complete = true;
                for (executable, _, link) in unix_slots(&p) {
                    match unix::observe(&link, &layout.executable(executable)) {
                        unix::SlotObservation::ExpectedSymlink => {}
                        unix::SlotObservation::Missing => complete = false,
                        unix::SlotObservation::RegularFile if p.path_state.migrates_legacy() => {
                            complete = false
                        }
                        _ => {
                            return Err(incompatible(format!(
                                "{} is occupied by a foreign object",
                                link.display()
                            )))
                        }
                    }
                }
                Ok(complete)
            }
            PathState::Windows { provenance, .. } => match provenance {
                WindowsPathProvenance::NotManaged | WindowsPathProvenance::PreExisting => Ok(true),
                WindowsPathProvenance::AddedByDraft => windows::reservation_satisfied(
                    ctx.registry
                        .ok_or_else(|| incompatible("the User PATH is unavailable"))?,
                    &windows::canonical_segment(layout),
                ),
            },
        },
        Phase::PathIntegrationReceiptCommitted => match receipt::read(layout)? {
            Some(existing) if !existing.is_intermediate() => {
                existing
                    .validate(layout, &ReceiptContext::FinalManaged)
                    .map_err(|error| incompatible(error.message))?;
                Ok(existing.installation_id == journal.op.installation_id
                    && existing.install_generation == 1)
            }
            Some(_) => Ok(false),
            None => Err(incompatible("the receipt vanished after path integration")),
        },
        _ => Ok(false),
    }
}

fn act_fresh(
    ctx: &Context<'_>,
    journal: &Journal<'_>,
    next: Phase,
    package: Option<&Package>,
) -> DraftResult<()> {
    let layout = journal.layout;
    let p = fresh(journal);
    let op = &journal.op.operation_id;
    match next {
        Phase::RootBinariesInstalled => {
            let package = package.ok_or_else(|| incompatible("no package to install from"))?;
            crate::support::fsutil::ensure_dir(&layout.bin_dir())?;
            for (executable, source) in [
                (Executable::Draft, &package.draft),
                (Executable::Draftd, &package.draftd),
            ] {
                let slot = layout.executable(executable);
                let identity = target_of(&p, executable);
                if identity.matches(&slot) {
                    continue;
                }
                if slot.exists() {
                    return Err(incompatible(format!(
                        "{} appeared during the install",
                        slot.display()
                    )));
                }
                let staged = layout.staged(op, executable);
                crate::support::fsutil::ensure_dir(staged.parent().expect("staged has a parent"))?;
                super::remove_file_if_present(&staged)?;
                std::fs::copy(source, &staged).map_err(|error| {
                    DraftError::storage(format!("stage {}: {error}", source.display()))
                })?;
                std::fs::File::open(&staged)
                    .and_then(|file| file.sync_all())
                    .map_err(|error| DraftError::storage(error.to_string()))?;
                if !identity.matches(&staged) {
                    return Err(fail(
                        InstallationFailure::InstallationEntryIdentityMismatch,
                        "staged copy differs",
                    ));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
                }
                std::fs::rename(&staged, &slot).map_err(|error| {
                    DraftError::storage(format!("install {}: {error}", slot.display()))
                })?;
            }
            crate::support::fsutil::sync_directory(&layout.bin_dir())
        }
        Phase::BinariesReceiptCommitted => {
            receipt::write(layout, &build_receipt(layout, journal, false))
        }
        Phase::LegacyPathStaged => {
            for (executable, slot, link) in unix_slots(&p) {
                if slot.moved_aside {
                    let identity = slot
                        .legacy_identity
                        .as_ref()
                        .ok_or_else(|| incompatible("no legacy identity"))?;
                    unix::stage_legacy(&link, &layout.legacy_slot(op, executable), identity)?;
                }
            }
            Ok(())
        }
        Phase::PathIntegrationApplied => match &p.path_state {
            PathState::Unix { .. } => {
                for (executable, _, link) in unix_slots(&p) {
                    unix::create(&link, &layout.executable(executable))?;
                }
                Ok(())
            }
            PathState::Windows { provenance, .. } => {
                if *provenance == WindowsPathProvenance::AddedByDraft {
                    windows::apply(
                        ctx.registry
                            .ok_or_else(|| incompatible("the User PATH is unavailable"))?,
                        &windows::canonical_segment(layout),
                    )?;
                }
                Ok(())
            }
        },
        Phase::PathIntegrationReceiptCommitted => {
            let final_receipt = build_receipt(layout, journal, true);
            final_receipt.validate(layout, &ReceiptContext::FinalManaged)?;
            receipt::write(layout, &final_receipt)
        }
        Phase::InstalledBinariesValidated => {
            for executable in [Executable::Draft, Executable::Draftd] {
                let exe = layout.executable(executable);
                let reported = ctx.host.binary_version(&exe).map_err(|error| {
                    fail(
                        InstallationFailure::ValidationFailed,
                        format!("{}: {}", exe.display(), error.message),
                    )
                })?;
                if reported != p.target_version {
                    return Err(fail(
                        InstallationFailure::ValidationFailed,
                        format!(
                            "{} reports {reported}, expected {}",
                            exe.display(),
                            p.target_version
                        ),
                    ));
                }
            }
            Ok(())
        }
        Phase::Committed | Phase::Finalized => Ok(()),
        Phase::CleanupPending => {
            let _ = std::fs::remove_dir_all(layout.staging(op));
            super::remove_dir_if_empty(&layout.staging_root());
            Ok(())
        }
        other => Err(incompatible(format!(
            "{other:?} is not a FreshInstall phase"
        ))),
    }
}

/// Forward from the current phase; before `Committed`, any genuine failure
/// takes the frozen rollback. An injected crash stops dead.
fn drive_fresh(ctx: &Context<'_>, mut journal: Journal<'_>, package: &Package) -> DraftResult<()> {
    match forward_fresh(ctx, &mut journal, Some(package)) {
        Ok(()) => journal.delete(),
        Err(error) if is_injected_crash(&error) => Err(error),
        Err(error) => {
            if !journal.op.reached(Phase::Committed) {
                rollback_fresh(ctx, journal)?;
            }
            Err(error)
        }
    }
}

fn forward_fresh(
    ctx: &Context<'_>,
    journal: &mut Journal<'_>,
    package: Option<&Package>,
) -> DraftResult<()> {
    while let Some(next) = journal.op.next_phase() {
        if probe_fresh(ctx, journal, next)? {
            // The action already completed: only the phase write remains, and
            // the same crash windows apply to it.
            ctx.faults.at(super::FaultPoint::BeforeAction(next))?;
            ctx.faults
                .at(super::FaultPoint::AfterActionBeforePhase(next))?;
            journal.advance(next)?;
            continue;
        }
        journal.step(next, |journal| act_fresh(ctx, journal, next, package))?;
    }
    Ok(())
}

/// I69/I71 rollback, in the frozen order: legacy objects restored first (a
/// slot this operation re-linked is unlinked just before its legacy file
/// returns), then only the path integration this operation owns, then the
/// receipt it wrote, the root binaries it created, its staging — leaving only
/// the inert skeleton. Nothing is deleted for merely occupying a pathname.
fn rollback_fresh(ctx: &Context<'_>, journal: Journal<'_>) -> DraftResult<()> {
    let layout = journal.layout;
    let p = fresh(&journal);
    let op = journal.op.operation_id.clone();
    for (executable, slot, link) in unix_slots(&p) {
        let target = layout.executable(executable);
        if slot.created_by_operation
            && unix::observe(&link, &target) == unix::SlotObservation::ExpectedSymlink
        {
            unix::remove(&link, &target)?;
        }
        if slot.moved_aside {
            let identity = slot
                .legacy_identity
                .as_ref()
                .ok_or_else(|| incompatible("no legacy identity"))?;
            unix::restore_legacy(&link, &layout.legacy_slot(&op, executable), identity)?;
        }
    }
    if let PathState::Windows {
        provenance: WindowsPathProvenance::AddedByDraft,
        value_pre_install,
        ..
    } = &p.path_state
    {
        windows::undo(
            ctx.registry
                .ok_or_else(|| incompatible("the User PATH is unavailable"))?,
            &windows::canonical_segment(layout),
            *value_pre_install,
        )?;
    }
    if let Some(existing) = receipt::read(layout).ok().flatten() {
        if existing.installation_id == journal.op.installation_id {
            super::remove_file_if_present(&layout.receipt())?;
        }
    }
    for executable in [Executable::Draft, Executable::Draftd] {
        let slot = layout.executable(executable);
        if target_of(&p, executable).matches(&slot) {
            super::remove_file_if_present(&slot)?;
        }
    }
    let _ = std::fs::remove_dir_all(layout.staging(&op));
    super::remove_dir_if_empty(&layout.staging_root());
    super::remove_dir_if_empty(&layout.bin_dir());
    journal.delete()
}

/// Recover a `FreshInstall` whose executor identity is already proven.
fn recover_fresh(
    ctx: &Context<'_>,
    layout: &InstallLayout,
    op: InstallationOperation,
    package: Option<&Package>,
) -> DraftResult<()> {
    let mut journal = Journal::resume(layout, op, ctx.faults);
    // Liminal reconciliation first.
    while let Some(next) = journal.op.next_phase() {
        if matches!(
            next,
            Phase::InstalledBinariesValidated
                | Phase::Committed
                | Phase::CleanupPending
                | Phase::Finalized
        ) {
            break;
        }
        if probe_fresh(ctx, &journal, next)? {
            journal.advance(next)?;
        } else {
            break;
        }
    }
    if !journal.op.reached(Phase::PathIntegrationApplied) {
        return rollback_fresh(ctx, journal);
    }
    match forward_fresh(ctx, &mut journal, package) {
        Ok(()) => journal.delete(),
        Err(error) if is_injected_crash(&error) => Err(error),
        Err(error) => {
            if !journal.op.reached(Phase::Committed) {
                rollback_fresh(ctx, journal)?;
            }
            Err(error)
        }
    }
}

/// Case C: the freshly extracted coordinator with no compatible installed
/// executor. It may continue only a supported `FreshInstall` whose target pair
/// it exactly is (I73); everything else fails closed.
pub fn recover_as_coordinator(
    ctx: &Context<'_>,
    install_root: &Path,
    package: &Package,
) -> DraftResult<()> {
    let layout = InstallLayout::new(layout::validate_selected_root(install_root)?, ctx.platform);
    let _lock = operation::lock(&layout, LIFECYCLE_LOCK_TIMEOUT)?;
    let Some(op) = operation::read(&layout)? else {
        return Ok(());
    };
    if op.kind != OperationKind::FreshInstall {
        return Err(incompatible(format!(
            "an interrupted {:?} at {} can only be resumed by the installed Draft that started it",
            op.kind,
            layout.root().display()
        ))
        .with_suggestion("This state needs installation recovery support; nothing was changed."));
    }
    op.validate(&layout)?;
    let OperationPayload::FreshInstall(p) = &op.payload else {
        unreachable!("kind checked")
    };
    let (draft, draftd) = package.identities()?;
    let mismatch = if package.version != p.target_version {
        Some(format!(
            "this installer is Draft {}, the interrupted install targets {}",
            package.version, p.target_version
        ))
    } else if package.platform_target != p.platform_target {
        Some(format!(
            "this installer targets {}, the interrupted install {}",
            package.platform_target, p.platform_target
        ))
    } else if draft != p.target_draft_identity {
        Some("the draft binary differs from the one the interrupted install authorized".into())
    } else if draftd != p.target_draftd_identity {
        Some("the draftd binary differs from the one the interrupted install authorized".into())
    } else {
        None
    };
    if let Some(why) = mismatch {
        let rerun = if ctx.platform == InstallPlatform::Windows {
            format!("install.ps1 -Version {}", p.target_version)
        } else {
            format!("DRAFT_VERSION={} sh install.sh", p.target_version)
        };
        return Err(fail(InstallationFailure::FreshInstallRecoveryArtifactMismatch, format!("{why}; nothing was changed"))
            .with_suggestion(format!(
                "Re-run the installer for exactly that release, with the same DRAFT_INSTALL_ROOT: {rerun}"
            )));
    }
    recover_fresh(ctx, &layout, op, Some(package))
}

/// Case B / I68: generic recovery on an *installed* `draft`, dispatching all
/// four kinds. Derives the root from its own canonical location and accepts no
/// path argument.
pub fn recover_installed(
    ctx: &Context<'_>,
    own_executable: &Path,
    update: impl FnOnce(&InstallLayout, InstallationOperation) -> DraftResult<()>,
) -> DraftResult<()> {
    let layout = layout::from_executable(own_executable, ctx.platform)?;
    let _lock = operation::lock(&layout, LIFECYCLE_LOCK_TIMEOUT)?;
    let Some(op) = operation::read(&layout)? else {
        return Ok(());
    };
    op.validate(&layout)?;
    match op.kind {
        OperationKind::FreshInstall => {
            let OperationPayload::FreshInstall(p) = &op.payload else {
                unreachable!("kind checked")
            };
            if !p.target_draft_identity.matches(own_executable) {
                return Err(incompatible(
                    "this draft is not the binary the interrupted install authorized",
                ));
            }
            let package = Package {
                draft: layout.executable(Executable::Draft),
                draftd: layout.executable(Executable::Draftd),
                version: p.target_version.clone(),
                platform_target: p.platform_target.clone(),
            };
            recover_fresh(ctx, &layout, op, Some(&package))
        }
        OperationKind::BinaryUpdate | OperationKind::ChannelOnly => update(&layout, op),
        OperationKind::Uninstall => super::uninstall::resume_from_installed(
            &super::uninstall::Context {
                layout: &layout,
                host: ctx.host,
                registry: ctx.registry,
                faults: ctx.faults,
            },
            op,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installation::layout::canonicalize;
    use crate::installation::path::windows::{MemoryRegistry, RegistryType, UserPathValue};
    use crate::installation::update::testing::FakeHost;
    use crate::installation::{FaultPoint, NoFaults, INJECTED_CRASH};
    use crate::support::error::DraftErrorKind;

    struct World {
        _dir: tempfile::TempDir,
        base: PathBuf,
        root: PathBuf,
        path_bin: PathBuf,
        package: Package,
        host: FakeHost,
    }

    fn make_world(version: &str) -> World {
        let dir = tempfile::tempdir().unwrap();
        let base = canonicalize(dir.path()).unwrap();
        let pkg = base.join("extracted/bin");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("draft"), format!("draft {version}\n")).unwrap();
        std::fs::write(pkg.join("draftd"), format!("draftd {version}\n")).unwrap();
        let path_bin = base.join("pathbin");
        std::fs::create_dir_all(&path_bin).unwrap();
        World {
            root: base.join("root"),
            path_bin,
            package: Package {
                draft: pkg.join("draft"),
                draftd: pkg.join("draftd"),
                version: version.into(),
                platform_target: "x86_64-unknown-linux-musl".into(),
            },
            base,
            _dir: dir,
            host: FakeHost::default(),
        }
    }

    fn config(world: &World, migrate: bool) -> InstallConfig {
        InstallConfig {
            install_root: world.root.clone(),
            path_bin: Some(world.path_bin.clone()),
            update_path_requested: false,
            migrate_legacy: migrate,
            process_path: None,
        }
    }

    fn ctx<'a>(world: &'a World, faults: &'a dyn Faults) -> Context<'a> {
        Context {
            host: &world.host,
            registry: None,
            faults,
            platform: InstallPlatform::Unix,
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

    fn assert_installed(world: &World) {
        let layout = InstallLayout::new(&world.root, InstallPlatform::Unix);
        let receipt = receipt::read_final(&layout).unwrap();
        assert_eq!(receipt.install_generation, 1);
        assert_eq!(receipt.path_links.len(), 2);
        for executable in [Executable::Draft, Executable::Draftd] {
            assert_eq!(
                unix::observe(
                    &world.path_bin.join(executable.stem()),
                    &layout.executable(executable)
                ),
                unix::SlotObservation::ExpectedSymlink
            );
        }
        assert!(!layout.operation().exists());
        assert!(!layout.staging_root().exists());
        // A copied PATH executable can never masquerade: the link canonicalizes home.
        assert_eq!(
            layout::from_executable(&world.path_bin.join("draft"), InstallPlatform::Unix)
                .unwrap()
                .root(),
            layout.root()
        );
    }

    fn assert_pristine(world: &World) {
        let layout = InstallLayout::new(&world.root, InstallPlatform::Unix);
        assert!(is_inert_skeleton(&layout), "only the skeleton may remain");
        assert!(!layout.receipt().exists());
        assert!(std::fs::symlink_metadata(world.path_bin.join("draft")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_fresh_install_records_both_links_at_generation_one() {
        let world = make_world("0.3.4");
        let outcome = install(
            &ctx(&world, &NoFaults),
            &config(&world, false),
            &world.package,
        )
        .unwrap();
        assert!(matches!(outcome, InstallOutcome::Installed { .. }));
        assert_installed(&world);
        // A re-run over the valid installation changes nothing.
        let again = install(
            &ctx(&world, &NoFaults),
            &config(&world, false),
            &world.package,
        )
        .unwrap();
        assert!(matches!(again, InstallOutcome::AlreadyInstalled { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn every_fresh_install_crash_converges_to_installed_or_pristine() {
        let phases = [
            Phase::RootBinariesInstalled,
            Phase::BinariesReceiptCommitted,
            Phase::PathIntegrationApplied,
            Phase::PathIntegrationReceiptCommitted,
            Phase::InstalledBinariesValidated,
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
                let world = make_world("0.3.4");
                assert!(install(
                    &ctx(&world, &CrashAt(point)),
                    &config(&world, false),
                    &world.package
                )
                .is_err());
                let layout = InstallLayout::new(&world.root, InstallPlatform::Unix);
                // With an installed draft, installed-Draft recovery owns it;
                // otherwise the coordinator, for the exact same pair.
                if layout.executable(Executable::Draft).exists() {
                    assert_eq!(classify(&layout), Route::B, "{point:?}");
                    recover_installed(
                        &ctx(&world, &NoFaults),
                        &layout.executable(Executable::Draft),
                        |_, _| unreachable!(),
                    )
                    .unwrap_or_else(|error| panic!("{point:?}: {error:?}"));
                } else {
                    recover_as_coordinator(&ctx(&world, &NoFaults), &world.root, &world.package)
                        .unwrap_or_else(|error| panic!("{point:?}: {error:?}"));
                }
                if layout.receipt().exists() {
                    assert_installed(&world);
                } else {
                    assert_pristine(&world);
                }
                assert!(!layout.operation().exists(), "{point:?}");
                // Rolled back before path integration; forward after it.
                let forward = [
                    Phase::PathIntegrationReceiptCommitted,
                    Phase::InstalledBinariesValidated,
                    Phase::Committed,
                    Phase::CleanupPending,
                    Phase::Finalized,
                ];
                let expect_installed = forward.contains(&phase)
                    || (phase == Phase::PathIntegrationApplied
                        && point != FaultPoint::BeforeAction(phase));
                assert_eq!(layout.receipt().exists(), expect_installed, "{point:?}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_legacy_copy_is_refused_by_default_and_migrated_only_with_the_opt_in() {
        let world = make_world("0.3.4");
        std::fs::write(world.path_bin.join("draft"), "draft 0.3.3\n").unwrap();
        std::fs::write(world.path_bin.join("draftd"), "draftd 0.3.3\n").unwrap();
        let refused = install(
            &ctx(&world, &NoFaults),
            &config(&world, false),
            &world.package,
        )
        .unwrap_err();
        assert_eq!(
            super::super::failure_of(&refused),
            Some(InstallationFailure::LegacyInstallationConflict)
        );
        assert_eq!(
            std::fs::read_to_string(world.path_bin.join("draft")).unwrap(),
            "draft 0.3.3\n"
        );

        // A failure after the legacy files moved restores them exactly.
        let crash = CrashAt(FaultPoint::AfterPhase(Phase::LegacyPathStaged));
        assert!(install(&ctx(&world, &crash), &config(&world, true), &world.package).is_err());
        let layout = InstallLayout::new(&world.root, InstallPlatform::Unix);
        recover_installed(
            &ctx(&world, &NoFaults),
            &layout.executable(Executable::Draft),
            |_, _| unreachable!(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(world.path_bin.join("draft")).unwrap(),
            "draft 0.3.3\n"
        );
        assert_eq!(
            std::fs::read_to_string(world.path_bin.join("draftd")).unwrap(),
            "draftd 0.3.3\n"
        );

        install(
            &ctx(&world, &NoFaults),
            &config(&world, true),
            &world.package,
        )
        .unwrap();
        assert_installed(&world);
    }

    #[cfg(unix)]
    #[test]
    fn an_unrelated_draft_or_a_half_legacy_pair_is_never_touched() {
        let world = make_world("0.3.4");
        std::fs::write(
            world.path_bin.join("draft"),
            "#!/bin/sh\necho something else\n",
        )
        .unwrap();
        assert!(install(
            &ctx(&world, &NoFaults),
            &config(&world, true),
            &world.package
        )
        .is_err());
        assert!(std::fs::read_to_string(world.path_bin.join("draft"))
            .unwrap()
            .contains("something else"));
        let layout = InstallLayout::new(&world.root, InstallPlatform::Unix);
        assert!(
            operation::read(&layout).unwrap().is_none(),
            "refused before Resolved"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_mismatched_recovery_package_never_retargets() {
        let world = make_world("0.3.4");
        let crash = CrashAt(FaultPoint::AfterPhase(Phase::Resolved));
        assert!(install(&ctx(&world, &crash), &config(&world, false), &world.package).is_err());
        let layout = InstallLayout::new(&world.root, InstallPlatform::Unix);
        let before = std::fs::read(layout.operation()).unwrap();
        let other = make_world("0.3.5");
        let error = recover_as_coordinator(&ctx(&world, &NoFaults), &world.root, &other.package)
            .unwrap_err();
        assert_eq!(
            super::super::failure_of(&error),
            Some(InstallationFailure::FreshInstallRecoveryArtifactMismatch)
        );
        assert!(error.suggestion.unwrap().contains("DRAFT_VERSION=0.3.4"));
        assert_eq!(
            std::fs::read(layout.operation()).unwrap(),
            before,
            "the journal target is never rewritten"
        );
        assert!(!layout.bin_dir().exists() || !dir_has_entries(&layout.bin_dir()));
        recover_as_coordinator(&ctx(&world, &NoFaults), &world.root, &world.package).unwrap();
        assert_pristine(&world);
    }

    #[cfg(unix)]
    #[test]
    fn a_managed_update_journal_is_never_reinterpreted_by_a_downloaded_coordinator() {
        let world = make_world("0.3.4");
        install(
            &ctx(&world, &NoFaults),
            &config(&world, false),
            &world.package,
        )
        .unwrap();
        let layout = InstallLayout::new(&world.root, InstallPlatform::Unix);
        let receipt = receipt::read_final(&layout).unwrap();
        let op = InstallationOperation::new(
            receipt.installation_id,
            InstallationOperationId::generate(),
            OperationPayload::ChannelOnly(operation::ChannelOnlyPayload {
                previous_release_channel: ReleaseChannel::Stable,
                target_release_channel: ReleaseChannel::Prerelease,
                previous_generation: 1,
                target_generation: 2,
            }),
        );
        super::super::write_private_json(&layout.operation(), &op).unwrap();
        let error = recover_as_coordinator(&ctx(&world, &NoFaults), &world.root, &world.package)
            .unwrap_err();
        assert_eq!(
            super::super::failure_of(&error),
            Some(InstallationFailure::InstallationRecoveryIncompatible)
        );
        assert!(layout.operation().exists());
        assert_eq!(classify(&layout), Route::B);
    }

    #[cfg(unix)]
    #[test]
    fn terminal_residue_is_consumed_ready_last_and_orphans_fail_closed() {
        let world = make_world("0.3.4");
        let layout = InstallLayout::new(&world.root, InstallPlatform::Unix);
        layout.ensure_skeleton().unwrap();
        let op = InstallationOperationId::new("ilo_0123456789ab");
        let record = TerminalRecord {
            installation_id: InstallationId::new("ins_0123456789ab"),
            operation_id: op.clone(),
            mode: InstallPlatform::Unix,
        };
        // Orphan helper residue with no READY: case E, nothing removed.
        std::fs::create_dir_all(layout.staging(&op)).unwrap();
        std::fs::write(layout.helper(&op), b"helper").unwrap();
        assert_eq!(classify(&layout), Route::E);
        assert!(install(
            &ctx(&world, &NoFaults),
            &config(&world, false),
            &world.package
        )
        .is_err());
        assert!(layout.helper(&op).exists());
        // With READY (a Windows-style retained residue): case D, consumed, then installed.
        super::super::write_private_bytes(&layout.terminal_record(), record.render().as_bytes())
            .unwrap();
        assert_eq!(classify(&layout), Route::D);
        install(
            &ctx(&world, &NoFaults),
            &config(&world, false),
            &world.package,
        )
        .unwrap();
        assert!(!layout.terminal_record().exists());
        assert!(!layout.helper(&op).exists());
        assert_installed(&world);
        // A malformed READY is case E.
        let other = make_world("0.3.4");
        let layout = InstallLayout::new(&other.root, InstallPlatform::Unix);
        layout.ensure_skeleton().unwrap();
        std::fs::write(
            layout.terminal_record(),
            b"draft-terminal-cleanup 1\nstate preparing\n",
        )
        .unwrap();
        assert_eq!(classify(&layout), Route::E);
        assert!(install(
            &ctx(&other, &NoFaults),
            &config(&other, false),
            &other.package
        )
        .is_err());
        assert!(layout.terminal_record().exists());
    }

    #[cfg(unix)]
    #[test]
    fn install_then_uninstall_then_reinstall_mints_a_new_installation_id() {
        let world = make_world("0.3.4");
        install(
            &ctx(&world, &NoFaults),
            &config(&world, false),
            &world.package,
        )
        .unwrap();
        let layout = InstallLayout::new(&world.root, InstallPlatform::Unix);
        let first = receipt::read_final(&layout).unwrap().installation_id;
        let uctx = super::super::uninstall::Context {
            layout: &layout,
            host: &world.host,
            registry: None,
            faults: &NoFaults,
        };
        let receipt = receipt::read_final(&layout).unwrap();
        let operation = {
            let _lock = operation::lock(&layout, LIFECYCLE_LOCK_TIMEOUT).unwrap();
            super::super::uninstall::begin(&uctx, &receipt, None).unwrap()
        };
        super::super::uninstall::run_helper(
            &uctx,
            &layout.helper(&operation),
            &first,
            &operation,
            super::super::uninstall::HelperMode::ParentExit { parent_pid: 1 },
        )
        .unwrap();
        assert_eq!(classify(&layout), Route::F);
        install(
            &ctx(&world, &NoFaults),
            &config(&world, false),
            &world.package,
        )
        .unwrap();
        let second = receipt::read_final(&layout).unwrap().installation_id;
        assert_ne!(first, second);
        let _ = &world.base;
    }

    #[test]
    fn windows_fresh_install_reserves_and_rolls_back_exactly_its_delta() {
        let world = make_world("0.3.4");
        let registry = MemoryRegistry::new(UserPathValue::Absent);
        let ctx = Context {
            host: &world.host,
            registry: Some(&registry),
            faults: &NoFaults,
            platform: InstallPlatform::Windows,
        };
        let mut config = config(&world, false);
        config.update_path_requested = true;
        config.path_bin = None;
        let layout = InstallLayout::new(&world.root, InstallPlatform::Windows);
        // The Windows package names draft.exe; reuse the Unix files by copying.
        let pkg_dir = world.package.draft.parent().unwrap();
        std::fs::copy(&world.package.draft, pkg_dir.join("draft.exe")).unwrap();
        std::fs::copy(&world.package.draftd, pkg_dir.join("draftd.exe")).unwrap();
        let package = Package {
            draft: pkg_dir.join("draft.exe"),
            draftd: pkg_dir.join("draftd.exe"),
            version: "0.3.4".into(),
            platform_target: "x86_64-pc-windows-msvc".into(),
        };
        // Crash right after the PATH append, before its phase write.
        let crash = CrashAt(FaultPoint::AfterActionBeforePhase(
            Phase::PathIntegrationApplied,
        ));
        let crash_ctx = Context {
            faults: &crash,
            ..Context {
                host: &world.host,
                registry: Some(&registry),
                faults: &NoFaults,
                platform: InstallPlatform::Windows,
            }
        };
        assert!(install(&crash_ctx, &config, &package).is_err());
        let segment = windows::canonical_segment(&layout);
        assert_eq!(
            registry.current(),
            UserPathValue::Present {
                kind: RegistryType::Sz,
                raw: segment.clone()
            }
        );
        recover_installed(
            &ctx,
            &layout.executable(Executable::Draft),
            |_, _| unreachable!(),
        )
        .unwrap();
        let receipt = receipt::read_final(&layout).unwrap();
        let entry = receipt.windows_path.unwrap();
        assert_eq!(entry.provenance, WindowsPathProvenance::AddedByDraft);
        assert_eq!(
            registry.mutations.borrow().len(),
            1,
            "the reservation was satisfied, not re-applied"
        );
    }
}
