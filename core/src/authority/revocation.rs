//! `AuthorityRevocation` — the immutable fact that a grant was withdrawn.

use draft_dcg_contract::identifier::ScopedId;
use draft_dcg_contract::ids::{ActorId, AuthorityGrantId};
use draft_dcg_contract::security::{SecurityControlKindId, SecurityFactRef};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;
use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use crate::support::immutable_store::ImmutableFactStore;

/// The reserved kind naming a revocation inside a [`SecurityFactRef`].
pub const AUTHORITY_REVOCATION_KIND: &str = "draft.security/authority-revocation.v1";

/// A grant, withdrawn.
///
/// # Why this names the grant by exact reference
///
/// A revocation that named the grant by id alone could be pointed at a
/// different grant by substituting the bytes beneath that id. Naming it by
/// exact [`SecurityFactRef`] means the revocation withdraws *the grant whose
/// content this is*, and a verifier re-resolving the reference detects any
/// substitution rather than trusting the id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityRevocation {
    /// The grant being withdrawn, named exactly.
    pub grant: SecurityFactRef,
    /// The grant's logical id, for retrieval.
    pub grant_id: AuthorityGrantId,
    pub revoked_by: ActorId,
    pub revoked_at: Timestamp,
    /// Why. Kept because "revoked for cause" and "revoked because the project
    /// finished" lead to different follow-up, and neither is recoverable from
    /// the fact that a revocation exists.
    pub reason: String,
}

impl AuthorityRevocation {
    pub fn digest(&self) -> DraftResult<Digest> {
        self.validate()?;
        Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
    }

    pub fn reference(&self) -> DraftResult<SecurityFactRef> {
        Ok(SecurityFactRef::new(
            SecurityControlKindId::parse(AUTHORITY_REVOCATION_KIND)
                .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
            Some(ScopedId::parse(self.grant_id.as_str()).map_err(|error| {
                DraftError::new(DraftErrorKind::CorruptData, error.to_string())
            })?),
            self.digest()?,
        ))
    }

    pub fn validate(&self) -> DraftResult<()> {
        if self.reason.trim().is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "a revocation must say why; an unexplained withdrawal cannot be reviewed",
            ));
        }
        let expected = SecurityControlKindId::parse(super::grant::AUTHORITY_GRANT_KIND)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;
        if self.grant.kind != expected {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "a revocation must name an authority grant, not '{}'",
                    self.grant.kind.as_namespaced()
                ),
            ));
        }
        Ok(())
    }
}

/// Create-once storage for revocations, keyed by the grant they withdraw.
///
/// One grant is revoked once. A second revocation of the same grant with
/// different terms is refused rather than layered, so "when and why was this
/// withdrawn" has exactly one answer.
pub struct AuthorityRevocationStore {
    revocations: ImmutableFactStore<AuthorityRevocation>,
}

impl AuthorityRevocationStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            revocations: ImmutableFactStore::new(directory),
        }
    }

    pub fn put(&self, revocation: &AuthorityRevocation) -> DraftResult<()> {
        revocation.validate()?;
        self.revocations
            .put(revocation.grant_id.as_str(), revocation)?;
        Ok(())
    }

    pub fn get(&self, grant: &AuthorityGrantId) -> DraftResult<Option<AuthorityRevocation>> {
        self.revocations.get(grant.as_str())
    }

    /// Every revocation on record.
    pub fn list(&self) -> DraftResult<Vec<AuthorityRevocation>> {
        let mut revocations = Vec::new();
        for id in self.revocations.list_ids()? {
            if let Some(revocation) = self.revocations.get(&id)? {
                revocations.push(revocation);
            }
        }
        revocations.sort_by(|left, right| left.grant_id.as_str().cmp(right.grant_id.as_str()));
        Ok(revocations)
    }

    pub fn is_revoked(&self, grant: &AuthorityGrantId) -> DraftResult<bool> {
        Ok(self.get(grant)?.is_some())
    }
}
