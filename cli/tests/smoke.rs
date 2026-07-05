use assert_cmd::Command as Assert;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

fn draft(dir: &std::path::Path) -> Assert {
    let mut c = Assert::cargo_bin("draft").unwrap();
    c.current_dir(dir);
    // Hermetic, per-test global `~/.draft/`: nested under the project `.draft/`
    // so it is always excluded from workspace scans and never touches the real
    // user home. Unique per test because `dir` is a unique tempdir.
    c.env("DRAFT_GLOBAL_HOME", dir.join(".draft").join("_global"));
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

fn create_verified_reviewed_pack(dir: &std::path::Path, name: &str) -> String {
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
    draft(dir).args(["init", "--global"]).assert().success();
    draft(dir)
        .args(["doctor"])
        .assert()
        .success()
        .stdout(contains("event-chain"));
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
fn pack_delete_keeps_current_cleanup_semantics_for_pack_owned_task_and_run() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    draft(dir)
        .args(["candidate", "add", "writer", "--", "cargo --version"])
        .assert()
        .success();
    draft(dir)
        .args(["task", "spawn", "write file", "-c", "writer", "--", "write"])
        .assert()
        .success();

    let packs = draft(dir).args(["list", "--json"]).output().unwrap();
    assert!(packs.status.success());
    let packs: serde_json::Value = serde_json::from_slice(&packs.stdout).unwrap();
    let pack = packs
        .as_array()
        .unwrap()
        .iter()
        .find(|pack| pack["name"] == "writer")
        .unwrap();
    let pack_id = pack["id"].as_str().unwrap();
    let task_id = pack["task_id"].as_str().unwrap();
    let run_id = pack["run_id"].as_str().unwrap();

    assert!(dir.join(format!(".draft/tasks/{task_id}.json")).exists());
    assert!(dir.join(format!(".draft/runs/{run_id}.json")).exists());
    draft(dir)
        .args(["pack", "-d", pack_id])
        .write_stdin("y\n")
        .assert()
        .success();
    assert!(!dir.join(format!(".draft/changepacks/{pack_id}")).exists());
    assert!(!dir.join(format!(".draft/tasks/{task_id}.json")).exists());
    assert!(!dir.join(format!(".draft/runs/{run_id}.json")).exists());
}

#[test]
fn init_fails_when_workspace_already_initialized() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    draft(dir)
        .args(["init"])
        .assert()
        .failure()
        .stderr(contains("already initialized"));
}

#[test]
fn event_command_supports_v032_contract_and_rejects_legacy_flags() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    draft(dir).args(["create", "event-pack"]).assert().success();

    draft(dir)
        .args(["event", "--page", "1", "--limit", "1"])
        .assert()
        .success()
        .stdout(contains("pack.selected"));
    draft(dir)
        .args(["event", "--limit", "1"])
        .assert()
        .success()
        .stdout(contains("pack.selected"));

    let raw = draft(dir)
        .args(["event", "--raw", "--limit", "1"])
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
    draft(dir).args(["event", "--top"]).assert().failure();
    draft(dir).args(["event", "--bottom"]).assert().failure();
    draft(dir)
        .args(["event", "-f", "checkpoint"])
        .assert()
        .failure();
    draft(dir)
        .args(["event", "--filter", "checkpoint"])
        .assert()
        .failure();
    draft(dir)
        .args(["event", "--verify-chain"])
        .assert()
        .failure();
}

