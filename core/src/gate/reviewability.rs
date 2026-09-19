//! Whether a change is small enough for a human to actually review.
//!
//! Every axis here is domain-neutral. Resource and review-unit counts are
//! things Draft always knows; anything finer — how many lines, records or frames
//! moved — arrives as a *contributed* metric, because only the extension that
//! produced the representation knows what its numbers mean. A budget naming a
//! metric nothing contributes simply does not fire, rather than firing against a
//! zero Draft invented.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftResult};
use draft_extension_contract::{ChangeMetricBudget, ReviewabilityBudget};

/// The budget a project applies, plus the limits Draft can always evaluate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectReviewabilityBudget {
    /// The contributed half: resources, review units and named metrics.
    #[serde(flatten)]
    pub contributed: ReviewabilityBudget,
    /// How many separate ownership domains one change may touch.
    pub max_ownership_domains: usize,
    /// How many unresolved warnings its evidence may carry.
    pub max_unresolved_warnings: usize,
}

impl Default for ProjectReviewabilityBudget {
    fn default() -> Self {
        Self {
            contributed: ReviewabilityBudget {
                max_resources: Some(30),
                max_review_units: None,
                max_change_metrics: Vec::new(),
            },
            max_ownership_domains: 4,
            max_unresolved_warnings: 10,
        }
    }
}

/// What one change looks like from a reviewer's point of view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewabilityFacts {
    pub resources_changed: u64,
    /// Units a person could accept or reject individually, when a
    /// representation produced any.
    pub review_units: Option<u64>,
    /// Contributed metric totals, keyed exactly as the contributing extension
    /// named them.
    pub change_metrics: BTreeMap<String, i64>,
    pub ownership_domains: usize,
    pub unresolved_warnings: usize,
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

/// The project's effective reviewability budget.
///
/// Contributed budgets compose conservatively — the tightest limit any
/// `control_policy` asks for wins — and `.draft/config.toml` then has the final
/// word, on the same precedence as every other project setting: a project that
/// genuinely reviews a hundred resources at a time must be able to say so.
///
/// A budget is advisory. Exceeding it produces a reviewability warning, never a
/// refusal, which is why the project layer is allowed to relax it here where a
/// protection or a risk gate would not be.
pub fn budget(
    root: &Path,
    contributions: &crate::extension::ActiveContributions,
) -> DraftResult<ProjectReviewabilityBudget> {
    let mut budget = ProjectReviewabilityBudget::default();
    for preset in &contributions.policies {
        let Some(contributed) = &preset.value.control_policy.reviewability_budget else {
            continue;
        };
        budget.contributed.max_resources =
            tightest(budget.contributed.max_resources, contributed.max_resources);
        budget.contributed.max_review_units = tightest(
            budget.contributed.max_review_units,
            contributed.max_review_units,
        );
        for metric in &contributed.max_change_metrics {
            match budget
                .contributed
                .max_change_metrics
                .iter_mut()
                .find(|existing| existing.metric == metric.metric)
            {
                Some(existing) => existing.limit = existing.limit.min(metric.limit),
                None => budget.contributed.max_change_metrics.push(metric.clone()),
            }
        }
    }
    budget
        .contributed
        .max_change_metrics
        .sort_by(|a, b| a.metric.cmp(&b.metric));
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
    if let Some(value) = get_u64(table, "max_resources") {
        budget.contributed.max_resources = Some(value);
    }
    if let Some(value) = get_u64(table, "max_review_units") {
        budget.contributed.max_review_units = Some(value);
    }
    if let Some(value) = get_usize(table, "max_ownership_domains") {
        budget.max_ownership_domains = value;
    }
    if let Some(value) = get_usize(table, "max_unresolved_warnings") {
        budget.max_unresolved_warnings = value;
    }
    // Contributed metric limits, written as `[reviewability.metrics]` with the
    // metric's own namespaced key.
    if let Some(metrics) = table.get("metrics").and_then(toml::Value::as_table) {
        for (metric, limit) in metrics {
            if let Some(limit) = limit.as_integer() {
                budget
                    .contributed
                    .max_change_metrics
                    .push(ChangeMetricBudget {
                        metric: metric.clone(),
                        limit,
                    });
            }
        }
    }
    Ok(budget)
}

