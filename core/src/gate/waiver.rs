//! `GateWaiver` — an immutable, expiring, authorized exception.
//!
//! # Why a waiver names the exact state it covers
//!
//! A waiver that said only "this finding is accepted" would keep applying
//! after the work changed, which is the opposite of what a reviewer meant when
//! they accepted a specific risk on a specific revision. So a waiver binds the
//! exact revision, and — like Evidence — never carries to the next one.
//!
//! # Why it expires
//!
//! An exception without an end is a policy change made without anyone deciding
//! to change the policy. Requiring an expiry forces the question back into the
//! open: either the underlying issue is fixed, or somebody re-authorizes the
//! exception and their name is on it again.
//!
//! # Why it carries exact authority
//!
//! §2.27: the waiver names the grant that permitted it by exact
//! `SecurityFactRef`. A waiver citing a grant by id alone could be pointed at
//! a different grant by substituting bytes, so "who allowed this?" would have
//! an answer that could be changed after the fact.

use draft_dcg_contract::ids::{ActorId, RevisionPackId};
use draft_dcg_contract::security::SecurityFactRef;
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;
use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use crate::support::immutable_store::ImmutableFactStore;

/// An authorized exception to one gate condition, on one revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateWaiver {
    pub id: String,
    /// The exact revision this waives a condition for.
    pub revision_pack: RevisionPackId,
    /// The condition being waived.
    pub condition: String,
    /// Why the exception was granted. Required: an unexplained waiver cannot
    /// be reviewed, renewed or revoked on its merits.
    pub reason: String,
    pub waived_by: ActorId,
    pub waived_at: Timestamp,
    /// When the exception lapses.
    pub expires_at: Timestamp,
    /// The exact grant that authorized this exception.
    pub authority: SecurityFactRef,
}

impl crate::contracts::VersionedContract for GateWaiver {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::GateWaiver;
}

impl GateWaiver {
    pub fn digest(&self) -> DraftResult<Digest> {
        self.validate()?;
        Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
    }

    pub fn validate(&self) -> DraftResult<()> {
        if self.reason.trim().is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "a waiver must say why the exception was granted; an unexplained one cannot be \
                 reviewed on its merits",
            ));
        }
        if self.expires_at <= self.waived_at {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "a waiver that expires no later than it was granted waives nothing",
            ));
        }
        Ok(())
    }

    /// Whether this waiver is in force for `condition` on `revision` at `now`.
    ///
    /// All three must hold. In particular the revision must match exactly: an
    /// exception accepted for the work as it stood is not an exception for
    /// whatever it became.
    pub fn is_in_force(&self, revision: &RevisionPackId, condition: &str, now: Timestamp) -> bool {
        &self.revision_pack == revision && self.condition == condition && self.expires_at > now
    }
}

/// Create-once storage for waivers.
pub struct GateWaiverStore {
    facts: ImmutableFactStore<GateWaiver>,
}

impl GateWaiverStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            facts: ImmutableFactStore::new(directory),
        }
    }

    pub fn put(&self, waiver: &GateWaiver) -> DraftResult<()> {
        waiver.validate()?;
        self.facts.put(&waiver.id, waiver)?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> DraftResult<Option<GateWaiver>> {
        self.facts.get(id)
    }

    /// Every waiver on record, in id order.
    ///
    /// Read from the store's own directory rather than an index: a waiver
    /// missing from an index would be invisible to the surface that has to
    /// warn about its renewal, which is exactly when it matters.
    pub fn list(&self) -> DraftResult<Vec<GateWaiver>> {
        let mut waivers = Vec::new();
        for id in self.facts.list_ids()? {
            if let Some(waiver) = self.facts.get(&id)? {
                waivers.push(waiver);
            }
        }
        Ok(waivers)
    }
}
