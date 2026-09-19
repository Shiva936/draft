//! `draft update` — the `BinaryUpdate` transaction and the `ChannelOnly`
//! commit.
//!
//! The two-binary replacement is transactional, never "one atomic rename":
//! backups are made and identity-checked first, each binary is replaced by a
//! same-filesystem rename, both installed binaries are executed by canonical
//! path, the daemon is restarted and health-checked when it had been running,
//! and only then is `receipt.json` atomically rewritten (`ReceiptCommitted`).
//! `Committed` can follow only `ReceiptCommitted`, so a new pair never stays
//! authoritative under an old receipt except transiently — and recovery closes
//! that window forward.
//!
//! Recovery (I28) always runs the liminal next-action probe (I44/I44a/I45)
//! first, then the phase's frozen branch:
//!
//! | Phase reached | Recovery |
//! |---|---|
//! | `Resolved` … `DaemonStopped` | discard staging; restart the old daemon if it had run |
//! | `BackupDraftCreated` … `DraftReplaced` | restore both binaries from `rollback/`, verified against `previous_*` |
//! | `DraftdReplaced` | validate the pair: forward on success, roll back on failure |
//! | `InstalledBinariesValidated`, `DaemonRestarted` | forward; roll back only if the restart fails |
//! | `ReceiptCommitted` and later | verify receipt ≡ installed pair ≡ target, finish — never roll back |

use std::path::Path;

use super::archive;
use super::layout::{Executable, InstallLayout};
use super::operation::{
    self, BinaryUpdatePayload, ChannelOnlyPayload, InstallationOperation, Journal,
    OperationPayload, Phase,
};
use super::receipt::{self, InstallationReceipt, ReleaseChannel};
use super::release::{self, Plan, ReleaseSource, TrustSet, MAX_RELEASE_ARTIFACT_BYTES};
use super::{
    fail, failure_of, is_injected_crash, Faults, Identity, InstallationFailure,
    InstallationOperationId, LifecycleHost, LIFECYCLE_LOCK_TIMEOUT,
};
use crate::support::error::{DraftError, DraftResult};

/// `draft update` flags.
#[derive(Debug, Clone, Default)]
pub struct UpdateRequest {
    pub check: bool,
    pub version: Option<semver::Version>,
    pub channel: Option<ReleaseChannel>,
    pub allow_downgrade: bool,
    /// Trust-bridge hops already taken by earlier executions of this command.
    pub hops: u32,
}

impl UpdateRequest {
    /// I36: meaningless combinations are rejected before any network call.
    pub fn validate(&self) -> DraftResult<()> {
        let reject = |why: &str| fail(InstallationFailure::InvalidUpdateFlagCombination, why);
        if self.version.is_some() && self.channel.is_some() {
            return Err(reject(
                "--version pins an exact release and --channel selects a track; use one",
            ));
        }
        if self.allow_downgrade && self.version.is_none() {
            return Err(reject(
                "--allow-downgrade is meaningful only with --version",
            ));
        }
        if self.allow_downgrade && self.check {
            return Err(reject(
                "--check mutates nothing, so --allow-downgrade has no meaning",
            ));
        }
        Ok(())
    }
}

/// What `--check` reports. Always exit 0 when produced.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CheckReport {
    pub current: String,
    pub latest: Option<String>,
    pub update_available: bool,
    pub channel: ReleaseChannel,
    /// A bridge release that would be installed first to refresh trust.
    pub bridge: Option<String>,
    pub below_trust_floor: bool,
    pub hop_limit_exceeded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum UpdateOutcome {
    UpToDate {
        version: String,
        channel: ReleaseChannel,
    },
    ChannelChanged {
        version: String,
        from: ReleaseChannel,
        to: ReleaseChannel,
    },
    Updated {
        from: String,
        to: String,
        channel: ReleaseChannel,
    },
    /// A trust bridge was installed; the caller re-executes the new binary with
    /// `hops + 1` to continue towards `target`.
    Bridged {
        via: String,
        target: String,
    },
    Checked(CheckReport),
}

/// Everything an update needs from its surroundings.
pub struct Context<'a> {
    pub layout: &'a InstallLayout,
    pub host: &'a dyn LifecycleHost,
    pub source: &'a dyn ReleaseSource,
    pub trust: &'a TrustSet,
    pub platform_target: &'a str,
    pub faults: &'a dyn Faults,
}

/// Run `draft update` against an already-validated official installation.
pub fn run(ctx: &Context<'_>, request: &UpdateRequest) -> DraftResult<UpdateOutcome> {
    request.validate()?;
    let _lock = operation::lock(ctx.layout, LIFECYCLE_LOCK_TIMEOUT)?;
    // An interrupted operation is resolved before anything new starts.
    if let Some(op) = operation::read(ctx.layout)? {
        recover(ctx, op)?;
    }
    let receipt = receipt::read_final(ctx.layout)?;
    let installed = semver::Version::parse(&receipt.installed_version).map_err(|_| {
        fail(
            InstallationFailure::InstallationReceiptInvalid,
            "bad version",
        )
    })?;
    let channel = request.channel.unwrap_or(receipt.release_channel);

    if request.check {
        return check(ctx, request, &receipt, &installed, channel).map(UpdateOutcome::Checked);
    }

    let plan = release::plan(
        ctx.source,
        ctx.trust,
        &installed,
        channel,
        request.version.as_ref(),
        ctx.platform_target,
        request.hops,
    )?;
    let channel_change = request
        .channel
        .filter(|wanted| *wanted != receipt.release_channel);
    match plan {
        Plan::UpToDate { .. } => up_to_date(ctx, &receipt, channel_change),
        Plan::Install(verified) => {
            if verified.version < installed {
                if request.version.is_none() {
                    // A channel resolution never selects a lower version.
                    return up_to_date(ctx, &receipt, channel_change);
                }
                if !request.allow_downgrade {
                    return Err(fail(
                        InstallationFailure::InvalidUpdateFlagCombination,
                        format!(
                            "Draft {} is older than the installed {installed}; pass \
                             --allow-downgrade to install it",
                            verified.version
                        ),
                    ));
                }
            }
            binary_update(ctx, &receipt, &verified, channel_change)?;
            Ok(UpdateOutcome::Updated {
                from: installed.to_string(),
                to: verified.version.to_string(),
                channel: channel_change.unwrap_or(receipt.release_channel),
            })
        }
        Plan::Bridge { bridge, target } => {
            // Each hop is a complete ordinary update; the track changes only
            // with the final commit.
            binary_update(ctx, &receipt, &bridge, None)?;
            Ok(UpdateOutcome::Bridged {
                via: bridge.version.to_string(),
                target: target.to_string(),
            })
        }
    }
}

