//! Phase 1: the authority an external effect happens under.
//!
//! Publication is the one thing Draft does that the world can see, so before
//! an attempt exists there has to be an answer to *whose* authority it occurs
//! under, *what* that authority was granted over, and *what state* the project
//! was in when it was decided. This is where those answers are established,
//! and the whole point is that they are established **under guards that stay
//! held** until the dispatch boundary is durable.
//!
//! ```text
//! fence (1)   the trust registry, frozen so two reads are comparable
//!   lease (3) this project's publication lease, fenced and monotonic
//!     control (4)  the project's generation, policy and security state
//!       binding (5)  the route, required to still select
//!         ── everything above stays held ──
//!         Phase 2  allocate       journal (6) → publication control (7)
//!         Phase 3  durable Dispatching
//!         ─────────────────────────────────────────────────────────────
//!         release EVERYTHING, then call the external system
//! ```
//!
//! # Why the guards are held across Phases 2 and 3 rather than re-taken
//!
//! Everything Phase 1 reads goes into the immutable `PublicationAttempt`: the
//! control generation, the policy, the security state, the registry revisions,
//! the binding generation. Those are claims about the moment the effect was
//! authorized. If the guards were released and reacquired, the attempt could
//! record a security state that had already been superseded — an external
//! effect asserting authority that had been withdrawn before it happened, with
//! nothing in the record to show it.
//!
//! So there is no re-read here and no revalidation: one acquisition, one set of
//! facts, held until the attempt is durable.
//!
//! # Why the lease is separate from the fence
//!
//! The fence stabilizes what everyone can read; the lease decides who may act.
//! A reader holding the fence must not thereby be able to publish, and a
//! publisher holding the lease must not thereby freeze everybody's reads for
//! the duration of a network call. They are different questions and they are
//! released at different times.
//!
//! # Why a refusal is still recorded
//!
//! [`evaluate`] returns a refused `AuthorityDecision` rather than an error, and
//! the caller refuses on it. "We asked and were told no" is a materially
//! different fact from "we never asked", and only one of them is worth keeping.

use std::collections::BTreeSet;
use std::time::Duration;

use draft_dcg_contract::authority::AuthorityDecision;
use draft_dcg_contract::identifier::ScopedId;
use draft_dcg_contract::ids::{ActorId, PublicationId};
use draft_dcg_contract::producer::ProducerIdentity;
use draft_dcg_contract::provider::ProviderRouteRef;
use draft_dcg_contract::security::{PolicyDigest, ProjectSecurityStateDigest, SecurityFactRef};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::{ProjectControlGeneration, ProviderBindingGeneration, RegistryRevisions};

use crate::authority::evaluation::{evaluate, AuthorityClaim, AuthorityInputs};
use crate::authority::grant::AuthorityGrantStore;
use crate::authority::revocation::AuthorityRevocationStore;
use crate::execution::lease::{LeaseRef, LeaseStore};
use crate::project::control::ProjectControlStore;
use crate::project::provider::ProviderBindingStore;
use crate::project::security::ProjectSecurityStateStore;
use crate::support::common::OperationId;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::lock_order::{self, LockOrder};
use crate::trust::fence::TrustReadFence;

/// How long Phase 1 waits for each guard before giving up.
///
/// Bounded rather than indefinite: a publication that cannot get the lease is
/// a publication somebody else is already making, and blocking forever would
/// turn a busy project into a hung one.
pub const PHASE_ONE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the publication lease is held.
///
/// Long enough to cover an external call, short enough that a crashed
/// publisher does not hold the project indefinitely. The fence is what makes
/// expiry safe: a lease that lapses and is retaken carries a higher fence, so
/// a returning straggler's writes are recognisable as stale.
pub const LEASE_TTL_SECONDS: i64 = 300;

/// The scope the publication lease is taken under.
pub const PUBLICATION_LEASE_SCOPE: &str = "publication";

