//! Attaching a project to an external system, and moving that attachment.
//!
//! # Three facts, not one
//!
//! ```text
//! ProviderSemanticDefinition   what a provider's namespace and endpoints mean
//! ProviderOperationalProfile   how it is driven, and what it can do
//! ProviderBinding              which of those this project currently points at
//! ```
//!
//! The first two are immutable, content-addressed and retained forever: a
//! Baseline's provenance names the definition an observation was made under,
//! and a definition that could be edited would silently reinterpret every
//! accepted state that cited it. The third is a revisioned pointer, and moving
//! it is the only thing any command here does to an existing attachment.
//!
//! # Why `unbind` deletes nothing
//!
//! Withdrawing a binding stops new observations, routing and delivery through
//! it. It does not touch history: verification, historical reads, GC
//! reachability and explicit recovery all keep working, and `rebind` is the
//! explicit reactivation. A provider you stop using is not a provider that
//! never established anything.
//!
//! # Why every mutation goes through the audited path
//!
//! A binding move changes where new work is sent. Writing it directly would
//! leave a crash between the write and the Activity append undecidable, and a
//! reader of history unable to tell a retarget that happened from one that did
//! not. Each mutation here is a journalled transaction whose audit fact is
//! durable before the record moves.

use draft_dcg_contract::ids::{ProjectId, ProviderBindingId};
use draft_dcg_contract::semantics::ResourceStateSemanticsContract;
use draft_dcg_contract::{ProviderKindId, ProviderSemanticDefinitionDigest};
use serde::Serialize;

use crate::activity::EventKind;
use crate::app::activity::{AuditedStores, DomainAuditFact, ProjectActivity};
use crate::dcg::semantics_registry::SemanticsContractRegistry;
use crate::project::provider::{ProviderBinding, ProviderBindingLifecycle, ProviderBindingStore};
use crate::project::provider_definition::{
    ProviderDefinitionStore, ProviderOperationalProfile, ProviderSemanticDefinition,
};
use crate::project::Workspace;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::mutation_journal::{AuditFactEnvelope, MutationJournalStore};
use crate::support::record_guard::ExpectedRecordState;

/// One binding, with the immutable facts it currently points at.
#[derive(Debug, Clone, Serialize)]
pub struct BindingView {
    pub binding: ProviderBinding,
    /// Whether new work may be routed through it at all.
    ///
    /// Separate from whether a particular plan against it is still valid: a
    /// binding can be perfectly usable while a route planned earlier is stale.
    pub routable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_definition: Option<ProviderSemanticDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operational_profile: Option<ProviderOperationalProfile>,
}

/// The stores a provider command reads and writes.
pub struct ProviderStores {
    pub bindings: ProviderBindingStore,
    pub definitions: ProviderDefinitionStore,
    pub semantics: SemanticsContractRegistry,
    pub journals: MutationJournalStore,
}

impl ProviderStores {
    pub fn for_workspace(workspace: &Workspace) -> Self {
        let layout = &workspace.layout;
        Self {
            bindings: ProviderBindingStore::new(layout.provider_bindings_dir()),
            definitions: ProviderDefinitionStore::new(layout.provider_definitions_dir()),
            semantics: SemanticsContractRegistry::new(layout.semantics_contracts_dir()),
            journals: MutationJournalStore::new(layout.journals_dir()),
        }
    }
}

/// Everything a reader needs to audit this project's provider attachment.
///
/// Definitions and profiles are listed beside the bindings rather than only
/// through them: an immutable definition a binding has since moved off is
/// still the definition an accepted Baseline was composed under, and a view
/// that showed only current bindings would hide it.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderCatalogView {
    pub bindings: Vec<BindingView>,
    /// Immutable, content-addressed, and never edited in place.
    pub definitions: Vec<ProviderSemanticDefinition>,
    pub profiles: Vec<ProviderOperationalProfile>,
}

/// Every binding, definition and profile this project holds.
pub fn catalog(workspace: &Workspace) -> DraftResult<ProviderCatalogView> {
    let stores = ProviderStores::for_workspace(workspace);
    Ok(ProviderCatalogView {
        bindings: list(workspace)?,
        definitions: stores.definitions.list_definitions()?,
        profiles: stores.definitions.list_profiles()?,
    })
}

