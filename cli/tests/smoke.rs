use assert_cmd::Command as Assert;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

fn draft(dir: &std::path::Path) -> Assert {
    let mut c = Assert::cargo_bin("draft").unwrap();
    c.current_dir(dir);
    c
}

#[test]
fn init_status_ignore_and_events_work_without_vcs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    draft(dir)
        .args(["init"])
        .assert()
        .success()
        .stdout(contains("Initialized Draft workspace"));
    assert!(dir.join(".draft/config.toml").exists());
    assert!(dir.join(".draft/.ignore").exists());
    assert!(dir.join(".draft/events/events.jsonl").exists());

    std::fs::write(dir.join("app.txt"), "hello\n").unwrap();
    std::fs::create_dir_all(dir.join("notes")).unwrap();
    std::fs::write(dir.join("notes/ignored.txt"), "ignored\n").unwrap();

    draft(dir)
        .args(["status"])
        .assert()
        .success()
        .stdout(contains("app.txt").and(contains("notes/ignored.txt")));
    draft(dir)
        .args(["status"])
        .assert()
        .success()
        .stdout(predicates::str::is_match("\\.draft").unwrap().not());

    draft(dir)
        .args(["ignore", "add", "notes/"])
        .assert()
        .success();
    draft(dir)
        .args(["ignore", "list"])
        .assert()
        .success()
        .stdout(contains("notes/"));
    draft(dir)
        .args(["events", "--verify-chain"])
        .assert()
        .success()
        .stdout(contains("Event chain verified"));
}

#[test]
fn changepack_verify_approve_and_save_native_only() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    std::fs::write(dir.join("new.txt"), "new\n").unwrap();

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
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap();

    draft(dir).args(["verify", pack_id]).assert().success();
    draft(dir)
        .args(["approve", pack_id, "--reason", "reviewed"])
        .assert()
        .success();
    draft(dir)
        .args(["save", pack_id])
        .assert()
        .success()
        .stdout(contains("Changepack saved"));

    let receipts = draft(dir)
        .args(["receipt", "list", "--json"])
        .output()
        .unwrap();
    let receipts: serde_json::Value = serde_json::from_slice(&receipts.stdout).unwrap();
    assert!(receipts
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["status"] == "saved_native_only"));
}

#[test]
fn target_local_is_opaque_and_captures_receipt() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    draft(dir)
        .args([
            "config",
            "set",
            "target.local",
            "printf %s {message} > saved-message.txt",
        ])
        .assert()
        .success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    let out = draft(dir)
        .args([
            "pack",
            "create",
            "--name",
            "opaque-save",
            "--from-working-tree",
            "--json",
        ])
        .output()
        .unwrap();
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap();
    draft(dir).args(["verify", pack_id]).assert().success();
    draft(dir).args(["approve", pack_id]).assert().success();
    draft(dir).args(["save", pack_id]).assert().success();
    assert!(dir.join("saved-message.txt").exists());
    let receipts = draft(dir)
        .args(["receipt", "list", "--json"])
        .output()
        .unwrap();
    let receipts: serde_json::Value = serde_json::from_slice(&receipts.stdout).unwrap();
    assert!(receipts
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["target_local_command_hash"].is_string()));
}

#[test]
fn index_rebuild_creates_real_sqlite_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    draft(dir)
        .args(["index", "rebuild"])
        .assert()
        .success()
        .stdout(contains("Index rebuilt"));

    let db = std::fs::read(dir.join(".draft/indexes/draft.sqlite")).unwrap();
    assert!(db.starts_with(b"SQLite format 3"));
}

#[test]
fn save_aborts_if_pack_candidate_contains_draft_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    draft(dir)
        .args(["config", "set", "target.local", "touch should-not-exist"])
        .assert()
        .success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    let out = draft(dir)
        .args([
            "pack",
            "create",
            "--name",
            "bad-candidate",
            "--from-working-tree",
            "--json",
        ])
        .output()
        .unwrap();
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap();
    let patch_path = dir
        .join(".draft/changepacks")
        .join(pack_id)
        .join("patch.json");
    let mut patch: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&patch_path).unwrap()).unwrap();
    patch["files"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": ".draft/leak",
            "old_path": null,
            "change_kind": "added",
            "hunks": [],
            "binary": false,
            "old_hash": null,
            "new_hash": "sha256:abc"
        }));
    std::fs::write(&patch_path, serde_json::to_string_pretty(&patch).unwrap()).unwrap();

    draft(dir).args(["verify", pack_id]).assert().success();
    draft(dir).args(["approve", pack_id]).assert().success();
    draft(dir)
        .args(["save", pack_id])
        .assert()
        .failure()
        .stderr(contains("SAVE_FAILED").and(contains(".draft/ is included")));
    assert!(!dir.join("should-not-exist").exists());
    draft(dir)
        .args(["events"])
        .assert()
        .success()
        .stdout(contains("SaveFailed"));
}

#[test]
fn remote_target_is_reserved() {
    let tmp = tempfile::tempdir().unwrap();
    draft(tmp.path()).args(["init"]).assert().success();
    draft(tmp.path())
        .args(["config", "set", "target.remote", "anything"])
        .assert()
        .failure()
        .stderr(contains("Remote targets are planned for Draft v0.4.0"));
}

#[test]
fn remote_push_commands_are_not_present_in_v03() {
    let tmp = tempfile::tempdir().unwrap();
    draft(tmp.path())
        .args(["push"])
        .assert()
        .failure()
        .stderr(contains("unrecognized subcommand"));
    draft(tmp.path())
        .args(["sync"])
        .assert()
        .failure()
        .stderr(contains("unrecognized subcommand"));
}

#[test]
fn status_outside_workspace_errors_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    draft(tmp.path())
        .args(["status"])
        .assert()
        .failure()
        .stderr(contains("WORKSPACE_NOT_FOUND"));
}
