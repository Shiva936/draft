//! What Draft requires before a change may be accepted, and whether it is met.
//!
//! Two things are kept rigorously apart here, because conflating them is how a
//! change-control system starts invalidating work for no reason:
//!
//! * **[`AcceptanceContext`]** — the *requirements*. Protections, verification
//!   gates, risk thresholds, reviewability budgets, waiver and approval rules,
//!   gap tolerance, recovery-readiness policy. Nothing about any particular
//!   change, candidate or snapshot.
//! * **[`AcceptanceEvaluation`]** — whether *this* change, with the evidence and
//!   human decisions that actually exist, meets them right now.
//!
//! Because the context contains no subject identity, tightening a policy cannot
//! supersede a candidate, manufacture a change, or alter a `ChangeSet` digest.
//! It invalidates a *readiness answer*, which is exactly what changed.
//!
//! Human decisions are immutable historical facts. Re-evaluating never asks
//! again for something already decided under requirements that still read the
//! same: each requirement carries its own digest, and a decision recorded
//! against that digest keeps satisfying it. Strengthening one requirement asks
//! for one new decision, not a fresh round of everything.

use serde::{Deserialize, Serialize};

use crate::contracts::{current_version, ContractId, VersionedContract};
use crate::support::common::{now, Timestamp};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::canonical_hash;

/// The Core semantics that decide whether requirements are met.
///
/// Independent of every aggregator revision: a change to how risk is scored is
/// not a change to what acceptance *means*, and sharing one constant would make
/// each look like the other.
pub const ACCEPTANCE_EVALUATOR_REVISION: u32 = 1;

/// One thing that must hold before a change is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementKind {
    /// Every domain the change touches was actually established.
    ObservationComplete,
    /// Presence and absence were provable everywhere the change claims them.
    DerivationComplete,
    /// The verification aggregate satisfies the gate.
    Verification,
    /// Risk was assessed and is within threshold.
    Risk,
    /// A review happened against current evidence.
    Review,
    /// A person approved, after that review.
    Approval,
    /// No protected resource is touched.
    Protections,
    /// The change is small enough to review honestly.
    Reviewability,
    /// Enough was retained to put the prior state back.
    RecoveryReadiness,
}

impl RequirementKind {
    /// The stable id a decision or waiver names.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ObservationComplete => "observation_complete",
            Self::DerivationComplete => "derivation_complete",
            Self::Verification => "verification",
            Self::Risk => "risk",
            Self::Review => "review",
            Self::Approval => "approval",
            Self::Protections => "protections",
            Self::Reviewability => "reviewability",
            Self::RecoveryReadiness => "recovery_readiness",
        }
    }

    /// Whether an explicit waiver can stand in for this requirement.
    ///
    /// One source of truth, because two would eventually disagree: the context
    /// digest commits to this set through its waiver policy, and the evaluator
    /// applies it. If the evaluator hardcoded a different answer, a project
    /// could carry a context claiming a requirement was waivable and an
    /// evaluator that refused every waiver for it — and the digest would say
    /// nothing was wrong.
    ///
    /// Three are not waivable, for two different reasons. A protection exists
    /// precisely to be the thing nobody can wave through in a hurry. Review and
    /// approval are the human judgements themselves: a waiver *is* a human
    /// judgement, so allowing one to stand in for them would mean a person
    /// signing off on not having to sign off.
    pub fn is_waivable(self) -> bool {
        !matches!(self, Self::Protections | Self::Review | Self::Approval)
    }

    pub const ALL: &'static [Self] = &[
        Self::ObservationComplete,
        Self::DerivationComplete,
        Self::Verification,
        Self::Risk,
        Self::Review,
        Self::Approval,
        Self::Protections,
        Self::Reviewability,
        Self::RecoveryReadiness,
    ];
}

/// The requirements in force, and nothing about any particular change.
///
/// Every field is a digest of one policy half. Keeping them separate is what
/// lets a decision survive a policy edit that did not touch the requirement it
/// answered: only the halves that actually changed invalidate anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceContext {
    pub schema_version: u32,
    /// Part of the identity: a change to how requirements are judged is a
    /// change to what acceptance means, even with identical policy text.
    pub evaluator_revision: u32,
    pub protection_policy_digest: String,
    pub verification_gate_digest: String,
    pub required_evidence_digest: String,
    pub risk_threshold_digest: String,
    pub reviewability_policy_digest: String,
    pub waiver_policy_digest: String,
    pub approval_policy_digest: String,
    pub observation_gap_policy_digest: String,
    pub derivation_gap_policy_digest: String,
    pub recovery_readiness_policy_digest: String,
    /// The tolerances the digests above already commit to, kept readable.
    ///
    /// The evaluator has to consult these, and a digest cannot be consulted.
    /// Leaving them out meant the context could commit to "incomplete
    /// observation is tolerated here" while the evaluator required completeness
    /// anyway — a policy nothing enforced, and a digest that named it. They are
    /// not extra identity: each is already inside its own policy-half digest,
    /// so carrying them changes nothing about what the context *is*.
    pub allow_incomplete_observation: bool,
    pub allow_derivation_gaps: bool,
    pub require_full_recovery: bool,
    pub context_digest: String,
}

