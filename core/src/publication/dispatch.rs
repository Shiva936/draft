//! The dispatch driver: Phase 0 through the external call and back.
//!
//! The pieces of the publication protocol each own one decision — the barrier
//! classifies local state, the control record allocates, the journal records
//! what was committed, the restart table says what an interrupted attempt
//! means. This runs them in the frozen order and is the only thing that does.
//!
//! ```text
//! PHASE 0  local bookkeeping barrier      ends holding nothing
//! PHASE 2  allocate: journal(6) → control(7)
//! PHASE 3  durable Dispatching, then release EVERYTHING
//!          ──────────────────────────────────────────────
//!          only now is the external system called
//! PHASE A  converge with the journal, record the primary outcome
//! ```
//!
//! # Why the external call is not made by this module
//!
//! The caller supplies a closure that performs the delivery, and this runs it
//! only after every lock is released. Putting the call inside would make it
//! possible — eventually inevitable — for somebody to hold a guard across it,
//! and an unreachable external system would then hold the project's own
//! records hostage.
//!
//! # Why a barrier result other than `Clean` never allocates
//!
//! `Clean` is the only value meaning "no unresolved local state". Every other
//! one says something about this Publication is unfinished, and allocating
//! past it would start a second external effect against a Publication Draft
//! cannot yet describe.

use draft_dcg_contract::ids::{ActivityEventId, PublicationAttemptId, PublicationId};
use draft_dcg_contract::publication::{
    DeliverySemantics, PublicationAttempt, PublicationAttemptRef, PublicationOutcome,
    PublicationOutcomeKind, PublicationRetryAuthorizationDigest,
};

use crate::publication::authority::{under_authority, AuthorityRequest, AuthorityStores};
use crate::publication::barrier::PublicationBookkeepingResult;
use crate::publication::barrier::{classify as classify_barrier, BarrierInputs};
use crate::publication::completion::prepare_and_record_primary_outcome;
use crate::publication::consistency::verify_dispatched_attempt;
use crate::publication::control::{PublicationControl, PublicationControlStore};
use crate::publication::delivery::recovery_class;
use crate::publication::journal::{
    AttemptJournal, AttemptJournalState, ControlTransition, PublicationJournalStore,
};
use crate::publication::outcome::{PrimaryOutcomeIdentity, PublicationOutcomeStore};
use crate::publication::recovery::recovery_may_allocate;
use crate::publication::restart::{
    classify as classify_attempt, AttemptResolution, AttemptSnapshot, OutcomePresence,
};
use crate::publication::store::{PublicationAttemptStore, PublicationStore};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// The stores one Publication's dispatch drives.
pub struct DispatchStores {
    pub control: PublicationControlStore,
    pub journals: PublicationJournalStore,
    pub outcomes: PublicationOutcomeStore,
    /// Where the immutable attempt objects live.
    pub attempts: PublicationAttemptStore,
    /// The Publications themselves, so an attempt can be checked against the
    /// exact object it claims rather than against its id.
    pub publications: PublicationStore,
    /// Where this project's publication records live, so the retry
    /// authorizations can be resolved from the same root.
    pub publication_root: std::path::PathBuf,
}

impl DispatchStores {
    pub fn for_layout(layout: &crate::project::layout::DraftLayout) -> Self {
        let root = layout.publication_dir();
        Self {
            control: PublicationControlStore::new(root.join("control")),
            journals: PublicationJournalStore::new(root.join("journal")),
            outcomes: PublicationOutcomeStore::new(root.join("outcome")),
            attempts: PublicationAttemptStore::for_layout(layout),
            publications: PublicationStore::for_layout(layout),
            publication_root: root,
        }
    }
}

/// What the external system reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryResult {
    Succeeded {
        external_reference: String,
    },
    Failed {
        reason: String,
    },
    /// The call did not complete in a way that establishes whether the effect
    /// occurred. Recorded honestly rather than guessed either way.
    Undetermined {
        reason: String,
    },
}

