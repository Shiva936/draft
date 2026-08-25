//! Local ownership matching for review readiness.

use crate::support::error::DraftResult;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipMatch {
    pub path: String,
    pub pattern: String,
    pub owners: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipReport {
    pub matches: Vec<OwnershipMatch>,
    pub domains: Vec<String>,
    pub missing_owner_review: bool,
}

impl OwnershipReport {
    pub fn empty() -> Self {
        Self {
            matches: Vec::new(),
            domains: Vec::new(),
            missing_owner_review: false,
        }
    }
}

pub fn evaluate(
    root: &Path,
    paths: &[String],
    reviewers: &[String],
) -> DraftResult<OwnershipReport> {
    let config = root.join(".draft/config.toml");
    if !config.exists() {
        return Ok(OwnershipReport::empty());
    }
    let (_, owners) = crate::workspace::config::rules(&config)?;
    if owners.is_empty() {
        return Ok(OwnershipReport::empty());
    }
    let mut matches = Vec::new();
    let mut domains = BTreeSet::new();
    for path in paths {
        for (pattern, owner_list) in owners.iter() {
            if owner_list.is_empty() || !glob_match(pattern, path) {
                continue;
            }
            for owner in owner_list {
                domains.insert(owner.clone());
            }
            matches.push(OwnershipMatch {
                path: path.clone(),
                pattern: pattern.clone(),
                owners: owner_list.clone(),
            });
        }
    }
    let reviewer_set: BTreeSet<_> = reviewers.iter().cloned().collect();
    let missing_owner_review = !domains.is_empty()
        && domains.iter().all(|owner| {
            !reviewer_set.contains(owner) && !reviewer_set.contains(owner.trim_start_matches('@'))
        });
    Ok(OwnershipReport {
        matches,
        domains: domains.into_iter().collect(),
        missing_owner_review,
    })
}

fn glob_match(pattern: &str, path: &str) -> bool {
    if pattern == "**" || pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return path.starts_with(prefix);
    }
    pattern == path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owners_match_domains_and_reviewers() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".draft")).unwrap();
        std::fs::write(
            tmp.path().join(".draft/config.toml"),
            "schema_version = 1\n[owners]\n\"core/**\" = [\"@core\"]\n",
        )
        .unwrap();
        let report = evaluate(tmp.path(), &["core/lib.rs".into()], &[]).unwrap();
        assert_eq!(report.domains, vec!["@core"]);
        assert!(report.missing_owner_review);
        let report = evaluate(tmp.path(), &["core/lib.rs".into()], &["@core".into()]).unwrap();
        assert!(!report.missing_owner_review);
    }
}
