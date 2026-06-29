use std::path::{Path, PathBuf};
use chrono::Utc;
use uuid::Uuid;

use crate::errors::DraftError;
use crate::models::{VerificationEvidence, VerificationStatus};
use crate::storage::DraftStorage;

pub struct VerificationEngine;

impl VerificationEngine {
    pub fn infer_command(repo_root: &Path) -> Option<String> {
        let cargo_toml = repo_root.join("Cargo.toml");
        if cargo_toml.exists() {
            return Some("cargo test".to_string());
        }

        let go_mod = repo_root.join("go.mod");
        if go_mod.exists() {
            return Some("go test ./...".to_string());
        }

        let package_json = repo_root.join("package.json");
        if package_json.exists() {
            return Some("npm test".to_string());
        }

        let pyproject_toml = repo_root.join("pyproject.toml");
        if pyproject_toml.exists() {
            return Some("pytest".to_string());
        }

        let makefile = repo_root.join("Makefile");
        if makefile.exists() {
            return Some("make test".to_string());
        }

        None
    }

    pub fn run(
        repo_root: &Path,
        command: &str,
        storage: &DraftStorage,
    ) -> Result<VerificationEvidence, DraftError> {
        let started_at = Utc::now();

        let output_result = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(repo_root)
            .output();

        let finished_at = Utc::now();
        let duration_ms = finished_at.signed_duration_since(started_at).num_milliseconds().max(0) as u64;

        let evidence = match output_result {
            Ok(output) => {
                let exit_code = output.status.code();
                let status = if output.status.success() {
                    VerificationStatus::Passed
                } else {
                    VerificationStatus::Failed
                };

                let stdout_raw = String::from_utf8_lossy(&output.stdout);
                let stderr_raw = String::from_utf8_lossy(&output.stderr);

                // Truncate stdout and stderr to avoid massive evidence files
                let limit = 10000;
                let stdout_summary = if stdout_raw.len() > limit {
                    format!("{}... [truncated]", &stdout_raw[..limit])
                } else {
                    stdout_raw.into_owned()
                };

                let stderr_summary = if stderr_raw.len() > limit {
                    format!("{}... [truncated]", &stderr_raw[..limit])
                } else {
                    stderr_raw.into_owned()
                };

                VerificationEvidence {
                    verification_id: Uuid::new_v4().to_string(),
                    command: command.to_string(),
                    exit_code,
                    status,
                    started_at,
                    finished_at,
                    duration_ms,
                    stdout_summary,
                    stderr_summary,
                }
            }
            Err(e) => {
                VerificationEvidence {
                    verification_id: Uuid::new_v4().to_string(),
                    command: command.to_string(),
                    exit_code: None,
                    status: VerificationStatus::Failed,
                    started_at,
                    finished_at,
                    duration_ms,
                    stdout_summary: String::new(),
                    stderr_summary: format!("Failed to spawn command process: {}", e),
                }
            }
        };

        // Write evidence to JSON
        let rel_path = PathBuf::from("verification").join(format!("{}.json", evidence.verification_id));
        storage.write_json(&rel_path, &evidence)?;

        storage.append_log(&format!("Verification run complete: {} -> {:?}", command, evidence.status))?;

        Ok(evidence)
    }
}