/// What dispatch concluded, projected from the engine's durable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchProgress {
    /// The attempt dispatched and its primary outcome is recorded.
    Completed {
        attempt: PublicationAttemptId,
        outcome: PublicationOutcomeKind,
    },
    /// An earlier attempt already concluded this Publication.
    AlreadyConcluded {
        attempt: PublicationAttemptId,
        outcome: PublicationOutcomeKind,
    },
    /// An earlier attempt must be resumed before a new one may begin.
    ResumeRequired { attempt: PublicationAttemptId },
    /// An earlier attempt may have caused an effect Draft cannot yet describe.
    ///
    /// Nothing local blocks; this Publication may not dispatch again until the
    /// effect is resolved.
    AwaitingExternalResolution { attempt: PublicationAttemptId },
    /// The local records contradict each other.
    Inconsistent { detail: String },
}

/// Begin the Publication's control record if it has none.
pub fn ensure_control(stores: &DispatchStores, publication: &PublicationId) -> DraftResult<()> {
    if stores.control.read_unlocked(publication)?.is_none() {
        stores
            .control
            .initialize(&PublicationControl::initial(publication.clone()))?;
    }
    Ok(())
}

/// Run Phase 0: what does local state say about this Publication?
///
/// Reads every attempt journal the control record and store know about, and
/// classifies each through the restart table before the barrier decides.
///
/// Takes the Publication's `DeliverySemantics`, not a recovery class. What an
/// unresolved attempt may be concluded to mean follows from what the delivery
/// guarantees, so it is derived here through
/// [`crate::publication::delivery::recovery_class`] rather than chosen by a
/// caller — a caller who could pick `ResolvableLocally` for non-idempotent
/// delivery would be choosing to risk a duplicated external effect.
pub fn bookkeeping(
    stores: &DispatchStores,
    publication: &PublicationId,
    semantics: DeliverySemantics,
) -> DraftResult<PublicationBookkeepingResult> {
    let recovery_class = recovery_class(semantics);
    let control = stores.control.read_unlocked(publication)?;
    let mut attempts = Vec::new();

    if let Some(in_flight) = control.as_ref().and_then(|c| c.in_flight_attempt.clone()) {
        let journal = stores.journals.read_unlocked(&in_flight)?;
        let resolution = match journal {
            Some(record) => {
                let outcome = primary_outcome_presence(stores, &in_flight)?;
                classify_attempt(&AttemptSnapshot {
                    attempt: &in_flight,
                    journal: &record.state,
                    control: control.as_ref(),
                    outcome: &outcome,
                    recovery_class,
                })
            }
            // A control record naming an attempt with no journal: Draft cannot
            // prove whether the request was dispatched, so it neither clears
            // the reference nor allocates past it.
            None => AttemptResolution::Inconsistent {
                detail: "the control record holds an attempt whose journal is missing",
            },
        };
        attempts.push((in_flight, resolution));
    }

    Ok(classify_barrier(&BarrierInputs {
        attempts: &attempts,
        in_flight_attempt: control.as_ref().and_then(|c| c.in_flight_attempt.as_ref()),
        control_readable: control.is_some(),
        undrained_local_work: false,
        unfinished_receipt_finalization: false,
    }))
}

/// Whether this attempt has a recorded primary outcome.
///
/// The reference comes from the journal, which recorded the exact bytes at the
/// dispatch boundary. Deriving one from the id instead would make every
/// reference to an attempt compare equal to every other with the same id,
/// which is precisely the substitution the digest exists to catch.
fn primary_outcome_presence(
    stores: &DispatchStores,
    attempt: &PublicationAttemptId,
) -> DraftResult<OutcomePresence> {
    let Some(record) = stores.journals.read_unlocked(attempt)? else {
        return Ok(OutcomePresence::Absent);
    };
    // Only a dispatched attempt can have an outcome, and only a dispatched
    // journal state names the reference to look it up by.
    let Some(reference) = record.state.dispatched_attempt() else {
        return Ok(OutcomePresence::Absent);
    };
    Ok(match stores.outcomes.primary_outcome(reference)? {
        Some(outcome) => OutcomePresence::Present(
            outcome
                .digest()
                .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
        ),
        None => OutcomePresence::Absent,
    })
}

