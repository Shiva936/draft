//! From evidence to authority: assessment, gate, decision.
//!
//! Four kinds of fact, each answering a different question, and none of them
//! able to stand in for the next:
//!
//! ```text
//! Evidence     what was observed or run
//! Assessment   what that evidence means, and how risky it is
//! Gate         whether the required conditions are satisfied
//! Decision     whether a person authorizes the work to proceed
//! ```
//!
//! # Why an assessment cannot authorize
//!
//! An assessment is a judgement about risk. "This is low risk" is not "you may
//! promote this" — the first is an opinion about the work, the second is an
//! exercise of authority over the project. Collapsing them would mean the
//! producer that assessed the work also decided its fate, which is exactly the
//! separation review exists to create.
//!
//! # Why a satisfied gate is not an approval either
//!
//! A gate says the required conditions were met. It says nothing about whether
//! anybody wants this change. Promotion on a green gate alone would make
//! review advisory: the checks would decide, and the reviewer would be a
//! formality who could only slow things down, never stop them.
//!
//! So the gate is a *precondition* of a valid approval, and the Decision is
//! the authority. Both are required, and neither substitutes for the other.
//!
//! # Why everything binds an exact revision
//!
//! A revision is sealed content. An assessment, gate or decision that named
//! only "the change" would silently transfer to work nobody looked at the
//! moment a new revision was sealed. Each fact here binds a
//! `RevisionPackId`, and the promotion path re-checks that binding rather
//! than trusting that it was right when written.

use std::collections::BTreeSet;

use draft_dcg_contract::ids::{ActorId, AssessmentId, DecisionId, EvidenceId, RevisionPackId};
use draft_dcg_contract::security::SecurityFactRef;
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;

use crate::dcg::decision::{Decision, DecisionOutcome, DecisionStore};
use crate::evidence::assessment::{AssessedRisk, Assessment, AssessmentStore};
use crate::evidence::context::EvaluationContext;
use crate::evidence::{Evidence, EvidenceStore};
use crate::gate::waiver::{GateWaiver, GateWaiverStore};
use crate::gate::{GateCondition, GateEvaluation, GateEvaluationStore};
use crate::project::layout::DraftLayout;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// The stores the authorization chain reads and writes.
pub struct AuthorizationStores {
    pub evidence: EvidenceStore,
    pub assessments: AssessmentStore,
    /// Records that somebody looked. Separate from a Decision on purpose: a
    /// reviewer can read a revision and conclude nothing yet, and a decision
    /// with no recorded review is precisely what an audit wants to notice.
    pub reviews: crate::dcg::review::ReviewStore,
    pub gates: GateEvaluationStore,
    pub decisions: DecisionStore,
    pub waivers: GateWaiverStore,
}

impl AuthorizationStores {
    pub fn for_layout(layout: &DraftLayout) -> Self {
        Self {
            evidence: EvidenceStore::new(layout.evidence_dir()),
            assessments: AssessmentStore::new(layout.assessments_dir()),
            reviews: crate::dcg::review::ReviewStore::new(layout.reviews_dir()),
            gates: GateEvaluationStore::new(layout.gates_dir()),
            decisions: DecisionStore::new(layout.decisions_dir()),
            waivers: GateWaiverStore::new(layout.waivers_dir()),
        }
    }
}

/// Record an assessment of one revision.
///
/// The evidence is verified to cover the same revision before the assessment
/// is written: an assessment resting on evidence about a *different* revision
/// would carry a judgement across sealed content, which is the transfer every
/// binding here exists to prevent.
pub fn assess(stores: &AuthorizationStores, assessment: &Assessment) -> DraftResult<()> {
    assessment.validate()?;
    for evidence_id in &assessment.inputs {
        let evidence = stores.evidence.get(evidence_id)?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                format!(
                    "assessment '{}' rests on evidence '{evidence_id}', which does not exist",
                    assessment.id
                ),
            )
        })?;
        if !evidence.covers(&assessment.revision_pack) {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "assessment '{}' judges revision '{}' using evidence '{evidence_id}' about \
                     '{}'; a judgement may not be carried across revisions",
                    assessment.id, assessment.revision_pack, evidence.revision_pack
                ),
            ));
        }
    }
    stores.assessments.put(assessment)
}

/// What a gate requires before a revision may be authorized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateRequirements {
    /// The conditions that must hold, by id and exact definition.
    pub required: Vec<(String, Digest)>,
    /// The risk level above which the gate refuses.
    pub max_risk: AssessedRisk,
}

