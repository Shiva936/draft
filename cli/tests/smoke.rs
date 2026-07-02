use assert_cmd::Command as Assert;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

fn draft(dir: &std::path::Path) -> Assert {
    let mut c = Assert::cargo_bin("draft").unwrap();
    c.current_dir(dir);
    c
}

fn write_saved_message_command() -> &'static str {
    if cfg!(windows) {
        "echo {{message}}> saved-message.txt"
    } else {
        "printf %s \"{{message}}\" > saved-message.txt"
    }
}

fn failing_command() -> &'static str {
    if cfg!(windows) {
        "exit /B 7"
    } else {
        "exit 7"
    }
}

fn hook_var_command() -> &'static str {
    if cfg!(windows) {
        "echo {{ticket}}:%DRAFT_VAR_TICKET%> hook-vars.txt"
    } else {
        "printf %s \"{{ticket}}:$DRAFT_VAR_TICKET\" > hook-vars.txt"
    }
}

fn same_canonical_path(left: &std::path::Path, right: &std::path::Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn create_verified_approved_pack(dir: &std::path::Path, name: &str) -> String {
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    let out = draft(dir)
        .args(["create", name, "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap().to_string();
    draft(dir)
        .args(["verify", "-p", &pack_id])
        .assert()
        .success();
    draft(dir)
        .args(["review", "-p", &pack_id])
        .assert()
        .success();
    draft(dir)
        .args(["approve", "-p", &pack_id])
        .assert()
        .success();
    pack_id
}

fn write_rich_hook_config(dir: &std::path::Path, command: &str, continue_on_error: bool) {
    let content = format!(
        r#"[identity]
username = "Ada"
email = "ada@example.com"

[save]
message_template = "{{{{title}}}}"

[hooks.save]
command = "{}"
enabled = true
phase = "after_success"
shell = "default"
cwd = "workspace"
continue_on_error = {}

[verification]
default_profile = "standard"

[policy]
require_verification = true
require_approval = true
require_human_approval_for_high_risk = true
block_if_tests_fail = true
"#,
        command.replace('\\', "\\\\").replace('"', "\\\""),
        continue_on_error
    );
    std::fs::write(dir.join(".draft/config.toml"), content).unwrap();
}

fn collect_files(path: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    if path.is_file() {
        files.push(path.to_path_buf());
        return;
    }
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_files(&path, files);
        } else {
            files.push(path);
        }
    }
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
        .args(["event", "--verify-chain"])
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
        .args(["create", "update-app", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap();

    assert!(pack_id.starts_with("pck_"));
    draft(dir)
        .args(["verify", "-p", pack_id])
        .assert()
        .success();
    draft(dir)
        .args(["review", "-p", pack_id])
        .assert()
        .success();
    draft(dir)
        .args(["approve", "-p", pack_id, "--reason", "reviewed"])
        .assert()
        .success();
    draft(dir)
        .args(["save", "-p", pack_id])
        .assert()
        .success()
        .stdout(contains("ChangePack saved"));

    let receipts = draft(dir)
        .args(["receipt", "list", "--json"])
        .output()
        .unwrap();
    let receipts: serde_json::Value = serde_json::from_slice(&receipts.stdout).unwrap();
    assert!(receipts
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["overall_status"] == "saved"
            && r["hook_status"] == "not_configured"
            && r["native_save_status"] == "saved"));
}

#[test]
fn top_level_pack_ux_supports_create_list_switch_and_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();

    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    let first = draft(dir)
        .args(["create", "first", "--json"])
        .output()
        .unwrap();
    assert!(first.status.success());
    let first: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let first_id = first["id"].as_str().unwrap();

    std::fs::write(dir.join("app.txt"), "v3\n").unwrap();
    draft(dir)
        .args(["create", "second", "-p", "first"])
        .assert()
        .success();

    draft(dir)
        .args(["list"])
        .assert()
        .success()
        .stdout(contains("first").and(contains("second")));
    draft(dir)
        .args(["pack", "-s", "first"])
        .assert()
        .success()
        .stdout(contains(first_id));
    draft(dir)
        .args(["pack"])
        .assert()
        .success()
        .stdout(contains("first"));

    draft(dir)
        .args(["pack", "-d", "first"])
        .write_stdin("n\n")
        .assert()
        .failure()
        .stderr(contains("ChangePack deletion aborted"));
    draft(dir)
        .args(["pack", "-d", "first"])
        .write_stdin("y\n")
        .assert()
        .success()
        .stdout(contains("ChangePack deleted"));
    draft(dir)
        .args(["list"])
        .assert()
        .success()
        .stdout(contains("second").and(contains("first").not()));
    draft(dir)
        .args(["event"])
        .assert()
        .success()
        .stdout(contains("pack.deleted"));
}

#[test]
fn event_command_supports_log_options_and_old_names_are_not_supported() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    draft(dir).args(["create", "event-pack"]).assert().success();

    draft(dir)
        .args(["event", "--top", "--limit", "1"])
        .assert()
        .success()
        .stdout(contains("repo.initialized"));
    draft(dir)
        .args(["event", "--page", "1", "--limit", "1"])
        .assert()
        .success()
        .stdout(contains("pack.selected"));
    draft(dir)
        .args(["event", "--bottom", "--limit", "1"])
        .assert()
        .success()
        .stdout(contains("pack.selected"));
    draft(dir)
        .args(["event", "-f", "checkpoint"])
        .assert()
        .success()
        .stdout(contains("checkpoint.created").and(contains("pack.created").not()));

    let raw = draft(dir)
        .args(["event", "--raw", "--bottom", "--limit", "1"])
        .output()
        .unwrap();
    assert!(
        raw.status.success(),
        "{}",
        String::from_utf8_lossy(&raw.stderr)
    );
    let raw_stdout = String::from_utf8(raw.stdout).unwrap();
    let raw_event: serde_json::Value = serde_json::from_str(raw_stdout.trim()).unwrap();
    assert_eq!(raw_event["type"], "pack.selected");

    draft(dir).args(["log"]).assert().failure();
    draft(dir).args(["events"]).assert().failure();
    draft(dir).args(["event", "-p", "1"]).assert().failure();
    draft(dir).args(["event", "-l", "1"]).assert().failure();
}

