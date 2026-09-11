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

/// A workspace whose accepted Baseline holds `app.txt`, and one edit to it.
///
/// The Baseline is accepted over whatever `init` observes, so the file has to
/// exist before it — and a Change may only be sealed when something in its
/// scope actually differs from what the Baseline accepts.
fn a_change_in_progress(dir: &std::path::Path) -> (String, String) {
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).arg("init").assert().success();

    let opened = draft(dir)
        .args([
            "change",
            "new",
            "edit the app",
            "--scope",
            "app.txt",
            "--json",
        ])
        .assert()
        .success();
    let change: serde_json::Value = serde_json::from_slice(&opened.get_output().stdout).unwrap();
    let change_id = change["id"].as_str().unwrap().to_string();

    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    let sealed = draft(dir)
        .args(["change", "revision", "seal", &change_id, "--json"])
        .assert()
        .success();
    let revision: serde_json::Value = serde_json::from_slice(&sealed.get_output().stdout).unwrap();
    let revision_id = revision["id"].as_str().unwrap().to_string();

    (change_id, revision_id)
}

/// A revision carried all the way onto the accepted Baseline.
///
/// Evidence, an assessment, a satisfied gate, an approving decision, then the
/// promotion. Nothing here is optional: each step is what makes the next one
/// legal, which is the point of the sequence.
fn a_promoted_revision(dir: &std::path::Path) -> (String, String) {
    // A check the project declares itself. Without one, nothing is installed
    // that could check anything and the evidence is `unavailable` — true, and
    // not a pass, so the gate would refuse. These tests are about what happens
    // *after* a gate is satisfied, so they give it something to satisfy it.
    let (change_id, revision_id) = a_change_in_progress(dir);
    std::fs::write(
        dir.join(".draft/verify.toml"),
        r#"schema_version = 1

[[checks]]
name = "always"
enabled = true

[checks.command]
program = "true"
args = []
"#,
    )
    .unwrap();

    draft(dir)
        .args(["change", "evidence", "run", &revision_id])
        .assert()
        .success();
    draft(dir)
        .args([
            "change",
            "assess",
            &revision_id,
            "--risk",
            "low",
            "--rationale",
            "reviewed in this test project",
        ])
        .assert()
        .success();
    let gate = draft(dir)
        .args(["change", "gates", "evaluate", &revision_id, "--json"])
        .assert()
        .success();
    let gate: serde_json::Value = serde_json::from_slice(&gate.get_output().stdout).unwrap();
    let gate_id = gate["id"].as_str().unwrap().to_string();

    let decision = draft(dir)
        .args([
            "change",
            "decide",
            &revision_id,
            "--approve",
            "--gate",
            &gate_id,
            "--json",
        ])
        .assert()
        .success();
    let decision: serde_json::Value =
        serde_json::from_slice(&decision.get_output().stdout).unwrap();
    let decision_id = decision["id"].as_str().unwrap().to_string();

    draft(dir)
        .args([
            "promote",
            &change_id,
            &revision_id,
            "--gate",
            &gate_id,
            "--decision",
            &decision_id,
        ])
        .assert()
        .success();

    (change_id, revision_id)
}