impl VersionedContract for AcceptanceContext {
    const CONTRACT: ContractId = ContractId::AcceptanceContext;
}

impl AcceptanceContext {
    /// Assemble a context from the effective acceptance semantics.
    ///
    /// Deliberately takes only policy. There is no parameter here through which
    /// a snapshot, a change set or a candidate could reach the digest, which is
    /// what makes "a policy change never supersedes work" structural rather
    /// than a rule someone has to remember.
    pub fn build(policy: &crate::project::policy::Policy, inputs: &AcceptanceInputs) -> Self {
        let protection_policy_digest = canonical_hash(&inputs.protections);
        let verification_gate_digest = canonical_hash(&serde_json::json!({
            "require_full": policy.require_full_verify_intents.clone(),
            "require_exploratory": policy.require_fuzz_intents.clone(),
            "reverify_on_change": policy.require_reverify_on_workspace_change,
            "local_verify_for_imports": policy.require_local_verify_for_imports,
            "gate": inputs.verification_gate,
        }));
        let required_evidence_digest = canonical_hash(&inputs.required_evidence);
        let risk_threshold_digest = canonical_hash(&serde_json::json!({
            "thresholds": inputs.risk_thresholds,
            "block_on_critical": policy.block_on_critical_risk,
        }));
        let reviewability_policy_digest = canonical_hash(&inputs.reviewability_budget);
        let waiver_policy_digest = canonical_hash(&serde_json::json!({
            "waivers_expire": true,
            "waivable": inputs.waivable_requirements,
        }));
        let approval_policy_digest = canonical_hash(&serde_json::json!({
            "require_approval_for_promotion": policy.require_approval_for_promotion,
            "require_approval_on_high_risk": policy.require_approval_on_high_risk,
        }));
        let observation_gap_policy_digest = canonical_hash(&serde_json::json!({
            "allow_incomplete_observation": inputs.allow_incomplete_observation,
        }));
        let derivation_gap_policy_digest = canonical_hash(&serde_json::json!({
            "allow_derivation_gaps": inputs.allow_derivation_gaps,
        }));
        let recovery_readiness_policy_digest = canonical_hash(&serde_json::json!({
            "require_full_recovery": inputs.require_full_recovery,
        }));

        let mut context = Self {
            schema_version: current_version(ContractId::AcceptanceContext),
            evaluator_revision: ACCEPTANCE_EVALUATOR_REVISION,
            protection_policy_digest,
            verification_gate_digest,
            required_evidence_digest,
            risk_threshold_digest,
            reviewability_policy_digest,
            waiver_policy_digest,
            approval_policy_digest,
            observation_gap_policy_digest,
            derivation_gap_policy_digest,
            recovery_readiness_policy_digest,
            allow_incomplete_observation: inputs.allow_incomplete_observation,
            allow_derivation_gaps: inputs.allow_derivation_gaps,
            require_full_recovery: inputs.require_full_recovery,
            context_digest: String::new(),
        };
        context.context_digest = context.compute_digest();
        context
    }

    fn compute_digest(&self) -> String {
        canonical_hash(&serde_json::json!({
            "evaluator_revision": self.evaluator_revision,
            "protection_policy_digest": self.protection_policy_digest,
            "verification_gate_digest": self.verification_gate_digest,
            "required_evidence_digest": self.required_evidence_digest,
            "risk_threshold_digest": self.risk_threshold_digest,
            "reviewability_policy_digest": self.reviewability_policy_digest,
            "waiver_policy_digest": self.waiver_policy_digest,
            "approval_policy_digest": self.approval_policy_digest,
            "observation_gap_policy_digest": self.observation_gap_policy_digest,
            "derivation_gap_policy_digest": self.derivation_gap_policy_digest,
            "recovery_readiness_policy_digest": self.recovery_readiness_policy_digest,
        }))
    }

    /// The digest a decision about `kind` is recorded against.
    ///
    /// This is the mechanism behind "ask again only for what actually changed":
    /// a prior human decision keeps satisfying its requirement while this value
    /// keeps reading the same, however much unrelated policy moved.
    pub fn requirement_digest(&self, kind: RequirementKind) -> String {
        let half = match kind {
            RequirementKind::ObservationComplete => &self.observation_gap_policy_digest,
            RequirementKind::DerivationComplete => &self.derivation_gap_policy_digest,
            RequirementKind::Verification => &self.verification_gate_digest,
            RequirementKind::Risk => &self.risk_threshold_digest,
            RequirementKind::Review => &self.required_evidence_digest,
            RequirementKind::Approval => &self.approval_policy_digest,
            RequirementKind::Protections => &self.protection_policy_digest,
            RequirementKind::Reviewability => &self.reviewability_policy_digest,
            RequirementKind::RecoveryReadiness => &self.recovery_readiness_policy_digest,
        };
        // The evaluator revision participates: the same policy text judged by
        // different semantics is a different requirement.
        canonical_hash(&serde_json::json!({
            "requirement": kind.as_str(),
            "policy": half,
            "evaluator_revision": self.evaluator_revision,
        }))
    }

