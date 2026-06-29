//! Lock manager tests (TEST-004): acquisition, mutual exclusion, timeout.

use std::time::Duration;

use draft_locks::{LockManager, LockType};

#[test]
fn acquire_and_release() {
    let dir = tempfile::tempdir().unwrap();
    let lm = LockManager::new(dir.path());
    {
        let _g = lm
            .acquire(LockType::Finalization, Duration::from_secs(1))
            .unwrap();
        // Lock file should exist while held.
        assert!(dir.path().join("locks/finalization.lock").exists());
    }
    // Released on drop.
    assert!(!dir.path().join("locks/finalization.lock").exists());
}

#[test]
fn second_acquire_times_out_while_held() {
    let dir = tempfile::tempdir().unwrap();
    let lm = LockManager::new(dir.path());
    let _g = lm
        .acquire(LockType::Finalization, Duration::from_secs(1))
        .unwrap();
    // A concurrent finalization lock must not be grantable.
    let err = lm
        .acquire(LockType::Finalization, Duration::from_millis(200))
        .unwrap_err();
    assert_eq!(err.kind, draft_core::error::DraftErrorKind::LockTimeout);
}

#[test]
fn different_lock_types_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    let lm = LockManager::new(dir.path());
    let _a = lm
        .acquire(LockType::Finalization, Duration::from_secs(1))
        .unwrap();
    // A different lock type is unaffected.
    let _b = lm
        .acquire(LockType::VerificationRun, Duration::from_secs(1))
        .unwrap();
}
