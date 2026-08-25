//! Rust-owned DTOs for the Draft Console HTTP contract.
//!
//! The generator binary emits committed TypeScript declarations and JSON
//! Schema from these definitions. Gateway and browser code never own a second
//! copy of the canonical transport shapes.

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
    pub repository_path: String,
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
    pub produced_packs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "PackSummary")]
pub struct PackSummaryDto {
    pub pack_id: String,
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
    pub packs: Vec<PackSummaryDto>,
    pub inbox: Vec<InboxItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "EditorFile")]
pub struct EditorFileDto {
    pub path: String,
    pub kind: String,
    pub protected: bool,
    #[ts(type = "number")]
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "EditorFileView")]
pub struct EditorFileViewDto {
    pub path: String,
    pub content: String,
    pub protected: bool,
    pub workspace_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(rename = "SearchResult")]
pub struct SearchResultDto {
    pub kind: String,
    pub workspace_id: Option<String>,
    pub id: Option<String>,
    pub pack_id: Option<String>,
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
    pub editor_file: EditorFileDto,
    pub editor_file_view: EditorFileViewDto,
    pub search_result: SearchResultDto,
    pub extension_source: ExtensionCatalogSourceStatusDto,
    pub discovered_extension: DiscoveredExtensionDto,
    pub job: ServiceJobDto,
    pub failure: ApiFailureDto,
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
        PackSummaryDto::decl(),
        InboxItemDto::decl(),
        ProjectSummaryDto::decl(),
        EditorFileDto::decl(),
        EditorFileViewDto::decl(),
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
        "// @generated by cargo run -p draft-ipc --bin generate-console-contracts\n// Do not edit by hand.\n\nexport const CONTRACT_VERSIONS = {{ apiEnvelope: {envelope_version}, apiFailure: {failure_version}, mutationRequest: {mutation_version}, session: {session_version}, serviceJob: {job_version} }} as const;\n\nexport type ApiResponse<T> = {{ schema_version: {envelope_version}, data: T }};\n\n{}\n",
        declarations
            .iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}
