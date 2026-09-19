//! The local Draft *installation* and its lifecycle: `draft update`,
//! `draft uninstall`, and the installer coordinator both installers launch.
//!
//! This manages the program, never a project. Nothing here reads or writes a
//! project `.draft/`, appends Activity, mints a receipt kind or a project
//! authority claim, and nothing here is reachable through IPC or the Console.
//!
//! # Authority root
//!
//! An official installation is `<install_root>/` holding `bin/{draft,draftd}`
//! and the private `<install_root>/.draft-install/`, found from the
//! canonicalized running executable — never from `DRAFT_GLOBAL_HOME`, the
//! working directory, configuration or a project. Its inventory is closed and
//! temporal: `receipt.json` (until an uninstall reaches `ReceiptRemoved`),
//! the permanent `lifecycle.lock`, `operation.json` (only while an operation is
//! active), `bootstrap.recovery` (uninstall only), `terminal-cleanup` (only once
//! the semantic→structural handoff commits), and `staging/` / `rollback/`.
//!
//! `lifecycle.lock` is never removed on any platform: `ProcessFileLock` owns
//! the open file description, so unlinking the path — held or released — would
//! let a second actor lock a different inode at the same name. A successful
//! uninstall therefore leaves the inert skeleton
//! `<install_root>/.draft-install/lifecycle.lock`.
//!
//! # Phases
//!
//! Every lifecycle operation is journalled in `operation.json`, whose `phase`
//! always names the *last durably completed* action. Recovery first runs the
//! liminal next-action probe (did the next action finish before its phase
//! write?) and only then applies the phase's frozen recovery branch.

pub mod archive;
pub mod bootstrap;
pub mod coordinator;
pub mod layout;
pub mod operation;
pub mod path;
pub mod provenance;
pub mod receipt;
pub mod release;
pub mod terminal;
pub mod uninstall;
pub mod update;

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

crate::id_newtype!(
    /// One installation lifetime. Minted before the first durable
    /// `FreshInstall` write, stable across every update and channel-only
    /// commit, and never reused after an uninstall completes.
    InstallationId, "ins_");
crate::id_newtype!(
    /// One lifecycle operation. Minted before its first durable write and
    /// never reused; derives `staging/<id>/`, `rollback/<id>/` and the helper
    /// slot. Not `op_`, which execution operations already own.
    InstallationOperationId, "ilo_");

/// `<prefix>` followed by exactly twelve lowercase hex digits.
fn is_prefixed_hex12(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|body| {
        body.len() == 12
            && body
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

impl InstallationId {
    pub fn is_well_formed(value: &str) -> bool {
        is_prefixed_hex12(value, "ins_")
    }
}

impl InstallationOperationId {
    pub fn is_well_formed(value: &str) -> bool {
        is_prefixed_hex12(value, "ilo_")
    }
}

pub use crate::support::error::InstallationFailure;

/// A typed installation failure as a `DraftError`.
pub fn fail(kind: InstallationFailure, message: impl Into<String>) -> DraftError {
    DraftError::new(DraftErrorKind::Installation(kind), message)
}

/// The installation failure a `DraftError` carries, if it is one.
pub fn failure_of(error: &DraftError) -> Option<InstallationFailure> {
    match error.kind {
        DraftErrorKind::Installation(kind) => Some(kind),
        _ => None,
    }
}

/// The byte identity of an executable: sha256 (64 lowercase hex) and size.
///
/// Recomputed before every replace or delete, and never taken through a
/// symlink: a `PathSymlink` is validated by its target, not its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub sha256: String,
    pub size: u64,
}

impl Identity {
    /// The identity of the regular file at `path`, which must not be a
    /// symlink.
    pub fn of_file(path: &Path) -> DraftResult<Self> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| DraftError::storage(format!("read {}: {error}", path.display())))?;
        if !metadata.is_file() {
            return Err(fail(
                InstallationFailure::InstallationEntryIdentityMismatch,
                format!("{} is not a regular file", path.display()),
            ));
        }
        let mut file = std::fs::File::open(path)
            .map_err(|error| DraftError::storage(format!("open {}: {error}", path.display())))?;
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        let mut buffer = vec![0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            let read = file.read(&mut buffer).map_err(|error| {
                DraftError::storage(format!("read {}: {error}", path.display()))
            })?;
            if read == 0 {
                break;
            }
            size += read as u64;
            sha2::Digest::update(&mut hasher, &buffer[..read]);
        }
        Ok(Self {
            sha256: crate::support::hashing::hex_encode(&sha2::Digest::finalize(hasher)),
            size,
        })
    }

    /// Whether `path` currently holds exactly this identity. Absent is `false`.
    pub fn matches(&self, path: &Path) -> bool {
        Self::of_file(path).is_ok_and(|actual| &actual == self)
    }

    pub fn is_well_formed(&self) -> bool {
        self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}