/// One gate evaluation, as a request.
///
/// Grouped because the fields travel together and are meaningless apart: a
/// gate is a definition and a scope applied to a revision's assessments under
/// one evaluation context.
#[derive(Debug, Clone)]
pub struct GateRequest {
    pub id: String,
    pub revision_pack: RevisionPackId,
    /// The exact definition and scope the revision was sealed against.
    pub definition: Digest,
    pub scope: Digest,
    pub assessments: BTreeSet<AssessmentId>,
    pub requirements: GateRequirements,
    /// Waivers offered for conditions this revision cannot satisfy.
    ///
    /// Offered, not applied: each is checked against the exact condition,
    /// revision and evaluation time before it excuses anything, and one that
    /// does not hold leaves its condition unsatisfied.
    #[allow(clippy::struct_field_names)]
    pub waivers: BTreeSet<String>,
    pub context: EvaluationContext,
}

/// One decision, as a request.
#[derive(Debug, Clone)]
pub struct DecisionRequest {
    pub id: DecisionId,
    pub revision_pack: RevisionPackId,
    pub outcome: DecisionOutcome,
    pub decided_by: ActorId,
    pub decided_at: Timestamp,
    /// The exact grants permitting an approval. Empty is legitimate for a
    /// rejection: refusing work needs no authority beyond being asked.
    pub authority: BTreeSet<SecurityFactRef>,
}

/// Evaluate a gate over the assessments covering a revision.
///
/// Every required condition produces a `GateCondition`, satisfied or not — a
/// gate that dropped its failures would tell a reader "not satisfied" without
/// saying what failed, which is not something anybody can act on.
pub fn evaluate_gate(
    stores: &AuthorizationStores,
    request: &GateRequest,
) -> DraftResult<GateEvaluation> {
    let GateRequest {
        id,
        revision_pack: revision,
        definition,
        scope,
        assessments,
        requirements,
        waivers,
        context,
    } = request;

    // Resolved once, so a waiver that does not load is simply not in force
    // rather than a silent pass.
    let mut offered = Vec::new();
    for waiver_id in waivers {
        if let Some(waiver) = stores.waivers.get(waiver_id)? {
            // A malformed waiver excuses nothing. Validating here rather than
            // trusting storage means a record written before a rule tightened
            // stops excusing conditions the moment it no longer holds.
            if waiver.validate().is_ok() {
                offered.push(waiver);
            }
        }
    }
    let mut evidence: BTreeSet<EvidenceId> = BTreeSet::new();
    // `None` rather than a starting level. `Unassessed` deliberately sorts
    // *above* `Critical` — "nobody looked" is the most dangerous answer — so
    // seeding an accumulator with it would make every gate refuse, and seeding
    // with `Low` would make an empty assessment set read as safe.
    let mut worst: Option<AssessedRisk> = None;
    let mut unassessed = Vec::new();

    for assessment_id in assessments {
        let assessment = stores.assessments.get(assessment_id)?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                format!("gate '{id}' cites assessment '{assessment_id}', which does not exist"),
            )
        })?;
        if !assessment.covers(revision) {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "gate '{id}' evaluates revision '{revision}' using assessment \
                     '{assessment_id}' about '{}'",
                    assessment.revision_pack
                ),
            ));
        }
        evidence.extend(assessment.inputs.iter().cloned());
        if assessment.risk.is_assessed() {
            worst = Some(worst.map_or(assessment.risk, |seen| seen.max(assessment.risk)));
        } else {
            unassessed.push(assessment_id.clone());
        }
    }

    let mut conditions = Vec::new();
    for (condition_id, condition_definition) in &requirements.required {
        // Whether this condition's evidence exists and satisfies. A required
        // condition with no evidence is unsatisfied, not absent: silently
        // dropping it would let a missing check read as a passing one.
        let satisfied = !evidence.is_empty()
            && evidence.iter().try_fold(true, |ok, evidence_id| {
                let entry = stores.evidence.get(evidence_id)?;
                DraftResult::Ok(ok && entry.is_some_and(|value| value.outcome.is_satisfying()))
            })?;
        // An unsatisfied condition may be excused by a waiver that is in
        // force for this exact condition, this exact revision, at this exact
        // moment. Recorded as satisfied *and* named, never silently: a reader
        // has to be able to see that a person allowed this rather than that
        // the check passed.
        let waiver = (!satisfied)
            .then(|| in_force(&offered, revision, condition_id, context.evaluated_at))
            .flatten();
        conditions.push(GateCondition {
            id: condition_id.clone(),
            definition: condition_definition.clone(),
            satisfied: satisfied || waiver.is_some(),
            detail: match (&waiver, satisfied) {
                (Some(waiver), _) => Some(format!(
                    "not satisfied; waived by '{}' until {} — {}",
                    waiver.id,
                    waiver.expires_at.as_unix_nanos(),
                    waiver.reason
                )),
                (None, true) => None,
                (None, false) if evidence.is_empty() => {
                    Some("no evidence was produced for this condition".to_string())
                }
                (None, false) => {
                    Some("the evidence for this condition did not satisfy it".to_string())
                }
            },
        });
    }

    // An unassessed input is its own refusal. "Nobody judged this" is not the
    // same as "it was judged and found acceptable", and treating them alike
    // would let unreviewed work through on an empty assessment.
    //
    // Both ways of nobody judging it count. An assessment that reached no
    // judgement refuses here, and so does the absence of any assessment at
    // all: `worst` stays `None` when nothing was assessed, and a `None` that
    // fell through to the satisfied branch would make "we never looked" the
    // one risk state that passes a gate.
    if !unassessed.is_empty() {
        conditions.push(GateCondition {
            id: "draft.gate/assessed".to_string(),
            definition: Digest::of_bytes(b"draft.gate/assessed.v1"),
            satisfied: false,
            detail: Some(format!(
                "{} assessment(s) reached no risk judgement",
                unassessed.len()
            )),
        });
    } else if worst.is_none() {
        conditions.push(GateCondition {
            id: "draft.gate/assessed".to_string(),
            definition: Digest::of_bytes(b"draft.gate/assessed.v1"),
            satisfied: false,
            detail: Some("no assessment judged this revision's risk".to_string()),
        });
    } else if worst.is_some_and(|risk| risk > requirements.max_risk) {
        conditions.push(GateCondition {
            id: "draft.gate/risk".to_string(),
            definition: Digest::of_bytes(b"draft.gate/risk.v1"),
            satisfied: false,
            detail: Some(format!(
                "assessed risk {:?} exceeds the permitted {:?}",
                worst.expect("a risk above the permitted level was assessed"),
                requirements.max_risk
            )),
        });
    } else {
        conditions.push(GateCondition {
            id: "draft.gate/risk".to_string(),
            definition: Digest::of_bytes(b"draft.gate/risk.v1"),
            satisfied: true,
            detail: None,
        });
    }

    let evaluation = GateEvaluation {
        id: id.clone(),
        revision_pack: revision.clone(),
        definition: definition.clone(),
        scope: scope.clone(),
        evidence,
        assessments: assessments.clone(),
        conditions,
        context: context.clone(),
    };
    evaluation.validate()?;
    stores.gates.put(&evaluation)?;
    Ok(evaluation)
}

