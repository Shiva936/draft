//! Provider-neutral risk evaluation (FR-RISK-001/002).
//!
//! Operates entirely on the neutral [`ProviderDelta`] — it has no provider
//! knowledge. Secret findings never include the raw secret value (NFR §8.2).

use serde::{Deserialize, Serialize};

use crate::common::WorkspacePath;
use crate::vcs::types::{DiffLine, FileDelta, FileStatus, ProviderDelta};
use crate::workspace::config::RiskConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    pub fn label(&self) -> &'static str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskFindingKind {
    SecretLikePattern,
    LargeDiff,
    DeletionHeavy,
    BinaryChange,
    DependencyOrConfigChange,
    SecuritySensitiveFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskFinding {
    pub kind: RiskFindingKind,
    pub path: Option<WorkspacePath>,
    pub message: String,
    pub severity: RiskLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskSummary {
    pub level: RiskLevel,
    pub score: u32,
    pub findings: Vec<RiskFinding>,
}

impl RiskSummary {
    pub fn low() -> Self {
        RiskSummary {
            level: RiskLevel::Low,
            score: 0,
            findings: vec![],
        }
    }
}

/// Evaluate a delta against the configured rules.
pub fn evaluate(delta: &ProviderDelta, config: &RiskConfig) -> RiskSummary {
    let mut findings = Vec::new();

    // Large diff.
    let total_lines = delta.stats.additions + delta.stats.deletions;
    if total_lines > config.large_diff_threshold_lines {
        findings.push(RiskFinding {
            kind: RiskFindingKind::LargeDiff,
            path: None,
            message: format!(
                "large change: {total_lines} lines across {} files",
                delta.stats.files_changed
            ),
            severity: RiskLevel::Medium,
        });
    }

    // Deletion-heavy.
    let deletions = delta
        .files
        .iter()
        .filter(|f| f.status == FileStatus::Deleted)
        .count();
    if deletions >= config.deletion_threshold_files {
        findings.push(RiskFinding {
            kind: RiskFindingKind::DeletionHeavy,
            path: None,
            message: format!("{deletions} files deleted"),
            severity: RiskLevel::High,
        });
    }

    for f in &delta.files {
        if f.binary && f.status != FileStatus::Deleted {
            findings.push(RiskFinding {
                kind: RiskFindingKind::BinaryChange,
                path: Some(f.path.clone()),
                message: "binary file changed".to_string(),
                severity: RiskLevel::Medium,
            });
        }
        if is_dependency_or_config(f.path.as_str()) {
            findings.push(RiskFinding {
                kind: RiskFindingKind::DependencyOrConfigChange,
                path: Some(f.path.clone()),
                message: "dependency/config file changed".to_string(),
                severity: RiskLevel::Medium,
            });
        }
        if is_security_sensitive(f.path.as_str()) {
            findings.push(RiskFinding {
                kind: RiskFindingKind::SecuritySensitiveFile,
                path: Some(f.path.clone()),
                message: "security-sensitive path changed".to_string(),
                severity: RiskLevel::High,
            });
        }
        if config.detect_secrets {
            if let Some(reason) = scan_for_secret(f) {
                findings.push(RiskFinding {
                    kind: RiskFindingKind::SecretLikePattern,
                    path: Some(f.path.clone()),
                    // Deliberately does NOT include the matched value.
                    message: format!("possible secret-like pattern ({reason})"),
                    severity: RiskLevel::Critical,
                });
            }
        }
    }

    summarize(findings)
}

fn summarize(findings: Vec<RiskFinding>) -> RiskSummary {
    let level = findings
        .iter()
        .map(|f| f.severity)
        .max()
        .unwrap_or(RiskLevel::Low);
    let score = findings
        .iter()
        .map(|f| match f.severity {
            RiskLevel::Low => 1,
            RiskLevel::Medium => 5,
            RiskLevel::High => 20,
            RiskLevel::Critical => 50,
        })
        .sum();
    RiskSummary {
        level,
        score,
        findings,
    }
}

fn is_dependency_or_config(path: &str) -> bool {
    const NAMES: &[&str] = &[
        "Cargo.toml",
        "Cargo.lock",
        "package.json",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "go.mod",
        "go.sum",
        "requirements.txt",
        "Pipfile",
        "poetry.lock",
        "Gemfile",
        "Gemfile.lock",
        "pom.xml",
        "build.gradle",
    ];
    let base = path.rsplit('/').next().unwrap_or(path);
    NAMES.contains(&base) || base.ends_with(".lock")
}

fn is_security_sensitive(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    const HINTS: &[&str] = &[
        "auth",
        "login",
        "password",
        "secret",
        "credential",
        "payment",
        "token",
        "crypto",
        ".env",
        "id_rsa",
        "private",
        "key",
    ];
    HINTS.iter().any(|h| p.contains(h))
}

/// Returns a *reason label* (not the secret) if a hunk's added lines look like
/// they introduce a credential.
fn scan_for_secret(f: &FileDelta) -> Option<&'static str> {
    for hunk in &f.hunks {
        for line in &hunk.lines {
            if let DiffLine::Added(content) = line {
                let lc = content.to_ascii_lowercase();
                if content.contains("-----BEGIN") && content.contains("PRIVATE KEY") {
                    return Some("private key block");
                }
                if content.contains("AKIA") && content.len() > 16 {
                    return Some("aws access key id");
                }
                if (lc.contains("password") || lc.contains("passwd"))
                    && lc.contains('=')
                    && has_value_after_assign(content)
                {
                    return Some("hardcoded password assignment");
                }
                if (lc.contains("api_key")
                    || lc.contains("apikey")
                    || lc.contains("secret")
                    || lc.contains("token"))
                    && lc.contains('=')
                    && has_value_after_assign(content)
                {
                    return Some("hardcoded secret assignment");
                }
            }
        }
    }
    None
}

fn has_value_after_assign(content: &str) -> bool {
    if let Some(idx) = content.find('=') {
        let rest = content[idx + 1..].trim().trim_matches(['"', '\'']);
        // A non-trivial value (avoids matching `password =` placeholders / `==`).
        rest.len() >= 6 && !rest.starts_with('=')
    } else {
        false
    }
}