/// The platform family the lifecycle rules branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallPlatform {
    Unix,
    Windows,
}

impl InstallPlatform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }

    pub fn exe_suffix(self) -> &'static str {
        match self {
            Self::Unix => "",
            Self::Windows => ".exe",
        }
    }
}

/// The release targets Draft publishes. Anything else is
/// `UnsupportedPlatform`, refused before any download.
pub const SUPPORTED_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

/// The release target this build installs from.
pub fn current_target() -> DraftResult<&'static str> {
    let target = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-musl"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-musl"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else {
        return Err(fail(
            InstallationFailure::UnsupportedPlatform,
            "this platform has no official Draft release",
        ));
    };
    Ok(target)
}

/// How long a spawned `--version` may take before it counts as a failure.
pub const BINARY_VALIDATION_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a lifecycle actor waits for `lifecycle.lock` before reporting busy.
pub const LIFECYCLE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);

/// Everything the lifecycle engines need from the machine they run on, so the
/// state machines stay testable against a fake daemon and fake binaries.
pub trait LifecycleHost {
    /// Whether a `draftd` is answering.
    fn daemon_running(&self) -> bool;
    /// Stop the daemon through `service.shutdown`, waiting bounded.
    fn stop_daemon(&self) -> DraftResult<()>;
    /// Start the daemon at this exact canonical path and wait bounded until it
    /// is running *and* answers `service.status` (never PID existence alone).
    fn start_daemon(&self, draftd: &Path) -> DraftResult<()>;
    /// `daemon_running()` and a successful `service.status`.
    fn daemon_healthy(&self) -> bool;
    /// Execute `<exe> --version` by canonical path with a bounded timeout and
    /// return the reported version string.
    fn binary_version(&self, exe: &Path) -> DraftResult<String>;
    /// Launch the staged lifecycle helper in normal parent-exit mode: it
    /// receives only the installation and operation ids plus this process's
    /// pid, waits for this process to exit, then takes the lock itself.
    fn launch_helper(
        &self,
        helper: &Path,
        installation: &InstallationId,
        operation: &InstallationOperationId,
    ) -> DraftResult<()>;
    /// Wait, bounded, for process `pid` to exit (synchronization only — a pid
    /// is never authority).
    fn wait_for_exit(&self, pid: u32, timeout: Duration) -> DraftResult<()>;
}

/// Parse the version out of `draft --version` / `draftd --version` output,
/// which prints `<name> <version>`.
pub fn parse_reported_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .nth(1)
        .map(|version| version.trim_start_matches('v').to_string())
}

/// Where a lifecycle engine may be interrupted, so tests can inject a crash
/// before an action, after the action but before its phase write, and after
/// the phase write — and prove each recovers deterministically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPoint {
    BeforeAction(operation::Phase),
    AfterActionBeforePhase(operation::Phase),
    AfterPhase(operation::Phase),
    /// After `operation.json` is deleted, before the structural tail.
    AfterJournalDeleted,
}

/// Crash injection. Production uses [`NoFaults`].
pub trait Faults {
    /// Return `Err` to simulate the process dying at `point`.
    fn at(&self, point: FaultPoint) -> DraftResult<()>;
}

/// The message an injected crash carries. A crash is the process dying: the
/// engines never run cleanup or rollback for it, exactly as a real crash would
/// not, and recovery must converge from whatever was left.
pub const INJECTED_CRASH: &str = "injected lifecycle crash";