#[test]
fn pack_names_are_unique_and_old_pack_subcommands_are_not_supported() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();

    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    draft(dir).args(["create", "unique"]).assert().success();
    draft(dir)
        .args(["create", "unique"])
        .assert()
        .failure()
        .stderr(contains("already exists"));

    draft(dir)
        .args(["pack", "create", "old-form"])
        .assert()
        .failure();
    draft(dir).args(["pack", "list"]).assert().failure();
}

#[test]
fn raw_hooks_save_is_opaque_and_captures_receipt() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    draft(dir)
        .args(["config", "set", "save.message_template", "{{title}}"])
        .assert()
        .success();
    draft(dir)
        .args(["config", "set", "hooks.save", write_saved_message_command()])
        .assert()
        .success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    let out = draft(dir)
        .args(["create", "opaque-save", "--json"])
        .output()
        .unwrap();
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap();
    draft(dir)
        .args(["verify", "-p", pack_id])
        .assert()
        .success();
    draft(dir)
        .args(["review", "-p", pack_id])
        .assert()
        .success();
    draft(dir)
        .args(["approve", "-p", pack_id])
        .assert()
        .success();
    draft(dir).args(["save", "-p", pack_id]).assert().success();
    assert!(dir.join("saved-message.txt").exists());
    let saved_message = std::fs::read_to_string(dir.join("saved-message.txt")).unwrap();
    assert!(saved_message.contains("opaque-save"));
    let receipts = draft(dir)
        .args(["receipt", "list", "--json"])
        .output()
        .unwrap();
    let receipts: serde_json::Value = serde_json::from_slice(&receipts.stdout).unwrap();
    assert!(receipts.as_array().unwrap().iter().any(|r| {
        let Some(working_dir) = r["hook_results"][0]["working_dir"].as_str() else {
            return false;
        };
        r["hook_results"][0]["command_hash"].is_string()
            && r["hook_results"][0]["exit_code"] == 0
            && same_canonical_path(std::path::Path::new(working_dir), dir)
            && r["hook_results"][0]["stdout_ref"].is_string()
            && r["hook_results"][0]["stderr_ref"].is_string()
            && r["hook_status"] == "succeeded"
            && r["overall_status"] == "saved"
    }));
}

#[test]
fn rich_hooks_save_supports_dynamic_vars_and_env() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    write_rich_hook_config(dir, hook_var_command(), false);
    let pack_id = create_verified_approved_pack(dir, "var-save");

    draft(dir)
        .args(["save", "-p", &pack_id, "--var", "ticket=AUTH-123"])
        .assert()
        .success();

    let rendered = std::fs::read_to_string(dir.join("hook-vars.txt")).unwrap();
    assert!(rendered.contains("AUTH-123:AUTH-123"));
}

#[test]
fn hooks_save_failure_obeys_continue_on_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    write_rich_hook_config(dir, failing_command(), true);
    let pack_id = create_verified_approved_pack(dir, "continue-hook-failure");

    draft(dir).args(["save", "-p", &pack_id]).assert().success();
    let receipts = draft(dir)
        .args(["receipt", "list", "--json"])
        .output()
        .unwrap();
    let receipts: serde_json::Value = serde_json::from_slice(&receipts.stdout).unwrap();
    assert!(receipts.as_array().unwrap().iter().any(|r| {
        r["native_save_status"] == "saved"
            && r["hook_status"] == "failed"
            && r["overall_status"] == "saved_with_hook_failure"
    }));
}

#[test]
fn hooks_save_failure_fails_closed_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    write_rich_hook_config(dir, failing_command(), false);
    let pack_id = create_verified_approved_pack(dir, "fail-closed-hook");

    draft(dir)
        .args(["save", "-p", &pack_id])
        .assert()
        .failure()
        .stderr(contains("SAVE_FAILED"));
    let receipts = draft(dir)
        .args(["receipt", "list", "--json"])
        .output()
        .unwrap();
    let receipts: serde_json::Value = serde_json::from_slice(&receipts.stdout).unwrap();
    assert!(receipts.as_array().unwrap().iter().any(|r| {
        r["native_save_status"] == "saved"
            && r["hook_status"] == "failed"
            && r["overall_status"] == "failed"
    }));
}

