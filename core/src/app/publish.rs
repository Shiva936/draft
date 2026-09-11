//! Publication — delivering a promoted Baseline, without authority over it.
//!
//! Publication consumes what promotion produced. It can fail, be retried, or
//! never happen at all, and none of that changes what the project accepts.
//!
//! ```text
//! Promotion  →  Baseline  →  Publication (optional, repeatable, external)
//!                   ↑
//!            never written back
//! ```
//!
//! # Why publication can never touch the Baseline
//!
//! A Baseline is what the project accepted, decided by people through review.
//! A publication is an attempt to deliver that somewhere else. If a failed
//! delivery could roll the Baseline back, then an unreachable external system
//! would hold a veto over the project's own accepted history — the project
//! would forget what it had agreed, because somebody else's server was down.
//!
//! So there is no path from a publication outcome to the Baseline store. This
//! module reads a Baseline and never writes one, and the type it returns
//! carries no Baseline mutation for a caller to apply.
//!
//! # Why it refuses an unpromoted Baseline
//!
//! Publishing state the project has not accepted would send work outside on
//! nobody's authority. The Baseline must exist, verify, and have a promotion
//! record — a Baseline that exists but was never promoted is the project's
//! initial state, which nothing decided to deliver.

use std::collections::BTreeSet;

use draft_dcg_contract::baseline::{BaselineId, BaselineManifest};
use draft_dcg_contract::ids::{ActivityEventId, PromotionId, PublicationAttemptId, PublicationId};
use draft_dcg_contract::provider::{ProviderProvenanceRef, ProviderRouteRef};
use draft_dcg_contract::publication::{
    DeliverySemantics, Publication, PublicationAttemptDigest, PublicationAttemptRef,
    PublicationOutcomeKind, PublicationPurposeId,
};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;

use crate::publication::dispatch::{
    dispatch, DeliveryResult, DispatchProgress, DispatchRequest, DispatchStores,
};
use crate::publication::outcome::PrimaryOutcomeIdentity;
use crate::publication::registry::CreationOutcome;
use crate::publication::store::{id_for_request_key, PublicationStore};

use crate::dcg::baseline::{current_baseline, BaselineOrigin, BaselineStore};
use crate::project::provider::ProviderBindingStore;
use crate::project::Workspace;
use crate::promotion::protocol::PromotionStores;
use crate::promotion::record::PromotionRecord;
use crate::read_model::baseline::{route_for, RouteRefusal};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// A Baseline cleared for delivery.
///
/// Holds the manifest and the promotion that accepted it, and deliberately no
/// means of changing either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishableBaseline {
    pub baseline: BaselineId,
    pub manifest: BaselineManifest,
    pub promotion: PromotionId,
}

/// What a publication attempt concluded, projected from the engine.
///
/// Every variant corresponds to a durable state the dispatch engine can
/// actually be in — this is a translation of `DispatchProgress`, not a second
/// lifecycle beside it. None of them says anything about the Baseline, because
/// none of them may.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum PublishOutcome {
    /// The attempt dispatched and recorded its primary outcome.
    Concluded {
        attempt: PublicationAttemptId,
        outcome: PublicationOutcomeKind,
    },
    /// An earlier attempt had already concluded this Publication. A retry
    /// converges on it rather than dispatching again.
    AlreadyConcluded {
        attempt: PublicationAttemptId,
        outcome: PublicationOutcomeKind,
    },
    /// An earlier attempt is allocated and must be resumed before a new one
    /// may begin.
    ResumeRequired { attempt: PublicationAttemptId },
    /// An earlier attempt may have caused an effect Draft cannot yet describe.
    ///
    /// Nothing local is blocked. This Publication may not dispatch again until
    /// the effect is resolved, which is the conservative half of "remote
    /// unavailability never blocks unrelated work".
    AwaitingExternalResolution { attempt: PublicationAttemptId },
    /// The engine's local records contradict each other.
    Inconsistent { detail: String },
}

