//! Adoption moves the semantics, the baseline and the audit record together.
//!
//! Three things have to change at once, and the ordering inside the store is
//! what makes a crash between them harmless: durable content first, the active
//! pointer last. There is no interleaving in which the pointer names a baseline
//! that does not exist, and none in which new semantics sit on an old baseline.

use draft_core::app::App;

mod support;
use support::lifecycle::{app_with, establish_baseline, excluding, project};

#[test]
fn adoption_installs_the_new_semantics_a_new_baseline_and_an_audit_record() {
    let project = project("observation-adoption");
    let root = project.path();
    let baseline = establish_baseline(root);
    let app = app_with(excluding("cache"));

    let transition = app.observation_adopt(root).unwrap();
    assert_eq!(transition.from_context_digest, baseline);
    assert_ne!(transition.to_context_digest, baseline);
    assert!(!transition.baseline_snapshot_digest.is_empty());

    // The three things that must move together have all moved.
    assert!(
        app.observation_pending(root).unwrap().is_none(),
        "the candidate became the active context"
    );
    let history = app.observation_transitions(root).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].transition_id, transition.transition_id);

    // The pointer names a baseline that is actually on disk. A pointer into
    // nothing is the failure the write ordering exists to prevent.
    let active = App::new().observation_context(root).unwrap();
    assert_eq!(active.context_digest, transition.to_context_digest);
    assert!(
        root.join(".draft/observation/active-context.json").exists(),
        "the adopted context is persisted, not re-derived"
    );

    // And the universe actually changed: the excluded resource is gone from it.
    let observed = app.status(root).unwrap();
    assert!(
        !observed
            .changes
            .iter()
            .any(|change| change.locator.body.starts_with("cache/")),
        "after adoption the excluded subtree is no longer project state"
    );
}
