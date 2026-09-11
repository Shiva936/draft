//! Lock manager tests (TEST-004): acquisition, mutual exclusion, timeout.

use std::time::Duration;

use draft_locks::{LockManager, LockType};

#[test]
fn acquire_and_release() {
    let dir = tempfile::tempdir().unwrap();
    let lm = LockManager::new(dir.path());
    let sidecar = dir.path().join("locks/promotion.lock");
    {
        let _g = lm
            .acquire(LockType::Promotion, Duration::from_secs(1))
            .unwrap();
        assert!(sidecar.exists(), "the sidecar is created on acquisition");
    }

    // The sidecar deliberately survives release. The lock belongs to the open
    // descriptor, not to the file's existence, and the path has to stay stable:
    // if releasing deleted it, a later acquirer would create a *new inode* and
    // lock that, while anyone still holding the old one believed they had
    // exclusion. Deleting it would reintroduce exactly the race the
    // descriptor-owned lock removes.
    assert!(
        sidecar.exists(),
        "the lock sidecar must be stable across acquisitions"
    );

    // What release actually means: the lock is grantable again.
    lm.acquire(LockType::Promotion, Duration::from_millis(200))
        .expect("the lock must be free once its guard is dropped");
}

#[test]
fn second_acquire_times_out_while_held() {
    let dir = tempfile::tempdir().unwrap();
    let lm = LockManager::new(dir.path());
    let _g = lm
        .acquire(LockType::Promotion, Duration::from_secs(1))
        .unwrap();
    // A concurrent promotion lock must not be grantable.
    let err = lm
        .acquire(LockType::Promotion, Duration::from_millis(200))
        .unwrap_err();
    assert_eq!(
        err.kind,
        draft_core::support::error::DraftErrorKind::LockTimeout
    );
}

#[test]
fn different_lock_types_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    let lm = LockManager::new(dir.path());
    let _a = lm
        .acquire(LockType::Promotion, Duration::from_secs(1))
        .unwrap();
    // A different lock type is unaffected.
    let _b = lm
        .acquire(LockType::VerificationRun, Duration::from_secs(1))
        .unwrap();
}