/// Check a dispatched attempt against everything outside itself.
///
/// §2.46's cross-record half: the attempt's own digest proves its bytes have
/// not changed and says nothing about whether it agrees with the Publication
/// it claims or the journal that dispatched it. Run at both boundaries where
/// an attempt is read back — resuming one, and concluding one — because those
/// are the two moments Draft acts on an attempt it did not just construct.
fn verify_against_records(
    stores: &DispatchStores,
    publication: &PublicationId,
    reference: &PublicationAttemptRef,
    journal: &AttemptJournalState,
) -> DraftResult<()> {
    let attempt = stores.attempts.get(reference)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!(
                "attempt '{}' reached the dispatch boundary but its object is not stored",
                reference.id
            ),
        )
    })?;
    let publication = stores.publications.get(publication)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!(
                "attempt '{}' claims a publication that is not stored",
                reference.id
            ),
        )
    })?;
    verify_dispatched_attempt(&attempt, reference, &publication, journal)
}

/// Everything one dispatch needs.
pub struct DispatchRequest {
    pub publication: PublicationId,
    pub attempt: PublicationAttemptId,
    pub identity: PrimaryOutcomeIdentity,
    pub dispatch_event: ActivityEventId,
    pub outcome_event: ActivityEventId,
    /// What the target guarantees about repeated delivery.
    ///
    /// The recovery class is derived from this and never supplied directly:
    /// see [`bookkeeping`].
    pub semantics: DeliverySemantics,
    /// The stores Phase 1 establishes authority from.
    pub authority: AuthorityStores,
    /// The exact route the Publication froze. Re-validated against the
    /// binding's current pointers, never re-resolved.
    pub route: draft_dcg_contract::provider::ProviderRouteRef,
    /// Whose authority the effect would happen under.
    pub actor: draft_dcg_contract::ids::ActorId,
    /// What the publish capability is claimed over.
    pub subject: draft_dcg_contract::identifier::ScopedId,
    /// Identifies this attempt to the publication lease.
    pub operation: crate::support::common::OperationId,
    pub provenance: draft_dcg_contract::producer::ProducerIdentity,
    /// The one-shot authorization permitting another attempt at something that
    /// may already have happened.
    ///
    /// Required only where the delivery semantics cannot rule out duplication;
    /// `None` is the ordinary first send. Spending it is the control record's
    /// job — see [`crate::publication::control::PublicationControl::plan_allocation`]
    /// — because the only place "has this been spent?" can be answered without
    /// a race is inside the lock that commits the allocation.
    pub retry_authorization: Option<PublicationRetryAuthorizationDigest>,
    pub started_at: draft_dcg_contract::value::Timestamp,
    pub concluded_at: draft_dcg_contract::value::Timestamp,
}

