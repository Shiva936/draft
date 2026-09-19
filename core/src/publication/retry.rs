//! Authorizing another attempt at something that may already have happened.
//!
//! When delivery ends `Indeterminate` — Draft asked and could not establish
//! whether the effect occurred — [`crate::publication::delivery`] says whether
//! another attempt is safe on the semantics alone. For three of the four
//! classes it is not, and the Publication stops there.
//!
//! This is the only way past that stop: a person states that they accept the
//! attempt may duplicate a real-world effect, and that statement becomes an
//! immutable fact the next attempt cites.
//!
//! # Why the acknowledgement is recorded rather than inferred
//!
//! "Retry" is a word that hides a decision. Re-sending a payment, a
//! notification or a deployment that may already have landed is a choice
//! somebody makes about the outside world, and the interesting question
//! afterwards is always *who decided that, and what did they know*. So the
//! authorization carries the actor, the grant, the commit-time security state
//! and the outcome that made a retry necessary — and refuses to exist without
//! the acknowledgement.
//!
//! # Why it is one-shot, and why the control record enforces that
//!
//! One authorization permits one allocated attempt. If it permitted a
//! standing capability, an operator's single decision about one uncertain
//! delivery would silently license every future one.
//!
//! Enforcement lives in [`crate::publication::control`] rather than here,
//! because the only place "has this been spent?" can be answered without a
//! race is inside the lock that commits the allocation. This module writes the
//! fact; the control record decides it is consumed.
//!
//! # Why it needs no head
//!
//! Unlike a Resolution, a retry authorization advances nothing. It is a
//! create-once fact whose consumption is recorded elsewhere, so there is no
//! compare-exchange to recover and no journal to keep — the transaction is one
//! durable write, and a crash before it leaves nothing to finish.

use std::path::PathBuf;

use draft_dcg_contract::ids::{ActorId, PublicationId};
use draft_dcg_contract::publication::{
    Publication, PublicationAttempt, PublicationOutcome, PublicationOutcomeKind,
    PublicationRetryAuthorization, PublicationRetryAuthorizationDigest,
};
use draft_dcg_contract::value::Timestamp;

use crate::publication::authority::DispatchAuthority;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::immutable_store::ImmutableFactStore;

/// The retry authorizations this project has issued, written once.
#[derive(Debug, Clone)]
pub struct RetryAuthorizationStore {
    facts: ImmutableFactStore<PublicationRetryAuthorization>,
}

impl RetryAuthorizationStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            facts: ImmutableFactStore::new(directory),
        }
    }

    pub fn for_layout(layout: &crate::project::layout::DraftLayout) -> Self {
        Self::for_layout_root(&layout.publication_dir())
    }

    /// Open the store under an already-resolved `publication/` root.
    pub fn for_layout_root(root: &std::path::Path) -> Self {
        Self::new(root.join("retry-authorizations"))
    }

    /// Store an authorization and return the digest that identifies it.
    pub fn put(
        &self,
        authorization: &PublicationRetryAuthorization,
    ) -> DraftResult<PublicationRetryAuthorizationDigest> {
        let digest = authorization
            .digest()
            .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?;
        self.facts.put(&key(&digest), authorization)?;
        Ok(digest)
    }

    /// Every authorization this project has issued, by its own digest.
    ///
    /// Issued, not unspent. Whether one has been consumed lives solely in
    /// `PublicationControl`, and folding that in here would put the one-shot
    /// bit in a second place.
    pub fn list(&self) -> DraftResult<Vec<String>> {
        self.facts.list_ids()
    }

    /// Load an authorization, verified against the digest it is filed under.
    pub fn get(
        &self,
        digest: &PublicationRetryAuthorizationDigest,
    ) -> DraftResult<Option<PublicationRetryAuthorization>> {
        let Some(authorization) = self.facts.get(&key(digest))? else {
            return Ok(None);
        };
        let recomputed = authorization
            .digest()
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;
        if &recomputed != digest {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("retry authorization {digest} holds bytes computing to {recomputed}"),
            ));
        }
        Ok(Some(authorization))
    }
}

fn key(digest: &PublicationRetryAuthorizationDigest) -> String {
    digest.digest().to_string()
}

