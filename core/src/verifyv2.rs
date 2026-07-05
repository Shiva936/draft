//! Evidence-based test and fuzz selection (PRD §9.18, TDD §36–37).
//!
//! Verification flows: changed files → changed symbols → related tests → fuzz
//! targets → a verification plan. The plan and its results are persisted as
//! `verify.json` evidence (what was selected, why, what ran, and a result hash),
//! so a review can see the rationale, not just a pass/fail. `--full` widens
//! selection to the whole configured suite; `--fuzz` adds fuzz targets.

use crate::hashing;
use serde::{Deserialize, Serialize};

/// A selected test with the reason it was chosen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectedTest {
    pub name: String,
    pub command: String,
    pub reason: String,
}

/// A selected fuzz target with the reason it was chosen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectedFuzzTarget {
    pub name: String,
    pub command: String,
    pub reason: String,
}

/// The verification evidence persisted to `verify.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyEvidence {
    pub selected_tests: Vec<SelectedTest>,
    pub selected_fuzz_targets: Vec<SelectedFuzzTarget>,
    pub selection_reason: String,
    pub coverage_basis: String,
    pub commands_run: Vec<String>,
    pub exit_codes: Vec<i32>,
    pub duration_ms: u64,
    pub stdout_digest: String,
    pub stderr_digest: String,
    pub result_hash: String,
}

impl VerifyEvidence {
    /// Recompute the result hash over the evidence (excluding the hash itself).
    pub fn compute_result_hash(&self) -> String {
        let content = serde_json::json!({
            "selected_tests": self.selected_tests,
            "selected_fuzz_targets": self.selected_fuzz_targets,
            "selection_reason": self.selection_reason,
            "coverage_basis": self.coverage_basis,
            "commands_run": self.commands_run,
            "exit_codes": self.exit_codes,
            "stdout_digest": self.stdout_digest,
            "stderr_digest": self.stderr_digest,
        });
        hashing::sha256_hex(hashing::canonical_json(&content).as_bytes())
    }

    /// True if every executed command succeeded (exit 0), or nothing ran.
    pub fn passed(&self) -> bool {
        self.exit_codes.iter().all(|&c| c == 0)
    }
}

/// Inputs to selection.
#[derive(Debug, Clone)]
pub struct SelectionInput {
    pub changed_files: Vec<String>,
    pub changed_symbols: Vec<String>,
    /// Test files that reference the changed symbols (from the LSIF index).
    pub test_files: Vec<String>,
    /// Fuzz target names available in the workspace.
    pub fuzz_targets: Vec<String>,
    pub full: bool,
    pub fuzz: bool,
}

/// Build the verification plan (selection only; no execution yet).
pub fn plan(input: &SelectionInput) -> VerifyEvidence {
    let mut selected_tests = Vec::new();
    for file in &input.test_files {
        selected_tests.push(SelectedTest {
            name: file.clone(),
            command: infer_test_command(file),
            reason: "references changed symbol(s)".to_string(),
        });
    }
    let coverage_basis = if input.full {
        selected_tests.push(SelectedTest {
            name: "full-suite".to_string(),
            command: full_suite_command(&input.changed_files),
            reason: "--full requested".to_string(),
        });
        "full configured suite".to_string()
    } else if selected_tests.is_empty() {
        "no symbol-linked tests found; relying on policy checks".to_string()
    } else {
        format!(
            "{} symbol-linked test file(s) selected from {} changed symbol(s)",
            selected_tests.len(),
            input.changed_symbols.len()
        )
    };

    let mut selected_fuzz_targets = Vec::new();
    if input.fuzz {
        for target in &input.fuzz_targets {
            selected_fuzz_targets.push(SelectedFuzzTarget {
                name: target.clone(),
                command: format!("cargo fuzz run {target}"),
                reason: "--fuzz requested for changed parser-adjacent code".to_string(),
            });
        }
    }

    let selection_reason = format!(
        "{} changed file(s) → {} changed symbol(s) → {} test(s), {} fuzz target(s)",
        input.changed_files.len(),
        input.changed_symbols.len(),
        selected_tests.len(),
        selected_fuzz_targets.len()
    );

    let mut evidence = VerifyEvidence {
        selected_tests,
        selected_fuzz_targets,
        selection_reason,
        coverage_basis,
        commands_run: Vec::new(),
        exit_codes: Vec::new(),
        duration_ms: 0,
        stdout_digest: hashing::sha256_hex(b""),
        stderr_digest: hashing::sha256_hex(b""),
        result_hash: String::new(),
    };
    evidence.result_hash = evidence.compute_result_hash();
    evidence
}

