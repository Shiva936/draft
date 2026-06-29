//! End-to-end Git workflow through the real `draft` binary (TDD §14.5,
//! Blueprint §26). Exercises the headline scenario and asserts the provider
//! object, receipt, and operation log are all produced correctly.

use std::path::Path;
use std::process::Command;

use assert_cmd::Command as Assert;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn draft(dir: &Path) -> Assert {
    let mut c = Assert::cargo_bin("draft").unwrap();
    c.current_dir(dir);
    c
}

#[test]
fn git_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // 1. Create a temp Git repo with an initial commit.
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "e2e@example.com"]);
    git(dir, &["config", "user.name", "E2E"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "initial"]);
    let head_before = git(dir, &["rev-parse", "HEAD"]);

    // 2. Detect provider.
    draft(dir)
        .args(["workspace", "detect"])
        .assert()
        .success()
        .stdout(predicates_str("git"));

    // 3. Init workspace.
    draft(dir).args(["workspace", "init"]).assert().success();
    assert!(dir.join(".draft/config.toml").exists());

    // 4. Make a change.
    std::fs::write(dir.join("a.txt"), "hello\nworld\n").unwrap();

    // 5. Status shows the change.
    draft(dir)
        .args(["status"])
        .assert()
        .success()
        .stdout(predicates_str("file(s)"));

    // 6. Review + approve.
    draft(dir).args(["review", "--yes"]).assert().success();

    // 7. Verify (explicit cross-platform command).
    draft(dir)
        .args(["verify", "git --version"])
        .assert()
        .success();

    // 8. Commit (finalize).
    draft(dir)
        .args(["commit", "-m", "draft: add world"])
        .assert()
        .success()
        .stdout(predicates_str("Finalized"));

    // 9. A new Git commit exists.
    let head_after = git(dir, &["rev-parse", "HEAD"]);
    assert_ne!(head_before, head_after);
    let log = git(dir, &["log", "-1", "--pretty=%s"]);
    assert!(log.contains("add world"), "commit subject was: {log}");

    // 10. The commit does NOT include .draft/ (it must be excluded).
    let files = git(dir, &["show", "--name-only", "--pretty=format:", "HEAD"]);
    assert!(!files.contains(".draft"), "commit leaked .draft: {files}");
    assert!(files.contains("a.txt"));

    // 11. A receipt exists and maps the change to the Git commit SHA.
    let receipts_json = draft(dir)
        .args(["receipt", "list", "--json"])
        .output()
        .unwrap();
    let receipts: serde_json::Value = serde_json::from_slice(&receipts_json.stdout).unwrap();
    let receipt_id = receipts[0]["id"].as_str().unwrap().to_string();

    let show = draft(dir)
        .args(["receipt", "show", &receipt_id, "--json"])
        .output()
        .unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    let mapped = receipt["provider_objects"][0]["object_id"]
        .as_str()
        .unwrap();
    assert_eq!(
        mapped, head_after,
        "receipt should map to the new commit SHA"
    );

    // 12. The operation log contains FinalizationCompleted.
    let index = std::fs::read_to_string(dir.join(".draft/operations/index.json")).unwrap();
    assert!(
        index.contains("FinalizationCompleted"),
        "operation log missing FinalizationCompleted: {index}"
    );

    // 13. Undo reverses the finalization (history back to before).
    draft(dir).args(["undo"]).assert().success();
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), head_before);
}

/// Tiny helper to avoid pulling in the `predicates` crate explicitly.
fn predicates_str(needle: &'static str) -> predicates::str::ContainsPredicate {
    predicates::str::contains(needle)
}

/// With `require_review = true`, commit must block until the change is approved,
/// then succeed — proving review decisions survive the rescan at finalize time
/// (i.e. change IDs are stable across scans).
#[test]
fn require_review_gate_blocks_then_allows() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "g@example.com"]);
    git(dir, &["config", "user.name", "G"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("a.txt"), "v1\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);

    draft(dir).args(["workspace", "init"]).assert().success();

    // Enable the review gate.
    let cfg_path = dir.join(".draft/config.toml");
    let cfg = std::fs::read_to_string(&cfg_path).unwrap();
    std::fs::write(
        &cfg_path,
        cfg.replace("require_review = false", "require_review = true"),
    )
    .unwrap();

    std::fs::write(dir.join("a.txt"), "v1\nv2\n").unwrap();

    // Commit before review must be blocked (REVIEW_REQUIRED → exit code 2).
    draft(dir)
        .args(["commit", "-m", "needs review"])
        .assert()
        .failure()
        .stderr(predicates_str("REVIEW_REQUIRED"));

    // Approve, then commit must succeed.
    draft(dir).args(["review", "--yes"]).assert().success();
    draft(dir)
        .args(["commit", "-m", "needs review"])
        .assert()
        .success()
        .stdout(predicates_str("Finalized"));
}
