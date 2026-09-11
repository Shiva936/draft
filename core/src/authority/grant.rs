//! `AuthorityGrant` — the immutable fact that a capability was granted.

use draft_dcg_contract::capability::CapabilityId;
use draft_dcg_contract::identifier::ScopedId;
use draft_dcg_contract::ids::{ActorId, AuthorityGrantId};
use draft_dcg_contract::security::{SecurityControlKindId, SecurityFactRef};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;
use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use crate::support::immutable_store::ImmutableFactStore;

/// The reserved kind naming a grant inside a [`SecurityFactRef`].
/// Spelled exactly as `draft-dcg-contract` freezes it: the SDK owns the
/// reserved vocabulary, and a near-miss here would parse as an unrecognised
/// reserved kind and be refused rather than silently accepted.
pub const AUTHORITY_GRANT_KIND: &str = "draft.security/authority-grant.v1";

/// One capability, granted to one actor, over one subject.
///
/// # Why the subject is part of the grant
///
/// A capability granted without a subject would be a capability over
/// everything. Every grant names exactly what it is a grant *over*, so
/// widening the reach of an existing permission requires a new grant that
/// says so — and leaves a record that it was issued.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityGrant {
    pub id: AuthorityGrantId,
    /// Who may exercise it.
    pub grantee: ActorId,
    /// What they may do.
    pub capability: CapabilityId,
    /// What they may do it to.
    pub subject: ScopedId,
    /// Who issued it.
    pub granted_by: ActorId,
    pub granted_at: Timestamp,
    /// When it stops being valid on its own, if it ever does.
    ///
    /// `None` is a grant that outlives every deadline, which is a decision
    /// somebody made rather than an oversight — so it is spelled explicitly
    /// rather than left as a missing field.
    pub expires_at: Option<Timestamp>,
}

impl AuthorityGrant {
    pub fn digest(&self) -> DraftResult<Digest> {
        self.validate()?;
        Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
    }

    /// This grant as the exact reference a canonical fact carries.
    pub fn reference(&self) -> DraftResult<SecurityFactRef> {
        Ok(SecurityFactRef::new(
            SecurityControlKindId::parse(AUTHORITY_GRANT_KIND)
                .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
            Some(ScopedId::parse(self.id.as_str()).map_err(|error| {
                DraftError::new(DraftErrorKind::CorruptData, error.to_string())
            })?),
            self.digest()?,
        ))
    }

    pub fn validate(&self) -> DraftResult<()> {
        // A reserved capability Draft does not implement would be a permission
        // to do something nothing can perform — accepting it would let a typo
        // in a reserved name look like a real grant.
        if !self.capability.is_acceptable() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "'{}' is in Draft's reserved namespace but is not a capability Draft \
                     implements",
                    self.capability.as_namespaced()
                ),
            ));
        }
        if let Some(expires_at) = self.expires_at {
            if expires_at <= self.granted_at {
                return Err(DraftError::new(
                    DraftErrorKind::Validation,
                    "a grant that expires no later than it was issued authorizes nothing",
                ));
            }
        }
        Ok(())
    }

    /// Whether this grant has lapsed by `now`.
    ///
    /// Separate from revocation: expiry is the grant's own terms running out,
    /// revocation is somebody withdrawing it. Both stop it authorizing, and
    /// the difference is worth keeping because only one of them is a decision.
    pub fn is_expired_at(&self, now: Timestamp) -> bool {
        self.expires_at.is_some_and(|expires| expires <= now)
    }

    /// Whether this grant covers exactly this capability over this subject.
    ///
    /// Exact on both. A grant over a different subject is not a narrower
    /// grant, it is a grant over something else.
    pub fn covers(&self, capability: &CapabilityId, subject: &ScopedId) -> bool {
        &self.capability == capability && &self.subject == subject
    }
}

/// Create-once storage for grants.
pub struct AuthorityGrantStore {
    grants: ImmutableFactStore<AuthorityGrant>,
}

impl AuthorityGrantStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            grants: ImmutableFactStore::new(directory),
        }
    }

    /// Record a grant. Re-recording identical content is idempotent; the same
    /// id with different terms is refused.
    pub fn put(&self, grant: &AuthorityGrant) -> DraftResult<()> {
        grant.validate()?;
        self.grants.put(grant.id.as_str(), grant)?;
        Ok(())
    }

    pub fn get(&self, id: &AuthorityGrantId) -> DraftResult<Option<AuthorityGrant>> {
        self.grants.get(id.as_str())
    }

    /// Every grant this project has ever issued, by id.
    ///
    /// Issued, not in force. Whether a grant currently confers anything is the
    /// project's security state's answer, and folding the two together here
    /// would make a revoked grant invisible — which is the opposite of what a
    /// record of who was permitted what is for.
    pub fn list(&self) -> DraftResult<Vec<AuthorityGrant>> {
        let mut grants = Vec::new();
        for id in self.grants.list_ids()? {
            if let Some(grant) = self.grants.get(&id)? {
                grants.push(grant);
            }
        }
        grants.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        Ok(grants)
    }
}
