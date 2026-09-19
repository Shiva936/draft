//! The Publication family: delivering an accepted Baseline to an external
//! provider, and proving afterwards what actually happened.
//!
//! Publication is the only part of Draft that causes effects outside Draft, so
//! its canonical facts are built to answer one question honestly: *did the
//! external mutation occur?* Several distinctions exist purely to stop that
//! question being answered by assumption.
//!
//! # The cryptographic chain
//!
//! ```text
//! PublicationRef -> PublicationAttemptRef -> PublicationOutcomeDigest
//!                                         -> PublicationResolutionDigest
//! ```
//!
//! Each link is exact — id **and** digest where the bytes matter — so no step
//! can be re-pointed at a different object after the fact.
//!
//! # Distinctions that are never collapsed
//!
//! * **A staged attempt is not a dispatched attempt.** A `PublicationAttempt`
//!   object existing on disk is never proof that a request was made.
//! * **`NoEffect` is not `Failed`.** `NoEffect` is the stronger claim that the
//!   external mutation provably did *not* happen; it is never used to mean "we
//!   never called the provider".
//! * **`Indeterminate` is a real outcome.** Not knowing is recorded, not
//!   guessed at.
//! * **An outcome is not a resolution.** A primary outcome is immutable and
//!   never re-competed; a later, better-informed interpretation becomes
//!   authoritative only through an authorized [`PublicationResolution`].
//! * **A retry authorization is not a permission to dispatch.** It is a
//!   historical one-shot fact; whether it may still be *used* is re-evaluated
//!   against current authority at dispatch.
//!
//! # Self-consistency
//!
//! A digest proves the bytes are unchanged. It does not prove the derived
//! fields inside those bytes agree with their own canonical inputs, or that an
//! object was filed under the right key. Every type here validates both, so a
//! self-consistently corrupted object with a valid outer digest is still
//! rejected.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::baseline::BaselineId;
use crate::canonical::canonical_bytes;
use crate::digest::{canonical_digest, domain_hash, Digest};
use crate::identifier::{NamespacedId, ScopedId};
use crate::ids::{ActorId, PromotionId, PublicationAttemptId, PublicationId, ReceiptId};
use crate::producer::ProducerIdentity;
use crate::provider::ProviderRouteRef;
use crate::receipt::ReceiptSignerBinding;
use crate::security::{
    CredentialAuthorityClass, PolicyDigest, ProjectSecurityStateDigest, SecurityFactRef,
};
use crate::value::{
    LeaseFence, LeaseId, ProjectControlGeneration, ProviderBindingGeneration, RegistryRevisions,
    Timestamp,
};
use crate::{FormatError, FormatResult};

/// Frozen domain separators.
pub const PUBLICATION_REQUEST_KEY_DOMAIN: &str = "draft.dcg.publication-request-key/v1";
pub const PUBLICATION_IDEMPOTENCY_KEY_DOMAIN: &str = "draft.dcg.publication-idempotency-key/v1";
pub const PUBLICATION_DIGEST_DOMAIN: &str = "draft.dcg.publication/v1";
pub const PUBLICATION_ATTEMPT_DIGEST_DOMAIN: &str = "draft.dcg.publication-attempt/v1";
pub const PUBLICATION_OUTCOME_DIGEST_DOMAIN: &str = "draft.dcg.publication-outcome/v1";
pub const PUBLICATION_RESOLUTION_DIGEST_DOMAIN: &str = "draft.dcg.publication-resolution/v1";
pub const PUBLICATION_RETRY_AUTHORIZATION_DIGEST_DOMAIN: &str =
    "draft.dcg.publication-retry-authorization/v1";

/// Longest a free-text reason or rationale may be.
pub const MAX_REASON_LENGTH: usize = 2048;

/// Why a Baseline is being published.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PublicationPurposeId(NamespacedId);

impl PublicationPurposeId {
    pub fn parse(value: &str) -> FormatResult<Self> {
        Ok(Self(NamespacedId::parse(value)?))
    }
}

impl std::fmt::Display for PublicationPurposeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Distinguishes a deliberate republication from the original.
///
/// Without it, republishing the same Baseline to the same route for the same
/// purpose would compute the same request key and be deduplicated into the
/// original — which is right for an accidental repeat and wrong for an intended
/// one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RepublishIntentId(ScopedId);

impl RepublishIntentId {
    pub fn parse(value: impl Into<String>) -> FormatResult<Self> {
        Ok(Self(ScopedId::parse(value)?))
    }
}

impl std::fmt::Display for RepublishIntentId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// What the provider guarantees about repeated delivery.
///
/// This is what decides whether a crash with an unknown outcome may safely be
/// retried, so it is canonical rather than operational.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliverySemantics {
    /// Re-sending with the same idempotency key cannot duplicate the effect.
    IdempotentByKey,
    /// The provider can be asked to reconcile by a client-supplied key.
    ReconcileByClientKey,
    /// The provider can be queried by a client-supplied key.
    QueryByClientKey,
    /// Re-sending may duplicate the effect. Never retried automatically.
    NonIdempotent,
}

