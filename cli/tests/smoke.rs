//! CLI smoke tests (TDD §14.4 / TEST-003) and v0.1.0 migration test (TEST-006).

use std::path::Path;
use std::process::Command;

use assert_cmd::Command as Assert;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

#[cfg(unix)]
use draft_ipc::{call, socket_path, Request};

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {:?}", args);
}

fn draft(dir: &Path) -> Assert {
    let mut c = Assert::cargo_bin("draft").unwrap();
    c.current_dir(dir);
    c
}

fn init_git(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "s@example.com"]);
    git(dir, &["config", "user.name", "S"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("f.txt"), "x\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

#[test]
fn provider_list_shows_git_and_experimental() {
    let tmp = tempfile::tempdir().unwrap();
    draft(tmp.path())
        .args(["provider", "list"])
        .assert()
        .success()
        .stdout(contains("git").and(contains("experimental")));
}

#[test]
fn provider_list_json_is_valid() {
    let tmp = tempfile::tempdir().unwrap();
    let out = draft(tmp.path())
        .args(["provider", "list", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v.as_array().unwrap().iter().any(|p| p["id"] == "git"));
}

#[test]
fn workspace_detect_in_git_repo() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    draft(tmp.path())
        .args(["workspace", "detect"])
        .assert()
        .success()
        .stdout(contains("git"));
}

#[test]
fn service_status_reports_embedded() {
    let tmp = tempfile::tempdir().unwrap();
    draft(tmp.path())
        .args(["service", "status"])
        .assert()
        .success()
        .stdout(contains("embedded").or(contains("Running")));
}

#[test]
#[cfg(unix)]
fn service_start_registers_current_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    draft(tmp.path())
        .args(["workspace", "init"])
        .assert()
        .success();

    draft(tmp.path())
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env("HOME", home.path())
        .args(["service", "start"])
        .assert()
        .success();

    std::env::set_var("XDG_RUNTIME_DIR", runtime.path());
    std::env::set_var("HOME", home.path());
    let resp = call(
        &socket_path(),
        &Request::new("test", "service.status", serde_json::Value::Null),
    )
    .unwrap();
    assert!(resp.ok);
    assert_eq!(resp.result.unwrap()["workspaces"], 1);

    draft(tmp.path())
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env("HOME", home.path())
        .args(["service", "stop"])
        .assert()
        .success();
}

#[test]
fn receipt_list_empty_is_ok() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    draft(tmp.path())
        .args(["workspace", "init"])
        .assert()
        .success();
    draft(tmp.path())
        .args(["receipt", "list"])
        .assert()
        .success()
        .stdout(contains("none"));
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

#[test]
fn migration_from_v0_1_0_metadata() {
    // Simulate a v0.1.0 workspace: a Git repo with an old-style .draft/ that has
    // config.toml + a legacy receipt but no workspace.json.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    init_git(dir);
    let draft_dir = dir.join(".draft");
    std::fs::create_dir_all(draft_dir.join("receipts")).unwrap();
    std::fs::write(
        draft_dir.join("config.toml"),
        "version = 1\nrepo_id = \"old\"\n",
    )
    .unwrap();
    std::fs::write(
        draft_dir.join("receipts/abc123.json"),
        r#"{"commit_hash":"abc123def456","commit_message":"old commit"}"#,
    )
    .unwrap();

    // Opening the workspace (via status) should transparently migrate it.
    draft(dir).args(["status"]).assert().success();
    assert!(
        draft_dir.join("workspace.json").exists(),
        "migration created workspace.json"
    );

    // The migration operation and a converted receipt should exist.
    let ops = std::fs::read_to_string(draft_dir.join("operations/index.json")).unwrap();
    assert!(ops.contains("WorkspaceMigrated"));
    draft(dir)
        .args(["receipt", "list"])
        .assert()
        .success()
        .stdout(contains("object(s)"));
}