/// Every binding this project has, withdrawn ones included.
pub fn list(workspace: &Workspace) -> DraftResult<Vec<BindingView>> {
    let stores = ProviderStores::for_workspace(workspace);
    stores
        .bindings
        .list()?
        .into_iter()
        .map(|binding| view(&stores, binding))
        .collect()
}

/// One binding, with the immutable facts it points at.
pub fn show(workspace: &Workspace, id: &ProviderBindingId) -> DraftResult<BindingView> {
    let stores = ProviderStores::for_workspace(workspace);
    let binding = stores.bindings.read_unlocked(id)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("provider binding '{id}' does not exist"),
        )
    })?;
    view(&stores, binding)
}

fn view(stores: &ProviderStores, binding: ProviderBinding) -> DraftResult<BindingView> {
    Ok(BindingView {
        routable: binding.is_routable(),
        semantic_definition: stores
            .definitions
            .definition(&binding.current_semantic_definition)?,
        operational_profile: stores
            .definitions
            .profile(&binding.current_operational_profile)?,
        binding,
    })
}

/// Attach this project to a provider.
///
/// The semantics contract is registered first and the definition validated
/// against it: a definition that disagreed with its own contract would
/// interpret every observation it produced under a rule the contract does not
/// state. One identifier means exactly one contract, so a changed contract
/// under an accepted identifier is refused rather than adopted.
pub fn bind(
    workspace: &Workspace,
    name: &str,
    contract: &ResourceStateSemanticsContract,
    definition: &ProviderSemanticDefinition,
    profile: &ProviderOperationalProfile,
) -> DraftResult<ProviderBinding> {
    if name.trim().is_empty() {
        return Err(DraftError::new(
            DraftErrorKind::Validation,
            "a binding needs a name: two attachments of the same kind are legitimate, and \
             without a name they would share one identity",
        ));
    }
    let stores = ProviderStores::for_workspace(workspace);
    stores.semantics.register(contract)?;
    definition.validate_against(contract)?;

    let semantic_definition = stores.definitions.add_definition(definition)?;
    let operational_profile = stores.definitions.add_profile(profile)?;
    let id = binding_id(&workspace.workspace_id, &definition.kind, name)?;

    let binding = ProviderBinding {
        generation: 0,
        id: id.clone(),
        project: workspace.workspace_id.clone(),
        kind: definition.kind.clone(),
        current_semantic_definition: semantic_definition,
        current_operational_profile: operational_profile,
        lifecycle: ProviderBindingLifecycle::Active,
    };
    commit(
        workspace,
        &stores,
        &ExpectedRecordState::Absent,
        &binding,
        EventKind::ProviderBindingAdded,
        serde_json::json!({
            "kind": binding.kind.to_string(),
            "name": name,
            "semantic_definition": binding.current_semantic_definition.digest().to_string(),
            "operational_profile": binding.current_operational_profile.digest().to_string(),
        }),
    )?;
    Ok(binding)
}

/// Point a binding at a different semantic definition.
pub fn redefine(
    workspace: &Workspace,
    id: &ProviderBindingId,
    contract: &ResourceStateSemanticsContract,
    definition: &ProviderSemanticDefinition,
) -> DraftResult<ProviderBinding> {
    let stores = ProviderStores::for_workspace(workspace);
    stores.semantics.register(contract)?;
    definition.validate_against(contract)?;
    let digest = stores.definitions.add_definition(definition)?;
    retarget(
        workspace,
        &stores,
        id,
        EventKind::ProviderBindingRetargeted,
        |next| next.current_semantic_definition = digest.clone(),
        serde_json::json!({ "semantic_definition": digest.digest().to_string() }),
    )
}

/// Point a binding at a different operational profile.
pub fn reprofile(
    workspace: &Workspace,
    id: &ProviderBindingId,
    profile: &ProviderOperationalProfile,
) -> DraftResult<ProviderBinding> {
    let stores = ProviderStores::for_workspace(workspace);
    let digest = stores.definitions.add_profile(profile)?;
    retarget(
        workspace,
        &stores,
        id,
        EventKind::ProviderBindingRetargeted,
        |next| next.current_operational_profile = digest.clone(),
        serde_json::json!({ "operational_profile": digest.digest().to_string() }),
    )
}

