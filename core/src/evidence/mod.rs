//! What was established about one exact revision, and what it was judged to mean.
//!
//! | | Answers | Produced by |
//! |---|---|---|
//! | [`Evidence`] | what was observed to be true | running checks |
//! | [`assessment::Assessment`] | what that means for risk | judging the evidence |
//!
//! # Nothing carries across revisions
//!
//! This is the rule the module exists to enforce. Evidence binds an *exact*
//! `ChangeRevisionId`, and there is deliberately no way to ask whether it also
//! covers a later one.
//!
//! The tempting shortcut is to let evidence follow the Change: the tests
//! passed, the author edited one file, surely the result still holds. But
//! nobody knows that without running them again, and an approval resting on
//! evidence gathered before the edit is an approval of work that was never
//! examined. Under §2.45 the revision id is backed by a create-once binding, so
//! "the same revision" means the same bytes rather than the same string — a
//! substituted revision cannot inherit judgements made about the original.
//!
//! # Why inputs are exact references
//!
//! Evidence names the observations it read by `ObservationRef` — id *and*
//! digest. Re-observing the same state later produces a different historical
//! observation, and evidence that named only the id would silently re-point at
//! it. What was actually examined has to stay pinned to what was examined.

pub mod assessment;
/// What installed extensions say a project's resources are.
///
/// Evidence rather than graph: classification is the one question about a
/// project whose answer depends on what happens to be installed, and the graph
/// itself must stay readable without it.
pub mod classification;
pub mod context;
pub mod representation;
pub mod risk;
pub mod verification;

use draft_dcg_contract::ids::{ChangeRevisionId, EvidenceId};
use draft_dcg_contract::observation::ObservationRef;
use draft_dcg_contract::producer::ProducerIdentity;
use draft_dcg_contract::Digest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::evidence::context::EvaluationContext;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use crate::support::immutable_store::ImmutableFactStore;

/// What running the checks concluded.
///
/// Five states, because "no checks ran" and "checks ran and passed" are
/// different facts and only one of them is a reason to proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceOutcome {
    Passed,
    Failed,
    /// No capability existed to ask. Installing something would change the
    /// answer, which is what distinguishes this from `NotApplicable`.
    Unavailable,
    /// Draft asked and nothing applied.
    NotApplicable,
    /// Selected but not yet run.
    NotEvaluated,
}

impl EvidenceOutcome {
    /// Whether this outcome is a reason to proceed.
    ///
    /// Only `Passed`. In particular `Unavailable` is not: the absence of a
    /// capability to check something is not evidence that it is fine.
    pub fn is_satisfying(self) -> bool {
        self == Self::Passed
    }
}

/// An immutable statement about one exact revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub id: EvidenceId,
    /// The exact revision this was established about.
    pub revision: ChangeRevisionId,
    /// The exact observations it read.
    pub inputs: BTreeSet<ObservationRef>,
    /// Who produced it.
    pub producer: ProducerIdentity,
    /// The digest of the configuration it ran under.
    ///
    /// Recorded because the same checks under different configuration are
    /// different evidence, and a reader comparing two results needs to know
    /// whether the rules changed underneath them.
    pub configuration: Digest,
    pub outcome: EvidenceOutcome,
    pub context: EvaluationContext,
}

impl crate::contracts::VersionedContract for Evidence {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::Evidence;
}

impl Evidence {
    pub fn digest(&self) -> DraftResult<Digest> {
        self.validate()?;
        Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
    }

    pub fn validate(&self) -> DraftResult<()> {
        // Evidence that read nothing established nothing. `Unavailable` and
        // `NotApplicable` are the honest ways to say "we could not look";
        // claiming a pass over no inputs is not.
        if self.outcome == EvidenceOutcome::Passed && self.inputs.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "evidence cannot pass over no inputs; report unavailable or not-applicable \
                 instead of asserting a result nothing was examined for",
            ));
        }
        Ok(())
    }

    /// Whether this evidence speaks to `revision`.
    ///
    /// Exact equality, and deliberately the only way to ask. There is no
    /// "close enough" revision: evidence gathered before an edit says nothing
    /// about the work after it.
    pub fn covers(&self, revision: &ChangeRevisionId) -> bool {
        &self.revision == revision
    }
}

/// Create-once storage for evidence.
pub struct EvidenceStore {
    facts: ImmutableFactStore<Evidence>,
}

impl EvidenceStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            facts: ImmutableFactStore::new(directory),
        }
    }

    pub fn put(&self, evidence: &Evidence) -> DraftResult<()> {
        evidence.validate()?;
        self.facts.put(evidence.id.as_str(), evidence)?;
        Ok(())
    }

    pub fn get(&self, id: &EvidenceId) -> DraftResult<Option<Evidence>> {
        self.facts.get(id.as_str())
    }

    /// Every piece of evidence this project holds.
    pub fn list(&self) -> DraftResult<Vec<Evidence>> {
        let mut found = Vec::new();
        for id in self.facts.list_ids()? {
            if let Some(evidence) = self.facts.get(&id)? {
                found.push(evidence);
            }
        }
        Ok(found)
    }
}
