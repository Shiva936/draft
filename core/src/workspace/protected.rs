//! Protected-file policy used by pack, submit, execution, and editor paths.

use crate::support::common::WorkspacePath;
use crate::support::error::{DraftError, DraftResult};
use crate::support::pathguard;
use crate::workspace::home::DraftGlobalStore;
use crate::workspace::layout::DraftLayout;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedFileRule {
    pub pattern: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedFileViolation {
    pub path: String,
    pub pattern: String,
    pub reason: String,
}

pub fn default_rules() -> Vec<ProtectedFileRule> {
    [
        (".draft/**", "Draft metadata is never user-editable content"),
        (".env", "environment files commonly contain secrets"),
        (".env.*", "environment files commonly contain secrets"),
        ("*.pem", "private key material is protected"),
        ("*.key", "private key material is protected"),
        ("*.p12", "certificate bundle material is protected"),
        ("*.pfx", "certificate bundle material is protected"),
        ("id_rsa", "SSH private keys are protected"),
        ("id_ed25519", "SSH private keys are protected"),
        ("secrets/**", "secret stores are protected"),
        (".aws/credentials", "cloud credentials are protected"),
        (".npmrc", "package-manager tokens are protected"),
        (".pypirc", "package-manager tokens are protected"),
    ]
    .into_iter()
    .map(|(pattern, reason)| ProtectedFileRule {
        pattern: pattern.to_string(),
        reason: reason.to_string(),
    })
    .collect()
}

pub fn rules_for_project(root: &Path) -> DraftResult<Vec<ProtectedFileRule>> {
    let mut rules = default_rules();
    let home = DraftGlobalStore::locate()?;
    extend_from_config(&mut rules, &home.config_toml())?;
    extend_from_config(&mut rules, &DraftLayout::for_root(root).config_toml())?;
    Ok(rules)
}

/// Whether `path` matches any of the given protected rules.
pub fn matches_rules(rules: &[ProtectedFileRule], path: &str) -> bool {
    first_violation(path, rules).is_some()
}

pub fn violations<'a>(
    root: &Path,
    paths: impl IntoIterator<Item = &'a WorkspacePath>,
) -> DraftResult<Vec<ProtectedFileViolation>> {
    let rules = rules_for_project(root)?;
    Ok(paths
        .into_iter()
        .filter_map(|path| first_violation(path.as_str(), &rules))
        .collect())
}

pub fn ensure_allowed(root: &Path, path: &WorkspacePath) -> DraftResult<()> {
    if let Some(v) = first_violation(path.as_str(), &rules_for_project(root)?) {
        return Err(DraftError::new(
            crate::support::error::DraftErrorKind::ProtectedFileAccess,
            format!("protected file access blocked: {}", v.path),
        )
        .with_context(format!("matched protected pattern '{}'", v.pattern))
        .with_suggestion("change the pack/editor scope or update protected-file policy"));
    }
    Ok(())
}

fn extend_from_config(rules: &mut Vec<ProtectedFileRule>, path: &Path) -> DraftResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let (protected, _) = crate::workspace::config::rules(path)?;
    for rule in protected.protected_files {
        rules.push(ProtectedFileRule {
            pattern: rule.pattern,
            reason: rule
                .reason
                .unwrap_or_else(|| "configured protected file".to_string()),
        });
    }
    Ok(())
}

fn first_violation(path: &str, rules: &[ProtectedFileRule]) -> Option<ProtectedFileViolation> {
    let normalized = path.replace('\\', "/");
    if pathguard::is_draft_path(&normalized) {
        return Some(ProtectedFileViolation {
            path: normalized,
            pattern: ".draft/**".to_string(),
            reason: "Draft metadata is never user-editable content".to_string(),
        });
    }
    rules.iter().find_map(|rule| {
        crate::workspace::snapshot::pattern_match(&rule.pattern, &normalized).then(|| {
            ProtectedFileViolation {
                path: normalized.clone(),
                pattern: rule.pattern.clone(),
                reason: rule.reason.clone(),
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rules_block_secrets_and_draft() {
        let rules = default_rules();
        assert!(first_violation(".env", &rules).is_some());
        assert!(first_violation("secrets/prod.json", &rules).is_some());
        assert!(first_violation("src/lib.rs", &rules).is_none());
        assert!(first_violation("nested/.draft/workspace.json", &rules).is_some());
    }
}