fn same_canonical_path(left: &std::path::Path, right: &std::path::Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

/// Install a language extension supplying the symbol and verification rules
/// Draft Core no longer carries itself, and authorize its commands.
///
/// The package is written outside the workspace on purpose: anything created
/// inside it would change the workspace hash and trip the save gate.
/// Install one extension that contributes real domain knowledge.
///
/// Deliberately minimal and deliberately *contributed*: a class it assigns, an
/// extractor that projects an attribute into an element, and a command-backed
/// check. None of this vocabulary exists anywhere in Draft — the point of these
/// tests is that installing the package is what makes the capability appear.
fn install_language_extension(dir: &std::path::Path) -> tempfile::TempDir {
    let package_home = tempfile::tempdir().unwrap();
    let package = package_home.path().join("draft.language.example");
    std::fs::create_dir_all(package.join("contributions")).unwrap();
    std::fs::create_dir_all(package.join("docs")).unwrap();

    std::fs::write(
        package.join("contributions/classification.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "class_id": "draft.language.example/source",
            "display_name": "Example source",
            "applies_to": { "predicate": "path_suffix", "suffix": ".rs" },
            "attributes": []
        }))
        .unwrap(),
    )
    .unwrap();

    std::fs::write(
        package.join("contributions/extraction.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "extractor_id": "draft.language.example/units",
            "applies_to": {
                "predicate": "has_class",
                "class_id": "draft.language.example/source"
            },
            "result_contract": {
                "schema_id": "draft.core/element-set",
                "revision": 1
            },
            "operation": {
                "request_contract": {
                    "schema_id": "draft.core/extraction-request",
                    "revision": 1
                },
                "response_contract": {
                    "schema_id": "draft.core/extraction-result",
                    "revision": 1
                },
                "max_response_bytes": 65536,
                "executor": {
                    "kind": "engine",
                    "engine": "attribute_projection",
                    "engine_revision": 1,
                    // Projects the executable-bit attribute the filesystem
                    // adapter records. Deliberately something Draft already
                    // observes: what it *means* is this extension's business.
                    "config": {
                        "elements": [
                            {
                                "attribute": "file.executable",
                                "kind": "draft.language.example/executable",
                                "carry": []
                            }
                        ],
                        "relations": []
                    }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    std::fs::write(
        package.join("contributions/verification.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "checks": [{
                "check_id": "draft.language.example/suite",
                "display_name": "Example suite",
                "applies_to": {
                    "predicate": "has_class",
                    "class_id": "draft.language.example/source"
                },
                "requirement": "required",
                "selection": "whole",
                "operation": {
                    "request_contract": {
                        "schema_id": "draft.core/verification-request",
                        "revision": 1
                    },
                    "response_contract": {
                        "schema_id": "draft.core/verification-result",
                        "revision": 1
                    },
                    "max_response_bytes": 65536,
                    "executor": {
                        "kind": "command",
                        "command": { "program": "true", "args": [] }
                    }
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    std::fs::write(package.join("docs/readme.md"), "# Example language\n").unwrap();
    std::fs::write(package.join("LICENSE.txt"), "Example license\n").unwrap();
    std::fs::write(
        package.join("extension.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "id": "draft.language.example",
            "name": "Example language",
            "version": "1.0.0",
            "publisher": "draft",
            "draft_api": "^0.3.4",
            "contributions": [
                {
                    "id": "classification",
                    "kind": "resource_classification",
                    "path": "contributions/classification.json"
                },
                {
                    "id": "extraction",
                    "kind": "element_extraction",
                    "path": "contributions/extraction.json"
                },
                {
                    "id": "verification",
                    "kind": "verification",
                    "path": "contributions/verification.json"
                }
            ],
            "permissions": ["process.execute"],
            "documentation": ["docs/readme.md"],
            "licenses": ["LICENSE.txt"],
            "assets": [],
            "schemas": []
        }))
        .unwrap(),
    )
    .unwrap();

    draft(dir)
        .args([
            "extension",
            "install",
            package.to_str().unwrap(),
            "--grant",
            "process.execute",
            "--json",
        ])
        .assert()
        .success();
    package_home
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
fn cli_startup_and_init_are_available_in_debug_builds() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    draft(dir)
        .args(["--version"])
        .assert()
        .success()
        .stdout(contains("0.3.4"));
    draft(dir)
        .args(["--help"])
        .assert()
        .success()
        .stdout(contains("Usage:"));
    draft(dir)
        .args(["task", "--help"])
        .assert()
        .success()
        .stdout(contains("Manage tasks"));

    let initialized = draft(dir).args(["init", "--json"]).output().unwrap();
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&initialized.stdout).unwrap();
    let reported_root = report["root"].as_str().expect("init root must be a string");
    assert!(
        same_canonical_path(std::path::Path::new(reported_root), dir),
        "reported root {reported_root} does not resolve to {}",
        dir.display()
    );
    assert_eq!(report["created"], true);
}

#[test]
fn console_is_the_only_ui_command_and_extensions_are_management_only() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let help = draft(dir).args(["--help"]).output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.lines().any(|line| line.starts_with("  console")));
    assert!(!help.lines().any(|line| line.starts_with("  ui")));
    assert!(!help.lines().any(|line| line.starts_with("  cockpit")));
    assert!(!help.lines().any(|line| line.starts_with("  identity")));

    draft(dir)
        .args(["console", "--help"])
        .assert()
        .success()
        .stdout(contains("web").and(contains("tui")));
    draft(dir)
        .args(["console", "web", "--help"])
        .assert()
        .success()
        .stdout(
            contains("--port")
                .and(contains("--project"))
                .and(contains("--no-open")),
        );
    draft(dir)
        .args(["console", "tui", "--help"])
        .assert()
        .success()
        .stdout(
            contains("--project")
                .and(contains("--no-preselect"))
                .and(contains("--port").not()),
        );
    draft(dir)
        .args(["console"])
        .assert()
        .failure()
        .stderr(contains("console mode is required"));
    draft(dir)
        .args(["console", "web", "--project", "prj_test", "--no-preselect"])
        .assert()
        .failure();
    for arguments in [
        ["console", "tui", "--port", "4318"],
        ["console", "tui", "--no-open", ""],
        ["console", "--tui", "", ""],
    ] {
        draft(dir)
            .args(arguments.into_iter().filter(|value| !value.is_empty()))
            .assert()
            .failure();
    }
    for retired in ["ui", "cockpit", "identity"] {
        draft(dir).args([retired, "--help"]).assert().failure();
    }
    draft(dir)
        .args(["extension", "run", "example"])
        .assert()
        .failure();

    let listed = draft(dir)
        .args(["extension", "list", "--json"])
        .output()
        .unwrap();
    assert!(listed.status.success());
    let extensions: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(extensions, serde_json::json!([]));
}

#[test]
fn extension_packages_can_be_managed_but_not_executed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let package = dir.join("example-extension");
    std::fs::create_dir_all(package.join("contributions")).unwrap();
    std::fs::create_dir_all(package.join("docs")).unwrap();
    std::fs::write(package.join("contributions/task.json"), "{}").unwrap();
    std::fs::write(package.join("docs/readme.md"), "# Example\n").unwrap();
    std::fs::write(package.join("LICENSE.txt"), "Example license\n").unwrap();
    std::fs::write(
        package.join("extension.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "id": "example",
            "name": "Example",
            "version": "1.0.0",
            "publisher": "test",
            "draft_api": "^0.3.4",
            "contributions": [{
                "id": "example-task",
                "kind": "task_template",
                "path": "contributions/task.json"
            }],
            "documentation": ["docs/readme.md"],
            "licenses": ["LICENSE.txt"],
            "assets": []
        }))
        .unwrap(),
    )
    .unwrap();

    draft(dir)
        .args(["extension", "install", package.to_str().unwrap(), "--json"])
        .assert()
        .success();
    let listed = draft(dir)
        .args(["extension", "list", "--json"])
        .output()
        .unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["manifest"]["id"], "example");
    assert!(listed[0].get("builtin").is_none());

    draft(dir)
        .args(["extension", "disable", "example", "--json"])
        .assert()
        .success()
        .stdout(contains("\"enabled\": false"));
    draft(dir)
        .args(["extension", "enable", "example", "--json"])
        .assert()
        .success()
        .stdout(contains("\"enabled\": true"));
    draft(dir)
        .args(["extension", "uninstall", "example", "--json"])
        .assert()
        .success();
}

