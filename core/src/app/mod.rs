pub mod adoption;
pub mod authority;
pub mod authorization;
pub mod baseline;
pub mod baseline_detail;
pub mod composition;
pub mod doctor;
pub mod gc;
pub mod impact;
pub mod maintenance;
pub mod observation;
pub mod pack_detail;
pub mod promotion;
pub mod provider;
pub mod publish;
pub mod representation;
pub mod resource;
pub mod roots;
pub mod security;
pub mod startup;
pub mod task;
pub mod verify;
pub mod workflow;

pub mod activity;

use draft_dcg_contract::ids::ProjectId;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::activity::EventKind;
use crate::app::activity::ProjectActivity;
use crate::dcg::change_set::{ChangeSet, ChangeSetId};
use crate::dcg::observation::ObservationContext;
use crate::dcg::resource::{RawResourceState, ResourceId, ResourceLocator};
use crate::dcg::revision_pack::ReviewProgressState;
use crate::dcg::snapshot::{
    pattern_match, read_ignore_lines, relative_path as rel_path, walk_dir, IgnoreMatcher,
    Snapshotter,
};
use crate::dcg::state::{Snapshot, WorkspaceStatus};
use crate::evidence::representation::{ResourceInterference, RevisionPackRepresentationBundle};
use crate::evidence::risk::RiskConfig;
use crate::evidence::verification::VerificationConfig;
use crate::execution::records::HookResult;
use crate::execution::workspace::{ChangePackWorkspace, Evidence};
use crate::project::config::{DraftConfig, HookEntry, ResolvedConfig};
use crate::project::object_store::{
    read_object_segment_index, write_object_segment_index, ObjectSegment, ObjectSegmentEntry,
    ObjectStore,
};
use crate::project::{DraftLayout, Workspace, WorkspaceMetadata};
use crate::read_model::activity::{ActivityEntry, ActivityReplay};
use crate::recovery::rollback::{RollbackPlan, RollbackRecord};
use crate::support::actor::{ActorKind, ActorRef};
use crate::support::common::{
    now, ActorId, EvidenceId, ReceiptId, RollbackPlanId, SnapshotId, TaskId, WorkspacePath,
};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::{
    ensure_dir, list_with_extension, read_toml, write_atomic, write_json, write_toml,
};
use crate::support::hashing::{blake3_hex, hex_encode, sha256_hex, try_canonical_hash};
use crate::support::process_lock::ProcessFileLock;
use crate::support::redaction::redact as redact_secrets;
use crate::trust::identity::resolve_actor;

const DRAFT_DIR: &str = ".draft";
use crate::contracts::{current_version, ContractId};

/// Draft's orchestration entry point.
///
/// The contribution source is how installed extensions reach Draft's domain
/// logic. It defaults to contributing nothing, which is Core-only mode: Draft
/// builds and runs as a generic platform with no extensions present, reporting
/// missing domain knowledge rather than guessing at it.
#[derive(Clone)]
pub struct App {
    contributions: std::sync::Arc<dyn crate::extension::ExtensionContributionSource>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("App")
            .field(
                "contributions",
                &self.contributions.active_contributions().is_empty(),
            )
            .finish()
    }
}

/// Report from `draft init --global`.
#[derive(Debug, Clone, Serialize)]
pub struct InitGlobalReport {
    pub root: String,
    pub created: bool,
    pub hidden: bool,
    pub actor_id: String,
    pub public_key_id: String,
}

/// A single named health check inside a doctor report.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub detail: String,
}

impl DoctorCheck {
    fn ok(name: &str, detail: impl Into<String>) -> Self {
        DoctorCheck {
            name: name.to_string(),
            ok: true,
            category: None,
            detail: detail.into(),
        }
    }
    fn fail(name: &str, detail: impl Into<String>) -> Self {
        DoctorCheck {
            name: name.to_string(),
            ok: false,
            category: Some(DraftErrorKind::CorruptData.code().to_string()),
            detail: detail.into(),
        }
    }

    fn fail_error(name: &str, error: DraftError) -> Self {
        DoctorCheck {
            name: name.to_string(),
            ok: false,
            category: Some(error.code().to_string()),
            detail: error.message,
        }
    }
}

/// Validation of one `.draft/` store (global or project).
#[derive(Debug, Clone, Serialize)]
pub struct DoctorScope {
    pub label: String,
    pub root: String,
    pub exists: bool,
    pub hidden: bool,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorScope {
    /// True if the store exists and every check passed.
    pub fn healthy(&self) -> bool {
        self.exists && self.checks.iter().all(|c| c.ok)
    }
}

/// Full `draft doctor` report across both stores.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub global: DoctorScope,
    pub project: Option<DoctorScope>,
}

