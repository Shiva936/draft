//! `draft update` / `draft uninstall` end to end, hermetically.
//!
//! Every test builds its own installation under a temporary directory from the
//! freshly built binaries, with `HOME`, `XDG_RUNTIME_DIR` and
//! `DRAFT_GLOBAL_HOME` all inside it, so nothing here can reach the
//! developer's installation, daemon or global store. A guard refuses any root
//! outside the tempdir.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

struct Sandbox {
    _dir: tempfile::TempDir,
    base: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        for sub in ["home", "run", "pathbin", "work"] {
            std::fs::create_dir_all(base.join(sub)).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(base.join("run"), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        Self { _dir: dir, base }
    }

    fn root(&self) -> PathBuf {
        self.base.join("root")
    }

    fn command(&self, exe: &Path) -> Command {
        let mut command = Command::new(exe);
        command
            .current_dir(self.base.join("work"))
            .env("HOME", self.base.join("home"))
            .env("XDG_RUNTIME_DIR", self.base.join("run"))
            .env("DRAFT_GLOBAL_HOME", self.base.join("home/.draft"))
            .env_remove("DRAFT_INSTALL_ROOT");
        command
    }

    /// A package directory like the installer extracts: `bin/{draft,draftd}`.
    fn package(&self) -> PathBuf {
        let built = assert_cmd::cargo::cargo_bin("draft");
        let bin = self.base.join("package/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::copy(&built, bin.join("draft")).unwrap();
        let daemon = built.with_file_name("draftd");
        if daemon.exists() {
            std::fs::copy(daemon, bin.join("draftd")).unwrap();
        } else {
            std::fs::write(
                bin.join("draftd"),
                format!("#!/bin/sh\necho draftd {}\n", draft_core::DRAFT_VERSION),
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    bin.join("draftd"),
                    std::fs::Permissions::from_mode(0o755),
                )
                .unwrap();
            }
        }
        bin.join("draft")
    }

    fn install(&self) -> Output {
        let coordinator = self.package();
        let root = self.root();
        assert!(
            root.starts_with(&self.base),
            "the root must be inside the sandbox"
        );
        self.command(&coordinator)
            .args(["__installer", "install", "--install-root"])
            .arg(&root)
            .arg("--path-bin")
            .arg(self.base.join("pathbin"))
            .arg("--json")
            .output()
            .unwrap()
    }

    fn installed(&self) -> PathBuf {
        self.root().join("bin/draft")
    }
}

fn ok(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null)
}

fn receipt(sandbox: &Sandbox) -> serde_json::Value {
    serde_json::from_slice(
        &std::fs::read(sandbox.root().join(".draft-install/receipt.json")).unwrap(),
    )
    .unwrap()
}

