//! Provider-neutral verification: run configured commands, capture results,
//! persist them, and summarize for the finalization gate (FR-VER-001/002/003).

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::common::{now, Timestamp, VerificationPlanId, VerificationResultId};
use crate::error::DraftResult;
use crate::fsutil::{list_with_extension, read_json, write_atomic, write_json};
use crate::workspace::config::{VerificationCommandConfig, VerificationConfig};
use crate::workspace::layout::DraftLayout;

const MAX_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationCommand {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub timeout_ms: Option<u64>,
}

impl From<&VerificationCommandConfig> for VerificationCommand {
    fn from(c: &VerificationCommandConfig) -> Self {
        VerificationCommand {
            name: c.name.clone(),
            command: c.command.clone(),
            args: c.args.clone(),
            timeout_ms: c.timeout_ms,
        }
    }
}

impl VerificationCommand {
    pub fn display(&self) -> String {
        if self.args.is_empty() {
            self.command.clone()
        } else {
            format!("{} {}", self.command, self.args.join(" "))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationPlan {
    pub id: VerificationPlanId,
    pub commands: Vec<VerificationCommand>,
}

impl VerificationPlan {
    /// Build a plan from workspace config, falling back to an inferred command.
    pub fn from_config(config: &VerificationConfig, provider_root: &Path) -> Self {
        let mut commands: Vec<VerificationCommand> =
            config.commands.iter().map(Into::into).collect();
        if commands.is_empty() {
            if let Some(inferred) = infer_command(provider_root) {
                commands.push(inferred);
            }
        }
        VerificationPlan {
            id: VerificationPlanId::generate(),
            commands,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerificationStatus {
    Passed,
    Failed,
    Skipped,
    TimedOut,
    Cancelled,
}

impl VerificationStatus {
    pub fn label(&self) -> &'static str {
        match self {
            VerificationStatus::Passed => "passed",
            VerificationStatus::Failed => "failed",
            VerificationStatus::Skipped => "skipped",
            VerificationStatus::TimedOut => "timed-out",
            VerificationStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandResult {
    pub name: String,
    pub command: String,
    pub status: VerificationStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout_summary: String,
    pub stderr_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationResult {
    pub id: VerificationResultId,
    pub plan_id: VerificationPlanId,
    pub status: VerificationStatus,
    pub command_results: Vec<CommandResult>,
    pub started_at: Timestamp,
    pub completed_at: Timestamp,
}

impl VerificationResult {
    pub fn summary(&self) -> VerificationSummary {
        VerificationSummary {
            result_id: self.id.clone(),
            status: self.status,
            commands: self.command_results.len(),
            passed: self
                .command_results
                .iter()
                .filter(|c| c.status == VerificationStatus::Passed)
                .count(),
        }
    }
}

/// Compact verification summary embedded in operations and receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationSummary {
    pub result_id: VerificationResultId,
    pub status: VerificationStatus,
    pub commands: usize,
    pub passed: usize,
}

/// Execute a plan against `provider_root` and persist the result under `.draft/`.
///
/// Commands are always surfaced to the caller before running (the caller prints
/// them); this function itself never runs anything silently beyond the plan.
pub fn run(
    layout: &DraftLayout,
    provider_root: &Path,
    plan: &VerificationPlan,
) -> DraftResult<VerificationResult> {
    let started_at = now();
    let mut command_results = Vec::new();
    let mut overall = VerificationStatus::Passed;

    for cmd in &plan.commands {
        let cr = run_one(provider_root, cmd);
        if cr.status != VerificationStatus::Passed && overall == VerificationStatus::Passed {
            overall = cr.status;
        }
        command_results.push(cr);
    }
    if plan.commands.is_empty() {
        overall = VerificationStatus::Skipped;
    }

    let result = VerificationResult {
        id: VerificationResultId::generate(),
        plan_id: plan.id.clone(),
        status: overall,
        command_results,
        started_at,
        completed_at: now(),
    };

    let path = layout
        .verification_dir()
        .join(format!("result_{}.json", result.id));
    write_json(&path, &result)?;
    // Persist raw logs alongside for auditability.
    persist_logs(layout, &result)?;
    Ok(result)
}

fn run_one(provider_root: &Path, cmd: &VerificationCommand) -> CommandResult {
    let start = Instant::now();
    let mut child = match Command::new(&cmd.command)
        .args(&cmd.args)
        .current_dir(provider_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return CommandResult {
                name: cmd.name.clone(),
                command: cmd.display(),
                status: VerificationStatus::Failed,
                exit_code: None,
                duration_ms: 0,
                stdout_summary: String::new(),
                stderr_summary: format!("failed to start command: {e}"),
            };
        }
    };

    // Timeout handling via polling (keeps us free of an async runtime).
    let timeout = cmd.timeout_ms.map(Duration::from_millis);
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if let Some(t) = timeout {
                    if start.elapsed() >= t {
                        let _ = child.kill();
                        let _ = child.wait();
                        timed_out = true;
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break,
        }
    }

    let output = child.wait_with_output();
    let duration_ms = start.elapsed().as_millis() as u64;
    let (exit_code, stdout, stderr) = match output {
        Ok(o) => (
            o.status.code(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ),
        Err(e) => (None, String::new(), e.to_string()),
    };

    let status = if timed_out {
        VerificationStatus::TimedOut
    } else if exit_code == Some(0) {
        VerificationStatus::Passed
    } else {
        VerificationStatus::Failed
    };

    CommandResult {
        name: cmd.name.clone(),
        command: cmd.display(),
        status,
        exit_code,
        duration_ms,
        stdout_summary: truncate(&stdout),
        stderr_summary: truncate(&stderr),
    }
}

fn truncate(s: &str) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        s.to_string()
    } else {
        let mut out = s[..MAX_OUTPUT_BYTES].to_string();
        out.push_str("\n…[truncated]");
        out
    }
}

fn persist_logs(layout: &DraftLayout, result: &VerificationResult) -> DraftResult<()> {
    let dir = layout.verification_logs_dir();
    let mut buf = String::new();
    for cr in &result.command_results {
        buf.push_str(&format!("$ {}\n", cr.command));
        buf.push_str(&format!("[status: {}]\n", cr.status.label()));
        if !cr.stdout_summary.is_empty() {
            buf.push_str("--- stdout ---\n");
            buf.push_str(&cr.stdout_summary);
            buf.push('\n');
        }
        if !cr.stderr_summary.is_empty() {
            buf.push_str("--- stderr ---\n");
            buf.push_str(&cr.stderr_summary);
            buf.push('\n');
        }
        buf.push('\n');
    }
    write_atomic(&dir.join(format!("{}.log", result.id)), buf.as_bytes())
}

/// Find the most recently completed verification result, if any.
pub fn latest(layout: &DraftLayout) -> DraftResult<Option<VerificationResult>> {
    let mut latest: Option<VerificationResult> = None;
    for path in list_with_extension(&layout.verification_dir(), "json")? {
        if let Ok(r) = read_json::<VerificationResult>(&path) {
            match &latest {
                None => latest = Some(r),
                Some(cur) if r.completed_at > cur.completed_at => latest = Some(r),
                _ => {}
            }
        }
    }
    Ok(latest)
}

/// Infer a verification command from the project layout (parity with v0.1.0).
pub fn infer_command(root: &Path) -> Option<VerificationCommand> {
    let has = |f: &str| root.join(f).exists();
    let mk = |name: &str, command: &str, args: &[&str]| {
        Some(VerificationCommand {
            name: name.to_string(),
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            timeout_ms: None,
        })
    };
    if has("Cargo.toml") {
        mk("test", "cargo", &["test"])
    } else if has("go.mod") {
        mk("test", "go", &["test", "./..."])
    } else if has("package.json") {
        mk("test", "npm", &["test"])
    } else if has("pyproject.toml") || has("pytest.ini") || has("setup.py") {
        mk("test", "pytest", &[])
    } else if has("Makefile") {
        mk("test", "make", &["test"])
    } else {
        None
    }
}