impl PublishOutcome {
    /// Whether the external effect is known to have occurred.
    pub fn is_delivered(&self) -> bool {
        matches!(
            self,
            Self::Concluded {
                outcome: PublicationOutcomeKind::Succeeded { .. },
                ..
            } | Self::AlreadyConcluded {
                outcome: PublicationOutcomeKind::Succeeded { .. },
                ..
            }
        )
    }

    /// The attempt this outcome is about, where there is one.
    pub fn attempt(&self) -> Option<&PublicationAttemptId> {
        match self {
            Self::Concluded { attempt, .. }
            | Self::AlreadyConcluded { attempt, .. }
            | Self::ResumeRequired { attempt }
            | Self::AwaitingExternalResolution { attempt } => Some(attempt),
            Self::Inconsistent { .. } => None,
        }
    }
}

impl From<DispatchProgress> for PublishOutcome {
    fn from(progress: DispatchProgress) -> Self {
        match progress {
            DispatchProgress::Completed { attempt, outcome } => {
                Self::Concluded { attempt, outcome }
            }
            DispatchProgress::AlreadyConcluded { attempt, outcome } => {
                Self::AlreadyConcluded { attempt, outcome }
            }
            DispatchProgress::ResumeRequired { attempt } => Self::ResumeRequired { attempt },
            DispatchProgress::AwaitingExternalResolution { attempt } => {
                Self::AwaitingExternalResolution { attempt }
            }
            DispatchProgress::Inconsistent { detail } => Self::Inconsistent { detail },
        }
    }
}

/// Resolve the Baseline a publication would deliver.
///
/// Refuses anything the project has not promoted. The check is on the
/// promotion record rather than on the Baseline alone, because a Baseline can
/// exist without anybody having decided to accept work into it — the project's
/// initial state is exactly that.
pub fn publishable(
    workspace: &Workspace,
    baseline: &BaselineId,
) -> DraftResult<PublishableBaseline> {
    let baselines = BaselineStore::new(workspace.layout.baselines_dir());
    let manifest = baselines.manifest(baseline)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("baseline {baseline} is not one this project accepted"),
        )
    })?;

    let promotion = promotion_of(workspace, baseline)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::ReviewRequired,
            format!("baseline {baseline} was never promoted, so nothing authorized delivering it"),
        )
        .with_suggestion(
            "promote an approved revision first, then publish the Baseline it accepted",
        )
    })?;

    Ok(PublishableBaseline {
        baseline: baseline.clone(),
        manifest,
        promotion: promotion.promotion,
    })
}

/// The Baseline the project currently accepts, if it may be published.
pub fn publishable_current(workspace: &Workspace) -> DraftResult<PublishableBaseline> {
    let accepted = current_baseline(&workspace.layout)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            "this project accepts no Baseline, so there is nothing to publish",
        )
    })?;
    publishable(workspace, &accepted)
}

/// The promotion that accepted a Baseline, if any did.
///
/// Read from the acceptance record's own origin. That record already states
/// what promoted the Baseline, so asking anywhere else would be deriving an
/// answer the project has written down.
pub fn promotion_of(
    workspace: &Workspace,
    baseline: &BaselineId,
) -> DraftResult<Option<PromotionRecord>> {
    let baselines = BaselineStore::new(workspace.layout.baselines_dir());
    let Some(record) = baselines.record(baseline)? else {
        return Ok(None);
    };
    // An initial Baseline has no promotion: nothing was decided into it, and
    // that absence is the correct answer rather than a lookup failure.
    let BaselineOrigin::Promotion { promotion, .. } = record.origin else {
        return Ok(None);
    };
    PromotionStores::for_layout(&workspace.layout)
        .records
        .get(&promotion)
}

