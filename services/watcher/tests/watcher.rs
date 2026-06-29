//! Watcher tests (TEST-004): ignore rules and debounced detection.

use std::path::Path;
use std::time::Duration;

use draft_watcher::{should_ignore, Watcher};

#[test]
fn ignores_provider_internals_and_draft_writeback() {
    assert!(should_ignore(Path::new("repo/.git/index")));
    assert!(should_ignore(Path::new(
        "repo/.draft/operations/0001.operation.json"
    )));
    assert!(should_ignore(Path::new("repo/target/debug/foo")));
    assert!(should_ignore(Path::new("repo/node_modules/x/y.js")));
    assert!(!should_ignore(Path::new("repo/src/main.rs")));
    assert!(!should_ignore(Path::new("repo/.draft/config.toml")));
}

#[test]
fn debounced_detects_a_real_change() {
    let dir = tempfile::tempdir().unwrap();
    let watcher = Watcher::start(dir.path()).unwrap();
    // Give the watcher a moment to register.
    std::thread::sleep(Duration::from_millis(100));
    std::fs::write(dir.path().join("hello.txt"), "hi").unwrap();

    let changed = watcher.poll_debounced(Duration::from_millis(200), Duration::from_secs(3));
    assert!(
        changed
            .iter()
            .any(|p| p.to_string_lossy().contains("hello.txt")),
        "expected to observe hello.txt, got {changed:?}"
    );
}