#[cfg(unix)]
#[test]
fn install_dry_run_uninstall_and_reinstall_through_the_real_binaries() {
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.base.join("pathbin/python3"), b"unrelated").unwrap();
    let outcome = ok(&sandbox.install());
    assert_eq!(outcome["outcome"], "installed");
    let first = receipt(&sandbox);
    assert_eq!(first["install_generation"], 1);
    assert_eq!(first["windows_path"], serde_json::Value::Null);
    assert_eq!(first["path_links"].as_array().unwrap().len(), 2);
    // PATH carries symlinks into the root, never copies.
    let link = sandbox.base.join("pathbin/draft");
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::canonicalize(&link).unwrap(), sandbox.installed());
    let version = sandbox.command(&link).arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).contains(draft_core::DRAFT_VERSION));

    // Outside any project, a dry run prints the real plan and changes nothing.
    let plan = ok(&sandbox
        .command(&link)
        .args(["uninstall", "--dry-run", "--json"])
        .output()
        .unwrap());
    assert_eq!(plan["remove_executables"].as_array().unwrap().len(), 2);
    assert!(plan["remains"]
        .as_str()
        .unwrap()
        .ends_with("lifecycle.lock"));
    assert!(sandbox.installed().exists());

    // A project is never touched.
    let project = sandbox.base.join("work/project");
    std::fs::create_dir_all(project.join(".draft")).unwrap();
    std::fs::write(project.join(".draft/marker"), b"mine").unwrap();

    ok(&sandbox
        .command(&sandbox.installed())
        .args(["uninstall", "--json"])
        .output()
        .unwrap());
    // The staged helper finishes after the parent exits.
    let lifecycle = sandbox.root().join(".draft-install");
    let deadline = Instant::now() + Duration::from_secs(120);
    while lifecycle.join("operation.json").exists() || lifecycle.join("terminal-cleanup").exists() {
        assert!(
            Instant::now() < deadline,
            "the uninstall helper did not finish"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(!sandbox.installed().exists());
    assert!(std::fs::symlink_metadata(&link).is_err());
    assert!(!lifecycle.join("receipt.json").exists());
    let remaining: Vec<_> = std::fs::read_dir(&lifecycle)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(
        remaining,
        ["lifecycle.lock"],
        "only the inert skeleton remains"
    );
    assert!(sandbox.root().exists());
    assert_eq!(
        std::fs::read(sandbox.base.join("pathbin/python3")).unwrap(),
        b"unrelated"
    );
    assert_eq!(
        std::fs::read(project.join(".draft/marker")).unwrap(),
        b"mine"
    );

    // A reinstall reuses the skeleton and mints a new installation id.
    ok(&sandbox.install());
    assert_ne!(
        receipt(&sandbox)["installation_id"],
        first["installation_id"]
    );
}

#[cfg(unix)]
#[test]
fn a_second_install_over_a_managed_installation_changes_nothing() {
    let sandbox = Sandbox::new();
    ok(&sandbox.install());
    let before = std::fs::read(sandbox.root().join(".draft-install/receipt.json")).unwrap();
    let again = ok(&sandbox.install());
    assert_eq!(again["outcome"], "already_installed");
    assert_eq!(
        std::fs::read(sandbox.root().join(".draft-install/receipt.json")).unwrap(),
        before
    );
}

#[test]
fn meaningless_update_flags_are_rejected_before_any_network_call() {
    let sandbox = Sandbox::new();
    let draft = assert_cmd::cargo::cargo_bin("draft");
    for args in [
        vec!["update", "--version", "1.0.0", "--channel", "stable"],
        vec!["update", "--allow-downgrade"],
        vec![
            "update",
            "--check",
            "--version",
            "1.0.0",
            "--allow-downgrade",
        ],
    ] {
        let output = sandbox.command(&draft).args(&args).output().unwrap();
        assert!(!output.status.success(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cannot be used with")
                || String::from_utf8_lossy(&output.stderr).contains("required"),
            "{args:?}"
        );
    }
}

#[test]
fn a_development_build_refuses_to_manage_itself_outside_any_project() {
    let sandbox = Sandbox::new();
    let draft = assert_cmd::cargo::cargo_bin("draft");
    for args in [vec!["update", "--check"], vec!["uninstall", "--dry-run"]] {
        let output = sandbox.command(&draft).args(&args).output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("UNSUPPORTED_INSTALLATION_METHOD"),
            "{stderr}"
        );
        assert!(
            !stderr.contains("WORKSPACE_NOT_FOUND"),
            "no project discovery: {stderr}"
        );
    }
}

#[test]
fn a_copied_binary_is_unknown_with_reinstall_guidance() {
    let sandbox = Sandbox::new();
    let copy = sandbox.base.join("pathbin/draft");
    std::fs::copy(assert_cmd::cargo::cargo_bin("draft"), &copy).unwrap();
    let output = sandbox
        .command(&copy)
        .args(["uninstall", "--dry-run"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("UNKNOWN_INSTALLATION_PROVENANCE"));
}

#[test]
fn exactly_three_hidden_surfaces_exist_and_none_is_in_the_public_surface() {
    let golden = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/cli-surface.txt"
    ))
    .unwrap();
    for hidden in [
        "__lifecycle-helper",
        "release-trust-set",
        "__installer",
        "install-finalize",
    ] {
        assert!(
            !golden.contains(hidden),
            "{hidden} leaked into the public surface"
        );
    }
    assert!(golden.contains("\ndraft update\n") && golden.contains("\ndraft uninstall\n"));
    let sandbox = Sandbox::new();
    let draft = assert_cmd::cargo::cargo_bin("draft");
    let finalize = sandbox
        .command(&draft)
        .arg("install-finalize")
        .output()
        .unwrap();
    assert!(
        !finalize.status.success(),
        "no install-finalize mode exists"
    );
    // release-trust-set is read-only and opens no project.
    let trust = sandbox
        .command(&draft)
        .args(["release-trust-set", "--json"])
        .output()
        .unwrap();
    let ids: Vec<String> = serde_json::from_slice(&trust.stdout).unwrap();
    assert_eq!(
        ids,
        draft_core::installation::release::RELEASE_TRUSTED_KEYS
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>()
    );
    // A helper invocation with both modes, or a path, is malformed.
    let both = sandbox
        .command(&draft)
        .args([
            "__lifecycle-helper",
            "--installation-id",
            "ins_0123456789ab",
            "--operation-id",
            "ilo_0123456789ab",
            "--parent-pid",
            "1",
            "--bootstrap-recovery",
        ])
        .output()
        .unwrap();
    assert!(!both.status.success());
}

/// The installer's POSIX grammar check for `bootstrap.recovery` accepts
/// exactly what the Rust record renders and nothing else.
#[cfg(unix)]
#[test]
fn the_installer_bootstrap_grammar_matches_the_rust_record() {
    let script =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../install.sh")).unwrap();
    let start = script.find("h='[0-9a-f]'").unwrap();
    let end = script.find("route() {").unwrap();
    let functions = &script[start..end];
    let record = draft_core::installation::bootstrap::BootstrapRecord {
        installation_id: draft_core::installation::InstallationId::new("ins_0123456789ab"),
        operation_id: draft_core::installation::InstallationOperationId::new("ilo_0123456789ab"),
        helper: draft_core::installation::Identity {
            sha256: "a".repeat(64),
            size: 42,
        },
    }
    .render();
    let sandbox = Sandbox::new();
    let check = |text: &str| {
        let path = sandbox.base.join("record");
        std::fs::write(&path, text).unwrap();
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!("{functions}\nread_bootstrap \"$1\" && echo \"$boot_installation $boot_operation $boot_sha256 $boot_size\"", ))
            .arg("sh")
            .arg(&path)
            .output()
            .unwrap();
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    assert_eq!(
        check(&record).as_deref(),
        Some(format!("ins_0123456789ab ilo_0123456789ab {} 42", "a".repeat(64)).as_str())
    );
    assert!(
        check(&record.replace('\n', "\r\n")).is_some(),
        "CRLF is accepted"
    );
    for bad in [
        record.replace("kind uninstall", "kind update"),
        record.replace("installation ins_", "installation  ins_"),
        record.replace("ilo_0123456789ab", "ilo_0123456789AB"),
        record.replace("helper-size 42", "helper-size 4x"),
        format!("{record}phase resolved\n"),
        record.trim_end().to_string(),
        record.replacen("draft-lifecycle-bootstrap 1\n", "", 1),
        record.replace("kind uninstall", "kind unin\u{7}stall"),
    ] {
        assert!(check(&bad).is_none(), "{bad:?}");
    }
}
