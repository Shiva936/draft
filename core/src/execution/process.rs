//! Draft's runner for commands an extension declared.
//!
//! An extension contributes a program name and an argument vector. Draft — not
//! the extension — decides whether to run it, and runs it itself: the program
//! is spawned directly with its argv, so there is no shell, and therefore no
//! quoting, globbing, redirection or command chaining available to a package.
//!
//! The hardening mirrors what Draft already applies to candidate executions: a
//! cleared environment with an explicit allowlist, a working directory confined
//! to the workspace, a runtime limit enforced by killing the child, and
//! captured output that is bounded and redacted before it reaches evidence.

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Environment variables a declared command inherits when present. Everything
/// else is cleared, so a command cannot read the invoking shell's environment.
const BASE_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "TMPDIR",
    "TEMP",
    "SYSTEMROOT",
];

/// Largest captured stream Draft keeps from one command.
const MAX_CAPTURED_OUTPUT: usize = 1024 * 1024;

/// Limits Draft imposes on a declared command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessLimits {
    pub timeout_ms: Option<u64>,
    /// Additional environment variable names the command may inherit.
    pub env_allowlist: Vec<String>,
}

impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            // Declared commands are bounded by default: an extension that omits
            // a timeout does not get an unbounded one.
            timeout_ms: Some(300_000),
            env_allowlist: Vec::new(),
        }
    }
}

/// What running one declared command produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutcome {
    /// The command as rendered for evidence.
    pub display: String,
    /// `-1` when the process was killed or could not be started.
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u64,
    pub timed_out: bool,
}

impl ProcessOutcome {
    pub fn succeeded(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }
}

/// Substitute `{{name}}` placeholders strictly.
///
/// An unknown placeholder is an error rather than text passed through to the
/// program: a command that does not mean what it says must not run at all.
pub fn interpolate(argument: &str, values: &BTreeMap<String, String>) -> DraftResult<String> {
    let mut rendered = String::with_capacity(argument.len());
    let mut remainder = argument;
    while let Some(open) = remainder.find("{{") {
        rendered.push_str(&remainder[..open]);
        let after = &remainder[open + 2..];
        let Some(close) = after.find("}}") else {
            return Err(DraftError::invalid_config(format!(
                "declared argument '{argument}' has an unclosed placeholder"
            )));
        };
        let name = after[..close].trim();
        let Some(value) = values.get(name) else {
            return Err(DraftError::invalid_config(format!(
                "declared argument '{argument}' uses unknown placeholder '{name}'"
            )));
        };
        rendered.push_str(value);
        remainder = &after[close + 2..];
    }
    rendered.push_str(remainder);
    Ok(rendered)
}

/// Resolve a command's declared working directory inside `workspace_root`.
pub fn resolve_working_directory(
    workspace_root: &Path,
    declared: Option<&str>,
) -> DraftResult<PathBuf> {
    let Some(declared) = declared else {
        return Ok(workspace_root.to_path_buf());
    };
    let normalized = crate::support::pathguard::check_relative(declared).map_err(|error| {
        DraftError::new(
            DraftErrorKind::ProtectedResourceAccess,
            format!("unsafe declared working directory '{declared}': {error}"),
        )
    })?;
    let resolved =
        crate::support::pathguard::safe_join(workspace_root, &normalized).map_err(|error| {
            DraftError::new(DraftErrorKind::ProtectedResourceAccess, error.to_string())
        })?;
    if !resolved.is_dir() {
        return Err(DraftError::invalid_config(format!(
            "declared working directory '{declared}' is not a directory in the workspace"
        )));
    }
    Ok(resolved)
}

/// Run one declared command and capture what it produced.
///
/// Never returns `Err` for a command that merely failed: a non-zero exit is
/// evidence, not an error. `Err` is reserved for a command Draft refused to run
/// — an unsafe working directory, an unresolvable placeholder, or a program
/// that could not be started.
pub fn run(
    program: &str,
    args: &[String],
    working_directory: &Path,
    limits: &ProcessLimits,
) -> DraftResult<ProcessOutcome> {
    let display = std::iter::once(program.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");

    let mut environment: BTreeMap<String, String> = BTreeMap::new();
    for key in BASE_ENV.iter().chain(
        limits
            .env_allowlist
            .iter()
            .map(|key| key.as_str())
            .collect::<Vec<_>>()
            .iter(),
    ) {
        if let Ok(value) = std::env::var(key) {
            environment.insert((*key).to_string(), value);
        }
    }

    let started = Instant::now();
    let mut child = Command::new(program)
        .args(args)
        .current_dir(working_directory)
        .env_clear()
        .envs(&environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            DraftError::new(
                DraftErrorKind::CandidateNotConfigured,
                format!("failed to start declared command '{display}': {error}"),
            )
            .with_suggestion(format!("check that `{program}` is installed and on PATH"))
        })?;

    let deadline = limits
        .timeout_ms
        .map(|timeout| Instant::now() + Duration::from_millis(timeout));
    let mut timed_out = false;
    let output = loop {
        match child.try_wait() {
            Ok(Some(_)) => break child.wait_with_output(),
            Ok(None) => {
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break Ok(std::process::Output {
                        status: Default::default(),
                        stdout: Vec::new(),
                        stderr: format!("declared command '{display}' exceeded its time limit")
                            .into_bytes(),
                    });
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                return Err(DraftError::storage(format!(
                    "failed while waiting for declared command '{display}': {error}"
                )))
            }
        }
    };
    let output = output.map_err(|error| {
        DraftError::storage(format!(
            "failed to collect output of declared command '{display}': {error}"
        ))
    })?;

    Ok(ProcessOutcome {
        exit_code: if timed_out {
            -1
        } else {
            output.status.code().unwrap_or(-1)
        },
        stdout: bounded_and_redacted(&output.stdout),
        stderr: bounded_and_redacted(&output.stderr),
        duration_ms: started.elapsed().as_millis() as u64,
        timed_out,
        display,
    })
}