    /// Refuse an evaluation that was made against different requirements.
    pub fn require_current(&self, evaluation: &AcceptanceEvaluation) -> DraftResult<()> {
        if evaluation.acceptance_context_digest == self.context_digest {
            return Ok(());
        }
        Err(DraftError::new(
            DraftErrorKind::EvidenceStale,
            "the acceptance evaluation was made against requirements that have since changed",
        )
        .with_suggestion("re-run readiness; only newly unmet requirements will need a decision"))
    }
}

/// The effective acceptance semantics, gathered from policy and contributions.
///
/// A plain input record rather than something clever: what matters is that
/// everything acceptance depends on is listed in one place, so a new policy
/// dimension cannot be added without deciding which requirement it belongs to.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AcceptanceInputs {
    pub protections: Vec<crate::project::protected::ProtectionRule>,
    pub verification_gate: VerificationGate,
    pub required_evidence: Vec<String>,
    pub risk_thresholds: Option<crate::evidence::risk::RiskThresholds>,
    pub reviewability_budget: crate::gate::reviewability::ProjectReviewabilityBudget,
    pub waivable_requirements: Vec<String>,
    pub allow_incomplete_observation: bool,
    pub allow_derivation_gaps: bool,
    pub require_full_recovery: bool,
}

/// Which verification aggregates satisfy the gate on their own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationGate {
    /// Only `Passed`. Every other state needs an explicit waiver.
    #[default]
    RequirePassed,
}

/// Where one requirement's policy came from.
///
/// Kept out of the context digest on purpose: two package revisions with
/// identical policy semantics must produce the same requirements — and
/// therefore keep prior decisions valid — while history still records which
/// artifact each came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum AcceptancePolicySource {
    /// Draft's own defaults or the project's `config.toml`.
    Core {
        component: String,
        implementation_revision: u32,
    },
    /// A contributed `control_policy`.
    Extension {
        producer: crate::extension::provenance::ProducerRef,
    },
}

/// Which artifacts one acceptance context was assembled from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceContextProvenance {
    pub schema_version: u32,
    pub acceptance_context_digest: String,
    /// Sorted, so two assemblies of the same sources agree.
    pub sources: Vec<AcceptancePolicySource>,
    pub assembled_at: Timestamp,
    pub provenance_digest: String,
}

impl VersionedContract for AcceptanceContextProvenance {
    const CONTRACT: ContractId = ContractId::AcceptanceContextProvenance;
}

impl AcceptanceContextProvenance {
    pub fn build(
        acceptance_context_digest: String,
        mut sources: Vec<AcceptancePolicySource>,
    ) -> Self {
        sources.sort_by_key(|source| match source {
            AcceptancePolicySource::Core { component, .. } => format!("core:{component}"),
            AcceptancePolicySource::Extension { producer } => {
                format!("ext:{}", producer.extension_id)
            }
        });
        let assembled_at = now();
        let provenance_digest = canonical_hash(&serde_json::json!({
            "acceptance_context_digest": acceptance_context_digest,
            "sources": sources,
        }));
        Self {
            schema_version: current_version(ContractId::AcceptanceContextProvenance),
            acceptance_context_digest,
            sources,
            assembled_at,
            provenance_digest,
        }
    }
}

/// Whether one requirement is met, and what met it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceRequirement {
    pub kind: RequirementKind,
    /// The digest this requirement is judged against. A decision naming it
    /// stays valid while it does.
    pub requirement_digest: String,
    pub satisfied: bool,
    /// What satisfied it — a receipt, an approval reference, a waiver id — or
    /// why it is not satisfied.
    pub detail: String,
    /// The immutable human decision reused, when one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub satisfied_by: Option<String>,
    /// Whether an explicit waiver could satisfy this.
    pub waivable: bool,
}

/// Whether this change meets the current requirements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceEvaluation {
    pub schema_version: u32,
    pub evaluator_revision: u32,
    pub change_id: String,
    /// The transition being accepted. Recorded, never an input to the context.
    pub change_set_digest: String,
    pub acceptance_context_digest: String,
    pub requirements: Vec<AcceptanceRequirement>,
    /// Only what is still missing. A decision already made under an unchanged
    /// requirement never appears here.
    pub missing_actions: Vec<String>,
    pub satisfied: bool,
    pub evaluated_at: Timestamp,
    pub evaluation_digest: String,
}

impl VersionedContract for AcceptanceEvaluation {
    const CONTRACT: ContractId = ContractId::AcceptanceEvaluation;
}