fn up_to_date(
    ctx: &Context<'_>,
    receipt: &InstallationReceipt,
    channel_change: Option<ReleaseChannel>,
) -> DraftResult<UpdateOutcome> {
    match channel_change {
        None => Ok(UpdateOutcome::UpToDate {
            version: receipt.installed_version.clone(),
            channel: receipt.release_channel,
        }),
        Some(to) => {
            channel_only(ctx, receipt, to)?;
            Ok(UpdateOutcome::ChannelChanged {
                version: receipt.installed_version.clone(),
                from: receipt.release_channel,
                to,
            })
        }
    }
}

/// `--check`: resolve and verify only. Never downloads for installation,
/// replaces, signals the daemon, touches the receipt or re-executes.
fn check(
    ctx: &Context<'_>,
    request: &UpdateRequest,
    receipt: &InstallationReceipt,
    installed: &semver::Version,
    channel: ReleaseChannel,
) -> DraftResult<CheckReport> {
    let mut report = CheckReport {
        current: installed.to_string(),
        latest: None,
        update_available: false,
        channel,
        bridge: None,
        below_trust_floor: false,
        hop_limit_exceeded: false,
    };
    let _ = receipt;
    match release::plan(
        ctx.source,
        ctx.trust,
        installed,
        channel,
        request.version.as_ref(),
        ctx.platform_target,
        request.hops,
    ) {
        Ok(Plan::UpToDate { target }) => report.latest = Some(target.to_string()),
        Ok(Plan::Install(verified)) => {
            report.update_available = verified.version > *installed || request.version.is_some();
            report.latest = Some(verified.version.to_string());
        }
        Ok(Plan::Bridge { bridge, target }) => {
            report.update_available = true;
            report.latest = Some(target.to_string());
            report.bridge = Some(bridge.version.to_string());
        }
        Err(error) => match failure_of(&error) {
            Some(InstallationFailure::ReleaseTrustBridgeUnavailable) => {
                report.below_trust_floor = true;
                report.update_available = true;
            }
            Some(InstallationFailure::ReleaseTrustHopLimitExceeded) => {
                report.hop_limit_exceeded = true;
                report.update_available = true;
            }
            _ => return Err(error),
        },
    }
    Ok(report)
}

/// I19/I27b: switch the track without touching a binary or the daemon.
fn channel_only(
    ctx: &Context<'_>,
    receipt: &InstallationReceipt,
    to: ReleaseChannel,
) -> DraftResult<()> {
    let op = InstallationOperation::new(
        receipt.installation_id.clone(),
        InstallationOperationId::generate(),
        OperationPayload::ChannelOnly(ChannelOnlyPayload {
            previous_release_channel: receipt.release_channel,
            target_release_channel: to,
            previous_generation: receipt.install_generation,
            target_generation: receipt.install_generation + 1,
        }),
    );
    let journal = Journal::create(ctx.layout, op, ctx.faults)?;
    channel_forward(ctx, journal)
}

fn channel_forward(ctx: &Context<'_>, mut journal: Journal<'_>) -> DraftResult<()> {
    let OperationPayload::ChannelOnly(payload) = journal.op.payload.clone() else {
        unreachable!("channel_forward runs ChannelOnly journals only")
    };
    if journal.phase() == Phase::Resolved {
        journal.step(Phase::ReceiptCommitted, |journal| {
            let mut next = receipt::read_final(journal.layout)?;
            next.release_channel = payload.target_release_channel;
            next.install_generation = payload.target_generation;
            receipt::write(journal.layout, &next)
        })?;
    }
    finish(ctx, journal)
}

/// The shared tail: `Committed` → `CleanupPending` → `Finalized` → journal gone.
fn finish(ctx: &Context<'_>, mut journal: Journal<'_>) -> DraftResult<()> {
    let op_id = journal.op.operation_id.clone();
    if journal.phase() == Phase::ReceiptCommitted {
        journal.step(Phase::Committed, |_| Ok(()))?;
    }
    if journal.phase() == Phase::Committed {
        journal.step(Phase::CleanupPending, |journal| {
            discard(journal.layout, &op_id);
            Ok(())
        })?;
    }
    if journal.phase() == Phase::CleanupPending {
        journal.step(Phase::Finalized, |journal| {
            discard(journal.layout, &op_id);
            Ok(())
        })?;
    }
    let _ = ctx;
    journal.delete()
}

/// Remove this operation's staging and rollback content.
fn discard(layout: &InstallLayout, op: &InstallationOperationId) {
    let _ = std::fs::remove_dir_all(layout.staging(op));
    let _ = std::fs::remove_dir_all(layout.rollback(op));
    super::remove_dir_if_empty(&layout.staging_root());
    super::remove_dir_if_empty(&layout.rollback_root());
}

fn payload(journal: &Journal<'_>) -> BinaryUpdatePayload {
    match &journal.op.payload {
        OperationPayload::BinaryUpdate(payload) => payload.clone(),
        _ => unreachable!("BinaryUpdate engine runs BinaryUpdate journals only"),
    }
}

fn target_identity(payload: &BinaryUpdatePayload, executable: Executable) -> DraftResult<Identity> {
    match executable {
        Executable::Draft => payload.target_draft_identity.clone(),
        Executable::Draftd => payload.target_draftd_identity.clone(),
    }
    .ok_or_else(|| {
        fail(
            InstallationFailure::RecoveryFailed,
            "target identities were never journalled",
        )
    })
}