#[test]
fn init_status_ignore_and_events_work_without_vcs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    draft(dir).args(["init"]).assert().success().stdout(
        contains("Initialized Draft workspace")
            .and(contains("draft task wizard"))
            .and(contains("draft console"))
            .and(contains("No command candidates")),
    );
    assert!(dir.join(".draft/config.toml").exists());
    assert!(dir.join(".draft/.ignore").exists());
    assert!(dir.join(".draft/events/events.log").exists());

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
        .args(["config", "ignore", "add", "notes/"])
        .assert()
        .success();
    draft(dir)
        .args(["config", "ignore", "list"])
        .assert()
        .success()
        .stdout(contains("notes/"));
    draft(dir).args(["init", "--global"]).assert().success();
    draft(dir)
        .args(["doctor"])
        .assert()
        .success()
        .stdout(contains("activity-chain"));
}

#[test]
fn task_rich_views_decompose_export_and_import() {
    let src = tempfile::tempdir().unwrap();
    let src = src.path();
    draft(src).args(["init"]).assert().success();

    let create = draft(src)
        .args([
            "task",
            "create",
            "docs-refactor",
            "--goal",
            "Refactor documentation layout",
            "--allow",
            "docs/**",
            "--success",
            "Docs remain accurate",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );
    let task: serde_json::Value = serde_json::from_slice(&create.stdout).unwrap();
    // Templates are contributed, and nothing is installed here. A task without
    // one is the ordinary case, not a degraded one.
    assert_eq!(task["template"], serde_json::Value::Null);

    let rich = draft(src)
        .args([
            "task",
            "show",
            "docs-refactor",
            "--executions",
            "--changes",
            "--evidence",
            "--timeline",
            "--lanes",
            "--explain",
            "--decompose",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        rich.status.success(),
        "{}",
        String::from_utf8_lossy(&rich.stderr)
    );
    let rich: serde_json::Value = serde_json::from_slice(&rich.stdout).unwrap();
    assert!(rich["executions"].is_array());
    assert!(rich["produced_changes"].is_array());
    assert!(rich["evidence"].is_array());
    assert!(rich["timeline"].is_array());
    assert!(rich["lanes"].is_array());
    assert_eq!(rich["explain"]["template"], serde_json::Value::Null);
    // Decomposition follows a template's steps, and templates are contributed.
    // With none installed there is nothing to decompose into, which the view
    // reports as an empty set rather than inventing child tasks.
    assert!(rich["decomposition"]["created_or_existing"]
        .as_array()
        .unwrap()
        .is_empty());

    let export_path = src.join("task-export.json");
    draft(src)
        .args([
            "task",
            "export",
            "docs-refactor",
            "--output",
            export_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();
    assert!(export_path.exists());

    let dst = tempfile::tempdir().unwrap();
    let dst = dst.path();
    draft(dst).args(["init"]).assert().success();
    let imported = draft(dst)
        .args([
            "task",
            "import",
            export_path.to_str().unwrap(),
            "--name",
            "imported-docs-refactor",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let imported: serde_json::Value = serde_json::from_slice(&imported.stdout).unwrap();
    assert_eq!(imported["task_name"], "imported-docs-refactor");

    let shown = draft(dst)
        .args(["task", "show", "imported-docs-refactor", "--json"])
        .output()
        .unwrap();
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["task"]["kind"], "imported");

    draft(dst)
        .args([
            "task",
            "import",
            export_path.to_str().unwrap(),
            "--name",
            "imported-docs-refactor",
            "--json",
        ])
        .assert()
        .failure()
        .stderr(contains("TASK_DEFINITION_CONFLICT"));
}

#[test]
fn status_component_filters_are_honored() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    draft(dir)
        .args([
            "task",
            "create",
            "status-task",
            "--goal",
            "Exercise status views",
            // Success criteria came from a built-in template before; with
            // templates contributed and none installed, the task states its own.
            "--success",
            "Status views render",
            "--candidate-preset",
            "fast",
            "--json",
        ])
        .assert()
        .success();

    let tasks = draft(dir)
        .args(["status", "-c", "tasks", "--json"])
        .output()
        .unwrap();
    assert!(tasks.status.success());
    let tasks: serde_json::Value = serde_json::from_slice(&tasks.stdout).unwrap();
    assert_eq!(tasks["component"], "tasks");
    assert_eq!(tasks["sections"]["tasks"]["count"], 1);
    assert!(tasks["sections"].get("candidates").is_none());

    let candidates = draft(dir)
        .args(["status", "-c", "candidates", "--full", "--json"])
        .output()
        .unwrap();
    assert!(candidates.status.success());
    let candidates: serde_json::Value = serde_json::from_slice(&candidates.stdout).unwrap();
    assert_eq!(candidates["component"], "candidates");
    assert!(candidates["sections"]["candidates"]["items"].is_array());

    let hooks = draft(dir)
        .args(["status", "-c", "hooks", "--json"])
        .output()
        .unwrap();
    assert!(hooks.status.success());
    let hooks: serde_json::Value = serde_json::from_slice(&hooks.stdout).unwrap();
    assert!(hooks["sections"]["hooks"]["verify"].is_string());
    // There is no submit slot. A hook can only name an operation Draft has,
    // and reporting one for an operation that was removed would offer a
    // configuration that could never fire.
    assert!(hooks["sections"]["hooks"].get("submit").is_none());

    draft(dir)
        .args(["status", "-c", "unknown"])
        .assert()
        .failure()
        .stderr(contains("unknown status component"));
}

#[test]
fn task_create_validates_candidate_preset_and_wizard_collects_full_contract() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();

    draft(dir)
        .args([
            "task",
            "create",
            "bad-preset",
            "--goal",
            "Reject unknown preset",
            "--success",
            "Validation fails",
            "--candidate-preset",
            "missing",
        ])
        .assert()
        .failure()
        .stderr(contains("task preset 'missing' is not configured"));

    draft(dir)
        .args([
            "task",
            "create",
            "protected-zone",
            "--goal",
            "Reject protected allowed zone",
            "--success",
            "Validation fails",
            // Draft's own control plane: the one protection Core owns, and the
            // one no configuration can lift. A credential-shaped path like
            // `.env` is *not* protected here, because with nothing installed
            // nobody has said it should be.
            "--allow",
            ".draft/**",
        ])
        .assert()
        .failure()
        .stderr(contains("conflicts with protected-file rules"));

    draft(dir)
        .args(["task", "wizard", "--json"])
        .write_stdin(
            "wizard-task\n\nUpdate docs\nbilling/**\ndocs/**\ndocs/private/**\nDocs are accurate\nlow\ny\nfast\ny\n",
        )
        .assert()
        .success();
    let shown = draft(dir)
        .args(["task", "show", "wizard-task", "--json"])
        .output()
        .unwrap();
    assert!(shown.status.success());
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["task"]["template"], serde_json::Value::Null);
    assert_eq!(shown["task"]["mode"], "plan_first");
    assert_eq!(shown["task"]["candidate_preset"], "fast");
    assert!(shown["task"]["forbidden_zones"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "billing/**"));

    draft(dir)
        .args(["task", "wizard", "--json"])
        .write_stdin("cancel-task\n\nDo not write\n\n\n\nSuccess\nlow\nn\nfast\nn\n")
        .assert()
        .failure()
        .stderr(contains("task wizard cancelled"));
    draft(dir)
        .args(["task", "show", "cancel-task"])
        .assert()
        .failure()
        .stderr(contains("was not found"));
}

#[test]
fn close_and_gc_follow_local_maintenance_contracts() {
    let tmp = tempfile::tempdir().unwrap();
    let clean = tmp.path().join("clean");
    let dirty = tmp.path().join("dirty");
    std::fs::create_dir_all(&clean).unwrap();
    std::fs::create_dir_all(&dirty).unwrap();

    draft(&clean).args(["init"]).assert().success();
    draft(&clean)
        .args(["maintenance", "gc"])
        .assert()
        .success()
        .stdout(contains("Accepted Baseline valid"));
    draft(&clean)
        .args(["maintenance", "remove-project"])
        .assert()
        .success()
        .stdout(contains("Draft closed"));
    assert!(!clean.join(".draft").exists());

    // An open Change is unfinished work: it stays open until a promotion
    // carries one of its revisions onto the Baseline. Removing the project
    // would destroy it, so the refusal is the whole point of the command
    // having a `--force` at all.
    std::fs::write(dirty.join("app.txt"), "v1\n").unwrap();
    draft(&dirty).args(["init"]).assert().success();
    let opened = draft(&dirty)
        .args(["change", "new", "pending", "--scope", "app.txt", "--json"])
        .output()
        .unwrap();
    let change: serde_json::Value = serde_json::from_slice(&opened.stdout).unwrap();
    let change_id = change["id"].as_str().unwrap().to_string();
    std::fs::write(dirty.join("app.txt"), "v2\n").unwrap();
    draft(&dirty)
        .args(["change", "revision", "seal", &change_id])
        .assert()
        .success();

    draft(&dirty)
        .args(["maintenance", "remove-project"])
        .assert()
        .failure()
        .stderr(contains("open change"));
    assert!(dirty.join(".draft").exists());
    draft(&dirty)
        .args(["maintenance", "remove-project", "--force"])
        .assert()
        .success();
    assert!(!dirty.join(".draft").exists());
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
        .stderr(contains("already exists"));
}

#[test]
fn activity_command_supports_pagination_and_rejects_retired_flags() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    a_promoted_revision(dir);

    // The promotion is on the record, and it is the newest thing there.
    draft(dir)
        .args(["activity", "list", "--limit", "5"])
        .assert()
        .success()
        .stdout(
            contains("PromotionCommitted")
                .and(contains("BaselinePromoted"))
                .and(contains("ChangeCompleted"))
                .and(contains("ReceiptIssued"))
                .and(contains("PromotionFinalized")),
        );
    // Newest first, so one event is the last thing the promotion recorded.
    draft(dir)
        .args(["activity", "list", "--limit", "1"])
        .assert()
        .success()
        .stdout(contains("PromotionFinalized"));
    // And the second page of one is the one before it.
    draft(dir)
        .args(["activity", "list", "--page", "1", "--limit", "1"])
        .assert()
        .success()
        .stdout(contains("ReceiptIssued"));

    let raw = draft(dir)
        .args(["activity", "list", "--raw", "--limit", "1"])
        .output()
        .unwrap();
    assert!(
        raw.status.success(),
        "{}",
        String::from_utf8_lossy(&raw.stderr)
    );
    let raw_stdout = String::from_utf8(raw.stdout).unwrap();
    let raw_event: serde_json::Value = serde_json::from_str(raw_stdout.trim()).unwrap();
    assert_eq!(raw_event["kind"], "PromotionFinalized");

    // One spelling per command, and one spelling per flag.
    draft(dir).args(["log"]).assert().failure();
    draft(dir).args(["events"]).assert().failure();
    draft(dir).args(["event"]).assert().failure();
    for retired in [
        vec!["activity", "list", "-p", "1"],
        vec!["activity", "list", "--top"],
        vec!["activity", "list", "--bottom"],
        vec!["activity", "list", "-f", "checkpoint"],
        vec!["activity", "list", "--filter", "checkpoint"],
        vec!["activity", "list", "--verify-chain"],
    ] {
        draft(dir).args(&retired).assert().failure();
    }
}

#[test]
fn public_docs_do_not_advertise_retired_command_spellings() {
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

    // Draft has one way to spell each command. A retired spelling left in the
    // docs is worse than a missing one: it reads as an alternative that works.
    const RETIRED: &[&str] = &[
        "draft events",
        "draft log",
        "draft create",
        "draft pack",
        "draft list",
        "draft intents",
        "draft presentation",
        "draft verify",
        "draft risk",
        "draft review",
        "draft approve",
        "draft reject",
        "draft waive",
        "draft checkpoint",
        "draft observation",
        "draft rollback",
        "draft event",
        "draft tool",
        "draft hook",
        "draft ignore",
        "draft receipt",
        "draft storage",
        "draft gc",
        "draft prune",
        "draft service",
        "draft close",
        "draft compose",
        "draft disperse",
        "draft compare",
        "draft candidate",
    ];

    let mut violations = Vec::new();
    for file in files {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (number, line) in content.lines().enumerate() {
            let lower = line.to_lowercase();
            // A line that says a spelling is gone is documentation, not an
            // advertisement for it.
            if lower.contains("no `draft")
                || lower.contains("retired")
                || lower.contains("unsupported")
                || lower.contains("rejects `draft")
            {
                continue;
            }
            for retired in RETIRED {
                // Case-sensitive: commands are lowercase, and prose says
                // "Draft storage" or "Does Draft Create ...", which are not
                // command spellings at all.
                let Some(at) = line.find(retired) else {
                    continue;
                };
                // `draft event` must not match inside `draft events`, and
                // `draft pack` must not match inside `draft packs`.
                let next = line[at + retired.len()..].chars().next();
                if next.is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                    continue;
                }
                violations.push(format!(
                    "{}:{} advertises `{retired}`",
                    file.display(),
                    number + 1
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "retired command spellings remain in public docs:\n{}",
        violations.join("\n")
    );
}

#[test]
fn reopening_a_change_converges_and_the_retired_vocabulary_is_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["init"]).assert().success();

    // A Change's identity comes from its intent and the Baseline it is worked
    // from, so asking for the same one twice is not an error to report — it is
    // the same Change, and a retried command converges on it.
    let open = || {
        let out = draft(dir)
            .args(["change", "new", "unique", "--scope", "app.txt", "--json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let change: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        change["id"].as_str().unwrap().to_string()
    };
    assert_eq!(open(), open());

    // Draft has one way to spell each command. The retired vocabulary is
    // removed, not kept as an alias — including everything the Change Graph
    // replaced, which is the whole submit-era surface.
    for retired in [
        vec!["pack"],
        vec!["pack", "create", "old-form"],
        vec!["pack", "list"],
        vec!["create", "old-form"],
        vec!["list"],
        vec!["verify"],
        vec!["risk"],
        vec!["approve"],
        vec!["reject"],
        vec!["waive"],
        vec!["submit"],
        vec!["compose"],
        vec!["disperse"],
        vec!["review"],
        vec!["dispose"],
        vec!["checkpoint", "old-form"],
        vec!["rollback", "rcp_x"],
        vec!["storage", "compact"],
        vec!["service", "status"],
    ] {
        draft(dir)
            .args(&retired)
            .assert()
            .failure()
            .stderr(contains("unrecognized subcommand"));
    }

    // And the commands that replaced them are real, so this test cannot pass
    // by everything having been deleted.
    for live in [
        vec!["change", "new", "--help"],
        vec!["change", "evidence", "run", "--help"],
        vec!["change", "gates", "evaluate", "--help"],
        vec!["change", "decide", "--help"],
        // `pack --export/--import` became top-level commands, so these are the
        // replacements rather than retired spellings.
        vec!["export", "--help"],
        vec!["import", "--help"],
        vec!["promote", "--help"],
        vec!["baseline", "receipts", "--help"],
    ] {
        draft(dir).args(&live).assert().success();
    }
}

#[test]
fn retired_workspace_profile_fails_closed_but_doctor_and_close_recover() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join(".draft/identity.json"), [0xff, 0xfe, 0xfd]).unwrap();
    draft(dir)
        .args(["status"])
        .assert()
        .failure()
        .stderr(contains("unsupported pre-release profile state"));
    draft(dir)
        .args(["doctor"])
        .assert()
        .failure()
        .stdout(contains("identity.json"));
    draft(dir)
        .args(["maintenance", "remove-project", "--force"])
        .assert()
        .success();
    assert!(!dir.join(".draft").exists());
}

#[test]
fn retired_config_namespace_and_xdg_profile_fail_without_value_parsing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(
        dir.join(".draft/config.toml"),
        "schema_version = 1\n[identity]\nemail = [not valid TOML\n",
    )
    .unwrap();
    draft(dir)
        .args(["status"])
        .assert()
        .failure()
        .stderr(contains("unsupported pre-release profile"));
    draft(dir)
        .args(["doctor"])
        .assert()
        .failure()
        .stdout(contains("unsupported pre-release profile"));
    draft(dir)
        .args(["maintenance", "remove-project", "--force"])
        .assert()
        .success();

    let second = tempfile::tempdir().unwrap();
    let dir = second.path();
    draft(dir).args(["init"]).assert().success();
    let xdg = dir.join("xdg");
    std::fs::create_dir_all(xdg.join("draft")).unwrap();
    std::fs::write(xdg.join("draft/identity.toml"), [0xff, 0xfe, 0xfd]).unwrap();
    draft(dir)
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["status"])
        .assert()
        .failure()
        .stderr(contains("identity.toml"));
    draft(dir)
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["doctor"])
        .assert()
        .failure()
        .stdout(contains("identity.toml"));
}