/// Confirm a publication outcome left the accepted Baseline untouched.
///
/// Called after any delivery attempt. It exists because "publication must not
/// change the Baseline" is the kind of property that holds until somebody adds
/// a convenience path, and an assertion at the boundary fails loudly the first
/// time that happens rather than quietly accepting the new behaviour.
pub fn assert_baseline_unchanged(
    workspace: &Workspace,
    expected: &BaselineId,
    outcome: &PublishOutcome,
) -> DraftResult<()> {
    let current = current_baseline(&workspace.layout)?;
    if current.as_ref() == Some(expected) {
        return Ok(());
    }
    Err(DraftError::new(
        DraftErrorKind::Internal,
        format!(
            "publication attempt {} changed the accepted Baseline from {expected} to {}; \
             delivery has no authority over what the project accepts",
            outcome
                .attempt()
                .map_or_else(|| "an attempt".to_string(), ToString::to_string),
            current.map_or_else(|| "none".to_string(), |value| value.to_string())
        ),
    ))
}

/// What a caller asks publication to do.
///
/// Everything here is a product decision somebody makes: which accepted
/// Baseline to deliver, what for, and what the target guarantees about a
/// repeated send. Nothing here is protocol machinery — there is no attempt id
/// to generate, no recovery class to choose, and no way to name a route the
/// Baseline was not observed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishRequest {
    /// The promoted Baseline to deliver.
    pub baseline: BaselineId,
    /// What this delivery is for. Part of the Publication's identity, so two
    /// purposes against the same Baseline are two Publications.
    pub purpose: PublicationPurposeId,
    /// What the target guarantees about repeated delivery.
    ///
    /// The recovery class follows from this and is never chosen separately:
    /// see [`crate::publication::delivery::recovery_class`].
    pub semantics: DeliverySemantics,
    /// The acknowledged decision permitting another attempt at something that
    /// may already have happened.
    ///
    /// `None` is the ordinary send. A delivery that ended `Indeterminate`
    /// against a target whose semantics cannot rule out duplication needs one,
    /// and [`crate::app::publish::authorize_retry`] is how it is obtained.
    pub retry_authorization:
        Option<draft_dcg_contract::publication::PublicationRetryAuthorizationDigest>,
    /// Why this Baseline is being delivered again, when it is.
    ///
    /// Part of the request key, so a republish is a different Publication
    /// rather than a second attempt at the first. `None` is the ordinary
    /// delivery. Retrying the *same* Publication is a retry authorization, and
    /// the two are never interchangeable: one says "send this again on
    /// purpose", the other says "we could not tell whether the first send
    /// happened".
    pub republish_intent: Option<draft_dcg_contract::publication::RepublishIntentId>,
    /// The caller's identity for *this attempt*.
    ///
    /// A surface passes its operation id. Retrying under the same id
    /// recomputes the same attempt and converges on what that attempt already
    /// concluded; a genuinely new send is a new id. This is what stops an HTTP
    /// retry, an SSE reconnect or a re-run command from delivering twice.
    pub request_id: String,
}

/// The route a Publication of this Baseline would take.
///
/// Joins what the Baseline was observed under to what the binding is now. A
/// caller cannot supply this: naming a route the Baseline's own provenance
/// does not support would authorize an external effect at a destination
/// nothing established the state from.
pub fn route_for_baseline(
    workspace: &Workspace,
    baseline: &BaselineId,
) -> DraftResult<ProviderRouteRef> {
    let baselines = BaselineStore::new(workspace.layout.baselines_dir());
    let composition = baselines.composition(baseline)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("baseline {baseline} has no stored composition, so nothing knows what                      established it"),
        )
    })?;

    let mut provenance: Option<&ProviderProvenanceRef> = None;
    for candidate in composition.resource_provenance.values() {
        match provenance {
            None => provenance = Some(candidate),
            Some(existing) if existing == candidate => {}
            // A Publication has one route. Picking one of several would send
            // state observed by one binding under another's authority.
            Some(existing) => {
                return Err(DraftError::new(
                    DraftErrorKind::Validation,
                    format!(
                        "baseline {baseline} was established by more than one binding ({} and                          {}), so it has no single route to publish over",
                        existing.binding, candidate.binding
                    ),
                ))
            }
        }
    }
    let provenance = provenance.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::Validation,
            format!("baseline {baseline} accepted no resources, so there is nothing to deliver"),
        )
    })?;

    let bindings = ProviderBindingStore::new(workspace.layout.provider_bindings_dir());
    let binding = bindings.read_unlocked(&provenance.binding)?;
    route_for(provenance, binding.as_ref()).map_err(|refusal| match refusal {
        RouteRefusal::NotRoutable => DraftError::new(
            DraftErrorKind::Validation,
            format!(
                "the binding {} that established baseline {baseline} is no longer active, so                  there is nowhere to publish it",
                provenance.binding
            ),
        )
        .with_suggestion("The Baseline is still accepted and verifiable; only delivery is blocked."),
        RouteRefusal::SemanticsRedefined => DraftError::new(
            DraftErrorKind::ConflictDetected,
            format!(
                "the binding {} now works under different semantics from the ones baseline                  {baseline} was observed under",
                provenance.binding
            ),
        ),
    })
}