pub fn is_injected_crash(error: &DraftError) -> bool {
    error.kind == DraftErrorKind::Internal && error.message == INJECTED_CRASH
}

/// The production fault policy: never interrupts.
pub struct NoFaults;

impl Faults for NoFaults {
    fn at(&self, _: FaultPoint) -> DraftResult<()> {
        Ok(())
    }
}

/// Wait, bounded, for process `pid` to exit. Synchronization only.
pub fn wait_for_process_exit(pid: u32, timeout: Duration) -> DraftResult<()> {
    let deadline = std::time::Instant::now() + timeout;
    while process_alive(pid) {
        if std::time::Instant::now() >= deadline {
            return Err(fail(
                InstallationFailure::InstallationBusy,
                format!("process {pid} did not exit within {timeout:?}"),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // Signal 0 checks existence without delivering anything.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let alive = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
        CloseHandle(handle);
        alive
    }
}

#[cfg(not(any(unix, windows)))]
fn process_alive(_: u32) -> bool {
    false
}

/// Write a structured lifecycle file with the repository's atomic discipline
/// (temp → sync → rename → directory sync) and restrict it to its owner.
pub(crate) fn write_private_json<T: Serialize>(path: &Path, value: &T) -> DraftResult<()> {
    crate::support::fsutil::write_json(path, value)?;
    let _ = crate::support::hidden::restrict_file(path, 0o600);
    Ok(())
}

/// Write a bounded line record atomically and restrict it to its owner.
pub(crate) fn write_private_bytes(path: &Path, bytes: &[u8]) -> DraftResult<()> {
    crate::support::fsutil::write_atomic(path, bytes)?;
    let _ = crate::support::hidden::restrict_file(path, 0o600);
    Ok(())
}

/// Remove a file if present. Absent is success; anything else is reported.
pub(crate) fn remove_file_if_present(path: &Path) -> DraftResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DraftError::storage(format!(
            "remove {}: {error}",
            path.display()
        ))),
    }
}

/// Remove a directory only when it is empty. Absent or non-empty is left alone.
pub(crate) fn remove_dir_if_empty(path: &Path) {
    let _ = std::fs::remove_dir(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_grammars_are_exact() {
        assert!(InstallationId::is_well_formed("ins_0123456789ab"));
        assert!(InstallationOperationId::is_well_formed("ilo_0123456789ab"));
        for bad in [
            "ins_0123456789a",
            "ins_0123456789abc",
            "ins_0123456789AB",
            "ilo_0123456789ab",
            "ins-0123456789ab",
            "",
        ] {
            assert!(!InstallationId::is_well_formed(bad), "{bad}");
        }
        assert!(!InstallationOperationId::is_well_formed("op_0123456789ab"));
        let minted = InstallationId::generate();
        assert!(InstallationId::is_well_formed(minted.as_str()));
        let op = InstallationOperationId::generate();
        assert!(InstallationOperationId::is_well_formed(op.as_str()));
        assert_ne!(InstallationOperationId::generate(), op);
    }

    #[test]
    fn every_failure_has_a_distinct_code() {
        let codes: std::collections::BTreeSet<_> = [
            InstallationFailure::InstallationBusy,
            InstallationFailure::RecoveryFailed,
            InstallationFailure::UninstallRecoveryFailed,
            InstallationFailure::WindowsPathConcurrentMutation,
            InstallationFailure::WindowsPathStateInvalid,
        ]
        .iter()
        .map(|kind| kind.code())
        .collect();
        assert_eq!(codes.len(), 5);
        let error = fail(InstallationFailure::InstallationBusy, "busy");
        assert_eq!(error.code(), "INSTALLATION_BUSY");
        assert_eq!(
            failure_of(&error),
            Some(InstallationFailure::InstallationBusy)
        );
    }

    #[test]
    fn identity_is_sha256_and_size_and_refuses_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("draft");
        std::fs::write(&file, b"abc").unwrap();
        let identity = Identity::of_file(&file).unwrap();
        assert_eq!(
            identity.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(identity.size, 3);
        assert!(identity.is_well_formed());
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert!(Identity::of_file(&link).is_err());
            assert!(!identity.matches(&link));
        }
    }
}
