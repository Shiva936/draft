//! Explainable, rule-first risk.
//!
//! Risk is deterministic and offline: contributed weighted rules produce a
//! 0–100 score, a level, human-readable explanations and required actions. Every
//! condition is domain-neutral — resource and element predicates, neutral change
//! aspects, counts, contributed metrics, evidence state, contributed intents,
//! provenance and uncertainty — so a domain expresses what *it* considers risky
//! without Core learning any of that vocabulary.
//!
//! Core ships no rules at all. With nothing contributed the outcome is
//! [`RiskAssessment::Unassessed`], never a quiet `Low`: "nobody told Draft what
//! matters here" and "Draft checked and this is fine" are different facts, and a
//! gate that cannot tell them apart is not a gate.

use crate::dcg::change_pack_store::ChangePackContentRevisionRecord;
use crate::extension::provenance::ProducerRef;
use crate::provenance::derived::{DerivationInputs, DerivedArtifactKind};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use draft_extension_contract::{
    ChangeAspectName, NamespacedId, RiskCondition, RiskRule, VerificationStateName,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The Core semantics that turn matched rules into a score and a level.
///
/// Recorded with every assessment: the rules are contributed, but the
/// aggregation is Draft's, and a change to it must not silently reinterpret a
/// stored result.
pub const RISK_AGGREGATOR_REVISION: u32 = 1;

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

/// Score bands. Thresholds are configurable; the vocabulary is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskThresholds {
    pub medium: u32,
    pub high: u32,
    pub critical: u32,
}

impl Default for RiskThresholds {
    fn default() -> Self {
        Self {
            medium: 25,
            high: 55,
            critical: 80,
        }
    }
}

impl RiskThresholds {
    pub fn level_for(&self, score: u32) -> RiskLevel {
        if score >= self.critical {
            RiskLevel::Critical
        } else if score >= self.high {
            RiskLevel::High
        } else if score >= self.medium {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        }
    }
}

/// Strict project risk configuration (`risk.toml`).
///
/// Note the absence of default rules. Core has no opinion about which resources
/// are sensitive: "auth", "payment" and "migration" are software-project
/// vocabulary, and a music or CAD project's sensitivities are entirely
/// different. Rules arrive from extensions or from this file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskConfig {
    pub schema_version: u32,
    /// The project's own thresholds. `None` means the project has not stated
    /// any, which is different from stating Draft's defaults: only in the
    /// `None` case do contributed thresholds apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<RiskThresholds>,
    #[serde(default)]
    pub rules: Vec<RiskRule>,
}

impl crate::contracts::VersionedContract for RiskConfig {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::RiskConfig;
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::RiskConfig,
            ),
            thresholds: None,
            rules: Vec::new(),
        }
    }
}

/// What the change set looks like, in terms Core can compute without any domain
/// knowledge.
///
/// Everything here is either counted from the authoritative transition, read
/// from a contributed metric, or a state Draft itself decided.
#[derive(Debug, Clone, Default)]
pub struct RiskFacts {
    /// How many changed resources matched each contributed predicate. Populated
    /// by the caller, which owns classification and can evaluate predicates.
    pub resources_matching: BTreeMap<String, u32>,
    /// How many changed resources carry each neutral aspect.
    pub aspect_counts: BTreeMap<ChangeAspectName, u32>,
    pub resource_count: u64,
    /// Metrics a representation summary supplied. Keys are contributed; Core
    /// never interprets them.
    pub change_metrics: BTreeMap<String, i64>,
    /// How many elements matched each contributed element predicate.
    pub elements_matching: BTreeMap<String, u32>,
    pub verification: Option<VerificationStateName>,
    pub intent: Option<NamespacedId>,
    pub imported: bool,
    pub agent_produced: bool,
    /// Rollback rate of the producing candidate, in permille. Integral so it
    /// participates in a canonical hash unambiguously.
    pub candidate_rollback_permille: u32,
    pub identity_uncertain: u32,
    pub observation_gaps: u32,
    pub derivation_gaps: u32,
    pub recovery_unanchored: u32,
    pub boundary_violations: u32,
}

/// One rule that fired, and who contributed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskRuleResult {
    pub code: NamespacedId,
    pub weight: i32,
    pub explanation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_action: Option<String>,
    /// Absent for a rule that came from project configuration rather than an
    /// extension. Configuration is the project's own voice, not a producer's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<ProducerRef>,
}

