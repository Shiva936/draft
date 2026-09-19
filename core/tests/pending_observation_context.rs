//! Installing something that would change the observed universe proposes; it
//! does not act.
//!
//! The active observation context is persisted, not re-derived from whatever
//! happens to be installed at the moment of observation. That distinction is
//! the whole point: without it, adding a package would silently rewrite what a
//! project's history claims it saw.

use draft_core::app::App;
use draft_core::dcg::observation_lifecycle::PendingReason;

mod support;
use support::lifecycle::{app_with, establish_baseline, excluding, project};

#[test]
fn installing_a_view_rule_proposes_a_change_and_alters_nothing() {
    let project = project("pending-observation-context");
    let root = project.path();
    let baseline = establish_baseline(root);

    // Installing the package changes what *would* be observed. It must not
    // change what *is* observed, or a project's history would silently acquire
    // a removal nobody made.
    let app = app_with(excluding("cache"));
    let pending = app
        .observation_pending(root)
        .unwrap()
        .expect("a view-rule change is a pending observation change");
    assert_eq!(pending.reasons, vec![PendingReason::ViewSemanticsChanged]);
    assert_eq!(pending.active_context_digest, baseline);
    assert_ne!(pending.candidate.context_digest, baseline);

    // The active context is untouched, and so is the observed universe.
    let status = app.status(root).unwrap();
    let observed: Vec<&str> = status
        .changes
        .iter()
        .map(|change| change.locator.body.as_str())
        .collect();
    assert!(
        observed.is_empty() || !observed.contains(&"cache/blob.bin"),
        "nothing about the observed universe may move before adoption"
    );
    assert_eq!(
        App::new().observation_context(root).unwrap().context_digest,
        baseline,
        "the adopted semantics are still the ones in force"
    );
}

#[test]
fn a_candidate_that_stops_differing_is_withdrawn() {
    let project = project("pending-observation-context");
    let root = project.path();
    establish_baseline(root);

    let with_rules = app_with(excluding("cache"));
    assert!(with_rules.observation_pending(root).unwrap().is_some());

    // The extension is disabled again. There is nothing left to decide, and a
    // banner that outlives its cause teaches people to ignore banners.
    let plain = App::new();
    assert!(plain.observation_pending(root).unwrap().is_none());
    assert!(!root
        .join(".draft/observation/pending-context.json")
        .exists());
}
