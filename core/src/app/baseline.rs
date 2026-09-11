//! The application-level accepted-state path.
//!
//! One method, used by every operation that advances what the project accepts:
//! initialization accepts the first Baseline, promotion accepts every later
//! one. Both go through here so the evidence behind an accepted state is
//! always the evidence from the observation that produced it.
//!
//! # Why the binding is ensured here
//!
//! An observation names the binding that made it. A project whose binding is
//! missing could not produce a valid observation at all, so the binding is
//! established on the way in rather than assumed — which also makes
//! initialization and every later acceptance take the identical path, instead
//! of initialization being a special case that sets something up.

use draft_dcg_contract::baseline::BaselineId;
use draft_dcg_contract::ids::ActorId;
use draft_dcg_contract::value::Timestamp;

use crate::dcg::accept::{self, Accepted};
use crate::dcg::baseline::{accepted_state_root, current_baseline, BaselineOrigin, BaselineStore};
use crate::dcg::filesystem_provider;
use crate::dcg::observation_set::ObservationStore;
use crate::dcg::observe::ObservingBinding;
use crate::project::layout::DraftLayout;
use crate::project::provider::ProviderBindingStore;
use crate::project::provider_definition::ProviderDefinitionStore;
use crate::project::Workspace;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// The stores an acceptance writes through.
pub struct AcceptanceStores {
    pub observations: ObservationStore,
    pub baselines: BaselineStore,
    pub bindings: ProviderBindingStore,
    pub definitions: ProviderDefinitionStore,
}

impl AcceptanceStores {
    pub fn for_layout(layout: &DraftLayout) -> Self {
        Self {
            observations: ObservationStore::new(layout.observations_dir()),
            baselines: BaselineStore::new(layout.baselines_dir()),
            bindings: ProviderBindingStore::new(layout.provider_bindings_dir()),
            definitions: ProviderDefinitionStore::new(layout.provider_definitions_dir()),
        }
    }
}

/// Ensure Draft's own filesystem binding exists, and return it.
///
/// Idempotent. A project always has somewhere to observe from, so this is a
/// precondition rather than a step somebody can forget.
pub fn ensure_filesystem_binding(
    stores: &AcceptanceStores,
    project: &draft_dcg_contract::ids::ProjectId,
) -> DraftResult<crate::project::provider::ProviderBinding> {
    if let Some(existing) = stores
        .bindings
        .read_unlocked(&filesystem_provider::filesystem_binding_id())?
    {
        return Ok(existing);
    }
    let binding = filesystem_provider::bind(project.clone(), &stores.definitions)?;
    stores.bindings.bind(&binding)?;
    Ok(binding)
}

/// Accept the project's current observed state as a Baseline.
///
/// `parent` is `None` only for the first acceptance; every later one names
/// what it succeeded, which is what makes lineage walkable.
pub fn accept_observed_state(
    workspace: &Workspace,
    snapshot: &crate::dcg::state::Snapshot,
    observation_context: &str,
    origin: BaselineOrigin,
    actor: ActorId,
    parent: Option<BaselineId>,
) -> DraftResult<Accepted> {
    let stores = AcceptanceStores::for_layout(&workspace.layout);
    let binding = ensure_filesystem_binding(&stores, &workspace.workspace_id)?;

    let observer = ObservingBinding {
        binding: binding.id.clone(),
        semantic_definition: binding.current_semantic_definition.clone(),
        producer: draft_dcg_contract::producer::ProducerIdentity::new(
            draft_dcg_contract::identifier::NamespacedId::parse("draft.core/filesystem-observer")
                .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
            crate::DRAFT_VERSION,
        )
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
        observation_context: draft_dcg_contract::Digest::of_bytes(observation_context.as_bytes()),
    };

    let observed_at = Timestamp::from_unix_nanos(
        snapshot
            .created_at
            .timestamp_nanos_opt()
            .unwrap_or_default(),
    );
    let enumeration = accept::canonicalize(snapshot, observed_at, observed_at)?;

    accept::accept(
        workspace.workspace_id.clone(),
        &observer,
        &enumeration,
        &stores.observations,
        &stores.baselines,
        origin,
        actor,
        observed_at,
        parent,
    )
}

/// Make an accepted Baseline the project's current accepted state.
///
/// The control record is the authority on what a project accepts, so a
/// Baseline is not accepted until this commits — writing the Baseline alone
/// would leave a valid, verifiable historical node that nothing points at.
pub fn record_accepted(layout: &DraftLayout, accepted: &Accepted) -> DraftResult<()> {
    let control = crate::project::control::ProjectControlStore::new(layout.project_control_dir());
    let project = accepted.manifest.project.clone();

    match control.read_unlocked()? {
        None => {
            // The state behind the digest, written before the control record
            // names it. A control record pointing at bytes nobody stored would
            // make every authority evaluation refuse for want of facts it
            // could not read — which looks exactly like a denied permission
            // and is nothing of the sort.
            let security = crate::project::security::ProjectSecurityStateStore::for_layout(layout)
                .put(&crate::project::security::ProjectSecurityState::default())?;
            control.initialize(&crate::project::control::ProjectControlState {
                generation: 0,
                project,
                accepted_baseline: accepted.record.baseline_id.clone(),
                current_policy_digest: draft_dcg_contract::security::PolicyDigest::new(
                    draft_dcg_contract::Digest::of_bytes(b"draft.core/default-policy"),
                ),
                project_security_state: security,
                project_lifecycle: crate::project::control::ProjectLifecycle::Active,
            })
        }
        Some(current) => {
            let advanced = current.advanced(|next| {
                next.accepted_baseline = accepted.record.baseline_id.clone();
            });
            control.with_locked_control(|guard| {
                let expected = crate::support::record_guard::ExpectedRecordState::of(&current)?;
                guard.compare_exchange_locked(&expected, &advanced)
            })
        }
    }
}

