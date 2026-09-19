//! Promotion nests two record locks and must not reacquire either.
//!
//! §2.34's commit sequence takes the ProjectControlStore lock (order 4) and,
//! while still holding it, the ChangePack's lock (order 8). `ProcessFileLock` is
//! deliberately **not** reentrant — `flock` is per-file-description, so a
//! second acquisition of the same lock by the same thread blocks forever
//! rather than succeeding.
//!
//! That makes non-reentrancy a correctness property with a nasty failure mode:
//! a refactor that reached for a convenience helper which acquires its own lock
//! would not fail a type check or return an error. It would hang, once, on
//! whichever machine happened to run a promotion — and hang identically
//! whether the cause was a deadlock or a slow disk.
//!
//! These tests make that failure fast and legible instead.

mod support;

use draft_core::dcg::{ChangePack, ChangePackLifecycle, ChangePackStore};
use draft_core::project::control::{ProjectControlStore, ProjectLifecycle};
use draft_core::project::security::ProjectSecurityState;
use draft_dcg_contract::ids::{ChangePackId, ProjectId};
use draft_dcg_contract::security::{PolicyDigest, ProjectSecurityStateDigest};
use draft_dcg_contract::{BaselineId, Digest};
use std::sync::mpsc;
use std::time::Duration;

/// Run `body` on a worker and fail if it does not finish promptly.
///
/// A self-deadlock manifests as "never returns", which a plain `#[test]` would
/// surface as the whole suite hanging. Bounding it turns that into a named
/// failure pointing at the sequence that reacquired.
fn must_not_hang<T: Send + 'static>(what: &str, body: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(body());
    });
    receiver
        .recv_timeout(Duration::from_secs(20))
        .unwrap_or_else(|_| panic!("{what} did not complete: a lock was acquired twice"))
}

fn control_state(
    project: &ProjectId,
    baseline: &[u8],
) -> draft_core::project::control::ProjectControlState {
    draft_core::project::control::ProjectControlState {
        generation: 0,
        project: project.clone(),
        accepted_baseline: BaselineId::new(Digest::of_bytes(baseline)),
        current_policy_digest: PolicyDigest::new(Digest::of_bytes(b"policy")),
        project_security_state: ProjectSecurityStateDigest::new(
            ProjectSecurityState::default()
                .digest()
                .unwrap()
                .digest()
                .clone(),
        ),
        project_lifecycle: ProjectLifecycle::Active,
    }
}

#[test]
fn the_promotion_commit_sequence_nests_two_distinct_locks_without_reacquiring() {
    let directory = tempfile::tempdir().unwrap();
    let project = ProjectId::parse("prj_000000000001").unwrap();

    let control = ProjectControlStore::new(directory.path().join("project"));
    control
        .initialize(&control_state(&project, b"baseline-1"))
        .unwrap();

    let changes = ChangePackStore::new(directory.path().join("graph/change-packs"));
    let change_pack_id = ChangePackId::parse("cpk_000000000001").unwrap();
    changes
        .create(&ChangePack {
            generation: 0,
            id: change_pack_id.clone(),
            project: project.clone(),
            current_definition: Digest::of_bytes(b"definition"),
            lifecycle: ChangePackLifecycle::Active,
        })
        .unwrap();

    let path = directory.path().to_path_buf();
    let completed = must_not_hang("the promotion commit sequence", move || {
        let control = ProjectControlStore::new(path.join("project"));
        let changes = ChangePackStore::new(path.join("graph/change-packs"));
        let id = ChangePackId::parse("cpk_000000000001").unwrap();

        // Exactly §2.34: control lock outermost, ChangePack lock nested inside it,
        // and each acquired once.
        control.with_locked_control(|_control_guard| {
            changes.with_locked_record(&id, |change_guard| {
                let current = change_guard.current()?.expect("the ChangePack exists");
                Ok(current.lifecycle)
            })
        })
    })
    .unwrap();

    assert_eq!(completed, ChangePackLifecycle::Active);
}

#[test]
fn reacquiring_the_control_lock_inside_its_own_critical_section_would_hang() {
    // The negative proof. Without it the test above could pass simply because
    // nothing in the sequence ever nests, and the guarantee would be vacuous.
    //
    // This asserts the hazard is real: the same lock taken twice does not
    // succeed, does not error, and does not return.
    let directory = tempfile::tempdir().unwrap();
    let project = ProjectId::parse("prj_000000000001").unwrap();
    let control = ProjectControlStore::new(directory.path().join("project"));
    control
        .initialize(&control_state(&project, b"baseline-1"))
        .unwrap();

    let path = directory.path().to_path_buf();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let control = ProjectControlStore::new(path.join("project"));
        let outcome = control.with_locked_control(|_outer| {
            // A helper that takes the lock itself, called from inside the
            // critical section — the shape a well-meaning refactor produces.
            control.with_locked_control(|_inner| Ok(()))
        });
        let _ = sender.send(outcome.is_ok());
    });

    assert!(
        receiver.recv_timeout(Duration::from_secs(3)).is_err(),
        "reacquisition must not silently succeed; the lock is not reentrant, and a sequence \
         that relies on it being reentrant would hang in production instead"
    );
}