#[test]
fn public_docs_do_not_advertise_retired_event_commands() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let mut files = Vec::new();
    for rel in ["docs", "examples", "README.md"] {
        let path = root.join(rel);
        if path.exists() {
            collect_files(&path, &mut files);
        }
    }

    let mut violations = Vec::new();
    for file in files {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        for line in content.lines() {
            let lower = line.to_lowercase();
            if lower.contains("draft events") {
                violations.push(format!("{} advertises draft events", file.display()));
            }
            if lower.contains("draft log")
                && !(lower.contains("no `draft log`")
                    || lower.contains("there is no `draft log`")
                    || lower.contains("rejects `draft log`")
                    || lower.contains("rejected legacy surfaces")
                    || lower.contains("unsupported"))
            {
                violations.push(format!("{} advertises draft log", file.display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "plural/log event commands remain:\n{}",
        violations.join("\n")
    );
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
fn risk_flags_security_sensitive_paths_with_explainable_output() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::create_dir_all(dir.join("src/auth")).unwrap();
    std::fs::write(dir.join("src/auth/session.rs"), "pub fn check() {}\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(
        dir.join("src/auth/session.rs"),
        "pub fn check_payment_token() {}\n",
    )
    .unwrap();
    let out = draft(dir)
        .args(["create", "auth-risk", "--json"])
        .output()
        .unwrap();
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap();
    let risk = draft(dir)
        .args([
            "risk",
            "-p",
            pack_id,
            "--explain",
            "--include-evidence",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(risk.status.success());
    let risk: serde_json::Value = serde_json::from_slice(&risk.stdout).unwrap();
    assert!(risk["reason_codes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|code| code == "auth_or_security_surface"));
    assert!(risk["evidence_summary"].is_array());
    assert!(risk["evidence_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|gap| gap == "verification receipt missing"));
}

#[test]
fn risk_options_control_explanation_and_evidence_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    let out = draft(dir)
        .args(["create", "risk-options", "--json"])
        .output()
        .unwrap();
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap();

    let compact = draft(dir)
        .args(["risk", "-p", pack_id, "--json"])
        .output()
        .unwrap();
    let compact: serde_json::Value = serde_json::from_slice(&compact.stdout).unwrap();
    assert!(compact["factors"].as_array().unwrap().is_empty());
    assert!(compact["evidence_summary"].as_array().unwrap().is_empty());

    let expanded = draft(dir)
        .args([
            "risk",
            "-p",
            pack_id,
            "--explain",
            "--include-evidence",
            "--json",
        ])
        .output()
        .unwrap();
    let expanded: serde_json::Value = serde_json::from_slice(&expanded.stdout).unwrap();
    assert!(!expanded["factors"].as_array().unwrap().is_empty());
    assert!(!expanded["evidence_summary"].as_array().unwrap().is_empty());
}

#[test]
fn compose_output_is_not_final_until_verified_and_reviewed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("left.txt"), "base\n").unwrap();
    std::fs::write(dir.join("right.txt"), "base\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    draft(dir)
        .args(["create", "common", "-p", "base"])
        .assert()
        .success();

    std::fs::write(dir.join("left.txt"), "left\n").unwrap();
    let left = draft(dir)
        .args(["create", "left", "-p", "common", "--json"])
        .output()
        .unwrap();
    let left: serde_json::Value = serde_json::from_slice(&left.stdout).unwrap();
    let left_id = left["id"].as_str().unwrap();

    std::fs::write(dir.join("left.txt"), "base\n").unwrap();
    draft(dir).args(["checkpoint", "base2"]).assert().success();
    std::fs::write(dir.join("right.txt"), "right\n").unwrap();
    let right = draft(dir)
        .args(["create", "right", "-p", "common", "--json"])
        .output()
        .unwrap();
    let right: serde_json::Value = serde_json::from_slice(&right.stdout).unwrap();
    let right_id = right["id"].as_str().unwrap();

    let composed = draft(dir)
        .args([
            "compose", left_id, right_id, "--output", "combined", "--json",
        ])
        .output()
        .unwrap();
    assert!(
        composed.status.success(),
        "{}",
        String::from_utf8_lossy(&composed.stderr)
    );
    let composed: serde_json::Value = serde_json::from_slice(&composed.stdout).unwrap();
    assert_eq!(composed["requires_verification"], true);
    assert_eq!(composed["requires_review"], true);
    assert_eq!(composed["final_success"], false);
}

#[test]
fn disperse_splits_patch_files_into_review_required_outputs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("a.txt"), "a0\n").unwrap();
    std::fs::write(dir.join("b.txt"), "b0\n").unwrap();
    std::fs::write(dir.join("c.txt"), "c0\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("a.txt"), "a1\n").unwrap();
    std::fs::write(dir.join("b.txt"), "b1\n").unwrap();
    std::fs::write(dir.join("c.txt"), "c1\n").unwrap();
    let out = draft(dir)
        .args(["create", "multi", "--json"])
        .output()
        .unwrap();
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap();
    let dispersed = draft(dir)
        .args([
            "disperse", pack_id, "--output", "part-a", "part-b", "--json",
        ])
        .output()
        .unwrap();
    assert!(
        dispersed.status.success(),
        "{}",
        String::from_utf8_lossy(&dispersed.stderr)
    );
    let dispersed: serde_json::Value = serde_json::from_slice(&dispersed.stdout).unwrap();
    assert_eq!(dispersed["requires_verification"], true);
    assert_eq!(dispersed["requires_review"], true);
    assert_eq!(dispersed["final_success"], false);
    let outputs = dispersed["output_pack_ids"].as_array().unwrap();
    let first = outputs[0].as_str().unwrap();
    let second = outputs[1].as_str().unwrap();
    let first_patch: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join(format!(".draft/changepacks/{first}/patch.json")))
            .unwrap(),
    )
    .unwrap();
    let second_patch: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join(format!(".draft/changepacks/{second}/patch.json")))
            .unwrap(),
    )
    .unwrap();
    let first_len = first_patch["files"].as_array().unwrap().len();
    let second_len = second_patch["files"].as_array().unwrap().len();
    assert_eq!(first_len + second_len, 3);
    assert!(first_len < 3);
    assert!(second_len < 3);
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
            && r["risk_level"] != "unknown"
            && r["event_refs"]
                .as_array()
                .map(|refs| !refs.is_empty())
                .unwrap_or(false)
    }));
    assert!(receipts
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["kind"] == "hook" && r["status"] == "succeeded"));
}

