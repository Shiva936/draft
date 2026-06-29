//! The single point through which **all** Git command execution flows.
//!
//! Rules (Blueprint §9.4): structured argument arrays only (no shell strings),
//! capture stdout/stderr, return structured [`ProviderError`]s. No other module
//! — and nothing in `core`, `cli`, `tui`, or `services` — spawns `git`.

use std::path::{Path, PathBuf};
use std::process::Command;

use draft_core::vcs::errors::{ProviderError, ProviderErrorKind};

pub const ZERO_OID: &str = "0000000000000000000000000000000000000000";

/// A Git command runner bound to a working directory.
#[derive(Debug, Clone)]
pub struct GitCommand {
    pub cwd: PathBuf,
}

/// The captured result of a git invocation.
pub struct GitOutput {
    pub status_code: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl GitCommand {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        GitCommand { cwd: cwd.into() }
    }

    /// Run git with the given args, returning the raw captured output without
    /// treating a non-zero exit as an error (callers decide).
    pub fn run_raw(&self, args: &[&str]) -> Result<GitOutput, ProviderError> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.cwd)
            .output()
            .map_err(|e| {
                ProviderError::new(
                    ProviderErrorKind::CommandFailed,
                    format!("failed to spawn git: {e}"),
                )
                .with_suggestion("Ensure `git` is installed and on PATH.")
            })?;
        Ok(GitOutput {
            status_code: output.status.code(),
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Run git and require success; returns trimmed stdout (trailing newline
    /// only — leading spaces matter for `status --porcelain`).
    pub fn run(&self, args: &[&str]) -> Result<String, ProviderError> {
        let out = self.run_raw(args)?;
        if out.success {
            Ok(out.stdout.trim_end_matches(['\n', '\r']).to_string())
        } else {
            Err(ProviderError::new(
                ProviderErrorKind::CommandFailed,
                format!("git {} failed", args.join(" ")),
            )
            .with_context(out.stderr.trim().to_string()))
        }
    }

    /// Run git and return raw stdout bytes (for binary-safe operations).
    pub fn run_bytes(&self, args: &[&str]) -> Result<Vec<u8>, ProviderError> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.cwd)
            .output()
            .map_err(|e| {
                ProviderError::new(
                    ProviderErrorKind::CommandFailed,
                    format!("failed to spawn git: {e}"),
                )
            })?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(ProviderError::new(
                ProviderErrorKind::CommandFailed,
                format!("git {} failed", args.join(" ")),
            )
            .with_context(String::from_utf8_lossy(&output.stderr).trim().to_string()))
        }
    }

    // --- common queries -----------------------------------------------------

    pub fn current_head(&self) -> Result<String, ProviderError> {
        match self.run(&["rev-parse", "HEAD"]) {
            Ok(oid) => Ok(oid),
            Err(e) if e.kind == ProviderErrorKind::CommandFailed => {
                // Unborn branch (freshly initialized repository).
                let ctx = e.context.clone().unwrap_or_default();
                if ctx.contains("ambiguous argument 'HEAD'")
                    || ctx.contains("Needed a single revision")
                {
                    Ok(ZERO_OID.to_string())
                } else {
                    Err(e)
                }
            }
            Err(e) => Err(e),
        }
    }

    pub fn branch_name(&self) -> Result<Option<String>, ProviderError> {
        let b = self.run(&["branch", "--show-current"])?;
        Ok(if b.is_empty() { None } else { Some(b) })
    }

    pub fn git_dir(&self) -> Result<PathBuf, ProviderError> {
        let s = self.run(&["rev-parse", "--git-dir"])?;
        let p = Path::new(&s);
        Ok(if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.cwd.join(p)
        })
    }

    pub fn toplevel(&self) -> Result<PathBuf, ProviderError> {
        let s = self.run(&["rev-parse", "--show-toplevel"])?;
        Ok(PathBuf::from(s))
    }

    pub fn status_porcelain(&self) -> Result<String, ProviderError> {
        self.run(&["status", "--porcelain=v1"])
    }

    pub fn config_get(&self, key: &str) -> Result<Option<String>, ProviderError> {
        let out = self.run_raw(&["config", "--get", key])?;
        if out.success {
            Ok(Some(out.stdout.trim_end().to_string()))
        } else if out.status_code == Some(1) {
            Ok(None)
        } else {
            Err(
                ProviderError::command_failed(format!("git config --get {key} failed"))
                    .with_context(out.stderr.trim().to_string()),
            )
        }
    }

    pub fn show_file(&self, revision: &str, path: &str) -> Result<Vec<u8>, ProviderError> {
        self.run_bytes(&["show", &format!("{revision}:{path}")])
    }
}
