use assert_cmd::Command as Assert;
use predicates::str::contains;

fn draft(dir: &std::path::Path) -> Assert {
    let mut c = Assert::cargo_bin("draft").unwrap();
    c.current_dir(dir);
    c
}

#[test]
fn plain_directory_end_to_end_with_rollback() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    draft(dir).args(["init"]).assert().success();
    draft(dir)
        .args(["config", "set", "identity.username", "E2E"])
        .assert()
        .success();
    draft(dir)
        .args(["config", "set", "identity.email", "e2e@example.com"])
        .assert()
        .success();

    std::fs::write(dir.join("app.txt"), "hello\n").unwrap();
    let checkpoint = draft(dir)
        .args(["checkpoint", "before change", "--json"])
        .output()
        .unwrap();
    let checkpoint: serde_json::Value = serde_json::from_slice(&checkpoint.stdout).unwrap();
    let snapshot_id = checkpoint["snapshot_id"].as_str().unwrap();

    std::fs::write(dir.join("app.txt"), "hello world\n").unwrap();
    std::fs::write(dir.join("notes.txt"), "new file\n").unwrap();
    draft(dir)
        .args(["task", "create", "update app text"])
        .assert()
        .success();
    let out = draft(dir)
        .args([
            "pack",
            "create",
            "--name",
            "update-app",
            "--from-working-tree",
            "--json",
        ])
        .output()
        .unwrap();
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap();

    draft(dir).args(["risk", pack_id]).assert().success();
    draft(dir)
        .args(["review", pack_id, "--comment", "looks fine"])
        .assert()
        .success();
    draft(dir).args(["verify", pack_id]).assert().success();
    draft(dir).args(["approve", pack_id]).assert().success();
    draft(dir).args(["save", pack_id]).assert().success();
    draft(dir)
        .args(["events"])
        .assert()
        .success()
        .stdout(contains("SaveCompleted"));

    draft(dir)
        .args(["rollback", snapshot_id, "--plan"])
        .assert()
        .success()
        .stdout(contains("app.txt"));
    draft(dir)
        .args(["rollback", snapshot_id, "--yes"])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(dir.join("app.txt")).unwrap(),
        "hello\n"
    );
    assert!(!dir.join("notes.txt").exists());
}