#[test]
fn user_profile_rejects_empty_values_and_unset_represents_absence() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    for key in ["user.name", "user.email"] {
        draft(dir)
            .args(["config", "set", key, "   "])
            .assert()
            .failure()
            .stderr(contains("cannot be empty"));
    }
    draft(dir)
        .args(["config", "set", "user.email", "contact label"])
        .assert()
        .success();
    draft(dir)
        .args(["config", "unset", "user.email"])
        .assert()
        .success();
    let config = std::fs::read_to_string(dir.join(".draft/config.toml")).unwrap();
    assert!(!config.contains("contact label"));
    assert!(!config.contains("email"));
}

#[test]
fn storage_doctor_checks_rebuildable_state() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    let out = draft(dir)
        .args(["change", "checkpoint", "base", "--json"])
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
        .args(["maintenance", "compact"])
        .assert()
        .success()
        .stdout(contains("Storage compact complete"));
    assert!(dir.join(".draft/objects/segments/index.json").exists());
    // Default output is human-readable; machine assertions use --json.
    draft(dir)
        .args(["doctor", "storage"])
        .assert()
        .success()
        .stdout(contains("Storage doctor complete"));
    draft(dir)
        .args(["doctor", "storage", "--json"])
        .assert()
        .success()
        .stdout(contains("\"objects_ok\": true"));
}