/// Run `commands` in `cwd`, folding results into `evidence` and refreshing the
/// result hash. Commands are executed via the platform shell.
pub fn execute(evidence: &mut VerifyEvidence, commands: &[String], cwd: &std::path::Path) {
    use std::time::Instant;
    let start = Instant::now();
    let mut all_stdout = Vec::new();
    let mut all_stderr = Vec::new();
    for cmd in commands {
        let output = run_shell(cmd, cwd);
        match output {
            Ok(out) => {
                evidence.exit_codes.push(out.status.code().unwrap_or(-1));
                all_stdout.extend_from_slice(&out.stdout);
                all_stderr.extend_from_slice(&out.stderr);
            }
            Err(e) => {
                evidence.exit_codes.push(-1);
                all_stderr.extend_from_slice(e.to_string().as_bytes());
            }
        }
        evidence.commands_run.push(cmd.clone());
    }
    evidence.duration_ms = start.elapsed().as_millis() as u64;
    evidence.stdout_digest = hashing::sha256_hex(&all_stdout);
    evidence.stderr_digest = hashing::sha256_hex(&all_stderr);
    evidence.result_hash = evidence.compute_result_hash();
}

fn run_shell(cmd: &str, cwd: &std::path::Path) -> std::io::Result<std::process::Output> {
    #[cfg(windows)]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(cmd);
        c
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    };
    command.current_dir(cwd).output()
}

fn infer_test_command(file: &str) -> String {
    match file.rsplit('.').next().unwrap_or("") {
        "rs" => "cargo test".to_string(),
        "js" | "jsx" | "ts" | "tsx" | "mjs" => "npm test".to_string(),
        "py" => format!("pytest {file}"),
        "go" => "go test ./...".to_string(),
        _ => format!("run tests for {file}"),
    }
}

fn full_suite_command(changed: &[String]) -> String {
    let any = |ext: &str| changed.iter().any(|f| f.ends_with(ext));
    if any(".rs") {
        "cargo test --workspace".to_string()
    } else if any(".py") {
        "pytest".to_string()
    } else if any(".go") {
        "go test ./...".to_string()
    } else if any(".ts") || any(".js") {
        "npm test".to_string()
    } else {
        "run full test suite".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input() -> SelectionInput {
        SelectionInput {
            changed_files: vec!["src/auth.rs".into()],
            changed_symbols: vec!["validate".into()],
            test_files: vec!["tests/auth_test.rs".into()],
            fuzz_targets: vec!["auth_token_parser".into()],
            full: false,
            fuzz: false,
        }
    }

    #[test]
    fn selects_symbol_linked_tests_with_reasons() {
        let e = plan(&base_input());
        assert_eq!(e.selected_tests.len(), 1);
        assert_eq!(e.selected_tests[0].command, "cargo test");
        assert!(e.selected_tests[0].reason.contains("changed symbol"));
        assert!(e.selected_fuzz_targets.is_empty());
        assert!(!e.result_hash.is_empty());
    }

    #[test]
    fn full_and_fuzz_widen_selection() {
        let mut input = base_input();
        input.full = true;
        input.fuzz = true;
        let e = plan(&input);
        assert!(e.selected_tests.iter().any(|t| t.name == "full-suite"));
        assert_eq!(e.selected_fuzz_targets.len(), 1);
        assert!(e.coverage_basis.contains("full"));
    }

    #[test]
    fn no_tests_records_coverage_basis() {
        let mut input = base_input();
        input.test_files.clear();
        let e = plan(&input);
        assert!(e.selected_tests.is_empty());
        assert!(e.coverage_basis.contains("no symbol-linked tests"));
    }

    #[test]
    fn result_hash_is_stable() {
        let a = plan(&base_input());
        let b = plan(&base_input());
        assert_eq!(a.result_hash, b.result_hash);
        assert_eq!(a.result_hash, a.compute_result_hash());
    }

    #[test]
    fn execute_records_commands_and_exit_codes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut e = plan(&base_input());
        execute(&mut e, &["exit 0".to_string()], tmp.path());
        assert_eq!(e.exit_codes, vec![0]);
        assert!(e.passed());
        assert_eq!(e.commands_run.len(), 1);
        assert_eq!(e.result_hash, e.compute_result_hash());
    }
}