/// Report from `draft pack inspect <cpk_id>`.
#[derive(Debug, Clone, Serialize)]
pub struct ChangePackInspectReport {
    pub manifest: crate::dcg::change_pack_store::ChangePackManifest,
    pub lifecycle: ReviewProgressState,
    pub valid_actions: Vec<String>,
    /// The authoritative identity of the transition this change carries.
    pub change_set_digest: String,
    pub base_snapshot_digest: String,
    pub result_snapshot_digest: String,
    /// The observation semantics both states were observed under.
    pub observation_context_digest: String,
    /// The exact historical observations this change relied on, base and result
    /// kept apart.
    ///
    /// Absent for a change whose sides were never observed — the empty base — and
    /// never resolved by picking the newest record for a state, which would let
    /// a later look silently change what this change says it used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_observation: Option<crate::dcg::observation::SnapshotObservationRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_observation: Option<crate::dcg::observation::SnapshotObservationRef>,
    /// Resources that actually changed, with proof.
    pub resources_changed: usize,
    /// What Draft could **not** determine — reported separately from changes,
    /// because uncertainty is not a change and must never be counted as one.
    pub derivation_gaps: Vec<crate::dcg::change_set::ChangeDerivationGap>,
    /// Contributed elements the change touches, when extraction found any.
    pub elements_touched: Vec<String>,
    /// Diagnostic digests of the derived layers, when they exist. Never part of
    /// this change's identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation_bundle_digest: Option<String>,
    pub receipts: Vec<String>,
    /// The five-state verification result, not a boolean.
    pub verification_state: String,
    pub verified: bool,
    pub content_revision_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangePackReopenReport {
    pub change_pack_id: String,
    pub progress: ReviewProgressState,
    pub content_revision_id: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusComponent {
    Repo,
    Tasks,
    Candidates,
    ChangePacks,
    Hooks,
}

impl StatusComponent {
    pub fn parse(value: &str) -> DraftResult<Self> {
        match value {
            "repo" => Ok(StatusComponent::Repo),
            "tasks" => Ok(StatusComponent::Tasks),
            "candidates" => Ok(StatusComponent::Candidates),
            "changes" => Ok(StatusComponent::ChangePacks),
            "hooks" => Ok(StatusComponent::Hooks),
            other => Err(
                DraftError::invalid_config(format!("unknown status component '{other}'"))
                    .with_suggestion("use one of: repo, tasks, candidates, changes, hooks"),
            ),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            StatusComponent::Repo => "repo",
            StatusComponent::Tasks => "tasks",
            StatusComponent::Candidates => "candidates",
            StatusComponent::ChangePacks => "changes",
            StatusComponent::Hooks => "hooks",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StatusOptions {
    pub change_pack_id: Option<String>,
    pub component: Option<StatusComponent>,
    pub full: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    pub workspace: WorkspaceStatus,
    pub component: Option<String>,
    pub change_pack_id: Option<String>,
    pub full: bool,
    pub sections: BTreeMap<String, Value>,
}

/// Report from `draft pack depends <cpk_id>`.
#[derive(Debug, Clone, Serialize)]
pub struct ChangePackDependsReport {
    pub change_pack_id: String,
    pub base_snapshot_digest: String,
    pub changed_resources: Vec<String>,
    /// Other changes touching the same contributed elements → the shared element
    /// ids.
    ///
    /// Empty with no extraction capability installed, which is not the same as
    /// "no relationship": Draft simply has no way to see one, and says so
    /// through the reported capability gap rather than through an empty map.
    pub shared_element_changes: std::collections::BTreeMap<String, Vec<String>>,
    pub declared_dependencies: Vec<String>,
}

/// A single detected conflict between two changes.
#[derive(Debug, Clone, Serialize)]
pub struct ConflictFinding {
    pub kind: String,
    pub detail: String,
    pub blocking: bool,
}

/// Report from `draft pack conflicts <a> <b>`.
#[derive(Debug, Clone, Serialize)]
pub struct ChangePackConflictsReport {
    pub change_a: String,
    pub change_b: String,
    pub conflicts: Vec<ConflictFinding>,
    pub blocking: bool,
}

/// Report from `draft pack compose <a> <b> --name <name>`.
#[derive(Debug, Clone, Serialize)]
pub struct ChangePackComposeReport {
    pub change_pack_id: String,
    pub name: String,
    pub dependencies: Vec<String>,
    pub requires_reverification: bool,
    pub composition_hash: String,
}

/// One observation, and the record of which observation it was.
///
/// The two travel together because they are only unambiguously paired at the
/// moment of observation. Anything that must later say "this ChangePack relied on
/// *that* observation" needs both halves, and resolving the second from the
/// store afterwards would mean choosing between records.
#[derive(Debug, Clone)]
pub struct ObservedSnapshot {
    pub snapshot: Snapshot,
    pub observation: crate::dcg::observation::SnapshotObservationRef,
}

/// Report from `draft pack evidence run cpk_<id>`.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    pub change_pack_id: String,
    /// The aggregate, as one of the five states. Never a boolean: "nothing was
    /// checked" and "everything passed" are different answers.
    pub state: String,
    /// Every check that was selected, whether or not it could run.
    pub check_results: Vec<crate::evidence::verification::VerificationCheckResult>,
    pub selection_reason: String,
    pub elements_touched: usize,
    pub result_hash: String,
    /// The classification the checks were selected against.
    pub classification_digest: String,
    /// Domain knowledge Draft did not have for some of the changed resources.
    ///
    /// Advisory: verification still ran, still honoured the project's own
    /// `verify.toml`, and still recorded evidence. Empty when nothing was
    /// uninterpretable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capability_gaps: Vec<crate::extension::CapabilityGap>,
    /// Capabilities an installed extension declares but is not authorized to
    /// use.
    ///
    /// Reported separately from a gap because the knowledge is present and
    /// only the permission is missing — the fix is to authorize, not to
    /// install. Without this an unauthorized extension would look exactly like
    /// no extension at all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub withheld_capabilities: Vec<crate::extension::WithheldCapability>,
}

/// The decision recorded for a check the project configured itself.
///
/// A project's own `verify.toml` is its own authority — there is no extension
/// artifact to authorize — but the evidence still records *something*, so no
/// result in the record is missing its permission story.
const PROJECT_CONFIGURED_DECISION: &str = "project-configured";

/// Result of a `--dry-run` for promotion or recovery: what would happen and why.
#[derive(Debug, Clone, Serialize)]
pub struct DryRunReport {
    pub action: String,
    pub target: String,
    pub would_proceed: bool,
    pub resulting_state: String,
    /// The locator bodies this action would touch. Opaque: shown, never parsed.
    pub affected_resources: Vec<String>,
    pub checks: Vec<DoctorCheck>,
}

/// One resource in the project, as the resource browser sees it.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceEntry {
    /// The opaque locator body. Draft never parses it; a `file`-scheme adapter
    /// happens to make it look like a path.
    pub locator: ResourceLocator,
    pub resource_id: ResourceId,
    /// The intrinsic shape of the resource, when its adapter states one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<draft_extension_contract::ResourceForm>,
    pub protected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_size: Option<u64>,
    /// **Every** class an installed extension assigns, sorted.
    ///
    /// A list rather than one value, because a resource genuinely is several
    /// things at once — a text document *and* a language source — and picking
    /// one would discard a correct classification. Empty when nothing is
    /// installed to recognize it, which is not an error: Draft does not need to
    /// know what a resource is to manage it.
    #[serde(default)]
    pub classes: Vec<String>,
    /// Classes installed extensions define incompatibly. Scoped to those
    /// classes: every other assignment on this resource still stands.
    #[serde(default)]
    pub class_collisions: Vec<String>,
}

/// What classes a project's resources carry, and which are disputed.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ClassificationReport {
    /// Every class assigned to at least one resource, sorted.
    pub assigned: Vec<String>,
    /// Resources whose classification installed extensions disagree about.
    pub collisions: Vec<String>,
    /// The digest of the bundle these came from, so a caller can tell whether
    /// two reports describe the same derivation.
    pub classification_digest: String,
    /// Set when nothing is installed to classify anything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<crate::extension::CapabilityGap>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceContentView {
    pub locator: ResourceLocator,
    pub content: String,
    pub protected: bool,
    pub workspace_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceSaveReport {
    pub locator: ResourceLocator,
    pub change_pack_id: String,
    pub backup_path: Option<String>,
    pub workspace_hash: String,
    pub protected: bool,
}

/// A task scoped to part of a resource.
///
/// The region is expressed in a contributed coordinate space, so a task may
/// name lines of a document, keys of a record set, or anything else a domain
/// defines — Core stores the coordinates without interpreting them.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceSelectionTaskReport {
    pub task_id: String,
    pub locator: ResourceLocator,
    pub coordinate_space: String,
    pub start: u64,
    pub length: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceMutationReport {
    pub locator: ResourceLocator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_locator: Option<ResourceLocator>,
    pub backup_path: Option<String>,
    pub workspace_hash: String,
    pub action: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceSearchHit {
    pub locator: ResourceLocator,
    pub line: u32,
    pub preview: String,
}

/// How one resource changed against a change's base, as far as Draft can explain.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceComparisonReport {
    pub locator: ResourceLocator,
    pub base_state_digest: Option<String>,
    pub result_state_digest: Option<String>,
    /// The derived explanation, when a comparison capability produced one.
    ///
    /// `None` is a real answer, not a failure: without an installed comparison
    /// Draft knows *that* the resource changed and says so, rather than
    /// inventing a rendering it cannot justify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<crate::evidence::representation::RevisionPackRepresentation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<crate::extension::CapabilityGap>,
    pub workspace_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceWorkspaceReport {
    pub mode: String,
    pub workspace_hash: String,
    pub pending_edits: usize,
    pub resources: usize,
    pub status: String,
}

impl DoctorReport {
    /// True if every present scope is healthy.
    pub fn healthy(&self) -> bool {
        self.global.healthy() && self.project.as_ref().map(|p| p.healthy()).unwrap_or(true)
    }
}

/// Write a minimal canonical manifest for the implicit base change (empty change).
fn write_base_canonical_manifest(
    root: &Path,
    policy: &crate::dcg::source_view::CanonicalSourcePolicy,
    change_pack_id: &draft_dcg_contract::ids::ChangePackId,
    name: &str,
    change_set: &ChangeSet,
) -> DraftResult<()> {
    use crate::dcg::change_pack_store::{
        ChangePackContentRevisionRecord, ChangePackContentStore, ChangePackManifest,
    };
    let workspace_hash = crate::dcg::source_view::workspace_hash(root, policy)?;
    let patch_bytes = to_pretty(change_set)?;
    // The revision binds the change set by its own canonical identity, so a
    // re-serialization cannot change what the revision points at.
    let change_set_digest = change_set.change_set_digest.clone();
    let mut manifest = ChangePackManifest {
        schema_version: current_version(ContractId::ChangePackManifest),
        change_pack_id: change_pack_id.to_string(),
        manifest_digest: String::new(),
        name: name.to_string(),
        description: "base change".to_string(),
        intent: crate::dcg::change_pack_store::unspecified_intent(),
        provenance: serde_json::json!({"origin": "local"}),
        author_id: "actor_local".to_string(),
        candidate_id: None,
        declared_dependencies: Vec::new(),
        created_at: now().to_rfc3339(),
    };
    let store = ChangePackContentStore::new(crate::project::layout::DraftLayout::for_root(root));
    manifest.refresh_manifest_digest();
    store.write_manifest(&manifest)?;
    let mut revision = ChangePackContentRevisionRecord {
        schema_version: current_version(ContractId::ChangePackContentRevisionRecord),
        change_pack_id: manifest.change_pack_id.clone(),
        manifest_digest: manifest.manifest_digest.clone(),
        content_revision_id: "content_initial".into(),
        content_revision_number: 1,
        content_revision_digest: String::new(),
        base_digest: workspace_hash.clone(),
        content_digest: workspace_hash.clone(),
        change_set_digest,
        target_digest: workspace_hash.clone(),
        resolved_dependency_digests: Vec::new(),
        created_at: now().to_rfc3339(),
    };
    revision.refresh_content_revision_digest();
    store.write_revision(&revision)?;
    write_atomic(
        &crate::project::layout::DraftLayout::for_root(root)
            .change_pack_changes(change_pack_id.as_str()),
        &patch_bytes,
    )?;
    store.write_review_progress(&crate::dcg::revision_pack::ReviewProgressRecord {
        schema_version: current_version(ContractId::ReviewProgressState),
        change_pack_id: manifest.change_pack_id.clone(),
        content_revision_id: revision.content_revision_id.clone(),
        content_revision_digest: revision.content_revision_digest.clone(),
        progress: crate::dcg::revision_pack::ReviewProgressState::Draft,
        updated_at: now(),
        last_operation_id: crate::support::common::OperationId::new("op_init"),
    })?;
    // An empty lockfile, so conflict and dependency queries have a resource set
    // to read rather than a missing file to interpret.
    let lock = empty_lockfile(change_pack_id.as_str(), change_set);
    store.write_lockfile(&lock)
}

/// A lockfile for a change that touches nothing.
///
/// The base change is a real change with a real (empty) change set, so its lock
/// records the same authoritative digests and Core revisions as any other —
/// there is no second, weaker shape for the empty case.
fn empty_lockfile(
    change_pack_id: &str,
    change_set: &ChangeSet,
) -> crate::dcg::change_pack_store::ChangePackLockfile {
    crate::dcg::change_pack_store::ChangePackLockfile {
        schema_version: current_version(ContractId::ChangePackLock),
        change_pack_id: change_pack_id.to_string(),
        base_snapshot_digest: change_set.base_snapshot_digest.clone(),
        result_snapshot_digest: change_set.result_snapshot_digest.clone(),
        // The base change's empty change set has no observation behind either
        // side. Recording `None` is the honest answer; inventing a reference
        // would name a record that does not exist.
        base_observation: None,
        result_observation: None,
        observation_context_digest: change_set.observation_context_digest.clone(),
        change_set_digest: change_set.change_set_digest.clone(),
        resource_state_digests: std::collections::BTreeMap::new(),
        policy_version: crate::DRAFT_VERSION.to_string(),
        change_derivation_revision: crate::dcg::change_set::CHANGE_DERIVATION_REVISION,
        classification_aggregator_revision:
            crate::evidence::classification::CLASSIFICATION_AGGREGATOR_REVISION,
        verification_aggregator_revision:
            crate::evidence::verification::VERIFICATION_AGGREGATOR_REVISION,
        risk_aggregator_revision: crate::evidence::risk::RISK_AGGREGATOR_REVISION,
        impact_merge_revision: crate::dcg::impact::IMPACT_MERGE_REVISION,
        verification_commands: Vec::new(),
        dependency_change_pack_ids: Vec::new(),
        receipt_digests: Vec::new(),
    }
}

/// Serialize a value to pretty JSON bytes for archive members.
fn to_pretty<T: Serialize>(value: &T) -> DraftResult<Vec<u8>> {
    serde_json::to_vec_pretty(value)
        .map_err(|e| DraftError::storage(format!("serialize failed: {e}")))
}

fn bool_check(
    name: &str,
    cond: bool,
    ok_detail: impl Into<String>,
    fail_detail: impl Into<String>,
) -> DoctorCheck {
    if cond {
        DoctorCheck::ok(name, ok_detail)
    } else {
        DoctorCheck::fail(name, fail_detail)
    }
}

#[cfg(unix)]
fn key_perms_check(key: &Path) -> DoctorCheck {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(key) {
        Ok(meta) => {
            let mode = meta.permissions().mode() & 0o777;
            if mode == 0o600 {
                DoctorCheck::ok("key-perms", "signing key is 0600")
            } else {
                DoctorCheck::fail(
                    "key-perms",
                    format!("signing key mode is {mode:o}, expected 600"),
                )
            }
        }
        Err(_) => DoctorCheck::fail("key-perms", "signing key not readable"),
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            contributions: std::sync::Arc::new(crate::extension::NoExtensions),
        }
    }

    /// Build an app that reads contributed domain knowledge from `source`.
    pub fn with_extension_contributions(
        source: std::sync::Arc<dyn crate::extension::ExtensionContributionSource>,
    ) -> Self {
        Self {
            contributions: source,
        }
    }

    /// What installed, enabled and authorized extensions currently contribute.
    pub fn active_contributions(&self) -> crate::extension::ActiveContributions {
        self.contributions.active_contributions()
    }

    /// The protections in force for this project.
    ///
    /// Core contributes only its own control plane; credentials, key material
    /// and registry tokens are domain judgement and arrive from project config
    /// or from an installed `control_policy`.
    pub fn protections(
        &self,
        root: &Path,
    ) -> DraftResult<Vec<crate::project::protected::ProtectionRule>> {
        crate::project::protected::rules_for_project(
            root,
            &contributed_protections(&self.active_contributions()),
        )
    }

    /// The canonical view this project observes through.
    ///
    /// The same contributed exclusions the observation scanner applies, so the
    /// workspace digest and the authoritative snapshot can never disagree about
    /// what the project contains — which would otherwise make an excluded
    /// resource's change look like a reason to re-verify.
    fn view_policy(&self) -> crate::dcg::source_view::CanonicalSourcePolicy {
        crate::dcg::source_view::CanonicalSourcePolicy {
            exclusions: contributed_view_rules(&self.active_contributions()),
            ..Default::default()
        }
    }

    fn workspace_hash(&self, root: &Path) -> DraftResult<String> {
        crate::dcg::source_view::workspace_hash(root, &self.view_policy())
    }

    /// Take one authoritative observation of the project.
    ///
    /// The single entry point on purpose, and the place the *pinning* rule is
    /// enforced: every snapshot is taken under the semantics this project
    /// **adopted**, never under whatever happens to be installed at this
    /// instant. Installing a package that would change the observed universe
    /// records a candidate and changes nothing until somebody adopts it.
    pub(crate) fn observe(&self, ws: &Workspace) -> DraftResult<Snapshot> {
        self.observe_recorded(ws).map(|observed| observed.snapshot)
    }

    /// Observe the project together with the semantics it was observed under.
    ///
    /// The context digest travels with the snapshot because an observation's
    /// provenance includes what it was interpreted under: the same bytes read
    /// under different semantics are different evidence.
    pub(crate) fn observe_for_acceptance(&self, ws: &Workspace) -> DraftResult<(Snapshot, String)> {
        let active = self.ensure_active_context(ws)?;
        let snapshot = self.observe(ws)?;
        Ok((snapshot, active.context.context_digest))
    }

    /// Observe, and keep hold of exactly which observation this was.
    ///
    /// Anything that must later say "this ChangePack relied on *that* observation"
    /// takes this form, because the provenance record is only unambiguous at
    /// the moment it is written.
    fn observe_recorded(&self, ws: &Workspace) -> DraftResult<ObservedSnapshot> {
        let active = self.ensure_active_context(ws)?;
        // Look at what is installed now, and record a candidate if it differs.
        // Detection is a side note; it never alters this observation.
        self.refresh_pending_context(ws, &active)?;

        let (snapshot, provenance_digest) = Snapshotter::new(ws, active.view_rules.clone())?
            .create_snapshot(
                resolve_actor(&ws.layout.draft_dir)?,
                &active.context.context_digest,
            )?;
        let observation = crate::dcg::observation::SnapshotObservationRef::new(
            snapshot.snapshot_digest.clone(),
            provenance_digest,
        );
        Ok(ObservedSnapshot {
            snapshot,
            observation,
        })
    }

    /// The semantics in force, adopting the effective ones if none are yet.
    ///
    /// A project's first context is not a transition: there is no prior
    /// universe for it to differ from, so nothing is superseded and nobody is
    /// asked to approve a change that is not a change.
    fn ensure_active_context(
        &self,
        ws: &Workspace,
    ) -> DraftResult<crate::dcg::observation_lifecycle::ActiveObservationContext> {
        use crate::dcg::observation_lifecycle::ActiveObservationContext;
        if let Some(active) = crate::dcg::observation_store::active(ws)? {
            return Ok(active);
        }
        let contributions = self.active_contributions();
        let context = effective_observation_context(ws, &contributions);
        let view_rules = contributed_view_rules(&contributions);
        let (snapshot, _) = Snapshotter::new(ws, view_rules.clone())?.create_snapshot(
            resolve_actor(&ws.layout.draft_dir)?,
            &context.context_digest,
        )?;
        let active = ActiveObservationContext {
            schema_version: current_version(ContractId::ActiveObservationContext),
            context,
            view_rules,
            baseline_snapshot_digest: snapshot.snapshot_digest.clone(),
            baseline_snapshot_id: snapshot.id.clone(),
            adopted_at: now(),
            transition_id: None,
        };
        crate::dcg::observation_store::write_active(ws, &active)?;
        Ok(active)
    }

    /// Note whether the installed extensions would observe differently.
    ///
    /// Writes a candidate, or clears a stale one. Never touches the active
    /// context: a project keeps observing under the semantics it adopted until
    /// somebody adopts different ones.
    fn refresh_pending_context(
        &self,
        ws: &Workspace,
        active: &crate::dcg::observation_lifecycle::ActiveObservationContext,
    ) -> DraftResult<Option<crate::dcg::observation_lifecycle::PendingObservationContext>> {
        use crate::dcg::observation_lifecycle::{diff_contexts, PendingObservationContext};
        let contributions = self.active_contributions();
        let effective = effective_observation_context(ws, &contributions);
        let reasons = diff_contexts(&active.context, &effective);
        if reasons.is_empty() {
            // A package update that observes identically, or one that was
            // disabled again. Either way there is nothing to decide, and a
            // banner that outlived its cause teaches people to ignore banners.
            crate::dcg::observation_store::clear_pending(ws)?;
            return Ok(None);
        }
        let pending = PendingObservationContext {
            schema_version: current_version(ContractId::PendingObservationContext),
            candidate: effective,
            candidate_view_rules: contributed_view_rules(&contributions),
            active_context_digest: active.context.context_digest.clone(),
            reasons,
            detected_at: now(),
        };
        crate::dcg::observation_store::write_pending(ws, &pending)?;
        Ok(Some(pending))
    }

    pub fn init(&self, root: &Path) -> DraftResult<InitReport> {
        self.init_with_base(root, "base")
    }

    pub fn init_with_base(&self, root: &Path, base_change_name: &str) -> DraftResult<InitReport> {
        let layout = DraftLayout::for_root(root);
        crate::trust::identity::reject_retired_profile_state(Some(&layout.draft_dir))?;
        let global_home = crate::project::home::DraftGlobalStore::locate()?;
        crate::trust::identity::global::reject_retired_actor_profile(&global_home)?;
        crate::project::config::reject_retired_profile_config(&layout.config_toml())?;
        crate::project::config::reject_retired_profile_config(&global_home.config_toml())?;
        if layout.draft_dir.exists() {
            return Err(DraftError::invalid_config(
                "Draft workspace state already exists; initialization never repairs or overwrites it",
            ));
        }
        self.init_global()?;
        let created = true;
        let hidden_status = layout.create_all()?;
        if let crate::support::hidden::HiddenStatus::Failed(reason) = &hidden_status {
            eprintln!(
                "warning: could not hide {}: {reason}",
                layout.draft_dir.display()
            );
        }
        if !layout.config_toml().exists() {
            write_toml(&layout.config_toml(), &DraftConfig::default())?;
        }
        if !layout.ignore_file().exists() {
            write_atomic(&layout.ignore_file(), DEFAULT_IGNORE.as_bytes())?;
        }
        if !layout.verify_toml().exists() {
            write_toml(&layout.verify_toml(), &VerificationConfig::default())?;
        }
        if !layout.risk_toml().exists() {
            write_toml(&layout.risk_toml(), &RiskConfig::default())?;
        }
        if !layout.policy_toml().exists() {
            write_toml(
                &layout.policy_toml(),
                &crate::project::policy::Policy::safe_default(),
            )?;
        }
        rebuild_index_for_layout(&layout)?;
        let meta = WorkspaceMetadata {
            schema_version: current_version(ContractId::WorkspaceMetadata),
            workspace_id: crate::project::mint_project_id(),
            draft_version: crate::DRAFT_VERSION.to_string(),
            created_at: now(),
        };
        write_json(&layout.project_json(), &meta)?;
        let store = ProjectActivity::new(layout.clone(), &meta.workspace_id);
        if created {
            store.append(
                EventKind::ProjectCreated,
                None,
                serde_json::json!({ "root": root.display().to_string() }),
            )?;
            // The base change records what `init` actually observed, not a
            // fabricated empty state. Initializing a directory that already has
            // content *has* observed that content, and pretending otherwise
            // would make every later comparison start from a baseline whose
            // coverage cannot prove anything — turning ordinary additions into
            // presence-uncertain gaps.
            // Constructed directly rather than through `open`, which requires
            // the stable head that initialization has not written yet.
            let workspace = Workspace {
                workspace_id: meta.workspace_id.clone(),
                root: root.to_path_buf(),
                layout: layout.clone(),
            };
            let observed = self.observe(&workspace)?;
            let mut base = ChangePackWorkspace::new(
                meta.workspace_id.clone(),
                None,
                None,
                observed.id.clone(),
                observed.id.clone(),
                Some(base_change_name.to_string()),
            );
            let change_dir = layout.change_pack_workspace_dir(&base.id);
            ensure_dir(&change_dir)?;
            // No transition: the base change is the starting point, so its change
            // set is empty between one observation and itself.
            let patch = empty_change_set_between(&base, &observed, &observed)?;
            let evidence = Evidence {
                schema_version: current_version(ContractId::RevisionPackEvidence),
                id: EvidenceId::generate(),
                change_pack_id: base.id.clone(),
                command_logs: Vec::new(),
                resources_touched: Vec::new(),
                representation_bundle_digest: None,
                check_results: Vec::new(),
                risk_summary_ref: None,
                agent_plan_ref: None,
                agent_transcript_ref: None,
                warnings: Vec::new(),
                created_at: now(),
            };
            base.change_set_refs.push(patch.id.to_string());
            base.evidence_refs.push(evidence.id.to_string());
            base.manifest_hash = hash_json(&base)?;
            write_json(&change_dir.join("staging.json"), &base)?;
            write_json(&change_dir.join("changes.json"), &patch)?;
            write_json(&change_dir.join("evidence.json"), &evidence)?;
            // Also write the immutable manifest and revision for the empty base
            // change. No trust receipt is minted for this implicit initial state.
            write_base_canonical_manifest(
                root,
                &self.view_policy(),
                &base.id,
                base_change_name,
                &patch,
            )?;
            write_atomic(
                layout.selected_change_pack_file().as_path(),
                base.id.to_string().as_bytes(),
            )?;
            store.append(
                EventKind::ChangePackCreated,
                Some(base.id.to_string()),
                serde_json::to_value(&base).expect("Draft-owned records must serialize"),
            )?;
        }
        // The project's first Baseline: what `init` actually observed, accepted
        // through the same path every later acceptance takes. There is no
        // separate initialization shape — initialization is simply the
        // acceptance with no parent.
        let (accepted_baseline, project_state_root) = {
            let workspace = Workspace {
                workspace_id: meta.workspace_id.clone(),
                root: root.to_path_buf(),
                layout: layout.clone(),
            };
            let stores = crate::app::baseline::AcceptanceStores::for_layout(&layout);
            match crate::dcg::baseline::current_baseline(&layout)? {
                Some(existing) => {
                    let manifest = stores.baselines.manifest(&existing)?.ok_or_else(|| {
                        DraftError::new(
                            DraftErrorKind::CorruptData,
                            "the project accepts a Baseline whose manifest is missing",
                        )
                    })?;
                    let root = manifest.project_state_root.clone();
                    (existing, root)
                }
                None => {
                    let active = self.ensure_active_context(&workspace)?;
                    let observed = self.observe(&workspace)?;
                    let accepted = crate::app::baseline::accept_observed_state(
                        &workspace,
                        &observed,
                        &active.context.context_digest,
                        crate::dcg::baseline::BaselineOrigin::Initial,
                        crate::app::baseline::actor_id_of(&layout)?,
                        None,
                    )?;
                    crate::app::baseline::record_accepted(&layout, &accepted)?;
                    let root = accepted.manifest.project_state_root.clone();
                    (accepted.record.baseline_id, root)
                }
            }
        };
        crate::project::registry::ProjectRegistry::global()?.upsert(
            meta.workspace_id.as_str(),
            root,
            Some(accepted_baseline.to_string()),
            // Derived here, above both: `app` can see the graph, and the
            // registry must not have to.
            crate::dcg::source_view::WorkspaceRevision::derive(root)
                .ok()
                .map(|revision| revision.content_digest),
        )?;
        Ok(InitReport {
            workspace_id: meta.workspace_id.to_string(),
            root: root.display().to_string(),
            created,
            draft_dir: layout.draft_dir.display().to_string(),
            baseline_id: accepted_baseline.to_string(),
            project_state_root: project_state_root.to_string(),
            next_actions: vec![
                "draft task wizard".to_string(),
                "draft task list".to_string(),
                "draft console tui".to_string(),
            ],
            candidate_guidance:
                "No command candidates are configured yet; use human/manual tasks or add [candidates.<name>] in .draft/config.toml."
                    .to_string(),
        })
    }

    pub fn open(&self, cwd: &Path) -> DraftResult<Workspace> {
        self.open_workspace(cwd, false)
    }

    fn open_workspace(
        &self,
        cwd: &Path,
        allow_retired_profile_recovery: bool,
    ) -> DraftResult<Workspace> {
        let root = find_workspace_root(cwd).ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::WorkspaceNotFound,
                "not inside a Draft workspace",
            )
            .with_suggestion("run `draft init`")
        })?;
        let layout = DraftLayout::for_root(&root);
        if !allow_retired_profile_recovery {
            crate::trust::identity::reject_retired_profile_state(Some(&layout.draft_dir))?;
            let home = crate::project::home::DraftGlobalStore::locate()?;
            crate::trust::identity::global::reject_retired_actor_profile(&home)?;
            crate::project::config::reject_retired_profile_config(&layout.config_toml())?;
            crate::project::config::reject_retired_profile_config(&home.config_toml())?;
        }
        let metadata_bytes = fs::read(layout.project_json()).map_err(|error| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!("workspace metadata is unreadable: {error}"),
            )
        })?;
        let meta: WorkspaceMetadata = crate::contracts::decode_persisted(&metadata_bytes)?;
        let paths = crate::project::layout::DraftLayout::for_root(&root);
        if !allow_retired_profile_recovery {
            // The recovery barrier, before anything reads or writes the
            // project. A recovery-mode open skips it for the same reason it
            // skips the checks above: `draft doctor` has to be able to reach a
            // project the barrier will not clear.
            crate::app::startup::ensure_recovered(&paths, &meta.workspace_id.to_string())?;
        }
        // A project is readable when it accepts a Baseline. A repository
        // written by an earlier Draft has none, and is not migrated: its
        // accepted state was established under an ontology this version does
        // not implement, and synthesizing a Baseline from it would fabricate
        // provenance for observations that were never recorded.
        if crate::dcg::baseline::current_baseline(&paths)?.is_none() {
            return Err(DraftError::new(
                DraftErrorKind::UnsupportedSchema,
                "this project has no accepted Baseline",
            )
            .with_suggestion(
                "initialize a new project with `draft init`; repositories created by earlier \
                 Draft versions are not readable by this one and are not migrated",
            ));
        }
        Ok(Workspace {
            workspace_id: meta.workspace_id,
            root,
            layout,
        })
    }

    pub fn config_set(&self, cwd: &Path, key: &str, value: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let ws = self.open(cwd)?;
        let reported = if matches!(key, "user.name" | "user.email") {
            crate::project::config::validate_profile_value(key, value)?
        } else {
            value.to_string()
        };
        crate::project::config::set_value(&ws.layout.config_toml(), key, &reported)?;
        append_project_config_event(&ws, key, "set")?;
        Ok(ConfigReport::single(key, &reported))
    }

    pub fn config_get(&self, cwd: &Path, key: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let ws = self.open(cwd)?;
        let cfg = ResolvedConfig::load(&ws)?;
        Ok(ConfigReport::single(key, &cfg.get(key).unwrap_or_default()))
    }

    pub fn config_unset(&self, cwd: &Path, key: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let ws = self.open(cwd)?;
        crate::project::config::remove_table(&ws.layout.config_toml(), key)?;
        append_project_config_event(&ws, key, "unset")?;
        Ok(ConfigReport::single(key, ""))
    }

    pub fn config_list(&self, cwd: &Path) -> DraftResult<ConfigReport> {
        let ws = self.open(cwd)?;
        Ok(ConfigReport {
            entries: ResolvedConfig::load(&ws)?.entries(),
        })
    }

    // ---- global store, doctor, security actor, layered config -----------

    /// `draft init --global`: create the hidden global `~/.draft/` store,
    /// provision the actor identity + Ed25519 signing key, and seed the
    /// default policy. Idempotent.
    pub fn init_global(&self) -> DraftResult<InitGlobalReport> {
        let home = crate::project::home::DraftGlobalStore::locate()?;
        crate::trust::identity::reject_retired_profile_state(None)?;
        crate::trust::identity::global::reject_retired_actor_profile(&home)?;
        crate::project::config::reject_retired_profile_config(&home.config_toml())?;
        let created = !home.exists();
        let hidden = home.create_all()?;
        // Seed a default policy file if absent (safe default).
        if !home.default_policy_toml().exists() {
            write_toml(
                &home.default_policy_toml(),
                &crate::project::policy::Policy::safe_default(),
            )?;
        }
        if !home.config_toml().exists() {
            write_toml(&home.config_toml(), &DraftConfig::default())?;
        }
        let profile = crate::trust::identity::global::ensure_actor(&home)?;
        Ok(InitGlobalReport {
            root: home.root().display().to_string(),
            created,
            hidden: hidden.is_ok(),
            actor_id: profile.actor_id,
            public_key_id: profile.public_key_id,
        })
    }

    /// Read-only stable actor and signing-key state for diagnostics/Console.
    pub fn identity_status(&self) -> DraftResult<crate::trust::identity::IdentityStatus> {
        let home = crate::project::home::DraftGlobalStore::locate()?;
        crate::trust::identity::global::status(&home)
    }

    /// `draft doctor receipts rcp_<id>`: verify a single signed receipt.
    pub fn receipt_verify(
        &self,
        cwd: &Path,
        receipt_id: &str,
    ) -> DraftResult<crate::receipt::ReceiptVerification> {
        validate_receipt_id(receipt_id)?;
        let ws = self.open(cwd)?;
        crate::receipt::verify_one(&ws.layout, receipt_id)
    }

    /// `draft doctor receipts --all`: verify the event chain, transparency chain,
    /// and every receipt. Fails closed if anything does not verify.
    pub fn receipt_verify_all(
        &self,
        cwd: &Path,
    ) -> DraftResult<crate::read_model::LedgerVerification> {
        let ws = self.open(cwd)?;
        crate::read_model::integrity::verify_all(&ws.layout, &ws.workspace_id)
    }

    /// `draft config set --global <key> <value>`.
    pub fn config_set_global(&self, key: &str, value: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let home = crate::project::home::DraftGlobalStore::locate()?;
        crate::trust::identity::reject_retired_profile_state(None)?;
        crate::trust::identity::global::reject_retired_actor_profile(&home)?;
        crate::project::config::reject_retired_profile_config(&home.config_toml())?;
        home.create_all()?;
        let reported = if matches!(key, "user.name" | "user.email") {
            crate::project::config::validate_profile_value(key, value)?
        } else {
            value.to_string()
        };
        crate::project::config::set_value(&home.config_toml(), key, &reported)?;
        self.audit_global_config_change(&home, &[key], "set")?;
        Ok(ConfigReport::single(key, &reported))
    }

    pub fn config_get_global(&self, key: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let home = crate::project::home::DraftGlobalStore::locate()?;
        crate::trust::identity::reject_retired_profile_state(None)?;
        crate::trust::identity::global::reject_retired_actor_profile(&home)?;
        let value =
            crate::project::config::get_value(&home.config_toml(), key)?.unwrap_or_default();
        Ok(ConfigReport::single(key, &value))
    }

    pub fn config_unset_global(&self, key: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let home = crate::project::home::DraftGlobalStore::locate()?;
        crate::trust::identity::reject_retired_profile_state(None)?;
        crate::trust::identity::global::reject_retired_actor_profile(&home)?;
        crate::project::config::reject_retired_profile_config(&home.config_toml())?;
        home.create_all()?;
        crate::project::config::remove_table(&home.config_toml(), key)?;
        self.audit_global_config_change(&home, &[key], "unset")?;
        Ok(ConfigReport::single(key, ""))
    }

    fn audit_global_config_change(
        &self,
        home: &crate::project::home::DraftGlobalStore,
        keys: &[&str],
        operation: &str,
    ) -> DraftResult<()> {
        let actor = crate::trust::identity::global::ensure_actor(home)?;
        let bytes = std::fs::read(home.config_toml())?;
        crate::activity::GlobalAuditLog::global()?.append(
            crate::activity::GlobalAuditEvent::UserProfileUpdated,
            Some(actor.actor_id.clone()),
            Some("global-config".into()),
            None,
            serde_json::json!({
                "scope": "global",
                "changed_keys": keys,
                "operation": operation,
                "resulting_config_digest": crate::support::hashing::sha256_hex(&bytes),
            }),
        )?;
        Ok(())
    }

    /// Atomically update the global non-security user profile. An outer `None`
    /// means unchanged; an inner `None` unsets the selected key.
    pub fn config_update_user_global(
        &self,
        name: Option<Option<&str>>,
        email: Option<Option<&str>>,
    ) -> DraftResult<ConfigReport> {
        let home = crate::project::home::DraftGlobalStore::locate()?;
        crate::trust::identity::reject_retired_profile_state(None)?;
        crate::trust::identity::global::reject_retired_actor_profile(&home)?;
        crate::project::config::reject_retired_profile_config(&home.config_toml())?;
        home.create_all()?;
        let mut updates = Vec::new();
        if let Some(value) = name {
            updates.push((
                "user.name".to_string(),
                value
                    .map(|value| crate::project::config::validate_profile_value("user.name", value))
                    .transpose()?,
            ));
        }
        if let Some(value) = email {
            updates.push((
                "user.email".to_string(),
                value
                    .map(|value| {
                        crate::project::config::validate_profile_value("user.email", value)
                    })
                    .transpose()?,
            ));
        }
        if updates.is_empty() {
            return Err(DraftError::invalid_config(
                "profile update must include user.name or user.email",
            ));
        }
        crate::project::config::update_values(&home.config_toml(), &updates)?;
        let keys = updates
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>();
        self.audit_global_config_change(&home, &keys, "update")?;
        let resolver =
            crate::project::config::ConfigResolver::load(None, Some(home.config_toml().as_path()))?;
        Ok(ConfigReport {
            entries: ["user.name", "user.email"]
                .into_iter()
                .map(|key| (key.to_string(), resolver.get(key).unwrap_or_default()))
                .collect(),
        })
    }

    /// `draft config get <key>` with full precedence: CLI > project > global >
    /// built-in default. Works outside a workspace (project layer is skipped).
    pub fn config_get_layered(&self, cwd: &Path, key: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        let home = crate::project::home::DraftGlobalStore::locate()?;
        let project_cfg = match self.open(cwd) {
            Ok(ws) => Some(ws.layout.config_toml()),
            Err(error) if error.kind == DraftErrorKind::WorkspaceNotFound => None,
            Err(error) => return Err(error),
        };
        let global_cfg = home.config_toml();
        let resolver = crate::project::config::ConfigResolver::load(
            project_cfg.as_deref(),
            Some(global_cfg.as_path()),
        )?;
        Ok(ConfigReport::single(
            key,
            &resolver.get(key).unwrap_or_default(),
        ))
    }

    pub fn hook_list(&self, cwd: &Path) -> DraftResult<ConfigReport> {
        let report = self.config_list(cwd)?;
        Ok(ConfigReport {
            entries: report
                .entries
                .into_iter()
                .filter(|(k, _)| k.starts_with("hooks."))
                .collect(),
        })
    }

    pub fn hook_get(&self, cwd: &Path, key: &str) -> DraftResult<ConfigReport> {
        self.config_get(cwd, key)
    }

    pub fn hook_set(&self, cwd: &Path, key: &str, value: &str) -> DraftResult<ConfigReport> {
        let full_key = if key.starts_with("hooks.") {
            key.to_string()
        } else {
            format!("hooks.{key}")
        };
        self.config_set(cwd, &full_key, value)
    }

    pub fn hook_unset(&self, cwd: &Path, key: &str) -> DraftResult<ConfigReport> {
        let full_key = if key.starts_with("hooks.") {
            key.to_string()
        } else {
            format!("hooks.{key}")
        };
        self.config_unset(cwd, &full_key)
    }

    pub fn hook_run(&self, cwd: &Path, hook_name: &str) -> DraftResult<HookRunReport> {
        let ws = self.open(cwd)?;
        let cfg = ResolvedConfig::load(&ws)?;
        let hook = cfg.hook(hook_name).ok_or_else(|| {
            DraftError::not_found(format!("hook '{hook_name}' is not configured"))
        })?;
        let store = ObjectStore::new(ws.layout.clone());
        let ctx = HookContext {
            message: String::new(),
            title: String::new(),
            description: String::new(),
            task_id: String::new(),
            execution_id: String::new(),
            change_pack_id: String::new(),
            receipt_id: ReceiptId::generate().to_string(),
            actor_name: resolve_actor(&ws.layout.draft_dir)?.id.to_string(),
            timestamp: now().to_rfc3339(),
            verified: "false".to_string(),
            risk_level: "unknown".to_string(),
            files_changed: "0".to_string(),
            workspace_root: ws.root.display().to_string(),
            hook_name: hook_name.to_string(),
            hook_phase: hook.phase.clone(),
            vars: BTreeMap::new(),
        };
        let result = run_hook(&ws, &store, hook_name, &hook, &ctx)
            .map_err(|e| DraftError::new(DraftErrorKind::HookFailed, e.message))?;
        ws.events()?.append(
            EventKind::OperationExecuted,
            Some(hook_name.to_string()),
            serde_json::to_value(&result).expect("Draft-owned records must serialize"),
        )?;
        Ok(HookRunReport {
            hook_name: hook_name.to_string(),
            exit_code: result.exit_code,
            stdout_ref: result.stdout_ref,
            stderr_ref: result.stderr_ref,
        })
    }

    pub fn ignore_add(&self, cwd: &Path, pattern: &str) -> DraftResult<IgnoreReport> {
        let ws = self.open(cwd)?;
        let mut patterns = read_ignore_lines(&ws.layout.ignore_file())?;
        if !patterns.iter().any(|p| p == pattern) {
            patterns.push(pattern.to_string());
            write_atomic(&ws.layout.ignore_file(), patterns.join("\n").as_bytes())?;
            ws.events()?.append(
                EventKind::PolicyUpdated,
                None,
                serde_json::json!({ "action": "add", "pattern": pattern }),
            )?;
        }
        self.ignore_list(cwd)
    }

    pub fn ignore_remove(&self, cwd: &Path, pattern: &str) -> DraftResult<IgnoreReport> {
        let ws = self.open(cwd)?;
        let mut patterns = read_ignore_lines(&ws.layout.ignore_file())?;
        patterns.retain(|p| p != pattern);
        write_atomic(&ws.layout.ignore_file(), patterns.join("\n").as_bytes())?;
        ws.events()?.append(
            EventKind::PolicyUpdated,
            None,
            serde_json::json!({ "action": "remove", "pattern": pattern }),
        )?;
        self.ignore_list(cwd)
    }

    pub fn ignore_list(&self, cwd: &Path) -> DraftResult<IgnoreReport> {
        let ws = self.open(cwd)?;
        Ok(IgnoreReport {
            patterns: read_ignore_lines(&ws.layout.ignore_file())?,
        })
    }

    pub fn status(&self, cwd: &Path) -> DraftResult<WorkspaceStatus> {
        let ws = self.open(cwd)?;
        // Observed through the adapter port, then compared. Status is a
        // question about the same universe an authoritative observation would
        // establish, so it must be asked the same way.
        let observed = crate::dcg::source::ResourceSource::enumerate(
            &crate::dcg::filesystem_source::FilesystemSource::new(&ws),
            &crate::dcg::source::ViewRules {
                exclusions: contributed_view_rules(&self.active_contributions()),
            },
        )?;
        let status = crate::dcg::snapshot::workspace_status(&ws, &observed)?;
        ws.events()?.append(
            EventKind::ResourceObserved,
            None,
            serde_json::json!({
                "changes": status.changes.len(),
                "ignored_count": status.ignored_count
            }),
        )?;
        Ok(status)
    }

    pub fn status_with_options(
        &self,
        cwd: &Path,
        options: StatusOptions,
    ) -> DraftResult<StatusReport> {
        let workspace = self.status(cwd)?;
        let ws = self.open(cwd)?;
        let mut sections = BTreeMap::new();
        let include = |component: StatusComponent| {
            options.full || options.component.map(|c| c == component).unwrap_or(false)
        };
        if options.component.is_none() || include(StatusComponent::Repo) {
            sections.insert(
                "repo".to_string(),
                serde_json::json!({
                    "workspace_id": workspace.workspace_id.to_string(),
                    "root_path": workspace.root_path.clone(),
                    "scanned_at": workspace.scanned_at,
                    "ignored_count": workspace.ignored_count,
                    "has_draft_dir_violation": workspace.has_draft_dir_violation,
                }),
            );
        }
        if options.component.is_none() || include(StatusComponent::ChangePacks) {
            sections.insert(
                "changes".to_string(),
                serde_json::json!({
                    "count": workspace.changes.len(),
                    "items": if options.full { serde_json::to_value(&workspace.changes)? } else { Value::Null },
                }),
            );
        }
        if include(StatusComponent::Tasks) {
            let tasks = self.task_list(cwd)?;
            let mut health = BTreeMap::<String, usize>::new();
            for task in &tasks {
                let view = self.task_view(cwd, task.id.as_str())?;
                *health
                    .entry(format!("{:?}", view.health).to_lowercase())
                    .or_default() += 1;
            }
            sections.insert(
                "tasks".to_string(),
                serde_json::json!({
                    "count": tasks.len(),
                    "health": health,
                    "items": if options.full { serde_json::to_value(tasks)? } else { Value::Null },
                }),
            );
        }
        if include(StatusComponent::Candidates) {
            let profiles = self.candidate_profiles(cwd)?;
            let presets = self.candidate_presets(cwd)?;
            sections.insert(
                "candidates".to_string(),
                serde_json::json!({
                    "profiles": profiles.len(),
                    "presets": presets.len(),
                    "profile_names": profiles.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                    "preset_names": presets.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                    "items": if options.full { serde_json::to_value(profiles)? } else { Value::Null },
                }),
            );
        }
        if include(StatusComponent::Hooks) {
            let cfg = ResolvedConfig::load(&ws)?;
            sections.insert(
                "hooks".to_string(),
                serde_json::json!({
                    "verify": cfg.get("hooks.verify").unwrap_or_default(),
                    "items": if options.full { serde_json::to_value(cfg.entries())? } else { Value::Null },
                }),
            );
        }
        if let Some(change_ref) = &options.change_pack_id {
            // The ChangePack as the graph holds it, and what may legally follow
            // from its newest sealed revision. Readiness is not a separate
            // notion here: whether work can proceed is what the authorization
            // view answers, and it answers it with a reason.
            let change = self
                .dcg_change_packs(cwd)?
                .into_iter()
                .find(|view| view.change_pack.as_str() == change_ref)
                .ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::NotFound,
                        format!("ChangePack '{change_ref}' is not in this project's graph"),
                    )
                })?;
            let authorization = match change.revisions.first() {
                Some(revision) => {
                    Some(self.dcg_authorization(cwd, change_ref, revision.id.as_str())?)
                }
                None => None,
            };
            sections.insert(
                "change".to_string(),
                serde_json::json!({
                    "change_pack_id": change.change_pack,
                    "lifecycle": change.lifecycle,
                    "revisions": change.revisions.len(),
                    "authorization": authorization,
                    "details": if options.full { serde_json::to_value(&change)? } else { Value::Null },
                }),
            );
        }
        Ok(StatusReport {
            workspace,
            component: options.component.map(|c| c.as_str().to_string()),
            change_pack_id: options.change_pack_id,
            full: options.full,
            sections,
        })
    }

    pub fn checkpoint(&self, cwd: &Path, message: &str) -> DraftResult<CheckpointReport> {
        let ws = self.open(cwd)?;
        let snapshot = self.observe(&ws)?;
        // The canonical source view is computed before the checkpoint is
        // recorded, not for its digest but for what computing it refuses: a
        // symlink escaping the project, an absolute path, `.draft/` reached
        // through a view rule. A checkpoint is a recovery target, and one
        // recorded over a project Draft could not safely read is a promise it
        // cannot keep.
        let workspace_hash =
            crate::dcg::source_view::workspace_hash(&ws.root, &self.view_policy())?;
        // A checkpoint is something that happened, so it is an Activity
        // event. It used to mint a signed receipt as well, which gave a reader
        // two records of one act and no rule for which was authoritative — and
        // v1 receipts attest promotions and publications, not local actions.
        let event = ws.events()?.append(
            EventKind::CheckpointCreated,
            Some(snapshot.id.to_string()),
            serde_json::json!({
                "message": message,
                "recovery_target": snapshot.id.to_string(),
                "workspace_hash": workspace_hash,
            }),
        )?;
        Ok(CheckpointReport {
            snapshot_id: snapshot.id.to_string(),
            event_id: event,
            resources: snapshot.resources.len(),
        })
    }

    pub fn inbox(&self, cwd: &Path) -> DraftResult<Vec<crate::read_model::inbox::InboxItem>> {
        let ws = self.open(cwd)?;
        let mut by_id = BTreeMap::<String, crate::read_model::inbox::InboxItem>::new();

        // Every item is derived from the graph. There is deliberately no
        // second evidence-and-decision store to fold in beside it: two records
        // of one review state, with no rule for which is authoritative, is how
        // an inbox starts lying about what is outstanding.
        for view in self.dcg_change_packs(&ws.root)? {
            let Some(revision) = view.revisions.first() else {
                continue;
            };
            let authorization =
                self.dcg_authorization(&ws.root, view.change_pack.as_str(), revision.id.as_str())?;
            let attention = crate::read_model::inbox::RevisionAttention {
                change_pack_id: view.change_pack.as_str(),
                revision_pack_id: revision.id.as_str(),
                approved: authorization.approving_decision().is_some(),
                changes_requested: authorization.decisions.iter().any(|decision| {
                    matches!(
                        decision.outcome,
                        crate::dcg::decision::DecisionOutcome::ChangesRequested { .. }
                    )
                }),
                unsatisfied_conditions: authorization
                    .gates
                    .iter()
                    .find(|gate| !gate.satisfied)
                    .map(|gate| gate.unsatisfied.clone())
                    .unwrap_or_default(),
            };
            for item in crate::read_model::inbox::derive(&attention) {
                by_id.insert(item.id.clone(), item);
            }
        }

        for execution in crate::task::ExecutionStore::for_root(&ws.root).list_all()? {
            match execution.status {
                crate::task::ExecutionStatus::Failed => insert_inbox(
                    &mut by_id,
                    format!("inbox:execution_failed:{}", execution.id),
                    "execution_failed",
                    execution.id.to_string(),
                    "failed",
                    format!(
                        "execution {} failed for candidate {}",
                        execution.id, execution.candidate
                    ),
                    format!(
                        "draft task spawn {} --retry {}",
                        execution.task_id, execution.id
                    ),
                ),
                crate::task::ExecutionStatus::Cancelled
                | crate::task::ExecutionStatus::Interrupted => insert_inbox(
                    &mut by_id,
                    format!("inbox:execution_resumable:{}", execution.id),
                    "execution_resumable",
                    execution.id.to_string(),
                    "resumable",
                    format!("execution {} can be resumed or retried", execution.id),
                    format!(
                        "draft task spawn {} --resume {}",
                        execution.task_id, execution.id
                    ),
                ),
                _ => {}
            }
        }

        // A waiver that is about to lapse is the last moment somebody can
        // renew it deliberately rather than discover the gate closed.
        let renewal_cutoff = draft_dcg_contract::value::Timestamp::from_unix_nanos(
            (now() + chrono::Duration::hours(72))
                .timestamp_nanos_opt()
                .unwrap_or(i64::MAX),
        );
        for waiver in crate::app::authorization::AuthorizationStores::for_layout(&ws.layout)
            .waivers
            .list()?
        {
            if waiver.expires_at <= renewal_cutoff {
                insert_inbox(
                    &mut by_id,
                    format!("inbox:waiver_renewal:{}", waiver.id),
                    "waiver_renewal",
                    waiver.revision_pack.to_string(),
                    "expires_soon",
                    format!("waiver {} expires soon", waiver.id),
                    format!(
                        "draft pack gates waive {} {} --reason <reason> --expires 7d",
                        waiver.revision_pack, waiver.condition
                    ),
                );
            }
        }

        for entry in crate::execution::operation::RecoveryStore::for_root(&ws.root).recoverable()? {
            insert_inbox(
                &mut by_id,
                format!("inbox:doctor_warning:{}", entry.recovery_id),
                "doctor_warning",
                entry
                    .subject_id
                    .clone()
                    .unwrap_or_else(|| entry.recovery_id.to_string()),
                "needs_recovery",
                format!("operation {} needs recovery", entry.operation),
                "draft doctor".to_string(),
            );
        }

        let pending_editor_dir = crate::project::layout::DraftLayout::for_root(&ws.root)
            .workspaces_dir()
            .join("pending");
        if pending_editor_dir.exists() {
            let pending = list_with_extension(&pending_editor_dir, "json")?;
            for path in pending {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("pending")
                    .to_string();
                insert_inbox(
                    &mut by_id,
                    format!("inbox:editor_pending:{id}"),
                    "editor_pending",
                    id.clone(),
                    "pending_edits",
                    format!("editor edits are pending in {id}"),
                    "draft console tui".to_string(),
                );
            }
        }

        Ok(by_id.into_values().collect())
    }

    /// What classes the project's resources carry.
    ///
    /// With nothing installed this reports no classes and one capability gap —
    /// which is the honest answer, and distinguishable from "classified, and
    /// nothing matched".
    pub fn classification_report(&self, cwd: &Path) -> DraftResult<ClassificationReport> {
        let ws = self.open(cwd)?;
        let snapshot = self.observe(&ws)?;
        let contributions = self.active_contributions();
        let classification =
            crate::evidence::classification::classify_snapshot(&snapshot, &contributions);

        let mut assigned: Vec<String> = classification
            .assignments
            .iter()
            .map(|assignment| assignment.class_id.qualified())
            .collect();
        assigned.sort();
        assigned.dedup();
        let mut collisions: Vec<String> = classification
            .collisions
            .iter()
            .map(|collision| collision.resource_id.as_str().to_string())
            .collect();
        collisions.sort();
        collisions.dedup();

        let gaps = if contributions.classifications.is_empty() {
            vec![crate::extension::CapabilityGap::new(
                crate::extension::ExtensionCapabilityKind::Classification,
                Vec::new(),
                "no installed extension classifies resources in this project",
            )]
        } else {
            Vec::new()
        };
        Ok(ClassificationReport {
            assigned,
            collisions,
            classification_digest: classification.classification_bundle_digest,
            gaps,
        })
    }

    pub fn workspace_commit(
        &self,
        cwd: &Path,
        workspace_id: &str,
        operation_id: crate::support::common::OperationId,
    ) -> DraftResult<crate::execution::workspace::WorkspaceCommitResult> {
        let ws = self.open(cwd)?;
        let store = crate::execution::workspace::WorkspaceStore::for_workspace(
            &ws.root,
            self.protections(&ws.root)?,
        );
        store.commit_with_validation(workspace_id, operation_id, |session, _revision| {
            if let crate::execution::workspace::EditAttribution::ChangePack { id }
            | crate::execution::workspace::EditAttribution::Review { id } = &session.attribution
            {
                // Nothing to invalidate: Evidence, Assessments, Decisions and
                // Gates each bind one exact RevisionPackId and never carry
                // to another, so a commit that moves the subject digest simply
                // leaves them describing the revision they were made about.
                let _ = id;
            }
            Ok(())
        })
    }

    /// Run one candidate command in an isolated copy of the workspace,
    /// enforce candidate limits and file guards, and turn accepted changes
    /// into a change against `baseline`. The working tree is restored to its
    /// pre-spawn contents before returning.
    fn run_candidate_execution(
        &self,
        ws: &Workspace,
        task: &crate::task::TaskDefinition,
        execution: &crate::task::Execution,
        profile: &crate::task::candidate::CandidateProfile,
        baseline: &Snapshot,
    ) -> DraftResult<Option<String>> {
        let exec_store = crate::task::ExecutionStore::for_root(&ws.root);
        let paths = crate::project::layout::DraftLayout::for_root(&ws.root);
        let exe_id = execution.id.as_str();
        let runtime_dir = paths.execution_runtime_dir(exe_id);
        ensure_dir(&runtime_dir)?;

        // The deterministic task contract handed to the candidate.
        write_json(
            &paths.execution_contract_file(exe_id),
            &serde_json::json!({
                "schema_version": current_version(ContractId::Execution),
                "execution_id": exe_id,
                "task_id": task.id.to_string(),
                "goal": task.goal,
                "allowed_zones": task.allowed_zones,
                "forbidden_zones": task.forbidden_zones,
                "success_criteria": task.success_criteria,
                "base_baseline": task.base_baseline,
                "required_evidence": task.required_evidence,
                "protected_resources": self.protections(&ws.root)?,
                "output": {
                    "mode": "edit_in_place",
                    "workspace": "current directory",
                    "note": "edit files in the working directory; Draft collects the diff"
                },
            }),
        )?;

        // Isolated workspace copy (default) or in-place run.
        let isolated =
            profile.limits.isolation_mode == crate::task::candidate::IsolationMode::Isolated;
        let work_dir = if isolated {
            let dir = paths.execution_work_dir(exe_id);
            ensure_dir(&dir)?;
            // Only filesystem-addressed resources can be copied into an
            // isolated run directory. Another scheme's resources stay where they
            // are; the candidate reaches them through their adapter or not at
            // all.
            for entry in baseline
                .resources
                .iter()
                .filter(|state| state.locator.scheme == crate::extension::FILE_SCHEME)
            {
                let rel = WorkspacePath::new(&entry.locator.body);
                let src = safe_workspace_dest(&ws.root, &rel)?;
                let dst = dir.join(rel.as_str());
                if let Some(parent) = dst.parent() {
                    ensure_dir(parent)?;
                }
                if src.exists() && !src.is_dir() {
                    fs::copy(&src, &dst).map_err(|e| {
                        DraftError::storage(format!(
                            "failed to copy {} into execution workspace: {e}",
                            rel.as_str()
                        ))
                    })?;
                }
            }
            dir
        } else {
            ws.root.clone()
        };

        // Restricted environment: allowlist plus a minimal base.
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        for key in [
            "PATH",
            "HOME",
            "USERPROFILE",
            "TMPDIR",
            "TEMP",
            "SYSTEMROOT",
        ] {
            if let Ok(v) = std::env::var(key) {
                env.insert(key.to_string(), v);
            }
        }
        for key in &profile.limits.env_allowlist {
            if let Ok(v) = std::env::var(key) {
                env.insert(key.clone(), v);
            }
        }
        env.insert(
            "DRAFT_TASK_CONTRACT".to_string(),
            paths.execution_contract_file(exe_id).display().to_string(),
        );
        env.insert("DRAFT_EXECUTION_ID".to_string(), exe_id.to_string());

        let command = &execution.command;
        if command.is_empty() {
            return Err(DraftError::invalid_config("spawn command is empty"));
        }
        if !profile.limits.command_allowlist.is_empty()
            && !profile
                .limits
                .command_allowlist
                .iter()
                .any(|c| c == &command[0])
        {
            return Err(DraftError::new(
                DraftErrorKind::ExecutionLimitExceeded,
                format!(
                    "command '{}' is not in the allowlist for candidate '{}'",
                    command[0], profile.name
                ),
            ));
        }

        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .current_dir(&work_dir)
            .env_clear()
            .envs(&env)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                DraftError::new(
                    DraftErrorKind::CandidateNotConfigured,
                    format!("failed to start candidate '{}': {e}", profile.name),
                )
                .with_suggestion(format!(
                    "check that `{}` is installed and on PATH",
                    command[0]
                ))
            })?;
        exec_store.mark_running(exe_id, Some(child.id()))?;
        ws.events()?.append(
            EventKind::OperationPlanned,
            Some(exe_id.to_string()),
            serde_json::json!({
                "task_id": task.id.to_string(),
                "candidate": profile.name,
                "pid": child.id(),
            }),
        )?;

        // Wait with the candidate's runtime limit.
        let deadline = profile
            .limits
            .max_runtime_seconds
            .map(|s| Instant::now() + Duration::from_secs(s));
        let output = loop {
            match child.try_wait() {
                Ok(Some(_)) => break child.wait_with_output(),
                Ok(None) => {
                    if deadline.map(|d| Instant::now() >= d).unwrap_or(false) {
                        let _ = child.kill();
                        let _ = child.wait();
                        exec_store.mark_failed(
                            exe_id,
                            &format!(
                                "candidate exceeded max runtime of {}s",
                                profile.limits.max_runtime_seconds.unwrap_or_default()
                            ),
                        )?;
                        return Err(DraftError::new(
                            DraftErrorKind::ExecutionLimitExceeded,
                            format!("candidate '{}' exceeded its max runtime", profile.name),
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => break Err(e),
            }
        }
        .map_err(|e| DraftError::storage(format!("failed to collect candidate output: {e}")))?;

        // Capture (capped, redacted) output into the object store.
        let cap = profile.limits.max_output_bytes.unwrap_or(u64::MAX) as usize;
        let object_store = ObjectStore::new(ws.layout.clone());
        let stdout_text = redact_secrets(&String::from_utf8_lossy(
            &output.stdout[..output.stdout.len().min(cap)],
        ));
        let stderr_text = redact_secrets(&String::from_utf8_lossy(
            &output.stderr[..output.stderr.len().min(cap)],
        ));
        let stdout_ref = object_store.put_bytes(stdout_text.as_bytes())?;
        let stderr_ref = object_store.put_bytes(stderr_text.as_bytes())?;
        let exit_code = output.status.code();
        exec_store.update(exe_id, |e| {
            e.stdout_ref = Some(stdout_ref.clone());
            e.stderr_ref = Some(stderr_ref.clone());
            e.exit_code = exit_code;
        })?;

        if !output.status.success() {
            let reason = format!(
                "candidate command exited with status {}",
                exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "unknown".into())
            );
            exec_store.mark_failed(exe_id, &reason)?;
            ws.events()?.append(
                EventKind::OperationRefused,
                Some(exe_id.to_string()),
                serde_json::json!({
                    "task_id": task.id.to_string(),
                    "candidate": profile.name,
                    "reason": reason,
                }),
            )?;
            return Ok(None);
        }

        // Collect changes from the isolated workspace.
        let changes = if isolated {
            collect_isolated_changes(&ws.root, &work_dir, baseline)?
        } else {
            // In-place runs mutate the tree directly; diff tree vs baseline.
            collect_isolated_changes(&ws.root, &ws.root, baseline)?
        };

        // Guards: protected files, forbidden zones, allowed zones, limits.
        self.enforce_execution_guards(ws, task, profile, execution, &changes)?;

        if changes.is_empty() {
            // A run that changed nothing produces no ChangePack. There is nothing
            // to seal a revision of, and an empty ChangePack would be a thing to
            // review that says nothing happened.
            exec_store.mark_completed(exe_id)?;
            ws.events()?.append(
                EventKind::OperationExecuted,
                Some(exe_id.to_string()),
                serde_json::json!({
                    "task_id": task.id.to_string(),
                    "candidate": profile.name,
                    "changed_files": 0,
                    "produced_change": Value::Null,
                }),
            )?;
            return Ok(None);
        }

        // Apply the run's output to the workspace, then seal a revision of it.
        //
        // An isolated run produces its files in a work directory, and a
        // revision seals what is *observed* in the workspace — so the output
        // has to land there before it can be sealed at all. It stays: a task's
        // work is visible in the tree like any other, and what happens to it
        // next is the ordinary chain of evidence, assessment, gate and
        // decision. Nothing here accepts it.
        if isolated {
            apply_isolated_changes(&ws.root, &work_dir, &changes)?;
        }
        let change_pack_id = Some(self.seal_execution_revision(ws, task, profile, &changes)?);
        exec_store.update(exe_id, |e| {
            e.produced_change = change_pack_id.clone();
        })?;
        exec_store.mark_completed(exe_id)?;
        ws.events()?.append(
            EventKind::OperationExecuted,
            Some(exe_id.to_string()),
            serde_json::json!({
                "task_id": task.id.to_string(),
                "candidate": profile.name,
                "changed_files": changes.len(),
                "produced_change": change_pack_id,
            }),
        )?;
        // Isolated work dir is no longer needed after a completed run.
        if isolated {
            let _ = fs::remove_dir_all(paths.execution_work_dir(exe_id));
        }
        Ok(change_pack_id)
    }

    /// Open a ChangePack for this execution and seal what it produced.
    ///
    /// The identity is derived from the task and what actually changed, so a
    /// re-run that produces the same output converges on the same ChangePack and
    /// the same revision rather than minting a second thing to review.
    ///
    /// The scope is the resources the run touched. Resolution narrows it to
    /// what the accepted Baseline holds — a file the run created is in the
    /// sealed state root but outside the reviewed boundary, because nobody has
    /// yet agreed that the ChangePack may reach it.
    fn seal_execution_revision(
        &self,
        ws: &Workspace,
        task: &crate::task::TaskDefinition,
        profile: &crate::task::candidate::CandidateProfile,
        changes: &[IsolatedChange],
    ) -> DraftResult<String> {
        let scope: Vec<String> = changes
            .iter()
            .map(|change| {
                crate::dcg::resource::resource_id_for_locator(&format!(
                    "file:{}",
                    change.path.as_str()
                ))
                .to_string()
            })
            .collect();
        let change = self.dcg_open_change_pack(
            &ws.root,
            &format!("{} via {}", task.name, profile.name),
            &scope,
        )?;
        self.dcg_seal(&ws.root, change.id.as_str())?;
        Ok(change.id.to_string())
    }

    /// Validate collected candidate changes against the task contract and
    /// candidate limits. Violations emit warning events and fail the
    /// execution without touching the working tree.
    fn enforce_execution_guards(
        &self,
        ws: &Workspace,
        task: &crate::task::TaskDefinition,
        profile: &crate::task::candidate::CandidateProfile,
        execution: &crate::task::Execution,
        changes: &[IsolatedChange],
    ) -> DraftResult<()> {
        let exec_store = crate::task::ExecutionStore::for_root(&ws.root);
        let mut violations: Vec<String> = Vec::new();
        let rules = self.protections(&ws.root)?;
        for change in changes {
            let path = change.path.as_str();
            if crate::project::protected::matches_rules(&rules, path) {
                violations.push(format!("protected file '{path}'"));
                let _ = ws.events()?.append(
                    EventKind::OperationRefused,
                    Some(execution.id.to_string()),
                    serde_json::json!({
                        "path": path,
                        "candidate": profile.name,
                        "task_id": task.id.to_string(),
                    }),
                );
                continue;
            }
            if task
                .forbidden_zones
                .iter()
                .chain(profile.limits.forbidden_paths.iter())
                .any(|zone| pattern_match(zone, path))
            {
                violations.push(format!("forbidden zone touched: '{path}'"));
                continue;
            }
            let allowed = {
                let task_ok = task.allowed_zones.is_empty()
                    || task.allowed_zones.iter().any(|z| pattern_match(z, path));
                let limit_ok = profile.limits.allowed_paths.is_empty()
                    || profile
                        .limits
                        .allowed_paths
                        .iter()
                        .any(|z| pattern_match(z, path));
                task_ok && limit_ok
            };
            if !allowed {
                violations.push(format!("outside allowed zones: '{path}'"));
            }
        }
        if let Some(max) = profile.limits.max_files_changed {
            if changes.len() as u32 > max {
                violations.push(format!(
                    "{} files changed exceeds the candidate limit of {max}",
                    changes.len()
                ));
            }
        }
        if let Some(max) = profile.limits.max_output_bytes_changed {
            let total: u64 = changes.iter().map(|change| change.bytes).sum();
            if total > max {
                violations.push(format!(
                    "{total} bytes changed exceeds the candidate limit of {max}"
                ));
            }
        }
        if violations.is_empty() {
            return Ok(());
        }
        let scope_result = serde_json::json!({ "ok": false, "violations": violations });
        let reason = format!("candidate output rejected: {}", violations.join("; "));
        exec_store.update(execution.id.as_str(), |e| {
            e.scope_result = Some(scope_result.clone());
        })?;
        exec_store.mark_failed(execution.id.as_str(), &reason)?;
        ws.events()?.append(
            EventKind::OperationRefused,
            Some(execution.id.to_string()),
            serde_json::json!({
                "task_id": task.id.to_string(),
                "candidate": profile.name,
                "violations": violations,
            }),
        )?;
        Err(
            DraftError::new(DraftErrorKind::ExecutionLimitExceeded, reason)
                .with_suggestion("narrow the task's allowed zones or adjust the candidate limits"),
        )
    }

    /// The Baseline this project accepts, or "uninitialized".
    fn accepted_baseline_ref(&self, ws: &Workspace) -> DraftResult<String> {
        Ok(crate::dcg::baseline::current_baseline(&ws.layout)?
            .map(|baseline| baseline.to_string())
            .unwrap_or_else(|| "uninitialized".to_string()))
    }

    fn candidate_registry_for(
        &self,
        ws: &Workspace,
    ) -> DraftResult<crate::task::candidate::CandidateRegistry> {
        let project_config = ws.layout.config_toml();
        let global_config = Some(crate::project::home::DraftGlobalStore::locate()?.config_toml());
        crate::task::candidate::CandidateRegistry::load(
            Some(project_config.as_path()),
            global_config.as_deref(),
        )
    }

    /// Resolved candidate profiles for this project (config + records + builtins).
    pub fn candidate_profiles(
        &self,
        cwd: &Path,
    ) -> DraftResult<Vec<crate::task::candidate::CandidateProfile>> {
        let ws = self.open(cwd)?;
        Ok(self.candidate_registry_for(&ws)?.profiles())
    }

    /// Resolved task presets for this project.
    pub fn candidate_presets(
        &self,
        cwd: &Path,
    ) -> DraftResult<Vec<crate::task::candidate::CandidatePreset>> {
        let ws = self.open(cwd)?;
        Ok(self.candidate_registry_for(&ws)?.presets())
    }

    pub fn candidate_list(
        &self,
        cwd: &Path,
    ) -> DraftResult<Vec<crate::task::candidate::CandidateProfile>> {
        self.candidate_profiles(cwd)
    }

    pub fn candidate_show(
        &self,
        cwd: &Path,
        name: &str,
    ) -> DraftResult<crate::task::candidate::CandidateProfile> {
        let ws = self.open(cwd)?;
        self.candidate_registry_for(&ws)?.profile(name)
    }

    pub fn candidate_add(
        &self,
        cwd: &Path,
        name: &str,
        kind: Option<&str>,
        template: Vec<String>,
    ) -> DraftResult<crate::task::candidate::CandidateProfile> {
        self.write_candidate(
            cwd,
            name,
            kind.unwrap_or("command"),
            "custom",
            template,
            EventKind::PolicyUpdated,
        )
    }

    pub fn candidate_update(
        &self,
        cwd: &Path,
        name: &str,
        kind: Option<&str>,
        template: Vec<String>,
    ) -> DraftResult<crate::task::candidate::CandidateProfile> {
        let existing_kind = match self.candidate_show(cwd, name)?.kind {
            crate::task::candidate::CandidateKind::Agent => "agent",
            crate::task::candidate::CandidateKind::Command => "command",
            crate::task::candidate::CandidateKind::Manual => "manual",
            crate::task::candidate::CandidateKind::Human => "human",
        };
        self.write_candidate(
            cwd,
            name,
            kind.unwrap_or(existing_kind),
            "custom",
            template,
            EventKind::PolicyUpdated,
        )
    }

    pub fn candidate_remove(
        &self,
        cwd: &Path,
        name: &str,
    ) -> DraftResult<crate::task::candidate::CandidateProfile> {
        let ws = self.open(cwd)?;
        let record = self.candidate_show(cwd, name)?;
        crate::project::config::remove_table(
            &ws.layout.config_toml(),
            &format!("candidates.{name}"),
        )?;
        ws.events()?.append(
            EventKind::PolicyUpdated,
            Some(name.to_string()),
            serde_json::json!({}),
        )?;
        Ok(record)
    }

    fn write_candidate(
        &self,
        cwd: &Path,
        name: &str,
        kind: &str,
        source: &str,
        template: Vec<String>,
        event: EventKind,
    ) -> DraftResult<crate::task::candidate::CandidateProfile> {
        let ws = self.open(cwd)?;
        let _ = crate::task::candidate::CandidateKind::parse(kind)?;
        crate::project::config::set_value(
            &ws.layout.config_toml(),
            &format!("candidates.{name}.kind"),
            kind,
        )?;
        if !template.is_empty() {
            crate::project::config::set_value(
                &ws.layout.config_toml(),
                &format!("candidates.{name}.command"),
                &template.join(" "),
            )?;
        }
        let record = self.candidate_registry_for(&ws)?.profile(name)?;
        ws.events()?.append(
            event,
            Some(name.to_string()),
            serde_json::json!({
                "source": source,
                "profile": record,
            }),
        )?;
        Ok(record)
    }

    /// How each changed resource should be presented, and by whom.
    ///
    /// Every resource gets an answer. Where nothing claims a resource, or where
    /// two publishers tie, the answer is the universal neutral rendering —
    /// which Draft always provides and no extension contributes — with the
    /// reason recorded so a reader can tell "nothing knows how to show this"
    /// from "two things disagree about how".
    pub fn presentation_bindings(
        &self,
        cwd: &Path,
        surface: &str,
    ) -> DraftResult<Vec<serde_json::Value>> {
        use draft_extension_contract::PresentationSurface;
        let surface = match surface {
            "resource" => PresentationSurface::ResourceView,
            "change" => PresentationSurface::ChangeView,
            other => {
                return Err(DraftError::invalid_config(format!(
                    "unknown presentation surface '{other}'; expected 'resource' or 'change'"
                )))
            }
        };
        let ws = self.open(cwd)?;
        let snapshot = self.observe(&ws)?;
        let contributions = self.active_contributions();
        let classification =
            crate::evidence::classification::classify_snapshot(&snapshot, &contributions);
        let classes = classification.by_resource();

        let mut out = Vec::with_capacity(snapshot.resources.len());
        for state in &snapshot.resources {
            let view = crate::extension::ResourceView {
                locator_scheme: state.locator.scheme.as_str(),
                locator_body: state.locator.body.as_str(),
                media_type: state.media_type.as_deref(),
                form: state.form,
                attributes: &state.attributes,
                content_size: state.content_size,
            };
            let empty = BTreeSet::new();
            let resolved = contributions.presentation_for(
                surface,
                &view,
                classes.get(&state.resource_id).unwrap_or(&empty),
            );
            out.push(match resolved {
                crate::extension::Resolution::Resolved {
                    value,
                    contributors,
                } => serde_json::json!({
                    "resource_id": state.resource_id.as_str(),
                    "locator": state.locator,
                    "state": "resolved",
                    "presentation_id": value.presentation_id.qualified(),
                    "engine": value.engine,
                    "config": value.config,
                    "contributors": contributors,
                }),
                crate::extension::Resolution::Ambiguous { candidates } => serde_json::json!({
                    "resource_id": state.resource_id.as_str(),
                    "locator": state.locator,
                    "state": "ambiguous",
                    "candidates": candidates
                        .iter()
                        .map(|candidate| serde_json::json!({
                            "presentation_id": candidate.value.presentation_id.qualified(),
                            "engine": candidate.value.engine,
                            "contributed_by": candidate.extension_id,
                        }))
                        .collect::<Vec<_>>(),
                    "fallback": NEUTRAL_PRESENTATION,
                }),
                crate::extension::Resolution::NoMatch => serde_json::json!({
                    "resource_id": state.resource_id.as_str(),
                    "locator": state.locator,
                    "state": "fallback",
                    "engine": NEUTRAL_PRESENTATION,
                }),
            });
        }
        Ok(out)
    }

    /// Resolve a template id against the installed contributions.
    ///
    /// Draft ships none, so with nothing installed this always fails — and it
    /// says so by naming what *is* available, rather than reporting an unknown
    /// id as though the caller mistyped one that exists.
    fn resolve_task_template(&self, id: &str) -> DraftResult<crate::task::TaskTemplate> {
        let contributions = self.active_contributions();
        let templates = contributions.task_templates();
        let parsed = draft_extension_contract::NamespacedId::parse(id)
            .map_err(|error| DraftError::invalid_config(format!("invalid template id: {error}")))?;
        match templates.get(&parsed) {
            Some(contributed) => crate::task::resolve_template(contributed),
            None if templates.is_empty() => Err(DraftError::not_found(format!(
                "no task template '{id}': no installed extension contributes any"
            ))
            .with_suggestion(
                "install an extension contributing `task_template`, or create the task without \
                 --template",
            )),
            None => {
                let available: Vec<String> = templates.keys().map(|key| key.qualified()).collect();
                Err(DraftError::not_found(format!(
                    "no task template '{id}'; available: {}",
                    available.join(", ")
                )))
            }
        }
    }

    /// The tool actions installed extensions offer, and what each applies to.
    ///
    /// An action whose artifact has no grant to execute is listed as withheld
    /// rather than omitted: "nothing offers this" and "something offers it and
    /// you have not authorized it" are different answers, and only the second
    /// has a fix.
    pub fn tool_list(&self, cwd: &Path) -> DraftResult<Vec<serde_json::Value>> {
        let ws = self.open(cwd)?;
        let snapshot = self.observe(&ws)?;
        let contributions = self.active_contributions();
        let classification =
            crate::evidence::classification::classify_snapshot(&snapshot, &contributions);
        let classes = classification.by_resource();
        let empty = BTreeSet::new();

        let mut out = Vec::new();
        for contributed in &contributions.tool_actions {
            let action = &contributed.value;
            let applies: Vec<String> = snapshot
                .resources
                .iter()
                .filter(|state| {
                    crate::extension::capability::matches(
                        &action.applies_to,
                        &resource_view_of(state),
                        classes.get(&state.resource_id).unwrap_or(&empty),
                    )
                })
                .map(|state| state.locator.body.clone())
                .collect();
            let (command, _) = self.authorized_command(
                &contributions,
                &contributed.extension_id,
                &action.operation,
            );
            out.push(serde_json::json!({
                "action_id": action.action_id.qualified(),
                "display_name": action.display_name,
                "description": action.description,
                "contributed_by": contributed.extension_id,
                "effect": action.effect,
                "authorized": command.is_some(),
                "applies_to": applies,
            }));
        }
        out.sort_by(|left, right| left["action_id"].as_str().cmp(&right["action_id"].as_str()));
        Ok(out)
    }

    /// Run one tool action and apply what it proposed, as Draft's own operation.
    ///
    /// The tool runs, returns findings and proposed mutations, and stops there.
    /// Draft opens an edit session under its own operation id and attribution,
    /// stages each proposal — which is where protections, path safety and the
    /// workspace lease apply, identically to a human edit — and commits. A
    /// proposal Draft refuses stops the whole operation: applying the half it
    /// liked would leave the project in a state neither the tool nor the user
    /// asked for.
    pub fn tool_invoke(
        &self,
        cwd: &Path,
        action_id: &str,
        apply: bool,
    ) -> DraftResult<serde_json::Value> {
        let ws = self.open(cwd)?;
        let contributions = self.active_contributions();
        let parsed = draft_extension_contract::NamespacedId::parse(action_id)
            .map_err(|error| DraftError::invalid_config(format!("invalid action id: {error}")))?;
        let Some(contributed) = contributions
            .tool_actions
            .iter()
            .find(|contributed| contributed.value.action_id == parsed)
        else {
            let available: Vec<String> = contributions
                .tool_actions
                .iter()
                .map(|contributed| contributed.value.action_id.qualified())
                .collect();
            return Err(DraftError::not_found(if available.is_empty() {
                format!("no tool action '{action_id}': no installed extension contributes any")
            } else {
                format!(
                    "no tool action '{action_id}'; available: {}",
                    available.join(", ")
                )
            }));
        };
        let action = &contributed.value;

        let (_, decision) =
            self.authorized_command(&contributions, &contributed.extension_id, &action.operation);
        let Some(decision) = decision else {
            return Err(DraftError::new(
                DraftErrorKind::CapabilityNotAuthorized,
                format!(
                    "'{action_id}' is contributed by {} but its artifact has no grant to execute",
                    contributed.extension_id
                ),
            )
            .with_suggestion(format!(
                "run `draft extension authorize {} --permission process.execute`",
                contributed.extension_id
            )));
        };

        // The resources the action declares itself applicable to, from the
        // current authoritative observation.
        let snapshot = self.observe(&ws)?;
        let classification =
            crate::evidence::classification::classify_snapshot(&snapshot, &contributions);
        let classes = classification.by_resource();
        let empty = BTreeSet::new();
        let subjects: Vec<&crate::dcg::resource::RawResourceState> = snapshot
            .resources
            .iter()
            .filter(|state| {
                crate::extension::capability::matches(
                    &action.applies_to,
                    &resource_view_of(state),
                    classes.get(&state.resource_id).unwrap_or(&empty),
                )
            })
            .collect();

        let operation_id = crate::support::common::OperationId::generate();
        let request = serde_json::json!({
            "action_id": action.action_id.qualified(),
            "resources": subjects
                .iter()
                .map(|state| serde_json::json!({
                    "resource_id": state.resource_id.as_str(),
                    "locator": state.locator,
                    "state_digest": state.state_digest,
                }))
                .collect::<Vec<_>>(),
        });
        let response = crate::execution::mechanism::invoke_command(
            &action.operation,
            &request,
            &crate::execution::mechanism::MechanismInputs::default(),
            &crate::execution::mechanism::MechanismContext {
                workspace_id: ws.workspace_id.to_string(),
                operation_id: operation_id.to_string(),
                producer: producer_ref_for(&contributions, &contributed.extension_id),
                authorization_decision: Some(decision.clone()),
            },
        )?;

        let result: crate::execution::mechanism::proposal::ToolActionResult =
            serde_json::from_value(response.payload.clone()).map_err(|error| {
                DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!("'{action_id}' returned a response Draft cannot read: {error}"),
                )
            })?;
        crate::execution::mechanism::proposal::check_within_declared_effect(
            action_id,
            &action.effect,
            &result,
        )?;

        let mut report = serde_json::json!({
            "action_id": action.action_id.qualified(),
            "contributed_by": contributed.extension_id,
            "operation_id": operation_id.to_string(),
            "authorization_decision": decision,
            "executable_identity": response.executable_identity,
            "exit_code": response.exit_code,
            "duration_ms": response.duration_ms,
            "summary": result.summary,
            "detail": result.detail,
            "proposed_mutations": result.proposed_mutations,
            "applied": false,
        });
        if !apply || result.proposed_mutations.is_empty() {
            return Ok(report);
        }

        // From here on it is Draft's operation. The session carries Draft's own
        // operation id and attribution; the tool named neither and could not.
        let store = crate::execution::workspace::WorkspaceStore::for_workspace(
            &ws.root,
            self.protections(&ws.root)?,
        );
        let session = store.open(
            crate::execution::workspace::EditAttribution::Task {
                id: operation_id.to_string(),
            },
            operation_id.clone(),
        )?;
        for proposal in &result.proposed_mutations {
            use crate::execution::mechanism::proposal::ProposedMutation as Proposal;
            match proposal {
                Proposal::SetContent { locator, content } => {
                    store.stage_content(
                        &session.id,
                        locator,
                        content.clone(),
                        operation_id.clone(),
                    )?;
                }
                Proposal::CreateCollection { locator } => {
                    store.stage_create_collection(&session.id, locator, operation_id.clone())?;
                }
                Proposal::Relocate { from, to } => {
                    store.stage_relocate(&session.id, from, to, operation_id.clone())?;
                }
                Proposal::Remove { locator, recursive } => {
                    store.stage_remove(&session.id, locator, *recursive, operation_id.clone())?;
                }
            }
        }
        let committed = store.commit(&session.id, operation_id.clone())?;
        ws.events()?.append(
            EventKind::OperationExecuted,
            Some(operation_id.to_string()),
            serde_json::json!({
                "action_id": action.action_id.qualified(),
                "contributed_by": contributed.extension_id,
                "authorization_decision": decision,
                "resources_changed": committed.resources_changed,
            }),
        )?;
        report["applied"] = serde_json::Value::Bool(true);
        report["resources_changed"] = serde_json::to_value(&committed.resources_changed)?;
        Ok(report)
    }

    /// Mutable work that belongs to a context other than the one given.
    ///
    /// ChangePacks and ChangePack workspaces only. A promoted ChangePack is
    /// history and is never superseded — it recorded what was true under the
    /// semantics of its day, and still does.
    fn context_sensitive_work(
        &self,
        ws: &Workspace,
        context_digest: &str,
    ) -> DraftResult<Vec<crate::dcg::observation_lifecycle::SupersededWork>> {
        use crate::dcg::observation_lifecycle::{SupersededKind, SupersededWork};
        let mut stranded = Vec::new();
        for change in
            crate::dcg::change_pack::ChangePackStore::new(ws.layout.change_packs_dir()).list()?
        {
            // A completed ChangePack is history. It recorded what was true under
            // the semantics of its day and still does; only work that could
            // still change is stranded.
            if change.lifecycle != crate::dcg::change_pack::ChangePackLifecycle::Active {
                continue;
            }
            // Any ChangePack with sealed work is reported, not only one sealed
            // under this exact context.
            //
            // A revision records the state root it sealed, not the observation
            // semantics that produced it — the context lives on the
            // Observations behind the roots, and nothing links a revision back
            // to them. So the precise question cannot be asked here, and of the
            // two available errors only one is safe: this warning exists to
            // stop somebody adopting new semantics and silently stranding
            // work, and a warning that misses work defeats it. Naming a
            // ChangePack that turns out to be unaffected costs a second look.
            let sealed =
                crate::dcg::revision_pack::RevisionPackStore::new(ws.layout.revision_packs_dir())
                    .list()?
                    .into_iter()
                    .any(|revision| revision.change_pack == change.id);
            if !sealed {
                continue;
            }
            stranded.push(SupersededWork {
                kind: SupersededKind::ChangePack,
                id: change.id.to_string(),
                context_digest: context_digest.to_string(),
            });
        }
        stranded.sort();
        Ok(stranded)
    }

    /// Every intent a caller may declare, with the vocabulary that declares it.
    pub fn intents(&self, cwd: &Path) -> DraftResult<Vec<serde_json::Value>> {
        let _ = self.open(cwd)?;
        let contributions = self.active_contributions();
        let mut out = vec![serde_json::json!({
            "intent_id": crate::dcg::change_pack_store::UNSPECIFIED_INTENT,
            "display_name": "Unspecified",
            "description": "No intent vocabulary is installed to name one.",
            "contributed_by": serde_json::Value::Null,
        })];
        for preset in &contributions.intent_vocabularies {
            for intent in &preset.value.intents {
                out.push(serde_json::json!({
                    "intent_id": intent.intent_id.qualified(),
                    "display_name": intent.display_name,
                    "description": intent.description,
                    "contributed_by": preset.extension_id,
                }));
            }
        }
        Ok(out)
    }

    pub fn selected_change_pack_id(&self, cwd: &Path) -> DraftResult<String> {
        let ws = self.open(cwd)?;
        let raw = fs::read_to_string(ws.layout.selected_change_pack_file()).map_err(|e| {
            DraftError::not_found(format!(
                "no selected change: {e}; run `draft pack select <cpk-id/name>`"
            ))
        })?;
        Ok(raw.trim().to_string())
    }

    /// Plan a rollback: what would be restored, what would be removed, and what
    /// could not be reached at all.
    ///
    /// Two things this deliberately does not do. It does not treat current state
    /// as evidence about the target — current state says only where the project
    /// is now. And it does not assume a resource is restorable because it was
    /// once observed: an anchor is separate, contemporaneous evidence, and
    /// without one the resource is reported as unrecoverable rather than
    /// silently skipped.
    pub fn rollback_plan(&self, cwd: &Path, reference: &str) -> DraftResult<RollbackPlan> {
        let ws = self.open(cwd)?;
        let target = resolve_snapshot_reference(&ws, reference)?;
        let current = self.observe(&ws)?;
        let anchors = load_anchor_set(&ws, &target)?;
        let recovery_status = anchors.status(&target);
        let plan = crate::dcg::anchor::ResourceRestorePlan::plan(
            crate::support::common::OperationId::generate(),
            &target,
            &anchors,
            &current,
        )?;

        let mut affected_locators: Vec<ResourceLocator> = plan
            .restore_targets
            .iter()
            .map(|restore| restore.target_locator.clone())
            .chain(
                plan.absence_targets
                    .iter()
                    .map(|absence| absence.current_locator.clone()),
            )
            .filter(|locator| {
                locator.scheme != crate::extension::FILE_SCHEME || !is_draft_path(&locator.body)
            })
            .collect();
        affected_locators.sort();
        affected_locators.dedup();

        let mut warnings = Vec::new();
        if !plan.absence_targets.is_empty() {
            warnings.push(format!(
                "{} resource(s) will be removed because the target state proves they were absent",
                plan.absence_targets.len()
            ));
        }
        if !plan.restore_targets.is_empty() {
            warnings.push(format!(
                "{} resource(s) will be overwritten with their target state",
                plan.restore_targets.len()
            ));
        }
        // Say plainly, up front, that this rollback cannot reach the target —
        // rather than letting it run and reporting a partial result afterwards.
        if !plan.can_be_complete() {
            warnings.push(
                "this rollback cannot fully restore the target; see the recorded uncertainties"
                    .to_string(),
            );
        }

        Ok(RollbackPlan {
            schema_version: current_version(ContractId::RollbackPlan),
            id: RollbackPlanId::generate(),
            rollback_snapshot_id: target.id.clone(),
            target_snapshot_digest: target.snapshot_digest.clone(),
            restored_resources: plan
                .restore_targets
                .iter()
                .map(|restore| restore.resource_id.clone())
                .collect(),
            removed_resources: plan
                .absence_targets
                .iter()
                .map(|absence| absence.current_resource_id.clone())
                .collect(),
            affected_locators,
            known_uncertainties: plan.known_uncertainties.clone(),
            recovery_status,
            destructive: true,
            warnings,
        })
    }

    /// Restore a past state, then prove whether the target was actually reached.
    ///
    /// The proof is the point. A successful mutation means changes were applied;
    /// only a fresh, complete observation whose every state digest equals the
    /// target's establishes that the project *is* in that state.
    pub fn rollback(&self, cwd: &Path, reference: &str, yes: bool) -> DraftResult<RollbackRecord> {
        let ws = self.open(cwd)?;
        let plan = self.rollback_plan(cwd, reference)?;
        if plan.destructive && !yes {
            return Err(DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                "rollback is destructive and requires explicit CLI invocation",
            ));
        }
        let started = now();
        let target = load_snapshot(&ws, &plan.rollback_snapshot_id)?;
        let current = self.observe(&ws)?;
        let anchors = load_anchor_set(&ws, &target)?;
        let restore_plan = crate::dcg::anchor::ResourceRestorePlan::plan(
            crate::support::common::OperationId::generate(),
            &target,
            &anchors,
            &current,
        )?;
        apply_restore_plan(&ws, &restore_plan, &anchors)?;

        // Re-observe under live fencing and compare the complete state — every
        // digest, every locator, and the absences too.
        let observed = self.observe(&ws)?;
        let outcome = crate::dcg::anchor::classify_rollback(&restore_plan, &target, &observed);

        let mut record = RollbackRecord {
            schema_version: current_version(ContractId::RollbackRecord),
            id: ReceiptId::generate(),
            rollback_plan_id: plan.id.clone(),
            actor_id: resolve_actor(&ws.layout.draft_dir)?.id,
            status: outcome.as_str().to_string(),
            outcome: outcome.clone(),
            started_at: started,
            ended_at: now(),
            record_digest: String::new(),
        };
        write_rollback_record(&ws, &mut record)?;
        ws.events()?.append(
            EventKind::RecoveryPerformed,
            Some(record.id.to_string()),
            serde_json::to_value(&record).expect("Draft-owned records must serialize"),
        )?;
        Ok(record)
    }

    /// `draft recover plan <target>`: resolve the target and report what
    /// would change and which safety checks pass, without mutating anything.
    pub fn rollback_dry_run(&self, cwd: &Path, reference: &str) -> DraftResult<DryRunReport> {
        let ws = self.open(cwd)?;
        let mut checks = Vec::new();
        // Target id prefix must be chk_/cpk_/rcp_ (validated by the resolver).
        let plan = match self.rollback_plan(cwd, reference) {
            Ok(plan) => {
                checks.push(DoctorCheck::ok("target", format!("resolved {reference}")));
                Some(plan)
            }
            Err(e) => {
                checks.push(DoctorCheck::fail("target", e.message.clone()));
                None
            }
        };
        let affected: Vec<String> = plan
            .as_ref()
            .map(|plan| {
                plan.affected_locators
                    .iter()
                    .map(|locator| locator.body.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // Rollback deletes where the target proves absence. A dry run must say
        // so before anyone runs the real thing.
        if let Some(plan) = plan.as_ref() {
            checks.push(bool_check(
                "removals-proved",
                plan.known_uncertainties.iter().all(|uncertainty| {
                    !matches!(
                        uncertainty,
                        crate::dcg::anchor::RollbackUncertainty::TargetStateUnknown { .. }
                    )
                }),
                format!(
                    "{} removal(s) backed by target coverage",
                    plan.removed_resources.len()
                ),
                "some resources cannot be proved absent in the target; they will not be removed",
            ));
            checks.push(bool_check(
                "recovery-material",
                plan.recovery_status.is_fully_anchored(),
                "every target resource has retained recovery material",
                format!(
                    "target is {} — rollback cannot be complete",
                    plan.recovery_status.as_str()
                ),
            ));
        }
        // No affected path may touch `.draft/` (already filtered, assert here).
        let draft_touch = affected.iter().any(|f| is_draft_path(f));
        checks.push(bool_check(
            "draft-exclusion",
            !draft_touch,
            ".draft/ is not touched",
            "rollback would touch .draft/",
        ));
        // Event chain must be intact to trust the rollback.
        checks.push(match self.verify_events(&ws.root) {
            Ok(_) => DoctorCheck::ok("event-chain", "intact"),
            Err(e) => DoctorCheck::fail("event-chain", e.message),
        });
        let allowed = plan.is_some() && checks.iter().all(|c| c.ok);
        Ok(DryRunReport {
            action: "rollback".to_string(),
            target: reference.to_string(),
            would_proceed: allowed,
            resulting_state: if allowed {
                "project restored to target and verified".to_string()
            } else {
                "blocked".to_string()
            },
            affected_resources: affected,
            checks,
        })
    }

    /// The command for a contributed operation, and the decision permitting it.
    ///
    /// Returns `None` for the command when the artifact holds no grant. The
    /// check is still selected — it is reported `Unavailable`, which is not a
    /// pass and not a silent omission.
    pub(crate) fn authorized_command(
        &self,
        contributions: &crate::extension::ActiveContributions,
        extension_id: &str,
        operation: &crate::extension::MechanismOperation,
    ) -> (
        Option<draft_extension_contract::StructuredCommand>,
        Option<String>,
    ) {
        let Some(command) = operation.executor.command() else {
            return (None, None);
        };
        // Command-bearing contributions arrive already withheld when the
        // artifact is not authorized to run them, so reaching one here means the
        // grant exists.
        let withheld = contributions
            .withheld
            .iter()
            .any(|withheld| withheld.extension_id == extension_id);
        if withheld {
            (None, None)
        } else {
            (
                Some(command.clone()),
                Some(format!("authorized:{extension_id}")),
            )
        }
    }

    /// The canonical hash-chained Activity Ledger.
    pub fn canonical_events(&self, cwd: &Path) -> DraftResult<Vec<ActivityEntry>> {
        let ws = self.open(cwd)?;
        crate::read_model::activity::entries(ws.events()?.log())
    }

    /// Every signed receipt this project holds.
    pub fn receipts(&self, cwd: &Path) -> DraftResult<Vec<Value>> {
        let ws = self.open(cwd)?;
        crate::receipt::ReceiptEnvelopeStore::for_layout(&ws.layout)
            .read_all()?
            .into_iter()
            .map(|envelope| serde_json::to_value(envelope).map_err(DraftError::from))
            .collect()
    }

    pub fn storage_stats(&self, cwd: &Path) -> DraftResult<StorageStats> {
        let ws = self.open(cwd)?;
        Ok(StorageStats {
            draft_size_bytes: dir_size(&ws.layout.draft_dir)?,
            repo_size_bytes: dir_size_excluding_draft(&ws.root)?,
            objects_size_bytes: dir_size(&ws.layout.objects_dir())?,
            changes_size_bytes: dir_size(&ws.layout.change_packs_content_dir())?,
            receipts_size_bytes: dir_size(&ws.layout.receipts_dir())?,
            events_size_bytes: fs::metadata(ws.layout.activity_log())
                .map(|m| m.len())
                .unwrap_or(0),
            draft_repo_ratio: storage_ratio(
                dir_size(&ws.layout.draft_dir)?,
                dir_size_excluding_draft(&ws.root)?,
            ),
            growth_status: storage_growth_status(
                dir_size(&ws.layout.draft_dir)?,
                dir_size_excluding_draft(&ws.root)?,
            ),
        })
    }

    pub fn storage_gc(&self, cwd: &Path) -> DraftResult<StorageMaintenanceReport> {
        let ws = self.open(cwd)?;
        let removed = garbage_collect_objects(&ws)?;
        ws.events()?.append(
            EventKind::MaintenanceCompleted,
            None,
            serde_json::json!({ "removed": removed }),
        )?;
        Ok(StorageMaintenanceReport::new(
            "gc",
            removed,
            "unreachable objects removed",
        ))
    }

    pub fn gc(&self, cwd: &Path) -> DraftResult<crate::app::maintenance::GcReport> {
        let ws = self.open(cwd)?;
        let paths = crate::project::layout::DraftLayout::for_root(&ws.root);
        let _lock =
            ProcessFileLock::acquire_exclusive(&paths.lock_file("gc"), Duration::from_secs(30))?;
        let activity = ws.events()?;
        activity.append(EventKind::MaintenanceStarted, None, serde_json::json!({}))?;
        match crate::app::maintenance::run(&paths) {
            Ok(report) => {
                activity.append(
                    EventKind::MaintenanceCompleted,
                    None,
                    serde_json::to_value(&report).expect("GC report is serializable"),
                )?;
                Ok(report)
            }
            Err(error) => {
                activity.append(
                    EventKind::MaintenanceFailed,
                    None,
                    serde_json::json!({ "reason": error.message }),
                )?;
                Err(error)
            }
        }
    }

    pub fn close(&self, cwd: &Path, force: bool) -> DraftResult<CloseReport> {
        // Recovery must remain possible when obsolete profile state blocks all
        // normal operations. Close never reads or applies that state.
        let ws = self.open_workspace(cwd, true)?;
        let paths = crate::project::layout::DraftLayout::for_root(&ws.root);
        let _lock =
            ProcessFileLock::acquire_exclusive(&paths.lock_file("close"), Duration::from_secs(30))?;
        let home = crate::project::home::DraftGlobalStore::locate()?;
        let retired_profile_present =
            crate::trust::identity::reject_retired_profile_state(Some(&paths.draft_dir)).is_err()
                || crate::trust::identity::global::reject_retired_actor_profile(&home).is_err()
                || crate::project::config::reject_retired_profile_config(&paths.config_toml())
                    .is_err()
                || crate::project::config::reject_retired_profile_config(&home.config_toml())
                    .is_err();
        let pending_changes = unsafe_pending_change_count(&paths)?;
        if pending_changes > 0 && !force {
            if !retired_profile_present {
                let _ = ws.events()?.append(
                    EventKind::MaintenanceFailed,
                    None,
                    serde_json::json!({
                        "reason": "pending changes",
                        "pending_changes": pending_changes
                    }),
                );
            }
            return Err(DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                format!("draft maintenance remove-project refused: {pending_changes} open change(s) remain"),
            )
            .with_suggestion(
                "Promote the work you want to keep, then remove the project; use --force only \
                 when you intend to discard it.",
            ));
        }
        if !retired_profile_present {
            let activity = ws.events()?;
            let detail = serde_json::json!({
                "forced": force,
                "pending_changes": pending_changes,
            });
            activity.append(EventKind::MaintenanceStarted, None, detail.clone())?;
            activity.append(EventKind::ProjectClosed, None, detail)?;
        }
        let draft_dir = ws.layout.draft_dir.display().to_string();
        crate::project::registry::ProjectRegistry::global()?.remove(ws.workspace_id.as_str())?;
        std::fs::remove_dir_all(&ws.layout.draft_dir)
            .map_err(|e| DraftError::storage(format!("remove .draft: {e}")))?;
        Ok(CloseReport {
            closed: true,
            forced: force,
            draft_dir,
            pending_changes,
        })
    }

    pub fn storage_compact(&self, cwd: &Path) -> DraftResult<StorageMaintenanceReport> {
        let ws = self.open(cwd)?;
        let compacted = compact_loose_objects(&ws)?;
        ws.events()?.append(
            EventKind::MaintenanceCompleted,
            None,
            serde_json::json!({ "compacted": compacted }),
        )?;
        Ok(StorageMaintenanceReport::new(
            "compact",
            compacted,
            "loose objects compacted",
        ))
    }

    pub fn storage_prune(&self, cwd: &Path) -> DraftResult<StorageMaintenanceReport> {
        let ws = self.open(cwd)?;
        let mut removed = 0;
        for dir in [ws.layout.cache_dir(), ws.layout.tmp_dir()] {
            if dir.exists() {
                for entry in fs::read_dir(&dir)? {
                    let path = entry?.path();
                    if path.is_file() {
                        fs::remove_file(path)?;
                        removed += 1;
                    }
                }
            }
        }
        ws.events()?.append(
            EventKind::MaintenanceCompleted,
            None,
            serde_json::json!({ "removed": removed }),
        )?;
        Ok(StorageMaintenanceReport::new(
            "prune",
            removed,
            "cache/tmp files pruned",
        ))
    }

    pub fn storage_doctor(&self, cwd: &Path) -> DraftResult<StorageDoctorReport> {
        let ws = self.open(cwd)?;
        let chain = self.verify_events(&ws.root)?;
        let object_errors = verify_objects(&ws)?;
        let receipt_errors = verify_receipts(&ws)?;
        let draft_exclusion_errors = verify_draft_hard_exclusion(&ws)?;
        Ok(StorageDoctorReport {
            activity_chain_ok: chain.ok,
            activity_chain_error: chain.error,
            draft_hard_excluded: draft_exclusion_errors.is_empty(),
            draft_exclusion_errors,
            objects_ok: object_errors.is_empty(),
            object_errors,
            receipts_ok: receipt_errors.is_empty(),
            receipt_errors,
            receipts: crate::receipt::ReceiptEnvelopeStore::for_layout(&ws.layout)
                .list_ids()?
                .len(),
            changes: self.dcg_change_packs(cwd)?.len(),
        })
    }

    pub fn receipt_show(&self, cwd: &Path, id: &str) -> DraftResult<Value> {
        validate_receipt_id(id)?;
        let ws = self.open(cwd)?;
        let receipt = draft_dcg_contract::ids::ReceiptId::parse(id)
            .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?;
        let envelope = crate::receipt::ReceiptEnvelopeStore::for_layout(&ws.layout)
            .get(&receipt)?
            .ok_or_else(|| DraftError::not_found(format!("no receipt '{id}'")))?;
        Ok(serde_json::to_value(envelope)?)
    }

    pub fn events(&self, cwd: &Path) -> DraftResult<Vec<ActivityEntry>> {
        let ws = self.open(cwd)?;
        crate::read_model::activity::entries(ws.events()?.log())
    }

    pub fn events_page(
        &self,
        cwd: &Path,
        top: bool,
        bottom: bool,
        page: Option<usize>,
        limit: Option<usize>,
        filter: Option<&str>,
    ) -> DraftResult<Vec<ActivityEntry>> {
        let ws = self.open(cwd)?;
        // `bottom` asks for the oldest end; anything else reads newest first,
        // which is what a person watching a project actually wants.
        crate::read_model::activity::page(ws.events()?.log(), !bottom || top, page, limit, filter)
    }

    pub fn replay_events(&self, cwd: &Path) -> DraftResult<ActivityReplay> {
        let ws = self.open(cwd)?;
        let activity = ws.events()?;
        let chain = activity.log().verify_chain();
        crate::read_model::activity::replay(activity.log(), ws.workspace_id.as_str(), chain)
    }

    pub fn index_rebuild(&self, cwd: &Path) -> DraftResult<IndexReport> {
        let ws = self.open(cwd)?;
        rebuild_index(&ws)
    }

    /// One Activity event, by id.
    pub fn event_show(&self, cwd: &Path, event_id: &str) -> DraftResult<ActivityEntry> {
        let ws = self.open(cwd)?;
        crate::read_model::activity::entry(ws.events()?.log(), event_id)
    }

    // ---------------------------------------------------------------------
    // The DCG application boundary.
    //
    // Every surface reaches promotion, publication and the authorization
    // chain through these and nothing else. They own transport-independent
    // input translation and delegate every decision to the domain: a surface
    // that reached past them would be a second opinion about what the project
    // accepts.
    // ---------------------------------------------------------------------

    /// One ChangePack: its lifecycle, its current definition and scope, and every
    /// revision sealed against it.
    ///
    /// The definition and the resolution are returned together because they
    /// answer two different questions — what the ChangePack is *for*, and what it
    /// may *touch* — and a reader given only one of them cannot tell whether
    /// work was in bounds.
    pub fn dcg_change_pack(&self, cwd: &Path, change: &str) -> DraftResult<Value> {
        let workspace = self.open(cwd)?;
        let change_pack_id = parse_change_pack_id(change)?;
        let view = crate::app::workflow::change_pack_views(&workspace)?
            .into_iter()
            .find(|view| view.change_pack == change_pack_id)
            .ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::NotFound,
                    format!("no ChangePack '{change}'"),
                )
            })?;

        let definitions = crate::dcg::definition::DefinitionStore::new(
            workspace.layout.definitions_dir(),
            workspace.layout.scope_resolutions_dir(),
        );
        let definition = definitions.definition(&view.current_definition)?;
        // The resolution of the *current* definition, if the revisions name
        // one. A ChangePack with no sealed revision has a declared scope but no
        // resolved one yet, and saying so is more useful than an empty set.
        let scope = match view.revisions.first() {
            Some(revision) => definitions.resolution(&revision.scope)?,
            None => None,
        };

        Ok(serde_json::json!({
            "change_pack_id": view.change_pack.to_string(),
            "lifecycle": view.lifecycle,
            "current_definition": view.current_definition,
            "definition": definition,
            "scope_resolution": scope,
            "revisions": view.revisions,
        }))
    }

    /// Stop work on a ChangePack, keeping everything recorded about it.
    ///
    /// Deliberately not a delete. "We tried this and stopped" is frequently
    /// the most useful thing in a project's history, and a ChangePack whose work
    /// is already in an accepted Baseline is refused rather than rewritten.
    pub fn dcg_abandon_change_pack(
        &self,
        cwd: &Path,
        change: &str,
    ) -> DraftResult<crate::dcg::change_pack::ChangePack> {
        let workspace = self.open(cwd)?;
        crate::app::promotion::change_pack_store(&workspace.layout)
            .abandon(&parse_change_pack_id(change)?)
    }

    /// Resume an abandoned ChangePack.
    pub fn dcg_reopen_change_pack(
        &self,
        cwd: &Path,
        change: &str,
    ) -> DraftResult<crate::dcg::change_pack::ChangePack> {
        let workspace = self.open(cwd)?;
        crate::app::promotion::change_pack_store(&workspace.layout)
            .reopen(&parse_change_pack_id(change)?)
    }

    /// What the project's authoritative state looks like right now.
    ///
    /// The read side of §2.56. A surface offering an action records this
    /// alongside the offer; the same surface re-reads it immediately before
    /// the mutation and compares. Both halves go through here, so an offer and
    /// its revalidation can never be derived from two different notions of
    /// "current".
    pub fn read_model_watermark(
        &self,
        cwd: &Path,
        change: Option<&str>,
    ) -> DraftResult<crate::read_model::ReadModelWatermark> {
        let workspace = self.open(cwd)?;
        let change = change.map(parse_change_pack_id).transpose()?;
        crate::read_model::freshness::current_watermark(
            &workspace.layout,
            &workspace.workspace_id,
            change.as_ref(),
        )
    }

    /// Judge a precondition against authoritative state read now.
    ///
    /// Deliberately not a comparison with whatever the caller sent back: a
    /// request that agrees with the descriptor it was issued proves the client
    /// echoed what it was given, and nothing about whether the project moved
    /// in between. That is exactly the window a stale action lands in.
    pub fn revalidate_precondition(
        &self,
        cwd: &Path,
        change: Option<&str>,
        precondition: &crate::read_model::RequestPrecondition,
    ) -> DraftResult<crate::read_model::ActionOutcome> {
        let current = self.read_model_watermark(cwd, change)?;
        Ok(crate::read_model::check(precondition, &current))
    }

    /// What this project currently accepts.
    ///
    /// The control record is the single place that answers it — the accepted
    /// Baseline, the policy and security state in force, and whether the
    /// project is open to new work at all — so a reader is never assembling
    /// that answer from four stores that may disagree.
    pub fn project_control(
        &self,
        cwd: &Path,
    ) -> DraftResult<crate::project::control::ProjectControlState> {
        let workspace = self.open(cwd)?;
        crate::project::control::ProjectControlStore::new(workspace.layout.project_control_dir())
            .read_unlocked()?
            .ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::CorruptData,
                    "this project has no control record",
                )
            })
    }

    /// Close this project to new work.
    ///
    /// A lifecycle transition, never a deletion: the history stays readable and
    /// verifiable, and every receipt this project issued keeps meaning what it
    /// meant. Removing Draft's metadata is `draft maintenance remove-project`,
    /// which is a different act with a different consequence.
    ///
    /// Closing an already-closed project is refused rather than treated as a
    /// no-op, because "closed" is a fact somebody recorded once and a second
    /// recording would claim it happened twice.
    pub fn project_close(
        &self,
        cwd: &Path,
    ) -> DraftResult<crate::project::control::ProjectControlState> {
        use crate::project::control::{ProjectControlStore, ProjectLifecycle};

        let workspace = self.open(cwd)?;
        let store = ProjectControlStore::new(workspace.layout.project_control_dir());
        let closed = store.with_locked_control(|guard| {
            let current = guard.current()?.ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::CorruptData,
                    "this project has no control record",
                )
            })?;
            if !current.is_active() {
                return Err(DraftError::new(
                    DraftErrorKind::Validation,
                    "this project is already closed",
                ));
            }
            let expected = guard.current_state()?;
            let closed = current.advanced(|state| {
                state.project_lifecycle = ProjectLifecycle::Closed;
            });
            guard.compare_exchange_locked(&expected, &closed)?;
            Ok(closed)
        })?;

        // Recorded after the transition committed. An event announcing a close
        // that the compare-exchange then refused would be a durable claim about
        // a state the project was never in.
        crate::app::activity::ProjectActivity::new(
            workspace.layout.clone(),
            &workspace.workspace_id,
        )
        .append(
            crate::activity::EventKind::ProjectClosed,
            Some(workspace.workspace_id.to_string()),
            serde_json::json!({ "generation": closed.generation }),
        )?;
        Ok(closed)
    }

    /// How two ChangePacks relate over the resources they both touch.
    ///
    /// Answered from the newest sealed revision on each side. A revision is
    /// what a ChangePack actually did — its `touched` set is checked against the
    /// scope it was sealed within — whereas a declared scope is only what it
    /// was allowed to do. Answering with the second would report every ChangePack
    /// scoped to a shared directory as interfering.
    ///
    /// Computed rather than stored, because the answer is only true of the two
    /// revisions as they are now: sealing a further revision invalidates a
    /// composability claim made earlier, and a cached one would keep asserting
    /// it.
    ///
    /// Resources only one side touched are absent from the result. Silence is
    /// the answer for them, and listing them would bury the ones that actually
    /// interfere.
    pub fn dcg_compare_change_packs(
        &self,
        cwd: &Path,
        left: &str,
        right: &str,
    ) -> DraftResult<Value> {
        let workspace = self.open(cwd)?;
        let left_id = parse_change_pack_id(left)?;
        let right_id = parse_change_pack_id(right)?;
        if left_id == right_id {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "a ChangePack does not interfere with itself",
            ));
        }

        let views = crate::app::workflow::change_pack_views(&workspace)?;
        let newest = |id: &draft_dcg_contract::ids::ChangePackId| {
            views
                .iter()
                .find(|view| &view.change_pack == id)
                .ok_or_else(|| {
                    DraftError::new(DraftErrorKind::NotFound, format!("no ChangePack '{id}'"))
                })
                .and_then(|view| {
                    view.revisions.first().cloned().ok_or_else(|| {
                        DraftError::new(
                            DraftErrorKind::NotFound,
                            format!(
                                "ChangePack {id} has sealed no revision, so what it touches is \
                                     not yet a fact"
                            ),
                        )
                        .with_suggestion("Seal a revision on both ChangePacks, then compare them.")
                    })
                })
        };
        let left_revision = newest(&left_id)?;
        let right_revision = newest(&right_id)?;

        // Where both sides recorded a representation, the neutral claim
        // algebra decides how they interfere; where either did not, it falls
        // back to whole-resource state. Either way the conservative direction
        // wins: two revisions that cannot be shown separable are reported as
        // interfering rather than assumed composable.
        let representations = representation_store(&workspace.layout);
        let left_bundle = representations.get(&left_revision.id)?;
        let right_bundle = representations.get(&right_revision.id)?;
        let interference = crate::evidence::representation::interference(
            &left_revision.touched,
            left_bundle.as_ref(),
            &right_revision.touched,
            right_bundle.as_ref(),
        );
        let shared: Vec<String> = interference
            .iter()
            .map(|finding| finding.resource_id.to_string())
            .collect();

        // Sealed against different Baselines, the two `touched` sets describe
        // work done from different starting points. Disjoint sets no longer
        // prove composability, so the answer is *indeterminate* rather than
        // yes — a distinction that fails closed.
        let same_base = left_revision.base_baseline == right_revision.base_baseline;
        let relation = if !shared.is_empty() {
            "conflicting"
        } else if same_base {
            "independent"
        } else {
            "indeterminate"
        };

        Ok(serde_json::json!({
            "left": {
                "change_pack_id": left_id.to_string(),
                "revision_pack_id": left_revision.id.to_string(),
                "base_baseline": left_revision.base_baseline.to_string(),
            },
            "right": {
                "change_pack_id": right_id.to_string(),
                "revision_pack_id": right_revision.id.to_string(),
                "base_baseline": right_revision.base_baseline.to_string(),
            },
            "relation": relation,
            "composable": relation == "independent",
            "shared_resources": shared,
            "interference": interference,
        }))
    }

    pub fn dcg_open_change_pack(
        &self,
        cwd: &Path,
        intent: &str,
        scope: &[String],
    ) -> DraftResult<crate::dcg::change_pack::ChangePack> {
        let workspace = self.open(cwd)?;
        let base = crate::dcg::baseline::current_baseline(&workspace.layout)?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                "this project accepts no baseline, so there is nothing to change from",
            )
        })?;

        let mut declared = BTreeSet::new();
        for value in scope {
            declared.insert(parse_scope_entry(value));
        }

        let change_pack_id = change_pack_id_for(&base.to_string(), intent)?;
        let actor = crate::app::baseline::actor_id_of(&workspace.layout)?;

        let definition = crate::dcg::definition::ChangePackDefinition {
            change_pack: change_pack_id.clone(),
            intent: intent.to_string(),
            scope_declaration: declared,
            created_by: actor.clone(),
            // Frozen at the epoch so the definition digest — and therefore the
            // ChangePack's identity — depends on what the change is, not on when
            // the command happened to run. Two identical requests are one
            // ChangePack; that is what makes the retry converge.
            created_at: draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
        };

        // What the ChangePack could legitimately land on: the accepted Baseline
        // plus what the project holds now. Resolution narrows the declaration
        // to these and may never exceed it.
        let (_, observed) = self.dcg_observe_state(&workspace)?;
        let resolvable = self.dcg_resolvable_resources(&workspace, observed.keys())?;
        let resolution = crate::dcg::definition::ScopeResolution::resolve(
            &definition,
            base,
            &resolvable,
            draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
        )?;

        let definitions = crate::dcg::definition::DefinitionStore::new(
            workspace.layout.definitions_dir(),
            workspace.layout.scope_resolutions_dir(),
        );
        let definition_digest = definitions.put_definition(&definition)?;
        definitions.put_resolution(&resolution)?;

        let store =
            crate::dcg::change_pack::ChangePackStore::new(workspace.layout.change_packs_dir());
        if let Some(existing) = store.read_unlocked(&change_pack_id)? {
            return Ok(existing);
        }
        let change = crate::dcg::change_pack::ChangePack {
            generation: 0,
            id: change_pack_id,
            project: workspace.workspace_id.clone(),
            current_definition: definition_digest,
            lifecycle: crate::dcg::change_pack::ChangePackLifecycle::Active,
        };
        store.create(&change)?;
        Ok(change)
    }

    /// Seal the workspace's current state as a revision of a ChangePack.
    ///
    /// The state root is observed rather than asserted, so a revision always
    /// says what the project actually looked like. Sealing the same state
    /// twice produces the same revision id and converges — a re-run after a
    /// dropped connection is not a second revision to review.
    pub fn dcg_seal(
        &self,
        cwd: &Path,
        change: &str,
    ) -> DraftResult<crate::dcg::revision_pack::RevisionPack> {
        let workspace = self.open(cwd)?;
        let change_pack_id = parse_change_pack_id(change)?;
        let store =
            crate::dcg::change_pack::ChangePackStore::new(workspace.layout.change_packs_dir());
        let record = store.read_unlocked(&change_pack_id)?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                format!("ChangePack '{change_pack_id}' does not exist"),
            )
        })?;
        if !record.lifecycle.accepts_work() {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "ChangePack '{change_pack_id}' is {:?}, so no further revision may be sealed against it",
                    record.lifecycle
                ),
            ));
        }

        let definitions = crate::dcg::definition::DefinitionStore::new(
            workspace.layout.definitions_dir(),
            workspace.layout.scope_resolutions_dir(),
        );
        let definition = definitions
            .definition(&record.current_definition)?
            .ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::NotFound,
                    format!("ChangePack '{change_pack_id}' names a definition that is not stored"),
                )
            })?;
        let base = crate::dcg::baseline::current_baseline(&workspace.layout)?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                "this project accepts no baseline to seal against",
            )
        })?;
        // Observed before the scope is resolved, because a ChangePack that adds a
        // Resource can only be bounded by what the project holds now. The full
        // run rather than the state alone: the representation recorded below
        // names the exact observations it read, and re-observing to derive it
        // would explain a different moment.
        let (outcome, snapshot) = self.dcg_observe_run(&workspace)?;
        let (state_root, _) = outcome.authoritative()?.build_roots()?;
        let observed: BTreeMap<
            draft_dcg_contract::ids::ResourceId,
            draft_dcg_contract::ResourceStateDigest,
        > = outcome
            .observations
            .iter()
            .map(|observation| (observation.resource.clone(), observation.state.clone()))
            .collect();
        let resolution = crate::dcg::definition::ScopeResolution::resolve(
            &definition,
            base.clone(),
            &self.dcg_resolvable_resources(&workspace, observed.keys())?,
            draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
        )?;
        definitions.put_resolution(&resolution)?;

        // What the revision actually changed, not merely what it may touch.
        // A resolution says where work was allowed to land; a reviewer reading
        // `touched` is being told what did land, and answering the first
        // question with the second would report every scoped file as edited.
        let accepted = crate::dcg::baseline::BaselineStore::new(workspace.layout.baselines_dir())
            .composition(&base)?
            .ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!("baseline '{base}' is accepted but its composition is not stored"),
                )
            })?;
        let touched: BTreeSet<_> = resolution
            .resources
            .iter()
            .filter(|resource| {
                // Absent from the observation is a deletion, and a state that
                // differs is an edit. Both are changes; equality is not.
                observed.get(*resource) != accepted.accepted_state_of(resource)
            })
            .cloned()
            .collect();
        // A revision proposes a change. Sealing a workspace that holds exactly
        // what the Baseline accepts would create something to verify, gate,
        // decide and promote that says nothing — and a promotion of it would
        // move the Baseline onto the state it already had.
        if touched.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "ChangePack '{change_pack_id}' has nothing to seal: within its scope the workspace \
                     holds exactly the state baseline '{base}' already accepts"
                ),
            )
            .with_suggestion(
                "Make the change this ChangePack declares, or widen its scope to the Resources you \
                 actually edited.",
            ));
        }

        let revision_id = revision_pack_id_for(&change_pack_id, &state_root.digest().to_string())?;
        let revision = crate::dcg::revision_pack::RevisionPack::seal(
            revision_id,
            &definition,
            &resolution,
            state_root,
            touched,
            crate::app::baseline::actor_id_of(&workspace.layout)?,
            draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
        )?;
        crate::dcg::revision_pack::RevisionPackStore::new(workspace.layout.revision_packs_dir())
            .put(&revision)?;

        // The explanation is derived from the same observations the revision
        // was sealed over, so it is about this revision and no other. Recorded
        // here rather than on demand because a later derivation would read a
        // workspace that has since moved — which is the whole reason Evidence
        // re-checks the state root before it records anything.
        self.record_representation(&workspace, &revision, &outcome, &snapshot, &observed)?;
        Ok(revision)
    }

    /// Derive and store the representation of a freshly sealed revision.
    fn record_representation(
        &self,
        workspace: &Workspace,
        revision: &crate::dcg::revision_pack::RevisionPack,
        outcome: &crate::dcg::observe::ObservationOutcome,
        snapshot: &Snapshot,
        observed: &BTreeMap<
            draft_dcg_contract::ids::ResourceId,
            draft_dcg_contract::ResourceStateDigest,
        >,
    ) -> DraftResult<()> {
        use draft_extension_contract::PresentationSurface;

        let mut observations = BTreeSet::new();
        for observation in &outcome.observations {
            observations.insert(observation.reference().map_err(|error| {
                DraftError::new(DraftErrorKind::CorruptData, error.to_string())
            })?);
        }

        let accepted = match crate::dcg::baseline::current_baseline(&workspace.layout)? {
            Some(baseline) => {
                crate::dcg::baseline::BaselineStore::new(workspace.layout.baselines_dir())
                    .composition(&baseline)?
                    .map(|composition| composition.accepted_state)
                    .unwrap_or_default()
            }
            None => BTreeMap::new(),
        };

        // Which contributed presentation, if any, claims each touched
        // Resource. Resolution is by specificity and ties are never arbitrated,
        // so an ambiguous Resource falls back to the neutral rendering exactly
        // as an unclaimed one does.
        let contributions = self.active_contributions();
        let classification =
            crate::evidence::classification::classify_snapshot(snapshot, &contributions);
        let classes = classification.by_resource();
        let mut strategies = BTreeMap::new();
        for state in &snapshot.resources {
            if !revision.touched.contains(&state.resource_id) {
                continue;
            }
            let view = crate::extension::ResourceView {
                locator_scheme: state.locator.scheme.as_str(),
                locator_body: state.locator.body.as_str(),
                media_type: state.media_type.as_deref(),
                form: state.form,
                attributes: &state.attributes,
                content_size: state.content_size,
            };
            let empty = BTreeSet::new();
            if let crate::extension::Resolution::Resolved {
                value,
                contributors,
            } = contributions.presentation_for(
                PresentationSurface::ChangeView,
                &view,
                classes.get(&state.resource_id).unwrap_or(&empty),
            ) {
                strategies.insert(
                    state.resource_id.clone(),
                    crate::app::representation::ResolvedStrategy {
                        strategy_id: value.presentation_id.clone(),
                        engine: format!("{:?}", value.engine),
                        contributed_by: contributors.first().map(|id| id.to_string()),
                    },
                );
            }
        }

        let bundle =
            crate::app::representation::derive(crate::app::representation::RepresentationInputs {
                revision,
                observations,
                observed,
                accepted: &accepted,
                strategies: &strategies,
                producer: dcg_producer("draft.core/representation")?,
            })?;
        crate::app::representation::record(&representation_store(&workspace.layout), &bundle)
    }

    /// Every piece of Evidence recorded about one exact revision.
    pub fn dcg_evidence_for(
        &self,
        cwd: &Path,
        revision: &str,
    ) -> DraftResult<Vec<crate::evidence::Evidence>> {
        let workspace = self.open(cwd)?;
        let revision = parse_revision_pack_id(revision)?;
        Ok(
            crate::app::authorization::AuthorizationStores::for_layout(&workspace.layout)
                .evidence
                .list()?
                .into_iter()
                .filter(|evidence| evidence.covers(&revision))
                .collect(),
        )
    }

    /// One piece of Evidence.
    pub fn dcg_evidence(
        &self,
        cwd: &Path,
        evidence: &str,
    ) -> DraftResult<Option<crate::evidence::Evidence>> {
        let workspace = self.open(cwd)?;
        let id = draft_dcg_contract::ids::EvidenceId::parse(evidence)
            .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?;
        crate::app::authorization::AuthorizationStores::for_layout(&workspace.layout)
            .evidence
            .get(&id)
    }

    /// Restate what a ChangePack is for.
    ///
    /// An intent lives in the ChangePack's definition, and a definition is an
    /// immutable fact — so this mints a new one and moves the ChangePack's pointer
    /// at it, rather than editing what a reviewer may already have read. The
    /// declared scope is carried across unchanged: amending an intent is not a
    /// way to widen what the work may touch.
    ///
    /// Any scope already resolved against the old definition is left stale by
    /// construction: sealing verifies the resolution against the definition in
    /// force, so a ChangePack amended after resolution is re-resolved rather than
    /// silently sealed under a boundary nobody approved.
    pub fn dcg_amend_intent(
        &self,
        cwd: &Path,
        change: &str,
        intent: &str,
    ) -> DraftResult<crate::dcg::definition::ChangePackDefinition> {
        let workspace = self.open(cwd)?;
        let change_pack_id = parse_change_pack_id(change)?;
        let store =
            crate::dcg::change_pack::ChangePackStore::new(workspace.layout.change_packs_dir());
        let record = store.read_unlocked(&change_pack_id)?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                format!("no ChangePack '{change}'"),
            )
        })?;
        let definitions = crate::dcg::definition::DefinitionStore::new(
            workspace.layout.definitions_dir(),
            workspace.layout.scope_resolutions_dir(),
        );
        let current = definitions
            .definition(&record.current_definition)?
            .ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::NotFound,
                    format!("ChangePack '{change_pack_id}' names a definition that is not stored"),
                )
            })?;
        if current.intent == intent {
            return Ok(current);
        }
        let amended = crate::dcg::definition::ChangePackDefinition {
            change_pack: change_pack_id.clone(),
            intent: intent.to_string(),
            scope_declaration: current.scope_declaration.clone(),
            created_by: crate::app::baseline::actor_id_of(&workspace.layout)?,
            created_at: draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
        };
        definitions.put_definition(&amended)?;
        let digest = amended.digest()?;
        let next = record.amend_definition(digest.clone())?;

        let activity = crate::app::activity::ProjectActivity::new(
            workspace.layout.clone(),
            &workspace.workspace_id,
        );
        let actor = crate::trust::identity::resolve_actor(&workspace.layout.draft_dir)?;
        let fact = crate::app::activity::DomainAuditFact::new(
            crate::activity::EventKind::ChangePackDefinitionAmended,
            actor.id.to_string(),
            draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
        )
        .about(change_pack_id.to_string())
        .with(serde_json::json!({ "definition": digest.to_string() }));
        let payload = crate::app::activity::payload_of(&fact);
        let transaction_id = format!("change-definition-{change_pack_id}-{}", next.generation);
        let event_id = crate::activity::log::event_id_for(&format!(
            "{transaction_id}|{}",
            crate::support::hashing::canonical_json(&payload)
        ));
        crate::app::activity::commit_audited_mutation(
            crate::app::activity::AuditedStores {
                records: store.records(),
                journals: &crate::support::mutation_journal::MutationJournalStore::new(
                    workspace.layout.journals_dir(),
                ),
                ledger: activity.log(),
            },
            change_pack_id.as_str(),
            &transaction_id,
            &crate::support::record_guard::ExpectedRecordState::of(&record)?,
            &next,
            crate::support::mutation_journal::AuditFactEnvelope {
                activity_event_id: event_id,
                payload,
            },
        )?;
        Ok(amended)
    }

    /// What this revision reaches, through contributed elements and relations.
    ///
    /// Extraction runs against the Resources the revision touched, and nothing
    /// is inferred: an element exists because an authorized extractor said so.
    /// Resources nothing can extract from are reported as such rather than
    /// silently contributing "no elements".
    pub fn dcg_impact(
        &self,
        cwd: &Path,
        revision: &str,
    ) -> DraftResult<crate::app::impact::ImpactReport> {
        let workspace = self.open(cwd)?;
        crate::app::impact::index_revision(self, &workspace, &parse_revision_pack_id(revision)?)
    }

    /// What the evidence about a revision actually speaks for.
    ///
    /// Conservative by construction. A Resource is covered only where evidence
    /// read an observation of that exact Resource; adjacency, directory layout
    /// and dependency produce no coverage at all.
    pub fn dcg_coverage(
        &self,
        cwd: &Path,
        revision: &str,
    ) -> DraftResult<crate::app::impact::CoverageReport> {
        let workspace = self.open(cwd)?;
        crate::app::impact::coverage_of(&workspace, &parse_revision_pack_id(revision)?)
    }

    /// Compose the newest sealed revisions of several ChangePacks.
    pub fn dcg_compose(
        &self,
        cwd: &Path,
        changes: &[String],
    ) -> DraftResult<crate::dcg::compose::Composition> {
        let workspace = self.open(cwd)?;
        let ids = changes
            .iter()
            .map(|value| parse_change_pack_id(value))
            .collect::<DraftResult<Vec<_>>>()?;
        crate::app::composition::compose(&workspace, &ids)
    }

    /// Take a composition apart into revisions that can move separately.
    pub fn dcg_disperse(
        &self,
        cwd: &Path,
        changes: &[String],
    ) -> DraftResult<Vec<crate::dcg::compose::DispersedRevision>> {
        let workspace = self.open(cwd)?;
        let ids = changes
            .iter()
            .map(|value| parse_change_pack_id(value))
            .collect::<DraftResult<Vec<_>>>()?;
        crate::app::composition::disperse(&workspace, &ids)
    }

    /// Every other ChangePack whose newest revision interferes with this one's.
    pub fn dcg_conflicts(
        &self,
        cwd: &Path,
        change: &str,
    ) -> DraftResult<Vec<crate::dcg::compose::PairwiseRelation>> {
        let workspace = self.open(cwd)?;
        crate::app::composition::conflicts(&workspace, &parse_change_pack_id(change)?)
    }

    /// What a ChangePack was built on: the Baseline it was sealed from, and the
    /// promotions that produced that Baseline's lineage.
    ///
    /// Lineage, not proximity. A ChangePack depends on the accepted history it was
    /// worked from; two ChangePacks touching neighbouring Resources depend on
    /// nothing of each other, and `conflicts` is the question that asks about
    /// them.
    pub fn dcg_depends(&self, cwd: &Path, change: &str) -> DraftResult<Value> {
        let workspace = self.open(cwd)?;
        let member = crate::app::composition::member(&workspace, &parse_change_pack_id(change)?)?;
        let baselines = crate::dcg::baseline::BaselineStore::new(workspace.layout.baselines_dir());
        let lineage = baselines.lineage(&member.base_baseline)?;
        let mut ancestry = Vec::new();
        for baseline in &lineage {
            let record = baselines.record(baseline)?;
            ancestry.push(serde_json::json!({
                "baseline": baseline.to_string(),
                "origin": record.map(|record| record.origin),
            }));
        }
        Ok(serde_json::json!({
            "change_pack_id": member.change_pack.to_string(),
            "revision_pack_id": member.revision_pack.to_string(),
            "base_baseline": member.base_baseline.to_string(),
            "lineage": ancestry,
        }))
    }

    /// Select the ChangePack subsequent commands default to.
    ///
    /// A convenience, never an authority: every command that acts still names
    /// the exact revision it acts on, and selecting one cannot widen what any
    /// of them may do.
    pub fn dcg_select_change_pack(&self, cwd: &Path, change: &str) -> DraftResult<String> {
        let workspace = self.open(cwd)?;
        let id = parse_change_pack_id(change)?;
        crate::dcg::change_pack::ChangePackStore::new(workspace.layout.change_packs_dir())
            .read_unlocked(&id)?
            .ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::NotFound,
                    format!("no ChangePack '{change}'"),
                )
            })?;
        crate::support::fsutil::write_atomic(
            &workspace.layout.selected_change_pack_file(),
            id.as_str().as_bytes(),
        )?;
        Ok(id.to_string())
    }

    /// Everything recorded about a ChangePack and its newest revision at once.
    ///
    /// `show` answers what the ChangePack is; this answers what has happened to
    /// it. Kept separate because the first is cheap and the second reads every
    /// judgement, explanation and conflict in the project.
    pub fn dcg_inspect(&self, cwd: &Path, change: &str) -> DraftResult<Value> {
        let workspace = self.open(cwd)?;
        let change_pack_id = parse_change_pack_id(change)?;
        let summary = self.dcg_change_pack(cwd, change)?;
        let newest = crate::app::workflow::change_pack_views(&workspace)?
            .into_iter()
            .find(|view| view.change_pack == change_pack_id)
            .and_then(|view| view.revisions.first().cloned());
        let (authorization, representation) = match &newest {
            Some(revision) => (
                Some(crate::app::workflow::authorization_view(
                    &workspace,
                    &change_pack_id,
                    &revision.id,
                )?),
                representation_store(&workspace.layout).get(&revision.id)?,
            ),
            None => (None, None),
        };
        Ok(serde_json::json!({
            "change_pack": summary,
            "newest_revision_pack_id": newest.as_ref().map(|revision| revision.id.to_string()),
            "authorization": authorization,
            "representation": representation,
            "conflicts": crate::app::composition::conflicts(&workspace, &change_pack_id)
                .unwrap_or_default(),
        }))
    }

    /// The receipts issued for a ChangePack's promotions.
    pub fn dcg_change_pack_receipts(&self, cwd: &Path, change: &str) -> DraftResult<Vec<Value>> {
        let workspace = self.open(cwd)?;
        let change_pack_id = parse_change_pack_id(change)?;
        let revisions: BTreeSet<String> = crate::app::workflow::change_pack_views(&workspace)?
            .into_iter()
            .find(|view| view.change_pack == change_pack_id)
            .map(|view| {
                view.revisions
                    .iter()
                    .map(|revision| revision.id.to_string())
                    .collect()
            })
            .unwrap_or_default();
        // A promotion receipt names the revision it accepted, so the ChangePack's
        // receipts are exactly those naming one of its revisions. Matching on
        // the ChangePack id instead would miss nothing today and quietly include
        // another ChangePack's work the moment a payload carried both.
        Ok(self
            .receipts(cwd)?
            .into_iter()
            .filter(|envelope| {
                let rendered = crate::support::hashing::canonical_json(envelope);
                revisions.iter().any(|revision| rendered.contains(revision))
            })
            .collect())
    }

    /// One Resource's accepted state, and what established it.
    pub fn dcg_resource_state(&self, cwd: &Path, resource: &str) -> DraftResult<Value> {
        let workspace = self.open(cwd)?;
        let resource_id = crate::dcg::resource::ResourceId::parse(resource)
            .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?;
        let Some(baseline) = crate::dcg::baseline::current_baseline(&workspace.layout)? else {
            return Err(DraftError::new(
                DraftErrorKind::NotFound,
                "this project accepts no baseline, so no Resource has an accepted state",
            ));
        };
        let composition =
            crate::dcg::baseline::BaselineStore::new(workspace.layout.baselines_dir())
                .composition(&baseline)?
                .ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::CorruptData,
                        format!("baseline '{baseline}' is accepted but its composition is missing"),
                    )
                })?;
        let accepted = composition.accepted_state.get(&resource_id);
        let provenance = composition.resource_provenance.get(&resource_id);
        if accepted.is_none() && provenance.is_none() {
            return Err(DraftError::new(
                DraftErrorKind::NotFound,
                format!("baseline '{baseline}' accepts no state for Resource '{resource_id}'"),
            ));
        }
        // What the workspace holds now, alongside what is accepted. A reader
        // shown only one of the two cannot tell whether the Resource has moved.
        let observed = self
            .dcg_observe_state(&workspace)
            .ok()
            .and_then(|(_, observed)| observed.get(&resource_id).cloned());
        Ok(serde_json::json!({
            "resource": resource_id.to_string(),
            "baseline": baseline.to_string(),
            "accepted_state": accepted.map(ToString::to_string),
            "accepted_provider_provenance": provenance,
            "matches_accepted": observed.as_ref() == accepted,
            "observed_state": observed.as_ref().map(ToString::to_string),
        }))
    }

    /// Every Baseline this project has accepted, newest first.
    pub fn dcg_baselines(&self, cwd: &Path) -> DraftResult<Vec<Value>> {
        let workspace = self.open(cwd)?;
        let store = crate::dcg::baseline::BaselineStore::new(workspace.layout.baselines_dir());
        let Some(current) = crate::dcg::baseline::current_baseline(&workspace.layout)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for baseline in store.lineage(&current)? {
            let record = store.record(&baseline)?;
            out.push(serde_json::json!({
                "baseline": baseline.to_string(),
                "accepted": baseline == current,
                "record": record,
            }));
        }
        Ok(out)
    }

    /// What one ChangePack is for, from the definition currently in force.
    pub fn dcg_change_pack_intent(
        &self,
        cwd: &Path,
        change: &str,
    ) -> DraftResult<crate::app::pack_detail::ChangePackIntentView> {
        let workspace = self.open(cwd)?;
        crate::app::pack_detail::intent(&workspace, &parse_change_pack_id(change)?)
    }

    /// What one ChangePack may touch, declared and resolved.
    pub fn dcg_change_pack_scope(
        &self,
        cwd: &Path,
        change: &str,
    ) -> DraftResult<crate::app::pack_detail::ChangePackScopeView> {
        let workspace = self.open(cwd)?;
        crate::app::pack_detail::scope(&workspace, &parse_change_pack_id(change)?)
    }

    /// Where an interrupted promotion of one ChangePack stands.
    pub fn dcg_change_pack_recovery(
        &self,
        cwd: &Path,
        change: &str,
    ) -> DraftResult<crate::app::pack_detail::ChangePackRecoveryView> {
        let workspace = self.open(cwd)?;
        crate::app::pack_detail::recovery(&workspace, &parse_change_pack_id(change)?)
    }

    /// Every Baseline in the accepted lineage, newest first, in full.
    pub fn dcg_baseline_details(
        &self,
        cwd: &Path,
    ) -> DraftResult<Vec<crate::app::baseline_detail::BaselineDetailView>> {
        crate::app::baseline_detail::list(&self.open(cwd)?)
    }

    /// One Baseline: its three roots, lineage, composition and deliveries.
    pub fn dcg_baseline_detail(
        &self,
        cwd: &Path,
        baseline: &str,
    ) -> DraftResult<crate::app::baseline_detail::BaselineDetailView> {
        let workspace = self.open(cwd)?;
        crate::app::baseline_detail::detail(&workspace, &parse_baseline_id(baseline)?)
    }

    /// Every binding, definition and profile this project holds.
    pub fn provider_catalog(
        &self,
        cwd: &Path,
    ) -> DraftResult<crate::app::provider::ProviderCatalogView> {
        crate::app::provider::catalog(&self.open(cwd)?)
    }

    /// Every provider binding this project has, withdrawn ones included.
    pub fn provider_list(&self, cwd: &Path) -> DraftResult<Vec<crate::app::provider::BindingView>> {
        crate::app::provider::list(&self.open(cwd)?)
    }

    /// One provider binding, with the immutable facts it points at.
    pub fn provider_show(
        &self,
        cwd: &Path,
        binding: &str,
    ) -> DraftResult<crate::app::provider::BindingView> {
        let workspace = self.open(cwd)?;
        crate::app::provider::show(&workspace, &parse_binding_id(binding)?)
    }

    /// Attach this project to a provider.
    pub fn provider_bind(
        &self,
        cwd: &Path,
        name: &str,
        contract: &draft_dcg_contract::semantics::ResourceStateSemanticsContract,
        definition: &crate::project::provider_definition::ProviderSemanticDefinition,
        profile: &crate::project::provider_definition::ProviderOperationalProfile,
    ) -> DraftResult<crate::project::provider::ProviderBinding> {
        let workspace = self.open(cwd)?;
        crate::app::provider::bind(&workspace, name, contract, definition, profile)
    }

    /// Point a binding at a different semantic definition.
    pub fn provider_redefine(
        &self,
        cwd: &Path,
        binding: &str,
        contract: &draft_dcg_contract::semantics::ResourceStateSemanticsContract,
        definition: &crate::project::provider_definition::ProviderSemanticDefinition,
    ) -> DraftResult<crate::project::provider::ProviderBinding> {
        let workspace = self.open(cwd)?;
        crate::app::provider::redefine(
            &workspace,
            &parse_binding_id(binding)?,
            contract,
            definition,
        )
    }

    /// Point a binding at a different operational profile.
    pub fn provider_profile(
        &self,
        cwd: &Path,
        binding: &str,
        profile: &crate::project::provider_definition::ProviderOperationalProfile,
    ) -> DraftResult<crate::project::provider::ProviderBinding> {
        let workspace = self.open(cwd)?;
        crate::app::provider::reprofile(&workspace, &parse_binding_id(binding)?, profile)
    }

    /// Withdraw a binding from new work. Deletes nothing.
    pub fn provider_unbind(
        &self,
        cwd: &Path,
        binding: &str,
    ) -> DraftResult<crate::project::provider::ProviderBinding> {
        let workspace = self.open(cwd)?;
        crate::app::provider::unbind(&workspace, &parse_binding_id(binding)?)
    }

    /// Reactivate a withdrawn binding.
    pub fn provider_rebind(
        &self,
        cwd: &Path,
        binding: &str,
    ) -> DraftResult<crate::project::provider::ProviderBinding> {
        let workspace = self.open(cwd)?;
        crate::app::provider::rebind(&workspace, &parse_binding_id(binding)?)
    }

    /// Every authority grant this project has issued, with its standing.
    pub fn authority_list(&self, cwd: &Path) -> DraftResult<Vec<crate::app::authority::GrantView>> {
        crate::app::authority::list(&self.open(cwd)?)
    }

    /// One authority grant, with its standing.
    pub fn authority_show(
        &self,
        cwd: &Path,
        grant: &str,
    ) -> DraftResult<crate::app::authority::GrantView> {
        let workspace = self.open(cwd)?;
        crate::app::authority::show(&workspace, &parse_grant_id(grant)?)
    }

    /// Withdraw an authority grant.
    pub fn authority_revoke(
        &self,
        cwd: &Path,
        grant: &str,
        reason: &str,
    ) -> DraftResult<crate::authority::revocation::AuthorityRevocation> {
        let workspace = self.open(cwd)?;
        crate::app::authority::revoke(&workspace, &parse_grant_id(grant)?, reason)
    }

    /// Record what was later established about an uncertain delivery.
    ///
    /// The primary outcome is never rewritten. This writes a Resolution beside
    /// it under current authority, because an interpretation is a new decision
    /// however old the outcome is.
    #[allow(clippy::too_many_arguments)]
    pub fn dcg_resolve_outcome(
        &self,
        cwd: &Path,
        purpose: &str,
        request_id: &str,
        outcome: Option<&str>,
        succeeded: Option<&str>,
        failed: Option<&str>,
        rationale: &str,
    ) -> DraftResult<String> {
        let workspace = self.open(cwd)?;
        let baseline =
            crate::dcg::baseline::current_baseline(&workspace.layout)?.ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::NotFound,
                    "this project accepts no baseline, so there is no delivery to resolve",
                )
            })?;
        let resolution =
            match (succeeded, failed) {
                (Some(reference), None) => {
                    draft_dcg_contract::publication::PublicationResolutionKind::ResolvedSucceeded {
                        external_reference: reference.to_string(),
                    }
                }
                (None, Some(reason)) => {
                    draft_dcg_contract::publication::PublicationResolutionKind::ResolvedFailed {
                        reason: reason.to_string(),
                    }
                }
                _ => return Err(DraftError::new(
                    DraftErrorKind::Validation,
                    "say which: --mark-succeeded <external-reference> or --mark-failed <reason>",
                )),
            };
        let request = crate::app::publish::PublishRequest {
            baseline,
            purpose: draft_dcg_contract::publication::PublicationPurposeId::parse(purpose)
                .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?,
            semantics: crate::app::publish::filesystem_delivery_semantics(),
            retry_authorization: None,
            republish_intent: None,
            request_id: request_id.to_string(),
        };
        let expected = match outcome {
            Some(value) => Some(
                draft_dcg_contract::publication::PublicationOutcomeDigest::new(
                    draft_dcg_contract::Digest::parse(value).map_err(|error| {
                        DraftError::new(DraftErrorKind::Validation, error.to_string())
                    })?,
                ),
            ),
            None => None,
        };
        let digest = crate::app::publish::resolve_outcome(
            &workspace,
            &request,
            expected.as_ref(),
            resolution,
            rationale,
        )?;
        Ok(digest.digest().to_string())
    }

    /// Deliver an already-published Baseline again, under a stated intent.
    ///
    /// A republish is a different Publication, not a retry of the old one: the
    /// intent is part of the request key, so the second delivery has its own
    /// identity, its own attempts and its own history. Retrying the *same*
    /// Publication is `publish retry`, and needs an authorization rather than
    /// an intent.
    pub fn dcg_republish(
        &self,
        cwd: &Path,
        baseline: Option<&str>,
        purpose: &str,
        intent: &str,
        request_id: &str,
    ) -> DraftResult<crate::app::publish::PublishOutcome> {
        let workspace = self.open(cwd)?;
        let baseline = match baseline {
            Some(value) => parse_baseline_id(value)?,
            None => {
                crate::dcg::baseline::current_baseline(&workspace.layout)?.ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::NotFound,
                        "this project accepts no baseline, so there is nothing to republish",
                    )
                })?
            }
        };
        let request = crate::app::publish::PublishRequest {
            baseline,
            purpose: draft_dcg_contract::publication::PublicationPurposeId::parse(purpose)
                .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?,
            semantics: crate::app::publish::filesystem_delivery_semantics(),
            retry_authorization: None,
            republish_intent: Some(
                draft_dcg_contract::publication::RepublishIntentId::parse(intent).map_err(
                    |error| DraftError::new(DraftErrorKind::Validation, error.to_string()),
                )?,
            ),
            request_id: request_id.to_string(),
        };
        let publication = crate::app::publish::ensure_publication(&workspace, &request)?;
        crate::app::publish::publish(&workspace, &request, || {
            crate::app::publish::deliver_to_filesystem(&workspace, &publication)
        })
    }

    /// The explanation recorded for one sealed revision, if any.
    pub fn dcg_representation(
        &self,
        cwd: &Path,
        revision: &str,
    ) -> DraftResult<Option<crate::evidence::representation::RevisionPackRepresentationBundle>>
    {
        let workspace = self.open(cwd)?;
        representation_store(&workspace.layout).get(&parse_revision_pack_id(revision)?)
    }

    /// Every explanation this project has recorded.
    pub fn dcg_representations(
        &self,
        cwd: &Path,
    ) -> DraftResult<Vec<crate::evidence::representation::RevisionPackRepresentationBundle>> {
        let workspace = self.open(cwd)?;
        representation_store(&workspace.layout).list()
    }

    /// Run the project's checks against a sealed revision and record Evidence.
    ///
    /// The workspace is observed and its state root compared against the
    /// revision's before anything is recorded. That is what makes the Evidence
    /// genuinely about *this* revision: checks that ran over different content
    /// say nothing about the work that was sealed, and would carry a judgement
    /// across an edit nobody reviewed.
    pub fn dcg_verify(&self, cwd: &Path, revision: &str) -> DraftResult<crate::evidence::Evidence> {
        let workspace = self.open(cwd)?;
        let revision_id = parse_revision_pack_id(revision)?;
        let sealed = crate::dcg::revision_pack::RevisionPackStore::new(
            workspace.layout.revision_packs_dir(),
        )
        .get(&revision_id)?
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                format!("RevisionPack '{revision_id}' has not been sealed"),
            )
        })?;

        let (observed, snapshot) = self.dcg_observe_run(&workspace)?;
        let (state_root, _) = observed.authoritative()?.build_roots()?;
        if state_root != sealed.project_state_root {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "the workspace no longer holds the state revision '{revision_id}' sealed;                      evidence gathered now would be about different content"
                ),
            )
            .with_suggestion("Seal a new revision, then verify that one."));
        }

        let mut inputs = BTreeSet::new();
        for observation in &observed.observations {
            inputs.insert(observation.reference().map_err(|error| {
                DraftError::new(DraftErrorKind::CorruptData, error.to_string())
            })?);
        }

        // The project's own checks, then every contributed check whose
        // predicate matches an observed resource. Both go through the same
        // runner and the same aggregation the rest of Draft uses — a second
        // runner here would be a second definition of what passing means.
        //
        // A check that cannot run is still selected and still reported, so a
        // missing capability can never shrink the required set.
        let configuration = read_or_default::<crate::evidence::verification::VerificationConfig>(
            &workspace.layout.verify_toml(),
        )?;
        let mut selected: Vec<crate::evidence::verification::SelectedCheck> = configuration
            .checks
            .iter()
            .filter(|check| check.enabled)
            .map(|check| {
                Ok(crate::evidence::verification::SelectedCheck {
                    check_id: crate::evidence::verification::project_check_id(&check.name)?,
                    display_name: check.name.clone(),
                    requirement: check.requirement,
                    selection: crate::extension::CheckSelection::Whole,
                    reason: "configured by this project".to_string(),
                    producer: crate::evidence::verification::project_producer(),
                    command: Some(check.command.clone()),
                    authorization_decision: Some(PROJECT_CONFIGURED_DECISION.to_string()),
                    ambiguous: false,
                })
            })
            .collect::<DraftResult<_>>()?;
        // What installed extensions contribute for the resources this revision
        // was sealed over. Without this, a project whose verification lives in
        // a language extension would get "nothing applied" and never learn its
        // checks had not run.
        let contributions = self.active_contributions();
        let classification =
            crate::evidence::classification::classify_snapshot(&snapshot, &contributions);
        let classes = classification.by_resource();
        let mut seen: BTreeSet<String> = selected
            .iter()
            .map(|check| check.check_id.qualified())
            .collect();
        let mut uncovered = Vec::new();
        for state in &snapshot.resources {
            let empty = BTreeSet::new();
            let resource_classes = classes.get(&state.resource_id).unwrap_or(&empty);
            let view = crate::extension::ResourceView {
                locator_scheme: state.locator.scheme.as_str(),
                locator_body: state.locator.body.as_str(),
                media_type: state.media_type.as_deref(),
                form: state.form,
                attributes: &state.attributes,
                content_size: state.content_size,
            };
            let applicable = contributions.checks_for(&view, resource_classes);
            if applicable.is_empty() {
                uncovered.push(state.locator.clone());
                continue;
            }
            for (check_id, contributed) in applicable {
                // `Whole` checks run once for the revision however many
                // resources match; `Exploratory` ones run only when asked for,
                // and nothing asks here.
                if matches!(
                    contributed.check.selection,
                    crate::extension::CheckSelection::Exploratory
                ) {
                    continue;
                }
                if !seen.insert(check_id.qualified()) {
                    continue;
                }
                let (command, decision) = self.authorized_command(
                    &contributions,
                    contributed.extension_id,
                    &contributed.check.operation,
                );
                selected.push(crate::evidence::verification::SelectedCheck {
                    check_id: check_id.clone(),
                    display_name: contributed.check.display_name.clone(),
                    requirement: contributed.check.requirement,
                    selection: contributed.check.selection,
                    reason: format!(
                        "contributed by {} for {}",
                        contributed.extension_id, state.locator.body
                    ),
                    producer: producer_ref_for(&contributions, contributed.extension_id),
                    command,
                    authorization_decision: decision,
                    ambiguous: contributed.ambiguous,
                });
            }
        }

        let run = crate::evidence::verification::run_checks(&selected, &workspace.root);
        let mut state = crate::evidence::verification::aggregate(&run.results);

        // "Draft asked and nothing applied" and "nothing existed to ask" are
        // different facts, and only one of them is fixed by installing
        // something. Both refuse the gate; only one tells the reader why.
        if matches!(
            state,
            crate::evidence::verification::VerificationState::NotApplicable { .. }
        ) && !uncovered.is_empty()
        {
            state = crate::evidence::verification::VerificationState::Unavailable {
                gaps: vec![crate::extension::CapabilityGap::new(
                    crate::extension::ExtensionCapabilityKind::Verification,
                    uncovered.iter().map(|l| l.body.clone()).collect::<Vec<_>>(),
                    "no installed extension contributes a check for these resources",
                )],
            };
        }

        // Each of the five verification states has one honest counterpart.
        // Collapsing "nothing could be asked" into a pass is the mistake this
        // mapping exists to make impossible.
        let outcome = if inputs.is_empty() {
            // Evidence that read nothing established nothing, whatever the
            // checks said.
            crate::evidence::EvidenceOutcome::Unavailable
        } else {
            match state {
                crate::evidence::verification::VerificationState::Passed => {
                    crate::evidence::EvidenceOutcome::Passed
                }
                crate::evidence::verification::VerificationState::Failed { .. } => {
                    crate::evidence::EvidenceOutcome::Failed
                }
                crate::evidence::verification::VerificationState::Unavailable { .. } => {
                    crate::evidence::EvidenceOutcome::Unavailable
                }
                crate::evidence::verification::VerificationState::NotEvaluated { .. } => {
                    crate::evidence::EvidenceOutcome::NotEvaluated
                }
                crate::evidence::verification::VerificationState::NotApplicable { .. } => {
                    crate::evidence::EvidenceOutcome::NotApplicable
                }
            }
        };

        let producer = dcg_producer("draft.core/verify")?;
        // The rules this ran under: the project's own `verify.toml` *and* the
        // checks that were actually selected. A contributed check is part of
        // the configuration as much as a configured one — hashing only the
        // file would let installing or removing an extension change which
        // checks ran while the digest said the rules had not moved, which is
        // the one thing this field exists to tell a reader.
        let mut rules = std::fs::read(workspace.layout.verify_toml()).unwrap_or_default();
        // Sorted, because a digest that depended on the order checks happened
        // to be selected in would move without the rules moving.
        let mut rule_lines: Vec<String> = selected
            .iter()
            .map(|check| format!("{} {:?}", check.check_id.qualified(), check.requirement))
            .collect();
        rule_lines.sort();
        for line in rule_lines {
            rules.push(b'\n');
            rules.extend_from_slice(line.as_bytes());
        }
        let configuration = draft_dcg_contract::Digest::of_bytes(&rules);

        // An immutable fact's identity is its content, and every field the
        // Evidence stores goes into it. Two verifications that read the same
        // observations under the same configuration and reached the same
        // answer are one fact, so a re-run converges on it. One that reached a
        // different answer — because a capability arrived, or the rules moved
        // — is a *different* fact, and gets its own identity rather than
        // trying to rewrite the first. Deriving the id from the revision alone
        // made every verification of a revision claim one identity, so the
        // second answer could only ever be refused as a rewrite.
        let mut identity = format!("{revision_id}|{producer:?}|{configuration}|{outcome:?}");
        for input in &inputs {
            identity.push('|');
            identity.push_str(&format!("{}:{}", input.id, input.digest));
        }

        let evidence = crate::evidence::Evidence {
            id: derived_id("evd_", &identity).and_then(|value| {
                draft_dcg_contract::ids::EvidenceId::parse(&value)
                    .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))
            })?,
            revision_pack: revision_id,
            inputs,
            producer,
            configuration,
            outcome,
            context: dcg_evaluation_context(),
        };
        crate::app::authorization::AuthorizationStores::for_layout(&workspace.layout)
            .evidence
            .put(&evidence)?;
        Ok(evidence)
    }

    /// Record a risk judgement over a revision's evidence.
    pub fn dcg_assess(
        &self,
        cwd: &Path,
        revision: &str,
        risk: &str,
        rationale: &str,
    ) -> DraftResult<crate::evidence::assessment::Assessment> {
        let workspace = self.open(cwd)?;
        let revision_id = parse_revision_pack_id(revision)?;
        let stores = crate::app::authorization::AuthorizationStores::for_layout(&workspace.layout);

        let inputs: BTreeSet<_> = stores
            .evidence
            .list()?
            .into_iter()
            .filter(|value| value.covers(&revision_id))
            .map(|value| value.id)
            .collect();

        let assessment = crate::evidence::assessment::Assessment {
            id: derived_id("asm_", &format!("{revision_id}|{risk}")).and_then(|value| {
                draft_dcg_contract::ids::AssessmentId::parse(&value)
                    .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))
            })?,
            revision_pack: revision_id,
            inputs,
            risk: parse_risk(risk)?,
            rationale: rationale.to_string(),
            producer: dcg_producer("draft.core/assess")?,
            configuration: draft_dcg_contract::Digest::of_bytes(
                std::fs::read(workspace.layout.risk_toml())
                    .unwrap_or_default()
                    .as_slice(),
            ),
            context: dcg_evaluation_context(),
        };
        crate::app::authorization::assess(&stores, &assessment)?;
        Ok(assessment)
    }

    /// Evaluate the project's gate over a revision.
    ///
    /// The requirement set is the project's, not the caller's. A caller that
    /// could choose which conditions applied would be choosing what counts as
    /// satisfied, which is the whole of what a gate decides.
    pub fn dcg_evaluate_gate(
        &self,
        cwd: &Path,
        revision: &str,
        waivers: &[String],
    ) -> DraftResult<crate::gate::GateEvaluation> {
        let workspace = self.open(cwd)?;
        let revision_id = parse_revision_pack_id(revision)?;
        let sealed = crate::dcg::revision_pack::RevisionPackStore::new(
            workspace.layout.revision_packs_dir(),
        )
        .get(&revision_id)?
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                format!("RevisionPack '{revision_id}' has not been sealed"),
            )
        })?;
        let stores = crate::app::authorization::AuthorizationStores::for_layout(&workspace.layout);

        let assessments: BTreeSet<_> = stores
            .assessments
            .list()?
            .into_iter()
            .filter(|value| value.covers(&revision_id))
            .map(|value| value.id)
            .collect();

        // The identity of an immutable fact is its content, and a gate's
        // content is the facts it reads. Deriving it from the revision alone
        // gave a revision exactly one evaluation for all time — so recording
        // an assessment and re-evaluating, which is the whole loop a person
        // runs, tried to rewrite the first evaluation and was refused. A
        // re-evaluation over the same facts still converges on the same
        // evaluation; one over new facts is a new fact.
        let mut identity = format!("{revision_id}|{}|{}", sealed.definition, sealed.scope);
        for assessment in &assessments {
            identity.push('|');
            identity.push_str(assessment.as_str());
        }
        for waiver in waivers {
            identity.push('|');
            identity.push_str(waiver);
        }

        let request = crate::app::authorization::GateRequest {
            id: derived_id("gate_", &identity)?,
            revision_pack: revision_id,
            // The exact definition and scope the revision was sealed against,
            // taken from the revision itself. A gate that named a different
            // pair would be evaluating a boundary nobody worked within.
            definition: sealed.definition.clone(),
            scope: sealed.scope.clone(),
            assessments,
            requirements: crate::app::authorization::GateRequirements {
                required: vec![(
                    "draft.gate/verified".to_string(),
                    draft_dcg_contract::Digest::of_bytes(b"draft.gate/verified.v1"),
                )],
                max_risk: crate::evidence::assessment::AssessedRisk::Medium,
            },
            waivers: waivers.iter().cloned().collect(),
            context: dcg_evaluation_context(),
        };
        crate::app::authorization::evaluate_gate(&stores, &request)
    }

    /// Grant this project's actor authority to publish from it.
    ///
    /// Publishing is its own capability. Somebody permitted to accept work
    /// into a Baseline has not thereby been permitted to announce it to the
    /// outside world, and collapsing the two would make every approver an
    /// unwitting publisher — so the grant is issued explicitly and recorded as
    /// a fact the attempt can cite.
    ///
    /// Granted over the project, not over an individual Publication: a
    /// Publication's id is derived from what it delivers, so a per-Publication
    /// grant would have to be re-issued for every send and nobody would read
    /// what they were approving.
    ///
    /// Idempotent. The grant id is derived from the actor and the project, so
    /// re-running converges on the grant already issued rather than minting a
    /// second one that says the same thing.
    pub fn dcg_grant_publish(
        &self,
        cwd: &Path,
    ) -> DraftResult<crate::authority::grant::AuthorityGrant> {
        self.authority_grant(cwd, "draft.publish/v1", None)
    }

    /// Grant one capability over this project.
    ///
    /// The capability is named rather than assumed. Publishing and operating
    /// are different permissions, and a grant that did not say which would
    /// make every reader of the record guess what was permitted.
    ///
    /// Reserved `draft.*` capabilities are implemented, never minted: one this
    /// build does not recognise is refused here rather than recorded as a
    /// permission nothing will ever check.
    ///
    /// Idempotent. The grant id is derived from the capability, the project
    /// and the grantee, so re-running converges on the grant already issued
    /// rather than minting a second one that says the same thing.
    pub fn authority_grant(
        &self,
        cwd: &Path,
        capability: &str,
        grantee: Option<&str>,
    ) -> DraftResult<crate::authority::grant::AuthorityGrant> {
        let workspace = self.open(cwd)?;
        let granter = crate::app::baseline::actor_id_of(&workspace.layout)?;
        let actor = match grantee {
            Some(value) => draft_dcg_contract::ids::ActorId::parse(value)
                .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?,
            None => granter.clone(),
        };
        let capability = draft_dcg_contract::capability::CapabilityId::parse(capability)
            .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?;
        if !capability.is_acceptable() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "'{capability}' is in Draft's reserved namespace but is not a capability this                      build implements; granting it would record a permission nothing checks"
                ),
            ));
        }
        let subject = crate::publication::authority::publish_subject(&workspace.workspace_id)?;
        let stores = crate::publication::authority::AuthorityStores::for_layout(&workspace.layout)?;

        let grant = crate::authority::grant::AuthorityGrant {
            id: derived_id(
                "auth_",
                &format!("{capability}|{}|{actor}", workspace.workspace_id),
            )
            .and_then(|value| {
                draft_dcg_contract::ids::AuthorityGrantId::parse(&value)
                    .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))
            })?,
            grantee: actor.clone(),
            capability,
            subject,
            granted_by: granter,
            // Frozen, so the grant's identity depends on who may do what to
            // which project rather than on when the command ran. That is what
            // makes re-running converge instead of issuing a second grant.
            granted_at: draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
            // Deliberately explicit rather than a missing field: a permission
            // that outlives every deadline is a decision somebody made.
            expires_at: None,
        };
        stores.grants.put(&grant)?;
        let reference = grant.reference()?;

        // Adopt it into the project's security state, under the control lock
        // and against the generation the read observed. A grant sitting in the
        // store that the state does not name confers nothing — which is what
        // makes "issued" and "in force" separate, checkable facts.
        stores.control.with_locked_control(|control| {
            let current = control.current()?.ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::NotFound,
                    "this project has no control record to grant against",
                )
            })?;
            let security = stores
                .security_states
                .get(&current.project_security_state)?
                .unwrap_or_default();
            if security.is_active(&reference) {
                return Ok(());
            }

            let granted = security.grant(reference.clone());
            crate::app::security::structurally_valid(&granted)?;
            // Every reference re-resolved and re-verified before it is
            // committed: a state naming a fact that does not resolve would
            // make the project's authority unreadable at the next dispatch.
            let resolvers = crate::app::security::SecurityResolvers {
                grants: &stores.grants,
                revocations: &stores.revocations,
            };
            let failures = crate::app::security::unresolved(&resolvers, &granted)?;
            if let Some((reference, resolution)) = failures.first() {
                return Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "security fact {} would be committed but resolves as {resolution:?}",
                        reference.digest
                    ),
                ));
            }

            let digest = stores.security_states.put(&granted)?;
            let expected = crate::support::record_guard::ExpectedRecordState::of(&current)?;
            let advanced = current.advanced(|next| {
                next.project_security_state = digest;
            });
            control.compare_exchange_locked(&expected, &advanced)
        })?;
        Ok(grant)
    }

    /// Grant an exception to one gate condition on one exact revision.
    ///
    /// Bound to the revision, not the ChangePack: an exception accepted for the
    /// work as it stood is not an exception for whatever it becomes. The gate
    /// records a waived condition as satisfied *and names the waiver*, so
    /// "somebody allowed this" never reads as "this passed".
    pub fn dcg_waive(
        &self,
        cwd: &Path,
        revision: &str,
        condition: &str,
        reason: &str,
        expires_in_days: u32,
    ) -> DraftResult<crate::gate::waiver::GateWaiver> {
        if expires_in_days == 0 {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "a waiver that expires immediately waives nothing",
            ));
        }
        let workspace = self.open(cwd)?;
        let revision_id = parse_revision_pack_id(revision)?;
        // Granted at the same instant every other DCG fact is evaluated at, so
        // a waiver and the gate that reads it agree about what "now" is.
        let waived_at = dcg_evaluation_context().evaluated_at;
        let waiver = crate::gate::waiver::GateWaiver {
            id: derived_id("wvr_", &format!("{revision_id}|{condition}"))?,
            revision_pack: revision_id,
            condition: condition.to_string(),
            reason: reason.to_string(),
            waived_by: crate::app::baseline::actor_id_of(&workspace.layout)?,
            waived_at,
            expires_at: draft_dcg_contract::value::Timestamp::from_unix_nanos(
                waived_at.as_unix_nanos()
                    + i64::from(expires_in_days) * 24 * 60 * 60 * 1_000_000_000,
            ),
            authority: project_decision_authority(&workspace)?,
        };
        crate::app::authorization::AuthorizationStores::for_layout(&workspace.layout)
            .waivers
            .put(&waiver)?;
        Ok(waiver)
    }

    /// The Resources the accepted Baseline holds.
    /// Every Resource a ChangePack opened now could legitimately be scoped to.
    ///
    /// The accepted Baseline and the observed project, together. A Resource in
    /// the Baseline may be edited or deleted; one only in the workspace may be
    /// introduced. Excluding the second would make adding a file
    /// unrepresentable — the declaration would resolve to nothing and the work
    /// would fall outside every scope a reviewer reads.
    fn dcg_resolvable_resources<'a>(
        &self,
        workspace: &Workspace,
        observed: impl IntoIterator<Item = &'a draft_dcg_contract::ids::ResourceId>,
    ) -> DraftResult<BTreeSet<draft_dcg_contract::ids::ResourceId>> {
        let mut resources = self.dcg_accepted_resources(workspace)?;
        resources.extend(observed.into_iter().cloned());
        Ok(resources)
    }

    fn dcg_accepted_resources(
        &self,
        workspace: &Workspace,
    ) -> DraftResult<BTreeSet<draft_dcg_contract::ids::ResourceId>> {
        let Some(baseline) = crate::dcg::baseline::current_baseline(&workspace.layout)? else {
            return Ok(BTreeSet::new());
        };
        let store = crate::dcg::baseline::BaselineStore::new(workspace.layout.baselines_dir());
        Ok(store
            .composition(&baseline)?
            .map(|value| value.resource_provenance.keys().cloned().collect())
            .unwrap_or_default())
    }

    /// Observe the workspace and canonicalize it into an observation run.
    fn dcg_observe_run(
        &self,
        workspace: &Workspace,
    ) -> DraftResult<(crate::dcg::observe::ObservationOutcome, Snapshot)> {
        let stores = crate::app::baseline::AcceptanceStores::for_layout(&workspace.layout);
        let binding =
            crate::app::baseline::ensure_filesystem_binding(&stores, &workspace.workspace_id)?;
        let (snapshot, context) = self.observe_for_acceptance(workspace)?;
        let observed_at = draft_dcg_contract::value::Timestamp::from_unix_nanos(
            snapshot
                .created_at
                .timestamp_nanos_opt()
                .unwrap_or_default(),
        );
        let enumeration = crate::dcg::accept::canonicalize(&snapshot, observed_at, observed_at)?;
        let observer = crate::dcg::observe::ObservingBinding {
            binding: binding.id.clone(),
            semantic_definition: binding.current_semantic_definition.clone(),
            producer: dcg_producer("draft.core/filesystem-observer")?,
            observation_context: draft_dcg_contract::Digest::of_bytes(context.as_bytes()),
        };
        let outcome = crate::dcg::observe::record(&observer, &enumeration)?;
        // Durable before anything cites them: evidence may not name an
        // observation that does not exist.
        stores.observations.put_run(&outcome.run)?;
        for observation in &outcome.observations {
            stores.observations.put(observation)?;
        }
        Ok((outcome, snapshot))
    }

    /// The state root the workspace would produce now, and what it holds.
    fn dcg_observe_state(
        &self,
        workspace: &Workspace,
    ) -> DraftResult<(
        draft_dcg_contract::roots::ProjectStateRoot,
        BTreeMap<draft_dcg_contract::ids::ResourceId, draft_dcg_contract::ResourceStateDigest>,
    )> {
        let (outcome, _) = self.dcg_observe_run(workspace)?;
        let authoritative = outcome.authoritative()?;
        let (state_root, _) = authoritative.build_roots()?;
        let observed = outcome
            .observations
            .iter()
            .map(|observation| (observation.resource.clone(), observation.state.clone()))
            .collect();
        Ok((state_root, observed))
    }

    /// The project's whole workflow state, with server-computed availability.
    pub fn dcg_project(
        &self,
        cwd: &Path,
    ) -> DraftResult<crate::app::workflow::ProjectWorkflowView> {
        crate::app::workflow::project_view(&self.open(cwd)?)
    }

    /// The Baseline the project currently accepts.
    pub fn dcg_baseline(
        &self,
        cwd: &Path,
    ) -> DraftResult<Option<crate::app::workflow::BaselineView>> {
        crate::app::workflow::baseline_view(&self.open(cwd)?)
    }

    /// Every ChangePack and the revisions sealed against it.
    pub fn dcg_change_packs(
        &self,
        cwd: &Path,
    ) -> DraftResult<Vec<crate::app::workflow::ChangePackView>> {
        crate::app::workflow::change_pack_views(&self.open(cwd)?)
    }

    /// Everything decided about one revision, and what may legally follow.
    pub fn dcg_authorization(
        &self,
        cwd: &Path,
        change: &str,
        revision: &str,
    ) -> DraftResult<crate::app::workflow::AuthorizationView> {
        let workspace = self.open(cwd)?;
        crate::app::workflow::authorization_view(
            &workspace,
            &parse_change_pack_id(change)?,
            &parse_revision_pack_id(revision)?,
        )
    }

    /// Record an immutable Decision about a revision.
    ///
    /// The gate is resolved and handed to the domain rather than re-checked
    /// here: whether an approval may be made over it is `app::authorization`'s
    /// judgement, and repeating it would be a second rule to keep in step.
    /// Record that somebody examined one exact revision.
    ///
    /// A Review is not a Decision and does not authorize anything. It records
    /// the act of looking, which is what makes "under review" representable at
    /// all — and what makes an approval with no review behind it visible.
    ///
    /// Derived from the revision and reviewer, so re-recording the same review
    /// converges instead of accumulating one entry per invocation.
    pub fn dcg_review(
        &self,
        cwd: &Path,
        revision: &str,
        comments: &[String],
    ) -> DraftResult<crate::dcg::review::Review> {
        let workspace = self.open(cwd)?;
        let revision = parse_revision_pack_id(revision)?;
        let reviewer = crate::app::baseline::actor_id_of(&workspace.layout)?;
        let now = crate::support::clock::Clock::now(&crate::support::clock::SystemClock);

        let stores = crate::app::authorization::AuthorizationStores::for_layout(&workspace.layout);
        let id = derived_id("rvw_", &format!("{revision}|{reviewer}"))?;
        let id = draft_dcg_contract::ids::ReviewId::parse(id)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;

        // The store is create-once, so a second review by the same reviewer of
        // the same revision must be byte-identical. Merging the new comments
        // into the existing record is what makes that true, rather than
        // refusing a reviewer who wrote a second note.
        let existing = stores.reviews.get(&id)?;
        let started_at = existing.as_ref().map_or(now, |review| review.started_at);
        let mut all: Vec<crate::dcg::review::ReviewComment> =
            existing.map(|review| review.comments).unwrap_or_default();
        for body in comments {
            if all.iter().any(|comment| comment.body == *body) {
                continue;
            }
            all.push(crate::dcg::review::ReviewComment {
                author: reviewer.clone(),
                body: body.clone(),
                written_at: now,
            });
        }

        let review = crate::dcg::review::Review {
            id,
            revision_pack: revision,
            reviewer,
            started_at,
            comments: all,
        };
        stores.reviews.put(&review)?;
        Ok(review)
    }

    pub fn dcg_decide(
        &self,
        cwd: &Path,
        revision: &str,
        gate: Option<&str>,
        approve: bool,
        reason: Option<&str>,
    ) -> DraftResult<crate::dcg::decision::Decision> {
        let workspace = self.open(cwd)?;
        let revision = parse_revision_pack_id(revision)?;
        let stores = crate::app::authorization::AuthorizationStores::for_layout(&workspace.layout);

        let evaluation = match gate {
            Some(id) => stores.gates.get(id)?,
            None => stores
                .gates
                .list()?
                .into_iter()
                .filter(|value| value.covers(&revision))
                .find(crate::gate::GateEvaluation::is_satisfied),
        };

        let outcome = if approve {
            crate::dcg::decision::DecisionOutcome::Approved
        } else {
            crate::dcg::decision::DecisionOutcome::Rejected {
                reason: reason
                    .unwrap_or("rejected without a stated reason")
                    .to_string(),
            }
        };
        let request = crate::app::authorization::DecisionRequest {
            id: decision_id_for(&revision, approve)?,
            revision_pack: revision,
            outcome,
            decided_by: crate::app::baseline::actor_id_of(&workspace.layout)?,
            decided_at: crate::support::clock::Clock::now(&crate::support::clock::SystemClock),
            // The grant this project decides under. One fact, cited by the
            // decision it authorizes: an approval resting on nothing is not a
            // permission, and the store refuses one.
            authority: [project_decision_authority(&workspace)?]
                .into_iter()
                .collect(),
        };
        crate::app::authorization::decide(&stores, &request, evaluation.as_ref())
    }

    /// Promote an authorized revision, advancing the project's Baseline.
    ///
    /// `expected_parent` states the Baseline the caller believed was accepted.
    /// A promotion decided against state that has since moved is refused
    /// rather than rebased, which is what makes a stale surface action fail
    /// safely instead of silently accepting work onto a parent nobody judged
    /// it against.
    pub fn dcg_promote(
        &self,
        cwd: &Path,
        change: &str,
        revision: &str,
        decision: &str,
        gate: &str,
        expected_parent: Option<&str>,
    ) -> DraftResult<crate::app::promotion::PromotionOutcome> {
        let workspace = self.open(cwd)?;
        let expected_parent = match expected_parent {
            Some(value) => Some(parse_baseline_id(value)?),
            None => None,
        };
        let request = crate::app::promotion::PromotionRequest {
            change_pack: parse_change_pack_id(change)?,
            revision_pack: parse_revision_pack_id(revision)?,
            decision: parse_decision_id(decision)?,
            gate: gate.to_string(),
            expected_parent,
        };
        crate::app::promotion::promote(self, &workspace, &request)
    }

    /// One promotion, projected from its own durable records.
    pub fn dcg_promotion(
        &self,
        cwd: &Path,
        promotion: &str,
    ) -> DraftResult<Option<crate::app::workflow::PromotionView>> {
        let workspace = self.open(cwd)?;
        crate::app::workflow::promotion_view(&workspace, &parse_promotion_id(promotion)?)
    }

    /// Publish a promoted Baseline through the publication engine.
    ///
    /// `request_id` is the caller's identity for this attempt. Retrying under
    /// the same id converges on what that attempt concluded; it is the reason
    /// an HTTP retry or a re-run command cannot deliver twice.
    ///
    /// Neither the recovery class nor the delivery semantics are parameters.
    /// The semantics are declared by the binding the route names, and the
    /// recovery class is derived from those — a caller that could assert
    /// either would be choosing what happens after a crash it cannot see.
    pub fn dcg_publish(
        &self,
        cwd: &Path,
        baseline: Option<&str>,
        purpose: &str,
        request_id: &str,
        retry_authorization: Option<&str>,
    ) -> DraftResult<crate::app::publish::PublishOutcome> {
        let workspace = self.open(cwd)?;
        let baseline = match baseline {
            Some(value) => parse_baseline_id(value)?,
            None => {
                crate::dcg::baseline::current_baseline(&workspace.layout)?.ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::NotFound,
                        "this project accepts no baseline, so there is nothing to publish",
                    )
                })?
            }
        };
        let request = crate::app::publish::PublishRequest {
            baseline,
            purpose: draft_dcg_contract::publication::PublicationPurposeId::parse(purpose)
                .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?,
            semantics: crate::app::publish::filesystem_delivery_semantics(),
            retry_authorization: match retry_authorization {
                Some(value) => Some(
                    draft_dcg_contract::publication::PublicationRetryAuthorizationDigest::new(
                        draft_dcg_contract::Digest::parse(value).map_err(|error| {
                            DraftError::new(DraftErrorKind::Validation, error.to_string())
                        })?,
                    ),
                ),
                None => None,
            },
            republish_intent: None,
            request_id: request_id.to_string(),
        };
        let publication = crate::app::publish::ensure_publication(&workspace, &request)?;
        crate::app::publish::publish(&workspace, &request, || {
            crate::app::publish::deliver_to_filesystem(&workspace, &publication)
        })
    }

    /// Authorize another attempt at a delivery Draft could not establish.
    ///
    /// A delivery that ended indeterminately against a target whose semantics
    /// cannot rule out duplication is stuck on purpose. This is the decision
    /// that unsticks it, recorded with who made it and what they knew — and it
    /// permits exactly one further attempt.
    pub fn dcg_authorize_retry(
        &self,
        cwd: &Path,
        purpose: &str,
        request_id: &str,
        rationale: &str,
    ) -> DraftResult<String> {
        let workspace = self.open(cwd)?;
        let baseline =
            crate::dcg::baseline::current_baseline(&workspace.layout)?.ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::NotFound,
                    "this project accepts no baseline, so there is nothing to retry publishing",
                )
            })?;
        let request = crate::app::publish::PublishRequest {
            baseline,
            purpose: draft_dcg_contract::publication::PublicationPurposeId::parse(purpose)
                .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?,
            semantics: crate::app::publish::filesystem_delivery_semantics(),
            retry_authorization: None,
            republish_intent: None,
            request_id: request_id.to_string(),
        };
        let digest = crate::app::publish::authorize_retry(&workspace, &request, rationale)?;
        Ok(digest.digest().to_string())
    }

    /// Withdraw an allocated attempt whose dispatch is no longer permitted.
    ///
    /// An attempt interrupted between its allocation and its dispatch boundary
    /// blocks its Publication forever: the barrier refuses a new one because it
    /// cannot prove the first caused no effect. This proves it — the journal
    /// never reached the boundary, so no external call was made — and releases
    /// the Publication. An attempt past the boundary is refused, because
    /// withdrawing it would assert something Draft cannot know.
    pub fn dcg_withdraw_attempt(
        &self,
        cwd: &Path,
        purpose: &str,
        request_id: &str,
        reason: &str,
    ) -> DraftResult<crate::publication::abandon::Withdrawn> {
        let workspace = self.open(cwd)?;
        let baseline =
            crate::dcg::baseline::current_baseline(&workspace.layout)?.ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::NotFound,
                    "this project accepts no baseline, so there is no attempt to withdraw",
                )
            })?;
        let request = crate::app::publish::PublishRequest {
            baseline,
            purpose: draft_dcg_contract::publication::PublicationPurposeId::parse(purpose)
                .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?,
            semantics: crate::app::publish::filesystem_delivery_semantics(),
            retry_authorization: None,
            republish_intent: None,
            request_id: request_id.to_string(),
        };
        crate::app::publish::withdraw_stalled_attempt(&workspace, &request, reason)
    }

    /// Every Publication, with the engine's own view of where each one is.
    pub fn dcg_publications(
        &self,
        cwd: &Path,
    ) -> DraftResult<Vec<crate::app::workflow::PublicationView>> {
        crate::app::workflow::publication_views(&self.open(cwd)?)
    }
}

