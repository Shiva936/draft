use assert_cmd::prelude::*;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn git(args: &[&str], dir: &std::path::Path) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git command failed");
    assert!(status.success(), "git {:?} failed", args);
}

#[test]
fn undo_from_nested_subdirectory_restores_checkpoint() {
    let temp_dir = TempDir::new().unwrap();
    let repo_root = temp_dir.path();

    // Init git repo with identity
    git(&["init"], repo_root);
    git(&["config", "user.name", "Test User"], repo_root);
    git(&["config", "user.email", "test@example.com"], repo_root);

    // Create and commit initial file
    let test_file = repo_root.join("hello.txt");
    fs::write(&test_file, "initial content").unwrap();
    git(&["add", "hello.txt"], repo_root);
    git(&["commit", "-m", "Initial commit"], repo_root);

    // draft start
    Command::cargo_bin("draft")
        .unwrap()
        .arg("start")
        .current_dir(repo_root)
        .assert()
        .success();

    // Modify the tracked file — this is the state draft will checkpoint
    fs::write(&test_file, "content before draft commit").unwrap();

    // draft commit — creates checkpoint then commits
    Command::cargo_bin("draft")
        .unwrap()
        .args(["commit", "-m", "Update hello", "--yes", "--no-verify"])
        .current_dir(repo_root)
        .assert()
        .success();

    // Verify the commit happened
    let committed = fs::read_to_string(&test_file).unwrap();
    assert_eq!(committed, "content before draft commit");

    // Now modify the file again (post-commit working tree state)
    fs::write(&test_file, "post-commit modification").unwrap();

    // Create nested subdir — undo must work from here
    let nested_dir = repo_root.join("subdir");
    fs::create_dir_all(&nested_dir).unwrap();

    // Run draft undo from nested subdir — must detect repo root automatically
    let output = Command::cargo_bin("draft")
        .unwrap()
        .args(["undo", "--yes"])
        .current_dir(&nested_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Undo failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Checkpoint captured "content before draft commit"
    // After undo, file must be restored to that content
    let restored = fs::read_to_string(&test_file).unwrap();
    assert_eq!(
        restored, "content before draft commit",
        "File not restored correctly. Got: {:?}", restored
    );
}