#[test]
fn hooks_save_missing_placeholder_fails_before_execution() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    let command = if cfg!(windows) {
        "echo ran> should-not-exist && echo {{missing}}"
    } else {
        "touch should-not-exist && echo {{missing}}"
    };
    write_rich_hook_config(dir, command, false);
    let pack_id = create_verified_approved_pack(dir, "missing-placeholder");

    draft(dir)
        .args(["save", "-p", &pack_id])
        .assert()
        .failure()
        .stderr(contains("SAVE_FAILED"));
    assert!(!dir.join("should-not-exist").exists());
}

#[test]
fn hook_var_tail_validation_rejects_invalid_values() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(dir, "bad-vars");

    draft(dir)
        .args(["save", "-p", &pack_id, "--var", "bad-name=x"])
        .assert()
        .failure()
        .stderr(contains("invalid hook variable name"));
    draft(dir)
        .args(["save", "-p", &pack_id, "--var", "missing_equals"])
        .assert()
        .failure()
        .stderr(contains("key=value"));
    draft(dir)
        .args(["save", "-p", &pack_id, "--var", "message=nope"])
        .assert()
        .failure()
        .stderr(contains("overrides a built-in"));
    draft(dir)
        .args(["save", "-p", &pack_id, "--var", "--json"])
        .assert()
        .failure()
        .stderr(contains("normal Draft flags"));
}

#[test]
fn storage_doctor_checks_rebuildable_state() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    let out = draft(dir)
        .args(["checkpoint", "base", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let checkpoint: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let snapshot_id = checkpoint["snapshot_id"].as_str().unwrap();
    let snapshot_path = dir
        .join(".draft/snapshots")
        .join(format!("{snapshot_id}.json"));
    let snapshot: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(snapshot_path).unwrap()).unwrap();
    assert!(snapshot["content_object_refs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|value| value.as_str().unwrap().starts_with("b3:")));

    assert!(dir.join(".draft/objects/blake3").exists());
    draft(dir)
        .args(["storage", "compact"])
        .assert()
        .success()
        .stdout(contains("Storage compact complete"));
    assert!(dir.join(".draft/objects/packs/index.json").exists());
    draft(dir)
        .args(["storage", "doctor"])
        .assert()
        .success()
        .stdout(contains("Storage doctor complete").and(contains("\"objects_ok\": true")));
}

#[test]
fn save_aborts_if_pack_candidate_contains_draft_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    draft(dir)
        .args(["config", "set", "hooks.save", "touch should-not-exist"])
        .assert()
        .success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    let out = draft(dir)
        .args(["create", "bad-candidate", "--json"])
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
            "new_hash": "b3:abc"
        }));
    std::fs::write(&patch_path, serde_json::to_string_pretty(&patch).unwrap()).unwrap();

    draft(dir)
        .args(["verify", "-p", pack_id])
        .assert()
        .success();
    draft(dir)
        .args(["review", "-p", pack_id])
        .assert()
        .success();
    draft(dir)
        .args(["approve", "-p", pack_id])
        .assert()
        .success();
    draft(dir)
        .args(["save", "-p", pack_id])
        .assert()
        .failure()
        .stderr(contains("SAVE_FAILED").and(contains(".draft/ is included")));
    assert!(!dir.join("should-not-exist").exists());
    draft(dir)
        .args(["event"])
        .assert()
        .success()
        .stdout(contains("save.completed"));
}

#[test]
fn docs_do_not_use_retired_external_action_terms() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let mut files = Vec::new();
    for rel in [
        "docs",
        "examples",
        "README.md",
        "RELEASE_NOTES.md",
        "SECURITY.md",
        "CONTRIBUTING.md",
    ] {
        let path = root.join(rel);
        if path.exists() {
            collect_files(&path, &mut files);
        }
    }

    let retired_terms = [
        "target.local",
        "target.remote",
        "remote target",
        "remote targets",
        "provider",
        "providers",
        "landing",
        "commit-native",
        "target_local_command_hash",
        "external command result",
        "[target]",
        "target-local",
        "remote-target",
        "hooks.remote",
        "draft push",
        "draft pr ",
        "branch",
        "branches",
    ];

    let mut violations = Vec::new();
    for file in files {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        let lower = content.to_lowercase();
        for term in retired_terms {
            if lower.contains(term) {
                violations.push(format!("{} contains {term}", file.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "retired external-action terms remain:\n{}",
        violations.join("\n")
    );
}

#[test]
fn old_target_keys_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    draft(tmp.path()).args(["init"]).assert().success();
    draft(tmp.path())
        .args(["config", "set", "target.local", "anything"])
        .assert()
        .failure()
        .stderr(contains("retired external-action config keys"));
    draft(tmp.path())
        .args(["config", "set", "target.remote", "anything"])
        .assert()
        .failure()
        .stderr(contains("retired external-action config keys"));
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