fn previous_identity(payload: &BinaryUpdatePayload, executable: Executable) -> Identity {
    match executable {
        Executable::Draft => payload.previous_draft_identity.clone(),
        Executable::Draftd => payload.previous_draftd_identity.clone(),
    }
}

fn validate_pair(
    host: &dyn LifecycleHost,
    draft: &Path,
    draftd: &Path,
    version: &str,
) -> DraftResult<()> {
    for exe in [draft, draftd] {
        let reported = host.binary_version(exe).map_err(|error| {
            fail(
                InstallationFailure::ValidationFailed,
                format!(
                    "{} did not report a version: {}",
                    exe.display(),
                    error.message
                ),
            )
        })?;
        if reported != version {
            return Err(fail(
                InstallationFailure::ValidationFailed,
                format!("{} reports {reported}, expected {version}", exe.display()),
            ));
        }
    }
    Ok(())
}

/// Replace `dest` with `staged` by same-filesystem rename. Where the platform
/// refuses to rename over a running image (Windows), the installed file is
/// first renamed aside inside this operation's staging, never deleted.
fn replace(staged: &Path, dest: &Path, displaced: &Path) -> DraftResult<()> {
    match std::fs::rename(staged, dest) {
        Ok(()) => Ok(()),
        Err(first) => {
            if cfg!(windows) && dest.exists() {
                std::fs::rename(dest, displaced)
                    .and_then(|()| std::fs::rename(staged, dest))
                    .map_err(|error| {
                        fail(
                            InstallationFailure::ReplacementFailed,
                            format!("{}: {error}", dest.display()),
                        )
                    })
            } else {
                Err(fail(
                    InstallationFailure::ReplacementFailed,
                    format!("replace {}: {first}", dest.display()),
                ))
            }
        }
    }?;
    if let Some(parent) = dest.parent() {
        crate::support::fsutil::sync_directory(parent)?;
    }
    Ok(())
}

/// Copy `from` to `to` (create-new), flush, and verify its identity.
fn copy_verified(from: &Path, to: &Path, identity: &Identity) -> DraftResult<()> {
    if let Some(parent) = to.parent() {
        crate::support::fsutil::ensure_dir(parent)?;
    }
    super::remove_file_if_present(to)?;
    std::fs::copy(from, to)
        .map_err(|error| DraftError::storage(format!("copy {}: {error}", from.display())))?;
    std::fs::File::open(to)
        .and_then(|file| file.sync_all())
        .map_err(|error| DraftError::storage(format!("sync {}: {error}", to.display())))?;
    if !identity.matches(to) {
        return Err(fail(
            InstallationFailure::InstallationEntryIdentityMismatch,
            format!("{} does not have the expected identity", to.display()),
        ));
    }
    Ok(())
}

fn binary_update(
    ctx: &Context<'_>,
    receipt: &InstallationReceipt,
    release: &release::VerifiedRelease,
    channel_change: Option<ReleaseChannel>,
) -> DraftResult<()> {
    let artifact = release.manifest.artifact_for(ctx.platform_target)?.clone();
    let op = InstallationOperation::new(
        receipt.installation_id.clone(),
        InstallationOperationId::generate(),
        OperationPayload::BinaryUpdate(BinaryUpdatePayload {
            previous_version: receipt.installed_version.clone(),
            target_version: release.version.to_string(),
            previous_generation: receipt.install_generation,
            target_generation: receipt.install_generation + 1,
            previous_release_channel: receipt.release_channel,
            target_release_channel: channel_change,
            previous_draft_identity: receipt.draft_executable.identity(),
            previous_draftd_identity: receipt.draftd_executable.identity(),
            target_draft_identity: None,
            target_draftd_identity: None,
            target_tag: release.tag.clone(),
            artifact_name: artifact.asset.clone(),
            artifact: Identity {
                sha256: artifact.sha256.clone(),
                size: artifact.size,
            },
            daemon_was_running: ctx.host.daemon_running(),
        }),
    );
    let journal = Journal::create(ctx.layout, op, ctx.faults)?;
    std::fs::create_dir_all(ctx.layout.staging(&journal.op.operation_id))
        .map_err(|error| DraftError::storage(format!("create staging: {error}")))?;
    drive(ctx, journal)
}

/// Run forward from the journal's phase; on a genuine failure, apply the
/// failing phase's frozen recovery branch. An injected crash stops dead.
fn drive(ctx: &Context<'_>, mut journal: Journal<'_>) -> DraftResult<()> {
    match forward(ctx, &mut journal) {
        Ok(()) => finish(ctx, journal),
        Err(error) if is_injected_crash(&error) => Err(error),
        Err(error) => {
            if journal.op.reached(Phase::ReceiptCommitted) {
                // The receipt names the target pair: never roll back now.
                return Err(error);
            }
            unwind(ctx, journal)?;
            Err(error)
        }
    }
}