/// Record an immutable Decision about a revision.
///
/// An approval requires a satisfied gate covering the same revision. Approving
/// over a failing gate would not be a decision made with the facts — it would
/// be a decision made instead of them.
///
/// A rejection needs no gate: refusing work is legitimate whatever the checks
/// say, and requiring a green gate to say "no" would make it impossible to
/// stop work that fails its own conditions.
pub fn decide(
    stores: &AuthorizationStores,
    request: &DecisionRequest,
    gate: Option<&GateEvaluation>,
) -> DraftResult<Decision> {
    let DecisionRequest {
        id,
        revision_pack: revision,
        outcome,
        decided_by,
        decided_at,
        authority,
    } = request;
    if matches!(outcome, DecisionOutcome::Approved) {
        let gate = gate.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::ReviewRequired,
                format!(
                    "decision '{id}' approves revision '{revision}' with no gate evaluation; an \
                     approval must be made with the facts, not instead of them"
                ),
            )
        })?;
        if !gate.covers(revision) {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "decision '{id}' approves revision '{revision}' citing a gate about '{}'",
                    gate.revision_pack
                ),
            ));
        }
        if !gate.is_satisfied() {
            let unsatisfied: Vec<&str> = gate
                .unsatisfied()
                .into_iter()
                .map(|condition| condition.id.as_str())
                .collect();
            return Err(DraftError::new(
                DraftErrorKind::ReviewRequired,
                format!(
                    "decision '{id}' cannot approve revision '{revision}': the gate is not \
                     satisfied ({})",
                    unsatisfied.join(", ")
                ),
            ));
        }
    }

    let decision = Decision {
        id: id.clone(),
        revision_pack: revision.clone(),
        outcome: outcome.clone(),
        decided_by: decided_by.clone(),
        decided_at: *decided_at,
        authority: authority.clone(),
    };
    decision.validate()?;
    stores.decisions.put(&decision)?;
    Ok(decision)
}

/// The evidence a revision has, for a caller assembling an assessment.
pub fn evidence_for(
    stores: &AuthorizationStores,
    revision: &RevisionPackId,
    ids: &BTreeSet<EvidenceId>,
) -> DraftResult<Vec<Evidence>> {
    let mut found = Vec::new();
    for id in ids {
        if let Some(evidence) = stores.evidence.get(id)? {
            if evidence.covers(revision) {
                found.push(evidence);
            }
        }
    }
    Ok(found)
}

/// The waiver in force for one condition, if any is.
///
/// Every clause has to hold: the exact revision, the exact condition, and an
/// expiry still in the future at the moment of evaluation. An exception
/// accepted for the work as it stood is not an exception for whatever it
/// became, and an expired one is a record of a past decision rather than a
/// current permission.
fn in_force<'a>(
    offered: &'a [GateWaiver],
    revision: &RevisionPackId,
    condition: &str,
    now: Timestamp,
) -> Option<&'a GateWaiver> {
    offered
        .iter()
        .find(|waiver| waiver.is_in_force(revision, condition, now))
}