fn parse_change_pack_id(value: &str) -> DraftResult<draft_dcg_contract::ids::ChangePackId> {
    draft_dcg_contract::ids::ChangePackId::parse(value)
        .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))
}

fn parse_revision_pack_id(value: &str) -> DraftResult<draft_dcg_contract::ids::RevisionPackId> {
    draft_dcg_contract::ids::RevisionPackId::parse(value)
        .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))
}

fn parse_decision_id(value: &str) -> DraftResult<draft_dcg_contract::ids::DecisionId> {
    draft_dcg_contract::ids::DecisionId::parse(value)
        .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))
}

fn parse_binding_id(value: &str) -> DraftResult<draft_dcg_contract::ids::ProviderBindingId> {
    draft_dcg_contract::ids::ProviderBindingId::parse(value)
        .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))
}

fn parse_grant_id(value: &str) -> DraftResult<draft_dcg_contract::ids::AuthorityGrantId> {
    draft_dcg_contract::ids::AuthorityGrantId::parse(value)
        .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))
}

fn parse_promotion_id(value: &str) -> DraftResult<draft_dcg_contract::ids::PromotionId> {
    draft_dcg_contract::ids::PromotionId::parse(value)
        .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))
}

fn parse_baseline_id(value: &str) -> DraftResult<draft_dcg_contract::baseline::BaselineId> {
    draft_dcg_contract::Digest::parse(value)
        .map(draft_dcg_contract::baseline::BaselineId::new)
        .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))
}