fn forward(ctx: &Context<'_>, journal: &mut Journal<'_>) -> DraftResult<()> {
    let op = journal.op.operation_id.clone();
    let layout = ctx.layout;
    let archive_path = layout.staging(&op).join(payload(journal).artifact_name);
    let extract_dir = layout.staging(&op).join("package");
    loop {
        let Some(next) = journal.op.next_phase() else {
            return Ok(());
        };
        let p = payload(journal);
        match next {
            Phase::Downloaded => journal.step(next, |_| {
                super::remove_file_if_present(&archive_path)?;
                if !ctx.source.fetch_to(
                    &p.target_tag,
                    &p.artifact_name,
                    MAX_RELEASE_ARTIFACT_BYTES,
                    &archive_path,
                )? {
                    return Err(fail(
                        InstallationFailure::ReleaseUnavailable,
                        format!("{} is not published", p.artifact_name),
                    ));
                }
                Ok(())
            })?,
            Phase::Verified => journal.step(next, |_| {
                archive::verify(
                    &archive_path,
                    &release::ManifestArtifact {
                        target: ctx.platform_target.into(),
                        asset: p.artifact_name.clone(),
                        sha256: p.artifact.sha256.clone(),
                        size: p.artifact.size,
                    },
                )
            })?,
            Phase::Extracted => journal.step(next, |_| {
                let _ = std::fs::remove_dir_all(&extract_dir);
                archive::extract(
                    &archive_path,
                    &p.target_version,
                    ctx.platform_target,
                    &extract_dir,
                    layout.platform(),
                )
                .map(|_| ())
            })?,
            Phase::Staged => {
                ctx.faults.at(super::FaultPoint::BeforeAction(next))?;
                let mut identities = Vec::new();
                for executable in [Executable::Draft, Executable::Draftd] {
                    let extracted = extract_dir.join(layout.relative(executable));
                    let identity = Identity::of_file(&extracted)?;
                    copy_verified(&extracted, &layout.staged(&op, executable), &identity)?;
                    identities.push(identity);
                }
                ctx.faults
                    .at(super::FaultPoint::AfterActionBeforePhase(next))?;
                let (draft, draftd) = (identities[0].clone(), identities[1].clone());
                journal.advance_with(next, |payload| {
                    if let OperationPayload::BinaryUpdate(payload) = payload {
                        payload.target_draft_identity = Some(draft);
                        payload.target_draftd_identity = Some(draftd);
                    }
                })?;
            }
            Phase::StagedBinariesValidated => journal.step(next, |_| {
                validate_pair(
                    ctx.host,
                    &layout.staged(&op, Executable::Draft),
                    &layout.staged(&op, Executable::Draftd),
                    &p.target_version,
                )
            })?,
            Phase::DaemonStopped => journal.step(next, |_| {
                if ctx.host.daemon_running() {
                    ctx.host.stop_daemon().map_err(|error| {
                        fail(InstallationFailure::DaemonStopFailed, error.message)
                    })?;
                }
                Ok(())
            })?,
            Phase::BackupDraftCreated | Phase::BackupDraftdCreated => {
                let executable = if next == Phase::BackupDraftCreated {
                    Executable::Draft
                } else {
                    Executable::Draftd
                };
                journal.step(next, |_| {
                    copy_verified(
                        &layout.executable(executable),
                        &layout.backup(&op, executable),
                        &previous_identity(&p, executable),
                    )
                })?
            }
            Phase::DraftReplaced | Phase::DraftdReplaced => {
                let executable = if next == Phase::DraftReplaced {
                    Executable::Draft
                } else {
                    Executable::Draftd
                };
                journal.step(next, |_| {
                    let installed = layout.executable(executable);
                    let target = target_identity(&p, executable)?;
                    if target.matches(&installed) {
                        return Ok(());
                    }
                    if !previous_identity(&p, executable).matches(&installed) {
                        return Err(fail(
                            InstallationFailure::InstallationEntryIdentityMismatch,
                            format!("{} changed underneath the update", installed.display()),
                        ));
                    }
                    let displaced = layout
                        .staging(&op)
                        .join(format!("{}.displaced", layout.file_name(executable)));
                    replace(&layout.staged(&op, executable), &installed, &displaced)
                })?
            }
            Phase::InstalledBinariesValidated => journal.step(next, |_| {
                validate_pair(
                    ctx.host,
                    &layout.executable(Executable::Draft),
                    &layout.executable(Executable::Draftd),
                    &p.target_version,
                )
            })?,
            Phase::DaemonRestarted => journal.step(next, |_| {
                if p.daemon_was_running && !ctx.host.daemon_healthy() {
                    ctx.host
                        .start_daemon(&layout.executable(Executable::Draftd))
                        .and_then(|()| {
                            if ctx.host.daemon_healthy() {
                                Ok(())
                            } else {
                                Err(DraftError::storage("the new daemon is not healthy"))
                            }
                        })
                        .map_err(|error| {
                            fail(InstallationFailure::DaemonRestartFailed, error.message)
                        })?;
                }
                Ok(())
            })?,
            Phase::ReceiptCommitted => journal.step(next, |journal| {
                let mut next_receipt = receipt::read_final(journal.layout)?;
                commit_receipt(&mut next_receipt, &p)?;
                receipt::write(journal.layout, &next_receipt)
            })?,
            Phase::Committed | Phase::CleanupPending | Phase::Finalized => return Ok(()),
            other => {
                return Err(fail(
                    InstallationFailure::RecoveryFailed,
                    format!("{other:?} is not a BinaryUpdate phase"),
                ))
            }
        }
    }
}

fn commit_receipt(receipt: &mut InstallationReceipt, p: &BinaryUpdatePayload) -> DraftResult<()> {
    let draft = target_identity(p, Executable::Draft)?;
    let draftd = target_identity(p, Executable::Draftd)?;
    receipt.installed_version = p.target_version.clone();
    receipt.install_generation = p.target_generation;
    receipt.draft_executable.sha256 = draft.sha256;
    receipt.draft_executable.size = draft.size;
    receipt.draftd_executable.sha256 = draftd.sha256;
    receipt.draftd_executable.size = draftd.size;
    if let Some(channel) = p.target_release_channel {
        receipt.release_channel = channel;
    }
    receipt.installed_at = crate::support::common::now();
    Ok(())
}

/// Whether the receipt equals the operation's target (I44a).
fn receipt_is_target(receipt: &InstallationReceipt, p: &BinaryUpdatePayload) -> bool {
    let (Some(draft), Some(draftd)) = (&p.target_draft_identity, &p.target_draftd_identity) else {
        return false;
    };
    receipt.installed_version == p.target_version
        && receipt.install_generation == p.target_generation
        && &receipt.draft_executable.identity() == draft
        && &receipt.draftd_executable.identity() == draftd
        && p.target_release_channel
            .is_none_or(|channel| receipt.release_channel == channel)
}

fn receipt_is_previous(receipt: &InstallationReceipt, p: &BinaryUpdatePayload) -> bool {
    receipt.installed_version == p.previous_version
        && receipt.install_generation == p.previous_generation
        && receipt.draft_executable.identity() == p.previous_draft_identity
        && receipt.draftd_executable.identity() == p.previous_draftd_identity
        && receipt.release_channel == p.previous_release_channel
}