/// The tighter of two optional limits. An absent limit constrains nothing, so
/// a present one always wins over it.
fn tightest(current: Option<u64>, contributed: Option<u64>) -> Option<u64> {
    match (current, contributed) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Evaluate a change against a budget.
pub fn evaluate(
    budget: &ProjectReviewabilityBudget,
    facts: &ReviewabilityFacts,
) -> ReviewabilityReport {
    let mut reasons = Vec::new();
    if let Some(limit) = budget.contributed.max_resources {
        if facts.resources_changed > limit {
            reasons.push(format!("{} resources changed", facts.resources_changed));
        }
    }
    // A review-unit budget applies only when something actually produced review
    // units. Without a comparison capability there are none, and a budget on
    // them is not silently satisfied by their absence — it simply does not
    // apply, which the report says by not mentioning it.
    if let (Some(limit), Some(units)) = (budget.contributed.max_review_units, facts.review_units) {
        if units > limit {
            reasons.push(format!("{units} review units"));
        }
    }
    for metric_budget in &budget.contributed.max_change_metrics {
        if let Some(value) = facts.change_metrics.get(&metric_budget.metric) {
            if *value > metric_budget.limit {
                reasons.push(format!("{} = {value}", metric_budget.metric));
            }
        }
    }
    if facts.ownership_domains > budget.max_ownership_domains {
        reasons.push(format!(
            "{} ownership domains touched",
            facts.ownership_domains
        ));
    }
    if facts.unresolved_warnings > budget.max_unresolved_warnings {
        reasons.push(format!("{} unresolved warnings", facts.unresolved_warnings));
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

    fn facts(resources: u64) -> ReviewabilityFacts {
        ReviewabilityFacts {
            resources_changed: resources,
            ..ReviewabilityFacts::default()
        }
    }

    #[test]
    fn too_many_resources_is_poor() {
        let mut budget = ProjectReviewabilityBudget::default();
        budget.contributed.max_resources = Some(1);
        let report = evaluate(&budget, &facts(2));
        assert_eq!(report.status, "poor");
        assert!(report.reasons[0].contains("2 resources"));
    }

    #[test]
    fn a_contributed_metric_budget_fires_on_the_contributed_key() {
        let mut budget = ProjectReviewabilityBudget::default();
        budget.contributed.max_change_metrics = vec![ChangeMetricBudget {
            metric: "draft.text.document/lines_changed".into(),
            limit: 100,
        }];
        let mut over = facts(1);
        over.change_metrics
            .insert("draft.text.document/lines_changed".into(), 500);
        assert_eq!(evaluate(&budget, &over).status, "poor");

        let mut under = facts(1);
        under
            .change_metrics
            .insert("draft.text.document/lines_changed".into(), 10);
        assert_eq!(evaluate(&budget, &under).status, "good");
    }

    #[test]
    fn a_budget_on_a_metric_nobody_contributes_does_not_fire() {
        // The honest behaviour: with no comparison capability there is no such
        // metric, and Draft must not invent a zero — nor a failure.
        let mut budget = ProjectReviewabilityBudget::default();
        budget.contributed.max_change_metrics = vec![ChangeMetricBudget {
            metric: "example/never-contributed".into(),
            limit: 1,
        }];
        assert_eq!(evaluate(&budget, &facts(1)).status, "good");
    }

    #[test]
    fn a_review_unit_budget_needs_review_units_to_exist() {
        let mut budget = ProjectReviewabilityBudget::default();
        budget.contributed.max_review_units = Some(5);
        // No representation derived: the budget does not apply.
        assert_eq!(evaluate(&budget, &facts(1)).status, "good");

        let mut with_units = facts(1);
        with_units.review_units = Some(50);
        assert_eq!(evaluate(&budget, &with_units).status, "poor");
    }

    #[test]
    fn ownership_and_warning_limits_still_apply_with_nothing_installed() {
        let budget = ProjectReviewabilityBudget::default();
        let mut wide = facts(1);
        wide.ownership_domains = 99;
        let report = evaluate(&budget, &wide);
        assert_eq!(report.status, "poor");
        assert!(report.reasons[0].contains("ownership domains"));
    }
}
