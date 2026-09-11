//! Several callers adopting at once produce one transition, not several.
//!
//! Adoption runs under the project lease. Without that, two racing adoptions
//! could each write a baseline and each record a transition, leaving a project
//! with two answers to "what am I observed under" and no way to choose.

use draft_core::app::App;

mod support;
use support::lifecycle::{app_with, establish_baseline, excluding, project};

#[test]
fn concurrent_adoption_is_serialized_into_one_transition() {
    let project = project("context-transition-concurrency");
    let root = project.path();
    establish_baseline(root);

    let contributions = excluding("cache");
    let outcomes: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let contributions = contributions.clone();
                let root = root.to_path_buf();
                scope.spawn(move || app_with(contributions).observation_adopt(&root))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });

    // Exactly one adoption happened. The rest either lost the lease or found
    // nothing left to adopt — both are correct refusals, and neither leaves a
    // second baseline behind.
    let succeeded = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
    assert!(
        succeeded >= 1,
        "at least one caller must have adopted: {outcomes:?}"
    );
    let history = App::new().observation_transitions(root).unwrap();
    assert_eq!(
        history.len(),
        succeeded,
        "every success must correspond to exactly one recorded transition"
    );
    assert_eq!(
        history.len(),
        1,
        "concurrent adoption of one candidate must produce one transition"
    );
}