/// The discard/rollback branch for the journal's current phase, then the
/// journal is removed: the operation failed and left the previous state.
fn unwind(ctx: &Context<'_>, journal: Journal<'_>) -> DraftResult<()> {
    let p = payload(&journal);
    let op = journal.op.operation_id.clone();
    let layout = ctx.layout;
    if journal.op.reached(Phase::BackupDraftCreated) {
        for executable in [Executable::Draft, Executable::Draftd] {
            let installed = layout.executable(executable);
            let previous = previous_identity(&p, executable);
            if previous.matches(&installed) {
                continue;
            }
            let backup = layout.backup(&op, executable);
            if !previous.matches(&backup) {
                return Err(fail(
                    InstallationFailure::RollbackFailed,
                    format!("no verified backup of {} to restore", installed.display()),
                ));
            }
            let staging = layout
                .staging(&op)
                .join(format!("{}.restore", layout.file_name(executable)));
            copy_verified(&backup, &staging, &previous)?;
            let displaced = layout
                .staging(&op)
                .join(format!("{}.rolledback", layout.file_name(executable)));
            replace(&staging, &installed, &displaced)?;
            if !previous.matches(&installed) {
                return Err(fail(
                    InstallationFailure::RollbackFailed,
                    format!(
                        "{} did not return to its previous identity",
                        installed.display()
                    ),
                ));
            }
        }
    }
    // The receipt was never touched before `ReceiptCommitted`.
    if p.daemon_was_running && !ctx.host.daemon_healthy() {
        if let Err(error) = ctx
            .host
            .start_daemon(&layout.executable(Executable::Draftd))
        {
            return Err(fail(
                InstallationFailure::RecoveryFailed,
                format!(
                    "the previous binaries are restored, but the previous daemon did not restart: {}",
                    error.message
                ),
            ));
        }
    }
    discard(layout, &op);
    journal.delete()
}

/// The liminal next-action probe (I44): `Some(true)` completed, `Some(false)`
/// not completed, `Err` ambiguous.
fn probe(ctx: &Context<'_>, journal: &Journal<'_>, next: Phase) -> DraftResult<bool> {
    let layout = ctx.layout;
    let op = &journal.op.operation_id;
    let ambiguous = |why: String| fail(InstallationFailure::RecoveryFailed, why);
    match &journal.op.payload {
        OperationPayload::ChannelOnly(p) if next == Phase::ReceiptCommitted => {
            let receipt = receipt::read_final(layout)?;
            if receipt.release_channel == p.target_release_channel
                && receipt.install_generation == p.target_generation
            {
                Ok(true)
            } else if receipt.release_channel == p.previous_release_channel
                && receipt.install_generation == p.previous_generation
            {
                Ok(false)
            } else {
                Err(ambiguous(
                    "the receipt matches neither the previous nor the target channel".into(),
                ))
            }
        }
        OperationPayload::BinaryUpdate(p) => match next {
            Phase::Downloaded => {
                let path = layout.staging(op).join(&p.artifact_name);
                if p.artifact.matches(&path) {
                    Ok(true)
                } else {
                    super::remove_file_if_present(&path)?;
                    Ok(false)
                }
            }
            Phase::Staged => Ok(
                match (&p.target_draft_identity, &p.target_draftd_identity) {
                    (Some(draft), Some(draftd)) => {
                        draft.matches(&layout.staged(op, Executable::Draft))
                            && draftd.matches(&layout.staged(op, Executable::Draftd))
                    }
                    _ => false,
                },
            ),
            Phase::DaemonStopped => Ok(!ctx.host.daemon_running()),
            Phase::BackupDraftCreated | Phase::BackupDraftdCreated => {
                let executable = if next == Phase::BackupDraftCreated {
                    Executable::Draft
                } else {
                    Executable::Draftd
                };
                let backup = layout.backup(op, executable);
                if !backup.exists() {
                    Ok(false)
                } else if previous_identity(p, executable).matches(&backup) {
                    Ok(true)
                } else {
                    Err(ambiguous(format!(
                        "{} matches no recorded identity",
                        backup.display()
                    )))
                }
            }
            Phase::DraftReplaced | Phase::DraftdReplaced => {
                let executable = if next == Phase::DraftReplaced {
                    Executable::Draft
                } else {
                    Executable::Draftd
                };
                let installed = layout.executable(executable);
                if previous_identity(p, executable).matches(&installed) {
                    Ok(false)
                } else if target_identity(p, executable)?.matches(&installed) {
                    Ok(true)
                } else {
                    Err(ambiguous(format!(
                        "{} matches neither the previous nor the target",
                        installed.display()
                    )))
                }
            }
            Phase::DaemonRestarted => Ok(!p.daemon_was_running || ctx.host.daemon_healthy()),
            Phase::ReceiptCommitted => {
                let receipt = receipt::read_final(layout)?;
                if receipt_is_target(&receipt, p) {
                    Ok(true)
                } else if receipt_is_previous(&receipt, p) {
                    Ok(false)
                } else {
                    Err(ambiguous(
                        "the receipt matches neither the previous nor the target".into(),
                    ))
                }
            }
            // Pure or idempotent actions are simply re-run by their branch.
            _ => Ok(false),
        },
        _ => Ok(false),
    }
}

