//! Reviewability budget evaluation for pack readiness.

use crate::error::{DraftError, DraftResult};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewabilityBudget {
    pub max_files: usize,
    pub max_changed_lines: u64,
    pub max_zones: usize,
    pub max_ownership_domains: usize,
    pub max_unresolved_warnings: usize,
}

impl Default for ReviewabilityBudget {
    fn default() -> Self {
        Self {
            max_files: 30,
            max_changed_lines: 800,
            max_zones: 6,
            max_ownership_domains: 4,
            max_unresolved_warnings: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewabilityReport {
    pub status: String,
    pub reasons: Vec<String>,
    pub recommended_action: Option<String>,
}

impl ReviewabilityReport {
    pub fn good() -> Self {
        Self {
            status: "good".into(),
            reasons: Vec::new(),
            recommended_action: None,
        }
    }

    pub fn poor(reasons: Vec<String>) -> Self {
        Self {
            status: "poor".into(),
            reasons,
            recommended_action: Some("draft task <task> --decompose".into()),
        }
    }
}

pub fn budget(root: &Path) -> DraftResult<ReviewabilityBudget> {
    let mut budget = ReviewabilityBudget::default();
    let config = root.join(".draft/config.toml");
    if !config.exists() {
        return Ok(budget);
    }
    let value = fs::read_to_string(&config)
        .map_err(|e| DraftError::storage(format!("failed to read {}: {e}", config.display())))?
        .parse::<toml::Value>()
        .map_err(|e| DraftError::invalid_config(format!("invalid config.toml: {e}")))?;
    let Some(table) = value.get("reviewability").and_then(toml::Value::as_table) else {
        return Ok(budget);
    };
    if let Some(v) = get_usize(table, "max_files") {
        budget.max_files = v;
    }
    if let Some(v) = get_u64(table, "max_changed_lines") {
        budget.max_changed_lines = v;
    }
    if let Some(v) = get_usize(table, "max_zones") {
        budget.max_zones = v;
    }
    if let Some(v) = get_usize(table, "max_ownership_domains") {
        budget.max_ownership_domains = v;
    }
    if let Some(v) = get_usize(table, "max_unresolved_warnings") {
        budget.max_unresolved_warnings = v;
    }
    Ok(budget)
}

pub fn evaluate(
    budget: &ReviewabilityBudget,
    files_changed: usize,
    changed_lines: u64,
    zones: usize,
    ownership_domains: usize,
    unresolved_warnings: usize,
) -> ReviewabilityReport {
    let mut reasons = Vec::new();
    if files_changed > budget.max_files {
        reasons.push(format!("{files_changed} files changed"));
    }
    if changed_lines > budget.max_changed_lines {
        reasons.push(format!("{changed_lines} changed lines"));
    }
    if zones > budget.max_zones {
        reasons.push(format!("{zones} zones touched"));
    }
    if ownership_domains > budget.max_ownership_domains {
        reasons.push(format!("{ownership_domains} ownership domains touched"));
    }
    if unresolved_warnings > budget.max_unresolved_warnings {
        reasons.push(format!("{unresolved_warnings} unresolved warnings"));
    }
    if reasons.is_empty() {
        ReviewabilityReport::good()
    } else {
        ReviewabilityReport::poor(reasons)
    }
}

fn get_usize(table: &toml::map::Map<String, toml::Value>, key: &str) -> Option<usize> {
    table.get(key)?.as_integer()?.try_into().ok()
}

fn get_u64(table: &toml::map::Map<String, toml::Value>, key: &str) -> Option<u64> {
    table.get(key)?.as_integer()?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_marks_large_changes_poor() {
        let budget = ReviewabilityBudget {
            max_files: 1,
            ..ReviewabilityBudget::default()
        };
        let report = evaluate(&budget, 2, 1, 1, 0, 0);
        assert_eq!(report.status, "poor");
        assert!(report.reasons[0].contains("2 files"));
    }
}