impl AcceptanceEvaluation {
    /// Seal the evaluation, deriving its identity from what it concluded.
    pub fn seal(mut self) -> Self {
        self.satisfied = self
            .requirements
            .iter()
            .all(|requirement| requirement.satisfied);
        self.missing_actions = self
            .requirements
            .iter()
            .filter(|requirement| !requirement.satisfied)
            .map(|requirement| format!("{}: {}", requirement.kind.as_str(), requirement.detail))
            .collect();
        self.evaluation_digest.clear();
        self.evaluation_digest = canonical_hash(&serde_json::json!({
            "evaluator_revision": self.evaluator_revision,
            "change_id": self.change_id,
            "change_set_digest": self.change_set_digest,
            "acceptance_context_digest": self.acceptance_context_digest,
            "requirements": self.requirements,
            "satisfied": self.satisfied,
        }));
        self
    }

    /// The requirement of a given kind, if it was evaluated.
    pub fn requirement(&self, kind: RequirementKind) -> Option<&AcceptanceRequirement> {
        self.requirements
            .iter()
            .find(|requirement| requirement.kind == kind)
    }
}

/// A human decision, as acceptance sees it.
///
/// The requirement digest is what makes reuse decidable. Without it, a
/// re-evaluation could only compare whole contexts and would have to discard
/// every decision whenever any policy moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedDecision {
    pub kind: RequirementKind,
    pub reference: String,
    pub requirement_digest: String,
}