#[test]
fn final_decision_requires_human_actor() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(
        dir.join(".draft/identity.json"),
        r#"{"id":"act_agent","kind":"agent","display_name":"agent"}"#,
    )
    .unwrap();
    let pack_id = create_verified_reviewed_pack(dir, "agent-blocked");
    draft(dir)
        .args(["approve", "-p", &pack_id])
        .assert()
        .failure()
        .stderr(contains("human actor"));
}

#[test]
fn review_lock_blocks_mutating_pack_actions_until_decision() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    let pack_id = create_verified_reviewed_pack(dir, "locked");
    draft(dir)
        .args(["pack", "-d", &pack_id])
        .write_stdin("y\n")
        .assert()
        .failure()
        .stderr(contains("locked for review"));
    draft(dir)
        .args(["save", "-p", &pack_id])
        .assert()
        .failure()
        .stderr(contains("locked for review"));
    draft(dir)
        .args(["approve", "-p", &pack_id])
        .assert()
        .success();
    draft(dir).args(["save", "-p", &pack_id]).assert().success();
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
fn save_requires_current_passed_verification_receipt() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(dir, "stale-verification");
    let patch_path = dir
        .join(".draft/changepacks")
        .join(&pack_id)
        .join("patch.json");
    let mut patch: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&patch_path).unwrap()).unwrap();
    patch["patch_graph_hash"] = serde_json::json!("sha256:changed-after-verification");
    std::fs::write(&patch_path, serde_json::to_string_pretty(&patch).unwrap()).unwrap();

    draft(dir)
        .args(["save", "-p", &pack_id])
        .assert()
        .failure()
        .stderr(contains("current passed verification receipt"));
}

#[test]
fn hooks_save_rejects_workspace_escape_and_draft_env_override() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(
        dir.join(".draft/config.toml"),
        r#"[identity]
username = "Ada"
email = "ada@example.com"

[save]
message_template = "{{title}}"

[hooks.save]
command = "echo should-not-run"
cwd = ".."

[verification]
default_profile = "standard"

[policy]
require_verification = true
require_approval = true
require_human_approval_for_high_risk = true
block_if_tests_fail = true
"#,
    )
    .unwrap();
    let pack_id = create_verified_approved_pack(dir, "escape-hook");
    draft(dir)
        .args(["save", "-p", &pack_id])
        .assert()
        .failure()
        .stderr(contains("hook cwd"));

    std::fs::write(
        dir.join(".draft/config.toml"),
        r#"[identity]
username = "Ada"
email = "ada@example.com"

[save]
message_template = "{{title}}"

[hooks.save]
command = "echo should-not-run"

[hooks.save.env]
DRAFT_RECEIPT_ID = "fake"

[verification]
default_profile = "standard"

[policy]
require_verification = true
require_approval = true
require_human_approval_for_high_risk = true
block_if_tests_fail = true
"#,
    )
    .unwrap();
    draft(dir)
        .args(["save", "-p", &pack_id])
        .assert()
        .failure()
        .stderr(contains("cannot override Draft-managed variables"));
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
fn storage_doctor_checks_receipt_references_and_draft_exclusion() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    draft(dir)
        .args(["config", "set", "hooks.save", write_saved_message_command()])
        .assert()
        .success();
    let pack_id = create_verified_approved_pack(dir, "doctor-refs");
    draft(dir).args(["save", "-p", &pack_id]).assert().success();
    let receipts = draft(dir)
        .args(["receipt", "list", "--json"])
        .output()
        .unwrap();
    let receipts: serde_json::Value = serde_json::from_slice(&receipts.stdout).unwrap();
    let save_receipt_id = receipts
        .as_array()
        .unwrap()
        .iter()
        .find(|receipt| {
            receipt["changepack_id"] == pack_id && receipt["hook_receipt_refs"].is_array()
        })
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let receipt_path = dir
        .join(".draft/receipts")
        .join(format!("{save_receipt_id}.json"));
    let mut receipt: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&receipt_path).unwrap()).unwrap();
    receipt["hook_receipt_refs"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!("rcp_missing"));
    std::fs::write(
        &receipt_path,
        serde_json::to_string_pretty(&receipt).unwrap(),
    )
    .unwrap();
    draft(dir)
        .args(["storage", "doctor"])
        .assert()
        .success()
        .stdout(contains("missing hook receipt ref rcp_missing"));

    let patch_path = dir
        .join(".draft/changepacks")
        .join(&pack_id)
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
        .args(["storage", "doctor"])
        .assert()
        .success()
        .stdout(contains("\"draft_hard_excluded\": false"));
}