#[test]
fn storage_doctor_reports_a_receipt_whose_signature_no_longer_verifies() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    // A receipt attests a Promotion, so one has to happen before there is a
    // signature to tamper with.
    a_promoted_revision(dir);

    let envelopes = dir.join(".draft/receipts/envelopes");
    let path = std::fs::read_dir(&envelopes)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .expect("a promotion issues one signed receipt");
    let mut envelope: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    envelope["signature"] = serde_json::json!("tampered");
    std::fs::write(&path, serde_json::to_string_pretty(&envelope).unwrap()).unwrap();

    // Reported rather than repaired, and named. Changing the signature changes
    // the receipt's canonical bytes, so the create-once digest binding catches
    // the substitution before the signature is even checked — which is the
    // stronger detection, and still a diagnosis rather than a repair. Doctor
    // must be able to say *which* receipt, because "one of them is damaged" is
    // not a finding anybody can act on.
    let report = draft(dir)
        .args(["doctor", "storage", "--json"])
        .output()
        .unwrap();
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(report["receipts_ok"], serde_json::json!(false));
    let errors = report["receipt_errors"].as_array().unwrap();
    assert!(
        errors.iter().any(|error| error
            .as_str()
            .is_some_and(|text| text.contains("rcp_") && text.contains("structure"))),
        "the damaged receipt is named: {errors:?}"
    );
}

