//! Whether a revision may proceed, and the exact evidence for that answer.
//!
//! # A gate evaluation is a historical fact, not a permission
//!
//! It records that a set of conditions was checked against a named revision,
//! under a named policy and security context, at a named moment. It never
//! means "this may proceed now". A gate satisfied last week rests on evidence
//! about a revision, policy and authority that may all have moved since, and
//! the commit boundary re-evaluates rather than trusting the record.
//!
//! Keeping the record anyway matters: "we checked and it passed" and "we never
//! checked" are different histories, and only one of them is auditable.
//!
//! # What a gate binds
//!
//! §2.31 requires the binding to be exhaustive — the exact revision, the exact
//! definition and scope digests, both the policy and security context digests,
//! the exact condition definitions, and the exact input ids. Anything left
//! unbound is something that could move without the evaluation noticing, and a
//! gate that cannot notice its own inputs moving is not a gate.

pub mod acceptance;
pub mod reviewability;
pub mod waiver;

use draft_dcg_contract::ids::{AssessmentId, ChangeRevisionId, EvidenceId};
use draft_dcg_contract::Digest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::evidence::context::EvaluationContext;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use crate::support::immutable_store::ImmutableFactStore;

/// One condition a gate checked.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateCondition {
    /// What was required, namespaced and contributed.
    pub id: String,
    /// The exact definition of the condition as it was applied.
    ///
    /// A condition named only by id could be redefined between evaluation and
    /// reading, and the record would still say "satisfied" about a rule that
    /// no longer exists.
    pub definition: Digest,
    pub satisfied: bool,
    /// Why not, when it was not. Absent for a satisfied condition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The immutable record of one gate evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateEvaluation {
    pub id: String,
    /// The exact revision evaluated.
    pub revision: ChangeRevisionId,
    /// The exact definition and scope that revision was sealed against.
    pub definition: Digest,
    pub scope: Digest,
    /// The exact inputs the conditions read.
    pub evidence: BTreeSet<EvidenceId>,
    pub assessments: BTreeSet<AssessmentId>,
    /// Every condition, satisfied or not.
    ///
    /// Failures are recorded rather than dropped: a reader needs to know what
    /// was checked and failed, not merely that the gate did not pass.
    pub conditions: Vec<GateCondition>,
    pub context: EvaluationContext,
}

impl GateEvaluation {
    pub fn digest(&self) -> DraftResult<Digest> {
        self.validate()?;
        Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
    }

    /// Whether every condition was satisfied.
    ///
    /// Not a permission to act. See the module note: this says what was true at
    /// `context.evaluated_at`, and the commit boundary asks again.
    pub fn is_satisfied(&self) -> bool {
        self.conditions.iter().all(|condition| condition.satisfied)
    }

    /// The conditions that were not satisfied.
    pub fn unsatisfied(&self) -> Vec<&GateCondition> {
        self.conditions
            .iter()
            .filter(|condition| !condition.satisfied)
            .collect()
    }

    pub fn validate(&self) -> DraftResult<()> {
        // A gate that checked nothing is not a satisfied gate. Without this an
        // empty condition set would pass `is_satisfied` vacuously, and a
        // misconfiguration that selected no conditions would read as approval.
        if self.conditions.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "a gate evaluation must record the conditions it checked; an empty set would \
                 pass vacuously and read as approval nobody gave",
            ));
        }
        for condition in &self.conditions {
            if !condition.satisfied && condition.detail.is_none() {
                return Err(DraftError::new(
                    DraftErrorKind::Validation,
                    format!(
                        "condition '{}' failed without saying why; an unexplained refusal \
                         cannot be acted on",
                        condition.id
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Whether this evaluation speaks to `revision`.
    pub fn covers(&self, revision: &ChangeRevisionId) -> bool {
        &self.revision == revision
    }
}

/// Create-once storage for gate evaluations.
pub struct GateEvaluationStore {
    facts: ImmutableFactStore<GateEvaluation>,
}

impl GateEvaluationStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            facts: ImmutableFactStore::new(directory),
        }
    }

    pub fn put(&self, evaluation: &GateEvaluation) -> DraftResult<()> {
        evaluation.validate()?;
        self.facts.put(&evaluation.id, evaluation)?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> DraftResult<Option<GateEvaluation>> {
        self.facts.get(id)
    }

    /// Every gate evaluation this project holds.
    pub fn list(&self) -> DraftResult<Vec<GateEvaluation>> {
        let mut found = Vec::new();
        for id in self.facts.list_ids()? {
            if let Some(evaluation) = self.facts.get(&id)? {
                found.push(evaluation);
            }
        }
        Ok(found)
    }
}