/// Declares a canonical digest over a publication-family object.
macro_rules! publication_digest {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Digest);

        impl $name {
            pub fn new(digest: Digest) -> Self {
                Self(digest)
            }

            pub fn digest(&self) -> &Digest {
                &self.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

publication_digest!(
    /// Deduplicates publication *requests*: one request key names one
    /// Publication, id and digest both.
    PublicationRequestKey);
publication_digest!(
    /// The key a provider uses to recognise a repeated delivery of the same
    /// Publication.
    PublicationIdempotencyKey);
publication_digest!(
    /// The canonical digest of a [`Publication`].
    PublicationDigest);
publication_digest!(
    /// The canonical digest of a [`PublicationAttempt`].
    PublicationAttemptDigest);
publication_digest!(
    /// The canonical digest of a [`PublicationOutcome`].
    PublicationOutcomeDigest);
publication_digest!(
    /// The canonical digest of a [`PublicationResolution`].
    PublicationResolutionDigest);
publication_digest!(
    /// The canonical digest of a [`PublicationRetryAuthorization`].
    PublicationRetryAuthorizationDigest);

/// An exact reference to one Publication.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationRef {
    pub id: PublicationId,
    pub digest: PublicationDigest,
}

/// An exact reference to one publication attempt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationAttemptRef {
    pub id: PublicationAttemptId,
    pub digest: PublicationAttemptDigest,
}

/// The immutable intent to deliver one exact Baseline to one exact route.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publication {
    pub id: PublicationId,
    /// The deduplication key, derived from this object's own inputs.
    pub request_key: PublicationRequestKey,
    pub promotion: PromotionId,
    pub baseline: BaselineId,
    /// The exact route, frozen here and never re-resolved at dispatch.
    pub route: ProviderRouteRef,
    pub purpose: PublicationPurposeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub republish_intent: Option<RepublishIntentId>,
    pub requested_by: ActorId,
    /// The exact security facts cited when this was requested.
    pub authority_inputs: BTreeSet<SecurityFactRef>,
    /// The non-secret authority class the effect would occur under.
    ///
    /// Never a credential, never a handle. Changing the secret material behind
    /// a handle may not change this, because it decides *whose* authority an
    /// external effect happens under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_authority_class: Option<CredentialAuthorityClass>,
    pub delivery_semantics: DeliverySemantics,
    /// The key the provider uses to recognise a repeat.
    pub idempotency_key: PublicationIdempotencyKey,
    pub created_at: Timestamp,
}

impl Publication {
    /// The request key these inputs must produce.
    pub fn compute_request_key(
        promotion: &PromotionId,
        baseline: &BaselineId,
        route: &ProviderRouteRef,
        purpose: &PublicationPurposeId,
        republish_intent: Option<&RepublishIntentId>,
    ) -> FormatResult<PublicationRequestKey> {
        let route_bytes = canonical_bytes(route)?;
        let intent = republish_intent
            .map(ToString::to_string)
            .unwrap_or_default();
        Ok(PublicationRequestKey(domain_hash(
            PUBLICATION_REQUEST_KEY_DOMAIN,
            [
                promotion.as_str().as_bytes(),
                baseline.digest().as_str().as_bytes(),
                route_bytes.as_slice(),
                purpose.to_string().as_bytes(),
                intent.as_bytes(),
            ],
        )))
    }

    /// The idempotency key these inputs must produce.
    pub fn compute_idempotency_key(
        id: &PublicationId,
        baseline: &BaselineId,
        route: &ProviderRouteRef,
    ) -> FormatResult<PublicationIdempotencyKey> {
        let route_bytes = canonical_bytes(route)?;
        Ok(PublicationIdempotencyKey(domain_hash(
            PUBLICATION_IDEMPOTENCY_KEY_DOMAIN,
            [
                id.as_str().as_bytes(),
                baseline.digest().as_str().as_bytes(),
                route_bytes.as_slice(),
            ],
        )))
    }

    /// Recompute every derived field from this object's own canonical inputs.
    ///
    /// A Publication whose `request_key` or `idempotency_key` disagrees with
    /// its own fields is invalid **even when its outer `PublicationDigest`
    /// matches its bytes** — the digest only proves nobody edited it since, not
    /// that it was coherent when written.
    pub fn validate(&self) -> FormatResult<()> {
        let request_key = Self::compute_request_key(
            &self.promotion,
            &self.baseline,
            &self.route,
            &self.purpose,
            self.republish_intent.as_ref(),
        )?;
        if request_key != self.request_key {
            return Err(FormatError::Consistency(format!(
                "publication '{}' carries request key {} but its own inputs compute {}",
                self.id, self.request_key, request_key
            )));
        }
        let idempotency_key = Self::compute_idempotency_key(&self.id, &self.baseline, &self.route)?;
        if idempotency_key != self.idempotency_key {
            return Err(FormatError::Consistency(format!(
                "publication '{}' carries idempotency key {} but its own inputs compute {}",
                self.id, self.idempotency_key, idempotency_key
            )));
        }
        Ok(())
    }

    pub fn digest(&self) -> FormatResult<PublicationDigest> {
        self.validate()?;
        Ok(PublicationDigest(canonical_digest(
            PUBLICATION_DIGEST_DOMAIN,
            self,
        )?))
    }

    pub fn reference(&self) -> FormatResult<PublicationRef> {
        Ok(PublicationRef {
            id: self.id.clone(),
            digest: self.digest()?,
        })
    }

    /// Verify a reference against this Publication.
    pub fn verify_reference(&self, reference: &PublicationRef) -> FormatResult<()> {
        if reference.id != self.id {
            return Err(FormatError::Integrity(format!(
                "reference names publication '{}' but the loaded object is '{}'",
                reference.id, self.id
            )));
        }
        let recomputed = self.digest()?;
        if recomputed != reference.digest {
            return Err(FormatError::Integrity(format!(
                "publication '{}' was expected to be {} but computes to {}",
                self.id, reference.digest, recomputed
            )));
        }
        Ok(())
    }
}

