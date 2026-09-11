//! Rust-owned DTOs for the Draft Console HTTP contract.
//!
//! The generator binary emits committed TypeScript declarations and JSON
//! Schema from these definitions. Gateway and browser code never own a second
//! copy of the canonical transport shapes.

use crate::console_application::{
    ActionInputField, ActionInputKind, ActionPresentation, ActionTarget, ActionTargetKind,
    CanonicalRevisions, ConsoleActionInvocation, ConsoleNavigationSection, ConsoleReadModel,
    ConsoleScope, ConsoleSubject, ModelFreshness, NextSafeAction, SelectOption,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "Session")]
pub struct ConsoleSessionDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub authenticated: bool,
    pub csrf_token: String,
    pub preselected_workspace_id: Option<String>,
}

impl draft_core::contracts::VersionedContract for ConsoleSessionDto {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::ConsoleSession;
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "WorkspaceRevision")]
pub struct WorkspaceRevisionDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub workspace_id: String,
    pub content_digest: String,
}

impl draft_core::contracts::VersionedContract for WorkspaceRevisionDto {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::ConsoleWorkspaceRevision;
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "RegistryProject")]
pub struct RegistryProjectDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub workspace_id: String,
    pub name: String,
    pub project_path: String,
    pub storage_path: String,
    pub created_at: String,
    pub last_seen_at: String,
    pub draft_version: String,
    pub health: String,
    #[ts(type = "number")]
    pub location_revision: u64,
    pub last_workspace_revision: Option<String>,
}

impl draft_core::contracts::VersionedContract for RegistryProjectDto {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::ConsoleRegistryProject;
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssigneeRefDto {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct TaskNextActionDto {
    pub id: String,
    pub label: String,
    pub completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "TaskDefinition")]
pub struct TaskDefinitionDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub goal: String,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub due_at: Option<String>,
    pub risk: String,
    pub mode: String,
    pub assignee_ref: Option<AssigneeRefDto>,
    pub next_actions: Vec<TaskNextActionDto>,
    pub updated_at: String,
}

impl draft_core::contracts::VersionedContract for TaskDefinitionDto {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::ConsoleTaskDefinition;
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "TaskView")]
pub struct TaskViewDto {
    pub task: TaskDefinitionDto,
    pub health: String,
    pub review_status: String,
    pub recommended_action: String,
    #[ts(type = "number")]
    pub execution_count: u64,
    #[ts(type = "number")]
    pub evidence_count: u64,
    pub produced_changes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "ChangeSummary")]
pub struct ChangeSummaryDto {
    pub change_id: String,
    pub name: String,
    pub intent: String,
    pub submit_state: String,
    pub import_state: Option<String>,
    #[ts(type = "number | string | null")]
    pub revision: Option<serde_json::Value>,
    pub valid_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "InboxItem")]
pub struct InboxItemDto {
    pub id: Option<String>,
    pub status: String,
    pub kind: String,
    pub subject_id: String,
    pub next_action: String,
    pub severity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "ProjectSummary")]
pub struct ProjectSummaryDto {
    pub project: RegistryProjectDto,
    pub revision: WorkspaceRevisionDto,
    #[ts(type = "Record<string, unknown>")]
    pub status: serde_json::Value,
    pub tasks: Vec<TaskDefinitionDto>,
    pub changes: Vec<ChangeSummaryDto>,
    pub inbox: Vec<InboxItemDto>,
}

/// Where a resource lives, in terms only its owning adapter understands.
///
/// The Console never parses `body`. A `file`-scheme body looks like a path
/// because that adapter chose paths; a catalog or timeline adapter's does not,
/// and the same components must render both.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "ResourceLocator")]
pub struct ResourceLocatorDto {
    pub scheme: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "Resource")]
pub struct ResourceDto {
    pub locator: ResourceLocatorDto,
    pub resource_id: String,
    /// The intrinsic shape of the resource, when its adapter states one.
    #[serde(default)]
    pub form: Option<String>,
    pub protected: bool,
    #[serde(default)]
    #[ts(type = "number | null")]
    pub content_size: Option<u64>,
    /// **Every** class an installed extension assigns, sorted.
    ///
    /// A list, not one value: a resource genuinely is several things at once —
    /// a text document *and* a language source — and rendering only the first
    /// would discard a correct classification. Empty means nothing is installed
    /// that recognizes it, which is not an error.
    #[serde(default)]
    pub classes: Vec<String>,
    /// Classes installed extensions define incompatibly, scoped to those
    /// classes. Every other assignment on this resource still stands.
    #[serde(default)]
    pub class_collisions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "ResourceContent")]
pub struct ResourceContentDto {
    pub locator: ResourceLocatorDto,
    pub content: String,
    pub protected: bool,
    pub workspace_hash: Option<String>,
}

