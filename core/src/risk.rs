//! Explainable, rule-first risk model (PRD §9.15, TDD §31–32).
//!
//! Risk is deterministic and offline by default: a transparent set of weighted
//! rules produces a 0–100 score, a level, human-readable explanations, and
//! required actions. Intent is a first-class input (a `docs` pack touching
//! source is suspicious; a `security` pack without tests is high-risk). A
//! feature vector is also emitted for an optional ML assist, but per spec the ML
//! score is never the sole blocker in P0.

use crate::pack::PackIntent;
use serde::{Deserialize, Serialize};

/// Risk severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Critical => "critical",
        }
    }
}

/// The explainable risk report persisted to `risk.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskReport {
    pub risk_level: RiskLevel,
    pub risk_score: u32,
    pub explanations: Vec<String>,
    pub required_actions: Vec<String>,
    /// ML-ready feature vector (advisory; never the sole blocker in P0).
    #[serde(default)]
    pub feature_vector: Vec<f64>,
}

/// Inputs to the risk model (all locally derivable, offline).
#[derive(Debug, Clone)]
pub struct RiskInputs {
    pub intent: PackIntent,
    pub files_touched: usize,
    pub lines_changed: usize,
    pub high_risk_paths: Vec<String>,
    pub has_tests: bool,
    pub has_fuzz: bool,
    pub public_api_changes: usize,
    pub imported: bool,
    pub dependency_count: usize,
    pub semantic_impact: usize,
    /// 0.0–1.0 rollback rate of the producing candidate (a signal, not a verdict).
    pub candidate_rollback_rate: f64,
}

impl Default for RiskInputs {
    fn default() -> Self {
        RiskInputs {
            intent: PackIntent::Feature,
            files_touched: 0,
            lines_changed: 0,
            high_risk_paths: Vec::new(),
            has_tests: false,
            has_fuzz: false,
            public_api_changes: 0,
            imported: false,
            dependency_count: 0,
            semantic_impact: 0,
            candidate_rollback_rate: 0.0,
        }
    }
}

/// Path fragments that indicate sensitive areas.
const HIGH_RISK_FRAGMENTS: &[&str] = &[
    "auth",
    "payment",
    "billing",
    "security",
    "crypto",
    "secret",
    "password",
    "token",
    "migration",
    "/.env",
    "key",
];

/// Return the sensitive fragments matched by any of `paths`.
pub fn high_risk_paths(paths: &[String]) -> Vec<String> {
    let mut hits = Vec::new();
    for p in paths {
        let lower = p.to_lowercase();
        for frag in HIGH_RISK_FRAGMENTS {
            if lower.contains(frag) && !hits.contains(&frag.to_string()) {
                hits.push(frag.to_string());
            }
        }
    }
    hits
}

/// Assess risk from inputs. Deterministic and fully explainable.
pub fn assess(inputs: &RiskInputs) -> RiskReport {
    let mut score: i32 = 0;
    let mut explanations = Vec::new();
    let mut required = Vec::new();

    // Size signals.
    if inputs.files_touched > 100 {
        score += 25;
        explanations.push(format!("very large change: {} files", inputs.files_touched));
    } else if inputs.files_touched > 20 {
        score += 12;
        explanations.push(format!("large change: {} files", inputs.files_touched));
    }
    if inputs.lines_changed > 1000 {
        score += 15;
        explanations.push(format!("{} lines changed", inputs.lines_changed));
    }

    // Sensitive paths.
    if !inputs.high_risk_paths.is_empty() {
        score += 20;
        explanations.push(format!(
            "touches sensitive areas: {}",
            inputs.high_risk_paths.join(", ")
        ));
        required.push("review sensitive-path changes carefully".to_string());
    }

    // Public API / semantic impact.
    if inputs.public_api_changes > 0 {
        score += 15;
        explanations.push(format!(
            "changes {} public API symbol(s)",
            inputs.public_api_changes
        ));
    }
    if inputs.semantic_impact > 5 {
        score += 10;
        explanations.push(format!(
            "broad semantic impact ({} symbols)",
            inputs.semantic_impact
        ));
    }

    // Evidence gaps.
    if !inputs.has_tests {
        score += 15;
        explanations.push("no tests selected for changed code".to_string());
        required.push("add or select tests covering the change".to_string());
    }

    // Imported packs are untrusted until locally verified.
    if inputs.imported {
        score += 15;
        explanations.push("imported pack — untrusted until locally verified".to_string());
        required.push("locally re-verify the imported pack before save".to_string());
    }

    // Candidate history (signal only).
    if inputs.candidate_rollback_rate > 0.3 {
        score += 8;
        explanations.push(format!(
            "producing candidate has a {:.0}% rollback rate",
            inputs.candidate_rollback_rate * 100.0
        ));
    }

    // Intent-aware rules.
    apply_intent_rules(inputs, &mut score, &mut explanations, &mut required);

    let score = score.clamp(0, 100) as u32;
    let level = level_for(score);
    if matches!(level, RiskLevel::High | RiskLevel::Critical) {
        required.push("require human approval before save".to_string());
    }
    if explanations.is_empty() {
        explanations.push("small, low-signal change".to_string());
    }
    RiskReport {
        risk_level: level,
        risk_score: score,
        explanations,
        required_actions: dedup(required),
        feature_vector: feature_vector(inputs),
    }
}

