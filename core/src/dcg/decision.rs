//! `Decision` — one immutable human judgement about one exact revision.
//!
//! # Why the outcome is stated, not inferred
//!
//! A decision carries its outcome explicitly rather than being implied by
//! which command produced it. "Approved" and "rejected" recorded as different
//! *kinds* of record would make the set of possible outcomes a function of the
//! code paths that exist, and adding a new one later would silently reinterpret
//! the old ones.
//!
//! # Why a rejection is kept
//!
//! A rejected revision is not a deleted one. "We looked and said no" is a
//! materially different history from "nobody looked", and the second is what
//! discarding a rejection would leave behind.
//!
//! # Why it binds an exact revision
//!
//! Same rule as Evidence: a decision approves the work that was in front of the
//! reviewer. Under §2.45 the revision id is a create-once binding, so a
//! substituted revision cannot inherit an approval given for the original.

use draft_dcg_contract::ids::{ActorId, ChangeRevisionId, DecisionId};
use draft_dcg_contract::security::SecurityFactRef;
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use crate::support::immutable_store::ImmutableFactStore;

/// What a reviewer concluded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DecisionOutcome {
    Approved,
    Rejected {
        reason: String,
    },
    /// The reviewer asked for changes. The work stays open.
    ///
    /// Distinct from `Rejected`: one says "not this", the other says "not
    /// yet", and treating them alike would either close work that was meant to
    /// continue or leave work open that was meant to stop.
    ChangesRequested {
        reason: String,
    },
}

/// One recorded judgement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub id: DecisionId,
    /// The exact revision judged.
    pub revision: ChangeRevisionId,
    pub outcome: DecisionOutcome,
    pub decided_by: ActorId,
    pub decided_at: Timestamp,
    /// The exact grants that permitted this decision.
    ///
    /// Empty is legitimate for a rejection: refusing work needs no authority
    /// beyond being asked to review it. An approval that cites none is refused,
    /// because approving is the act that lets work proceed.
    #[serde(default)]
    pub authority: BTreeSet<SecurityFactRef>,
}

impl crate::contracts::VersionedContract for Decision {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::Decision;
}

impl Decision {
    pub fn digest(&self) -> DraftResult<Digest> {
        self.validate()?;
        Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
    }

    pub fn validate(&self) -> DraftResult<()> {
        match &self.outcome {
            DecisionOutcome::Approved if self.authority.is_empty() => Err(DraftError::new(
                DraftErrorKind::Validation,
                "an approval must cite the authority it was made under; approving is what lets \
                 work proceed, and a permission resting on nothing is not a permission",
            )),
            DecisionOutcome::Rejected { reason } | DecisionOutcome::ChangesRequested { reason }
                if reason.trim().is_empty() =>
            {
                Err(DraftError::new(
                    DraftErrorKind::Validation,
                    "a refusal must say why; the author cannot act on an unexplained one",
                ))
            }
            _ => Ok(()),
        }
    }

    /// Whether this decision speaks to `revision`.
    pub fn covers(&self, revision: &ChangeRevisionId) -> bool {
        &self.revision == revision
    }

    /// Whether this decision permits the work to proceed.
    pub fn is_approval(&self) -> bool {
        matches!(self.outcome, DecisionOutcome::Approved)
    }

    /// Whether the work remains open after this decision.
    ///
    /// Both refusals leave it open. A rejected revision can be revised and
    /// resealed; only promotion finishes a Change.
    pub fn leaves_work_open(&self) -> bool {
        !self.is_approval()
    }
}

/// Create-once storage for decisions.
pub struct DecisionStore {
    facts: ImmutableFactStore<Decision>,
}

impl DecisionStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            facts: ImmutableFactStore::new(directory),
        }
    }

    pub fn put(&self, decision: &Decision) -> DraftResult<()> {
        decision.validate()?;
        self.facts.put(decision.id.as_str(), decision)?;
        Ok(())
    }

    pub fn get(&self, id: &DecisionId) -> DraftResult<Option<Decision>> {
        self.facts.get(id.as_str())
    }

    /// Every decision this project holds.
    pub fn list(&self) -> DraftResult<Vec<Decision>> {
        let mut found = Vec::new();
        for id in self.facts.list_ids()? {
            if let Some(decision) = self.facts.get(&id)? {
                found.push(decision);
            }
        }
        Ok(found)
    }
}
