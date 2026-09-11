//! Looking at what an adoption would do is not a way of doing it.
//!
//! A preview that quietly persisted a snapshot, or that moved the active
//! context, would make "let me see first" the same act as "go ahead" — and the
//! only person who would find out is the one who did not want the change.

use draft_core::dcg::observation_lifecycle::PendingReason;

mod support;
use support::lifecycle::{app_with, establish_baseline, excluding, project};

#[test]
fn preview_shows_what_would_change_without_changing_it() {
    let project = project("observation-preview");
    let root = project.path();
    establish_baseline(root);
    let app = app_with(excluding("cache"));

    let before = std::fs::read_dir(root.join(".draft/snapshots"))
        .map(|entries| entries.count())
        .unwrap_or(0);

    let preview = app.observation_preview(root).unwrap();
    assert!(
        preview
            .would_leave
            .iter()
            .any(|locator| locator.body == "cache/blob.bin"),
        "the preview must say which resources stop being project state"
    );
    assert!(preview.would_enter.is_empty());
    assert_eq!(preview.reasons, vec![PendingReason::ViewSemanticsChanged]);

    // The load-bearing half: previewing is not a way of adopting. No snapshot
    // was persisted, and the active context did not move.
    let after = std::fs::read_dir(root.join(".draft/snapshots"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(before, after, "a preview must not persist a snapshot");
    assert!(
        app.observation_pending(root).unwrap().is_some(),
        "the candidate is still merely a candidate"
    );
}