/// Everything Phase 1 established, as one value.
///
/// Every field goes into the immutable `PublicationAttempt`, so this is the
/// complete answer to "what was true when this effect was authorized?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchAuthority {
    /// The exact route, validated against the binding's current pointers.
    pub route: ProviderRouteRef,
    /// The exact security facts cited.
    pub attempt_authority: BTreeSet<SecurityFactRef>,
    /// The commit-time decision, so recovery never needs current authority.
    pub authority_decision: AuthorityDecision,
    pub project_control_generation: ProjectControlGeneration,
    pub project_security_state: ProjectSecurityStateDigest,
    pub policy_digest: PolicyDigest,
    pub registry_revisions: RegistryRevisions,
    pub binding_generation: ProviderBindingGeneration,
    pub lease: LeaseRef,
}

/// The stores Phase 1 reads.
///
/// Passed in rather than located, so the facts an attempt records and the ones
/// the caller validated under its guards cannot come from different places.
pub struct AuthorityStores {
    pub control: ProjectControlStore,
    pub bindings: ProviderBindingStore,
    pub security_states: ProjectSecurityStateStore,
    pub grants: AuthorityGrantStore,
    pub revocations: AuthorityRevocationStore,
    pub leases: LeaseStore,
    /// Where the trust registry lives.
    pub global_store: std::path::PathBuf,
}

impl AuthorityStores {
    pub fn for_layout(layout: &crate::project::layout::DraftLayout) -> DraftResult<Self> {
        Ok(Self {
            control: ProjectControlStore::new(layout.project_control_dir()),
            bindings: ProviderBindingStore::new(layout.provider_bindings_dir()),
            security_states: ProjectSecurityStateStore::for_layout(layout),
            grants: AuthorityGrantStore::new(layout.authority_grants_dir()),
            revocations: AuthorityRevocationStore::new(layout.authority_revocations_dir()),
            leases: LeaseStore::at(layout.leases_dir()),
            global_store: layout.trust_registry_dir(),
        })
    }
}

/// What Phase 1 needs told, as opposed to what it reads.
pub struct AuthorityRequest<'a> {
    pub publication: &'a PublicationId,
    /// The route the Publication froze. Re-validated, never re-resolved.
    pub route: &'a ProviderRouteRef,
    pub actor: ActorId,
    /// The subject the publish capability is claimed over.
    pub subject: ScopedId,
    /// Identifies this attempt to the lease, so a retry reuses its lease
    /// rather than contending with itself.
    pub operation: OperationId,
    pub evaluator: ProducerIdentity,
    pub now: Timestamp,
}

/// Establish authority, then run `body` with every guard still held.
///
/// `body` is Phases 2 and 3. It returns when the dispatch boundary is durable,
/// and **every guard is released before this function returns** — which is what
/// lets the caller make the external call holding nothing.
///
/// The nesting is the frozen order and there is no other way to reach the
/// inside of it, so a caller cannot acquire these in a different sequence or
/// forget one.
pub fn under_authority<R>(
    stores: &AuthorityStores,
    request: &AuthorityRequest<'_>,
    body: impl FnOnce(&DispatchAuthority) -> DraftResult<R>,
) -> DraftResult<R> {
    // (1) The trust registry, frozen for the duration.
    let fence = TrustReadFence::acquire(&stores.global_store, PHASE_ONE_TIMEOUT)?;
    let registry_revisions = fence.observed_revisions()?;

    // (3) The publication lease. Registered with the ordering discipline as
    // well as acquired, because the lease is a durable record rather than a
    // held file lock and would otherwise be invisible to it.
    let _rank = lock_order::enter(LockOrder::PublicationLease)?;
    let lease = stores.leases.acquire(
        PUBLICATION_LEASE_SCOPE,
        request.operation.clone(),
        chrono::Duration::seconds(LEASE_TTL_SECONDS),
    )?;
    let held = LeaseGuard {
        leases: &stores.leases,
        lease,
    };
    let lease_ref = LeaseRef::of(&held.lease)?;

    // (4) Project control: the generation, policy and security state this
    // effect happens under.
    let outcome = stores.control.with_locked_control(|control| {
        let state = control.current()?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                "this project has no control record, so nothing can be published from it",
            )
        })?;
        if !state.is_active() {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "project {} is {:?}, so it authorizes no external effects",
                    state.project, state.project_lifecycle
                ),
            ));
        }

        // The facts behind the digest the control record names. Read here,
        // inside the lock, so the grants weighed below are exactly the ones
        // the project's committed state says are in force.
        let security = stores
            .security_states
            .get(&state.project_security_state)?
            .ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "the project's control record names security state {} but nothing \
                         stores it, so no authority can be resolved",
                        state.project_security_state
                    ),
                )
            })?;

        // (5) The binding, which must still select the frozen route.
        stores
            .bindings
            .with_locked_record(request.route.binding(), |binding| {
                let bound = ProviderBindingStore::require_selects(binding, request.route)?;
                let decision = decide(stores, &security, request)?;
                let authority = DispatchAuthority {
                    route: request.route.clone(),
                    attempt_authority: decision.considered.clone(),
                    authority_decision: decision,
                    project_control_generation: ProjectControlGeneration::new(state.generation),
                    project_security_state: state.project_security_state.clone(),
                    policy_digest: state.current_policy_digest.clone(),
                    registry_revisions: registry_revisions.clone(),
                    binding_generation: ProviderBindingGeneration::new(bound.generation),
                    lease: lease_ref.clone(),
                };
                body(&authority)
            })
    });

    // The registry must be exactly what the decision was taken against. A
    // difference here means something moved inside the fence, which the fence
    // exists to prevent — so it is a bypass, not concurrency.
    fence.require_unchanged(&registry_revisions)?;
    outcome
}