/// The outcome of assessing risk.
///
/// `Unassessed` is a first-class result, not an error and not a zero score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RiskOutcome {
    /// No applicable rule set. Never scored, and never reported as `Low`.
    Unassessed {
        reason: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        gaps: Vec<crate::extension::CapabilityGap>,
    },
    Assessed {
        level: RiskLevel,
        score: u32,
        explanations: Vec<String>,
        required_actions: Vec<String>,
        rule_results: Vec<RiskRuleResult>,
    },
}

impl RiskOutcome {
    /// The level, when one was actually determined.
    ///
    /// Returns `None` for an unassessed outcome rather than a default, so a
    /// caller cannot accidentally treat "not assessed" as "low".
    pub fn level(&self) -> Option<RiskLevel> {
        match self {
            Self::Unassessed { .. } => None,
            Self::Assessed { level, .. } => Some(*level),
        }
    }

    pub fn is_assessed(&self) -> bool {
        matches!(self, Self::Assessed { .. })
    }
}

/// The explainable risk assessment persisted to `risk.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskAssessment {
    pub schema_version: u32,
    pub change_pack_id: String,
    pub content_revision_id: String,
    pub content_revision_digest: String,
    pub dependency_digests: Vec<String>,
    /// Exactly what this assessment consumed. An artifact it never read cannot
    /// invalidate it.
    pub inputs: DerivationInputs,
    /// The Core semantics that produced the score.
    pub aggregator_revision: u32,
    pub outcome: RiskOutcome,
    /// ML-ready feature vector (advisory; never the sole blocker).
    pub feature_vector: Vec<f64>,
}

impl crate::contracts::VersionedContract for RiskAssessment {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::RiskAssessment;
}

impl RiskAssessment {
    pub fn validate_binding(&self, revision: &ChangePackContentRevisionRecord) -> DraftResult<()> {
        if !crate::contracts::supports_version(
            crate::contracts::ContractId::RiskAssessment,
            self.schema_version,
        ) {
            return Err(DraftError::new(
                DraftErrorKind::UnsupportedSchema,
                format!(
                    "risk evidence schema {} is unsupported",
                    self.schema_version
                ),
            ));
        }
        if self.change_pack_id != revision.change_pack_id
            || self.content_revision_id != revision.content_revision_id
            || self.content_revision_digest != revision.content_revision_digest
            || self.dependency_digests != revision.resolved_dependency_digests
        {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "risk evidence is bound to a different ChangePack revision or dependencies",
            ));
        }
        Ok(())
    }
}

/// The user-facing projection of one assessment, linked to its signed receipt.
///
/// `level` is optional for the same reason [`RiskOutcome::level`] is: a
/// projection that defaulted an unassessed change to `Low` would put the very
/// confusion this model removes back into the surface humans read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskSummary {
    pub change_pack_id: String,
    pub receipt_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<RiskLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<u32>,
    pub assessed: bool,
    pub factors: Vec<String>,
    pub reason_codes: Vec<String>,
    /// Resources the matched rules pointed at. Locators, not paths: what they
    /// mean is the owning adapter's business.
    pub hotspots: Vec<crate::dcg::resource::ResourceLocator>,
    pub evidence_gaps: Vec<String>,
    #[serde(default)]
    pub evidence_summary: Vec<String>,
    pub policy_decision: String,
    pub resources_changed: usize,
}

impl RiskSummary {
    /// Project one outcome for display.
    pub fn from_outcome(
        change_pack_id: impl Into<String>,
        receipt_id: impl Into<String>,
        outcome: &RiskOutcome,
        hotspots: Vec<crate::dcg::resource::ResourceLocator>,
        resources_changed: usize,
        policy_decision: impl Into<String>,
    ) -> Self {
        let (level, score, factors, reason_codes, assessed) = match outcome {
            RiskOutcome::Unassessed { reason, gaps } => (
                None,
                None,
                vec![reason.clone()],
                gaps.iter().map(|gap| gap.gap_id()).collect(),
                false,
            ),
            RiskOutcome::Assessed {
                level,
                score,
                explanations,
                rule_results,
                ..
            } => (
                Some(*level),
                Some(*score),
                explanations.clone(),
                rule_results
                    .iter()
                    .map(|result| result.code.qualified())
                    .collect(),
                true,
            ),
        };
        Self {
            change_pack_id: change_pack_id.into(),
            receipt_id: receipt_id.into(),
            level,
            score,
            assessed,
            factors,
            reason_codes,
            hotspots,
            evidence_gaps: Vec::new(),
            evidence_summary: Vec::new(),
            policy_decision: policy_decision.into(),
            resources_changed,
        }
    }
}

