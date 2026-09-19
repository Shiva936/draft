//! A revision that observes identically is not an observation change.
//!
//! This is the routine upgrade, and it is the case that has to stay cheap. The
//! context digest carries effective semantics and no producer reference, so a
//! publisher shipping a no-op revision changes provenance and nothing else: no
//! pending context, no preview, no new baseline, no superseded work. A prompt
//! that fires when nothing changed is a prompt people learn to dismiss, and the
//! one that matters arrives looking the same.

use draft_core::app::App;

mod support;
use support::lifecycle::{app_with, establish_baseline, excluding, project};

#[test]
fn a_package_update_with_identical_semantics_proposes_nothing() {
    let project = project("same-semantics-no-rebaseline");
    let root = project.path();
    establish_baseline(root);

    // Two separate "installations" whose observation semantics are identical.
    let first = app_with(excluding("cache"));
    first.observation_adopt(root).unwrap();
    let adopted = App::new().observation_context(root).unwrap().context_digest;
    let snapshots = || {
        std::fs::read_dir(root.join(".draft/snapshots"))
            .map(|entries| entries.count())
            .unwrap_or(0)
    };
    let baselines = snapshots();

    let second = app_with(excluding("cache"));
    assert!(
        second.observation_pending(root).unwrap().is_none(),
        "identical observation semantics are not a change"
    );
    assert_eq!(
        App::new().observation_context(root).unwrap().context_digest,
        adopted
    );
    assert_eq!(
        second.observation_transitions(root).unwrap().len(),
        1,
        "no second transition was recorded"
    );
    assert_eq!(
        snapshots(),
        baselines,
        "no rebaseline: the semantics did not move, so the baseline must not either"
    );
}