/// Recover an interrupted `BinaryUpdate` or `ChannelOnly` (I28).
pub fn recover(ctx: &Context<'_>, op: InstallationOperation) -> DraftResult<()> {
    op.validate(ctx.layout)?;
    let mut journal = Journal::resume(ctx.layout, op, ctx.faults);
    // Liminal reconciliation: advance past every action that completed before
    // its phase write, never repeating it.
    while let Some(next) = journal.op.next_phase() {
        if matches!(
            next,
            Phase::Committed | Phase::CleanupPending | Phase::Finalized
        ) {
            break;
        }
        if probe(ctx, &journal, next)? {
            journal.advance(next)?;
        } else {
            break;
        }
    }
    match journal.op.payload.clone() {
        OperationPayload::ChannelOnly(_) => {
            if journal.op.reached(Phase::ReceiptCommitted) {
                finish(ctx, journal)
            } else {
                // Nothing was mutated.
                journal.delete()
            }
        }
        OperationPayload::BinaryUpdate(p) => {
            let phase = journal.phase();
            if journal.op.reached(Phase::ReceiptCommitted) {
                let receipt = receipt::read_final(ctx.layout)?;
                let pair_ok = [Executable::Draft, Executable::Draftd]
                    .iter()
                    .all(|executable| {
                        target_identity(&p, *executable).is_ok_and(|identity| {
                            identity.matches(&ctx.layout.executable(*executable))
                        })
                    });
                if !receipt_is_target(&receipt, &p) || !pair_ok {
                    return Err(fail(
                        InstallationFailure::RecoveryFailed,
                        "the receipt and the installed pair disagree with the committed target",
                    ));
                }
                finish(ctx, journal)
            } else if phase == Phase::DraftdReplaced
                || journal.op.reached(Phase::InstalledBinariesValidated)
            {
                drive(ctx, journal)
            } else {
                unwind(ctx, journal)
            }
        }
        _ => Err(fail(
            InstallationFailure::RecoveryFailed,
            "the update engine only recovers BinaryUpdate and ChannelOnly operations",
        )),
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! A fake machine: binaries are small files reading `<name> <version>`.
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;

    #[derive(Default)]
    pub struct FakeHost {
        pub running: Cell<bool>,
        pub stops: Cell<u32>,
        pub starts: RefCell<Vec<PathBuf>>,
        /// Daemon binaries whose content contains this refuse to start.
        pub refuse_start_marker: RefCell<Option<String>>,
        #[allow(clippy::type_complexity)]
        pub launched: RefCell<
            Vec<(
                PathBuf,
                crate::installation::InstallationId,
                InstallationOperationId,
            )>,
        >,
    }

    impl LifecycleHost for FakeHost {
        fn daemon_running(&self) -> bool {
            self.running.get()
        }
        fn stop_daemon(&self) -> DraftResult<()> {
            self.stops.set(self.stops.get() + 1);
            self.running.set(false);
            Ok(())
        }
        fn start_daemon(&self, draftd: &Path) -> DraftResult<()> {
            self.starts.borrow_mut().push(draftd.to_path_buf());
            let content = std::fs::read_to_string(draftd).unwrap_or_default();
            if let Some(marker) = self.refuse_start_marker.borrow().as_ref() {
                if content.contains(marker.as_str()) {
                    return Err(DraftError::storage("the daemon did not start"));
                }
            }
            self.running.set(true);
            Ok(())
        }
        fn daemon_healthy(&self) -> bool {
            self.running.get()
        }
        fn binary_version(&self, exe: &Path) -> DraftResult<String> {
            let text = std::fs::read_to_string(exe)
                .map_err(|error| DraftError::storage(error.to_string()))?;
            super::super::parse_reported_version(&text)
                .filter(|_| !text.contains("broken"))
                .ok_or_else(|| DraftError::storage("no version"))
        }
        fn launch_helper(
            &self,
            helper: &Path,
            installation: &crate::installation::InstallationId,
            operation: &InstallationOperationId,
        ) -> DraftResult<()> {
            self.launched.borrow_mut().push((
                helper.to_path_buf(),
                installation.clone(),
                operation.clone(),
            ));
            Ok(())
        }
        fn wait_for_exit(&self, _: u32, _: std::time::Duration) -> DraftResult<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeHost;
    use super::*;
    use crate::installation::archive::testing::package;
    use crate::installation::layout::canonicalize;
    use crate::installation::operation::Phase::*;
    use crate::installation::receipt::fixtures;
    use crate::installation::release::testing::{self as rel, Key};
    use crate::installation::{FaultPoint, InstallPlatform, NoFaults, INJECTED_CRASH};
    use crate::support::error::DraftErrorKind;

    const TARGET: &str = "x86_64-unknown-linux-musl";

    struct World {
        _dir: tempfile::TempDir,
        layout: InstallLayout,
        host: FakeHost,
        releases: rel::FakeHost,
        key: Key,
    }

    fn binary(name: &str, version: &str) -> Vec<u8> {
        format!("{name} {version}\n").into_bytes()
    }

    fn make_world(installed: &str) -> World {
        let dir = tempfile::tempdir().unwrap();
        let root = canonicalize(dir.path()).unwrap().join("root");
        let layout = InstallLayout::new(&root, InstallPlatform::Unix);
        std::fs::create_dir_all(layout.bin_dir()).unwrap();
        std::fs::write(
            layout.executable(Executable::Draft),
            binary("draft", installed),
        )
        .unwrap();
        std::fs::write(
            layout.executable(Executable::Draftd),
            binary("draftd", installed),
        )
        .unwrap();
        let mut receipt = fixtures::unix(&layout, &dir.path().join("pathbin"));
        receipt.installed_version = installed.into();
        receipt.draft_executable.sha256 = Identity::of_file(&layout.executable(Executable::Draft))
            .unwrap()
            .sha256;
        receipt.draft_executable.size = binary("draft", installed).len() as u64;
        receipt.draftd_executable.sha256 =
            Identity::of_file(&layout.executable(Executable::Draftd))
                .unwrap()
                .sha256;
        receipt.draftd_executable.size = binary("draftd", installed).len() as u64;
        layout.ensure_skeleton().unwrap();
        receipt::write(&layout, &receipt).unwrap();
        World {
            _dir: dir,
            layout,
            host: FakeHost::default(),
            releases: rel::FakeHost::default(),
            key: Key::new(1),
        }
    }

    fn publish(world: &mut World, version: &str, draftd_payload: &[u8]) {
        let bytes = package(version, TARGET, &binary("draft", version), draftd_payload);
        let asset = archive::artifact_name(version, TARGET);
        let identity = {
            let path = world._dir.path().join(&asset);
            std::fs::write(&path, &bytes).unwrap();
            Identity::of_file(&path).unwrap()
        };
        world.releases.publish(
            version,
            version.contains('-'),
            &world.key,
            &[],
            &[&world.key],
            vec![release::ManifestArtifact {
                target: TARGET.into(),
                asset: asset.clone(),
                sha256: identity.sha256,
                size: identity.size,
            }],
        );
        world
            .releases
            .assets
            .insert((format!("v{version}"), asset), bytes);
    }

    fn run_with(
        world: &World,
        request: &UpdateRequest,
        faults: &dyn Faults,
    ) -> DraftResult<UpdateOutcome> {
        let trust = rel::trust(&[&world.key]);
        run(
            &Context {
                layout: &world.layout,
                host: &world.host,
                source: &world.releases,
                trust: &trust,
                platform_target: TARGET,
                faults,
            },
            request,
        )
    }

    fn installed_versions(world: &World) -> (String, String, String, u64) {
        let receipt = receipt::read_final(&world.layout).unwrap();
        let read = |e| {
            std::fs::read_to_string(world.layout.executable(e))
                .unwrap()
                .trim()
                .to_string()
        };
        (
            read(Executable::Draft),
            read(Executable::Draftd),
            receipt.installed_version,
            receipt.install_generation,
        )
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

    #[test]
    fn a_normal_update_replaces_both_binaries_and_commits_the_receipt() {
        let mut world = make_world("0.3.4");
        publish(&mut world, "0.4.0", &binary("draftd", "0.4.0"));
        world.host.running.set(true);
        let outcome = run_with(&world, &UpdateRequest::default(), &NoFaults).unwrap();
        assert!(matches!(outcome, UpdateOutcome::Updated { .. }));
        assert_eq!(
            installed_versions(&world),
            (
                "draft 0.4.0".into(),
                "draftd 0.4.0".into(),
                "0.4.0".into(),
                2
            )
        );
        let receipt = receipt::read_final(&world.layout).unwrap();
        assert!(receipt
            .draft_executable
            .identity()
            .matches(&world.layout.executable(Executable::Draft)));
        assert!(world.host.running.get(), "a running daemon is restarted");
        assert!(!world.layout.operation().exists());
        assert!(!world.layout.staging_root().exists());
        assert!(world.layout.lock().exists());
        // Idempotent: the same run is now a no-mutation success.
        assert!(matches!(
            run_with(&world, &UpdateRequest::default(), &NoFaults).unwrap(),
            UpdateOutcome::UpToDate { .. }
        ));
    }

    #[test]
    fn every_crash_window_converges_to_the_previous_or_the_target_pair() {
        let phases = [
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
        ];
        for phase in phases {
            for point in [
                FaultPoint::BeforeAction(phase),
                FaultPoint::AfterActionBeforePhase(phase),
                FaultPoint::AfterPhase(phase),
            ] {
                let mut world = make_world("0.3.4");
                publish(&mut world, "0.4.0", &binary("draftd", "0.4.0"));
                world.host.running.set(true);
                let crashed = run_with(&world, &UpdateRequest::default(), &CrashAt(point));
                assert!(crashed.is_err(), "{point:?}");
                // The next invocation recovers first, then evaluates again.
                let result = run_with(&world, &UpdateRequest::default(), &NoFaults);
                assert!(result.is_ok(), "{point:?}: {result:?}");
                let (draft, draftd, version, generation) = installed_versions(&world);
                assert_eq!(
                    (draft.as_str(), draftd.as_str(), version.as_str()),
                    ("draft 0.4.0", "draftd 0.4.0", "0.4.0"),
                    "{point:?}"
                );
                assert_eq!(generation, 2, "{point:?}: never a double increment");
                assert!(!world.layout.operation().exists(), "{point:?}");
                assert!(world.host.running.get(), "{point:?}");
            }
        }
    }

    #[test]
    fn a_crash_before_the_receipt_commit_is_resolved_by_rolling_back_or_forward_but_never_mixed() {
        for phase in [BackupDraftCreated, BackupDraftdCreated, DraftReplaced] {
            let mut world = make_world("0.3.4");
            publish(&mut world, "0.4.0", &binary("draftd", "0.4.0"));
            let _ = run_with(
                &world,
                &UpdateRequest::default(),
                &CrashAt(FaultPoint::AfterPhase(phase)),
            );
            let op = operation::read(&world.layout).unwrap().unwrap();
            let trust = rel::trust(&[&world.key]);
            let ctx = Context {
                layout: &world.layout,
                host: &world.host,
                source: &world.releases,
                trust: &trust,
                platform_target: TARGET,
                faults: &NoFaults,
            };
            let _lock = operation::lock(&world.layout, LIFECYCLE_LOCK_TIMEOUT).unwrap();
            recover(&ctx, op).unwrap();
            assert_eq!(
                installed_versions(&world),
                (
                    "draft 0.3.4".into(),
                    "draftd 0.3.4".into(),
                    "0.3.4".into(),
                    1
                ),
                "{phase:?}"
            );
        }
    }

    #[test]
    fn a_validation_or_restart_failure_rolls_the_pair_back() {
        // The new daemon binary is broken: installed validation fails.
        let mut world = make_world("0.3.4");
        publish(&mut world, "0.4.0", b"draftd 0.4.0 broken\n");
        let error = run_with(&world, &UpdateRequest::default(), &NoFaults).unwrap_err();
        assert_eq!(
            failure_of(&error),
            Some(InstallationFailure::ValidationFailed)
        );
        assert_eq!(installed_versions(&world).2, "0.3.4");

        // The new daemon will not start: roll back and restart the old one.
        let mut world = make_world("0.3.4");
        publish(&mut world, "0.4.0", b"draftd 0.4.0 nostart\n");
        world.host.running.set(true);
        *world.host.refuse_start_marker.borrow_mut() = Some("nostart".into());
        let error = run_with(&world, &UpdateRequest::default(), &NoFaults).unwrap_err();
        assert_eq!(
            failure_of(&error),
            Some(InstallationFailure::DaemonRestartFailed)
        );
        assert_eq!(
            installed_versions(&world),
            (
                "draft 0.3.4".into(),
                "draftd 0.3.4".into(),
                "0.3.4".into(),
                1
            )
        );
        assert!(world.host.running.get(), "the old daemon is back");
    }

    #[test]
    fn a_stopped_daemon_stays_stopped() {
        let mut world = make_world("0.3.4");
        publish(&mut world, "0.4.0", &binary("draftd", "0.4.0"));
        run_with(&world, &UpdateRequest::default(), &NoFaults).unwrap();
        assert!(!world.host.running.get());
        assert!(world.host.starts.borrow().is_empty());
    }

    #[test]
    fn a_channel_only_commit_touches_no_binary_and_increments_once() {
        let mut world = make_world("0.4.0");
        publish(&mut world, "0.4.0", &binary("draftd", "0.4.0"));
        let before = std::fs::read(world.layout.executable(Executable::Draft)).unwrap();
        let request = UpdateRequest {
            channel: Some(ReleaseChannel::Prerelease),
            ..Default::default()
        };
        let outcome = run_with(&world, &request, &NoFaults).unwrap();
        assert!(matches!(outcome, UpdateOutcome::ChannelChanged { .. }));
        let receipt = receipt::read_final(&world.layout).unwrap();
        assert_eq!(
            (receipt.release_channel, receipt.install_generation),
            (ReleaseChannel::Prerelease, 2)
        );
        assert_eq!(
            std::fs::read(world.layout.executable(Executable::Draft)).unwrap(),
            before
        );
        assert_eq!(world.host.stops.get(), 0);
        assert!(!world
            .releases
            .fetches
            .borrow()
            .iter()
            .any(|asset| asset.ends_with(".tar.gz")));

        for point in [
            FaultPoint::AfterActionBeforePhase(ReceiptCommitted),
            FaultPoint::BeforeAction(ReceiptCommitted),
        ] {
            let mut world = make_world("0.4.0");
            publish(&mut world, "0.4.0", &binary("draftd", "0.4.0"));
            let _ = run_with(&world, &request, &CrashAt(point));
            run_with(&world, &UpdateRequest::default(), &NoFaults).unwrap();
            let receipt = receipt::read_final(&world.layout).unwrap();
            if point == FaultPoint::BeforeAction(ReceiptCommitted) {
                assert_eq!(
                    receipt.release_channel,
                    ReleaseChannel::Stable,
                    "nothing committed, nothing kept"
                );
            } else {
                assert_eq!(
                    (receipt.release_channel, receipt.install_generation),
                    (ReleaseChannel::Prerelease, 2)
                );
            }
            assert!(!world.layout.operation().exists());
        }
    }

    #[test]
    fn the_flag_matrix_rejects_meaningless_combinations() {
        let v = Some(semver::Version::parse("1.0.0").unwrap());
        for request in [
            UpdateRequest {
                version: v.clone(),
                channel: Some(ReleaseChannel::Stable),
                ..Default::default()
            },
            UpdateRequest {
                allow_downgrade: true,
                ..Default::default()
            },
            UpdateRequest {
                check: true,
                allow_downgrade: true,
                version: v.clone(),
                ..Default::default()
            },
        ] {
            assert_eq!(
                failure_of(&request.validate().unwrap_err()),
                Some(InstallationFailure::InvalidUpdateFlagCombination)
            );
        }
        UpdateRequest {
            check: true,
            version: v,
            ..Default::default()
        }
        .validate()
        .unwrap();
        UpdateRequest {
            check: true,
            channel: Some(ReleaseChannel::Prerelease),
            ..Default::default()
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn check_reports_and_mutates_nothing_and_a_downgrade_needs_its_flag() {
        let mut world = make_world("0.4.0");
        publish(&mut world, "0.3.4", &binary("draftd", "0.3.4"));
        publish(&mut world, "0.5.0", &binary("draftd", "0.5.0"));
        let receipt_before = std::fs::read(world.layout.receipt()).unwrap();
        match run_with(
            &world,
            &UpdateRequest {
                check: true,
                ..Default::default()
            },
            &NoFaults,
        )
        .unwrap()
        {
            UpdateOutcome::Checked(report) => {
                assert_eq!(report.latest.as_deref(), Some("0.5.0"));
                assert!(report.update_available);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            std::fs::read(world.layout.receipt()).unwrap(),
            receipt_before
        );
        assert!(!world.layout.staging_root().exists());

        let older = UpdateRequest {
            version: Some(semver::Version::parse("0.3.4").unwrap()),
            ..Default::default()
        };
        assert!(run_with(&world, &older, &NoFaults).is_err());
        let allowed = UpdateRequest {
            allow_downgrade: true,
            ..older
        };
        run_with(&world, &allowed, &NoFaults).unwrap();
        let receipt = receipt::read_final(&world.layout).unwrap();
        assert_eq!(
            (receipt.installed_version.as_str(), receipt.release_channel),
            ("0.3.4", ReleaseChannel::Stable)
        );
    }

    #[test]
    fn check_reports_a_trust_floor_and_installs_nothing() {
        let mut world = make_world("0.4.0");
        publish(&mut world, "0.5.0", &binary("draftd", "0.5.0"));
        let stranger = Key::new(9);
        let trust = rel::trust(&[&stranger]);
        let ctx = Context {
            layout: &world.layout,
            host: &world.host,
            source: &world.releases,
            trust: &trust,
            platform_target: TARGET,
            faults: &NoFaults,
        };
        match run(
            &ctx,
            &UpdateRequest {
                check: true,
                ..Default::default()
            },
        )
        .unwrap()
        {
            UpdateOutcome::Checked(report) => assert!(report.below_trust_floor),
            other => panic!("{other:?}"),
        }
        assert!(run(&ctx, &UpdateRequest::default()).is_err());
        assert_eq!(installed_versions(&world).2, "0.4.0");
    }
}