/// One durably authorized attempt to deliver a Publication.
///
/// Complete before the durable `Dispatching` transition, so the exact authority
/// and state that permitted the dispatch is recorded before the provider can
/// possibly be called.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationAttempt {
    pub id: PublicationAttemptId,
    /// The exact Publication this attempts.
    pub publication: PublicationRef,
    /// The authoritative attempt number, allocated only by a committed
    /// `PublicationControl` mutation. Unique and monotonic; gaps are legal.
    pub attempt_number: u32,
    /// The exact route validated against the binding's current pointers.
    pub route: ProviderRouteRef,
    pub attempt_authority: BTreeSet<SecurityFactRef>,
    pub authority_decision: crate::authority::AuthorityDecision,
    pub project_control_generation_at_dispatch: ProjectControlGeneration,
    pub project_security_state_at_dispatch: ProjectSecurityStateDigest,
    pub policy_digest_at_dispatch: PolicyDigest,
    pub global_registry_revisions_at_dispatch: RegistryRevisions,
    pub provider_binding_generation_at_dispatch: ProviderBindingGeneration,
    /// The one-shot authorization consumed to permit this attempt, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_authorization: Option<PublicationRetryAuthorizationDigest>,
    pub lease_id: LeaseId,
    pub lease_fence: LeaseFence,
    pub started_at: Timestamp,
    pub provenance: ProducerIdentity,
}

impl PublicationAttempt {
    /// Check this attempt against the Publication it claims to attempt.
    ///
    /// The route check is the important one: an attempt that claimed a
    /// different route from its Publication would be an external effect nobody
    /// authorized at that destination.
    pub fn validate_against(&self, publication: &Publication) -> FormatResult<()> {
        publication.verify_reference(&self.publication)?;
        if self.route != publication.route {
            return Err(FormatError::Consistency(format!(
                "attempt '{}' claims a route that is not its publication's route",
                self.id
            )));
        }
        if !self.authority_decision.is_permitted() {
            return Err(FormatError::Consistency(format!(
                "attempt '{}' was dispatched on an authority decision that did not permit it",
                self.id
            )));
        }
        self.authority_decision.validate()?;
        Ok(())
    }

    pub fn digest(&self) -> FormatResult<PublicationAttemptDigest> {
        Ok(PublicationAttemptDigest(canonical_digest(
            PUBLICATION_ATTEMPT_DIGEST_DOMAIN,
            self,
        )?))
    }

    pub fn reference(&self) -> FormatResult<PublicationAttemptRef> {
        Ok(PublicationAttemptRef {
            id: self.id.clone(),
            digest: self.digest()?,
        })
    }

    pub fn verify_reference(&self, reference: &PublicationAttemptRef) -> FormatResult<()> {
        if reference.id != self.id {
            return Err(FormatError::Integrity(format!(
                "reference names attempt '{}' but the loaded object is '{}'",
                reference.id, self.id
            )));
        }
        let recomputed = self.digest()?;
        if recomputed != reference.digest {
            return Err(FormatError::Integrity(format!(
                "attempt '{}' was expected to be {} but computes to {}",
                self.id, reference.digest, recomputed
            )));
        }
        Ok(())
    }
}

/// What an attempt concluded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PublicationOutcomeKind {
    /// The external effect occurred, and the provider named it.
    Succeeded { external_reference: String },
    /// The external effect did not occur, and the provider said why.
    Failed { reason: String },
    /// The external effect provably did *not* occur.
    ///
    /// Strictly stronger than `Failed`, and never used to mean "the provider
    /// was never invoked" — an attempt that was never dispatched has no
    /// outcome at all.
    NoEffect { evidence: String },
    /// Draft cannot establish whether the effect occurred.
    ///
    /// A real, recordable answer. Retrying from here needs explicit authority
    /// where the delivery semantics cannot rule out duplication.
    Indeterminate { reason: String },
}

/// The one primary outcome of one attempt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationOutcome {
    /// The exact attempt this concludes.
    pub attempt: PublicationAttemptRef,
    pub receipt_id: ReceiptId,
    pub receipt_signer: ReceiptSignerBinding,
    pub outcome: PublicationOutcomeKind,
    pub concluded_at: Timestamp,
    pub provenance: ProducerIdentity,
}

impl PublicationOutcome {
    /// Check this outcome is being filed beneath the right attempt.
    ///
    /// An outcome for one attempt must never be recorded under another's head:
    /// that would attribute an external effect to a dispatch that did not cause
    /// it.
    pub fn validate_under(
        &self,
        head_attempt: &PublicationAttemptRef,
        expected_receipt: &ReceiptId,
        expected_signer: &ReceiptSignerBinding,
    ) -> FormatResult<()> {
        if &self.attempt != head_attempt {
            return Err(FormatError::Consistency(format!(
                "outcome names attempt '{}' but is being recorded under '{}'",
                self.attempt.id, head_attempt.id
            )));
        }
        if &self.receipt_id != expected_receipt {
            return Err(FormatError::Consistency(format!(
                "outcome for attempt '{}' carries receipt '{}' but '{}' was preallocated",
                self.attempt.id, self.receipt_id, expected_receipt
            )));
        }
        if &self.receipt_signer != expected_signer {
            return Err(FormatError::Consistency(format!(
                "outcome for attempt '{}' carries a signer binding that was not the frozen one",
                self.attempt.id
            )));
        }
        check_reason(self.outcome_text())?;
        Ok(())
    }