/// Cap and redact a captured stream before it can reach durable evidence.
fn bounded_and_redacted(bytes: &[u8]) -> Vec<u8> {
    let mut text = String::from_utf8_lossy(bytes).to_string();
    if text.len() > MAX_CAPTURED_OUTPUT {
        text.truncate(MAX_CAPTURED_OUTPUT);
        text.push_str("\n[Draft output truncated]\n");
    }
    crate::support::redaction::redact(&text).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn placeholders_resolve_or_refuse() {
        let bound = values(&[("file", "src/main.rs")]);
        assert_eq!(interpolate("{{file}}", &bound).unwrap(), "src/main.rs");
        assert_eq!(
            interpolate("--path={{file}}!", &bound).unwrap(),
            "--path=src/main.rs!"
        );
        assert_eq!(interpolate("plain", &bound).unwrap(), "plain");

        // An unknown or unclosed placeholder must never be passed through as
        // literal text to the program.
        assert!(interpolate("{{target}}", &bound).is_err());
        assert!(interpolate("{{file", &bound).is_err());
    }

    #[test]
    fn a_declared_working_directory_cannot_leave_the_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("sub")).unwrap();

        assert_eq!(
            resolve_working_directory(workspace.path(), None).unwrap(),
            workspace.path()
        );
        assert!(resolve_working_directory(workspace.path(), Some("sub")).is_ok());

        let escaped =
            resolve_working_directory(workspace.path(), Some("../elsewhere")).unwrap_err();
        assert_eq!(escaped.kind, DraftErrorKind::ProtectedResourceAccess);
        // A directory that simply is not there is a configuration mistake, not
        // an attempted escape.
        assert_eq!(
            resolve_working_directory(workspace.path(), Some("missing"))
                .unwrap_err()
                .kind,
            DraftErrorKind::InvalidConfig
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_command_is_evidence_and_not_an_error() {
        let workspace = tempfile::tempdir().unwrap();
        let outcome = run(
            "sh",
            &["-c".to_string(), "exit 3".to_string()],
            workspace.path(),
            &ProcessLimits::default(),
        )
        .unwrap();
        assert_eq!(outcome.exit_code, 3);
        assert!(!outcome.succeeded());
        assert_eq!(outcome.display, "sh -c exit 3");
    }

    #[cfg(unix)]
    #[test]
    fn arguments_reach_the_program_without_a_shell() {
        let workspace = tempfile::tempdir().unwrap();
        // If a shell were interpreting this, the `;` would start a second
        // command and the literal would not survive as one argument.
        let outcome = run(
            "echo",
            &["a; rm -rf /".to_string()],
            workspace.path(),
            &ProcessLimits::default(),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&outcome.stdout).trim(),
            "a; rm -rf /"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_command_that_overruns_is_killed() {
        let workspace = tempfile::tempdir().unwrap();
        let outcome = run(
            "sh",
            &["-c".to_string(), "sleep 30".to_string()],
            workspace.path(),
            &ProcessLimits {
                timeout_ms: Some(150),
                env_allowlist: Vec::new(),
            },
        )
        .unwrap();
        assert!(outcome.timed_out);
        assert_eq!(outcome.exit_code, -1);
        assert!(!outcome.succeeded());
    }

    #[cfg(unix)]
    #[test]
    fn the_environment_is_cleared_apart_from_the_allowlist() {
        let workspace = tempfile::tempdir().unwrap();
        std::env::set_var("DRAFT_TEST_SECRET_VALUE", "leaked");
        std::env::set_var("DRAFT_TEST_ALLOWED_VALUE", "permitted");

        let hidden = run(
            "sh",
            &[
                "-c".to_string(),
                "echo ${DRAFT_TEST_SECRET_VALUE:-absent}".to_string(),
            ],
            workspace.path(),
            &ProcessLimits::default(),
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&hidden.stdout).trim(), "absent");

        let allowed = run(
            "sh",
            &[
                "-c".to_string(),
                "echo ${DRAFT_TEST_ALLOWED_VALUE:-absent}".to_string(),
            ],
            workspace.path(),
            &ProcessLimits {
                timeout_ms: Some(30_000),
                env_allowlist: vec!["DRAFT_TEST_ALLOWED_VALUE".to_string()],
            },
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&allowed.stdout).trim(), "permitted");

        std::env::remove_var("DRAFT_TEST_SECRET_VALUE");
        std::env::remove_var("DRAFT_TEST_ALLOWED_VALUE");
    }

    #[test]
    fn a_program_that_cannot_start_is_refused_clearly() {
        let workspace = tempfile::tempdir().unwrap();
        let error = run(
            "draft-no-such-program-exists",
            &[],
            workspace.path(),
            &ProcessLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CandidateNotConfigured);
    }

    #[cfg(unix)]
    #[test]
    fn captured_output_is_redacted_before_it_becomes_evidence() {
        let workspace = tempfile::tempdir().unwrap();
        let outcome = run(
            "sh",
            &[
                "-c".to_string(),
                "echo 'AWS_SECRET_ACCESS_KEY=AKIAIOSFODNN7EXAMPLEKEY0'".to_string(),
            ],
            workspace.path(),
            &ProcessLimits::default(),
        )
        .unwrap();
        let captured = String::from_utf8_lossy(&outcome.stdout);
        assert!(
            !captured.contains("AKIAIOSFODNN7EXAMPLEKEY0"),
            "secret survived redaction: {captured}"
        );
    }
}
