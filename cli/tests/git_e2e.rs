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
        .args(["task", "spawn", "update app text", "--", "update app text"])
        .assert()
        .success();
    let out = draft(dir)
        .args(["create", "update-app", "--json"])
        .output()
        .unwrap();
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap();

    assert!(pack_id.starts_with("pck_"));
    assert!(snapshot_id.starts_with("chk_"));

    draft(dir).args(["risk", "-p", pack_id]).assert().success();
    draft(dir)
        .args(["review", "-p", pack_id, "--comment", "looks fine"])
        .assert()
        .success();
    draft(dir)
        .args(["verify", "-p", pack_id])
        .assert()
        .success();
    draft(dir)
        .args(["approve", "-p", pack_id])
        .assert()
        .success();
    draft(dir).args(["save", "-p", pack_id]).assert().success();
    draft(dir)
        .args(["event"])
        .assert()
        .success()
        .stdout(contains("save.completed"));

    draft(dir)
        .args(["rollback", snapshot_id])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(dir.join("app.txt")).unwrap(),
        "hello\n"
    );
    assert!(!dir.join("notes.txt").exists());
}