    fn outcome_text(&self) -> &str {
        match &self.outcome {
            PublicationOutcomeKind::Succeeded { external_reference } => external_reference,
            PublicationOutcomeKind::Failed { reason }
            | PublicationOutcomeKind::Indeterminate { reason } => reason,
            PublicationOutcomeKind::NoEffect { evidence } => evidence,
        }
    }

    pub fn digest(&self) -> FormatResult<PublicationOutcomeDigest> {
        Ok(PublicationOutcomeDigest(canonical_digest(
            PUBLICATION_OUTCOME_DIGEST_DOMAIN,
            self,
        )?))
    }
}

/// An authorized interpretation of an outcome.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "resolution", rename_all = "snake_case")]
pub enum PublicationResolutionKind {
    ResolvedSucceeded { external_reference: String },
    ResolvedFailed { reason: String },
}

/// The only way a later, better-informed interpretation becomes authoritative.
///
/// The primary outcome is never replaced or re-competed. A resolution sits
/// alongside it, cites current authority, and can itself be superseded — within
/// the same outcome's chain and never across outcomes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationResolution {
    /// The exact outcome being interpreted.
    pub outcome: PublicationOutcomeDigest,
    pub receipt_id: ReceiptId,
    pub receipt_signer: ReceiptSignerBinding,
    pub resolution: PublicationResolutionKind,
    /// The resolution this replaces, within the same outcome's chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<PublicationResolutionDigest>,
    pub actor: ActorId,
    /// The exact grant cited at resolution time.
    pub authority: SecurityFactRef,
    /// The commit-time decision, so recovery never needs current authority.
    pub authority_decision: crate::authority::AuthorityDecision,
    pub project_security_state_at_resolution: ProjectSecurityStateDigest,
    pub policy_digest_at_resolution: PolicyDigest,
    pub global_registry_revisions_at_resolution: RegistryRevisions,
    pub rationale: String,
    pub resolved_at: Timestamp,
}

impl PublicationResolution {
    /// Check this resolution against the head it is advancing.
    ///
    /// A supersession chain may never cross outcomes: replacing outcome A's
    /// interpretation with one written about outcome B would silently
    /// re-attribute an external result.
    pub fn validate_advancing(
        &self,
        head_outcome: &PublicationOutcomeDigest,
        current_head: Option<&PublicationResolution>,
    ) -> FormatResult<()> {
        if &self.outcome != head_outcome {
            return Err(FormatError::Consistency(format!(
                "resolution names outcome {} but is advancing the head of {head_outcome}",
                self.outcome
            )));
        }
        check_reason(&self.rationale)?;
        self.authority_decision.validate()?;
        if !self.authority_decision.is_permitted() {
            return Err(FormatError::Consistency(
                "a resolution cannot become authoritative on a decision that refused it".into(),
            ));
        }
        match (&self.supersedes, current_head) {
            (None, None) => Ok(()),
            (None, Some(_)) => Err(FormatError::Consistency(
                "a resolution head already exists, so a new resolution must supersede it".into(),
            )),
            (Some(_), None) => Err(FormatError::Consistency(
                "resolution supersedes a prior resolution, but no head exists".into(),
            )),
            (Some(superseded), Some(head)) => {
                if head.outcome != self.outcome {
                    return Err(FormatError::Consistency(
                        "a supersession chain may not cross outcomes".into(),
                    ));
                }
                let head_digest = head.digest()?;
                if superseded != &head_digest {
                    return Err(FormatError::Consistency(format!(
                        "resolution supersedes {superseded} but the current head is {head_digest}"
                    )));
                }
                Ok(())
            }
        }
    }

    pub fn digest(&self) -> FormatResult<PublicationResolutionDigest> {
        Ok(PublicationResolutionDigest(canonical_digest(
            PUBLICATION_RESOLUTION_DIGEST_DOMAIN,
            self,
        )?))
    }
}

/// A one-shot, explicitly acknowledged permission to make one more attempt.
///
/// Bound to an exact [`PublicationRef`] rather than a bare id, so changing the
/// bytes beneath `pub_` can never widen or redirect an existing authorization.
///
/// Creation and consumption are different things with different owners: this
/// fact records that the authorization was *issued*. Whether it has been
/// *used* lives solely in `PublicationControl`, and there is no consumption
/// flag here or anywhere else.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationRetryAuthorization {
    /// The exact Publication this authorizes another attempt at.
    pub publication: PublicationRef,
    /// The outcome that made a retry necessary.
    pub prior_outcome: PublicationOutcomeDigest,
    pub actor: ActorId,
    pub authority: SecurityFactRef,
    pub authority_decision: crate::authority::AuthorityDecision,
    pub project_security_state_at_authorization: ProjectSecurityStateDigest,
    pub policy_digest_at_authorization: PolicyDigest,
    pub global_registry_revisions_at_authorization: RegistryRevisions,
    pub rationale: String,
    /// Always `true`. A retry may duplicate an external effect, and the
    /// authorizer is recorded as having said so.
    pub duplicate_risk_acknowledged: bool,
    /// Always `true`. One authorization permits one allocated attempt.
    pub authorizes_one_attempt: bool,
    pub authorized_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