/// The acting actor's canonical id.
///
/// Parsed rather than assumed well-formed: an actor id that does not fit the
/// contract cannot be recorded as having accepted anything.
pub fn actor_id_of(layout: &DraftLayout) -> DraftResult<ActorId> {
    let actor = crate::trust::identity::local::resolve_actor(&layout.draft_dir)?;
    ActorId::parse(actor.id.to_string()).map_err(|error| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!("the local actor id is not a canonical actor id: {error}"),
        )
    })
}

/// Observe the project now and accept what is seen as its next Baseline.
///
/// The acceptance a promotion performs. The parent is whatever the project
/// currently accepts, so lineage is continuous without the caller tracking it.
pub fn accept_current(
    app: &crate::app::App,
    workspace: &Workspace,
    origin: BaselineOrigin,
) -> DraftResult<Accepted> {
    let parent = current_baseline(&workspace.layout)?;
    let (snapshot, context) = app.observe_for_acceptance(workspace)?;
    let accepted = accept_observed_state(
        workspace,
        &snapshot,
        &context,
        origin,
        actor_id_of(&workspace.layout)?,
        parent,
    )?;
    record_accepted(&workspace.layout, &accepted)?;
    Ok(accepted)
}

/// Refuse an action when the project has Draft-visible edits.
///
/// Compares the state root the project would observe *now* against the one its
/// accepted Baseline holds. A digest over file bytes would answer a similar
/// question less precisely: the state root is what the project actually
/// accepted, so this compares like with like.
pub fn require_workspace_matches_baseline(
    app: &crate::app::App,
    workspace: &Workspace,
    action: &str,
    suggestion: &str,
) -> DraftResult<()> {
    let Some(accepted) = accepted_state_root(&workspace.layout)? else {
        return Ok(());
    };
    let (snapshot, _) = app.observe_for_acceptance(workspace)?;
    let observed_at = Timestamp::from_unix_nanos(
        snapshot
            .created_at
            .timestamp_nanos_opt()
            .unwrap_or_default(),
    );
    let enumeration = accept::canonicalize(&snapshot, observed_at, observed_at)?;

    let stores = AcceptanceStores::for_layout(&workspace.layout);
    let binding = ensure_filesystem_binding(&stores, &workspace.workspace_id)?;
    let mut set = crate::dcg::observation_set::AuthoritativeObservations::new();
    for observed in &enumeration.resources {
        set.insert(draft_dcg_contract::observation::Observation {
            id: draft_dcg_contract::ids::ObservationId::parse("obs_comparisononly000001")
                .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
            resource: observed.resource.clone(),
            state: observed.state.clone(),
            provider_binding: binding.id.clone(),
            provider_semantic_definition: binding.current_semantic_definition.clone(),
            stability: observed.stability,
            observation_context: draft_dcg_contract::Digest::of_bytes(b"comparison"),
            run: draft_dcg_contract::observation::ObservationRunRef {
                id: draft_dcg_contract::ids::ObservationRunId::parse("run_comparisononly000001")
                    .map_err(|error| {
                        DraftError::new(DraftErrorKind::CorruptData, error.to_string())
                    })?,
                digest: draft_dcg_contract::observation::ObservationRunDigest::new(
                    draft_dcg_contract::Digest::of_bytes(b"comparison"),
                ),
            },
            execution: None,
            observed_at,
        })?;
    }
    let (current, _) = set.build_roots()?;
    if current == accepted {
        return Ok(());
    }
    Err(DraftError::new(
        DraftErrorKind::DirtyWorkspace,
        format!("workspace has Draft-visible edits that are not part of the {action} Baseline"),
    )
    .with_context(format!(
        "the accepted Baseline holds state root {accepted}, the workspace observes {current}"
    ))
    .with_suggestion(suggestion))
}

/// The promotion that accepts a Change's work onto a parent Baseline.
///
/// Derived from the Change and the Baseline it advances, so a retried promotion
/// converges on the same promotion rather than minting a second one for the
/// same acceptance.
pub fn promotion_id_for(
    change_id: &str,
    parent: &BaselineId,
) -> DraftResult<draft_dcg_contract::ids::PromotionId> {
    let seed = draft_dcg_contract::Digest::of_bytes(format!("{change_id}|{parent}").as_bytes());
    draft_dcg_contract::ids::PromotionId::parse(format!("pro_{}", short_hex(&seed)))
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

/// The revision identity of the Change being promoted.
pub fn change_revision_id_for(
    change_id: &str,
) -> DraftResult<draft_dcg_contract::ids::ChangeRevisionId> {
    let seed = draft_dcg_contract::Digest::of_bytes(change_id.as_bytes());
    draft_dcg_contract::ids::ChangeRevisionId::parse(format!("rev_{}", short_hex(&seed)))
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

/// The hex body of a digest, without its algorithm prefix.
fn short_hex(digest: &draft_dcg_contract::Digest) -> String {
    digest
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect()
}