/// Whether an existing decision still answers a current requirement.
pub fn decision_still_satisfies(
    decision: &RecordedDecision,
    context: &AcceptanceContext,
    kind: RequirementKind,
) -> bool {
    decision.kind == kind && decision.requirement_digest == context.requirement_digest(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> AcceptanceInputs {
        AcceptanceInputs::default()
    }

    fn policy() -> crate::project::policy::Policy {
        crate::project::policy::Policy::safe_default()
    }

    #[test]
    fn the_context_cannot_see_the_change_it_judges() {
        // Structural, not a convention: `build` takes policy and policy alone,
        // so there is no parameter through which a candidate or change set
        // could reach the digest. This is why tightening a policy can never
        // supersede work.
        let first = AcceptanceContext::build(&policy(), &inputs());
        let second = AcceptanceContext::build(&policy(), &inputs());
        assert_eq!(first.context_digest, second.context_digest);
        let encoded = serde_json::to_value(&first).unwrap();
        for forbidden in [
            "change_set_digest",
            "snapshot_digest",
            "change_id",
            "candidate_id",
        ] {
            assert!(
                encoded.get(forbidden).is_none(),
                "acceptance requirements must not carry {forbidden}"
            );
        }
    }

    #[test]
    fn a_policy_change_only_invalidates_the_requirement_it_touched() {
        let before = AcceptanceContext::build(&policy(), &inputs());

        let tightened = AcceptanceInputs {
            risk_thresholds: Some(crate::evidence::risk::RiskThresholds {
                medium: 2,
                high: 4,
                critical: 6,
            }),
            ..inputs()
        };
        let after = AcceptanceContext::build(&policy(), &tightened);

        // The whole context moved, so readiness is stale...
        assert_ne!(before.context_digest, after.context_digest);
        // ...but only risk actually changed. An approval given yesterday still
        // answers the approval requirement, and asking for it again would be
        // asking a person to re-decide something nobody altered.
        assert_ne!(
            before.requirement_digest(RequirementKind::Risk),
            after.requirement_digest(RequirementKind::Risk)
        );
        for untouched in [
            RequirementKind::Approval,
            RequirementKind::Verification,
            RequirementKind::Review,
            RequirementKind::Protections,
        ] {
            assert_eq!(
                before.requirement_digest(untouched),
                after.requirement_digest(untouched),
                "{} must not be disturbed by a risk-threshold edit",
                untouched.as_str()
            );
        }
    }

    #[test]
    fn a_decision_is_reused_exactly_while_its_requirement_reads_the_same() {
        let before = AcceptanceContext::build(&policy(), &inputs());
        let approval = RecordedDecision {
            kind: RequirementKind::Approval,
            reference: "rcpt_1".into(),
            requirement_digest: before.requirement_digest(RequirementKind::Approval),
        };

        let tightened = AcceptanceInputs {
            risk_thresholds: Some(crate::evidence::risk::RiskThresholds {
                medium: 1,
                high: 2,
                critical: 3,
            }),
            ..inputs()
        };
        let after = AcceptanceContext::build(&policy(), &tightened);
        assert!(decision_still_satisfies(
            &approval,
            &after,
            RequirementKind::Approval
        ));

        // Change the approval rule itself and the same decision no longer
        // answers it: that is a genuinely different question.
        let mut relaxed = policy();
        relaxed.require_approval_for_promotion = !relaxed.require_approval_for_promotion;
        let changed = AcceptanceContext::build(&relaxed, &inputs());
        assert!(!decision_still_satisfies(
            &approval,
            &changed,
            RequirementKind::Approval
        ));
    }

    #[test]
    fn an_evaluation_reports_only_what_is_still_missing() {
        let context = AcceptanceContext::build(&policy(), &inputs());
        let evaluation = AcceptanceEvaluation {
            schema_version: current_version(ContractId::AcceptanceEvaluation),
            evaluator_revision: ACCEPTANCE_EVALUATOR_REVISION,
            change_id: "chg_1".into(),
            change_set_digest: "chg-digest".into(),
            acceptance_context_digest: context.context_digest.clone(),
            requirements: vec![
                AcceptanceRequirement {
                    kind: RequirementKind::Approval,
                    requirement_digest: context.requirement_digest(RequirementKind::Approval),
                    satisfied: true,
                    detail: "approved in rcpt_1".into(),
                    satisfied_by: Some("rcpt_1".into()),
                    waivable: false,
                },
                AcceptanceRequirement {
                    kind: RequirementKind::Verification,
                    requirement_digest: context.requirement_digest(RequirementKind::Verification),
                    satisfied: false,
                    detail: "verification is 'unavailable'".into(),
                    satisfied_by: None,
                    waivable: true,
                },
            ],
            missing_actions: Vec::new(),
            satisfied: false,
            evaluated_at: now(),
            evaluation_digest: String::new(),
        }
        .seal();

        assert!(!evaluation.satisfied);
        assert_eq!(evaluation.missing_actions.len(), 1);
        assert!(evaluation.missing_actions[0].starts_with("verification:"));
        assert!(!evaluation.evaluation_digest.is_empty());
    }

    #[test]
    fn a_stale_evaluation_is_refused_by_the_current_requirements() {
        let context = AcceptanceContext::build(&policy(), &inputs());
        let mut evaluation = AcceptanceEvaluation {
            schema_version: current_version(ContractId::AcceptanceEvaluation),
            evaluator_revision: ACCEPTANCE_EVALUATOR_REVISION,
            change_id: "chg_1".into(),
            change_set_digest: "chg".into(),
            acceptance_context_digest: context.context_digest.clone(),
            requirements: Vec::new(),
            missing_actions: Vec::new(),
            satisfied: true,
            evaluated_at: now(),
            evaluation_digest: String::new(),
        }
        .seal();
        assert!(context.require_current(&evaluation).is_ok());

        evaluation.acceptance_context_digest = "some-older-context".into();
        let error = context.require_current(&evaluation).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::EvidenceStale);
    }

    #[test]
    fn identical_policy_from_different_artifacts_keeps_one_context_and_two_histories() {
        // The point of separating provenance from requirements: an upgraded
        // policy package that says exactly the same thing must not invalidate a
        // single human decision, while a receipt must still be able to name the
        // artifact it actually relied on.
        let context = AcceptanceContext::build(&policy(), &inputs());
        let producer = |version: &str| crate::extension::provenance::ProducerRef {
            extension_id: "draft.filesystem.policy".into(),
            extension_version: version.into(),
            package_digest: format!("pkg-{version}"),
            attestation_digest: format!("att-{version}"),
        };
        let first = AcceptanceContextProvenance::build(
            context.context_digest.clone(),
            vec![AcceptancePolicySource::Extension {
                producer: producer("1.0.0"),
            }],
        );
        let second = AcceptanceContextProvenance::build(
            context.context_digest.clone(),
            vec![AcceptancePolicySource::Extension {
                producer: producer("1.0.1"),
            }],
        );
        assert_eq!(
            first.acceptance_context_digest,
            second.acceptance_context_digest
        );
        assert_ne!(first.provenance_digest, second.provenance_digest);
    }
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// Everything the evaluator needs, gathered once by the caller.
///
/// A plain record of facts rather than a bundle of callbacks: evaluation must be
/// a pure function of what was established, so the same facts always produce the
/// same answer and a readiness result can be reproduced from what it recorded.
#[derive(Debug, Clone)]
pub struct EvaluationFacts {
    pub change_id: String,
    pub change_set_digest: String,
    /// Whether every coverage domain the change touches was established.
    pub observation_complete: bool,
    pub observation_detail: String,
    /// Whether presence and absence were provable wherever the change claims
    /// them.
    pub derivation_complete: bool,
    pub derivation_detail: String,
    pub verification: crate::evidence::verification::VerificationState,
    pub verification_receipt_id: Option<String>,
    pub risk_assessed: bool,
    pub risk_within_threshold: bool,
    pub risk_detail: String,
    pub review_receipt_id: Option<String>,
    pub approval_ref: Option<String>,
    /// Protected resources the change would touch. Empty means none.
    pub protected_violations: Vec<String>,
    pub reviewability_ok: bool,
    pub reviewability_detail: String,
    pub recovery_fully_anchored: bool,
    pub recovery_detail: String,
    /// Requirement ids a live waiver currently covers, with the waiver's id.
    pub waived: Vec<(String, String)>,
    /// Human decisions already recorded, with the requirement digest each was
    /// made against.
    pub prior_decisions: Vec<RecordedDecision>,
}

/// Judge one change against the requirements in force.
///
/// Every requirement is evaluated, including the ones that pass: a reader
/// deserves to see that verification was considered and satisfied, not merely
/// that nothing complained. `missing_actions` is derived from the failures, so
/// it can never disagree with the requirement list it summarises.
pub fn evaluate(context: &AcceptanceContext, facts: &EvaluationFacts) -> AcceptanceEvaluation {
    let waived = |kind: RequirementKind| -> Option<&String> {
        facts
            .waived
            .iter()
            .find(|(requirement, _)| requirement == kind.as_str())
            .map(|(_, waiver_id)| waiver_id)
    };

    let mut requirements = Vec::new();
    let mut push =
        |kind: RequirementKind, satisfied: bool, detail: String, satisfied_by: Option<String>| {
            let waivable = kind.is_waivable();
            // A live waiver satisfies a waivable requirement, and says so by name.
            // The requirement is not quietly removed: a reader must be able to see
            // that something was accepted *despite* not being met.
            let (satisfied, detail, satisfied_by) = match (satisfied, waivable, waived(kind)) {
                (false, true, Some(waiver_id)) => (
                    true,
                    format!("{detail} (waived by {waiver_id})"),
                    Some(waiver_id.clone()),
                ),
                _ => (satisfied, detail, satisfied_by),
            };
            requirements.push(AcceptanceRequirement {
                kind,
                requirement_digest: context.requirement_digest(kind),
                satisfied,
                detail,
                satisfied_by,
                waivable,
            });
        };

    // Both of these fail closed by default: if Draft could not see part of the
    // project, it will not treat what it could not observe as unchanged. A
    // project may declare that it tolerates the uncertainty, and then the
    // requirement is satisfied — but the detail still records exactly what
    // could not be established, because tolerating uncertainty is not the same
    // as not having any.
    push(
        RequirementKind::ObservationComplete,
        facts.observation_complete || context.allow_incomplete_observation,
        facts.observation_detail.clone(),
        None,
    );
    push(
        RequirementKind::DerivationComplete,
        facts.derivation_complete || context.allow_derivation_gaps,
        facts.derivation_detail.clone(),
        None,
    );

    // Only `Passed` satisfies the gate on its own. Every other state — including
    // `NotApplicable` — needs somebody to say, on the record, that shipping
    // without it is acceptable here.
    let verification_passed = facts.verification.satisfies_gate_unconditionally();
    push(
        RequirementKind::Verification,
        verification_passed && facts.verification_receipt_id.is_some(),
        if verification_passed && facts.verification_receipt_id.is_none() {
            "verification passed but no current receipt records it".to_string()
        } else {
            format!("verification is '{}'", facts.verification.as_str())
        },
        facts.verification_receipt_id.clone(),
    );
    push(
        RequirementKind::Risk,
        facts.risk_assessed && facts.risk_within_threshold,
        facts.risk_detail.clone(),
        None,
    );
    push(
        RequirementKind::Review,
        facts.review_receipt_id.is_some(),
        facts
            .review_receipt_id
            .clone()
            .map(|id| format!("reviewed in {id}"))
            .unwrap_or_else(|| "no current review receipt".to_string()),
        facts.review_receipt_id.clone(),
    );
    push(
        RequirementKind::Approval,
        facts.approval_ref.is_some(),
        facts
            .approval_ref
            .clone()
            .map(|reference| format!("approved in {reference}"))
            .unwrap_or_else(|| "no human approval after the current review".to_string()),
        facts.approval_ref.clone(),
    );
    push(
        RequirementKind::Protections,
        facts.protected_violations.is_empty(),
        if facts.protected_violations.is_empty() {
            "no protected resource is touched".to_string()
        } else {
            format!(
                "protected resources touched: {}",
                facts.protected_violations.join(", ")
            )
        },
        None,
    );
    push(
        RequirementKind::Reviewability,
        facts.reviewability_ok,
        facts.reviewability_detail.clone(),
        None,
    );
    push(
        RequirementKind::RecoveryReadiness,
        // Policy decides whether missing anchors are a blocker or a report.
        // Core owns the mechanism and the reporting either way, so the
        // requirement is still evaluated and still shown — it is satisfied
        // because the project said it did not require full recovery, and the
        // detail still says what the recovery status actually is.
        facts.recovery_fully_anchored || !context.require_full_recovery,
        facts.recovery_detail.clone(),
        None,
    );

    // Reuse: a requirement a person already decided, under a digest that still
    // reads the same, stays decided. This is what stops a policy edit from
    // asking somebody to re-approve something nobody changed.
    for requirement in &mut requirements {
        if requirement.satisfied {
            continue;
        }
        if let Some(decision) = facts
            .prior_decisions
            .iter()
            .find(|decision| decision_still_satisfies(decision, context, requirement.kind))
        {
            requirement.satisfied = true;
            requirement.detail = format!(
                "{} (already decided in {})",
                requirement.detail, decision.reference
            );
            requirement.satisfied_by = Some(decision.reference.clone());
        }
    }

    AcceptanceEvaluation {
        schema_version: current_version(ContractId::AcceptanceEvaluation),
        evaluator_revision: ACCEPTANCE_EVALUATOR_REVISION,
        change_id: facts.change_id.clone(),
        change_set_digest: facts.change_set_digest.clone(),
        acceptance_context_digest: context.context_digest.clone(),
        requirements,
        missing_actions: Vec::new(),
        satisfied: false,
        evaluated_at: now(),
        evaluation_digest: String::new(),
    }
    .seal()
}

#[cfg(test)]
mod evaluation_tests {
    use super::*;
    use crate::evidence::verification::VerificationState;

    fn context() -> AcceptanceContext {
        AcceptanceContext::build(
            &crate::project::policy::Policy::safe_default(),
            &AcceptanceInputs::default(),
        )
    }

    fn clean_facts() -> EvaluationFacts {
        EvaluationFacts {
            change_id: "chg_1".into(),
            change_set_digest: "chg".into(),
            observation_complete: true,
            observation_detail: "every domain established".into(),
            derivation_complete: true,
            derivation_detail: "no derivation gaps".into(),
            verification: VerificationState::Passed,
            verification_receipt_id: Some("rcpt_verify".into()),
            risk_assessed: true,
            risk_within_threshold: true,
            risk_detail: "low".into(),
            review_receipt_id: Some("rcpt_review".into()),
            approval_ref: Some("rcpt_approve".into()),
            protected_violations: Vec::new(),
            reviewability_ok: true,
            reviewability_detail: "within budget".into(),
            recovery_fully_anchored: true,
            recovery_detail: "fully anchored".into(),
            waived: Vec::new(),
            prior_decisions: Vec::new(),
        }
    }

    #[test]
    fn a_fully_satisfied_change_reports_no_missing_actions() {
        let evaluation = evaluate(&context(), &clean_facts());
        assert!(evaluation.satisfied);
        assert!(evaluation.missing_actions.is_empty());
        // Every requirement is listed, including the satisfied ones: a reader
        // should see what was considered, not just what complained.
        assert_eq!(evaluation.requirements.len(), RequirementKind::ALL.len());
    }

    #[test]
    fn zero_verification_checks_never_become_passed() {
        let context = context();
        let mut facts = clean_facts();
        facts.verification = VerificationState::NotApplicable {
            reason: "no required check applies".into(),
            evidence: Vec::new(),
        };
        let evaluation = evaluate(&context, &facts);
        let requirement = evaluation
            .requirement(RequirementKind::Verification)
            .unwrap();
        assert!(
            !requirement.satisfied,
            "'nothing applied' must not satisfy a verification gate"
        );
        assert!(!evaluation.satisfied);

        // The same holds for `unavailable`, which is the zero-extension case.
        facts.verification =
            crate::evidence::verification::VerificationState::Unavailable { gaps: Vec::new() };
        let evaluation = evaluate(&context, &facts);
        assert!(
            !evaluation
                .requirement(RequirementKind::Verification)
                .unwrap()
                .satisfied
        );
    }

    #[test]
    fn a_waiver_satisfies_a_waivable_requirement_and_says_which() {
        let mut facts = clean_facts();
        facts.verification = VerificationState::Unavailable { gaps: Vec::new() };
        facts.waived = vec![("verification".into(), "wv_1".into())];
        let evaluation = evaluate(&context(), &facts);
        let requirement = evaluation
            .requirement(RequirementKind::Verification)
            .unwrap();
        assert!(requirement.satisfied);
        // Not silently removed: the record still shows the requirement was not
        // met, and names what let it through anyway.
        assert!(requirement.detail.contains("unavailable"));
        assert!(requirement.detail.contains("wv_1"));
        assert_eq!(requirement.satisfied_by.as_deref(), Some("wv_1"));
    }

    #[test]
    fn a_human_decision_can_never_be_waived_through_either() {
        // A waiver is itself a human judgement. Accepting one in place of a
        // review or an approval would mean somebody signing off on not having
        // to sign off, which is the one substitution that empties the whole
        // requirement of meaning.
        let mut facts = clean_facts();
        facts.review_receipt_id = None;
        facts.approval_ref = None;
        facts.waived = vec![
            ("review".into(), "wv_1".into()),
            ("approval".into(), "wv_2".into()),
        ];
        let evaluation = evaluate(&context(), &facts);
        for kind in [RequirementKind::Review, RequirementKind::Approval] {
            let requirement = evaluation.requirement(kind).unwrap();
            assert!(!requirement.waivable, "{} is not waivable", kind.as_str());
            assert!(
                !requirement.satisfied,
                "{} was waived through",
                kind.as_str()
            );
        }
        assert!(!evaluation.satisfied);
    }

    #[test]
    fn the_context_decides_whether_uncertainty_blocks() {
        // Core owns the mechanism and the reporting; policy owns the gate. A
        // project that has said it tolerates incomplete observation is not
        // blocked by it — but the requirement is still evaluated, still listed,
        // and its detail still says exactly what could not be established.
        // Tolerating uncertainty is not the same as not having any.
        let mut facts = clean_facts();
        facts.observation_complete = false;
        facts.observation_detail = "2 coverage domain(s) could not be established".into();
        facts.derivation_complete = false;
        facts.derivation_detail = "2 unresolved derivation gap(s)".into();
        facts.recovery_fully_anchored = false;
        facts.recovery_detail = "partially anchored".into();

        // Default policy: uncertainty fails closed.
        let strict = evaluate(&context(), &facts);
        assert!(
            !strict
                .requirement(RequirementKind::ObservationComplete)
                .unwrap()
                .satisfied
        );
        assert!(
            !strict
                .requirement(RequirementKind::DerivationComplete)
                .unwrap()
                .satisfied
        );

        // A project that declared the tolerance is not blocked by it. Recovery
        // is the case that matters most here: full recovery is not required by
        // default, so a partially anchored state must not silently block
        // submission that was never gated on it.
        let tolerant = AcceptanceContext::build(
            &crate::project::policy::Policy::safe_default(),
            &AcceptanceInputs {
                allow_incomplete_observation: true,
                allow_derivation_gaps: true,
                ..AcceptanceInputs::default()
            },
        );
        let permitted = evaluate(&tolerant, &facts);
        let observation = permitted
            .requirement(RequirementKind::ObservationComplete)
            .unwrap();
        assert!(observation.satisfied);
        assert!(
            observation.detail.contains("could not be established"),
            "the uncertainty is still reported, not erased: {}",
            observation.detail
        );
        assert!(
            permitted
                .requirement(RequirementKind::DerivationComplete)
                .unwrap()
                .satisfied
        );
        assert!(
            permitted
                .requirement(RequirementKind::RecoveryReadiness)
                .unwrap()
                .satisfied,
            "full recovery is not required by default"
        );
    }

    #[test]
    fn a_protection_can_never_be_waived_through() {
        let mut facts = clean_facts();
        facts.protected_violations = vec![".env".into()];
        facts.waived = vec![("protections".into(), "wv_1".into())];
        let evaluation = evaluate(&context(), &facts);
        let requirement = evaluation
            .requirement(RequirementKind::Protections)
            .unwrap();
        assert!(
            !requirement.satisfied,
            "a protection exists to be the thing a waiver cannot wave through"
        );
        assert!(!evaluation.satisfied);
    }

    #[test]
    fn only_the_newly_unmet_requirement_is_asked_for_again() {
        // The scenario the whole design exists for. A risk threshold is
        // tightened; the human approval given yesterday still stands, and the
        // only thing asked for is the one thing that changed.
        let before = context();
        let approval = RecordedDecision {
            kind: RequirementKind::Approval,
            reference: "rcpt_approve".into(),
            requirement_digest: before.requirement_digest(RequirementKind::Approval),
        };

        let tightened = AcceptanceInputs {
            risk_thresholds: Some(crate::evidence::risk::RiskThresholds {
                medium: 1,
                high: 2,
                critical: 3,
            }),
            ..AcceptanceInputs::default()
        };
        let after =
            AcceptanceContext::build(&crate::project::policy::Policy::safe_default(), &tightened);

        let mut facts = clean_facts();
        facts.approval_ref = None; // the approval is a prior decision, not a live field
        facts.risk_within_threshold = false;
        facts.risk_detail = "score 5 exceeds the high threshold of 2".into();
        facts.prior_decisions = vec![approval];

        let evaluation = evaluate(&after, &facts);
        assert!(
            evaluation
                .requirement(RequirementKind::Approval)
                .unwrap()
                .satisfied
        );
        assert!(
            !evaluation
                .requirement(RequirementKind::Risk)
                .unwrap()
                .satisfied
        );
        assert_eq!(
            evaluation.missing_actions.len(),
            1,
            "only the tightened requirement should be asked for: {:?}",
            evaluation.missing_actions
        );
        assert!(evaluation.missing_actions[0].starts_with("risk:"));
    }

    #[test]
    fn a_decision_from_a_changed_requirement_is_not_reused() {
        let before = context();
        let stale = RecordedDecision {
            kind: RequirementKind::Approval,
            reference: "rcpt_old".into(),
            requirement_digest: before.requirement_digest(RequirementKind::Approval),
        };
        let mut relaxed = crate::project::policy::Policy::safe_default();
        relaxed.require_approval_for_promotion = !relaxed.require_approval_for_promotion;
        let after = AcceptanceContext::build(&relaxed, &AcceptanceInputs::default());

        let mut facts = clean_facts();
        facts.approval_ref = None;
        facts.prior_decisions = vec![stale];
        let evaluation = evaluate(&after, &facts);
        assert!(
            !evaluation
                .requirement(RequirementKind::Approval)
                .unwrap()
                .satisfied,
            "the approval rule itself changed, so the old decision answers a different question"
        );
    }
}