/// Withdraw a binding from new work. Deletes nothing.
pub fn unbind(workspace: &Workspace, id: &ProviderBindingId) -> DraftResult<ProviderBinding> {
    let stores = ProviderStores::for_workspace(workspace);
    retarget(
        workspace,
        &stores,
        id,
        EventKind::ProviderBindingUnbound,
        |next| next.lifecycle = ProviderBindingLifecycle::Unbound,
        serde_json::json!({ "lifecycle": "unbound" }),
    )
}

/// Reactivate a withdrawn binding.
pub fn rebind(workspace: &Workspace, id: &ProviderBindingId) -> DraftResult<ProviderBinding> {
    let stores = ProviderStores::for_workspace(workspace);
    retarget(
        workspace,
        &stores,
        id,
        EventKind::ProviderBindingRebound,
        |next| next.lifecycle = ProviderBindingLifecycle::Active,
        serde_json::json!({ "lifecycle": "active" }),
    )
}

fn retarget(
    workspace: &Workspace,
    stores: &ProviderStores,
    id: &ProviderBindingId,
    kind: EventKind,
    mutate: impl FnOnce(&mut ProviderBinding),
    metadata: serde_json::Value,
) -> DraftResult<ProviderBinding> {
    // Read to build the replacement, then re-checked inside the record's
    // critical section: the expected state travels with the transaction, so a
    // caller that read before the lock was free cannot commit over a value
    // that has since moved.
    let current = stores.bindings.read_unlocked(id)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("provider binding '{id}' does not exist"),
        )
    })?;
    let expected = ExpectedRecordState::of(&current)?;
    let next = current.advanced(mutate);
    if next == current.advanced(|_| {}) {
        return Err(DraftError::new(
            DraftErrorKind::Validation,
            format!("provider binding '{id}' already holds that value"),
        ));
    }
    commit(workspace, stores, &expected, &next, kind, metadata)?;
    Ok(next)
}

fn commit(
    workspace: &Workspace,
    stores: &ProviderStores,
    expected: &ExpectedRecordState,
    replacement: &ProviderBinding,
    kind: EventKind,
    metadata: serde_json::Value,
) -> DraftResult<()> {
    let activity = ProjectActivity::new(workspace.layout.clone(), &workspace.workspace_id);
    let actor = crate::trust::identity::resolve_actor(&workspace.layout.draft_dir)?;
    let fact = DomainAuditFact::new(
        kind,
        actor.id.to_string(),
        // Frozen with the transaction: the drain is idempotent on the payload,
        // so a recovery replaying it has to produce byte-identical bytes, and
        // reading the clock again would not.
        draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
    )
    .about(replacement.id.to_string())
    .with(crate::support::redaction::redact_value(metadata));

    let payload = crate::app::activity::payload_of(&fact);
    let transaction_id = format!("provider-{}-{}", replacement.id, replacement.generation);
    let event_id = crate::activity::log::event_id_for(&format!(
        "{transaction_id}|{}",
        crate::support::hashing::canonical_json(&payload)
    ));

    crate::app::activity::commit_audited_mutation(
        AuditedStores {
            records: stores.bindings.records(),
            journals: &stores.journals,
            ledger: activity.log(),
        },
        replacement.id.as_str(),
        &transaction_id,
        expected,
        replacement,
        AuditFactEnvelope {
            activity_event_id: event_id,
            payload,
        },
    )?;
    Ok(())
}

/// The id a named attachment of one kind gets.
///
/// Derived rather than minted, so re-running `bind` with the same name and
/// kind converges on the attachment already made instead of creating a second
/// one that means the same thing. The name is what lets a project hold two
/// attachments of the same kind — Scenario E — without them colliding.
fn binding_id(
    project: &ProjectId,
    kind: &ProviderKindId,
    name: &str,
) -> DraftResult<ProviderBindingId> {
    let digest =
        draft_dcg_contract::Digest::of_bytes(format!("{project}|{kind}|{name}").as_bytes());
    let hex: String = digest
        .to_string()
        .trim_start_matches("sha256:")
        .chars()
        .take(12)
        .collect();
    ProviderBindingId::parse(format!("pbd_{hex}"))
        .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))
}

/// A binding's current semantic definition digest, for callers that only need
/// to know where it points.
pub fn current_definition(
    workspace: &Workspace,
    id: &ProviderBindingId,
) -> DraftResult<ProviderSemanticDefinitionDigest> {
    Ok(show(workspace, id)?.binding.current_semantic_definition)
}
