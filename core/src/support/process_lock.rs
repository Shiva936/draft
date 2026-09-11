//! `ProcessFileLock` — the correctness-grade lock.
//!
//! # Why the lock Draft used to have was not this
//!
//! Draft previously serialized its critical sections with `FileGuard`: a
//! create-new lock file with a 30-second wall-clock stale takeover. That
//! takeover was a lost-update race, not a nicety:
//!
//! ```text
//! A acquire -> read generation N -> compare (matches) -> stalls > 30 s
//! B acquire -> sees mtime elapsed -> removes the lock file, takes over
//! B          -> read N -> write N+1 = B -> release
//! A wakes    -> writes N+1 = A            <- B's committed update is LOST
//! ```
//!
//! A *live but slow* holder had its lock stolen. Nothing about being slow makes
//! a process's in-flight compare-exchange invalid, so any authoritative
//! read/compare/write protected that way could silently lose a commit.
//!
//! `ProcessFileLock` fixes that by making the kernel the owner:
//!
//! | | The old advisory lock | `ProcessFileLock` |
//! |---|---|---|
//! | Owner | a file that exists | the open file descriptor / handle |
//! | Stealable | yes, after 30 s | **never** — elapsed time is not a signal |
//! | Released on crash | by the next stale takeover | immediately, by the kernel |
//! | Inherited by children | yes | **no** |
//!
//! The old type is gone rather than deprecated. An unused primitive with this
//! failure mode is an invitation, and removing it makes the guarantee absolute
//! instead of policed.
//!
//! # Two rules that make it usable
//!
//! **Lock a stable sidecar, never the record.** Authoritative records are
//! replaced by atomic rename, which creates a *new inode*. A lock held on the
//! old inode protects nothing once the rename lands, so every lock here targets
//! a stable `<record>.lock` path that no compare-exchange ever replaces.
//!
//! **It is not reentrant.** `flock` is per-file-description, so a second
//! `acquire_exclusive` on the same path from the same thread deadlocks against
//! itself until the timeout. This is why the Store API exposes
//! `with_locked_record(|guard| …)` and a `*_locked` mutation that takes the
//! guard, rather than a `compare_exchange` that would reacquire.
//!
//! # Non-inheritance
//!
//! Draft spawns extensions and helper processes. If a child inherited a
//! correctness-lock descriptor, the lock would outlive its owner: the parent
//! could die, and the lock would stay held for as long as the child lived, with
//! nothing able to take it and nothing able to release it. Descriptors are
//! opened close-on-exec on Unix and non-inheritable on Windows.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::lock_order::{self, LockOrder, LockOrderGuard};

/// The first wait between acquisition attempts.
const INITIAL_POLL_INTERVAL: Duration = Duration::from_millis(2);
/// The longest wait between acquisition attempts.
///
/// Bounded so a waiter that backed off still notices a release promptly.
const MAX_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// An exclusive, kernel-owned advisory lock on an open file descriptor.
///
/// Released when dropped, and — just as importantly — when the owning process
/// dies for any reason, including a kill that runs no destructor.
///
/// # Acquisition is not FIFO-fair
///
/// `flock` offers no queue, so waiting means retrying. A fixed retry interval
/// makes that actively worse: every waiter wakes on the same cadence, they fall
/// into lockstep, and one can lose every race for the whole timeout while the
/// others make progress. The backoff below is randomised for exactly that
/// reason — it scatters the retries so no waiter is systematically last.
///
/// This still guarantees exclusion, not fairness. Draft's contention is a
/// handful of processes rather than a thundering herd, so a bounded wait with
/// a clear timeout is the right trade; a caller that needs ordering should take
/// a product lease, which is what leases are for.
#[derive(Debug)]
pub struct ProcessFileLock {
    path: PathBuf,
    // Held for its side effect: the lock belongs to this open descriptor, so
    // closing it is what releases the lock.
    file: File,
    // Present when the caller declared where this lock sits in the partial
    // order. Dropping it releases the position, so the held set follows the
    // lock's real lifetime rather than a caller remembering to say so.
    #[allow(dead_code)]
    order: Option<LockOrderGuard>,
}

