//! `Assessment` — what the evidence was judged to mean.
//!
//! # Why this is separate from Evidence
//!
//! Evidence says what was observed. An assessment says what that implies about
//! risk, and the two have different authors, different failure modes and
//! different lifetimes. Folding them together would make "the tests passed"
//! and "this change is low risk" the same claim — and the second is a judgement
//! somebody made, which a reviewer may disagree with, while the first is not.
//!
//! Like Evidence it binds an exact `ChangeRevisionId` and never carries.

use draft_dcg_contract::ids::{AssessmentId, ChangeRevisionId, EvidenceId};
use draft_dcg_contract::producer::ProducerIdentity;
use draft_dcg_contract::Digest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::evidence::context::EvaluationContext;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use crate::support::immutable_store::ImmutableFactStore;

/// The judged risk of a revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssessedRisk {
    Low,
    Medium,
    High,
    Critical,
    /// Nothing assessed it.
    ///
    /// Deliberately not `Low`. Defaulting an unassessed change to low risk
    /// would make "nobody looked" indistinguishable from "somebody looked and
    /// found nothing", and only one of those is a reason to proceed.
    Unassessed,
}

impl AssessedRisk {
    /// Whether this level was actually reached by assessing something.
    pub fn is_assessed(self) -> bool {
        self != Self::Unassessed
    }
}

/// An immutable judgement about one exact revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assessment {
    pub id: AssessmentId,
    /// The exact revision judged.
    pub revision: ChangeRevisionId,
    /// The exact evidence this judgement rests on.
    pub inputs: BTreeSet<EvidenceId>,
    pub risk: AssessedRisk,
    /// Why, in the assessor's words. Opaque to Core.
    pub rationale: String,
    pub producer: ProducerIdentity,
    /// The digest of the rule set applied.
    pub configuration: Digest,
    pub context: EvaluationContext,
}

impl Assessment {
    pub fn digest(&self) -> DraftResult<Digest> {
        self.validate()?;
        Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
    }

    pub fn validate(&self) -> DraftResult<()> {
        // An assessment resting on no evidence is an opinion. It may still be
        // recorded — `Unassessed` says exactly that — but it cannot claim a
        // level it did not reach by examining anything.
        if self.risk.is_assessed() && self.inputs.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "an assessment that examined no evidence cannot claim a risk level; record it \
                 as unassessed rather than asserting a judgement nothing supports",
            ));
        }
        Ok(())
    }

    pub fn covers(&self, revision: &ChangeRevisionId) -> bool {
        &self.revision == revision
    }
}

/// Create-once storage for assessments.
pub struct AssessmentStore {
    facts: ImmutableFactStore<Assessment>,
}

impl AssessmentStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            facts: ImmutableFactStore::new(directory),
        }
    }

    pub fn put(&self, assessment: &Assessment) -> DraftResult<()> {
        assessment.validate()?;
        self.facts.put(assessment.id.as_str(), assessment)?;
        Ok(())
    }

    pub fn get(&self, id: &AssessmentId) -> DraftResult<Option<Assessment>> {
        self.facts.get(id.as_str())
    }

    /// Every assessment this project holds.
    pub fn list(&self) -> DraftResult<Vec<Assessment>> {
        let mut found = Vec::new();
        for id in self.facts.list_ids()? {
            if let Some(assessment) = self.facts.get(&id)? {
                found.push(assessment);
            }
        }
        Ok(found)
    }
}