#[test]
fn save_fails_closed_when_canonical_ledger_is_tampered() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(dir, "tamper-ledger");

    let event_log = dir.join(".draft/events/event.log");
    let text = std::fs::read_to_string(&event_log).unwrap();
    std::fs::write(&event_log, text.replacen("PackApproved", "PackTampered", 1)).unwrap();

    draft(dir)
        .args(["save", "-p", &pack_id])
        .assert()
        .failure()
        .stderr(contains("OPERATION_LOG_CORRUPT"));
}

#[test]
fn tui_review_cockpit_shows_required_review_sections() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("auth.rs"), "fn auth() {}\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("auth.rs"), "fn auth_token() {}\n").unwrap();
    draft(dir).args(["create", "tui-risk"]).assert().success();
    draft(dir)
        .args(["review", "--tui"])
        .assert()
        .success()
        .stdout(
            contains("Overview")
                .and(contains("Hotspots"))
                .and(contains("Evidence Gaps"))
                .and(contains("Provenance"))
                .and(contains("Semantic Diff"))
                .and(contains("Raw Diff"))
                .and(contains("Timeline"))
                .and(contains("Decision"))
                .and(contains("Help")),
        );
}

#[test]
fn policy_failures_use_documented_exit_codes() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    let out = draft(dir)
        .args(["create", "exit-codes", "--json"])
        .output()
        .unwrap();
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap();
    let save = draft(dir).args(["save", "-p", pack_id]).output().unwrap();
    assert_eq!(save.status.code(), Some(5));

    draft(dir)
        .args(["verify", "-p", pack_id])
        .assert()
        .success();
    let save = draft(dir).args(["save", "-p", pack_id]).output().unwrap();
    assert_eq!(save.status.code(), Some(7));
}

#[test]
fn rollback_receipts_must_be_explicitly_reversible() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    let checkpoint = draft(dir)
        .args(["checkpoint", "base", "--json"])
        .output()
        .unwrap();
    let checkpoint: serde_json::Value = serde_json::from_slice(&checkpoint.stdout).unwrap();
    let checkpoint_id = checkpoint["snapshot_id"].as_str().unwrap();
    let checkpoint_receipt = checkpoint["receipt_id"].as_str().unwrap();

    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    draft(dir)
        .args(["rollback", checkpoint_receipt])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(dir.join("app.txt")).unwrap(),
        "v1\n"
    );

    std::fs::write(dir.join("app.txt"), "v3\n").unwrap();
    let pack_id = create_verified_approved_pack(dir, "non-reversible-save");
    draft(dir).args(["save", "-p", &pack_id]).assert().success();
    let receipts = draft(dir)
        .args(["receipt", "list", "--json"])
        .output()
        .unwrap();
    let receipts: serde_json::Value = serde_json::from_slice(&receipts.stdout).unwrap();
    let save_receipt = receipts
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["changepack_id"] == pack_id)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    draft(dir)
        .args(["rollback", &save_receipt])
        .assert()
        .failure()
        .stderr(contains("not reversible"));

    draft(dir)
        .args(["rollback", checkpoint_id])
        .assert()
        .success();
}