#[test]
fn a_tampered_activity_chain_is_detected_and_named() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).args(["init"]).assert().success();
    draft(dir)
        .args(["change", "checkpoint", "base"])
        .assert()
        .success();

    draft(dir)
        .args(["activity", "verify"])
        .assert()
        .success()
        .stdout(contains("Activity chain verified"));

    // Each entry hashes the one before it, so editing any of them breaks every
    // link after. The chain is checked on demand rather than on every command:
    // a promotion's authority comes from the evidence, gate and decision it
    // cites — each verified by digest as it is loaded — not from the audit
    // history, and refusing all further work because an old audit line was
    // edited would turn a recoverable project into a dead one.
    // Same length, so the physical frame stays well-formed and what is caught
    // is the record's own integrity rather than a torn tail.
    let activity_log = dir.join(".draft/events/events.log");
    let bytes = std::fs::read(&activity_log).unwrap();
    let text = String::from_utf8_lossy(&bytes).replacen("CheckpointCreated", "TamperedEventKi", 1);
    std::fs::write(&activity_log, text.as_bytes()).unwrap();

    // Named, and never repaired. A physically complete frame that fails its
    // checksum is a record that *was* written and has since been changed, so
    // it is refused rather than truncated away — "something is wrong
    // somewhere" is not a finding a person can act on, and silently rewriting
    // history is worse than refusing.
    draft(dir)
        .args(["activity", "verify"])
        .assert()
        .failure()
        .stderr(contains("checksum"));
}

#[test]
fn receipt_show_human_output_includes_proof_and_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // A receipt attests a Promotion, a publication outcome, or a resolution of
    // one — nothing else. Local acts that are already immutable facts in their
    // own stores are deliberately not receipted, so a promotion is what this
    // has to run to have a receipt to show at all.
    a_promoted_revision(dir);

    let receipts = draft(dir)
        .args(["baseline", "receipts", "--json"])
        .output()
        .unwrap();
    let receipts: serde_json::Value = serde_json::from_slice(&receipts.stdout).unwrap();
    let receipt_id = receipts
        .as_array()
        .unwrap()
        .first()
        .expect("a promotion issues exactly one receipt")["payload"]["receipt_id"]
        .as_str()
        .unwrap()
        .to_string();

    draft(dir)
        .args(["baseline", "receipts", &receipt_id])
        .assert()
        .success()
        .stdout(
            contains("Proof:")
                .and(contains("a Promotion accepted a Baseline"))
                .and(contains("Receipt IDs:"))
                .and(contains(&receipt_id)),
        );
}

#[test]
fn obsolete_command_level_tui_flags_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    // Against commands that exist, so this proves the flag is refused rather
    // than the command being absent — which is what it would have proved once
    // the commands it used to name were removed.
    for arguments in [
        vec!["change", "list", "--tui"],
        vec!["change", "new", "intent", "--tui"],
        vec!["change", "gates", "evaluate", "rev_x", "--tui"],
        vec!["promote", "chg_x", "rev_x", "--tui"],
        vec!["activity", "list", "--tui"],
    ] {
        draft(dir)
            .args(arguments)
            .assert()
            .failure()
            .stderr(contains("--tui"));
    }
}

#[test]
fn authorization_failures_use_documented_exit_codes() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (change_id, revision_id) = a_change_in_progress(dir);

    // Nothing has been decided, so promotion has no authority to act on. The
    // review code, because that is what is missing — not a generic failure.
    let promote = draft(dir)
        .args(["promote", &change_id, &revision_id])
        .output()
        .unwrap();
    assert_eq!(promote.status.code(), Some(7));

    // A gate over a revision nobody verified or assessed is unsatisfied, and a
    // decision citing it cannot approve. Same code: the missing thing is still
    // the human authority a promotion needs.
    let gate = draft(dir)
        .args(["change", "gates", "evaluate", &revision_id, "--json"])
        .output()
        .unwrap();
    let gate: serde_json::Value = serde_json::from_slice(&gate.stdout).unwrap();
    let gate_id = gate["id"].as_str().unwrap().to_string();
    assert!(
        gate["conditions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|condition| condition["id"] == "draft.gate/assessed"
                && condition["satisfied"] == false),
        "nobody assessed this revision, and that is its own refusal: {gate:?}"
    );
    let approve = draft(dir)
        .args([
            "change",
            "decide",
            &revision_id,
            "--approve",
            "--gate",
            &gate_id,
        ])
        .output()
        .unwrap();
    assert_eq!(approve.status.code(), Some(7));
}