fn apply_intent_rules(
    inputs: &RiskInputs,
    score: &mut i32,
    explanations: &mut Vec<String>,
    required: &mut Vec<String>,
) {
    match inputs.intent {
        // A docs pack that modifies source/sensitive code is suspicious.
        PackIntent::Docs
            if inputs.public_api_changes > 0
                || (inputs.files_touched > 0 && !inputs.high_risk_paths.is_empty()) =>
        {
            *score += 20;
            explanations.push("docs-intent pack modifies source/sensitive code".to_string());
            required.push("reclassify the pack intent or split the change".to_string());
        }
        PackIntent::Refactor if inputs.public_api_changes > 0 => {
            *score += 15;
            explanations.push("refactor changes public behavior/API".to_string());
        }
        PackIntent::Security if !inputs.has_tests || !inputs.has_fuzz => {
            *score += 20;
            explanations.push("security-intent pack lacks tests and/or fuzzing".to_string());
            required.push("run full verification with fuzzing".to_string());
        }
        PackIntent::Migration if !inputs.has_tests => {
            *score += 18;
            explanations.push("migration-intent pack lacks migration tests".to_string());
            required.push("add migration-specific tests".to_string());
        }
        PackIntent::Generated => {
            *score += 8;
            explanations.push("machine-generated pack".to_string());
            required.push("require human review of generated changes".to_string());
        }
        _ => {}
    }
}

fn level_for(score: u32) -> RiskLevel {
    match score {
        0..=24 => RiskLevel::Low,
        25..=54 => RiskLevel::Medium,
        55..=79 => RiskLevel::High,
        _ => RiskLevel::Critical,
    }
}

/// ML-ready feature vector. The ML score, when available, is advisory only and
/// never the sole blocker in P0.
pub fn feature_vector(inputs: &RiskInputs) -> Vec<f64> {
    vec![
        inputs.files_touched as f64,
        inputs.lines_changed as f64,
        inputs.high_risk_paths.len() as f64,
        inputs.public_api_changes as f64,
        inputs.has_tests as u8 as f64,
        inputs.has_fuzz as u8 as f64,
        inputs.dependency_count as f64,
        inputs.imported as u8 as f64,
        inputs.semantic_impact as f64,
        inputs.candidate_rollback_rate,
    ]
}

fn dedup(mut v: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    v.retain(|x| seen.insert(x.clone()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_change_is_low_risk() {
        let inputs = RiskInputs {
            files_touched: 1,
            lines_changed: 5,
            has_tests: true,
            ..Default::default()
        };
        let r = assess(&inputs);
        assert_eq!(r.risk_level, RiskLevel::Low);
    }

    #[test]
    fn security_without_tests_is_high() {
        let inputs = RiskInputs {
            intent: PackIntent::Security,
            files_touched: 3,
            high_risk_paths: vec!["auth".into()],
            has_tests: false,
            has_fuzz: false,
            public_api_changes: 1,
            ..Default::default()
        };
        let r = assess(&inputs);
        assert!(matches!(
            r.risk_level,
            RiskLevel::High | RiskLevel::Critical
        ));
        assert!(r.required_actions.iter().any(|a| a.contains("approval")));
        assert!(r.explanations.iter().any(|e| e.contains("security-intent")));
    }

    #[test]
    fn docs_touching_sensitive_source_is_flagged() {
        let inputs = RiskInputs {
            intent: PackIntent::Docs,
            files_touched: 2,
            high_risk_paths: vec!["auth".into()],
            has_tests: true,
            ..Default::default()
        };
        let r = assess(&inputs);
        assert!(r.explanations.iter().any(|e| e.contains("docs-intent")));
    }

    #[test]
    fn assessment_is_deterministic() {
        let inputs = RiskInputs {
            files_touched: 30,
            lines_changed: 1200,
            public_api_changes: 2,
            ..Default::default()
        };
        assert_eq!(assess(&inputs).risk_score, assess(&inputs).risk_score);
        assert_eq!(high_risk_paths(&["src/auth/mod.rs".into()]), vec!["auth"]);
        assert_eq!(feature_vector(&inputs).len(), 10);
        // The report emits the feature vector (persisted to risk.json).
        assert_eq!(assess(&inputs).feature_vector, feature_vector(&inputs));
    }

    #[test]
    fn risk_report_roundtrips_and_reads_legacy_json() {
        let report = assess(&RiskInputs::default());
        let json = serde_json::to_string(&report).unwrap();
        let back: RiskReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.feature_vector, report.feature_vector);
        assert_eq!(back.feature_vector.len(), 10);

        // A pre-feature-vector risk.json must still deserialize.
        let legacy = r#"{
            "risk_level": "low",
            "risk_score": 3,
            "explanations": ["small, low-signal change"],
            "required_actions": []
        }"#;
        let back: RiskReport = serde_json::from_str(legacy).unwrap();
        assert!(back.feature_vector.is_empty());
    }
}