/// What classes a project's resources carry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "ClassificationBundle")]
pub struct ClassificationBundleDto {
    pub assigned: Vec<String>,
    pub collisions: Vec<String>,
    pub classification_digest: String,
    /// Present when nothing is installed to classify anything. Distinguishes
    /// "classified, and nothing matched" from "nothing can classify".
    #[serde(default)]
    pub gaps: Vec<CapabilityGapDto>,
}

/// A capability nothing installed can supply.
///
/// Never names a package: Draft reports what is missing, and does not
/// advertise on any publisher's behalf.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "CapabilityGap")]
pub struct CapabilityGapDto {
    pub capability: String,
    #[serde(default)]
    pub resource_classes: Vec<String>,
    pub reason: String,
}

/// One check's result, in the five-state model.
///
/// `state` is one of `passed`, `failed`, `unavailable`, `not_evaluated` or
/// `not_applicable`. A surface that collapses these to a boolean would let a
/// reader mistake "nothing checked this" for "everything passed".
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "VerificationCheckResult")]
pub struct VerificationCheckResultDto {
    pub check_id: String,
    pub display_name: String,
    pub requirement: String,
    pub state: String,
    #[serde(default)]
    pub detail: Option<String>,
    pub reason: String,
    /// The artifact that contributed this check, shown separately from the
    /// decision that permitted it: trust and authorization are different facts.
    pub producer_extension_id: String,
    #[serde(default)]
    pub authorization_decision: Option<String>,
}

/// How two changes relate over one resource they both touch.
///
/// `relation` is `independent`, `conflicting` or `indeterminate`.
/// `indeterminate` is shown as itself — "Draft cannot tell whether these are
/// separable" is a different statement from "they collide", even though both
/// block composition.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "ResourceInterference")]
pub struct ResourceInterferenceDto {
    pub resource_id: String,
    pub relation: String,
    pub detail: String,
}

/// One resource's transition, as the Console renders it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "ResourceChange")]
pub struct ResourceChangeDto {
    pub resource_id: String,
    pub locator: ResourceLocatorDto,
    /// Neutral aspect names: `added`, `removed`, `content_changed`,
    /// `metadata_changed`, `relocated`, `form_changed`, `attributes_changed`.
    pub aspects: Vec<String>,
    #[serde(default)]
    pub before_state_digest: Option<String>,
    #[serde(default)]
    pub after_state_digest: Option<String>,
}

/// Something Draft could not determine about a transition.
///
/// Rendered in its own section, never mixed in with the changes: a gap means
/// "unknown", and showing it beside real changes would read as "unchanged".
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "ChangeDerivationGap")]
pub struct ChangeDerivationGapDto {
    /// `presence_uncertain` or `absence_uncertain`.
    pub kind: String,
    pub resource_id: String,
    #[serde(default)]
    pub locator: Option<ResourceLocatorDto>,
    /// Which side of the comparison was not covered.
    pub uncovered_side: String,
}

/// A Change's authoritative transition, plus any derived explanation of it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "ChangeView")]
pub struct ChangeViewDto {
    pub change_set_digest: String,
    pub base_snapshot_digest: String,
    pub result_snapshot_digest: String,
    pub observation_context_digest: String,
    /// What changed, proved.
    pub resources: Vec<ResourceChangeDto>,
    /// What could not be determined. A separate field on purpose.
    #[serde(default)]
    pub derivation_gaps: Vec<ChangeDerivationGapDto>,
    /// Contributed review units, when a comparison capability derived any.
    /// Absent means Draft knows *that* the resources changed without being
    /// able to say how — a real answer, not an empty rendering.
    #[serde(default)]
    pub review_units: Vec<ReviewUnitDto>,
    #[serde(default)]
    pub capability_gaps: Vec<CapabilityGapDto>,
}

/// One independently decidable unit of a change.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "ReviewUnit")]
pub struct ReviewUnitDto {
    pub unit_id: String,
    pub resource_id: String,
    #[serde(default)]
    pub label: Option<String>,
    /// Contributed metric totals for this unit. The Console displays the keys
    /// as given and attaches meaning to none of them.
    #[serde(default)]
    #[ts(type = "Record<string, number>")]
    pub summary: std::collections::BTreeMap<String, i64>,
}

/// How completely a past state could be restored, and what it cost.
///
/// Separate from observation completeness: a snapshot may be perfectly complete
/// and entirely unrestorable, and a surface must never imply otherwise.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "RollbackOutcome")]
pub struct RollbackOutcomeDto {
    /// `complete`, `incomplete` or `refused`. Never rendered as "restored"
    /// unless it is `complete`.
    pub outcome: String,
    pub target_snapshot_digest: String,
    #[serde(default)]
    pub resulting_snapshot_digest: Option<String>,
    /// Typed causes, so a surface can say precisely why rather than
    /// "rollback incomplete".
    #[serde(default)]
    pub uncertainties: Vec<RollbackUncertaintyDto>,
    /// Domains whose target state can never be verified for this target,
    /// however observable they become later.
    #[serde(default)]
    pub permanently_unverifiable_domains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "RollbackUncertainty")]