/// Dispatch one attempt, calling `deliver` with no lock held.
pub fn dispatch(
    stores: &DispatchStores,
    request: &DispatchRequest,
    deliver: impl FnOnce() -> DeliveryResult,
) -> DraftResult<DispatchProgress> {
    ensure_control(stores, &request.publication)?;

    // Phase 0. Only `Clean` continues into a new attempt.
    match bookkeeping(stores, &request.publication, request.semantics)? {
        PublicationBookkeepingResult::Clean => {}
        PublicationBookkeepingResult::RecoverAllocatedAttempt { attempt } => {
            return resume_or_report(stores, attempt)
        }
        PublicationBookkeepingResult::PendingExternalResolution { attempt } => {
            return Ok(DispatchProgress::AwaitingExternalResolution { attempt })
        }
        PublicationBookkeepingResult::Inconsistent { detail } => {
            return Ok(DispatchProgress::Inconsistent { detail })
        }
    }

    // This exact attempt has been here before. A surface retry recomputes the
    // same attempt id, and it must converge on what that attempt concluded
    // rather than allocating a second one — allocation would refuse the reused
    // id, but as an integrity error rather than as the answer the caller asked
    // for.
    if stores.journals.read_unlocked(&request.attempt)?.is_some() {
        return resume_or_report(stores, request.attempt.clone());
    }

    // Phases 1 through 3, under one continuous set of guards.
    //
    // Everything the attempt records about the project — its generation, its
    // policy, its security state, the registry revisions, the binding — is read
    // inside this call and stays true until the dispatch boundary is durable.
    // Releasing and reacquiring between reading those facts and committing the
    // attempt would let it assert authority that had already been withdrawn.
    let authority_request = AuthorityRequest {
        publication: &request.publication,
        route: &request.route,
        actor: request.actor.clone(),
        subject: request.subject.clone(),
        operation: request.operation.clone(),
        evaluator: request.provenance.clone(),
        now: request.started_at,
    };
    let (allocation, dispatched) =
        under_authority(&request.authority, &authority_request, |authority| {
            // A refusal is a decision, recorded and then acted on. The attempt
            // object refuses to hold a non-permitted decision, so this must be
            // checked before one is built rather than after.
            if !authority.authority_decision.is_permitted() {
                return Err(refusal(&request.publication, &authority.authority_decision));
            }

            // An authorization is bound to one exact Publication — id *and*
            // digest. Checking it here, where it is spent, is what stops one
            // issued for an uncertain export being used to send an announce,
            // or surviving the bytes under its Publication changing beneath
            // it. Creation checks the same thing, but creation is not the
            // moment the external effect happens.
            require_authorization_targets(stores, request)?;

            // Phase 2: journal (6) outermost, control (7) nested, each once.
            let (allocation, attempt_number) =
                stores
                    .journals
                    .with_locked_attempt(&request.attempt, |journal| {
                        stores
                            .control
                            .with_locked_control(&request.publication, |control| {
                                let planned = control.plan_allocation(
                                    request.attempt.clone(),
                                    request.retry_authorization.clone(),
                                )?;
                                let allocation = ControlTransition {
                                    expected: planned.expected_control.clone(),
                                    planned: planned.planned_control.clone(),
                                };
                                journal.open(&AttemptJournal {
                                    generation: 0,
                                    attempt: request.attempt.clone(),
                                    publication: request.publication.clone(),
                                    state: AttemptJournalState::AttemptPrepared {
                                        candidate_attempt_number: planned.candidate_attempt_number,
                                        allocation: allocation.clone(),
                                        retry_authorization: request.retry_authorization.clone(),
                                    },
                                })?;
                                control.commit_allocation(&planned)?;
                                Ok((allocation, planned.candidate_attempt_number))
                            })
                    })?;

            // The immutable attempt, built from what Phase 1 established and
            // the number the allocation actually reserved. Written before the
            // journal names it: a boundary naming an object nobody stored
            // would be a dispatch nothing could describe.
            let attempt = build_attempt(stores, request, authority, attempt_number)?;
            let dispatched = stores.attempts.put(&attempt)?;

            // Phase 3: the durable dispatch boundary, naming the exact bytes.
            stores
                .journals
                .with_locked_attempt(&request.attempt, |journal| {
                    journal.transition_locked(
                        &AttemptJournalState::AttemptPrepared {
                            candidate_attempt_number: attempt_number,
                            allocation: allocation.clone(),
                            retry_authorization: request.retry_authorization.clone(),
                        },
                        AttemptJournalState::Dispatching {
                            attempt: dispatched.clone(),
                            attempt_number,
                            dispatch_event: request.dispatch_event.clone(),
                        },
                    )?;
                    Ok(())
                })?;

            Ok((allocation, dispatched))
        })?;

    // No lock, no lease, no guard is held here.
    let delivered = deliver();

    record_outcome(stores, request, &allocation, &dispatched, delivered)
}