/// One contributed rule, with its origin.
#[derive(Debug, Clone)]
pub struct ApplicableRule<'a> {
    pub rule: &'a RiskRule,
    pub producer: Option<ProducerRef>,
}

/// Assess risk from contributed rules and neutral facts.
///
/// With no rules the outcome is `Unassessed`. That is the honest answer: Draft
/// has no built-in notion of what makes a change risky in an arbitrary domain,
/// and inventing a `Low` would let a submission gate pass on an assumption
/// nobody made.
pub fn assess(
    rules: &[ApplicableRule<'_>],
    facts: &RiskFacts,
    thresholds: RiskThresholds,
    gaps: Vec<crate::extension::CapabilityGap>,
) -> RiskOutcome {
    if rules.is_empty() {
        return RiskOutcome::Unassessed {
            reason: "no risk rules are configured or contributed for this project".into(),
            gaps,
        };
    }

    let mut score: i32 = 0;
    let mut explanations = Vec::new();
    let mut required_actions = Vec::new();
    let mut rule_results = Vec::new();

    for applicable in rules {
        if !condition_holds(&applicable.rule.when, facts) {
            continue;
        }
        score += applicable.rule.weight;
        explanations.push(applicable.rule.explanation.clone());
        if let Some(action) = &applicable.rule.required_action {
            required_actions.push(action.clone());
        }
        rule_results.push(RiskRuleResult {
            code: applicable.rule.code.clone(),
            weight: applicable.rule.weight,
            explanation: applicable.rule.explanation.clone(),
            required_action: applicable.rule.required_action.clone(),
            producer: applicable.producer.clone(),
        });
    }

    let score = score.clamp(0, 100) as u32;
    let level = thresholds.level_for(score);
    if matches!(level, RiskLevel::High | RiskLevel::Critical) {
        required_actions.push("require human approval before promotion".to_string());
    }
    if explanations.is_empty() {
        explanations.push("no contributed risk rule matched this change".to_string());
    }
    RiskOutcome::Assessed {
        level,
        score,
        explanations,
        required_actions: dedup(required_actions),
        rule_results,
    }
}

/// Whether one contributed condition holds against the neutral facts.
fn condition_holds(condition: &RiskCondition, facts: &RiskFacts) -> bool {
    match condition {
        RiskCondition::All { of } => of.iter().all(|c| condition_holds(c, facts)),
        RiskCondition::Any { of } => of.iter().any(|c| condition_holds(c, facts)),
        RiskCondition::Not { of } => !condition_holds(of, facts),
        RiskCondition::ResourcesMatching {
            predicate,
            at_least,
        } => {
            let key = predicate_key(predicate);
            facts.resources_matching.get(&key).copied().unwrap_or(0) >= *at_least
        }
        RiskCondition::AspectCount { aspect, at_least } => {
            facts.aspect_counts.get(aspect).copied().unwrap_or(0) >= *at_least
        }
        RiskCondition::ResourceCount { at_least, at_most } => {
            at_least.is_none_or(|bound| facts.resource_count >= bound)
                && at_most.is_none_or(|bound| facts.resource_count <= bound)
        }
        RiskCondition::ChangeMetric { metric, at_least } => facts
            .change_metrics
            .get(metric)
            .is_some_and(|value| value >= at_least),
        RiskCondition::ElementsMatching {
            predicate,
            at_least,
        } => {
            let key = element_predicate_key(predicate);
            facts.elements_matching.get(&key).copied().unwrap_or(0) >= *at_least
        }
        RiskCondition::EvidenceState { verification } => facts.verification == Some(*verification),
        RiskCondition::IntentIs { intent } => facts.intent.as_ref() == Some(intent),
        RiskCondition::Provenance {
            imported,
            agent_produced,
        } => {
            imported.is_none_or(|expected| facts.imported == expected)
                && agent_produced.is_none_or(|expected| facts.agent_produced == expected)
        }
        RiskCondition::CandidateHistory {
            rollback_rate_permille_at_least,
        } => facts.candidate_rollback_permille >= *rollback_rate_permille_at_least,
        RiskCondition::IdentityUncertain { at_least } => facts.identity_uncertain >= *at_least,
        RiskCondition::ObservationGaps { at_least } => facts.observation_gaps >= *at_least,
        RiskCondition::DerivationGaps { at_least } => facts.derivation_gaps >= *at_least,
        RiskCondition::RecoveryUnanchored { at_least } => facts.recovery_unanchored >= *at_least,
        RiskCondition::BoundaryViolation { at_least } => facts.boundary_violations >= *at_least,
    }
}

/// A stable key for one predicate, so the caller and the evaluator agree on
/// which count belongs to which rule without Core evaluating the predicate here.
pub fn predicate_key(predicate: &draft_extension_contract::ResourcePredicate) -> String {
    crate::support::hashing::canonical_hash(predicate)
}

/// The same, for element predicates.
pub fn element_predicate_key(predicate: &draft_extension_contract::ElementPredicate) -> String {
    crate::support::hashing::canonical_hash(predicate)
}

/// ML-ready feature vector. Advisory only, and never the sole blocker.
///
/// Every axis is domain-neutral: counts, gaps and provenance flags, not
/// software metrics.
pub fn feature_vector(facts: &RiskFacts) -> Vec<f64> {
    vec![
        facts.resource_count as f64,
        facts
            .aspect_counts
            .values()
            .copied()
            .map(f64::from)
            .sum::<f64>(),
        facts.identity_uncertain as f64,
        facts.observation_gaps as f64,
        facts.derivation_gaps as f64,
        facts.recovery_unanchored as f64,
        facts.boundary_violations as f64,
        facts.imported as u8 as f64,
        facts.agent_produced as u8 as f64,
        f64::from(facts.candidate_rollback_permille) / 1000.0,
    ]
}

/// The dependency header for an assessment over one change set.
pub fn inputs_for(
    change_set_digest: &str,
    consumed: impl IntoIterator<Item = (DerivedArtifactKind, String)>,
) -> DerivationInputs {
    DerivationInputs::new(
        crate::provenance::derived::SubjectRef::ChangeSet {
            change_set_digest: change_set_digest.to_string(),
        },
        consumed
            .into_iter()
            .map(|(kind, digest)| crate::provenance::derived::DerivedArtifactRef { kind, digest }),
    )
}

fn dedup(mut values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_extension_contract::{RawResourcePredicate, ResourcePredicate};

    fn id(value: &str) -> NamespacedId {
        NamespacedId::parse(value).unwrap()
    }

    fn rule(code: &str, weight: i32, when: RiskCondition) -> RiskRule {
        RiskRule {
            code: id(code),
            weight,
            when,
            explanation: format!("{code} matched"),
            required_action: None,
        }
    }

    fn applicable<'a>(rules: &'a [RiskRule]) -> Vec<ApplicableRule<'a>> {
        rules
            .iter()
            .map(|rule| ApplicableRule {
                rule,
                producer: None,
            })
            .collect()
    }

    #[test]
    fn core_ships_no_risk_rules() {
        // Sensitivity is domain vocabulary. A default rule set would silently
        // impose one project's idea of "risky" on every other domain.
        assert!(RiskConfig::default().rules.is_empty());
    }

    #[test]
    fn no_rules_means_unassessed_and_never_low() {
        let outcome = assess(
            &[],
            &RiskFacts::default(),
            RiskThresholds::default(),
            vec![],
        );
        assert!(!outcome.is_assessed());
        assert_eq!(
            outcome.level(),
            None,
            "an unassessed outcome must not resolve to a level a gate could accept"
        );
        match outcome {
            RiskOutcome::Unassessed { reason, .. } => assert!(reason.contains("no risk rules")),
            other => panic!("expected Unassessed, got {other:?}"),
        }
    }

    #[test]
    fn a_matching_rule_scores_and_explains_itself() {
        let rules = vec![rule(
            "ex.pub/large",
            30,
            RiskCondition::ResourceCount {
                at_least: Some(20),
                at_most: None,
            },
        )];
        let facts = RiskFacts {
            resource_count: 25,
            ..RiskFacts::default()
        };
        let outcome = assess(
            &applicable(&rules),
            &facts,
            RiskThresholds::default(),
            vec![],
        );
        match outcome {
            RiskOutcome::Assessed {
                level,
                score,
                rule_results,
                explanations,
                ..
            } => {
                assert_eq!(score, 30);
                assert_eq!(level, RiskLevel::Medium);
                assert_eq!(rule_results.len(), 1);
                assert_eq!(rule_results[0].code, id("ex.pub/large"));
                assert!(explanations
                    .iter()
                    .any(|line| line.contains("ex.pub/large")));
            }
            other => panic!("expected Assessed, got {other:?}"),
        }
    }

    #[test]
    fn rules_that_do_not_match_produce_an_assessed_low() {
        // Distinct from Unassessed: rules exist, they were evaluated, and none
        // fired.
        let rules = vec![rule(
            "ex.pub/large",
            30,
            RiskCondition::ResourceCount {
                at_least: Some(20),
                at_most: None,
            },
        )];
        let outcome = assess(
            &applicable(&rules),
            &RiskFacts::default(),
            RiskThresholds::default(),
            vec![],
        );
        assert_eq!(outcome.level(), Some(RiskLevel::Low));
        assert!(outcome.is_assessed());
    }

    #[test]
    fn every_condition_is_domain_neutral_and_reads_contributed_facts() {
        let predicate = ResourcePredicate::Raw {
            of: RawResourcePredicate::LocatorScheme {
                equals: "catalog".into(),
            },
        };
        let rules = vec![
            rule(
                "ex.pub/matched",
                10,
                RiskCondition::ResourcesMatching {
                    predicate: predicate.clone(),
                    at_least: 2,
                },
            ),
            rule(
                "ex.pub/metric",
                10,
                RiskCondition::ChangeMetric {
                    metric: "ex.pub/units".into(),
                    at_least: 100,
                },
            ),
            rule(
                "ex.pub/intent",
                10,
                RiskCondition::IntentIs {
                    intent: id("ex.pub/risky"),
                },
            ),
            rule(
                "ex.pub/gaps",
                10,
                RiskCondition::DerivationGaps { at_least: 1 },
            ),
        ];
        let facts = RiskFacts {
            resources_matching: BTreeMap::from([(predicate_key(&predicate), 3)]),
            change_metrics: BTreeMap::from([("ex.pub/units".to_string(), 150)]),
            intent: Some(id("ex.pub/risky")),
            derivation_gaps: 2,
            ..RiskFacts::default()
        };
        let outcome = assess(
            &applicable(&rules),
            &facts,
            RiskThresholds::default(),
            vec![],
        );
        // All four conditions fire, one per rule, and the level follows the
        // thresholds rather than any rule declaring one.
        match outcome {
            RiskOutcome::Assessed {
                score,
                level,
                rule_results,
                ..
            } => {
                assert_eq!(score, 40);
                assert_eq!(rule_results.len(), 4);
                assert_eq!(level, RiskThresholds::default().level_for(40));
                assert_eq!(level, RiskLevel::Medium);
            }
            other => panic!("expected Assessed, got {other:?}"),
        }
    }

    #[test]
    fn an_unmatched_metric_key_is_absence_not_zero() {
        // Core does not know what a contributed metric means, so a missing key
        // must not be read as a value of zero that happens to satisfy a bound.
        let rules = vec![rule(
            "ex.pub/metric",
            50,
            RiskCondition::ChangeMetric {
                metric: "ex.pub/units".into(),
                at_least: -1,
            },
        )];
        let outcome = assess(
            &applicable(&rules),
            &RiskFacts::default(),
            RiskThresholds::default(),
            vec![],
        );
        assert_eq!(outcome.level(), Some(RiskLevel::Low));
    }

    #[test]
    fn a_rule_result_names_the_publisher_that_contributed_it() {
        let rules = [rule(
            "ex.pub/always",
            10,
            RiskCondition::ResourceCount {
                at_least: None,
                at_most: None,
            },
        )];
        let producer = ProducerRef {
            extension_id: "ex.pub".into(),
            extension_version: "1.0.0".into(),
            package_digest: "sha256:pkg".into(),
            attestation_digest: "sha256:att".into(),
        };
        let with_producer = vec![ApplicableRule {
            rule: &rules[0],
            producer: Some(producer.clone()),
        }];
        match assess(
            &with_producer,
            &RiskFacts::default(),
            RiskThresholds::default(),
            vec![],
        ) {
            RiskOutcome::Assessed { rule_results, .. } => {
                assert_eq!(rule_results[0].producer, Some(producer));
            }
            other => panic!("expected Assessed, got {other:?}"),
        }
    }

    #[test]
    fn thresholds_are_configurable_and_bands_are_ordered() {
        let strict = RiskThresholds {
            medium: 5,
            high: 10,
            critical: 15,
        };
        assert_eq!(strict.level_for(0), RiskLevel::Low);
        assert_eq!(strict.level_for(5), RiskLevel::Medium);
        assert_eq!(strict.level_for(10), RiskLevel::High);
        assert_eq!(strict.level_for(99), RiskLevel::Critical);
    }
}