#[test]
fn pack_export_import_quarantine_name_conflicts_and_security() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    let dst = root.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    let artifact = root.path().join("pack.draftpack");

    // Source: create, verify, approve, save a pack, then export it.
    draft(&src).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(&src, "portable");
    draft(&src)
        .args(["save", "-p", &pack_id])
        .assert()
        .success();
    draft(&src)
        .args([
            "pack",
            "--export",
            &pack_id,
            "--output",
            artifact.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(artifact.exists());

    // Destination: import enters quarantine (imported_quarantined).
    draft(&dst).args(["init"]).assert().success();
    let out = draft(&dst)
        .args(["pack", "--import", artifact.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["quarantined"], true);
    let imported_id = report["pack_id"].as_str().unwrap();
    let manifest = dst
        .join(".draft/imports/quarantine")
        .join(imported_id)
        .join("manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(manifest["import_state"], "imported_quarantined");

    // Duplicate name without --name fails; with a unique --name it succeeds.
    draft(&dst)
        .args(["pack", "--import", artifact.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("duplicate pack name"));
    draft(&dst)
        .args([
            "pack",
            "--import",
            artifact.to_str().unwrap(),
            "--name",
            "portable-2",
        ])
        .assert()
        .success();

    // A malicious artifact that writes into .draft/ is rejected, fail closed.
    let evil = root.path().join("evil.draftpack");
    write_tar_with_entry(&evil, ".draft/keys/signing.key", b"stolen");
    draft(&dst)
        .args(["pack", "--import", evil.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains(".draft/"));
}

#[test]
fn verify_v2_produces_risk_lsif_and_selection_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    std::fs::write(dir.join("src/auth.rs"), "fn old() {}\n").unwrap();
    std::fs::write(dir.join("tests/auth_test.rs"), "fn t() {}\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("src/auth.rs"), "pub fn validate_token() {}\n").unwrap();
    std::fs::write(
        dir.join("tests/auth_test.rs"),
        "fn t() { validate_token(); }\n",
    )
    .unwrap();

    let out = draft(dir)
        .args(["create", "sec-fix", "--json"])
        .output()
        .unwrap();
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_id = pack["id"].as_str().unwrap();

    // Evidence-based verify with a positional target.
    let out = draft(dir)
        .args(["verify", pack_id, "--explain", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(report["public_api_changed"].as_u64().unwrap() >= 1);
    assert!(report["symbols_touched"].as_u64().unwrap() >= 1);

    // Canonical evidence files exist.
    let pack_dir = dir.join(".draft/packs").join(pack_id);
    for f in ["risk.json", "verify.json", "lsif.json"] {
        assert!(pack_dir.join(f).exists(), "missing {f}");
    }
    let lsif: serde_json::Value =
        serde_json::from_slice(&std::fs::read(pack_dir.join("lsif.json")).unwrap()).unwrap();
    assert_eq!(lsif["backend"], "basic");

    // The PackVerified receipt is present and verifies.
    draft(dir)
        .args(["receipt", "verify", "--all"])
        .assert()
        .success();
}

#[test]
fn pack_algebra_inspect_conflicts_and_compose() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.rs"), "fn a() {}\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();
    std::fs::write(dir.join("src/a.rs"), "pub fn alpha() {}\n").unwrap();
    let out = draft(dir)
        .args(["create", "pack-a", "--json"])
        .output()
        .unwrap();
    let pack: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pack_a = pack["id"].as_str().unwrap().to_string();
    draft(dir).args(["verify", &pack_a]).assert().success();

    // inspect shows verified lifecycle and public API impact.
    let out = draft(dir)
        .args(["pack", "inspect", &pack_a, "--json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["verified"], true);
    assert!(report["public_api_changed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s == "alpha"));

    // A pack semantically conflicts with itself (same symbols) — blocking.
    draft(dir)
        .args(["pack", "conflicts", &pack_a, &pack_a])
        .assert()
        .failure();

    // Composing a pack with itself is refused due to the blocking conflict.
    draft(dir)
        .args(["pack", "compose", &pack_a, &pack_a, "--name", "combo"])
        .assert()
        .failure()
        .stderr(contains("blocking conflicts"));

    // Composing with the (empty) base pack succeeds and requires re-verification.
    let base = pack_id_by_name(dir, "base");
    let out = draft(dir)
        .args([
            "pack", "compose", &base, &pack_a, "--name", "combo", "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["requires_reverification"], true);
    assert_eq!(report["dependencies"].as_array().unwrap().len(), 2);
}

fn pack_id_by_name(dir: &std::path::Path, name: &str) -> String {
    let out = draft(dir).args(["list", "--json"]).output().unwrap();
    let packs: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    packs
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Build a minimal tar with a single (possibly malicious) entry for tests.
fn write_tar_with_entry(path: &std::path::Path, name: &str, data: &[u8]) {
    let mut block = [0u8; 512];
    let nb = name.as_bytes();
    let n = nb.len().min(100);
    block[..n].copy_from_slice(&nb[..n]);
    block[100..108].copy_from_slice(b"0000644\0");
    block[108..116].copy_from_slice(b"0000000\0");
    block[116..124].copy_from_slice(b"0000000\0");
    block[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
    block[136..148].copy_from_slice(b"00000000000\0");
    block[156] = b'0';
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    for b in block.iter_mut().skip(148).take(8) {
        *b = b' ';
    }
    let sum: u32 = block.iter().map(|&b| b as u32).sum();
    block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    let mut out = block.to_vec();
    out.extend_from_slice(data);
    let pad = (512 - data.len() % 512) % 512;
    out.resize(out.len() + pad, 0u8);
    out.extend_from_slice(&[0u8; 1024]);
    std::fs::write(path, out).unwrap();
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

#[test]
fn save_blocked_on_critical_canonical_risk() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(dir, "critical-risk");

    // Escalate the canonical risk report to critical: the save gate must
    // fail closed even though the pack is verified and approved.
    let risk_path = dir.join(".draft/packs").join(&pack_id).join("risk.json");
    let mut risk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&risk_path).unwrap()).unwrap();
    risk["risk_level"] = serde_json::Value::String("critical".into());
    risk["risk_score"] = serde_json::json!(95);
    std::fs::write(&risk_path, serde_json::to_string_pretty(&risk).unwrap()).unwrap();

    draft(dir)
        .args(["save", "-p", &pack_id])
        .assert()
        .failure()
        .stderr(contains("critical risk"));
}

#[test]
fn project_policy_can_disable_critical_block() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(dir, "policy-override");

    let risk_path = dir.join(".draft/packs").join(&pack_id).join("risk.json");
    let mut risk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&risk_path).unwrap()).unwrap();
    risk["risk_level"] = serde_json::Value::String("critical".into());
    std::fs::write(&risk_path, serde_json::to_string_pretty(&risk).unwrap()).unwrap();

    // Canonical policy keys are top-level; prepend so any legacy tables in the
    // seeded policy.toml stay in their own sections.
    let policy_path = dir.join(".draft/policy.toml");
    let existing = std::fs::read_to_string(&policy_path).unwrap_or_default();
    std::fs::write(
        &policy_path,
        format!(
            "block_on_critical_risk = false\nrequire_approval_on_high_risk = false\n{existing}"
        ),
    )
    .unwrap();

    draft(dir).args(["save", "-p", &pack_id]).assert().success();
}

#[test]
fn import_rejects_wrong_schema_receipt() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    let dst = root.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    let artifact = root.path().join("pack.draftpack");

    draft(&src).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(&src, "receipt-schema");
    draft(&src)
        .args([
            "pack",
            "--export",
            &pack_id,
            "--output",
            artifact.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Tamper: inject a wrong-schema receipt into an otherwise valid artifact.
    let archive = draft_core::importexport::read_archive(&artifact).unwrap();
    let mut entries: Vec<(String, Vec<u8>)> = archive.entries.into_iter().collect();
    entries.push((
        "receipts/rcp_bogus.json".to_string(),
        br#"{"not":"a receipt"}"#.to_vec(),
    ));
    let tampered = root.path().join("tampered.draftpack");
    draft_core::importexport::write_archive(&tampered, &entries).unwrap();

    draft(&dst).args(["init"]).assert().success();
    // Rejected under --dry-run and for real; quarantine stays empty.
    draft(&dst)
        .args(["pack", "--import", tampered.to_str().unwrap(), "--dry-run"])
        .assert()
        .failure()
        .stderr(contains("receipt"));
    draft(&dst)
        .args(["pack", "--import", tampered.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("receipt"));
    let quarantine = dst.join(".draft/imports/quarantine");
    let empty = !quarantine.exists() || std::fs::read_dir(&quarantine).unwrap().next().is_none();
    assert!(empty, "quarantine must stay empty after a rejected import");

    // The untampered artifact (whose receipts are schema-valid) still imports.
    draft(&dst)
        .args(["pack", "--import", artifact.to_str().unwrap(), "--json"])
        .assert()
        .success();
}

#[test]
fn import_fails_on_tampered_embedded_object() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    let dst = root.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    let artifact = root.path().join("pack.draftpack");

    draft(&src).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(&src, "objectful");
    draft(&src)
        .args([
            "pack",
            "--export",
            &pack_id,
            "--output",
            artifact.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Format 2 must embed the content-addressed objects for the patch.
    let archive = draft_core::importexport::read_archive(&artifact).unwrap();
    let object_names: Vec<String> = archive
        .entries
        .keys()
        .filter(|k| k.starts_with("objects/"))
        .cloned()
        .collect();
    assert!(
        !object_names.is_empty(),
        "export must embed content objects"
    );

    // Tamper with one object's bytes: content no longer matches its name.
    let mut entries: Vec<(String, Vec<u8>)> = archive.entries.into_iter().collect();
    for (name, bytes) in entries.iter_mut() {
        if name == &object_names[0] {
            bytes.push(b'!');
        }
    }
    let tampered = root.path().join("tampered.draftpack");
    draft_core::importexport::write_archive(&tampered, &entries).unwrap();

    draft(&dst).args(["init"]).assert().success();
    draft(&dst)
        .args(["pack", "--import", tampered.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("content hash mismatch"));
}

#[test]
fn imported_pack_full_lifecycle_to_save() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    let dst = root.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    let artifact = root.path().join("pack.draftpack");

    // Workspace A: full local trust path, then export.
    draft(&src).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(&src, "lifecycle");
    draft(&src)
        .args(["save", "-p", &pack_id])
        .assert()
        .success();
    draft(&src)
        .args([
            "pack",
            "--export",
            &pack_id,
            "--output",
            artifact.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Workspace B: same baseline so the change applies cleanly.
    draft(&dst).args(["init"]).assert().success();
    std::fs::write(dst.join("app.txt"), "v1\n").unwrap();

    let out = draft(&dst)
        .args(["pack", "--import", artifact.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = report["pack_id"].as_str().unwrap().to_string();

    let inspect = |dir: &std::path::Path, id: &str| -> serde_json::Value {
        let out = draft(dir)
            .args(["pack", "inspect", id, "--json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };

    // Quarantined: origin trust never carries over.
    let report = inspect(&dst, &id);
    assert_eq!(report["lifecycle"], "imported_quarantined");
    assert_eq!(report["verified"], false);

    // Save and approve are blocked before local verification.
    draft(&dst)
        .args(["save", "-p", &id])
        .assert()
        .failure()
        .stderr(contains("verified"));
    draft(&dst)
        .args(["approve", "-p", &id])
        .assert()
        .failure()
        .stderr(contains("locally verified"));

    // Local re-verification from embedded content.
    draft(&dst).args(["verify", &id]).assert().success();
    assert_eq!(inspect(&dst, &id)["lifecycle"], "import_verified");

    // Approval is still required before save.
    draft(&dst)
        .args(["save", "-p", &id])
        .assert()
        .failure()
        .stderr(contains("approved"));
    draft(&dst).args(["approve", "-p", &id]).assert().success();
    assert_eq!(inspect(&dst, &id)["lifecycle"], "import_approved");

    // Save applies the embedded content and promotes out of quarantine.
    draft(&dst).args(["save", "-p", &id]).assert().success();
    assert_eq!(
        std::fs::read_to_string(dst.join("app.txt")).unwrap(),
        "v2\n"
    );
    assert!(!dst.join(".draft/imports/quarantine").join(&id).exists());
    assert!(dst
        .join(".draft/packs")
        .join(&id)
        .join("manifest.json")
        .exists());
    assert_eq!(inspect(&dst, &id)["lifecycle"], "import_saved");

    // The full trust chain in B verifies, and every lifecycle event exists.
    draft(&dst)
        .args(["receipt", "verify", "--all"])
        .assert()
        .success();
    let events = std::fs::read_to_string(dst.join(".draft/events/event.log")).unwrap();
    for kind in ["PackImported", "PackVerified", "PackApproved", "PackSaved"] {
        assert!(events.contains(kind), "missing {kind} in: {events}");
    }
}

#[test]
fn imported_pack_reject_is_terminal() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    let dst = root.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    let artifact = root.path().join("pack.draftpack");

    draft(&src).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(&src, "reject-me");
    draft(&src)
        .args([
            "pack",
            "--export",
            &pack_id,
            "--output",
            artifact.to_str().unwrap(),
        ])
        .assert()
        .success();

    draft(&dst).args(["init"]).assert().success();
    let out = draft(&dst)
        .args(["pack", "--import", artifact.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = report["pack_id"].as_str().unwrap().to_string();

    draft(&dst).args(["reject", "-p", &id]).assert().success();

    // Terminal: verify, approve, and save all refuse a rejected import.
    draft(&dst)
        .args(["verify", &id])
        .assert()
        .failure()
        .stderr(contains("cannot be verified"));
    draft(&dst).args(["approve", "-p", &id]).assert().failure();
    draft(&dst)
        .args(["save", "-p", &id])
        .assert()
        .failure()
        .stderr(contains("rejected"));
}

#[test]
fn import_save_conflicts_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    let dst = root.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    let artifact = root.path().join("pack.draftpack");

    draft(&src).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(&src, "conflicting");
    draft(&src)
        .args([
            "pack",
            "--export",
            &pack_id,
            "--output",
            artifact.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Workspace B has DIFFERENT local content at the changed path.
    draft(&dst).args(["init"]).assert().success();
    std::fs::write(dst.join("app.txt"), "locally different\n").unwrap();

    let out = draft(&dst)
        .args(["pack", "--import", artifact.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = report["pack_id"].as_str().unwrap().to_string();

    draft(&dst).args(["verify", &id]).assert().success();
    draft(&dst).args(["approve", "-p", &id]).assert().success();
    draft(&dst)
        .args(["save", "-p", &id])
        .assert()
        .failure()
        .stderr(contains("conflict").or(contains("differs")));

    // Nothing was written and the pack stays approved in quarantine.
    assert_eq!(
        std::fs::read_to_string(dst.join("app.txt")).unwrap(),
        "locally different\n"
    );
    assert!(dst.join(".draft/imports/quarantine").join(&id).exists());
}

#[test]
fn import_verify_fails_on_tampered_changes_patch() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    let dst = root.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    let artifact = root.path().join("pack.draftpack");

    draft(&src).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(&src, "tamper-me");
    draft(&src)
        .args([
            "pack",
            "--export",
            &pack_id,
            "--output",
            artifact.to_str().unwrap(),
        ])
        .assert()
        .success();

    draft(&dst).args(["init"]).assert().success();
    let out = draft(&dst)
        .args(["pack", "--import", artifact.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = report["pack_id"].as_str().unwrap().to_string();

    // Tamper with the quarantined changes.patch after import.
    let patch_path = dst
        .join(".draft/imports/quarantine")
        .join(&id)
        .join("changes.patch");
    let mut bytes = std::fs::read(&patch_path).unwrap();
    bytes.push(b' ');
    std::fs::write(&patch_path, bytes).unwrap();

    draft(&dst)
        .args(["verify", &id])
        .assert()
        .failure()
        .stderr(contains("changes_hash"));
    // State is unchanged: still quarantined.
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            dst.join(".draft/imports/quarantine")
                .join(&id)
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["import_state"], "imported_quarantined");
}

/// Find canonical receipt ids in `.draft/receipts` for a given event type.
fn canonical_receipts_of_type(dir: &std::path::Path, event_type: &str) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir.join(".draft/receipts")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_slice(&std::fs::read(&path).unwrap())
        {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value["event_type"] == event_type {
            if let Some(id) = value["receipt_id"].as_str() {
                out.push(id.to_string());
            }
        }
    }
    out
}

#[test]
fn rollback_accepts_canonical_receipt_id() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "original\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();

    let receipts = canonical_receipts_of_type(dir, "CheckpointCreated");
    assert!(
        !receipts.is_empty(),
        "checkpoint must record a canonical CheckpointCreated receipt"
    );

    std::fs::write(dir.join("app.txt"), "mutated\n").unwrap();
    draft(dir)
        .args(["rollback", &receipts[0]])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(dir.join("app.txt")).unwrap(),
        "original\n"
    );
}

#[test]
fn rollback_rejects_non_eligible_canonical_receipt() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("ws");
    std::fs::create_dir_all(&dir).unwrap();
    draft(&dir).args(["init"]).assert().success();
    let pack_id = create_verified_approved_pack(&dir, "exported");
    draft(&dir)
        .args([
            "pack",
            "--export",
            &pack_id,
            "--output",
            root.path().join("x.draftpack").to_str().unwrap(),
        ])
        .assert()
        .success();

    let receipts = canonical_receipts_of_type(&dir, "PackExported");
    assert!(!receipts.is_empty());
    draft(&dir)
        .args(["rollback", &receipts[0]])
        .assert()
        .failure()
        .stderr(contains("not rollback-eligible"));
}

#[test]
fn doctor_global_lists_adapter_statuses() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init", "--global"]).assert().success();
    let out = draft(dir)
        .args(["doctor", "--global", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(text.contains("acp-comm"), "{text}");
    assert!(text.contains("experimental"), "{text}");
}

#[test]
fn pack_depends_reports_shared_symbol_packs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("lib.rs"), "pub fn shared_api() {}\n").unwrap();
    draft(dir).args(["checkpoint", "base"]).assert().success();

    std::fs::write(dir.join("lib.rs"), "pub fn shared_api() { /* a */ }\n").unwrap();
    let out = draft(dir)
        .args(["create", "pack-a", "--json"])
        .output()
        .unwrap();
    let a: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let a_id = a["id"].as_str().unwrap().to_string();
    draft(dir).args(["verify", &a_id]).assert().success();

    draft(dir).args(["checkpoint", "mid"]).assert().success();
    std::fs::write(dir.join("lib.rs"), "pub fn shared_api() { /* b */ }\n").unwrap();
    let out = draft(dir)
        .args(["create", "pack-b", "--json"])
        .output()
        .unwrap();
    let b: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let b_id = b["id"].as_str().unwrap().to_string();
    draft(dir).args(["verify", &b_id]).assert().success();

    let out = draft(dir)
        .args(["pack", "depends", &a_id, "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let shared = report["shared_symbol_packs"].as_object().unwrap();
    assert!(
        shared.contains_key(&b_id),
        "pack-b should be shortlisted as touching shared_api: {report}"
    );
}