/// Refuse a retry authorization that is not this Publication's.
fn require_authorization_targets(
    stores: &DispatchStores,
    request: &DispatchRequest,
) -> DraftResult<()> {
    let Some(digest) = request.retry_authorization.as_ref() else {
        return Ok(());
    };
    let authorization = crate::publication::retry::RetryAuthorizationStore::for_layout_root(
        &stores.publication_root,
    )
    .get(digest)?
    .ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("retry authorization {digest} is not stored, so it authorizes nothing"),
        )
    })?;
    let publication = stores
        .publications
        .get(&request.publication)?
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                format!("publication '{}' is not stored", request.publication),
            )
        })?;
    crate::publication::retry::require_targets(&authorization, &publication)
}

/// The error a refused authority decision produces.
///
/// Carries the reason the evaluation gave rather than a generic denial: "not
/// authorized" without saying which grant was missing, revoked or expired is
/// not something anybody can act on.
fn refusal(
    publication: &PublicationId,
    decision: &draft_dcg_contract::authority::AuthorityDecision,
) -> DraftError {
    let reason = match &decision.outcome {
        draft_dcg_contract::authority::AuthorityDecisionOutcome::Refused { reason } => {
            reason.clone()
        }
        draft_dcg_contract::authority::AuthorityDecisionOutcome::Permitted => {
            "the decision was permitted".to_string()
        }
    };
    DraftError::new(
        DraftErrorKind::CapabilityNotAuthorized,
        format!("publication '{publication}' is not authorized: {reason}"),
    )
    .with_suggestion(
        "Grant publish authority over this project, then publish again. Publishing is its own \
         capability: permission to accept work into a Baseline is not permission to announce it.",
    )
}

/// The immutable attempt this dispatch is about to make.
fn build_attempt(
    stores: &DispatchStores,
    request: &DispatchRequest,
    authority: &crate::publication::authority::DispatchAuthority,
    attempt_number: u32,
) -> DraftResult<PublicationAttempt> {
    let publication = stores
        .publications
        .get(&request.publication)?
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                format!("publication '{}' is not stored", request.publication),
            )
        })?;
    let reference = publication
        .reference()
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;
    Ok(PublicationAttempt {
        id: request.attempt.clone(),
        publication: reference,
        attempt_number,
        route: authority.route.clone(),
        attempt_authority: authority.attempt_authority.clone(),
        authority_decision: authority.authority_decision.clone(),
        project_control_generation_at_dispatch: authority.project_control_generation,
        project_security_state_at_dispatch: authority.project_security_state.clone(),
        policy_digest_at_dispatch: authority.policy_digest.clone(),
        global_registry_revisions_at_dispatch: authority.registry_revisions.clone(),
        provider_binding_generation_at_dispatch: authority.binding_generation,
        retry_authorization: request.retry_authorization.clone(),
        lease_id: authority.lease.lease_id.clone(),
        lease_fence: authority.lease.fence,
        started_at: request.started_at,
        provenance: request.provenance.clone(),
    })
}