#[test]
fn the_activity_event_that_recorded_a_checkpoint_is_a_recovery_target() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    let checkpoint = draft(dir)
        .args(["change", "checkpoint", "base", "--json"])
        .output()
        .unwrap();
    let checkpoint: serde_json::Value = serde_json::from_slice(&checkpoint.stdout).unwrap();
    let checkpoint_id = checkpoint["snapshot_id"].as_str().unwrap().to_string();
    let checkpoint_event = checkpoint["event_id"].as_str().unwrap().to_string();

    // The event names what it recorded, so a person can recover by pointing at
    // the record of the action rather than having to know a snapshot id. No
    // signature is involved: the Activity chain is hash-linked and verified,
    // and v1 receipts attest promotions and publications rather than local
    // acts.
    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    draft(dir)
        .args(["recover", "run", &checkpoint_event])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(dir.join("app.txt")).unwrap(),
        "v1\n"
    );

    // And the snapshot itself is still a target: two spellings of one state.
    std::fs::write(dir.join("app.txt"), "v3\n").unwrap();
    draft(dir)
        .args(["recover", "run", &checkpoint_id])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(dir.join("app.txt")).unwrap(),
        "v1\n"
    );
}

#[test]
fn an_installed_extension_changes_both_the_outcome_and_the_recorded_configuration() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/auth.rs"), "fn old() {}\n").unwrap();
    draft(dir).args(["init"]).assert().success();

    let opened = draft(dir)
        .args([
            "change",
            "new",
            "sec-fix",
            "--scope",
            "src/auth.rs",
            "--json",
        ])
        .output()
        .unwrap();
    let change: serde_json::Value = serde_json::from_slice(&opened.stdout).unwrap();
    let change_id = change["id"].as_str().unwrap().to_string();
    std::fs::write(dir.join("src/auth.rs"), "pub fn validate_token() {}\n").unwrap();
    let sealed = draft(dir)
        .args(["change", "revision", "seal", &change_id, "--json"])
        .output()
        .unwrap();
    let revision: serde_json::Value = serde_json::from_slice(&sealed.stdout).unwrap();
    let revision_id = revision["id"].as_str().unwrap().to_string();

    // Before: nothing is installed, so nothing could be asked. `unavailable`
    // is not a pass.
    let before = draft(dir)
        .args(["change", "evidence", "run", &revision_id, "--json"])
        .output()
        .unwrap();
    assert!(
        before.status.success(),
        "{}",
        String::from_utf8_lossy(&before.stderr)
    );
    let before: serde_json::Value = serde_json::from_slice(&before.stdout).unwrap();
    assert_eq!(before["outcome"], "unavailable");

    let _extension = install_language_extension(dir);

    // After: the contributed check ran, which is precisely what installing the
    // package buys.
    let after = draft(dir)
        .args(["change", "evidence", "run", &revision_id, "--json"])
        .output()
        .unwrap();
    assert!(
        after.status.success(),
        "{}",
        String::from_utf8_lossy(&after.stderr)
    );
    let after: serde_json::Value = serde_json::from_slice(&after.stdout).unwrap();
    assert_ne!(after["outcome"], "unavailable");

    // Two verifications of one revision are two facts, not one rewritten.
    // Their identities differ because their content does, and the recorded
    // configuration moved with the check set — an evidence digest that ignored
    // the contributed checks would claim the rules had not changed.
    assert_ne!(before["id"], after["id"]);
    assert_ne!(
        before["configuration"], after["configuration"],
        "the checks that ran are part of the configuration the evidence names"
    );
    assert_eq!(before["revision"], after["revision"]);
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

    // The banned sense is a *remote action vocabulary*: Draft landing work
    // somewhere else, or naming an ecosystem's own "provider" for a domain it
    // must not know about.
    //
    // `provider` itself is no longer on this list, and the reconciliation is
    // the same one `scripts/check-extension-architecture.sh` already made in
    // code: a ProviderBinding — a project's configured attachment to an
    // external system — is first-class, domain-neutral architecture, and a
    // filesystem, a ticket tracker and a deploy target are all providers in
    // that sense. Documentation that could not name a concept Draft is built
    // on would be worse than the drift the ban exists to prevent. What the
    // remaining terms still catch is the thing that was actually wrong:
    // treating an external system as the place work becomes real.
    let retired_terms = [
        "target.local",
        "target.remote",
        "remote target",
        "remote targets",
        "landing",
        "commit-native",
        "target_local_command_hash",
        "external command result",
        "[target]",
        "target-local",
        "remote-target",
        "hooks.remote",
        "hooks.submit",
        "draft push",
        "draft pr ",
        "draft submit",
        // retired-architecture-ok: naming the retired vocabulary is the point.
        "stable head",
        "pack_id",
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
fn remote_push_commands_are_not_present() {
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
fn project_only_commands_use_project_scope_error_outside_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    draft(tmp.path())
        .args(["task", "list"])
        .assert()
        .failure()
        .stderr(contains("PROJECT_SCOPE_REQUIRED"))
        .stderr(contains("draft init"));
}

/// Find canonical receipt ids in `.draft/receipts` for a given event type.
/// The Activity events of one kind, newest last.
///
/// Reads through the CLI rather than the store: what a person can name as a
/// recovery target is exactly what `draft activity list --json` shows them.
fn activity_events_of_kind(dir: &std::path::Path, kind: &str) -> Vec<String> {
    let out = draft(dir)
        .args(["activity", "list", "--json"])
        .output()
        .unwrap();
    let events: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    events
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["kind"] == kind)
        .filter_map(|event| event["event_id"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn recovery_accepts_the_activity_event_that_recorded_a_checkpoint() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();
    std::fs::write(dir.join("app.txt"), "original\n").unwrap();
    draft(dir)
        .args(["change", "checkpoint", "base"])
        .assert()
        .success();

    let checkpoints = activity_events_of_kind(dir, "CheckpointCreated");
    assert!(
        !checkpoints.is_empty(),
        "a checkpoint must record a CheckpointCreated Activity event"
    );

    std::fs::write(dir.join("app.txt"), "mutated\n").unwrap();
    draft(dir)
        .args(["recover", "run", &checkpoints[0]])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(dir.join("app.txt")).unwrap(),
        "original\n"
    );
}

#[test]
fn doctor_global_omits_retired_protocol_statuses() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init", "--global"]).assert().success();
    let out = draft(dir)
        .args(["doctor", "--global", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let checks = report["global"]["checks"].as_array().unwrap();
    assert!(checks.iter().all(|check| !check["name"]
        .as_str()
        .unwrap_or_default()
        .starts_with("extension:")));
}

// ---- Canonical pipeline and rollback-guidance contracts -------------------

#[test]
fn event_default_output_is_human_readable_not_json() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    draft(dir).args(["init"]).assert().success();

    let out = draft(dir).args(["activity", "list"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        serde_json::from_str::<serde_json::Value>(stdout.trim()).is_err(),
        "default `draft event` output must not be JSON: {stdout}"
    );

    let raw = draft(dir)
        .args(["activity", "list", "--raw"])
        .output()
        .unwrap();
    assert!(raw.status.success());
    let raw_stdout = String::from_utf8_lossy(&raw.stdout);
    let lines: Vec<&str> = raw_stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert!(!lines.is_empty(), "`draft event --raw` must emit events");
    for line in lines {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "`draft event --raw` must emit one valid JSON event per line: {line}"
        );
    }
}

// ---- Project lifecycle and the interference question ---------------------

/// Closing is a lifecycle transition, and the history survives it.
#[test]
fn closing_a_project_keeps_everything_it_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    draft(dir).arg("init").assert().success();

    let before = draft(dir)
        .args(["project", "control", "--json"])
        .output()
        .unwrap();
    assert!(before.status.success());
    let before: serde_json::Value = serde_json::from_slice(&before.stdout).unwrap();
    assert_eq!(before["project_lifecycle"], "active");

    draft(dir)
        .args(["project", "close", "--json"])
        .assert()
        .success();

    let after = draft(dir)
        .args(["project", "control", "--json"])
        .output()
        .unwrap();
    let after: serde_json::Value = serde_json::from_slice(&after.stdout).unwrap();
    assert_eq!(after["project_lifecycle"], "closed");
    // The same accepted Baseline, one generation on. Closing decides what the
    // project accepts *next*; it says nothing about what it already accepted.
    assert_eq!(after["accepted_baseline"], before["accepted_baseline"]);
    assert_eq!(
        after["generation"].as_u64().unwrap(),
        before["generation"].as_u64().unwrap() + 1
    );

    // Recorded as a fact, and refused a second time rather than repeated.
    draft(dir)
        .args(["activity", "list", "--raw"])
        .assert()
        .success()
        .stdout(contains("ProjectClosed"));
    draft(dir)
        .args(["project", "close"])
        .assert()
        .failure()
        .stderr(contains("already closed"));
}