/// Create the Publication this request names, or converge on the existing one.
///
/// The identity is derived from what the request is *about*, so the same
/// delivery of the same Baseline for the same purpose is the same Publication
/// however many times it is asked for.
pub fn ensure_publication(
    workspace: &Workspace,
    request: &PublishRequest,
) -> DraftResult<Publication> {
    let format = |error: draft_dcg_contract::FormatError| {
        DraftError::new(DraftErrorKind::CorruptData, error.to_string())
    };
    let publishable = publishable(workspace, &request.baseline)?;
    let route = route_for_baseline(workspace, &request.baseline)?;

    let request_key = Publication::compute_request_key(
        &publishable.promotion,
        &request.baseline,
        &route,
        &request.purpose,
        request.republish_intent.as_ref(),
    )
    .map_err(format)?;

    // The id follows from the key, and the idempotency key follows from the
    // id: derived in that order, so the object cannot disagree with itself.
    let id = id_for_request_key(&request_key)?;
    let candidate = Publication {
        idempotency_key: Publication::compute_idempotency_key(&id, &request.baseline, &route)
            .map_err(format)?,
        id,
        request_key,
        promotion: publishable.promotion.clone(),
        baseline: request.baseline.clone(),
        route: route.clone(),
        purpose: request.purpose.clone(),
        republish_intent: request.republish_intent.clone(),
        requested_by: crate::app::baseline::actor_id_of(&workspace.layout)?,
        authority_inputs: BTreeSet::new(),
        credential_authority_class: None,
        delivery_semantics: request.semantics,
        created_at: Timestamp::from_unix_nanos(0),
    };

    let store = PublicationStore::for_layout(&workspace.layout);
    match store.create(&candidate)? {
        CreationOutcome::Created(_) => Ok(candidate),
        CreationOutcome::Existing(reference) => store.get(&reference.id)?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!("publication {} is mapped but not stored", reference.id),
            )
        }),
        CreationOutcome::Corrupt { detail } => {
            Err(DraftError::new(DraftErrorKind::CorruptData, detail))
        }
    }
}