impl PublicationRetryAuthorization {
    /// Structural validation.
    pub fn validate(&self) -> FormatResult<()> {
        if !self.duplicate_risk_acknowledged {
            return Err(FormatError::Consistency(
                "a retry authorization must record that duplicate risk was acknowledged".into(),
            ));
        }
        if !self.authorizes_one_attempt {
            return Err(FormatError::Consistency(
                "a retry authorization is one-shot; it cannot authorize more than one attempt"
                    .into(),
            ));
        }
        if let Some(expires_at) = self.expires_at {
            if expires_at.as_unix_nanos() <= self.authorized_at.as_unix_nanos() {
                return Err(FormatError::Consistency(
                    "a retry authorization cannot expire before it is issued".into(),
                ));
            }
        }
        check_reason(&self.rationale)?;
        self.authority_decision.validate()?;
        if !self.authority_decision.is_permitted() {
            return Err(FormatError::Consistency(
                "a retry authorization cannot rest on a decision that refused it".into(),
            ));
        }
        Ok(())
    }

    /// Check the authorization is about the Publication and outcome it claims.
    ///
    /// The `prior_outcome` must belong to an attempt of *this* Publication, so
    /// an authorization can never be replayed against a different Publication
    /// merely because the same actor or provider is involved.
    pub fn validate_for(
        &self,
        publication: &Publication,
        prior_outcome: &PublicationOutcome,
        prior_attempt: &PublicationAttempt,
    ) -> FormatResult<()> {
        self.validate()?;
        publication.verify_reference(&self.publication)?;
        let outcome_digest = prior_outcome.digest()?;
        if outcome_digest != self.prior_outcome {
            return Err(FormatError::Consistency(format!(
                "retry authorization names prior outcome {} but the loaded outcome is {}",
                self.prior_outcome, outcome_digest
            )));
        }
        prior_attempt.verify_reference(&prior_outcome.attempt)?;
        if prior_attempt.publication != self.publication {
            return Err(FormatError::Consistency(format!(
                "retry authorization for publication '{}' cites an outcome belonging to '{}'",
                self.publication.id, prior_attempt.publication.id
            )));
        }
        Ok(())
    }

    /// Whether the authorization has expired at `now`.
    ///
    /// Expiry is only one of several reasons an authorization may be unusable:
    /// a live one still faces current-authority validation at dispatch.
    pub fn is_expired_at(&self, now: Timestamp) -> bool {
        self.expires_at
            .is_some_and(|expires| now.as_unix_nanos() >= expires.as_unix_nanos())
    }

    pub fn digest(&self) -> FormatResult<PublicationRetryAuthorizationDigest> {
        self.validate()?;
        Ok(PublicationRetryAuthorizationDigest(canonical_digest(
            PUBLICATION_RETRY_AUTHORIZATION_DIGEST_DOMAIN,
            self,
        )?))
    }
}