/// An id derived from what it names, so a retry recomputes it.
///
/// Every id a surface can cause to be minted goes through here. A randomly
/// generated one would make a dropped connection produce a second immutable
/// record of the same fact, which create-once storage would then be unable to
/// tell from a genuine second fact.
/// A ChangePack's identity: derived from the Baseline it was opened against and
/// its intent, so re-running the same request converges on the same ChangePack.
fn change_pack_id_for(
    base_baseline: &str,
    intent: &str,
) -> DraftResult<draft_dcg_contract::ids::ChangePackId> {
    derived_id(
        draft_dcg_contract::ids::ChangePackId::PREFIX,
        &format!("{base_baseline}|{intent}"),
    )
    .and_then(|value| parse_change_pack_id(&value))
}

/// A RevisionPack's identity: its ChangePack and the proposed state root, and
/// nothing else — not the definition, scope, actor or time.
fn revision_pack_id_for(
    change_pack: &draft_dcg_contract::ids::ChangePackId,
    project_state_root: &str,
) -> DraftResult<draft_dcg_contract::ids::RevisionPackId> {
    derived_id(
        draft_dcg_contract::ids::RevisionPackId::PREFIX,
        &format!("{change_pack}|{project_state_root}"),
    )
    .and_then(|value| parse_revision_pack_id(&value))
}