/// Everything authorizing a retry needs told.
pub struct RetryRequest<'a> {
    /// The Publication another attempt would be made at.
    pub publication: &'a Publication,
    /// The outcome that made a retry necessary, and the attempt it concluded.
    pub prior_outcome: &'a PublicationOutcome,
    pub prior_attempt: &'a PublicationAttempt,
    pub actor: ActorId,
    /// Why this is worth the risk, in the authorizer's words.
    pub rationale: String,
    pub authorized_at: Timestamp,
}

/// Issue the authorization, having established current authority.
///
/// `authority` is Phase 1's result, so this runs under the same guards a
/// dispatch would and cites the same commit-time state. Proposing something
/// genuinely new is always a fresh decision, evaluated against the security
/// state as it is now — which is why this takes an established decision rather
/// than an actor's say-so.
pub fn authorize(
    request: &RetryRequest<'_>,
    authority: &DispatchAuthority,
) -> DraftResult<PublicationRetryAuthorization> {
    // Only an uncertain outcome needs one. A success needs no retry, and a
    // refusal is a real answer the target gave — re-sending after either would
    // be a different operation with a different justification, and letting one
    // authorization cover all three would make "we do not know" and "it said
    // no" interchangeable.
    if !matches!(
        request.prior_outcome.outcome,
        PublicationOutcomeKind::Indeterminate { .. }
    ) {
        return Err(DraftError::new(
            DraftErrorKind::Validation,
            format!(
                "publication '{}' concluded {}, which needs no retry authorization; one is for \
                 an outcome Draft could not establish",
                request.publication.id,
                describe(&request.prior_outcome.outcome)
            ),
        ));
    }

    let reference = request
        .publication
        .reference()
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;
    let prior_outcome = request
        .prior_outcome
        .digest()
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;

    // The grant this rests on. A permitted decision always cites at least one,
    // so an empty set here would mean the decision permitted on nothing.
    let grant = authority
        .authority_decision
        .considered
        .iter()
        .next()
        .cloned()
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                "a permitted decision cited no authority, so nothing authorizes the retry",
            )
        })?;

    let authorization = PublicationRetryAuthorization {
        publication: reference,
        prior_outcome,
        actor: request.actor.clone(),
        authority: grant,
        authority_decision: authority.authority_decision.clone(),
        project_security_state_at_authorization: authority.project_security_state.clone(),
        policy_digest_at_authorization: authority.policy_digest.clone(),
        global_registry_revisions_at_authorization: authority.registry_revisions.clone(),
        rationale: request.rationale.clone(),
        // Both are always true and both are recorded rather than assumed: the
        // fact refuses to exist without them, so a reader never has to work out
        // whether the authorizer understood what they were permitting.
        duplicate_risk_acknowledged: true,
        authorizes_one_attempt: true,
        authorized_at: request.authorized_at,
        expires_at: None,
    };

    // Checked against the Publication, the outcome and the attempt it claims,
    // so an authorization can never be replayed against a different
    // Publication merely because the same actor or target is involved.
    authorization
        .validate_for(
            request.publication,
            request.prior_outcome,
            request.prior_attempt,
        )
        .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?;

    // Counted where the fact becomes valid, not where a caller asked for one:
    // this is the number an operator watches to see how often Draft is being
    // asked to risk duplicating a real-world effect.
    crate::support::telemetry::Counter::PublicationUnsafeRetryAuthorizations.increment();
    Ok(authorization)
}

/// Refuse an authorization that does not belong to the Publication spending it.
///
/// The check is on the `PublicationRef` — id **and** digest — rather than a
/// bare id. That is what stops the bytes under `pub_A` changing beneath an
/// existing authorization and quietly widening it to a different route,
/// baseline or purpose.
pub fn require_targets(
    authorization: &PublicationRetryAuthorization,
    publication: &Publication,
) -> DraftResult<()> {
    crate::publication::consistency::retry_authorization_targets(
        &authorization.publication,
        publication,
    )
}

fn describe(outcome: &PublicationOutcomeKind) -> &'static str {
    match outcome {
        PublicationOutcomeKind::Succeeded { .. } => "successfully",
        PublicationOutcomeKind::Failed { .. } => "with a refusal from the target",
        PublicationOutcomeKind::NoEffect { .. } => "with no effect",
        PublicationOutcomeKind::Indeterminate { .. } => "indeterminately",
    }
}

/// The publication id an authorization is about.
pub fn publication_of(authorization: &PublicationRetryAuthorization) -> &PublicationId {
    &authorization.publication.id
}