/// Publish an already-promoted Baseline through the dispatch engine.
///
/// `deliver` is the external call. It runs inside the engine's dispatch, which
/// invokes it only after every lock is released — this function never holds
/// anything across it, because it never holds anything at all.
///
/// The Baseline is resolved and checked *before* dispatch and never written
/// after it. There is no path from `deliver`'s result to the Baseline store.
pub fn publish(
    workspace: &Workspace,
    request: &PublishRequest,
    deliver: impl FnOnce() -> DeliveryResult,
) -> DraftResult<PublishOutcome> {
    // Refuses anything the project has not promoted, before anything external
    // happens.
    let publication = ensure_publication(workspace, request)?;
    let attempt = attempt_id_for(&publication.id, &request.request_id)?;

    let dispatch_request = DispatchRequest {
        publication: publication.id.clone(),
        attempt: attempt.clone(),
        identity: PrimaryOutcomeIdentity {
            // The reference the engine will replace with the real one once the
            // attempt object exists. Only the receipt and signer travel from
            // here: what the outcome is *about* is decided at the dispatch
            // boundary, from bytes that exist by then.
            attempt: PublicationAttemptRef {
                id: attempt.clone(),
                digest: PublicationAttemptDigest::new(Digest::of_bytes(
                    attempt.as_str().as_bytes(),
                )),
            },
            receipt: receipt_id_for(&attempt)?,
            signer: signer_for(workspace)?,
        },
        // Phase 1's inputs. The route is the one the Publication froze, and
        // the subject is the project — publishing is granted per project, not
        // per delivery, because a grant nobody reads is not a decision.
        authority: crate::publication::authority::AuthorityStores::for_layout(&workspace.layout)?,
        route: publication.route.clone(),
        actor: crate::app::baseline::actor_id_of(&workspace.layout)?,
        subject: crate::publication::authority::publish_subject(&workspace.workspace_id)?,
        operation: crate::support::common::OperationId::new(format!(
            "publish-{}",
            request.request_id
        )),
        started_at: Timestamp::from_unix_nanos(0),
        dispatch_event: event_id_for(&attempt, "dispatch")?,
        outcome_event: event_id_for(&attempt, "outcome")?,
        // Derived from the Publication's own semantics. Nothing above this
        // line could have chosen it.
        semantics: publication.delivery_semantics,
        retry_authorization: request.retry_authorization.clone(),
        provenance: draft_dcg_contract::producer::ProducerIdentity::new(
            draft_dcg_contract::identifier::NamespacedId::parse("draft.core/publication")
                .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
            crate::DRAFT_VERSION,
        )
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
        concluded_at: Timestamp::from_unix_nanos(0),
    };

    let stores = DispatchStores::for_layout(&workspace.layout);
    let progress = dispatch(&stores, &dispatch_request, deliver)?;
    let outcome = PublishOutcome::from(progress);

    // The invariant, asserted at the boundary rather than assumed: whatever
    // the delivery concluded, the project still accepts what it accepted.
    assert_baseline_unchanged(workspace, &request.baseline, &outcome)?;
    Ok(outcome)
}