pub struct RollbackUncertaintyDto {
    /// `target_state_unknown`, `recovery_material_unavailable`,
    /// `current_observation_incomplete`, `restore_verification_failed`,
    /// `adapter_recovery_unavailable` or `context_incompatible`.
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "SearchResult")]
pub struct SearchResultDto {
    pub kind: String,
    pub workspace_id: Option<String>,
    pub id: Option<String>,
    pub change_id: Option<String>,
    pub title: String,
    pub subtitle: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtensionCatalogLocationDto {
    LocalDirectory { path: String },
    Https { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct ExtensionCatalogSourceDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub id: String,
    pub location: ExtensionCatalogLocationDto,
    pub configured_at: String,
    pub catalog_id: Option<String>,
    pub last_refreshed_at: Option<String>,
    pub last_error: Option<String>,
    /// A disabled source is not searched, refreshed or installed from. Its
    /// configuration, trust and installed packages are all untouched.
    pub enabled: bool,
    /// Built-in sources come from the build's official bootstrap. They can be
    /// disabled, but their trust anchor is not the user's to edit.
    pub builtin: bool,
}

impl draft_core::contracts::VersionedContract for ExtensionCatalogSourceDto {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::ConsoleCatalogSource;
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionCatalogUsabilityDto {
    Untrusted,
    Usable,
    Expired,
    Invalid,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct ExtensionCatalogSourceStatusDto {
    pub source: ExtensionCatalogSourceDto,
    pub configured: bool,
    pub trusted: bool,
    pub usability: ExtensionCatalogUsabilityDto,
    #[ts(type = "number | null")]
    pub root_version: Option<u64>,
    #[ts(type = "number")]
    pub cached_package_count: u64,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct ExtensionCatalogTargetDto {
    pub id: String,
    pub version: String,
    pub publisher: String,
    pub draft_api: String,
    pub artifact_path: String,
    #[ts(type = "number")]
    pub length: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct DiscoveredExtensionDto {
    pub source_id: String,
    pub catalog_id: String,
    pub freshness: ExtensionCatalogUsabilityDto,
    pub target: ExtensionCatalogTargetDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "ServiceJob")]
pub struct ServiceJobDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub id: String,
    pub kind: String,
    pub workspace_path: String,
    pub status: String,
    pub submitted_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    #[ts(type = "unknown")]
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub operation_id: Option<String>,
    pub workspace_id: Option<String>,
    pub phase: String,
    #[ts(type = "number")]
    pub progress_completed: u64,
    #[ts(type = "number | null")]
    pub progress_total: Option<u64>,
    pub cancellation_requested: bool,
    pub correlation_id: String,
    #[ts(type = "number")]
    pub attempt: u64,
    pub recovered_at: Option<String>,
}

impl draft_core::contracts::VersionedContract for ServiceJobDto {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::ConsoleServiceJob;
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct ApiErrorBodyDto {
    pub code: String,
    pub message: String,
    #[ts(type = "unknown")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "ApiFailure")]
pub struct ApiFailureDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub error: ApiErrorBodyDto,
}

impl draft_core::contracts::VersionedContract for ApiFailureDto {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::ConsoleApiFailure;
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConsoleContractsSchema {
    pub schema_version: u32,
    pub session: ConsoleSessionDto,
    pub project: ProjectSummaryDto,
    pub task_view: TaskViewDto,
    pub resource: ResourceDto,
    pub resource_content: ResourceContentDto,
    pub classification_bundle: ClassificationBundleDto,
    pub capability_gap: CapabilityGapDto,
    pub verification_check_result: VerificationCheckResultDto,
    pub resource_interference: ResourceInterferenceDto,
    pub change_view: ChangeViewDto,
    pub rollback_outcome: RollbackOutcomeDto,
    pub search_result: SearchResultDto,
    pub extension_source: ExtensionCatalogSourceStatusDto,
    pub discovered_extension: DiscoveredExtensionDto,
    pub job: ServiceJobDto,
    pub failure: ApiFailureDto,
    /// The authoritative Console read model the browser renders, and the
    /// invocation it sends back. These are the Console application protocol's
    /// own types rather than gateway-local DTOs: Web and TUI must consume the
    /// same contract, so there is deliberately no second copy to drift.
    pub console_model: ConsoleReadModel,
    pub console_action_invocation: ConsoleActionInvocation,
}

impl draft_core::contracts::VersionedContract for ConsoleContractsSchema {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::ConsoleContractsSchema;
}

pub fn typescript_declarations() -> String {
    let declarations = [
        ConsoleSessionDto::decl(),
        WorkspaceRevisionDto::decl(),
        RegistryProjectDto::decl(),
        AssigneeRefDto::decl(),
        TaskNextActionDto::decl(),
        TaskDefinitionDto::decl(),
        TaskViewDto::decl(),
        ChangeSummaryDto::decl(),
        InboxItemDto::decl(),
        ProjectSummaryDto::decl(),
        // Resources and what installed extensions say about them. Declared
        // bottom-up so each type is defined before the one that uses it.
        ResourceLocatorDto::decl(),
        CapabilityGapDto::decl(),
        ResourceDto::decl(),
        ResourceContentDto::decl(),
        ClassificationBundleDto::decl(),
        VerificationCheckResultDto::decl(),
        ResourceInterferenceDto::decl(),
        ResourceChangeDto::decl(),
        ChangeDerivationGapDto::decl(),
        ReviewUnitDto::decl(),
        ChangeViewDto::decl(),
        RollbackUncertaintyDto::decl(),
        RollbackOutcomeDto::decl(),
        SearchResultDto::decl(),
        ExtensionCatalogLocationDto::decl(),
        ExtensionCatalogSourceDto::decl(),
        ExtensionCatalogUsabilityDto::decl(),
        ExtensionCatalogSourceStatusDto::decl(),
        ExtensionCatalogTargetDto::decl(),
        DiscoveredExtensionDto::decl(),
        ServiceJobDto::decl(),
        ApiErrorBodyDto::decl(),
        ApiFailureDto::decl(),
        // Console application protocol — the action contract the browser
        // renders and invokes. Declared bottom-up so each type is defined
        // before the one that uses it.
        CanonicalRevisions::decl(),
        ModelFreshness::decl(),
        ConsoleScope::decl(),
        ConsoleSubject::decl(),
        SelectOption::decl(),
        ActionInputKind::decl(),
        ActionInputField::decl(),
        ActionTargetKind::decl(),
        ActionTarget::decl(),
        ActionPresentation::decl(),
        NextSafeAction::decl(),
        ConsoleNavigationSection::decl(),
        ConsoleReadModel::decl(),
        ConsoleActionInvocation::decl(),
    ];
    let envelope_version = draft_core::contracts::current_version(
        draft_core::contracts::ContractId::ConsoleApiEnvelope,
    );
    let failure_version = draft_core::contracts::current_version(
        draft_core::contracts::ContractId::ConsoleApiFailure,
    );
    let mutation_version = draft_core::contracts::current_version(
        draft_core::contracts::ContractId::ConsoleMutationRequest,
    );
    let session_version =
        draft_core::contracts::current_version(draft_core::contracts::ContractId::ConsoleSession);
    let job_version = draft_core::contracts::current_version(
        draft_core::contracts::ContractId::ConsoleServiceJob,
    );
    format!(
        "// @generated by cargo run -p draft-ipc --bin generate-console-contracts\n// Do not edit by hand.\n\nexport const CONTRACT_VERSIONS = {{ apiEnvelope: {envelope_version}, apiFailure: {failure_version}, mutationRequest: {mutation_version}, session: {session_version}, serviceJob: {job_version} }} as const;\n\nexport type ApiResponse<T> = {{ schema_version: {envelope_version}, data: T }};\n\n{}\n\n{}",
        declarations
            .iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n"),
        navigation_declaration()
    )
}

/// The frozen §8.3 information architecture, emitted for the browser.
///
/// Generated rather than restated, so the sections the browser renders are the
/// sections the daemon serves. A hand-written copy in the web tree would be a
/// third list, and the first one to change would be right while the other two
/// silently disagreed.
fn navigation_declaration() -> String {
    use crate::console_application::{navigation_for, ConsoleScope};
    let render = |scope: ConsoleScope| {
        navigation_for(scope)
            .into_iter()
            .map(|section| {
                let children = section
                    .children
                    .iter()
                    .map(|child| format!("\"{child}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "  {{ label: \"{}\", children: [{children}] }}",
                    section.label
                )
            })
            .collect::<Vec<_>>()
            .join(",\n")
    };
    format!(
        "/** §8.3's information architecture. Generated; the daemon serves the same list. */\nexport const CONSOLE_NAVIGATION = {{\n GLOBAL: [\n{}\n ],\n PROJECT: [\n{}\n ],\n CHANGE: [\n{}\n ],\n BASELINE: [\n{}\n ],\n}} as const;\n",
        render(ConsoleScope::Global),
        render(ConsoleScope::Project),
        render(ConsoleScope::Change),
        render(ConsoleScope::Baseline),
    )
}