impl ProcessFileLock {
    /// Acquire the exclusive lock at `path`, waiting up to `timeout`.
    ///
    /// `path` must be a stable sidecar that no compare-exchange replaces.
    ///
    /// A timeout means someone else genuinely holds the lock right now — it is
    /// never a reason to take the lock anyway.
    pub fn acquire_exclusive(path: &Path, timeout: Duration) -> DraftResult<Self> {
        Self::acquire(path, timeout, None)
    }

    /// Acquire the lock, declaring where it sits in the frozen partial order.
    ///
    /// The order is checked *before* the lock is taken, so a reverse
    /// acquisition is reported as the ordering mistake it is rather than
    /// appearing later as an unexplained deadlock between two operations.
    pub fn acquire_exclusive_ordered(
        path: &Path,
        timeout: Duration,
        order: LockOrder,
    ) -> DraftResult<Self> {
        Self::acquire(path, timeout, Some(order))
    }

    fn acquire(path: &Path, timeout: Duration, order: Option<LockOrder>) -> DraftResult<Self> {
        // Checked first: blocking on a lock that would violate the order wastes
        // the whole timeout and then reports contention, hiding the real fault.
        let order = order.map(lock_order::enter).transpose()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                DraftError::storage(format!(
                    "cannot create lock directory {}: {error}",
                    parent.display()
                ))
            })?;
        }

        let file = open_lock_file(path)?;
        let start = Instant::now();
        let mut backoff = INITIAL_POLL_INTERVAL;
        loop {
            match try_lock_exclusive(&file) {
                Ok(true) => {
                    crate::support::telemetry::record_lock_wait(start.elapsed());
                    return Ok(Self {
                        path: path.to_path_buf(),
                        file,
                        order,
                    });
                }
                Ok(false) => {
                    if start.elapsed() >= timeout {
                        return Err(DraftError::new(
                            DraftErrorKind::LockTimeout,
                            format!("timed out after {:?} acquiring {}", timeout, path.display()),
                        )
                        .with_suggestion(
                            "Another Draft operation holds this lock. It is released as soon as \
                             that operation finishes or its process exits.",
                        ));
                    }
                    std::thread::sleep(jitter(backoff));
                    backoff = (backoff * 2).min(MAX_POLL_INTERVAL);
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// The sidecar this lock is held on.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ProcessFileLock {
    fn drop(&mut self) {
        // Closing the descriptor releases the lock, so an explicit unlock is
        // belt-and-braces. Failure is ignored because the close that follows
        // releases it regardless.
        let _ = unlock(&self.file);
    }
}

/// Scatter a backoff across `[interval/2, interval]`.
///
/// Without this every waiter retries on the same cadence and they synchronise,
/// which is how one ends up starved for an entire timeout.
fn jitter(interval: Duration) -> Duration {
    let nanos = interval.as_nanos() as u64;
    let half = nanos / 2;
    // A cheap, dependency-free spread; the quality of the randomness does not
    // matter, only that two waiters do not agree.
    let spread = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.subsec_nanos() as u64)
        .unwrap_or(0);
    Duration::from_nanos(half + spread % half.max(1))
}

#[cfg(unix)]
fn open_lock_file(path: &Path) -> DraftResult<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        // Explicit, though Rust's std already sets it: a child that inherited
        // this descriptor would hold the lock after the owner died.
        .custom_flags(libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| {
            DraftError::storage(format!("cannot open lock {}: {error}", path.display()))
        })
}

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> DraftResult<bool> {
    use std::os::unix::io::AsRawFd;

    // SAFETY: `file` is a live, owned descriptor for the duration of the call,
    // and flock takes no pointers.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        // Held by someone else. Not an error — the caller waits.
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => Ok(false),
        Some(code) if code == libc::EINTR => Ok(false),
        _ => Err(DraftError::storage(format!("flock failed: {error}"))),
    }
}

#[cfg(unix)]
fn unlock(file: &File) -> DraftResult<()> {
    use std::os::unix::io::AsRawFd;

    // SAFETY: as above.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if result == 0 {
        Ok(())
    } else {
        Err(DraftError::storage(format!(
            "flock unlock failed: {}",
            std::io::Error::last_os_error()
        )))
    }
}