fn derived_id(prefix: &str, seed: &str) -> DraftResult<String> {
    let digest = draft_dcg_contract::Digest::of_bytes(seed.as_bytes());
    let short: String = digest
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect();
    Ok(format!("{prefix}{short}"))
}

fn dcg_producer(name: &str) -> DraftResult<draft_dcg_contract::producer::ProducerIdentity> {
    draft_dcg_contract::producer::ProducerIdentity::new(
        draft_dcg_contract::identifier::NamespacedId::parse(name)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
        crate::DRAFT_VERSION,
    )
    .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

/// The context every DCG evaluation in this process records.
///
/// Frozen at the epoch, and deliberately so: evidence, assessments and gates
/// are content-addressed facts, and stamping wall-clock time into them would
/// make the same judgement over the same content a different fact each run —
/// which create-once storage would refuse as a conflicting rewrite.
///
/// Waivers are the one thing that reads this as a real instant, and a waiver
/// evaluated against the epoch has simply not expired yet.
/// One entry of a declared ChangePack scope.
///
/// A canonical Resource id when the caller has one, and otherwise a locator —
/// `file:src/auth.rs`, or just `src/auth.rs` for the filesystem. The locator
/// form is not a convenience: a Resource the project does not hold yet has no
/// id to look up, so a ChangePack that introduces one could not be declared at all
/// without it. Both forms derive the same id for the same locator.
fn parse_scope_entry(value: &str) -> draft_dcg_contract::ids::ResourceId {
    if let Ok(resource) = draft_dcg_contract::ids::ResourceId::parse(value) {
        return resource;
    }
    let locator = match value.split_once(':') {
        Some((scheme, _)) if !scheme.is_empty() && !scheme.contains('/') => value.to_string(),
        _ => format!("file:{value}"),
    };
    crate::dcg::resource::resource_id_for_locator(&locator)
}

fn dcg_evaluation_context() -> crate::evidence::context::EvaluationContext {
    crate::evidence::context::EvaluationContext {
        evaluated_at: draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
        clock_source: draft_dcg_contract::identifier::NamespacedId::parse("draft.core/fixed-clock")
            .expect("a frozen literal is valid"),
        policy_digest: draft_dcg_contract::security::PolicyDigest::new(
            draft_dcg_contract::Digest::of_bytes(b"draft.core/default-policy"),
        ),
        security_context_digest: draft_dcg_contract::security::SecurityContextDigest::new(
            draft_dcg_contract::Digest::of_bytes(b"draft.core/default-security-context"),
        ),
        core_evaluator_revision: crate::DRAFT_VERSION.to_string(),
    }
}

fn parse_risk(value: &str) -> DraftResult<crate::evidence::assessment::AssessedRisk> {
    use crate::evidence::assessment::AssessedRisk;
    match value.to_ascii_lowercase().as_str() {
        "low" => Ok(AssessedRisk::Low),
        "medium" => Ok(AssessedRisk::Medium),
        "high" => Ok(AssessedRisk::High),
        "critical" => Ok(AssessedRisk::Critical),
        // Never parsed from input. "Nobody looked" is a state Draft reaches by
        // not being told, not one a caller asserts.
        other => Err(DraftError::new(
            DraftErrorKind::Validation,
            format!("'{other}' is not a risk level; use low, medium, high or critical"),
        )),
    }
}

/// The authority a decision in this project is made under.
///
/// Derived from the project so it is stable across calls, and named as a
/// grant so a reader can see what an approval rested on. It is a real fact
/// about this project rather than a constant, because a decision citing a
/// grant that means nothing would satisfy the letter of "cite your authority"
/// while defeating the point of it.
fn project_decision_authority(
    workspace: &Workspace,
) -> DraftResult<draft_dcg_contract::security::SecurityFactRef> {
    Ok(draft_dcg_contract::security::SecurityFactRef::new(
        draft_dcg_contract::security::SecurityControlKindId::parse(
            "draft.security/authority-grant.v1",
        )
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
        None,
        draft_dcg_contract::Digest::of_bytes(
            format!(
                "draft.core/project-decision-authority|{}",
                workspace.workspace_id
            )
            .as_bytes(),
        ),
    ))
}

/// The decision this actor's judgement of this revision is.
///
/// Derived so that a surface retry recomputes the same id and the create-once
/// decision store converges, rather than a dropped connection producing a
/// second immutable record of the same judgement.
fn decision_id_for(
    revision: &draft_dcg_contract::ids::RevisionPackId,
    approve: bool,
) -> DraftResult<draft_dcg_contract::ids::DecisionId> {
    let verdict = if approve { "approved" } else { "rejected" };
    let seed = draft_dcg_contract::Digest::of_bytes(format!("{revision}|{verdict}").as_bytes());
    let short: String = seed
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect();
    draft_dcg_contract::ids::DecisionId::parse(format!("dec_{short}"))
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

fn append_project_config_event(ws: &Workspace, key: &str, operation: &str) -> DraftResult<()> {
    let bytes = std::fs::read(ws.layout.config_toml())?;
    // One event for both: a project's configuration is its policy about how it
    // is read and checked, and the changed key is in the payload for a reader
    // who needs to know which part moved.
    ws.events()?.append(
        EventKind::PolicyUpdated,
        None,
        serde_json::json!({
            "scope": "project",
            "changed_keys": [key],
            "operation": operation,
            "resulting_config_digest": crate::support::hashing::sha256_hex(&bytes),
        }),
    )?;
    Ok(())
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl Workspace {
    fn events(&self) -> DraftResult<ProjectActivity> {
        Ok(ProjectActivity::new(
            self.layout.clone(),
            &self.workspace_id,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitReport {
    pub workspace_id: String,
    pub root: String,
    pub created: bool,
    pub draft_dir: String,
    /// The Baseline this project now accepts.
    pub baseline_id: String,
    /// The material state that Baseline accepted.
    pub project_state_root: String,
    #[serde(default)]
    pub next_actions: Vec<String>,
    #[serde(default)]
    pub candidate_guidance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseReport {
    pub closed: bool,
    pub forced: bool,
    pub draft_dir: String,
    pub pending_changes: usize,
}

const DEFAULT_IGNORE: &str = "# Draft private metadata is always excluded.\n.draft/\n";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigReport {
    pub entries: BTreeMap<String, String>,
}

impl ConfigReport {
    fn single(key: &str, value: &str) -> Self {
        let mut entries = BTreeMap::new();
        entries.insert(key.to_string(), value.to_string());
        Self { entries }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRunReport {
    pub hook_name: String,
    pub exit_code: i32,
    pub stdout_ref: String,
    pub stderr_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IgnoreReport {
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStats {
    pub draft_size_bytes: u64,
    pub repo_size_bytes: u64,
    pub objects_size_bytes: u64,
    pub changes_size_bytes: u64,
    pub receipts_size_bytes: u64,
    pub events_size_bytes: u64,
    pub draft_repo_ratio: f64,
    pub growth_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageMaintenanceReport {
    pub operation: String,
    pub removed: usize,
    pub status: String,
}

impl StorageMaintenanceReport {
    fn new(operation: &str, removed: usize, status: &str) -> Self {
        Self {
            operation: operation.to_string(),
            removed,
            status: status.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageDoctorReport {
    pub activity_chain_ok: bool,
    pub activity_chain_error: Option<String>,
    pub draft_hard_excluded: bool,
    #[serde(default)]
    pub draft_exclusion_errors: Vec<String>,
    pub objects_ok: bool,
    pub object_errors: Vec<String>,
    pub receipts_ok: bool,
    pub receipt_errors: Vec<String>,
    pub receipts: usize,
    pub changes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointReport {
    pub snapshot_id: String,
    /// The Activity event that records the checkpoint, and the reference a
    /// later `draft recover` names to come back to it.
    pub event_id: String,
    pub resources: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpawnReport {
    pub task_id: String,
    pub task_name: String,
    pub task_kind: String,
    pub preset: Option<String>,
    pub parent_change: Option<String>,
    pub executions: Vec<ExecutionSummary>,
    pub next_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSummary {
    pub execution_id: String,
    pub candidate: String,
    pub status: String,
    pub produced_change: Option<String>,
    pub error: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExportReport {
    pub task_id: String,
    pub task_name: String,
    pub output: String,
    pub next_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskImportReport {
    pub task_id: String,
    pub task_name: String,
    pub source: String,
    pub next_action: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TaskViewOptions {
    pub full: bool,
    pub executions: bool,
    pub changes: bool,
    pub conflicts: bool,
    pub lanes: bool,
    pub evidence: bool,
    pub timeline: bool,
    pub explain: bool,
    pub decompose: bool,
    pub compare_stable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateChangePackAssignment {
    pub change_pack_id: String,
    pub candidate: String,
    pub task_id: Option<String>,
    pub execution_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangePackReport {
    pub lifecycle: ReviewProgressState,
    pub change: ChangePackWorkspace,
    /// The authoritative transition. Always present, whatever is installed.
    pub change_set: ChangeSet,
    /// The derived explanation of it, when a comparison capability produced
    /// one. `None` is a real answer: Draft knows *that* the resources changed
    /// and between which states, without being able to say how.
    pub representations: Option<RevisionPackRepresentationBundle>,
    pub evidence: Option<Evidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareReport {
    pub id: String,
    pub left_change: String,
    pub right_change: String,
    /// Resources both ChangePacks touch.
    pub overlapping_resources: Vec<ResourceId>,
    /// How the two ChangePacks relate on each shared resource, where they do not
    /// simply compose.
    ///
    /// `Indeterminate` appears here as itself rather than as a conflict or a
    /// pass, because "Draft cannot tell whether these are separable" is a
    /// different fact from "they collide" — even though both block composition.
    #[serde(default)]
    pub interference: Vec<ResourceInterference>,
    pub unique_left_resources: Vec<ResourceId>,
    pub unique_right_resources: Vec<ResourceId>,
    #[serde(default)]
    pub compatible: bool,
    pub warnings: Vec<String>,
    pub recommendation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeResult {
    pub output_pack_id: String,
    pub source_changes: Vec<String>,
    pub receipt_id: String,
    #[serde(default)]
    pub resources: usize,
    #[serde(default)]
    pub compatible: bool,
    #[serde(default)]
    pub requires_verification: bool,
    #[serde(default)]
    pub requires_review: bool,
    #[serde(default)]
    pub final_success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisperseResult {
    pub source_change_pack_id: String,
    pub output_pack_ids: Vec<String>,
    pub receipt_id: String,
    #[serde(default)]
    pub requires_verification: bool,
    #[serde(default)]
    pub requires_review: bool,
    #[serde(default)]
    pub final_success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexReport {
    pub path: String,
    pub events: usize,
    pub tasks: usize,
    pub executions: usize,
    pub changes: usize,
    pub receipts: usize,
    pub snapshots: usize,
}

fn find_workspace_root(cwd: &Path) -> Option<PathBuf> {
    let mut cur = cwd
        .canonicalize()
        .ok()
        .or_else(|| Some(cwd.to_path_buf()))?;
    loop {
        if cur.join(DRAFT_DIR).is_dir() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

fn reject_remote_key(key: &str) -> DraftResult<()> {
    if key.starts_with("target.") {
        return Err(DraftError::invalid_config(
            "retired external-action config keys are unsupported; use hooks.*",
        ));
    }
    Ok(())
}

fn validate_config_key(key: &str) -> DraftResult<()> {
    if key == "identity" || key.starts_with("identity.") {
        return Err(DraftError::new(
            DraftErrorKind::UnsupportedSchema,
            "identity.* is unsupported pre-release profile state",
        )
        .with_suggestion("use user.name or user.email through `draft config`"));
    }
    if key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_')
        && key.contains('.')
    {
        Ok(())
    } else {
        Err(DraftError::invalid_config(
            "config keys must be lowercase dotted keys",
        ))
    }
}

trait ConfigContract {
    fn declared_schema_version(&self) -> u32;
}

impl ConfigContract for RiskConfig {
    fn declared_schema_version(&self) -> u32 {
        self.schema_version
    }
}

impl ConfigContract for VerificationConfig {
    fn declared_schema_version(&self) -> u32 {
        self.schema_version
    }
}

fn read_or_default<T>(path: &Path) -> DraftResult<T>
where
    T: for<'de> Deserialize<'de> + Default + ConfigContract + crate::contracts::VersionedContract,
{
    let value = if path.exists() {
        read_toml(path)?
    } else {
        T::default()
    };
    if !crate::contracts::supports_version(T::CONTRACT, value.declared_schema_version()) {
        return Err(DraftError::new(
            DraftErrorKind::UnsupportedSchema,
            format!(
                "configuration schema {} is unsupported",
                value.declared_schema_version()
            ),
        ));
    }
    Ok(value)
}

fn is_draft_path(path: &str) -> bool {
    // Delegate to the central path guard so *every* `.draft` component (nested,
    // case-insensitive, backslash-separated) is hard-excluded (spec §9.2), not
    // just a top-level `.draft/`.
    crate::support::pathguard::is_draft_path(path)
}

fn validate_task_definition(
    protections: &[crate::project::protected::ProtectionRule],
    task: &crate::task::TaskDefinition,
) -> DraftResult<()> {
    if task.success_criteria.is_empty() {
        return Err(
            DraftError::invalid_config("task success criteria cannot be empty")
                .with_suggestion("pass --success <criteria> or choose a template with defaults"),
        );
    }
    if task.allowed_zones.iter().any(|zone| zone.trim().is_empty())
        || task
            .forbidden_zones
            .iter()
            .any(|zone| zone.trim().is_empty())
    {
        return Err(DraftError::invalid_config(
            "task zones cannot contain empty patterns",
        ));
    }
    if let Some(zone) = task
        .allowed_zones
        .iter()
        .find(|zone| crate::project::protected::matches_rules(protections, zone))
    {
        return Err(DraftError::new(
            DraftErrorKind::ProtectedResourceAccess,
            format!("task allowed zone '{zone}' conflicts with protected-file rules"),
        )
        .with_suggestion("remove protected paths from --allow and keep them in --forbid"));
    }
    serde_json::to_value(task)
        .and_then(serde_json::from_value::<crate::task::TaskDefinition>)
        .map_err(|err| DraftError::storage(format!("task schema round-trip failed: {err}")))?;
    Ok(())
}

/// The baseline every project starts from: an observation of nothing.
///
/// Its coverage is complete, which is what makes the first real snapshot's
/// resources genuinely `Added` rather than merely unexplained.
fn empty_snapshot(ws: &Workspace) -> Snapshot {
    empty_snapshot_for(&ws.workspace_id, &ws.layout)
}

/// The same, for `init`, before a workspace can be opened.
fn empty_snapshot_for(
    workspace_id: &ProjectId,
    layout: &crate::project::layout::DraftLayout,
) -> Snapshot {
    let _ = layout;
    Snapshot {
        schema_version: current_version(ContractId::WorkspaceSnapshot),
        id: SnapshotId::new("chk_empty"),
        workspace_id: workspace_id.clone(),
        observation_context_digest: baseline_observation_context().context_digest,
        resources: vec![],
        observation_map: crate::dcg::observation::SnapshotObservationMap {
            domains: vec![crate::dcg::observation::ObservationCoverage {
                domain: crate::dcg::snapshot::domain(crate::dcg::snapshot::ROOT_DOMAIN),
                status: crate::dcg::observation::CoverageStatus::Complete,
            }],
            resource_membership: vec![],
        },
        gaps: vec![],
        untrackable: vec![],
        identity_proofs: vec![],
        content_object_refs: vec![],
        created_at: now(),
        created_by: ActorRef {
            id: ActorId::new("act_system"),
            kind: ActorKind::Service,
            display_name: "draft".to_string(),
        },
        snapshot_digest: String::new(),
    }
    .seal()
}

/// The observation semantics currently in force for a workspace.
///
/// With no contributed adapter or view rule this is Draft's own filesystem
/// observer alone — which is the zero-extension case, and is a real context
/// rather than an absent one.
/// The observation semantics currently in force for this project.
///
/// Draft's own filesystem observer, plus every installed `view_rules`
/// contribution as a declarative binding. A view-rule binding structurally
/// carries no mechanism — it changes what is observed, never how — and its
/// presence in the context digest is what makes adopting a new one a
/// re-observation rather than a silent change of meaning.
/// What the installed extensions *would* observe under.
///
/// Deliberately not "the active context". This is a candidate: it is what the
/// currently installed and authorized contributions add up to right now, and a
/// project only observes under it once somebody has adopted it. Confusing the
/// two is the failure the whole lifecycle exists to prevent — an install would
/// silently change what the project claims it saw, retroactively.
fn effective_observation_context(
    ws: &Workspace,
    contributions: &crate::extension::ActiveContributions,
) -> ObservationContext {
    let _ = ws;
    let mut bindings: Vec<crate::dcg::observation::ViewRuleBinding> = contributions
        .policies
        .iter()
        .filter(|preset| !preset.value.view_rules.exclusions.is_empty())
        .map(|preset| crate::dcg::observation::ViewRuleBinding {
            binding_id: crate::dcg::observation::ViewRuleBindingId(format!(
                "view:{}",
                preset.extension_id
            )),
            contribution_id: preset.extension_id.clone(),
            contribution_semantics_digest: try_canonical_hash(&preset.value.view_rules)
                .unwrap_or_default(),
        })
        .collect();
    // Deterministic and installation-order independent: two projects with the
    // same packages installed observe under the same context digest.
    bindings.sort_by(|a, b| a.binding_id.0.cmp(&b.binding_id.0));
    bindings.dedup_by(|a, b| a.binding_id == b.binding_id);

    let mut context = baseline_observation_context();
    context.view_rule_bindings = bindings;
    ObservationContext::build(context.adapter_bindings, context.view_rule_bindings)
}

/// A predicate-evaluable view of one observed resource.
fn resource_view_of(
    state: &crate::dcg::resource::RawResourceState,
) -> crate::extension::ResourceView<'_> {
    crate::extension::ResourceView {
        locator_scheme: state.locator.scheme.as_str(),
        locator_body: state.locator.body.as_str(),
        media_type: state.media_type.as_deref(),
        form: state.form,
        attributes: &state.attributes,
        content_size: state.content_size,
    }
}

/// The rendering that always exists.
///
/// It is a platform engine, not a contribution, so a resource from a domain
/// nothing understands still displays — as its own intrinsic facts. That is
/// what makes an unknown domain usable rather than blank.
const NEUTRAL_PRESENTATION: &str = "metadata_summary";

/// Where revision-bound representations live.
fn representation_store(
    layout: &crate::project::layout::DraftLayout,
) -> crate::evidence::representation::RepresentationStore {
    crate::evidence::representation::RepresentationStore::new(layout.representations_dir())
}

/// Every exclusion the installed presets contribute, in a stable order.
fn contributed_view_rules(
    contributions: &crate::extension::ActiveContributions,
) -> Vec<draft_extension_contract::ResourceRule> {
    let mut rules: Vec<_> = contributions
        .policies
        .iter()
        .flat_map(|preset| preset.value.view_rules.exclusions.iter().cloned())
        .collect();
    rules.sort_by_key(|rule| {
        try_canonical_hash(&rule.predicate).unwrap_or_else(|_| rule.reason.clone())
    });
    rules
}

/// The zero-extension observation context: Draft's own filesystem observer and
/// nothing else.
fn baseline_observation_context() -> ObservationContext {
    ObservationContext::build(
        vec![crate::dcg::observation::AdapterObservationBinding {
            binding_id: crate::dcg::snapshot::filesystem_binding_id(),
            contribution_id: crate::dcg::snapshot::FILESYSTEM_BINDING.to_string(),
            contribution_semantics_digest: try_canonical_hash(&serde_json::json!({
                "adapter": crate::dcg::snapshot::FILESYSTEM_BINDING,
                "revision": crate::dcg::snapshot::FILESYSTEM_OBSERVER_REVISION,
            }))
            .unwrap_or_default(),
            mechanism: crate::dcg::observation::EffectiveObservationMechanism::Engine {
                engine: crate::extension::EngineId::ResourceEnumeration,
                engine_revision: crate::dcg::snapshot::FILESYSTEM_OBSERVER_REVISION,
                engine_config_digest: try_canonical_hash(&serde_json::json!({}))
                    .unwrap_or_default(),
                request_schema_digest: String::new(),
                response_schema_digest: String::new(),
                coverage_domain_semantics_digest: try_canonical_hash(&serde_json::json!({
                    "partition": "single-universe-with-incomplete-subtrees",
                    "revision": crate::dcg::snapshot::FILESYSTEM_OBSERVER_REVISION,
                }))
                .unwrap_or_default(),
            },
        }],
        vec![],
    )
}

fn load_snapshot(ws: &Workspace, id: &SnapshotId) -> DraftResult<Snapshot> {
    if id.as_str() == "chk_empty" {
        return Ok(empty_snapshot(ws));
    }
    crate::contracts::read_persisted(&ws.layout.snapshot_file(id.as_str()))
}

/// The locators a transition touches, for display and for risk hotspots.
fn changed_locators(change_set: &ChangeSet) -> Vec<ResourceLocator> {
    let mut locators: Vec<ResourceLocator> = change_set
        .resources
        .iter()
        .filter_map(|change| {
            change
                .after
                .as_ref()
                .or(change.before.as_ref())
                .map(|side| side.locator.clone())
        })
        .collect();
    locators.sort();
    locators.dedup();
    locators
}

fn load_json_dir<T: serde::de::DeserializeOwned + crate::contracts::VersionedContract>(
    dir: &Path,
) -> DraftResult<Vec<T>> {
    let mut out = Vec::new();
    for p in list_with_extension(dir, "json")? {
        out.push(crate::contracts::read_persisted(&p)?);
    }
    Ok(out)
}

fn load_change(ws: &Workspace, id: &str) -> DraftResult<ChangePackWorkspace> {
    let change: ChangePackWorkspace = crate::contracts::read_persisted(
        &ws.layout
            .change_pack_workspaces_dir()
            .join(id)
            .join("staging.json"),
    )?;
    change.validate()?;
    Ok(change)
}

/// Work that removing the project would destroy.
///
/// Every ChangePack still open. A ChangePack stays open until a promotion carries one
/// of its revisions onto the accepted Baseline, so an open one is unfinished
/// work by definition — and this is the only thing standing between a person
/// and deleting it, which is why counting the wrong records here is worse than
/// not counting at all: the refusal still prints, and it always says zero.
fn unsafe_pending_change_count(paths: &crate::project::layout::DraftLayout) -> DraftResult<usize> {
    let store = crate::dcg::change_pack::ChangePackStore::new(paths.change_packs_dir());
    Ok(store
        .list()?
        .into_iter()
        .filter(|change| change.lifecycle.accepts_work())
        .count())
}

/// The authoritative transition a ChangePack carries.
///
/// Validated on load: a change set names its base and result by digest, and a
/// substituted or corrupted record is refused rather than silently trusted.
fn load_change_set(ws: &Workspace, change: &ChangePackWorkspace) -> DraftResult<ChangeSet> {
    let change_set: ChangeSet = crate::contracts::read_persisted(
        &ws.layout
            .change_pack_workspace_dir(&change.id)
            .join("changes.json"),
    )?;
    // Validated by recomputing the canonical identity, not by hashing the
    // record: a re-recorded transition keeps its identity, and a substituted
    // one loses it.
    if change_set.change_set_digest != change_set.compute_digest() {
        return Err(DraftError::new(
            DraftErrorKind::CorruptData,
            format!("ChangePack {} change set digest mismatch", change.id),
        ));
    }
    Ok(change_set)
}

/// The producer record for one contributed extension.
pub(crate) fn producer_ref_for(
    contributions: &crate::extension::ActiveContributions,
    extension_id: &str,
) -> crate::extension::ProducerRef {
    contributions
        .attestations
        .get(extension_id)
        .cloned()
        .unwrap_or_else(|| crate::extension::ProducerRef {
            extension_id: extension_id.to_string(),
            extension_version: String::new(),
            package_digest: String::new(),
            attestation_digest: String::new(),
        })
}

fn insert_inbox(
    by_id: &mut BTreeMap<String, crate::read_model::inbox::InboxItem>,
    id: String,
    kind: &str,
    subject_id: String,
    status: &str,
    summary: String,
    next_action: String,
) {
    by_id
        .entry(id.clone())
        .or_insert(crate::read_model::inbox::InboxItem {
            schema_version: current_version(ContractId::InboxItem),
            id,
            kind: kind.into(),
            subject_id,
            status: status.into(),
            summary,
            next_action,
        });
}

fn index_status(name: &str, path: PathBuf, inputs: &[PathBuf]) -> DraftResult<Value> {
    let path_meta = match fs::metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(serde_json::json!({
                "name": name,
                "path": path,
                "state": "missing",
                "reason": "index file is missing",
            }));
        }
        Err(e) => {
            return Ok(serde_json::json!({
                "name": name,
                "path": path,
                "state": "failed",
                "reason": e.to_string(),
            }));
        }
    };
    let mut stale_inputs = Vec::new();
    if let Ok(index_modified) = path_meta.modified() {
        for input in inputs {
            if !input.exists() {
                continue;
            }
            let mut newest = None;
            collect_newest_mtime(input, &mut newest)?;
            if newest.map(|t| t > index_modified).unwrap_or(false) {
                stale_inputs.push(input.display().to_string());
            }
        }
    }
    let state = if stale_inputs.is_empty() {
        "fresh"
    } else {
        "stale"
    };
    Ok(serde_json::json!({
        "name": name,
        "path": path,
        "state": state,
        "stale_inputs": stale_inputs,
    }))
}

fn collect_newest_mtime(
    path: &Path,
    newest: &mut Option<std::time::SystemTime>,
) -> DraftResult<()> {
    let meta = fs::metadata(path)?;
    if let Ok(modified) = meta.modified() {
        if newest.map(|current| modified > current).unwrap_or(true) {
            *newest = Some(modified);
        }
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            collect_newest_mtime(&entry?.path(), newest)?;
        }
    }
    Ok(())
}

fn write_rollback_record(ws: &Workspace, receipt: &mut RollbackRecord) -> DraftResult<()> {
    receipt.record_digest.clear();
    receipt.record_digest = hash_json(receipt)?;
    let directory = ws.layout.recovery_dir().join("rollback-records");
    ensure_dir(&directory)?;
    write_json(&directory.join(format!("{}.json", receipt.id)), receipt)?;
    Ok(())
}

fn rebuild_index(ws: &Workspace) -> DraftResult<IndexReport> {
    rebuild_index_for_layout(&ws.layout)?;
    let conn = open_index(&ws.layout)?;
    conn.execute("DELETE FROM events", []).map_err(sql_err)?;
    conn.execute("DELETE FROM tasks", []).map_err(sql_err)?;
    conn.execute("DELETE FROM executions", [])
        .map_err(sql_err)?;
    conn.execute("DELETE FROM changes", []).map_err(sql_err)?;
    conn.execute("DELETE FROM receipts", []).map_err(sql_err)?;
    conn.execute("DELETE FROM snapshots", []).map_err(sql_err)?;

    let events = ws.events()?.read_all()?;
    for event in &events {
        conn.execute(
            "INSERT INTO events (id, event_type, subject_id, time, event_hash) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.event_id,
                event.kind,
                event.subject,
                event.previous_hash,
                event.record_hash
            ],
        )
        .map_err(sql_err)?;
    }

    let tasks = crate::task::TaskStore::for_root(&ws.root).list()?;
    for task in &tasks {
        conn.execute(
            "INSERT INTO tasks (id, title, status, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                task.id.to_string(),
                task.name,
                format!("{:?}", task.status),
                task.created_at.to_rfc3339()
            ],
        )
        .map_err(sql_err)?;
    }

    let executions = crate::task::ExecutionStore::for_root(&ws.root).list_all()?;
    for execution in &executions {
        conn.execute(
            "INSERT INTO executions (id, task_id, status, started_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                execution.id.to_string(),
                execution.task_id.to_string(),
                format!("{:?}", execution.status),
                execution.started_at.map(|time| time.to_rfc3339())
            ],
        )
        .map_err(sql_err)?;
    }

    // The graph's ChangePacks. A ChangePack has no name of its own — what it is for
    // lives in its definition — so the newest sealed revision stands in, which
    // is what a searcher is actually looking for.
    for change in App::new().dcg_change_packs(&ws.root)? {
        let revision = change
            .revisions
            .first()
            .map(|revision| revision.id.to_string())
            .unwrap_or_default();
        conn.execute(
            "INSERT INTO changes (id, name, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                change.change_pack.to_string(),
                revision,
                format!("{:?}", change.lifecycle),
                String::new(),
                String::new()
            ],
        )
        .map_err(sql_err)?;
    }

    let receipts = App::new().receipts(&ws.root)?;
    for receipt in &receipts {
        conn.execute(
            "INSERT INTO receipts (id, kind, status, subject_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                receipt
                    .get("receipt_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                receipt
                    .get("event_type")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "signed",
                receipt
                    .get("subject_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                receipt
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ],
        )
        .map_err(sql_err)?;
    }

    let snapshots: Vec<Snapshot> = load_json_dir(&ws.layout.snapshots_dir())?;
    for snapshot in &snapshots {
        conn.execute(
            "INSERT INTO snapshots (id, snapshot_digest, created_at, resource_count) VALUES (?1, ?2, ?3, ?4)",
            params![
                snapshot.id.to_string(),
                snapshot.snapshot_digest,
                snapshot.created_at.to_rfc3339(),
                snapshot.resources.len() as i64
            ],
        )
        .map_err(sql_err)?;
    }

    Ok(IndexReport {
        path: ws.layout.index_file().display().to_string(),
        events: events.len(),
        tasks: tasks.len(),
        executions: executions.len(),
        changes: App::new().dcg_change_packs(&ws.root)?.len(),
        receipts: receipts.len(),
        snapshots: snapshots.len(),
    })
}

fn rebuild_index_for_layout(layout: &DraftLayout) -> DraftResult<()> {
    let conn = open_index(layout)?;

    // The index is a derived cache, and a rebuild that inherited a stale table
    // shape would not be one: `CREATE TABLE IF NOT EXISTS` silently keeps the
    // old columns, and the first insert then fails on a column that is not
    // there. So when the recorded revision is not the one this build writes,
    // the derived tables are dropped and recreated. Authoritative state is
    // untouched — everything here is re-derived from it on the next few lines.
    let recorded: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_info WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .ok();
    let expected = current_version(ContractId::WorkspaceIndex).to_string();
    if recorded.is_some_and(|found| found != expected) {
        conn.execute_batch(
            "
            DROP TABLE IF EXISTS events;
            DROP TABLE IF EXISTS tasks;
            DROP TABLE IF EXISTS executions;
            DROP TABLE IF EXISTS changes;
            DROP TABLE IF EXISTS receipts;
            DROP TABLE IF EXISTS snapshots;
            ",
        )
        .map_err(sql_err)?;
    }

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_info (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY,
            event_type TEXT NOT NULL,
            subject_id TEXT,
            time TEXT NOT NULL,
            event_hash TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS executions (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            status TEXT NOT NULL,
            started_at TEXT
        );
        CREATE TABLE IF NOT EXISTS changes (
            id TEXT PRIMARY KEY,
            name TEXT,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS receipts (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            status TEXT NOT NULL,
            subject_id TEXT,
            created_at TEXT
        );
        CREATE TABLE IF NOT EXISTS snapshots (
            id TEXT PRIMARY KEY,
            snapshot_digest TEXT NOT NULL,
            created_at TEXT NOT NULL,
            resource_count INTEGER NOT NULL
        );
        ",
    )
    .map_err(sql_err)?;
    conn.execute(
        "INSERT OR REPLACE INTO schema_info (key, value) VALUES ('schema_version', ?1)",
        params![current_version(ContractId::WorkspaceIndex).to_string()],
    )
    .map_err(sql_err)?;
    Ok(())
}

fn open_index(layout: &DraftLayout) -> DraftResult<Connection> {
    ensure_dir(&layout.indexes_dir())?;
    Connection::open(layout.index_file()).map_err(sql_err)
}

fn sql_err(e: rusqlite::Error) -> DraftError {
    DraftError::storage(format!("SQLite index error: {e}"))
}

fn resolve_snapshot_reference(ws: &Workspace, reference: &str) -> DraftResult<Snapshot> {
    if reference.starts_with("chk_") {
        validate_checkpoint_id(reference)?;
        return load_snapshot(ws, &SnapshotId::new(reference));
    }
    if reference.starts_with("cpk_") {
        validate_change_pack_id(reference)?;
        let staging = ws.layout.change_pack_workspace_dir(reference);
        if !staging.join("staging.json").exists()
            && crate::dcg::change_pack_store::ChangePackContentStore::new(ws.layout.clone())
                .exists(reference)
        {
            return Err(DraftError::invalid_config(format!(
                "promoted change '{reference}' is immutable and its mutable staging snapshot was \
                 disposed; recover to a checkpoint or the Activity event that recorded one"
            ))
            .with_suggestion("`draft activity list` shows the checkpoints this project recorded"));
        }
        let change = load_change(ws, reference)?;
        return load_snapshot(ws, &change.base_snapshot_id);
    }
    if reference.starts_with("evt_") {
        return resolve_activity_target(ws, reference);
    }
    Err(DraftError::invalid_config(format!(
        "recovery reference '{reference}' must start with chk_, cpk_, or evt_"
    )))
}

/// Resolve an Activity event to the state it can be recovered to.
///
/// The anchor is the event itself rather than a signed receipt over it: v1
/// receipts attest promotions and publications, and requiring one here made a
/// local checkpoint depend on a signing identity it has no reason to need.
/// The ledger is hash-chained and verified, so the event is the durable fact.
fn resolve_activity_target(ws: &Workspace, event_id: &str) -> DraftResult<Snapshot> {
    let activity = ws.events()?;
    let entry = crate::read_model::activity::entry(activity.log(), event_id)?;
    if entry.kind != crate::activity::EventKind::CheckpointCreated.as_str() {
        return Err(DraftError::invalid_config(format!(
            "Activity event '{event_id}' is a {} and names no state to recover to",
            entry.kind
        )));
    }
    match entry.subject.as_deref() {
        Some(subject) if subject.starts_with("chk_") => {
            validate_checkpoint_id(subject)?;
            load_snapshot(ws, &SnapshotId::new(subject))
        }
        other => Err(DraftError::invalid_config(format!(
            "Activity event '{event_id}' subject '{}' is not a recovery target",
            other.unwrap_or("<none>")
        ))),
    }
}

fn validate_checkpoint_id(id: &str) -> DraftResult<()> {
    validate_prefixed_id(id, "chk_", "checkpoint")
}

fn validate_change_pack_id(id: &str) -> DraftResult<()> {
    validate_prefixed_id(id, "cpk_", "change")
}

fn validate_receipt_id(id: &str) -> DraftResult<()> {
    validate_prefixed_id(id, "rcp_", "receipt")
}

fn validate_prefixed_id(id: &str, prefix: &str, label: &str) -> DraftResult<()> {
    let rest = id.strip_prefix(prefix).ok_or_else(|| {
        DraftError::invalid_config(format!("{label} id '{id}' must start with {prefix}"))
    })?;
    if rest.len() >= 6
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Ok(())
    } else {
        Err(DraftError::invalid_config(format!(
            "malformed {label} id '{id}'"
        )))
    }
}

fn dir_size(path: &Path) -> DraftResult<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0;
    for entry in walkdir::WalkDir::new(path) {
        let entry = entry.map_err(|e| DraftError::storage(e.to_string()))?;
        if entry.file_type().is_file() {
            total += entry
                .metadata()
                .map_err(|e| DraftError::storage(e.to_string()))?
                .len();
        }
    }
    Ok(total)
}

fn dir_size_excluding_draft(path: &Path) -> DraftResult<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0;
    for entry in walkdir::WalkDir::new(path)
        .into_iter()
        .filter_entry(|entry| {
            entry
                .path()
                .strip_prefix(path)
                .ok()
                .and_then(|rel| rel.to_str())
                .map(|rel| !is_draft_path(rel))
                .unwrap_or(true)
        })
    {
        let entry = entry.map_err(|e| DraftError::storage(e.to_string()))?;
        if entry.file_type().is_file() {
            total += entry
                .metadata()
                .map_err(|e| DraftError::storage(e.to_string()))?
                .len();
        }
    }
    Ok(total)
}

fn storage_ratio(draft_size: u64, repo_size: u64) -> f64 {
    if repo_size == 0 {
        0.0
    } else {
        draft_size as f64 / repo_size as f64
    }
}

fn storage_growth_status(draft_size: u64, repo_size: u64) -> String {
    let ratio = storage_ratio(draft_size, repo_size);
    if ratio >= 0.5 {
        "critical".to_string()
    } else if ratio >= 0.25 {
        "warning".to_string()
    } else {
        "ok".to_string()
    }
}

fn garbage_collect_objects(ws: &Workspace) -> DraftResult<usize> {
    let reachable = collect_reachable_object_refs(ws)?;
    let mut removed = 0usize;
    if !ws.layout.objects_dir().exists() {
        return Ok(0);
    }
    for path in collect_object_files(&ws.layout.objects_dir())? {
        let Some(object_ref) = object_ref_for_path(&ws.layout, &path) else {
            continue;
        };
        if !reachable.contains(&object_ref) {
            fs::remove_file(path)?;
            // Counted after the deletion succeeded. A collection that failed
            // is not a collection, and counting the intent would tell an
            // operator storage was reclaimed when it was not.
            crate::support::telemetry::Counter::GcObjectsCollected.increment();
            removed += 1;
        }
    }
    let _ = prune_empty_dirs(&ws.layout.objects_dir())?;
    Ok(removed)
}

fn verify_objects(ws: &Workspace) -> DraftResult<Vec<String>> {
    let mut errors = Vec::new();
    let store = ObjectStore::new(ws.layout.clone());
    for path in collect_object_files(&ws.layout.objects_dir())? {
        let Some(object_ref) = object_ref_for_path(&ws.layout, &path) else {
            errors.push(format!("unrecognized object path {}", path.display()));
            continue;
        };
        if let Err(e) = store.get_bytes(&object_ref) {
            errors.push(format!("{object_ref}: {e}"));
        }
    }
    let index = read_object_segment_index(&ws.layout)?;
    for object_ref in index.objects.keys() {
        if let Err(e) = store.get_bytes(object_ref) {
            errors.push(format!("{object_ref}: {e}"));
        }
    }
    Ok(errors)
}

fn compact_loose_objects(ws: &Workspace) -> DraftResult<usize> {
    let mut entries = Vec::new();
    let mut loose_paths = Vec::new();
    for path in collect_object_files(&ws.layout.objects_dir())? {
        let Some(object_ref) = object_ref_for_path(&ws.layout, &path) else {
            continue;
        };
        let compressed = fs::read(&path)?;
        entries.push(ObjectSegmentEntry {
            object_ref,
            compressed_hex: hex_encode(&compressed),
        });
        loose_paths.push(path);
    }
    if entries.is_empty() {
        return Ok(0);
    }
    ensure_dir(&ws.layout.object_segments_dir())?;
    let change_pack_id = format!("opk_{}", uuid::Uuid::new_v4().simple());
    let change_name = format!("{change_pack_id}.json.zst");
    let change = ObjectSegment {
        schema_version: current_version(ContractId::ObjectSegment),
        id: change_pack_id,
        created_at: now(),
        entries,
    };
    let json = serde_json::to_vec(&change).map_err(json_err)?;
    let compressed = zstd::stream::encode_all(json.as_slice(), 3)
        .map_err(|e| DraftError::storage(format!("object change compression failed: {e}")))?;
    write_atomic(
        &ws.layout.object_segments_dir().join(&change_name),
        &compressed,
    )?;

    let mut index = read_object_segment_index(&ws.layout)?;
    for entry in &change.entries {
        index
            .objects
            .insert(entry.object_ref.clone(), change_name.clone());
    }
    write_object_segment_index(&ws.layout, &index)?;
    let store = ObjectStore::new(ws.layout.clone());
    for entry in &change.entries {
        store.get_bytes(&entry.object_ref)?;
    }

    let removed = loose_paths.len();
    for path in loose_paths {
        fs::remove_file(path)?;
    }
    let _ = prune_empty_dirs(&ws.layout.objects_dir())?;
    Ok(removed)
}

fn verify_receipts(ws: &Workspace) -> DraftResult<Vec<String>> {
    let verification = crate::read_model::integrity::verify_all(&ws.layout, &ws.workspace_id)?;
    let mut errors = Vec::new();
    if !verification.activity_chain_ok {
        errors.push("Activity chain verification failed".into());
    }
    for receipt in verification
        .receipts
        .into_iter()
        .filter(|receipt| !receipt.ok)
    {
        let failed = receipt
            .checks
            .into_iter()
            .filter(|check| check.status != crate::receipt::CheckStatus::Valid)
            .map(|check| check.name)
            .collect::<Vec<_>>()
            .join(", ");
        errors.push(format!("{}: {failed}", receipt.receipt_id));
    }
    Ok(errors)
}

fn verify_draft_hard_exclusion(ws: &Workspace) -> DraftResult<Vec<String>> {
    let mut errors = Vec::new();
    if !ws.layout.change_pack_workspaces_dir().exists() {
        return Ok(errors);
    }
    for entry in fs::read_dir(ws.layout.change_pack_workspaces_dir())? {
        let manifest = entry?.path().join("staging.json");
        if !manifest.exists() {
            continue;
        }
        let change: ChangePackWorkspace = crate::contracts::read_persisted(&manifest)?;
        change.validate()?;
        let patch = load_change_set(ws, &change)?;
        // Both sides: `.draft/**` must not enter project state through either a
        // change's result or the state it came from.
        for locator in changed_locators(&patch) {
            if locator.scheme == crate::extension::FILE_SCHEME && is_draft_path(&locator.body) {
                errors.push(format!(
                    "{} includes Draft control-plane locator {}",
                    change.id, locator.body
                ));
            }
        }
    }
    Ok(errors)
}

fn collect_reachable_object_refs(ws: &Workspace) -> DraftResult<HashSet<String>> {
    let mut refs = HashSet::new();
    collect_object_refs_from_json_dir(&ws.layout.draft_dir, &ws.layout, &mut refs)?;
    Ok(refs)
}

fn collect_object_refs_from_json_dir(
    dir: &Path,
    layout: &DraftLayout,
    refs: &mut HashSet<String>,
) -> DraftResult<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if should_skip_storage_scan(&path, layout) {
            continue;
        }
        if path.is_dir() {
            collect_object_refs_from_json_dir(&path, layout, refs)?;
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("json") | Some("jsonl")
        ) {
            let text = fs::read_to_string(&path)?;
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                for (line_number, line) in text
                    .lines()
                    .enumerate()
                    .filter(|(_, line)| !line.trim().is_empty())
                {
                    let value = serde_json::from_str::<Value>(line).map_err(|error| {
                        DraftError::new(
                            DraftErrorKind::CorruptData,
                            format!(
                                "malformed authoritative JSONL {} at line {}: {error}",
                                path.display(),
                                line_number + 1
                            ),
                        )
                    })?;
                    collect_object_refs_from_value(&value, refs);
                }
            } else {
                let value = serde_json::from_str::<Value>(&text).map_err(|error| {
                    DraftError::new(
                        DraftErrorKind::CorruptData,
                        format!("malformed authoritative JSON {}: {error}", path.display()),
                    )
                })?;
                collect_object_refs_from_value(&value, refs);
            }
        }
    }
    Ok(())
}

fn should_skip_storage_scan(path: &Path, layout: &DraftLayout) -> bool {
    path.starts_with(layout.objects_dir())
        || path.starts_with(layout.cache_dir())
        || path.starts_with(layout.tmp_dir())
}

fn collect_object_refs_from_value(value: &Value, refs: &mut HashSet<String>) {
    match value {
        Value::String(s) if is_object_ref(s) => {
            refs.insert(s.to_string());
        }
        Value::Array(values) => {
            for value in values {
                collect_object_refs_from_value(value, refs);
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                collect_object_refs_from_value(value, refs);
            }
        }
        _ => {}
    }
}

fn is_object_ref(value: &str) -> bool {
    value
        .strip_prefix("b3:")
        .map(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or(false)
}

fn collect_object_files(dir: &Path) -> DraftResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(collect_object_files(&path)?);
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(files)
}

fn object_ref_for_path(layout: &DraftLayout, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(layout.objects_dir()).ok()?;
    let parts: Vec<_> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    if parts.len() != 2 {
        return None;
    }
    let hash = format!("{}{}", parts[0], parts[1]);
    if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("b3:{hash}"))
    } else {
        None
    }
}

fn prune_empty_dirs(dir: &Path) -> DraftResult<bool> {
    if !dir.exists() || !dir.is_dir() {
        return Ok(false);
    }
    let mut empty = true;
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            if !prune_empty_dirs(&path)? {
                empty = false;
            }
        } else {
            empty = false;
        }
    }
    if empty {
        fs::remove_dir(dir)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// The change set for a ChangePack that touches nothing.
///
/// A real change set with real snapshot digests, so it carries the same
/// identity and the same validation as any other. Its emptiness is a fact about
/// the transition, not a weaker shape.
fn empty_change_set_between(
    change: &ChangePackWorkspace,
    base: &Snapshot,
    result: &Snapshot,
) -> DraftResult<ChangeSet> {
    let mut patch = ChangeSet {
        schema_version: current_version(ContractId::ChangeSet),
        id: ChangeSetId::generate(),
        base_snapshot_id: change.base_snapshot_id.clone(),
        result_snapshot_id: change.result_snapshot_id.clone(),
        base_snapshot_digest: base.snapshot_digest.clone(),
        result_snapshot_digest: result.snapshot_digest.clone(),
        observation_context_digest: base.observation_context_digest.clone(),
        resources: Vec::new(),
        derivation_gaps: Vec::new(),
        change_derivation_revision: crate::dcg::change_set::CHANGE_DERIVATION_REVISION,
        change_set_digest: String::new(),
    };
    patch = patch.seal();
    Ok(patch)
}

fn execution_status_label(status: crate::task::ExecutionStatus) -> &'static str {
    match status {
        crate::task::ExecutionStatus::Queued => "queued",
        crate::task::ExecutionStatus::Running => "running",
        crate::task::ExecutionStatus::Cancelled => "cancelled",
        crate::task::ExecutionStatus::Interrupted => "interrupted",
        crate::task::ExecutionStatus::Retrying => "retrying",
        crate::task::ExecutionStatus::Completed => "completed",
        crate::task::ExecutionStatus::Failed => "failed",
    }
}

fn inline_task_name(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in input.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 80 {
            break;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "inline-task".to_string()
    } else {
        trimmed
    }
}

#[cfg(unix)]
fn terminate_process(pid: u32) -> std::io::Result<()> {
    std::process::Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .map(|_| ())
}

#[cfg(not(unix))]
fn terminate_process(_pid: u32) -> std::io::Result<()> {
    Ok(())
}

/// One accepted change collected from an execution workspace.
#[derive(Debug, Clone)]
struct IsolatedChange {
    path: WorkspacePath,
    kind: IsolatedChangeKind,
    /// How much content the change produced, in bytes.
    bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IsolatedChangeKind {
    Added,
    Modified,
    Deleted,
}

/// Diff an execution workspace against the pre-spawn baseline snapshot.
/// `work_dir` may be the isolated copy or the real root (in-place runs).
fn collect_isolated_changes(
    real_root: &Path,
    work_dir: &Path,
    baseline: &Snapshot,
) -> DraftResult<Vec<IsolatedChange>> {
    let ignore = IgnoreMatcher::load(&DraftLayout::for_root(real_root).ignore_file())?;
    // Only the baseline's filesystem-addressed resources can be compared to
    // files on disk. Another scheme's resources are not in this directory at
    // all, and pretending otherwise would report them all as deleted.
    let baseline_by_path: BTreeMap<&str, &RawResourceState> = baseline
        .resources
        .iter()
        .filter(|state| state.locator.scheme == crate::extension::FILE_SCHEME)
        .map(|state| (state.locator.body.as_str(), state))
        .collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut changes = Vec::new();
    walk_dir(work_dir, &mut |path| {
        if path.is_dir() {
            return Ok(());
        }
        let rel = rel_path(work_dir, path)?;
        if ignore.is_ignored(rel.as_str()) {
            return Ok(());
        }
        seen.insert(rel.as_str().to_string());
        let data = fs::read(path)?;
        let hash = format!("b3:{}", blake3_hex(&data));
        match baseline_by_path.get(rel.as_str()) {
            Some(state) if state.content_digest.as_deref() == Some(hash.as_str()) => {}
            Some(_) => changes.push(IsolatedChange {
                path: rel,
                kind: IsolatedChangeKind::Modified,
                bytes: data.len() as u64,
            }),
            None => changes.push(IsolatedChange {
                path: rel,
                kind: IsolatedChangeKind::Added,
                bytes: data.len() as u64,
            }),
        }
        Ok(())
    })?;
    for body in baseline_by_path.keys() {
        if !seen.contains(*body) && !ignore.is_ignored(body) {
            changes.push(IsolatedChange {
                path: WorkspacePath::new(*body),
                kind: IsolatedChangeKind::Deleted,
                bytes: 0,
            });
        }
    }
    Ok(changes)
}

fn apply_isolated_changes(
    root: &Path,
    work_dir: &Path,
    changes: &[IsolatedChange],
) -> DraftResult<()> {
    for change in changes {
        let dest = safe_workspace_dest(root, &change.path)?;
        match change.kind {
            IsolatedChangeKind::Deleted => {
                if dest.exists() {
                    fs::remove_file(&dest).map_err(|e| {
                        DraftError::storage(format!(
                            "failed to apply deletion of {}: {e}",
                            change.path.as_str()
                        ))
                    })?;
                }
            }
            IsolatedChangeKind::Added | IsolatedChangeKind::Modified => {
                let src = work_dir.join(change.path.as_str());
                let data = fs::read(&src).map_err(|e| {
                    DraftError::storage(format!(
                        "failed to read execution output {}: {e}",
                        change.path.as_str()
                    ))
                })?;
                if let Some(parent) = dest.parent() {
                    ensure_dir(parent)?;
                }
                write_atomic(&dest, &data)?;
            }
        }
    }
    Ok(())
}

/// The anchors retained for one observed state.
///
/// A missing file is not an error: it means nothing was retained for that state,
/// and the resulting empty set reports `NotAnchored` — which is the truth, and
/// what stops a rollback claiming more than it can deliver.
/// The anchors captured alongside `snapshot`, verified against their objects.
///
/// Shared with the Baseline recoverability projection so both answer "what can
/// be restored?" from the same load and the same integrity check.
pub(crate) fn anchor_set_for(
    ws: &Workspace,
    snapshot: &Snapshot,
) -> DraftResult<crate::dcg::anchor::RecoveryAnchorSet> {
    load_anchor_set(ws, snapshot)
}

fn load_anchor_set(
    ws: &Workspace,
    snapshot: &Snapshot,
) -> DraftResult<crate::dcg::anchor::RecoveryAnchorSet> {
    let path = ws.layout.recovery_anchor_file(&snapshot.snapshot_digest);
    if !path.exists() {
        return crate::dcg::anchor::RecoveryAnchorSet::build(snapshot, Vec::new());
    }
    let set: crate::dcg::anchor::RecoveryAnchorSet = crate::contracts::read_persisted(&path)?;
    // Material referenced by a retained set must still be there. Its absence is
    // an integrity failure, not a quiet downgrade of what rollback promises.
    let store = ObjectStore::new(ws.layout.clone());
    for object in set.referenced_objects() {
        if store.get_bytes(&object).is_err() {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("recovery material {object} is missing from the object store"),
            )
            .with_suggestion(
                "run `draft doctor`; retained recovery material must not be collected",
            ));
        }
    }
    set.validate_against(snapshot)?;
    Ok(set)
}

/// Apply a Draft-authored restore plan: presence and absence both.
///
/// Draft owns the plan — the target locators, the anchor set it may draw on,
/// the preconditions and the authority. What it does not own is how an opaque
/// anchor becomes state again: that is the adapter's, and it is reached through
/// the ordinary port. Keeping a second filesystem restore here would mean two
/// implementations of one operation, and the day they disagreed the receipt
/// would still claim they had not.
fn apply_restore_plan(
    ws: &Workspace,
    plan: &crate::dcg::anchor::ResourceRestorePlan,
    anchors: &crate::dcg::anchor::RecoveryAnchorSet,
) -> DraftResult<()> {
    use crate::dcg::source::ResourceSource;
    crate::dcg::filesystem_source::FilesystemSource::new(ws).restore(plan, anchors)?;
    Ok(())
}

fn safe_workspace_dest(root: &Path, rel: &WorkspacePath) -> DraftResult<PathBuf> {
    if rel.as_str().is_empty()
        || rel.as_str().starts_with('/')
        || rel.as_str().contains('\0')
        || rel.as_str().split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || (cfg!(windows) && (part.contains(':') || part.contains('\\')))
        })
        || is_draft_path(rel.as_str())
    {
        return Err(DraftError::storage(format!(
            "unsafe workspace path '{}'",
            rel.as_str()
        )));
    }
    let root_canon = root
        .canonicalize()
        .map_err(|e| DraftError::storage(format!("cannot canonicalize workspace root: {e}")))?;
    let dest = root.join(rel.as_str());
    if let Some(parent) = dest.parent() {
        if parent.exists() {
            let parent_canon = parent.canonicalize().map_err(|e| {
                DraftError::storage(format!(
                    "cannot canonicalize rollback parent {}: {e}",
                    parent.display()
                ))
            })?;
            if !parent_canon.starts_with(&root_canon) {
                return Err(DraftError::storage(format!(
                    "rollback path escapes workspace: '{}'",
                    rel.as_str()
                )));
            }
        }
    }
    Ok(dest)
}

/// The filesystem path behind a `file`-scheme locator.
///
/// The only place Core reads a locator body as a path — and it refuses any
/// other scheme rather than treating a body that merely contains slashes as
/// one. Another scheme's resources are reached through their own adapter.
fn filesystem_relative(locator: &ResourceLocator) -> DraftResult<WorkspacePath> {
    if locator.scheme != crate::extension::FILE_SCHEME {
        return Err(DraftError::invalid_config(format!(
            "the '{}' scheme is handled by its own adapter, not by Draft's filesystem path",
            locator.scheme
        )));
    }
    Ok(WorkspacePath::new(
        crate::support::pathguard::check_relative(&locator.body).map_err(|error| {
            DraftError::new(
                DraftErrorKind::ProtectedResourceAccess,
                format!("unsafe locator body '{}': {error}", locator.body),
            )
        })?,
    ))
}

/// The same, additionally refused when the resource is protected.
fn checked_resource_path(
    protections: &[crate::project::protected::ProtectionRule],
    locator: &ResourceLocator,
) -> DraftResult<WorkspacePath> {
    let rel = filesystem_relative(locator)?;
    crate::project::protected::ensure_allowed(protections, &rel)?;
    Ok(rel)
}

fn editor_backup_path(root: &Path, rel: &WorkspacePath) -> DraftResult<PathBuf> {
    let project_paths = crate::project::layout::DraftLayout::for_root(root);
    Ok(project_paths.workspaces_dir().join("backups").join(format!(
        "{}-{}",
        now().timestamp_millis(),
        rel.as_str().replace('/', "__")
    )))
}

#[derive(Debug)]
struct HookContext {
    message: String,
    title: String,
    description: String,
    task_id: String,
    execution_id: String,
    change_pack_id: String,
    receipt_id: String,
    actor_name: String,
    timestamp: String,
    verified: String,
    risk_level: String,
    files_changed: String,
    workspace_root: String,
    hook_name: String,
    hook_phase: String,
    vars: BTreeMap<String, String>,
}

#[derive(Debug)]
struct HookFailure {
    message: String,
}

fn run_hook(
    ws: &Workspace,
    store: &ObjectStore,
    hook_name: &str,
    hook: &HookEntry,
    ctx: &HookContext,
) -> Result<HookResult, HookFailure> {
    let mut values = hook_values(ctx);
    for (k, v) in &ctx.vars {
        values.insert(k.clone(), v.clone());
    }
    let command = interpolate_strict(&hook.command, &values)?;
    let resolved_cwd = match hook.cwd.as_str() {
        "workspace" | "" => ws.root.clone(),
        other => resolve_hook_cwd(&ws.root, other)?,
    };
    let cwd = resolved_cwd.canonicalize().unwrap_or(resolved_cwd);
    ensure_workspace_child(&ws.root, &cwd)?;
    let mut env = hook_env(ctx);
    for (k, v) in &hook.env {
        if k.starts_with("DRAFT_") {
            return Err(HookFailure {
                message: format!("hook env key '{k}' cannot override Draft-managed variables"),
            });
        }
        env.insert(k.clone(), v.clone());
    }
    let mut env_keys: Vec<String> = env.keys().cloned().collect();
    env_keys.sort();
    let shell = resolve_hook_shell(&hook.shell)?;
    let hash = command_hash(&shell.name, &cwd, &command, &ctx.message);
    let started_at = now();
    let out = shell_with_env_timeout(&command, &cwd, &env, hook.timeout_ms, &shell);
    let ended_at = now();
    let (exit_code, stdout, stderr) = match out {
        Ok(o) => (o.status.code().unwrap_or(-1), o.stdout, o.stderr),
        Err(e) => (-1, Vec::new(), e.to_string().into_bytes()),
    };
    let stdout = sanitize_output_bytes(&stdout);
    let stderr = sanitize_output_bytes(&stderr);
    let stdout_ref = store
        .put_bytes(&stdout)
        .map_err(|e| HookFailure { message: e.message })?;
    let stderr_ref = store
        .put_bytes(&stderr)
        .map_err(|e| HookFailure { message: e.message })?;
    Ok(HookResult {
        hook_name: hook_name.to_string(),
        hook_phase: hook.phase.clone(),
        shell: shell.name,
        working_dir: cwd.display().to_string(),
        command_hash: hash,
        exit_code,
        stdout_ref,
        stderr_ref,
        started_at,
        ended_at,
        env_keys,
    })
}

fn resolve_hook_cwd(root: &Path, configured: &str) -> Result<PathBuf, HookFailure> {
    let relative = Path::new(configured);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(HookFailure {
            message: "hook cwd must be a relative path inside the workspace".to_string(),
        });
    }
    Ok(root.join(relative))
}

fn ensure_workspace_child(root: &Path, path: &Path) -> Result<(), HookFailure> {
    let root = root.canonicalize().map_err(|e| HookFailure {
        message: format!("cannot canonicalize workspace root: {e}"),
    })?;
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !path.starts_with(&root) {
        return Err(HookFailure {
            message: "hook cwd escapes workspace".to_string(),
        });
    }
    Ok(())
}

fn hook_values(ctx: &HookContext) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("message".to_string(), ctx.message.clone()),
        ("title".to_string(), ctx.title.clone()),
        ("description".to_string(), ctx.description.clone()),
        ("task_id".to_string(), ctx.task_id.clone()),
        ("execution_id".to_string(), ctx.execution_id.clone()),
        ("change_pack_id".to_string(), ctx.change_pack_id.clone()),
        ("receipt_id".to_string(), ctx.receipt_id.clone()),
        ("actor_name".to_string(), ctx.actor_name.clone()),
        ("timestamp".to_string(), ctx.timestamp.clone()),
        ("verified".to_string(), ctx.verified.clone()),
        ("risk_level".to_string(), ctx.risk_level.clone()),
        ("files_changed".to_string(), ctx.files_changed.clone()),
        ("workspace_root".to_string(), ctx.workspace_root.clone()),
        ("hook_name".to_string(), ctx.hook_name.clone()),
        ("hook_phase".to_string(), ctx.hook_phase.clone()),
    ])
}

fn hook_env(ctx: &HookContext) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("DRAFT_HOOK_NAME".to_string(), ctx.hook_name.clone());
    env.insert("DRAFT_HOOK_PHASE".to_string(), ctx.hook_phase.clone());
    env.insert(
        "DRAFT_WORKSPACE_ROOT".to_string(),
        ctx.workspace_root.clone(),
    );
    env.insert("DRAFT_RECEIPT_ID".to_string(), ctx.receipt_id.clone());
    env.insert("DRAFT_PACK_ID".to_string(), ctx.change_pack_id.clone());
    env.insert("DRAFT_ACTOR_NAME".to_string(), ctx.actor_name.clone());
    for (k, v) in &ctx.vars {
        env.insert(format!("DRAFT_VAR_{}", k.to_ascii_uppercase()), v.clone());
    }
    env
}

pub fn parse_hook_vars(values: Vec<String>) -> DraftResult<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for item in values {
        if item.starts_with('-') {
            return Err(DraftError::invalid_config(
                "normal Draft flags are not allowed after --var",
            ));
        }
        let (key, value) = item
            .split_once('=')
            .ok_or_else(|| DraftError::invalid_config("--var entries must be key=value"))?;
        if !valid_var_name(key) {
            return Err(DraftError::invalid_config(format!(
                "invalid hook variable name '{key}'"
            )));
        }
        if builtin_placeholder_names().contains(key) {
            return Err(DraftError::invalid_config(format!(
                "hook variable '{key}' overrides a built-in placeholder"
            )));
        }
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

fn valid_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn builtin_placeholder_names() -> BTreeSet<&'static str> {
    BTreeSet::from([
        "message",
        "title",
        "description",
        "task_id",
        "execution_id",
        "change_pack_id",
        "receipt_id",
        "actor_name",
        "timestamp",
        "verified",
        "risk_level",
        "files_changed",
        "workspace_root",
        "hook_name",
        "hook_phase",
    ])
}

fn interpolate_strict(
    template: &str,
    values: &BTreeMap<String, String>,
) -> Result<String, HookFailure> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| HookFailure {
            message: "unclosed hook placeholder".to_string(),
        })?;
        let name = &after[..end];
        let value = values.get(name).ok_or_else(|| HookFailure {
            message: format!("missing hook placeholder '{{{{{name}}}}}'"),
        })?;
        out.push_str(value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn shell_with_env_timeout(
    command: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    timeout_ms: Option<u64>,
    shell: &HookShell,
) -> std::io::Result<std::process::Output> {
    if timeout_ms.is_none() {
        return shell_with_env_unbounded(command, cwd, env, shell);
    }
    let mut cmd = shell.command(command);
    let mut child = cmd
        .current_dir(cwd)
        .envs(env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let timeout = Duration::from_millis(timeout_ms.unwrap_or_default());
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return child.wait_with_output();
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("hook timed out after {} ms", timeout.as_millis()),
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn shell_with_env_unbounded(
    command: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    shell: &HookShell,
) -> std::io::Result<std::process::Output> {
    let mut cmd = shell.command(command);
    cmd.current_dir(cwd).envs(env).output()
}

#[derive(Debug, Clone)]
struct HookShell {
    name: String,
    program: String,
    args_before_command: Vec<String>,
}

impl HookShell {
    fn command(&self, command: &str) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args_before_command).arg(command);
        cmd
    }
}

fn resolve_hook_shell(name: &str) -> Result<HookShell, HookFailure> {
    let normalized = name.trim().to_ascii_lowercase();
    let normalized = normalized.as_str();
    if normalized.is_empty() || normalized == "default" {
        return Ok(default_hook_shell_runtime());
    }
    match normalized {
        "cmd" | "cmd.exe" => {
            if cfg!(windows) {
                Ok(HookShell {
                    name: "cmd.exe /S /C".to_string(),
                    program: "cmd".to_string(),
                    args_before_command: vec!["/S".to_string(), "/C".to_string()],
                })
            } else {
                Err(HookFailure {
                    message: "hook shell 'cmd' is only supported on Windows".to_string(),
                })
            }
        }
        "sh" => Ok(HookShell {
            name: "sh -c".to_string(),
            program: "sh".to_string(),
            args_before_command: vec!["-c".to_string()],
        }),
        other => Err(HookFailure {
            message: format!("unsupported hook shell '{other}'"),
        }),
    }
}

fn sanitize_output_bytes(bytes: &[u8]) -> Vec<u8> {
    const MAX_CAPTURED_OUTPUT: usize = 1024 * 1024;
    let mut text = String::from_utf8_lossy(bytes).to_string();
    if text.len() > MAX_CAPTURED_OUTPUT {
        text.truncate(MAX_CAPTURED_OUTPUT);
        text.push_str("\n[Draft output truncated]\n");
    }
    redact_secrets(&text).into_bytes()
}

fn default_hook_shell_runtime() -> HookShell {
    if cfg!(windows) {
        HookShell {
            name: "cmd.exe /S /C".to_string(),
            program: "cmd".to_string(),
            args_before_command: vec!["/S".to_string(), "/C".to_string()],
        }
    } else {
        HookShell {
            name: "sh -c".to_string(),
            program: "sh".to_string(),
            args_before_command: vec!["-c".to_string()],
        }
    }
}

fn command_hash(shell: &str, cwd: &Path, command: &str, rendered: &str) -> String {
    sha256_hex(format!("{shell}\n{}\n{command}\n{rendered}", cwd.display()).as_bytes())
}

fn hash_json<T: Serialize>(value: &T) -> DraftResult<String> {
    try_canonical_hash(value)
}

fn json_err(e: serde_json::Error) -> DraftError {
    DraftError::storage(format!("JSON error: {e}"))
}

impl From<serde_json::Error> for DraftError {
    fn from(e: serde_json::Error) -> Self {
        json_err(e)
    }
}

// ---------------------------------------------------------------------------
// The authoritative ChangePack lifecycle
//
// A ChangePack's lifecycle record answers "how far through review is this revision".
// A ChangePack record answers "is this work still open". Those were one value, and
// separating them is what lets a change be abandoned — a decision the ChangePack
// model had no way to express, since leaving it in Draft claims it is still
// being worked on and moving it to Rejected claims a reviewer turned it down.
//
// While both exist, the ChangePack record still drives the review surfaces and the
// ChangePack record is authoritative for the work lifecycle. Readers move over
// before the ChangePack record is retired.
// ---------------------------------------------------------------------------

/// The ChangePack store for a project.
/// Protections contributed by installed extensions, reduced to project rules.
///
/// `project` cannot reach the extension layer to gather these, so the
/// conversion happens here — above both — and the rules are handed down.
fn contributed_protections(
    contributions: &crate::extension::ActiveContributions,
) -> Vec<crate::project::protected::ProtectionRule> {
    contributions
        .policies
        .iter()
        .flat_map(|preset| {
            preset
                .value
                .control_policy
                .protections
                .iter()
                .map(move |rule| crate::project::protected::ProtectionRule {
                    predicate: rule.predicate.clone(),
                    reason: rule.reason.clone(),
                    source: crate::project::protected::ProtectionSource::Extension {
                        extension_id: preset.extension_id.clone(),
                    },
                })
        })
        .collect()
}

#[cfg(test)]
mod app_tests {
    #[test]
    fn pack_identities_are_derived_from_their_frozen_seeds() {
        let seed_digest = |seed: &str| {
            draft_dcg_contract::Digest::of_bytes(seed.as_bytes())
                .as_str()
                .rsplit(':')
                .next()
                .unwrap()[..24]
                .to_string()
        };
        let base = "bas_000000000001";
        let change_pack = super::change_pack_id_for(base, "tidy the docs").unwrap();
        assert_eq!(
            change_pack.as_str(),
            format!("cpk_{}", seed_digest("bas_000000000001|tidy the docs"))
        );
        // The same request converges on the same ChangePack.
        assert_eq!(
            super::change_pack_id_for(base, "tidy the docs").unwrap(),
            change_pack
        );

        let root = "sha256:0011";
        let revision = super::revision_pack_id_for(&change_pack, root).unwrap();
        assert_eq!(
            revision.as_str(),
            format!("rpk_{}", seed_digest(&format!("{change_pack}|{root}")))
        );
        // The same state under a different ChangePack is a different RevisionPack.
        let other = super::change_pack_id_for(base, "something else").unwrap();
        assert_ne!(super::revision_pack_id_for(&other, root).unwrap(), revision);
        // retired-architecture-ok: the retired family must not parse as a RevisionPack.
        assert!(draft_dcg_contract::ids::RevisionPackId::parse(
            revision.as_str().replacen("rpk_", "rev_", 1)
        )
        .is_err());
    }

    #[test]
    fn no_production_derivation_uses_a_retired_prefix() {
        let source = include_str!("mod.rs");
        let code: String = source
            .lines()
            .filter(|line| !line.contains("retired-architecture-ok"))
            .collect();
        for retired in ["\"chg_\"", "\"rev_\""] {
            assert!(
                !code.contains(&format!("derived_id({retired}")),
                "{retired}"
            );
        }
    }

    /// Accept an initial Baseline for a test project.
    ///
    /// The same path production takes, so a test fixture cannot drift into a
    /// state production could never produce.
    fn accept_initial_baseline_for_tests(app: &App, root: &std::path::Path) {
        // Acceptance records *who* accepted, which the retired stable head
        // never did — so a fixture needs the actor state a real project has.
        let home = crate::project::home::DraftGlobalStore::locate()
            .expect("a Draft global store is locatable in tests");
        crate::trust::identity::global::ensure_actor(&home).expect("a test actor identity exists");
        let layout = crate::project::layout::DraftLayout::for_root(root);
        let workspace = Workspace {
            workspace_id: crate::contracts::read_persisted::<WorkspaceMetadata>(
                &layout.project_json(),
            )
            .expect("test project metadata is readable")
            .workspace_id,
            root: root.to_path_buf(),
            layout: layout.clone(),
        };
        crate::app::baseline::accept_current(
            app,
            &workspace,
            crate::dcg::baseline::BaselineOrigin::Initial,
        )
        .expect("a test project accepts its initial Baseline");
    }

    use super::*;

    use crate::project::home::ScopedGlobalHome;

    fn manifest_for(
        candidate: Option<&str>,
        change_pack_id: &str,
    ) -> crate::dcg::change_pack_store::ChangePackManifest {
        crate::dcg::change_pack_store::ChangePackManifest {
            schema_version: current_version(ContractId::ChangePackManifest),
            change_pack_id: change_pack_id.to_string(),
            manifest_digest: String::new(),
            name: change_pack_id.to_string(),
            description: String::new(),
            intent: crate::dcg::change_pack_store::unspecified_intent(),
            provenance: serde_json::json!({"origin": "test"}),
            author_id: "act_t".into(),
            candidate_id: candidate.map(|c| c.to_string()),
            declared_dependencies: Vec::new(),
            created_at: "2026-07-04T00:00:00+00:00".into(),
        }
    }

    #[test]
    fn hex_decode_validates_pairs_and_digits() {
        assert_eq!(
            crate::support::hashing::hex_decode("00aFff").unwrap(),
            vec![0x00, 0xaf, 0xff]
        );
        assert_eq!(
            crate::support::hashing::hex_decode("0")
                .unwrap_err()
                .message,
            "invalid hex length"
        );
        assert_eq!(
            crate::support::hashing::hex_decode("0g")
                .unwrap_err()
                .message,
            "invalid hex byte"
        );
    }

    #[test]
    fn doctor_reports_recoverable_operations() {
        let tmp = tempfile::tempdir().unwrap();
        let app = App::new();
        let layout = DraftLayout::for_root(tmp.path());
        layout.create_all().unwrap();
        let project_paths = crate::project::layout::DraftLayout::for_root(tmp.path());
        project_paths.create_all().unwrap();
        let id = crate::project::mint_project_id();
        write_json(
            &layout.project_json(),
            &WorkspaceMetadata {
                schema_version: current_version(ContractId::WorkspaceMetadata),
                workspace_id: id.clone(),
                draft_version: crate::DRAFT_VERSION.to_string(),
                created_at: now(),
            },
        )
        .unwrap();
        crate::execution::operation::RecoveryStore::for_root(tmp.path())
            .start("doctor.recovery-test", None, serde_json::json!({}))
            .unwrap();

        let ws = Workspace {
            workspace_id: id,
            root: tmp.path().to_path_buf(),
            layout,
        };
        let project = app.doctor_project_scope(&ws).unwrap();
        let check = project
            .checks
            .iter()
            .find(|check| check.name == "recovery")
            .unwrap();
        assert!(!check.ok);
        assert!(check.detail.contains("interrupted operation"));
    }

    #[test]
    fn task_view_uses_shared_status_display() {
        let tmp = tempfile::tempdir().unwrap();
        let app = App::new();
        let layout = DraftLayout::for_root(tmp.path());
        layout.create_all().unwrap();
        let project_paths = crate::project::layout::DraftLayout::for_root(tmp.path());
        project_paths.create_all().unwrap();
        write_json(
            &layout.project_json(),
            &WorkspaceMetadata {
                schema_version: current_version(ContractId::WorkspaceMetadata),
                workspace_id: crate::project::mint_project_id(),
                draft_version: crate::DRAFT_VERSION.to_string(),
                created_at: now(),
            },
        )
        .unwrap();
        let task = crate::task::TaskDefinition::new(
            "review-docs".into(),
            "Review the docs".into(),
            "head".into(),
            "human".into(),
        )
        .unwrap();
        crate::task::TaskStore::for_root(tmp.path())
            .create(&task)
            .unwrap();
        accept_initial_baseline_for_tests(&app, tmp.path());

        let view = app.task_view(tmp.path(), "review-docs").unwrap();
        assert_eq!(view.health, crate::task::TaskViewStatus::Defined);
        assert_eq!(view.review_status, crate::task::TaskViewStatus::Pending);
        assert!(view.recommended_action.contains("draft task spawn"));
    }

    #[test]
    fn inbox_includes_failed_execution_and_recoverable_operation() {
        let tmp = tempfile::tempdir().unwrap();
        let app = App::new();
        let layout = DraftLayout::for_root(tmp.path());
        layout.create_all().unwrap();
        let project_paths = crate::project::layout::DraftLayout::for_root(tmp.path());
        project_paths.create_all().unwrap();
        write_json(
            &layout.project_json(),
            &WorkspaceMetadata {
                schema_version: current_version(ContractId::WorkspaceMetadata),
                workspace_id: crate::project::mint_project_id(),
                draft_version: crate::DRAFT_VERSION.to_string(),
                created_at: now(),
            },
        )
        .unwrap();
        accept_initial_baseline_for_tests(&app, tmp.path());
        let task = crate::task::TaskDefinition::new(
            "fix-build".into(),
            "Fix the failing build".into(),
            "head".into(),
            "human".into(),
        )
        .unwrap();
        crate::task::TaskStore::for_root(tmp.path())
            .create(&task)
            .unwrap();
        let execution = crate::task::Execution::queued(
            &task,
            "codex".into(),
            vec!["example-test".into(), "--all".into()],
        );
        let execution_store = crate::task::ExecutionStore::for_root(tmp.path());
        execution_store.write(&execution).unwrap();
        execution_store
            .mark_failed(execution.id.as_str(), "tests failed")
            .unwrap();
        crate::execution::operation::RecoveryStore::for_root(tmp.path())
            .start(
                "doctor.recovery-test",
                Some("draft".into()),
                serde_json::json!({}),
            )
            .unwrap();

        let inbox = app.inbox(tmp.path()).unwrap();
        assert!(inbox.iter().any(
            |item| item.kind == "execution_failed" && item.subject_id == execution.id.as_str()
        ));
        assert!(inbox
            .iter()
            .any(|item| item.kind == "doctor_warning" && item.status == "needs_recovery"));
    }

    #[test]
    fn resource_lifecycle_uses_guards_backups_and_search() {
        let tmp = tempfile::tempdir().unwrap();
        // Editing resolves the stable security actor, so this test needs a
        // global store of its own rather than whatever one happens to exist.
        let global = tempfile::tempdir().unwrap();
        let _global = ScopedGlobalHome::set(global.path().join(".draft-global"));
        let app = App::new();
        app.init_global().unwrap();
        let layout = DraftLayout::for_root(tmp.path());
        layout.create_all().unwrap();
        let project_paths = crate::project::layout::DraftLayout::for_root(tmp.path());
        project_paths.create_all().unwrap();
        write_json(
            &layout.project_json(),
            &WorkspaceMetadata {
                schema_version: current_version(ContractId::WorkspaceMetadata),
                workspace_id: crate::project::mint_project_id(),
                draft_version: crate::DRAFT_VERSION.to_string(),
                created_at: now(),
            },
        )
        .unwrap();
        accept_initial_baseline_for_tests(&app, tmp.path());

        let source = ResourceLocator::file("src/notes.txt");
        let renamed_to = ResourceLocator::file("src/renamed.txt");

        let created = app
            .resource_create(tmp.path(), &source, "needle\n")
            .unwrap();
        assert_eq!(created.action, "created");
        assert_eq!(
            app.resource_search(tmp.path(), "needle", 10).unwrap()[0].locator,
            source
        );

        let relocated = app
            .resource_relocate(tmp.path(), &source, &renamed_to)
            .unwrap();
        assert_eq!(relocated.previous_locator.as_ref(), Some(&source));
        assert!(tmp.path().join("src/renamed.txt").exists());

        let deleted = app.resource_delete(tmp.path(), &renamed_to).unwrap();
        assert_eq!(deleted.action, "deleted");
        assert!(deleted.backup_path.is_some());
        assert!(!tmp.path().join("src/renamed.txt").exists());

        // The control plane can never be reached through a resource locator,
        // whatever scheme it claims.
        let err = app
            .resource_create(tmp.path(), &ResourceLocator::file(".draft/owned.txt"), "")
            .unwrap_err();
        assert!(matches!(
            err.kind,
            DraftErrorKind::ProtectedResourceAccess | DraftErrorKind::Storage
        ));
        let foreign = ResourceLocator {
            scheme: "example.catalog".into(),
            body: "sku/A-100".into(),
        };
        assert!(
            app.resource_create(tmp.path(), &foreign, "").is_err(),
            "another scheme's resources go through their own adapter"
        );
    }

    #[test]
    fn global_config_get_unset_are_scoped_to_global_home() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join(".draft-global");
        let _global = ScopedGlobalHome::set(&global);
        let app = App::new();

        app.config_set_global("risk.block_on_critical", "false")
            .unwrap();
        let report = app.config_get_global("risk.block_on_critical").unwrap();
        assert_eq!(
            report
                .entries
                .get("risk.block_on_critical")
                .map(String::as_str),
            Some("false")
        );
        app.config_unset_global("risk.block_on_critical").unwrap();
        let report = app.config_get_global("risk.block_on_critical").unwrap();
        assert_eq!(
            report
                .entries
                .get("risk.block_on_critical")
                .map(String::as_str),
            Some("")
        );
    }

    fn tree_digest(root: &Path) -> String {
        if !root.exists() {
            return sha256_hex(b"absent");
        }
        let mut entries = walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| {
                let relative = entry.path().strip_prefix(root).unwrap().to_path_buf();
                (relative, std::fs::read(entry.path()).unwrap())
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut bytes = Vec::new();
        for (path, content) in entries {
            bytes.extend_from_slice(path.to_string_lossy().as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(&content);
            bytes.push(0xff);
        }
        sha256_hex(&bytes)
    }

    #[test]
    fn user_profile_changes_preserve_all_preexisting_security_and_canonical_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("source.txt"), "canonical source\n").unwrap();
        let global_root = temp.path().join("global");
        let _guard = ScopedGlobalHome::set(&global_root);
        let app = App::new();
        let initialized = app.init(&root).unwrap();
        let home = crate::project::home::DraftGlobalStore::at(&global_root);
        let actor = crate::trust::identity::global::load_actor(&home)
            .unwrap()
            .unwrap();
        let candidate = crate::trust::identity::global::register_candidate(
            &home,
            "profile-invariance-candidate",
            crate::trust::identity::CandidateKind::Ai,
            "test",
        )
        .unwrap();

        let workspace = app.open(&root).unwrap();
        let mut manifest = manifest_for(Some(&candidate.candidate_id), "cpk_profile_invariance");
        manifest.author_id = actor.actor_id.clone();
        manifest.refresh_manifest_digest();
        let change_pack_store =
            crate::dcg::change_pack_store::ChangePackContentStore::new(workspace.layout.clone());
        change_pack_store.write_manifest(&manifest).unwrap();
        crate::support::fsutil::write_json(
            &home.revoked_keys_json(),
            &serde_json::json!({
                "schema_version": current_version(ContractId::RevokedKeyRegistry),
                "public_key_ids": [],
            }),
        )
        .unwrap();
        crate::activity::GlobalAuditLog::global()
            .unwrap()
            .append(
                crate::activity::GlobalAuditEvent::ExtensionSourceConfigured,
                Some(actor.actor_id.clone()),
                Some("profile-invariance".into()),
                None,
                serde_json::json!({}),
            )
            .unwrap();

        assert!(
            crate::read_model::integrity::verify_all(
                &crate::project::layout::DraftLayout::for_root(&root),
                &draft_dcg_contract::ids::ProjectId::parse(&initialized.workspace_id).unwrap(),
            )
            .unwrap()
            .all_ok
        );

        let layout = crate::project::layout::DraftLayout::for_root(&root);
        let actor_bytes = std::fs::read(home.actor_json()).unwrap();
        let signing_key_bytes = std::fs::read(home.signing_key()).unwrap();
        let public_keys_digest = tree_digest(&home.public_keys_dir());
        let trust_digest = tree_digest(&home.trust_dir());
        let candidate_registry_bytes = std::fs::read(home.candidates_json()).unwrap();
        let manifest_bytes =
            std::fs::read(layout.change_pack_manifest(manifest.change_pack_id.as_str())).unwrap();
        let events_before = app.events(&root).unwrap();
        let event_log_before = std::fs::read(layout.activity_log()).unwrap();
        let receipts_before = crate::receipt::ReceiptEnvelopeStore::for_layout(&layout)
            .read_all()
            .unwrap();
        let receipts_digest = tree_digest(&layout.receipts_dir());
        let source_policy = crate::dcg::source_view::CanonicalSourcePolicy::default();
        let source_digest =
            crate::dcg::source_view::CanonicalSourceView::build(&root, &source_policy)
                .unwrap()
                .content_digest;
        let workspace_digest =
            crate::dcg::source_view::workspace_hash(&root, &Default::default()).unwrap();
        let ownership_before =
            crate::project::ownership::evaluate(&root, &["source.txt".into()], &["@owner".into()])
                .unwrap();
        let policy_before = crate::project::policy::Policy::resolve(
            Some(&layout.policy_toml()),
            Some(&home.default_policy_toml()),
        )
        .unwrap();
        let audit_before = crate::activity::GlobalAuditLog::global()
            .unwrap()
            .read_all()
            .unwrap();

        app.config_update_user_global(Some(Some("Global User")), Some(Some("global@example.test")))
            .unwrap();
        app.config_set(&root, "user.name", "Project User").unwrap();
        app.config_set(&root, "user.email", "project@example.test")
            .unwrap();
        app.config_unset(&root, "user.email").unwrap();
        app.config_unset_global("user.email").unwrap();

        let resolved = ResolvedConfig::load(&app.open(&root).unwrap()).unwrap();
        assert_eq!(resolved.user_name, "Project User");
        assert_eq!(resolved.user_email, None);
        assert_eq!(std::fs::read(home.actor_json()).unwrap(), actor_bytes);
        assert_eq!(
            std::fs::read(home.signing_key()).unwrap(),
            signing_key_bytes
        );
        assert_eq!(tree_digest(&home.public_keys_dir()), public_keys_digest);
        assert_eq!(tree_digest(&home.trust_dir()), trust_digest);
        assert_eq!(
            std::fs::read(home.candidates_json()).unwrap(),
            candidate_registry_bytes
        );
        assert_eq!(
            std::fs::read(layout.change_pack_manifest(manifest.change_pack_id.as_str())).unwrap(),
            manifest_bytes
        );
        assert_eq!(
            manifest.candidate_id.as_deref(),
            Some(candidate.candidate_id.as_str())
        );
        assert_eq!(tree_digest(&layout.receipts_dir()), receipts_digest);
        assert_eq!(
            crate::receipt::ReceiptEnvelopeStore::for_layout(&layout)
                .read_all()
                .unwrap(),
            receipts_before
        );
        assert_eq!(
            crate::dcg::source_view::CanonicalSourceView::build(&root, &source_policy)
                .unwrap()
                .content_digest,
            source_digest
        );
        assert_eq!(
            crate::dcg::source_view::workspace_hash(&root, &Default::default()).unwrap(),
            workspace_digest
        );
        assert_eq!(
            crate::project::ownership::evaluate(&root, &["source.txt".into()], &["@owner".into()],)
                .unwrap(),
            ownership_before
        );
        assert_eq!(
            crate::project::policy::Policy::resolve(
                Some(&layout.policy_toml()),
                Some(&home.default_policy_toml()),
            )
            .unwrap(),
            policy_before
        );

        let events_after = app.events(&root).unwrap();
        assert_eq!(
            &events_after[..events_before.len()],
            events_before.as_slice()
        );
        assert!(std::fs::read(layout.activity_log())
            .unwrap()
            .starts_with(&event_log_before));
        for event in &events_after[events_before.len()..] {
            assert_eq!(event.actor, actor.actor_id);
            assert_eq!(event.kind, "PolicyUpdated");
            assert_eq!(event.metadata["scope"], "project");
            assert!(event.metadata.get("changed_keys").is_some());
            assert!(event.metadata.get("resulting_config_digest").is_some());
            let serialized = serde_json::to_string(event).unwrap();
            assert!(!serialized.contains("Project User"));
            assert!(!serialized.contains("project@example.test"));
        }
        assert!(
            crate::read_model::integrity::verify_all(
                &layout,
                &draft_dcg_contract::ids::ProjectId::parse(&initialized.workspace_id).unwrap(),
            )
            .unwrap()
            .all_ok
        );

        let audit_after = crate::activity::GlobalAuditLog::global()
            .unwrap()
            .read_all()
            .unwrap();
        assert_eq!(&audit_after[..audit_before.len()], audit_before.as_slice());
        for event in &audit_after[audit_before.len()..] {
            let serialized = serde_json::to_string(event).unwrap();
            assert!(!serialized.contains("Global User"));
            assert!(!serialized.contains("global@example.test"));
            assert!(serialized.contains("changed_keys"));
            assert!(serialized.contains("resulting_config_digest"));
        }
    }
}