/// Decide the publish claim against the project's committed security facts.
///
/// Only grants the project's own state lists as active are weighed. A grant
/// sitting in the store that the state does not name has not been adopted by
/// this project, and treating it as authority would mean a grant could confer
/// permission merely by existing on disk.
fn decide(
    stores: &AuthorityStores,
    security: &crate::project::security::ProjectSecurityState,
    request: &AuthorityRequest<'_>,
) -> DraftResult<AuthorityDecision> {
    let mut inputs = AuthorityInputs::default();
    for reference in &security.active_authority_grants {
        let Some(logical) = reference.logical_id.as_ref() else {
            continue;
        };
        let id = draft_dcg_contract::ids::AuthorityGrantId::parse(logical.as_str())
            .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?;
        let Some(grant) = stores.grants.get(&id)? else {
            // The state names a grant nothing stores. Refusing to weigh it is
            // the safe half: it can only ever have permitted something.
            continue;
        };
        // The reference carries a digest so that resolving it proves you got
        // the fact it names. A substituted grant would otherwise inherit every
        // authorization the original earned.
        if grant.reference()? != *reference {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "grant '{}' does not match the reference the project's security state \
                     holds for it",
                    id.as_str()
                ),
            ));
        }
        if let Some(revocation) = stores.revocations.get(&id)? {
            inputs.revocations.push(revocation);
        }
        inputs.grants.push(grant);
    }

    let claim = AuthorityClaim::publish(request.actor.clone(), request.subject.clone())?;
    let decision = evaluate(&claim, &inputs, request.now, request.evaluator.clone())?;
    if !decision.is_permitted() {
        // Refused under the current-security guard, which is the whole point
        // of evaluating here rather than earlier: a grant that was valid when
        // the caller read it is not a permission to act now.
        crate::support::telemetry::Counter::PublicationAuthorityLinearizationRefusals.increment();
    }
    Ok(decision)
}

/// Releases the publication lease however Phase 1 ends.
///
/// A lease held past a panic would block the project until it expired, so the
/// release is a destructor rather than a line somebody has to reach.
struct LeaseGuard<'a> {
    leases: &'a LeaseStore,
    lease: crate::execution::lease::FencedLease,
}

impl Drop for LeaseGuard<'_> {
    fn drop(&mut self) {
        // Best effort: the lease is fenced and time-bounded, so failing to
        // release it delays the next publisher rather than losing anything.
        let _ = self.leases.release(&self.lease);
    }
}

/// The subject a project's publish authority is granted over.
///
/// The project, not the individual Publication. A Publication's id is derived
/// from what it delivers, so granting per-Publication would mean re-granting
/// for every send — and nobody would read what they were approving. The
/// meaningful unit is "this actor may publish from this project".
pub fn publish_subject(project: &draft_dcg_contract::ids::ProjectId) -> DraftResult<ScopedId> {
    ScopedId::parse(project.as_str())
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}