fn check_reason(text: &str) -> FormatResult<()> {
    if text.len() > MAX_REASON_LENGTH {
        return Err(FormatError::Consistency(format!(
            "text exceeds {MAX_REASON_LENGTH} bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{AuthorityDecision, AuthorityDecisionOutcome, AuthorityScopeClaim};
    use crate::capability::CapabilityId;
    use crate::ids::ProviderBindingId;
    use crate::provider::{
        ProviderOperationalProfileDigest, ProviderProvenanceRef, ProviderSemanticDefinitionDigest,
    };
    use crate::security::SecurityControlKindId;

    fn route() -> ProviderRouteRef {
        ProviderRouteRef {
            provenance: ProviderProvenanceRef {
                binding: ProviderBindingId::parse("pbd_a1").unwrap(),
                semantic_definition: ProviderSemanticDefinitionDigest::new(Digest::of_bytes(
                    b"SD1",
                )),
            },
            operational_profile: ProviderOperationalProfileDigest::new(Digest::of_bytes(b"OP1")),
        }
    }

    fn grant() -> SecurityFactRef {
        SecurityFactRef::new(
            SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap(),
            Some(ScopedId::parse("auth_1").unwrap()),
            Digest::of_bytes(b"grant"),
        )
    }

    fn producer() -> ProducerIdentity {
        ProducerIdentity::new(
            NamespacedId::parse("draft.core/publication").unwrap(),
            "0.3.4",
        )
        .unwrap()
    }

    fn decision(subject: &str, permitted: bool) -> AuthorityDecision {
        AuthorityDecision::new(
            AuthorityScopeClaim {
                capability: CapabilityId::parse("draft.publish/v1").unwrap(),
                subject: ScopedId::parse(subject).unwrap(),
            },
            if permitted {
                AuthorityDecisionOutcome::Permitted
            } else {
                AuthorityDecisionOutcome::Refused {
                    reason: "not in scope".into(),
                }
            },
            if permitted {
                BTreeSet::from([grant()])
            } else {
                BTreeSet::new()
            },
            producer(),
            Timestamp::from_unix_nanos(1_000),
        )
        .unwrap()
    }

    fn baseline() -> BaselineId {
        BaselineId::new(Digest::of_bytes(b"baseline"))
    }

    fn purpose() -> PublicationPurposeId {
        PublicationPurposeId::parse("draft.publish/deploy").unwrap()
    }

    fn publication() -> Publication {
        let id = PublicationId::parse("pub_a1b2c3").unwrap();
        let promotion = PromotionId::parse("pro_a1b2c3").unwrap();
        Publication {
            request_key: Publication::compute_request_key(
                &promotion,
                &baseline(),
                &route(),
                &purpose(),
                None,
            )
            .unwrap(),
            idempotency_key: Publication::compute_idempotency_key(&id, &baseline(), &route())
                .unwrap(),
            id,
            promotion,
            baseline: baseline(),
            route: route(),
            purpose: purpose(),
            republish_intent: None,
            requested_by: ActorId::parse("act_a1").unwrap(),
            authority_inputs: BTreeSet::from([grant()]),
            credential_authority_class: Some(
                CredentialAuthorityClass::parse("acme.cloud/tenant-prod").unwrap(),
            ),
            delivery_semantics: DeliverySemantics::IdempotentByKey,
            created_at: Timestamp::from_unix_nanos(1_000),
        }
    }

    fn signer() -> ReceiptSignerBinding {
        ReceiptSignerBinding::new(ActorId::parse("act_signer").unwrap(), "key-1", "ed25519")
            .unwrap()
    }

    fn attempt() -> PublicationAttempt {
        PublicationAttempt {
            id: PublicationAttemptId::parse("pat_a1b2c3").unwrap(),
            publication: publication().reference().unwrap(),
            attempt_number: 1,
            route: route(),
            attempt_authority: BTreeSet::from([grant()]),
            authority_decision: decision("pub_a1b2c3", true),
            project_control_generation_at_dispatch: ProjectControlGeneration::new(7),
            project_security_state_at_dispatch: ProjectSecurityStateDigest::new(Digest::of_bytes(
                b"security",
            )),
            policy_digest_at_dispatch: PolicyDigest::new(Digest::of_bytes(b"policy")),
            global_registry_revisions_at_dispatch: RegistryRevisions::new(),
            provider_binding_generation_at_dispatch: ProviderBindingGeneration::new(3),
            retry_authorization: None,
            lease_id: LeaseId::parse("lease-1").unwrap(),
            lease_fence: LeaseFence::new(42),
            started_at: Timestamp::from_unix_nanos(2_000),
            provenance: producer(),
        }
    }

    fn outcome(kind: PublicationOutcomeKind) -> PublicationOutcome {
        PublicationOutcome {
            attempt: attempt().reference().unwrap(),
            receipt_id: ReceiptId::parse("rcp_a1").unwrap(),
            receipt_signer: signer(),
            outcome: kind,
            concluded_at: Timestamp::from_unix_nanos(3_000),
            provenance: producer(),
        }
    }

    fn succeeded() -> PublicationOutcome {
        outcome(PublicationOutcomeKind::Succeeded {
            external_reference: "deploy-991".into(),
        })
    }

    fn indeterminate() -> PublicationOutcome {
        outcome(PublicationOutcomeKind::Indeterminate {
            reason: "provider unreachable after dispatch".into(),
        })
    }

    // ---- Publication --------------------------------------------------

    #[test]
    fn a_publication_validates_its_own_derived_keys() {
        publication().validate().unwrap();
        publication().reference().unwrap();
    }

    #[test]
    fn a_self_consistently_corrupted_publication_is_still_rejected() {
        // The outer digest will match these bytes perfectly. The point is that
        // matching bytes are not the same as coherent bytes.
        let mut forged = publication();
        forged.request_key = PublicationRequestKey::new(Digest::of_bytes(b"forged"));
        let error = forged.validate().unwrap_err();
        assert!(matches!(error, FormatError::Consistency(_)), "{error}");
        assert!(forged.digest().is_err());
    }

    #[test]
    fn an_idempotency_key_binds_the_publication_baseline_and_route() {
        let mut other_route = publication();
        other_route.route.operational_profile =
            ProviderOperationalProfileDigest::new(Digest::of_bytes(b"OP2"));
        // Stale derived keys no longer agree, which is exactly the detection.
        assert!(other_route.validate().is_err());
    }

    #[test]
    fn a_republish_intent_distinguishes_a_deliberate_repeat() {
        let original = publication().request_key;
        let intent = RepublishIntentId::parse("rerun-1").unwrap();
        let republished = Publication::compute_request_key(
            &PromotionId::parse("pro_a1b2c3").unwrap(),
            &baseline(),
            &route(),
            &purpose(),
            Some(&intent),
        )
        .unwrap();
        assert_ne!(original, republished);
    }

    #[test]
    fn the_canonical_publication_carries_no_credential_handle() {
        let encoded = serde_json::to_string(&publication()).unwrap();
        assert!(!encoded.contains("credential_handle"), "{encoded}");
        // A document carrying one is refused rather than quietly ignored.
        let widened = encoded.replace("{\"id\"", "{\"credential_handle_ref\":\"h1\",\"id\"");
        assert!(serde_json::from_str::<Publication>(&widened).is_err());
    }

    #[test]
    fn substituting_publication_bytes_is_detected_by_the_reference() {
        let reference = publication().reference().unwrap();
        let mut redirected = publication();
        redirected.requested_by = ActorId::parse("act_other").unwrap();
        let error = redirected.verify_reference(&reference).unwrap_err();
        assert!(matches!(error, FormatError::Integrity(_)), "{error}");
    }

    // ---- PublicationAttempt --------------------------------------------

    #[test]
    fn an_attempt_must_share_its_publications_route() {
        attempt().validate_against(&publication()).unwrap();

        let mut redirected = attempt();
        redirected.route.provenance.binding = ProviderBindingId::parse("pbd_evil").unwrap();
        let error = redirected.validate_against(&publication()).unwrap_err();
        assert!(matches!(error, FormatError::Consistency(_)), "{error}");
    }

    #[test]
    fn an_attempt_cannot_rest_on_a_refusal() {
        let mut refused = attempt();
        refused.authority_decision = decision("pub_a1b2c3", false);
        assert!(refused.validate_against(&publication()).is_err());
    }

    #[test]
    fn the_dispatch_snapshot_is_part_of_attempt_identity() {
        let base = attempt().digest().unwrap();
        let mut moved = attempt();
        moved.provider_binding_generation_at_dispatch = ProviderBindingGeneration::new(4);
        assert_ne!(base, moved.digest().unwrap());

        let mut refenced = attempt();
        refenced.lease_fence = LeaseFence::new(43);
        assert_ne!(base, refenced.digest().unwrap());
    }

    // ---- PublicationOutcome --------------------------------------------

    #[test]
    fn an_outcome_may_only_be_filed_under_its_own_attempt() {
        let head = attempt().reference().unwrap();
        succeeded()
            .validate_under(&head, &ReceiptId::parse("rcp_a1").unwrap(), &signer())
            .unwrap();

        let mut other_attempt = attempt();
        other_attempt.id = PublicationAttemptId::parse("pat_999999").unwrap();
        let foreign_head = other_attempt.reference().unwrap();
        let error = succeeded()
            .validate_under(
                &foreign_head,
                &ReceiptId::parse("rcp_a1").unwrap(),
                &signer(),
            )
            .unwrap_err();
        assert!(matches!(error, FormatError::Consistency(_)), "{error}");
    }

    #[test]
    fn an_outcome_must_carry_the_preallocated_receipt_and_frozen_signer() {
        let head = attempt().reference().unwrap();
        assert!(succeeded()
            .validate_under(&head, &ReceiptId::parse("rcp_other").unwrap(), &signer())
            .is_err());

        let other_signer =
            ReceiptSignerBinding::new(ActorId::parse("act_other").unwrap(), "key-9", "ed25519")
                .unwrap();
        assert!(succeeded()
            .validate_under(&head, &ReceiptId::parse("rcp_a1").unwrap(), &other_signer)
            .is_err());
    }

    #[test]
    fn no_effect_and_failed_are_different_facts() {
        let failed = outcome(PublicationOutcomeKind::Failed {
            reason: "rejected by provider".into(),
        });
        let no_effect = outcome(PublicationOutcomeKind::NoEffect {
            evidence: "provider confirmed no record was created".into(),
        });
        assert_ne!(failed.digest().unwrap(), no_effect.digest().unwrap());
    }

    #[test]
    fn indeterminate_is_a_recordable_outcome_not_an_absence() {
        indeterminate().digest().unwrap();
        assert_ne!(
            indeterminate().digest().unwrap(),
            outcome(PublicationOutcomeKind::Failed {
                reason: "provider unreachable after dispatch".into()
            })
            .digest()
            .unwrap()
        );
    }

    // ---- PublicationResolution -----------------------------------------

    fn resolution(
        outcome: PublicationOutcomeDigest,
        supersedes: Option<PublicationResolutionDigest>,
    ) -> PublicationResolution {
        PublicationResolution {
            outcome,
            receipt_id: ReceiptId::parse("rcp_r1").unwrap(),
            receipt_signer: signer(),
            resolution: PublicationResolutionKind::ResolvedSucceeded {
                external_reference: "deploy-991".into(),
            },
            supersedes,
            actor: ActorId::parse("act_a1").unwrap(),
            authority: grant(),
            authority_decision: decision("pub_a1b2c3", true),
            project_security_state_at_resolution: ProjectSecurityStateDigest::new(
                Digest::of_bytes(b"security-now"),
            ),
            policy_digest_at_resolution: PolicyDigest::new(Digest::of_bytes(b"policy-now")),
            global_registry_revisions_at_resolution: RegistryRevisions::new(),
            rationale: "provider confirmed the deploy landed".into(),
            resolved_at: Timestamp::from_unix_nanos(5_000),
        }
    }

    #[test]
    fn a_first_resolution_advances_an_empty_head() {
        let digest = indeterminate().digest().unwrap();
        resolution(digest.clone(), None)
            .validate_advancing(&digest, None)
            .unwrap();
    }

    #[test]
    fn a_resolution_must_supersede_the_existing_head() {
        let digest = indeterminate().digest().unwrap();
        let head = resolution(digest.clone(), None);

        // Ignoring the head is refused...
        assert!(resolution(digest.clone(), None)
            .validate_advancing(&digest, Some(&head))
            .is_err());

        // ...and superseding exactly it is accepted.
        let next = resolution(digest.clone(), Some(head.digest().unwrap()));
        next.validate_advancing(&digest, Some(&head)).unwrap();

        // Superseding something else is refused.
        let wrong = resolution(
            digest.clone(),
            Some(PublicationResolutionDigest::new(Digest::of_bytes(b"other"))),
        );
        assert!(wrong.validate_advancing(&digest, Some(&head)).is_err());
    }

    #[test]
    fn a_supersession_chain_may_never_cross_outcomes() {
        // The re-attribution this rule prevents: interpreting outcome A by
        // superseding a resolution written about outcome B.
        let outcome_a = indeterminate().digest().unwrap();
        let outcome_b = succeeded().digest().unwrap();
        let head_of_b = resolution(outcome_b, None);
        let crossing = resolution(outcome_a.clone(), Some(head_of_b.digest().unwrap()));
        let error = crossing
            .validate_advancing(&outcome_a, Some(&head_of_b))
            .unwrap_err();
        assert!(matches!(error, FormatError::Consistency(_)), "{error}");
    }

    #[test]
    fn a_resolution_for_another_outcome_cannot_advance_this_head() {
        let outcome_a = indeterminate().digest().unwrap();
        let outcome_b = succeeded().digest().unwrap();
        assert!(resolution(outcome_b, None)
            .validate_advancing(&outcome_a, None)
            .is_err());
    }

    #[test]
    fn a_resolution_cannot_rest_on_a_refusal() {
        let digest = indeterminate().digest().unwrap();
        let mut refused = resolution(digest.clone(), None);
        refused.authority_decision = decision("pub_a1b2c3", false);
        assert!(refused.validate_advancing(&digest, None).is_err());
    }

    #[test]
    fn a_resolution_records_its_commit_time_security_context() {
        // So recovery can finalize it from the frozen snapshot without needing
        // the grant to still be current.
        let digest = indeterminate().digest().unwrap();
        let base = resolution(digest.clone(), None).digest().unwrap();
        let mut elsewhere = resolution(digest, None);
        elsewhere.project_security_state_at_resolution =
            ProjectSecurityStateDigest::new(Digest::of_bytes(b"security-later"));
        assert_ne!(base, elsewhere.digest().unwrap());
    }

    // ---- PublicationRetryAuthorization ---------------------------------

    fn retry_authorization() -> PublicationRetryAuthorization {
        PublicationRetryAuthorization {
            publication: publication().reference().unwrap(),
            prior_outcome: indeterminate().digest().unwrap(),
            actor: ActorId::parse("act_a1").unwrap(),
            authority: grant(),
            authority_decision: decision("pub_a1b2c3", true),
            project_security_state_at_authorization: ProjectSecurityStateDigest::new(
                Digest::of_bytes(b"security"),
            ),
            policy_digest_at_authorization: PolicyDigest::new(Digest::of_bytes(b"policy")),
            global_registry_revisions_at_authorization: RegistryRevisions::new(),
            rationale: "operator accepts the duplicate risk".into(),
            duplicate_risk_acknowledged: true,
            authorizes_one_attempt: true,
            authorized_at: Timestamp::from_unix_nanos(4_000),
            expires_at: Some(Timestamp::from_unix_nanos(9_000)),
        }
    }

    #[test]
    fn a_retry_authorization_is_bound_to_its_exact_publication_and_outcome() {
        retry_authorization()
            .validate_for(&publication(), &indeterminate(), &attempt())
            .unwrap();
    }

    #[test]
    fn a_retry_authorization_cannot_be_replayed_against_another_publication() {
        // Same actor, same provider, different Publication. The exact
        // PublicationRef is what refuses it.
        let mut other = publication();
        other.id = PublicationId::parse("pub_999999").unwrap();
        other.idempotency_key =
            Publication::compute_idempotency_key(&other.id, &baseline(), &route()).unwrap();
        let error = retry_authorization()
            .validate_for(&other, &indeterminate(), &attempt())
            .unwrap_err();
        assert!(matches!(error, FormatError::Integrity(_)), "{error}");
    }

    #[test]
    fn a_retry_authorization_must_cite_the_outcome_it_names() {
        let error = retry_authorization()
            .validate_for(&publication(), &succeeded(), &attempt())
            .unwrap_err();
        assert!(matches!(error, FormatError::Consistency(_)), "{error}");
    }

    #[test]
    fn duplicate_risk_must_be_acknowledged_and_the_grant_is_one_shot() {
        let mut unacknowledged = retry_authorization();
        unacknowledged.duplicate_risk_acknowledged = false;
        assert!(unacknowledged.validate().is_err());

        let mut unlimited = retry_authorization();
        unlimited.authorizes_one_attempt = false;
        assert!(unlimited.validate().is_err());
    }

    #[test]
    fn there_is_no_consumption_flag_anywhere_on_the_fact() {
        // Consumption is PublicationControl's, and only its. A second copy of
        // that bit here is what would let one authorization be spent twice.
        let encoded = serde_json::to_string(&retry_authorization()).unwrap();
        for forbidden in ["consumed", "used", "spent", "remaining"] {
            assert!(!encoded.contains(forbidden), "{forbidden} in {encoded}");
        }
    }

    #[test]
    fn expiry_is_checked_against_a_supplied_clock() {
        let authorization = retry_authorization();
        assert!(!authorization.is_expired_at(Timestamp::from_unix_nanos(8_999)));
        assert!(authorization.is_expired_at(Timestamp::from_unix_nanos(9_000)));

        let mut backwards = retry_authorization();
        backwards.expires_at = Some(Timestamp::from_unix_nanos(1));
        assert!(backwards.validate().is_err());
    }

    #[test]
    fn the_wire_forms_round_trip() {
        for encoded in [
            serde_json::to_string(&publication()).unwrap(),
            serde_json::to_string(&attempt()).unwrap(),
        ] {
            assert!(!encoded.is_empty());
        }
        assert_eq!(
            serde_json::from_str::<Publication>(&serde_json::to_string(&publication()).unwrap())
                .unwrap(),
            publication()
        );
        assert_eq!(
            serde_json::from_str::<PublicationAttempt>(&serde_json::to_string(&attempt()).unwrap())
                .unwrap(),
            attempt()
        );
        assert_eq!(
            serde_json::from_str::<PublicationRetryAuthorization>(
                &serde_json::to_string(&retry_authorization()).unwrap()
            )
            .unwrap(),
            retry_authorization()
        );
    }
}