/// Interference is what two Changes actually touched, not what they may touch.
#[test]
fn comparing_two_changes_reports_only_what_they_share() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("app.txt"), "v1\n").unwrap();
    std::fs::write(dir.join("docs.txt"), "d1\n").unwrap();
    draft(dir).arg("init").assert().success();

    let json = |args: &[&str]| -> serde_json::Value {
        let out = draft(dir).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let id_of = |value: &serde_json::Value| -> String {
        value["change"]["id"]
            .as_str()
            .or_else(|| value["id"].as_str())
            .expect("a Change id")
            .to_string()
    };

    let one = id_of(&json(&[
        "change",
        "new",
        "edit the app",
        "--scope",
        "app.txt",
        "--json",
    ]));
    std::fs::write(dir.join("app.txt"), "v2\n").unwrap();
    json(&["change", "revision", "seal", &one, "--json"]);

    let two = id_of(&json(&[
        "change",
        "new",
        "edit the docs",
        "--scope",
        "docs.txt",
        "--json",
    ]));
    std::fs::write(dir.join("docs.txt"), "d2\n").unwrap();
    json(&["change", "revision", "seal", &two, "--json"]);

    // Disjoint touched sets from the same Baseline: nothing to report, and
    // `composable` says so directly rather than by an empty list.
    let report = json(&["change", "compare", &one, &two, "--json"]);
    assert_eq!(report["shared_resources"].as_array().unwrap().len(), 0);
    assert_eq!(report["relation"], "independent");
    assert_eq!(report["composable"], true);

    // A third Change over a resource the first already touched interferes,
    // and the resource is named rather than merely counted.
    let three = id_of(&json(&[
        "change",
        "new",
        "edit the app again",
        "--scope",
        "app.txt",
        "--json",
    ]));
    std::fs::write(dir.join("app.txt"), "v3\n").unwrap();
    json(&["change", "revision", "seal", &three, "--json"]);

    let clash = json(&["change", "compare", &one, &three, "--json"]);
    assert_eq!(clash["relation"], "conflicting");
    assert_eq!(clash["composable"], false);
    assert_eq!(clash["shared_resources"].as_array().unwrap().len(), 1);

    // A Change never interferes with itself; asking is a mistake, not a verdict.
    draft(dir)
        .args(["change", "compare", &one, &one])
        .assert()
        .failure()
        .stderr(contains("itself"));

    // And a Change that has sealed nothing has no answer to give, rather than
    // a reassuring empty one.
    let unsealed = id_of(&json(&[
        "change",
        "new",
        "not started",
        "--scope",
        "docs.txt",
        "--json",
    ]));
    draft(dir)
        .args(["change", "compare", &one, &unsealed])
        .assert()
        .failure()
        .stderr(contains("sealed no revision"));
}