/// Resume an attempt Phase 0 says is already allocated.
///
/// This function reports; it never allocates. That rule is
/// [`crate::publication::recovery::recovery_may_allocate`], read here rather
/// than left for a reviewer to notice — recovery and creation being one
/// transaction is how a resumed attempt turns into a second external effect.
fn resume_or_report(
    stores: &DispatchStores,
    attempt: PublicationAttemptId,
) -> DraftResult<DispatchProgress> {
    debug_assert!(
        !recovery_may_allocate(),
        "recovery resumes the attempt that exists; it never creates another"
    );
    let Some(record) = stores.journals.read_unlocked(&attempt)? else {
        return Ok(DispatchProgress::Inconsistent {
            detail: format!("attempt '{attempt}' has no journal to resume"),
        });
    };

    // The restart boundary. An attempt that reached the dispatch boundary is
    // checked against the Publication it claims and the journal that recorded
    // it *before* anything is concluded from it — this is exactly the moment
    // Draft acts on an attempt it did not construct in this process, and a
    // substituted one would otherwise be indistinguishable.
    if let Some(reference) = record.state.dispatched_attempt() {
        if let Err(error) =
            verify_against_records(stores, &record.publication, reference, &record.state)
        {
            return Ok(DispatchProgress::Inconsistent {
                detail: error.message,
            });
        }
    }

    match &record.state {
        // Concluded already: the recorded outcome is authoritative and this
        // call is not a second candidate for it. Looked up by the exact
        // reference the journal recorded, so a stored outcome for different
        // bytes under the same id does not answer for this attempt.
        AttemptJournalState::OutcomeRecorded {
            attempt: reference, ..
        } => {
            let outcome = stores
                .outcomes
                .primary_outcome(reference)?
                .map(|value| value.outcome);
            Ok(match outcome {
                Some(outcome) => DispatchProgress::AlreadyConcluded { attempt, outcome },
                None => DispatchProgress::ResumeRequired { attempt },
            })
        }
        _ => Ok(DispatchProgress::ResumeRequired { attempt }),
    }
}

/// Phase A: converge with the journal, then commit the primary outcome.
fn record_outcome(
    stores: &DispatchStores,
    request: &DispatchRequest,
    allocation: &ControlTransition,
    dispatched: &PublicationAttemptRef,
    delivered: DeliveryResult,
) -> DraftResult<DispatchProgress> {
    // The completion boundary. The attempt is read back here rather than
    // carried, so what the outcome concludes about is checked against the
    // Publication it claims and the journal that dispatched it.
    let journal = stores
        .journals
        .read_unlocked(&request.attempt)?
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "attempt '{}' lost its journal mid-dispatch",
                    request.attempt
                ),
            )
        })?;
    verify_against_records(stores, &request.publication, dispatched, &journal.state)?;

    let kind = match delivered {
        DeliveryResult::Succeeded { external_reference } => {
            PublicationOutcomeKind::Succeeded { external_reference }
        }
        DeliveryResult::Failed { reason } => PublicationOutcomeKind::Failed { reason },
        DeliveryResult::Undetermined { reason } => PublicationOutcomeKind::Indeterminate { reason },
    };
    let candidate = PublicationOutcome {
        attempt: dispatched.clone(),
        receipt_id: request.identity.receipt.clone(),
        receipt_signer: request.identity.signer.clone(),
        outcome: kind.clone(),
        concluded_at: request.concluded_at,
        provenance: request.provenance.clone(),
    };

    let clear = ControlTransition {
        expected: allocation.planned.clone(),
        planned: allocation.planned.advanced(|next| {
            next.in_flight_attempt = None;
        }),
    };

    // The identity names the exact attempt the journal dispatched, not the one
    // the caller guessed at before it existed. The caller supplies the receipt
    // and its signer; what the outcome is *about* is settled here, from bytes
    // that are durable by now.
    let identity = PrimaryOutcomeIdentity {
        attempt: dispatched.clone(),
        receipt: request.identity.receipt.clone(),
        signer: request.identity.signer.clone(),
    };

    stores
        .journals
        .with_locked_attempt(&request.attempt, |journal| {
            prepare_and_record_primary_outcome(
                journal,
                &stores.outcomes,
                &identity,
                &candidate,
                request.outcome_event.clone(),
                clear.clone(),
            )
        })?;

    // The exact clear, by whole value on both sides.
    stores
        .control
        .with_locked_control(&request.publication, |control| {
            control.commit_exact(&clear.expected, &clear.planned)
        })?;

    Ok(DispatchProgress::Completed {
        attempt: request.attempt.clone(),
        outcome: kind,
    })
}