/// The attempt this caller's request names.
///
/// Derived from the Publication and the caller's request id, so a retry under
/// the same id is the same attempt and converges rather than delivering again.
pub fn attempt_id_for(
    publication: &PublicationId,
    request_id: &str,
) -> DraftResult<PublicationAttemptId> {
    let seed = Digest::of_bytes(format!("attempt|{publication}|{request_id}").as_bytes());
    PublicationAttemptId::parse(format!("pat_{}", short_hex(&seed)))
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

fn receipt_id_for(
    attempt: &PublicationAttemptId,
) -> DraftResult<draft_dcg_contract::ids::ReceiptId> {
    let seed = Digest::of_bytes(format!("publication-receipt|{attempt}").as_bytes());
    draft_dcg_contract::ids::ReceiptId::parse(format!("rcp_{}", short_hex(&seed)))
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

fn event_id_for(attempt: &PublicationAttemptId, phase: &str) -> DraftResult<ActivityEventId> {
    let seed = Digest::of_bytes(format!("publication-event|{attempt}|{phase}").as_bytes());
    ActivityEventId::parse(format!("evt_{}", short_hex(&seed)))
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

fn signer_for(
    workspace: &Workspace,
) -> DraftResult<draft_dcg_contract::receipt::ReceiptSignerBinding> {
    draft_dcg_contract::receipt::ReceiptSignerBinding::new(
        crate::app::baseline::actor_id_of(&workspace.layout)?,
        "draft.core/publication",
        "ed25519",
    )
    .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

fn short_hex(digest: &Digest) -> String {
    digest
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect()
}

/// Deliver a Baseline through the binding its route names.
///
/// The only binding Draft ships is the filesystem one, and what it delivers
/// is the Baseline's manifest written outside the graph — a file somebody else
/// can pick up. That is a genuine external effect: once written, Draft cannot
/// take it back, which is exactly the property the dispatch engine exists to
/// handle.
///
/// It is idempotent by construction: the path is derived from the Publication,
/// and the bytes are derived from the Baseline. Re-sending the same
/// Publication rewrites the same file with the same content, which is why
/// [`filesystem_delivery_semantics`] can say so honestly.
pub fn deliver_to_filesystem(workspace: &Workspace, publication: &Publication) -> DeliveryResult {
    let directory = workspace.layout.exports_dir();
    let path = directory.join(format!("{}.json", publication.id));
    let manifest = match BaselineStore::new(workspace.layout.baselines_dir())
        .manifest(&publication.baseline)
    {
        Ok(Some(manifest)) => manifest,
        Ok(None) => {
            return DeliveryResult::Failed {
                reason: format!("baseline {} is not stored", publication.baseline),
            }
        }
        Err(error) => {
            return DeliveryResult::Failed {
                reason: error.message,
            }
        }
    };
    let encoded = match serde_json::to_vec_pretty(&manifest) {
        Ok(encoded) => encoded,
        Err(error) => {
            return DeliveryResult::Failed {
                reason: error.to_string(),
            }
        }
    };
    if let Err(error) = std::fs::create_dir_all(&directory) {
        return DeliveryResult::Failed {
            reason: error.to_string(),
        };
    }
    match crate::support::fsutil::write_atomic(&path, &encoded) {
        // The path is the external reference: it is how somebody outside Draft
        // names what was delivered.
        Ok(()) => DeliveryResult::Succeeded {
            external_reference: path.display().to_string(),
        },
        // A write that failed part-way is not a failure Draft can rule out
        // having had an effect: the atomic rename either happened or did not,
        // and an io error here does not say which.
        Err(error) => DeliveryResult::Undetermined {
            reason: error.message,
        },
    }
}

/// What the built-in filesystem export guarantees about a repeated send.
///
/// Declared by the binding, not chosen by a caller. Whether re-sending can
/// duplicate an effect is a fact about the target, and a surface that could
/// assert `IdempotentByKey` about a target that is not would be choosing to
/// risk a duplicated external effect on the user's behalf.
pub fn filesystem_delivery_semantics() -> DeliverySemantics {
    DeliverySemantics::IdempotentByKey
}

/// Authorize another attempt at a delivery Draft could not establish.
///
/// The gate this passes is not a technical one. A delivery that ended
/// `Indeterminate` against a target whose semantics cannot rule out
/// duplication is stuck on purpose: re-sending might duplicate a real-world
/// effect, and nothing Draft can read locally will tell it whether it would.
/// So a person decides, and the decision is recorded with what they knew.
///
/// Runs under Phase 1, so the authorization cites the same commit-time
/// authority a dispatch would — proposing something genuinely new is always a
/// fresh decision, evaluated against the security state as it is now.
pub fn authorize_retry(
    workspace: &Workspace,
    request: &PublishRequest,
    rationale: &str,
) -> DraftResult<draft_dcg_contract::publication::PublicationRetryAuthorizationDigest> {
    let publication = ensure_publication(workspace, request)?;
    let stores = DispatchStores::for_layout(&workspace.layout);

    // The outcome that made a retry necessary, and the attempt it concluded.
    // Both are read back rather than described by the caller: an authorization
    // naming an outcome nobody recorded would permit a retry of nothing.
    let (prior_outcome, prior_attempt) = last_conclusion(&stores, &publication.id)?;

    let authority_stores =
        crate::publication::authority::AuthorityStores::for_layout(&workspace.layout)?;
    let authority_request = crate::publication::authority::AuthorityRequest {
        publication: &publication.id,
        route: &publication.route,
        actor: crate::app::baseline::actor_id_of(&workspace.layout)?,
        subject: crate::publication::authority::publish_subject(&workspace.workspace_id)?,
        operation: crate::support::common::OperationId::new(format!(
            "authorize-retry-{}",
            request.request_id
        )),
        evaluator: producer()?,
        now: Timestamp::from_unix_nanos(0),
    };

    let authorization = crate::publication::authority::under_authority(
        &authority_stores,
        &authority_request,
        |authority| {
            if !authority.authority_decision.is_permitted() {
                return Err(DraftError::new(
                    DraftErrorKind::CapabilityNotAuthorized,
                    format!(
                        "publication '{}' is not authorized, so no retry of it can be either",
                        publication.id
                    ),
                ));
            }
            crate::publication::retry::authorize(
                &crate::publication::retry::RetryRequest {
                    publication: &publication,
                    prior_outcome: &prior_outcome,
                    prior_attempt: &prior_attempt,
                    actor: crate::app::baseline::actor_id_of(&workspace.layout)?,
                    rationale: rationale.to_string(),
                    authorized_at: Timestamp::from_unix_nanos(0),
                },
                authority,
            )
        },
    )?;

    // Checked against the Publication it will be spent on, by whole reference
    // rather than by id, before it is stored at all.
    crate::publication::retry::require_targets(&authorization, &publication)?;
    crate::publication::retry::RetryAuthorizationStore::for_layout(&workspace.layout)
        .put(&authorization)
}

/// The outcome this Publication last concluded, and the attempt behind it.
fn last_conclusion(
    stores: &DispatchStores,
    publication: &PublicationId,
) -> DraftResult<(
    draft_dcg_contract::publication::PublicationOutcome,
    draft_dcg_contract::publication::PublicationAttempt,
)> {
    for attempt in stores.journals.list()? {
        let Some(record) = stores.journals.read_unlocked(&attempt)? else {
            continue;
        };
        if &record.publication != publication {
            continue;
        }
        let Some(reference) = record.state.concluded_attempt() else {
            continue;
        };
        let Some(outcome) = stores.outcomes.primary_outcome(reference)? else {
            continue;
        };
        let Some(object) = stores.attempts.get(reference)? else {
            continue;
        };
        return Ok((outcome, object));
    }
    Err(DraftError::new(
        DraftErrorKind::NotFound,
        format!(
            "publication '{publication}' has concluded no attempt, so there is nothing to \
             authorize a retry of"
        ),
    ))
}

fn producer() -> DraftResult<draft_dcg_contract::producer::ProducerIdentity> {
    draft_dcg_contract::producer::ProducerIdentity::new(
        draft_dcg_contract::identifier::NamespacedId::parse("draft.core/publication")
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
        crate::DRAFT_VERSION,
    )
    .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

/// Withdraw an allocated attempt whose dispatch is no longer permitted.
///
/// The one interruption Draft can resolve without asking anybody. An attempt
/// that never reached the dispatch boundary made no external call — the
/// journal proves it — so the allocation can be withdrawn and the Publication
/// freed. Without this it blocks every later attempt forever: the barrier
/// correctly refuses to start a second one, and nothing was able to prove the
/// first caused no effect.
///
/// Runs the phase discipline in full: classify holding nothing, revalidate
/// under Phase 1's guards, then commit under those same guards.
pub fn withdraw_stalled_attempt(
    workspace: &Workspace,
    request: &PublishRequest,
    reason: &str,
) -> DraftResult<crate::publication::abandon::Withdrawn> {
    let publication = ensure_publication(workspace, request)?;
    let stores = DispatchStores::for_layout(&workspace.layout);
    let attempt = attempt_id_for(&publication.id, &request.request_id)?;

    // Phase AR0, holding nothing at the end.
    let Some(boundary) = crate::publication::abandon::classify(&stores, &attempt)? else {
        return Ok(crate::publication::abandon::Withdrawn::NotWithdrawable {
            detail: format!(
                "attempt '{attempt}' is not an allocated attempt awaiting dispatch, so there is \
                 nothing to withdraw"
            ),
        });
    };

    let authority_stores =
        crate::publication::authority::AuthorityStores::for_layout(&workspace.layout)?;
    let authority_request = crate::publication::authority::AuthorityRequest {
        publication: &publication.id,
        route: &publication.route,
        actor: crate::app::baseline::actor_id_of(&workspace.layout)?,
        subject: crate::publication::authority::publish_subject(&workspace.workspace_id)?,
        operation: crate::support::common::OperationId::new(format!("withdraw-{attempt}")),
        evaluator: producer()?,
        now: Timestamp::from_unix_nanos(0),
    };

    // Phases AR1 and AC, under one continuous set of guards. Deciding to
    // abandon on one security snapshot and committing under another would let
    // the two disagree about whether abandoning was even the right answer.
    crate::publication::authority::under_authority(
        &authority_stores,
        &authority_request,
        |_authority| {
            crate::publication::abandon::withdraw(
                &stores,
                &boundary,
                &crate::publication::recovery::FreshValidation::RefusesDispatch {
                    reason: reason.to_string(),
                },
                event_id_for(&attempt, "abandonment")?,
                Timestamp::from_unix_nanos(0),
            )
        },
    )
}

/// Record what was later established about an uncertain delivery.
///
/// The primary outcome is never rewritten. An `Indeterminate` outcome is the
/// honest record of what Draft could establish at the time, and rewriting it
/// would make that unrecoverable — "we did not know, then we learned" is a
/// different history from "we knew all along", and only one of them explains
/// why a retry authorization was issued in between.
///
/// So this writes a Resolution alongside it, citing current authority: an
/// interpretation is a new decision however old the outcome is.
pub fn resolve_outcome(
    workspace: &Workspace,
    request: &PublishRequest,
    expected_outcome: Option<&draft_dcg_contract::publication::PublicationOutcomeDigest>,
    resolution: draft_dcg_contract::publication::PublicationResolutionKind,
    rationale: &str,
) -> DraftResult<draft_dcg_contract::publication::PublicationResolutionDigest> {
    let publication = ensure_publication(workspace, request)?;
    let stores = DispatchStores::for_layout(&workspace.layout);
    let (outcome, _attempt) = last_conclusion(&stores, &publication.id)?;

    // §2.46: a Resolution names the exact outcome whose head it advances. A
    // caller that named one is held to it — resolving "whatever concluded most
    // recently" when they meant a specific outcome would attach an
    // interpretation to the wrong fact.
    if let Some(expected) = expected_outcome {
        let recomputed = outcome
            .digest()
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;
        if &recomputed != expected {
            crate::support::telemetry::Counter::PublicationSelfConsistencyRejections.increment();
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "publication '{}' most recently concluded {recomputed}, not the outcome \
                     {expected} this resolution names",
                    publication.id
                ),
            ));
        }
    }

    let authority_stores =
        crate::publication::authority::AuthorityStores::for_layout(&workspace.layout)?;
    let authority_request = crate::publication::authority::AuthorityRequest {
        publication: &publication.id,
        route: &publication.route,
        actor: crate::app::baseline::actor_id_of(&workspace.layout)?,
        subject: crate::publication::authority::publish_subject(&workspace.workspace_id)?,
        operation: crate::support::common::OperationId::new(format!(
            "resolve-{}",
            request.request_id
        )),
        evaluator: producer()?,
        now: Timestamp::from_unix_nanos(0),
    };

    let store = crate::publication::resolve::ResolutionStore::for_layout(&workspace.layout);
    crate::publication::authority::under_authority(
        &authority_stores,
        &authority_request,
        |authority| {
            if !authority.authority_decision.is_permitted() {
                return Err(DraftError::new(
                    DraftErrorKind::CapabilityNotAuthorized,
                    format!(
                        "publication '{}' is not authorized, so its outcome cannot be resolved",
                        publication.id
                    ),
                ));
            }
            crate::publication::resolve::resolve(
                &store,
                &crate::publication::resolve::ResolveRequest {
                    outcome: &outcome,
                    resolution: resolution.clone(),
                    actor: crate::app::baseline::actor_id_of(&workspace.layout)?,
                    signer: signer_for(workspace)?,
                    receipt: receipt_id_for(&attempt_id_for(
                        &publication.id,
                        &format!("resolution-{}", request.request_id),
                    )?)?,
                    rationale: rationale.to_string(),
                    resolved_at: Timestamp::from_unix_nanos(0),
                },
                authority,
            )
        },
    )
}