#[cfg(windows)]
fn open_lock_file(path: &Path) -> DraftResult<File> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT};
    use windows_sys::Win32::System::Threading::SetHandleInformation;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| {
            DraftError::storage(format!("cannot open lock {}: {error}", path.display()))
        })?;

    // Rust's std already creates non-inheritable handles; making it explicit
    // means the guarantee does not depend on that staying true.
    // SAFETY: the handle is live and owned by `file`.
    let ok =
        unsafe { SetHandleInformation(file.as_raw_handle() as HANDLE, HANDLE_FLAG_INHERIT, 0) };
    if ok == 0 {
        return Err(DraftError::storage(format!(
            "cannot make lock handle non-inheritable: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(file)
}

#[cfg(windows)]
fn try_lock_exclusive(file: &File) -> DraftResult<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_LOCK_VIOLATION, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is live, and `overlapped` outlives the call.
    let ok = unsafe {
        LockFileEx(
            file.as_raw_handle() as HANDLE,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if ok != 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(code) if code == ERROR_LOCK_VIOLATION as i32 => Ok(false),
        _ => Err(DraftError::storage(format!("LockFileEx failed: {error}"))),
    }
}

#[cfg(windows)]
fn unlock(file: &File) -> DraftResult<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    let ok = unsafe {
        UnlockFileEx(
            file.as_raw_handle() as HANDLE,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if ok != 0 {
        Ok(())
    } else {
        Err(DraftError::storage(format!(
            "UnlockFileEx failed: {}",
            std::io::Error::last_os_error()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_path(directory: &tempfile::TempDir) -> PathBuf {
        directory.path().join("record.lock")
    }

    #[test]
    fn an_uncontended_lock_is_acquired_and_released() {
        let directory = tempfile::tempdir().unwrap();
        let path = lock_path(&directory);
        {
            let guard = ProcessFileLock::acquire_exclusive(&path, Duration::from_secs(1)).unwrap();
            assert_eq!(guard.path(), path);
        }
        // Released on drop, so the next acquisition is immediate.
        ProcessFileLock::acquire_exclusive(&path, Duration::from_millis(50)).unwrap();
    }

    #[test]
    fn the_lock_directory_is_created_on_demand() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("publication/journal/pat_a1.lock");
        ProcessFileLock::acquire_exclusive(&nested, Duration::from_secs(1)).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn a_second_holder_in_another_process_is_refused_until_the_first_releases() {
        // Cross-process is the case that matters: two `draft` invocations.
        let directory = tempfile::tempdir().unwrap();
        let path = lock_path(&directory);
        let guard = ProcessFileLock::acquire_exclusive(&path, Duration::from_secs(1)).unwrap();

        let held = std::process::Command::new("flock")
            .arg("--exclusive")
            .arg("--nonblock")
            .arg(&path)
            .arg("--command")
            .arg("true")
            .status();
        match held {
            Ok(status) => assert!(
                !status.success(),
                "another process took a lock we are holding"
            ),
            // No `flock(1)` on this system; the in-process checks still apply.
            Err(_) => return,
        }

        drop(guard);
        let free = std::process::Command::new("flock")
            .arg("--exclusive")
            .arg("--nonblock")
            .arg(&path)
            .arg("--command")
            .arg("true")
            .status()
            .unwrap();
        assert!(free.success(), "the lock was not released on drop");
    }

    #[test]
    fn a_live_holder_is_never_displaced_by_elapsed_time() {
        // The whole reason this type exists. The old advisory lock handed it over
        // after 30 seconds; waiting is the only correct outcome here.
        let directory = tempfile::tempdir().unwrap();
        let path = lock_path(&directory);
        let _held = ProcessFileLock::acquire_exclusive(&path, Duration::from_secs(1)).unwrap();

        // Backdate the sidecar far past any plausible staleness threshold.
        let ancient = std::time::SystemTime::now() - Duration::from_secs(86_400);
        let file = File::options().write(true).open(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(ancient))
            .unwrap();

        let contender = std::process::Command::new("flock")
            .arg("--exclusive")
            .arg("--nonblock")
            .arg(&path)
            .arg("--command")
            .arg("true")
            .status();
        if let Ok(status) = contender {
            assert!(
                !status.success(),
                "an old mtime let a live holder's lock be taken"
            );
        }
    }

    /// Spawn a single process that holds an exclusive lock on `path` and does
    /// nothing else.
    ///
    /// `exec` replaces the shell in place, so the returned child is the *only*
    /// process holding the descriptor. `flock(1)`'s ordinary `flock FILE CMD`
    /// form would not do: it deliberately passes the descriptor to a child, so
    /// killing the wrapper would leave the lock held by a survivor and the test
    /// would be measuring flock(1)'s design rather than this module's.
    fn spawn_lock_holder(path: &Path) -> Option<std::process::Child> {
        let script = format!(
            "exec 9>>'{}'; flock -x 9 || exit 1; exec sleep 30",
            path.display()
        );
        let child = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .spawn()
            .ok()?;
        // Give it a moment to actually take the lock.
        std::thread::sleep(Duration::from_millis(300));
        Some(child)
    }

    #[test]
    fn a_timeout_reports_contention_rather_than_taking_the_lock() {
        let directory = tempfile::tempdir().unwrap();
        let path = lock_path(&directory);
        let Some(mut holder) = spawn_lock_holder(&path) else {
            return;
        };

        let attempt = ProcessFileLock::acquire_exclusive(&path, Duration::from_millis(200));
        let _ = holder.kill();
        let _ = holder.wait();

        let error = attempt.expect_err("a held lock must not be handed over on timeout");
        assert_eq!(error.kind, DraftErrorKind::LockTimeout);
    }

    #[test]
    fn a_child_process_does_not_inherit_the_lock() {
        // Scenario DN. If the descriptor were inheritable, the lock would
        // outlive its owner: nothing could take it and nothing could release
        // it for as long as the child lived.
        let directory = tempfile::tempdir().unwrap();
        let path = lock_path(&directory);

        let guard = ProcessFileLock::acquire_exclusive(&path, Duration::from_secs(1)).unwrap();
        let mut child = match std::process::Command::new("sleep").arg("30").spawn() {
            Ok(child) => child,
            Err(_) => return, // no `sleep`; nothing to prove here
        };

        // Release in the parent while the child is still running.
        drop(guard);

        let reacquired = ProcessFileLock::acquire_exclusive(&path, Duration::from_millis(500));
        let outcome = reacquired.is_ok();
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            outcome,
            "the spawned child inherited the lock and kept it held"
        );
    }

    #[test]
    fn the_lock_is_released_when_the_owning_process_dies() {
        // No destructor runs on SIGKILL, so this is the kernel's guarantee
        // rather than ours — and it is why a crashed Draft leaves behind no
        // lock that a human has to clean up by hand.
        let directory = tempfile::tempdir().unwrap();
        let path = lock_path(&directory);
        let Some(mut holder) = spawn_lock_holder(&path) else {
            return;
        };

        let while_alive = ProcessFileLock::acquire_exclusive(&path, Duration::from_millis(200));
        assert!(
            while_alive.is_err(),
            "the spawned holder did not actually take the lock"
        );

        holder.kill().unwrap();
        holder.wait().unwrap();

        ProcessFileLock::acquire_exclusive(&path, Duration::from_secs(2))
            .expect("the lock must be released when its owning process dies");
    }

    #[test]
    fn locking_a_sidecar_survives_the_record_being_replaced() {
        // Records are replaced by atomic rename, which creates a new inode. A
        // lock on the record itself would protect nothing after the rename;
        // the sidecar is what makes the critical section meaningful.
        let directory = tempfile::tempdir().unwrap();
        let record = directory.path().join("control.json");
        let sidecar = directory.path().join("control.lock");
        std::fs::write(&record, b"generation 1").unwrap();

        let guard = ProcessFileLock::acquire_exclusive(&sidecar, Duration::from_secs(1)).unwrap();

        let replacement = directory.path().join("control.json.tmp");
        std::fs::write(&replacement, b"generation 2").unwrap();
        std::fs::rename(&replacement, &record).unwrap();

        // The lock is still held on the same sidecar inode.
        assert_eq!(guard.path(), sidecar);
        assert!(
            ProcessFileLock::acquire_exclusive(&sidecar, Duration::from_millis(50)).is_err()
                || cfg!(not(unix)),
            "the sidecar lock must still be held after the record was replaced"
        );
    }
}
