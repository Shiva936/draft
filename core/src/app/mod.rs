pub mod adapters;
pub mod adoption;
pub mod maintenance;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::operation::records::{
    HookResult, HookStatus, NativeSubmitStatus, RollbackPlan, RollbackRecord, SubmitOverallStatus,
    SubmitRecord,
};
use crate::pack::lifecycle::PackLifecycle;
use crate::pack::staging::{Evidence, FilePatch, HunkOverlap, PackWorkspace, PatchHunk, PatchSet};
use crate::pack::PatchSetId;
use crate::review::risk::{RiskConfig, RiskLevel, RiskSummary};
use crate::review::session::{
    Decision, DecisionKind, ReviewComment, ReviewFile, ReviewReport, ReviewUnit,
};
use crate::review::verification::VerificationConfig;
use crate::review::workflow::{DecisionId, ReviewCommentId};
use crate::support::actor::{ActorKind, ActorRef};
use crate::support::common::{
    now, ActorId, EvidenceId, PackId, ReceiptId, RollbackPlanId, SnapshotId, TaskId, WorkspaceId,
    WorkspacePath,
};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::{
    ensure_dir, list_with_extension, read_toml, write_atomic, write_json, write_toml,
};
use crate::support::hashing::{
    blake3_hex, canonical_json, hex_encode, sha256_hex, try_canonical_hash,
};
use crate::support::lock::FileGuard;
use crate::support::redaction::{redact as redact_secrets, redact_value};
use crate::trust::event::{EventReplayReport, HashChainStatus, WorkspaceEventLog};
use crate::trust::identity::resolve_actor;
use crate::trust::receipt::ActionReceiptDraft;
use crate::workspace::config::{DraftConfig, HookEntry, ResolvedConfig, SubmitHookPhase};
use crate::workspace::object_store::{
    read_object_pack_index, write_object_pack_index, ObjectPack, ObjectPackEntry, ObjectStore,
};
use crate::workspace::snapshot::{
    diff_manifests, latest_snapshot, pattern_match, read_ignore_lines, relative_path as rel_path,
    walk_dir, IgnoreMatcher, Scanner, Snapshotter,
};
use crate::workspace::state::{
    FileChangeKind, FileKind, FileManifestEntry, Snapshot, WorkspaceStatus,
};
use crate::workspace::{DraftLayout, Workspace, WorkspaceMetadata};

const DRAFT_DIR: &str = ".draft";
use crate::contracts::{current_version, ContractId};

#[derive(Debug, Clone)]
pub struct App;

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

/// Report from `draft pack inspect <pck_id>`.
#[derive(Debug, Clone, Serialize)]
pub struct PackInspectReport {
    pub manifest: crate::pack::PackManifest,
    pub lifecycle: PackLifecycle,
    pub quarantine: Option<crate::pack::PackQuarantineRecord>,
    pub valid_actions: Vec<String>,
    pub symbols_touched: Vec<String>,
    pub public_api_changed: Vec<String>,
    pub receipts: Vec<String>,
    pub verified: bool,
    pub revision_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackReopenReport {
    pub pack_id: String,
    pub lifecycle: PackLifecycle,
    pub revision_id: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusComponent {
    Repo,
    Tasks,
    Candidates,
    Changes,
    Hooks,
}

impl StatusComponent {
    pub fn parse(value: &str) -> DraftResult<Self> {
        match value {
            "repo" => Ok(StatusComponent::Repo),
            "tasks" => Ok(StatusComponent::Tasks),
            "candidates" => Ok(StatusComponent::Candidates),
            "changes" => Ok(StatusComponent::Changes),
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
            StatusComponent::Changes => "changes",
            StatusComponent::Hooks => "hooks",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StatusOptions {
    pub pack: Option<String>,
    pub component: Option<StatusComponent>,
    pub full: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    pub workspace: WorkspaceStatus,
    pub component: Option<String>,
    pub pack: Option<String>,
    pub full: bool,
    pub sections: BTreeMap<String, Value>,
}

/// Report from `draft pack depends <pck_id>`.
#[derive(Debug, Clone, Serialize)]
pub struct PackDependsReport {
    pub pack_id: String,
    pub base_workspace_hash: String,
    pub changed_files: Vec<String>,
    /// Other packs sharing symbols with this one → the shared symbol names.
    pub shared_symbol_packs: std::collections::BTreeMap<String, Vec<String>>,
    pub declared_dependencies: Vec<String>,
}

/// A single detected conflict between two packs.
#[derive(Debug, Clone, Serialize)]
pub struct ConflictFinding {
    pub kind: String,
    pub detail: String,
    pub blocking: bool,
}

/// Report from `draft pack conflicts <a> <b>`.
#[derive(Debug, Clone, Serialize)]
pub struct PackConflictsReport {
    pub pack_a: String,
    pub pack_b: String,
    pub conflicts: Vec<ConflictFinding>,
    pub blocking: bool,
}

/// Report from `draft pack compose <a> <b> --name <name>`.
#[derive(Debug, Clone, Serialize)]
pub struct PackComposeReport {
    pub pack_id: String,
    pub name: String,
    pub dependencies: Vec<String>,
    pub requires_reverification: bool,
    pub composition_hash: String,
}

/// Report from `draft verify pck_<id>`.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    pub pack_id: String,
    pub risk_level: String,
    pub risk_score: u32,
    pub explanations: Vec<String>,
    pub required_actions: Vec<String>,
    pub selected_tests: Vec<crate::review::verification::SelectedTest>,
    pub selected_fuzz_targets: Vec<crate::review::verification::SelectedFuzzTarget>,
    pub selection_reason: String,
    pub coverage_basis: String,
    pub symbols_touched: usize,
    pub public_api_changed: usize,
    pub result_hash: String,
}

/// Report from `draft pack --export`.
#[derive(Debug, Clone, Serialize)]
pub struct PackExportReport {
    pub pack_id: String,
    pub name: String,
    pub output: String,
    pub bytes: u64,
}

/// Report from `draft pack --import`.
#[derive(Debug, Clone, Serialize)]
pub struct PackImportReport {
    pub pack_id: String,
    pub name: String,
    pub quarantined: bool,
    pub remapped: bool,
    pub external_receipts: usize,
    pub applied: bool,
}

/// Parameters describing one canonical-pack lifecycle sync.
struct PackSyncSpec {
    kind: crate::trust::event::EventKind,
    intent: crate::pack::PackIntent,
    lifecycle: crate::pack::lifecycle::PackLifecycle,
    metadata: Value,
}

/// Result of a `--dry-run` for submit or rollback: what would happen and why.
#[derive(Debug, Clone, Serialize)]
pub struct DryRunReport {
    pub action: String,
    pub target: String,
    pub would_proceed: bool,
    pub resulting_state: String,
    pub affected_files: Vec<String>,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditorFileEntry {
    pub path: String,
    pub kind: String,
    pub protected: bool,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditorFileView {
    pub path: String,
    pub content: String,
    pub protected: bool,
    pub workspace_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditorSaveReport {
    pub path: String,
    pub pack_id: String,
    pub backup_path: Option<String>,
    pub workspace_hash: String,
    pub protected: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditorSelectionTaskReport {
    pub task_id: String,
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditorMutationReport {
    pub path: String,
    pub old_path: Option<String>,
    pub backup_path: Option<String>,
    pub workspace_hash: String,
    pub action: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditorSearchHit {
    pub path: String,
    pub line: u32,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditorDiffReport {
    pub path: String,
    pub base: String,
    pub unified_diff: String,
    pub workspace_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditorWorkspaceReport {
    pub mode: String,
    pub workspace_hash: String,
    pub pending_edits: usize,
    pub files: usize,
    pub status: String,
}

impl DoctorReport {
    /// True if every present scope is healthy.
    pub fn healthy(&self) -> bool {
        self.global.healthy() && self.project.as_ref().map(|p| p.healthy()).unwrap_or(true)
    }
}

/// Write a minimal canonical manifest for the implicit base pack (empty change).
fn write_base_canonical_manifest(
    root: &Path,
    pack_id: &PackId,
    name: &str,
    patch: &PatchSet,
) -> DraftResult<()> {
    use crate::pack::{PackManifest, PackRevision, PackStore};
    let workspace_hash = crate::workspace::source_view::workspace_hash(root)?;
    let patch_bytes = to_pretty(patch)?;
    let diff_digest = sha256_hex(&patch_bytes);
    let mut manifest = PackManifest {
        schema_version: current_version(ContractId::PackManifest),
        pack_id: pack_id.to_string(),
        manifest_digest: String::new(),
        name: name.to_string(),
        description: "base pack".to_string(),
        intent: crate::pack::PackIntent::Feature,
        provenance: serde_json::json!({"origin": "local"}),
        author_id: "actor_local".to_string(),
        candidate_id: None,
        declared_dependencies: Vec::new(),
        created_at: now().to_rfc3339(),
    };
    let store = PackStore::new(crate::workspace::layout::DraftLayout::for_root(root));
    manifest.refresh_manifest_digest();
    store.write_manifest(&manifest)?;
    let mut revision = PackRevision {
        schema_version: current_version(ContractId::PackRevision),
        pack_id: manifest.pack_id.clone(),
        manifest_digest: manifest.manifest_digest.clone(),
        revision_id: "rev_initial".into(),
        revision_number: 1,
        revision_digest: String::new(),
        base_digest: workspace_hash.clone(),
        content_digest: workspace_hash.clone(),
        diff_digest,
        target_digest: workspace_hash.clone(),
        resolved_dependency_digests: Vec::new(),
        created_at: now().to_rfc3339(),
    };
    revision.refresh_revision_digest();
    store.write_revision(&revision)?;
    write_atomic(
        &store
            .dir_for(crate::pack::PackLocation::Store, pack_id.as_str())
            .join("changes.patch"),
        &patch_bytes,
    )?;
    store.write_lifecycle_in(
        crate::pack::PackLocation::Store,
        &crate::pack::lifecycle::PackLifecycleRecord {
            schema_version: current_version(ContractId::PackLifecycle),
            pack_id: manifest.pack_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_digest: revision.revision_digest.clone(),
            lifecycle: crate::pack::lifecycle::PackLifecycle::Draft,
            updated_at: now(),
            last_operation_id: crate::support::common::OperationId::new("op_init"),
        },
    )?;
    // Empty lockfile so conflicts/depends have a file set to read.
    let lock = crate::pack::PackLockfile {
        schema_version: current_version(ContractId::PackLock),
        pack_id: pack_id.to_string(),
        workspace_hash,
        file_hashes: std::collections::BTreeMap::new(),
        policy_version: crate::DRAFT_VERSION.to_string(),
        risk_engine_version: crate::DRAFT_VERSION.to_string(),
        verification_commands: Vec::new(),
        lsif_version: crate::DRAFT_VERSION.to_string(),
        test_selector_version: crate::DRAFT_VERSION.to_string(),
        fuzz_selector_version: crate::DRAFT_VERSION.to_string(),
        dependency_pack_hashes: Vec::new(),
        receipt_digests: Vec::new(),
    };
    store.write_lockfile(&lock)
}

/// Scan the workspace for test source files (excluding `.draft/`), returning
/// (relative path, content). Used to discover tests that reference changed
/// symbols during evidence-based selection.
fn scan_test_files(root: &Path) -> DraftResult<Vec<(String, String)>> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .parents(false)
        .build();
    for dent in walker {
        let dent = dent.map_err(|error| DraftError::storage(error.to_string()))?;
        let path = dent.path();
        if !path.is_file() || crate::support::pathguard::path_is_draft(path) {
            continue;
        }
        let rel = match path.strip_prefix(root) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        let lower = rel.to_lowercase();
        let is_test = lower.contains("test")
            || lower.contains("spec")
            || lower.contains("/tests/")
            || lower.starts_with("tests/");
        if is_test {
            let bytes = fs::read(path)?;
            if let Ok(content) = String::from_utf8(bytes) {
                out.push((rel, content));
            }
        }
    }
    Ok(out)
}

/// Discover available fuzz target names under a `fuzz/fuzz_targets/` directory.
fn scan_fuzz_targets(root: &Path) -> DraftResult<Vec<String>> {
    let dir = root.join("fuzz/fuzz_targets");
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&dir)? {
        let p = entry?.path();
        if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                out.push(stem.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Names of packs currently sitting in the import quarantine.
fn quarantine_names(paths: &crate::workspace::layout::DraftLayout) -> DraftResult<Vec<String>> {
    let mut names = Vec::new();
    let qdir = paths.quarantine_dir();
    if !qdir.exists() {
        return Ok(names);
    }
    for entry in std::fs::read_dir(&qdir)? {
        let manifest = entry?.path().join("manifest.json");
        let m: crate::pack::PackManifest = crate::contracts::read_persisted(&manifest)?;
        names.push(m.name);
    }
    Ok(names)
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
        App
    }

    pub fn init(&self, root: &Path) -> DraftResult<InitReport> {
        self.init_with_base(root, "base")
    }

    pub fn init_with_base(&self, root: &Path, base_pack_name: &str) -> DraftResult<InitReport> {
        let layout = DraftLayout::for_root(root);
        crate::trust::identity::reject_retired_profile_state(Some(&layout.draft_dir))?;
        let global_home = crate::workspace::home::DraftGlobalStore::locate()?;
        crate::trust::identity::global::reject_retired_actor_profile(&global_home)?;
        crate::workspace::config::reject_retired_profile_config(&layout.config_toml())?;
        crate::workspace::config::reject_retired_profile_config(&global_home.config_toml())?;
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
                &crate::review::policy::Policy::safe_default(),
            )?;
        }
        rebuild_index_for_layout(&layout)?;
        let meta = WorkspaceMetadata {
            schema_version: current_version(ContractId::WorkspaceMetadata),
            workspace_id: WorkspaceId::generate(),
            draft_version: crate::DRAFT_VERSION.to_string(),
            created_at: now(),
        };
        write_json(&layout.workspace_json(), &meta)?;
        let store = WorkspaceEventLog::new(layout.clone(), meta.workspace_id.clone());
        if created {
            let workspace_hash = crate::workspace::source_view::workspace_hash(root)?;
            let ledger = crate::trust::ledger::TrustLedger::open(root, meta.workspace_id.as_str())?;
            ledger.record(
                crate::trust::event::EventKind::InitStarted,
                None,
                None,
                workspace_hash,
                serde_json::json!({ "root": root.display().to_string() }),
            )?;
            store.append(
                "repo.initialized",
                None,
                serde_json::json!({ "root": root.display().to_string() }),
            )?;
            let mut base = PackWorkspace::new(
                meta.workspace_id.clone(),
                None,
                None,
                SnapshotId::new("chk_empty"),
                SnapshotId::new("chk_empty"),
                Some(base_pack_name.to_string()),
            );
            let pack_dir = layout.pack_workspace_dir(&base.id);
            ensure_dir(&pack_dir)?;
            let patch = empty_patch_for_pack(&base)?;
            let evidence = Evidence {
                schema_version: current_version(ContractId::PackEvidence),
                id: EvidenceId::generate(),
                pack_id: base.id.clone(),
                command_logs: Vec::new(),
                files_touched: Vec::new(),
                generated_diff_ref: None,
                test_results: Vec::new(),
                lint_results: Vec::new(),
                risk_summary_ref: None,
                agent_plan_ref: None,
                agent_transcript_ref: None,
                warnings: Vec::new(),
                created_at: now(),
            };
            base.patch_refs.push(patch.id.to_string());
            base.evidence_refs.push(evidence.id.to_string());
            base.manifest_hash = hash_json(&base)?;
            write_json(&pack_dir.join("staging.json"), &base)?;
            write_json(&pack_dir.join("patch.json"), &patch)?;
            write_json(&pack_dir.join("evidence.json"), &evidence)?;
            // Also write the immutable manifest and revision for the empty base
            // pack. No trust receipt is minted for this implicit initial state.
            write_base_canonical_manifest(root, &base.id, base_pack_name, &patch)?;
            write_atomic(
                layout.selected_pack_file().as_path(),
                base.id.to_string().as_bytes(),
            )?;
            store.append(
                "pack.created",
                Some(base.id.to_string()),
                serde_json::to_value(&base).expect("Draft-owned records must serialize"),
            )?;
            store.append(
                "pack.selected",
                Some(base.id.to_string()),
                serde_json::json!({ "name": base_pack_name }),
            )?;
        }
        let stable_store = crate::workspace::stable::StableHeadStore::new(layout.clone());
        let stable_head = if stable_store.exists() {
            stable_store.read()?
        } else {
            let workspace_hash = crate::workspace::source_view::workspace_hash(root)?;
            let ledger = crate::trust::ledger::TrustLedger::open(root, meta.workspace_id.as_str())?;
            let outcome = ledger.record(
                crate::trust::event::EventKind::InitialStableBaseCreated,
                None,
                None,
                workspace_hash,
                serde_json::json!({ "source": "init" }),
            )?;
            stable_store.initialize(root, outcome.receipt.receipt_id)?
        };
        crate::workspace::registry::ProjectRegistry::global()?.upsert(
            meta.workspace_id.as_str(),
            root,
            Some(stable_head.stable_head_hash.clone()),
        )?;
        Ok(InitReport {
            workspace_id: meta.workspace_id.to_string(),
            root: root.display().to_string(),
            created,
            draft_dir: layout.draft_dir.display().to_string(),
            stable_head_id: stable_head.id,
            stable_head_receipt_id: stable_head.receipt_id,
            workspace_hash: stable_head.workspace_hash,
            next_actions: vec![
                "draft task wizard".to_string(),
                "draft task list".to_string(),
                "draft console".to_string(),
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
            let home = crate::workspace::home::DraftGlobalStore::locate()?;
            crate::trust::identity::global::reject_retired_actor_profile(&home)?;
            crate::workspace::config::reject_retired_profile_config(&layout.config_toml())?;
            crate::workspace::config::reject_retired_profile_config(&home.config_toml())?;
        }
        let metadata_bytes = fs::read(layout.workspace_json()).map_err(|error| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!("workspace metadata is unreadable: {error}"),
            )
        })?;
        let meta: WorkspaceMetadata = crate::contracts::decode_persisted(&metadata_bytes)?;
        let paths = crate::workspace::layout::DraftLayout::for_root(&root);
        let stable_store = crate::workspace::stable::StableHeadStore::new(paths.clone());
        if !stable_store.exists() {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "workspace stable-head state is missing",
            )
            .with_suggestion("restore canonical v1 state from a trusted backup; Draft will not synthesize or migrate authoritative history"));
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
            crate::workspace::config::validate_profile_value(key, value)?
        } else {
            value.to_string()
        };
        crate::workspace::config::set_value(&ws.layout.config_toml(), key, &reported)?;
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
        crate::workspace::config::remove_table(&ws.layout.config_toml(), key)?;
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
        let home = crate::workspace::home::DraftGlobalStore::locate()?;
        crate::trust::identity::reject_retired_profile_state(None)?;
        crate::trust::identity::global::reject_retired_actor_profile(&home)?;
        crate::workspace::config::reject_retired_profile_config(&home.config_toml())?;
        let created = !home.exists();
        let hidden = home.create_all()?;
        // Seed a default policy file if absent (safe default).
        if !home.default_policy_toml().exists() {
            write_toml(
                &home.default_policy_toml(),
                &crate::review::policy::Policy::safe_default(),
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
        let home = crate::workspace::home::DraftGlobalStore::locate()?;
        crate::trust::identity::global::status(&home)
    }

    /// `draft receipt verify rcp_<id>`: verify a single signed receipt.
    pub fn receipt_verify(
        &self,
        cwd: &Path,
        receipt_id: &str,
    ) -> DraftResult<crate::trust::receipt::ReceiptVerification> {
        validate_receipt_id(receipt_id)?;
        let ws = self.open(cwd)?;
        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        ledger.verify_receipt(receipt_id)
    }

    /// `draft receipt verify --all`: verify the event chain, transparency chain,
    /// and every receipt. Fails closed if anything does not verify.
    pub fn receipt_verify_all(
        &self,
        cwd: &Path,
    ) -> DraftResult<crate::trust::ledger::LedgerVerification> {
        let ws = self.open(cwd)?;
        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        ledger.verify_all()
    }

    /// `draft config set --global <key> <value>`.
    pub fn config_set_global(&self, key: &str, value: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let home = crate::workspace::home::DraftGlobalStore::locate()?;
        crate::trust::identity::reject_retired_profile_state(None)?;
        crate::trust::identity::global::reject_retired_actor_profile(&home)?;
        crate::workspace::config::reject_retired_profile_config(&home.config_toml())?;
        home.create_all()?;
        let reported = if matches!(key, "user.name" | "user.email") {
            crate::workspace::config::validate_profile_value(key, value)?
        } else {
            value.to_string()
        };
        crate::workspace::config::set_value(&home.config_toml(), key, &reported)?;
        self.audit_global_config_change(&home, &[key], "set")?;
        Ok(ConfigReport::single(key, &reported))
    }

    pub fn config_get_global(&self, key: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let home = crate::workspace::home::DraftGlobalStore::locate()?;
        crate::trust::identity::reject_retired_profile_state(None)?;
        crate::trust::identity::global::reject_retired_actor_profile(&home)?;
        let value =
            crate::workspace::config::get_value(&home.config_toml(), key)?.unwrap_or_default();
        Ok(ConfigReport::single(key, &value))
    }

    pub fn config_unset_global(&self, key: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let home = crate::workspace::home::DraftGlobalStore::locate()?;
        crate::trust::identity::reject_retired_profile_state(None)?;
        crate::trust::identity::global::reject_retired_actor_profile(&home)?;
        crate::workspace::config::reject_retired_profile_config(&home.config_toml())?;
        home.create_all()?;
        crate::workspace::config::remove_table(&home.config_toml(), key)?;
        self.audit_global_config_change(&home, &[key], "unset")?;
        Ok(ConfigReport::single(key, ""))
    }

    fn audit_global_config_change(
        &self,
        home: &crate::workspace::home::DraftGlobalStore,
        keys: &[&str],
        operation: &str,
    ) -> DraftResult<()> {
        let actor = crate::trust::identity::global::ensure_actor(home)?;
        let bytes = std::fs::read(home.config_toml())?;
        crate::trust::audit::GlobalAuditLog::global()?.append(
            if keys.iter().all(|key| key.starts_with("user.")) {
                "user.profile.updated"
            } else {
                "config.updated"
            },
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
        let home = crate::workspace::home::DraftGlobalStore::locate()?;
        crate::trust::identity::reject_retired_profile_state(None)?;
        crate::trust::identity::global::reject_retired_actor_profile(&home)?;
        crate::workspace::config::reject_retired_profile_config(&home.config_toml())?;
        home.create_all()?;
        let mut updates = Vec::new();
        if let Some(value) = name {
            updates.push((
                "user.name".to_string(),
                value
                    .map(|value| {
                        crate::workspace::config::validate_profile_value("user.name", value)
                    })
                    .transpose()?,
            ));
        }
        if let Some(value) = email {
            updates.push((
                "user.email".to_string(),
                value
                    .map(|value| {
                        crate::workspace::config::validate_profile_value("user.email", value)
                    })
                    .transpose()?,
            ));
        }
        if updates.is_empty() {
            return Err(DraftError::invalid_config(
                "profile update must include user.name or user.email",
            ));
        }
        crate::workspace::config::update_values(&home.config_toml(), &updates)?;
        let keys = updates
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>();
        self.audit_global_config_change(&home, &keys, "update")?;
        let resolver = crate::workspace::config::ConfigResolver::load(
            None,
            Some(home.config_toml().as_path()),
        )?;
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
        let home = crate::workspace::home::DraftGlobalStore::locate()?;
        let project_cfg = match self.open(cwd) {
            Ok(ws) => Some(ws.layout.config_toml()),
            Err(error) if error.kind == DraftErrorKind::WorkspaceNotFound => None,
            Err(error) => return Err(error),
        };
        let global_cfg = home.config_toml();
        let resolver = crate::workspace::config::ConfigResolver::load(
            project_cfg.as_deref(),
            Some(global_cfg.as_path()),
        )?;
        Ok(ConfigReport::single(
            key,
            &resolver.get(key).unwrap_or_default(),
        ))
    }

    /// `draft doctor`: validate the global store and (if present) the project
    /// store for the current directory.
    pub fn doctor(&self, cwd: &Path) -> DraftResult<DoctorReport> {
        let global = self.doctor_global_scope()?;
        let project = match self.open(cwd) {
            Ok(ws) => Some(self.doctor_project_scope(&ws)?),
            Err(error) if error.kind == DraftErrorKind::WorkspaceNotFound => None,
            Err(error) => Some(DoctorScope {
                label: "project".to_string(),
                root: cwd.display().to_string(),
                exists: cwd.join(DRAFT_DIR).exists(),
                hidden: crate::support::hidden::is_hidden(&cwd.join(DRAFT_DIR)),
                checks: vec![DoctorCheck::fail_error("contract-open", error)],
            }),
        };
        Ok(DoctorReport { global, project })
    }

    /// `draft doctor --global`: validate only the global store.
    pub fn doctor_global(&self) -> DraftResult<DoctorReport> {
        Ok(DoctorReport {
            global: self.doctor_global_scope()?,
            project: None,
        })
    }

    fn doctor_global_scope(&self) -> DraftResult<DoctorScope> {
        let home = crate::workspace::home::DraftGlobalStore::locate()?;
        let exists = home.exists();
        let mut checks = Vec::new();
        if exists {
            for (name, check) in [
                (
                    "retired-profile-location",
                    crate::trust::identity::reject_retired_profile_state(None),
                ),
                (
                    "retired-actor-profile",
                    crate::trust::identity::global::reject_retired_actor_profile(&home),
                ),
                (
                    "retired-config-namespace",
                    crate::workspace::config::reject_retired_profile_config(&home.config_toml()),
                ),
            ] {
                match check {
                    Ok(()) => {
                        checks.push(DoctorCheck::ok(name, "unsupported profile state absent"))
                    }
                    Err(error) => checks.push(DoctorCheck::fail_error(name, error)),
                }
            }
            checks.push(bool_check(
                "identity",
                home.actor_json().exists(),
                "actor.json present",
                "actor.json missing — run `draft init --global`",
            ));
            checks.push(bool_check(
                "signing-key",
                home.signing_key().exists(),
                "signing key present",
                "signing key missing — run `draft init --global`",
            ));
            checks.push(bool_check(
                "keys-dir",
                home.keys_dir().is_dir(),
                "keys/ present",
                "keys/ missing",
            ));
            checks.push(bool_check(
                "default-policy",
                home.default_policy_toml().exists(),
                "default policy present",
                "default policy missing",
            ));
            match crate::trust::audit::GlobalAuditLog::global().and_then(|audit| audit.verify()) {
                Ok(count) => checks.push(DoctorCheck::ok(
                    "global-audit",
                    format!("{count} hash-chained audit records verified"),
                )),
                Err(error) => checks.push(DoctorCheck::fail_error("global-audit", error)),
            }
            #[cfg(unix)]
            checks.push(key_perms_check(&home.signing_key()));
        } else {
            checks.push(DoctorCheck::fail(
                "exists",
                "global store missing — run `draft init --global`",
            ));
        }
        Ok(DoctorScope {
            label: "global".to_string(),
            root: home.root().display().to_string(),
            exists,
            hidden: crate::support::hidden::is_hidden(home.root()),
            checks,
        })
    }

    fn doctor_project_scope(&self, ws: &Workspace) -> DraftResult<DoctorScope> {
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let mut checks = Vec::new();
        checks.push(bool_check(
            "workspace-json",
            paths.workspace_json().exists(),
            "workspace.json present",
            "workspace.json missing",
        ));
        // Event chain integrity (reuses the existing verified replay).
        match self.verify_events(&ws.root) {
            Ok(_) => checks.push(DoctorCheck::ok("event-chain", "event hash chain intact")),
            Err(e) => checks.push(DoctorCheck::fail_error("event-chain", e)),
        }
        // Canonical trust ledger: event log, receipts, transparency chain.
        match crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str()) {
            Ok(ledger) => match ledger.verify_all() {
                Ok(v) => {
                    checks.push(bool_check(
                        "trust-event-log",
                        v.event_chain_ok,
                        format!("{} canonical events verified", v.event_count),
                        "canonical event log broken",
                    ));
                    checks.push(bool_check(
                        "transparency-chain",
                        v.transparency_ok,
                        format!("{} transparency entries verified", v.transparency_count),
                        "transparency chain broken",
                    ));
                    let bad = v.receipts.iter().filter(|r| !r.ok).count();
                    checks.push(bool_check(
                        "receipts",
                        bad == 0,
                        format!("{} receipts verified", v.receipts.len()),
                        format!("{bad} receipt(s) failed verification"),
                    ));
                }
                Err(e) => checks.push(DoctorCheck::fail_error("trust-ledger", e)),
            },
            Err(e) => checks.push(DoctorCheck::fail_error("trust-ledger", e)),
        }
        for (name, dir) in [
            ("events-dir", paths.events_dir()),
            ("receipts-dir", paths.receipts_dir()),
            ("transparency-dir", paths.transparency_dir()),
            ("packs-dir", paths.packs_dir()),
            ("quarantine-dir", paths.quarantine_dir()),
            ("recovery-dir", paths.recovery_dir()),
        ] {
            checks.push(bool_check(
                name,
                dir.is_dir(),
                format!("{} present", dir.display()),
                format!("{} missing", dir.display()),
            ));
        }
        match crate::operation::RecoveryStore::for_root(&ws.root).recoverable() {
            Ok(entries) if entries.is_empty() => {
                checks.push(DoctorCheck::ok("recovery", "no interrupted operations"))
            }
            Ok(entries) => checks.push(DoctorCheck::fail(
                "recovery",
                format!(
                    "{} interrupted operation(s) need recovery: {}",
                    entries.len(),
                    entries
                        .iter()
                        .map(|entry| format!("{}:{}", entry.recovery_id, entry.operation))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
            Err(e) => checks.push(DoctorCheck::fail_error("recovery", e)),
        }
        Ok(DoctorScope {
            label: "project".to_string(),
            root: ws.root.display().to_string(),
            exists: true,
            hidden: crate::support::hidden::is_hidden(paths.draft_dir()),
            checks,
        })
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
        ws.events()?.append(
            "hook.started",
            Some(hook_name.to_string()),
            serde_json::json!({}),
        )?;
        let store = ObjectStore::new(ws.layout.clone());
        let ctx = HookContext {
            message: String::new(),
            title: String::new(),
            description: String::new(),
            task_id: String::new(),
            execution_id: String::new(),
            pack_id: String::new(),
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
            .map_err(|e| DraftError::new(DraftErrorKind::SubmitFailed, e.message))?;
        ws.events()?.append(
            "hook.completed",
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
                "ignore.added",
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
            "ignore.removed",
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
        let status = Scanner::new(&ws)?.status()?;
        ws.events()?.append(
            "workspace.scanned",
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
        if options.component.is_none() || include(StatusComponent::Changes) {
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
                    "submit": cfg.get("hooks.submit").unwrap_or_default(),
                    "verify": cfg.get("hooks.verify").unwrap_or_default(),
                    "items": if options.full { serde_json::to_value(cfg.entries())? } else { Value::Null },
                }),
            );
        }
        if let Some(pack_ref) = &options.pack {
            let inspect = self.pack_inspect(cwd, pack_ref)?;
            let readiness = self.submit_readiness_selected(cwd, Some(&inspect.manifest.pack_id))?;
            sections.insert(
                "pack".to_string(),
                serde_json::json!({
                    "pack_id": inspect.manifest.pack_id,
                    "name": inspect.manifest.name,
                    "lifecycle": inspect.lifecycle,
                    "verified": inspect.verified,
                    "lifecycle": inspect.lifecycle,
                    "readiness": readiness,
                    "details": if options.full { serde_json::to_value(inspect)? } else { Value::Null },
                }),
            );
        }
        Ok(StatusReport {
            workspace,
            component: options.component.map(|c| c.as_str().to_string()),
            pack: options.pack,
            full: options.full,
            sections,
        })
    }

    pub fn checkpoint(&self, cwd: &Path, message: &str) -> DraftResult<CheckpointReport> {
        let ws = self.open(cwd)?;
        let snapshot =
            Snapshotter::new(&ws)?.create_snapshot(resolve_actor(&ws.layout.draft_dir)?)?;
        let receipt = ActionReceiptDraft::new(
            "checkpoint",
            "completed",
            Some(snapshot.id.to_string()),
            serde_json::json!({ "message": message }),
        )
        .reversible_to(snapshot.id.to_string());
        write_receipt(&ws, &receipt)?;
        Ok(CheckpointReport {
            snapshot_id: snapshot.id.to_string(),
            receipt_id: receipt.id.to_string(),
            files: snapshot.files.len(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn task_create(
        &self,
        cwd: &Path,
        name: &str,
        goal: &str,
        template: Option<String>,
        allowed_zones: Vec<String>,
        forbidden_zones: Vec<String>,
        success_criteria: Vec<String>,
        risk: Option<&str>,
        mode: Option<&str>,
        candidate_preset: Option<String>,
    ) -> DraftResult<crate::task::TaskDefinition> {
        let ws = self.open(cwd)?;
        let stable_store = crate::workspace::stable::StableHeadStore::new(
            crate::workspace::layout::DraftLayout::for_root(&ws.root),
        );
        let stable = if stable_store.exists() {
            stable_store.read()?.stable_head_hash
        } else {
            "uninitialized".to_string()
        };
        let actor = format!("{:?}", resolve_actor(&ws.layout.draft_dir)?);
        let mut task =
            crate::task::TaskDefinition::new(name.to_string(), goal.to_string(), stable, actor)?;
        if let Some(template) = template {
            crate::task::apply_template(&mut task, &template)?;
        }
        if !allowed_zones.is_empty() {
            task.allowed_zones = allowed_zones;
        }
        if !forbidden_zones.is_empty() {
            task.forbidden_zones = forbidden_zones;
            if !task.forbidden_zones.iter().any(|p| p == ".draft/**") {
                task.forbidden_zones.push(".draft/**".into());
            }
        }
        if !success_criteria.is_empty() {
            task.success_criteria = success_criteria;
        }
        task.risk = match risk.unwrap_or("medium") {
            "low" => crate::task::TaskRisk::Low,
            "high" => crate::task::TaskRisk::High,
            "critical" => crate::task::TaskRisk::Critical,
            _ => crate::task::TaskRisk::Medium,
        };
        task.mode = match mode.unwrap_or("normal") {
            "safe" => crate::task::TaskMode::Safe,
            "plan-first" => crate::task::TaskMode::PlanFirst,
            _ => crate::task::TaskMode::Normal,
        };
        if let Some(candidate_preset) = candidate_preset {
            self.candidate_registry_for(&ws)?
                .preset(&candidate_preset)?;
            task.candidate_preset = Some(candidate_preset);
        }
        validate_task_definition(&ws, &task)?;
        crate::task::TaskStore::for_root(&ws.root).create(&task)?;
        ws.events()?.append(
            "task.created",
            Some(task.id.to_string()),
            serde_json::to_value(&task).expect("Draft-owned records must serialize"),
        )?;
        Ok(task)
    }

    pub fn task_list(&self, cwd: &Path) -> DraftResult<Vec<crate::task::TaskDefinition>> {
        let ws = self.open(cwd)?;
        crate::task::TaskStore::for_root(&ws.root).list()
    }

    pub fn task_show(
        &self,
        cwd: &Path,
        id_or_name: &str,
    ) -> DraftResult<crate::task::TaskDefinition> {
        let ws = self.open(cwd)?;
        crate::task::TaskStore::for_root(&ws.root)
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn task_update(
        &self,
        cwd: &Path,
        id_or_name: &str,
        status: Option<crate::task::TaskLifecycleStatus>,
        priority: Option<crate::task::TaskPriority>,
        due_at: Option<Option<crate::support::common::Timestamp>>,
        assignee_ref: Option<Option<crate::task::AssigneeRef>>,
    ) -> DraftResult<crate::task::TaskDefinition> {
        let ws = self.open(cwd)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        let mut task = store
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        if let Some(status) = status {
            task.status = status;
        }
        if let Some(priority) = priority {
            task.priority = priority;
        }
        if let Some(due_at) = due_at {
            task.due_at = due_at;
        }
        if let Some(assignee_ref) = assignee_ref {
            if let Some(assignee) = &assignee_ref {
                if !matches!(assignee.kind.as_str(), "actor" | "candidate")
                    || assignee.id.trim().is_empty()
                {
                    return Err(DraftError::invalid_config(
                        "assignee must be a stable actor or candidate reference",
                    ));
                }
            }
            task.assignee_ref = assignee_ref;
        }
        task.updated_at = now();
        store.update(&task)?;
        ws.events()?.append(
            "task.updated",
            Some(task.id.to_string()),
            serde_json::to_value(&task).expect("Draft-owned records must serialize"),
        )?;
        Ok(task)
    }

    pub fn task_add_next_action(
        &self,
        cwd: &Path,
        id_or_name: &str,
        label: &str,
    ) -> DraftResult<crate::task::TaskDefinition> {
        let ws = self.open(cwd)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        let mut task = store
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        if label.trim().is_empty() {
            return Err(DraftError::invalid_config(
                "next action label cannot be empty",
            ));
        }
        task.next_actions.push(crate::task::NextAction {
            id: format!("actn_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
            label: label.trim().into(),
            completed: false,
        });
        task.updated_at = now();
        store.update(&task)?;
        ws.events()?.append(
            "task.next_action_added",
            Some(task.id.to_string()),
            serde_json::to_value(&task).expect("Draft-owned records must serialize"),
        )?;
        Ok(task)
    }

    pub fn task_set_next_action(
        &self,
        cwd: &Path,
        id_or_name: &str,
        action_id: &str,
        completed: bool,
    ) -> DraftResult<crate::task::TaskDefinition> {
        let ws = self.open(cwd)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        let mut task = store
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        let action = task
            .next_actions
            .iter_mut()
            .find(|action| action.id == action_id)
            .ok_or_else(|| {
                DraftError::not_found(format!("next action '{action_id}' was not found"))
            })?;
        action.completed = completed;
        task.updated_at = now();
        store.update(&task)?;
        ws.events()?.append(
            "task.next_action_updated",
            Some(task.id.to_string()),
            serde_json::to_value(&task).expect("Draft-owned records must serialize"),
        )?;
        Ok(task)
    }

    pub fn task_view(&self, cwd: &Path, id_or_name: &str) -> DraftResult<crate::task::TaskView> {
        let ws = self.open(cwd)?;
        let task = crate::task::TaskStore::for_root(&ws.root)
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        let executions = crate::task::ExecutionStore::for_root(&ws.root).list_for_task(&task.id)?;
        let workflow = crate::review::workflow::WorkflowStore::for_root(&ws.root);
        let evidence = workflow.evidence()?;
        let decisions = workflow.decisions()?;
        let latest_execution = executions.last().map(|e| crate::task::ExecutionView {
            execution_id: e.id.to_string(),
            candidate: e.candidate.clone(),
            status: execution_status_label(e.status).to_string(),
            produced_pack: e.produced_pack.clone(),
            error: e
                .failure_reason
                .clone()
                .or_else(|| e.cancellation_reason.clone()),
            note: None,
        });
        let produced_packs = executions
            .iter()
            .filter_map(|e| e.produced_pack.clone())
            .collect::<Vec<_>>();
        let failed = executions
            .iter()
            .filter(|e| matches!(e.status, crate::task::ExecutionStatus::Failed))
            .count();
        let running = executions
            .iter()
            .filter(|e| {
                matches!(
                    e.status,
                    crate::task::ExecutionStatus::Queued
                        | crate::task::ExecutionStatus::Running
                        | crate::task::ExecutionStatus::Retrying
                )
            })
            .count();
        let task_evidence = evidence
            .iter()
            .filter(|e| e.task_id.as_deref() == Some(task.id.as_str()))
            .count();
        let approved_packs = decisions
            .iter()
            .filter(|d| {
                d.decision_type == crate::review::workflow::DecisionType::Approve
                    && d.invalidated_at.is_none()
                    && d.pack_id
                        .as_ref()
                        .map(|p| produced_packs.iter().any(|pack| pack == p))
                        .unwrap_or(false)
            })
            .count();
        let health = if failed > 0 {
            crate::task::TaskViewStatus::Blocked
        } else if running > 0 {
            crate::task::TaskViewStatus::Running
        } else if produced_packs.is_empty() {
            crate::task::TaskViewStatus::Defined
        } else if approved_packs > 0 {
            crate::task::TaskViewStatus::Approved
        } else {
            crate::task::TaskViewStatus::NeedsReview
        };
        let review_status = if approved_packs > 0 {
            crate::task::TaskViewStatus::Approved
        } else if produced_packs.is_empty() {
            crate::task::TaskViewStatus::Pending
        } else {
            crate::task::TaskViewStatus::NeedsReview
        };
        let recommended_action = if let Some(pack) = produced_packs.last() {
            if approved_packs > 0 {
                format!("draft submit {pack}")
            } else {
                format!("draft review {pack}")
            }
        } else if running > 0 {
            format!("draft task {id_or_name} --executions")
        } else {
            format!("draft task spawn {} -c <candidate>", task.name)
        };
        Ok(crate::task::TaskView {
            task,
            health,
            latest_execution,
            review_status,
            recommended_action,
            execution_count: executions.len(),
            evidence_count: task_evidence,
            produced_packs,
        })
    }

    pub fn task_view_with_options(
        &self,
        cwd: &Path,
        id_or_name: &str,
        options: TaskViewOptions,
    ) -> DraftResult<Value> {
        let ws = self.open(cwd)?;
        let task = crate::task::TaskStore::for_root(&ws.root)
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        let mut out = serde_json::to_value(self.task_view(cwd, id_or_name)?)?;
        let Some(map) = out.as_object_mut() else {
            return Ok(out);
        };
        let include_all = options.full;
        let exec_store = crate::task::ExecutionStore::for_root(&ws.root);
        let executions = exec_store.list_for_task(&task.id)?;
        let produced_packs = executions
            .iter()
            .filter_map(|execution| execution.produced_pack.clone())
            .collect::<Vec<_>>();

        if include_all || options.executions {
            map.insert("executions".to_string(), serde_json::to_value(&executions)?);
        }
        if include_all || options.packs {
            let mut packs = Vec::new();
            for pack in &produced_packs {
                match self.pack_inspect(cwd, pack) {
                    Ok(inspect) => packs.push(serde_json::to_value(inspect)?),
                    Err(err) => packs.push(serde_json::json!({
                        "pack_id": pack,
                        "error": err.message,
                    })),
                }
            }
            map.insert("packs".to_string(), Value::Array(packs));
        }
        if include_all || options.evidence {
            let workflow = crate::review::workflow::WorkflowStore::for_root(&ws.root);
            let task_id = task.id.to_string();
            let evidence = workflow
                .evidence()?
                .into_iter()
                .filter(|e| {
                    e.task_id.as_deref() == Some(task_id.as_str())
                        || e.pack_id
                            .as_ref()
                            .map(|pack| produced_packs.iter().any(|p| p == pack))
                            .unwrap_or(false)
                })
                .collect::<Vec<_>>();
            map.insert("evidence".to_string(), serde_json::to_value(evidence)?);
        }
        if include_all || options.conflicts {
            let mut conflicts = Vec::new();
            for i in 0..produced_packs.len() {
                for j in (i + 1)..produced_packs.len() {
                    match self.pack_conflicts(cwd, &produced_packs[i], &produced_packs[j]) {
                        Ok(report) => conflicts.push(serde_json::to_value(report)?),
                        Err(err) => conflicts.push(serde_json::json!({
                            "left": produced_packs[i],
                            "right": produced_packs[j],
                            "error": err.message,
                        })),
                    }
                }
            }
            map.insert("conflicts".to_string(), Value::Array(conflicts));
        }
        if include_all || options.lanes {
            let lanes = executions
                .iter()
                .map(|execution| {
                    serde_json::json!({
                        "candidate": execution.candidate.clone(),
                        "execution_id": execution.id.to_string(),
                        "status": execution_status_label(execution.status),
                        "produced_pack": execution.produced_pack.clone(),
                        "attempt": execution.attempt,
                    })
                })
                .collect::<Vec<_>>();
            map.insert("lanes".to_string(), Value::Array(lanes));
        }
        if include_all || options.timeline {
            let task_id = task.id.to_string();
            let execution_ids = executions
                .iter()
                .map(|execution| execution.id.to_string())
                .collect::<BTreeSet<_>>();
            let pack_ids = produced_packs.iter().cloned().collect::<BTreeSet<_>>();
            let events = ws
                .events()?
                .read_all()?
                .into_iter()
                .filter(|event| {
                    event
                        .subject_id
                        .as_ref()
                        .map(|id| {
                            id == &task_id || execution_ids.contains(id) || pack_ids.contains(id)
                        })
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>();
            map.insert("timeline".to_string(), serde_json::to_value(events)?);
        }
        if include_all || options.explain {
            map.insert(
                "explain".to_string(),
                serde_json::json!({
                    "template": task.template.clone(),
                    "required_evidence": task.required_evidence.clone(),
                    "review_questions": task.review_questions.clone(),
                    "next_action": map.get("recommended_action").cloned().expect("Draft-owned records must serialize"),
                }),
            );
        }
        if include_all || options.diff_stable {
            let mut diffs = BTreeMap::new();
            for pack in &produced_packs {
                match self.pack_diff_text(cwd, pack) {
                    Ok(diff) => {
                        diffs.insert(pack.clone(), Value::String(diff));
                    }
                    Err(err) => {
                        diffs.insert(pack.clone(), serde_json::json!({ "error": err.message }));
                    }
                }
            }
            map.insert("diff_stable".to_string(), serde_json::to_value(diffs)?);
        }
        if include_all || options.decompose {
            let children = self.task_decompose(&ws, &task)?;
            map.insert(
                "decomposition".to_string(),
                serde_json::json!({
                    "created_or_existing": children,
                    "next_action": "inspect child tasks, then spawn the candidate lane for each child task",
                }),
            );
        }
        Ok(out)
    }

    fn task_decompose(
        &self,
        ws: &Workspace,
        task: &crate::task::TaskDefinition,
    ) -> DraftResult<Vec<crate::task::TaskDefinition>> {
        let Some(template) = task.template.as_deref() else {
            return Ok(Vec::new());
        };
        let template = crate::task::builtin_template(template)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        let mut children = Vec::new();
        for rule in template.decomposition_rules {
            let name = format!("{}-{}", task.name, rule.id);
            if let Some(existing) = store.resolve(&name)? {
                children.push(existing);
                continue;
            }
            let mut child = crate::task::TaskDefinition::new(
                name,
                format!("{}: {}", task.goal, rule.description),
                task.base_stable_head.clone(),
                task.created_by.clone(),
            )?;
            child.kind = crate::task::TaskKind::Generated;
            child.template = rule.child_template.clone();
            child.allowed_zones = rule.zones.clone();
            child.forbidden_zones = task
                .allowed_zones
                .iter()
                .filter(|zone| !child.allowed_zones.iter().any(|allowed| allowed == *zone))
                .cloned()
                .chain(task.forbidden_zones.iter().cloned())
                .collect();
            child.required_evidence = task.required_evidence.clone();
            child.review_questions = task.review_questions.clone();
            child.candidate_preset = task.candidate_preset.clone();
            child.parent_pack = task.parent_pack.clone();
            child.metadata.insert(
                "parent_task".to_string(),
                Value::String(task.id.to_string()),
            );
            child.metadata.insert(
                "decomposition_rule".to_string(),
                Value::String(rule.id.clone()),
            );
            store.create(&child)?;
            ws.events()?.append(
                "task.generated",
                Some(child.id.to_string()),
                serde_json::json!({
                    "parent_task": task.id.to_string(),
                    "rule": rule.id,
                    "task_name": child.name.clone(),
                }),
            )?;
            children.push(child);
        }
        Ok(children)
    }

    pub fn task_drop(
        &self,
        cwd: &Path,
        id_or_name: &str,
        hard: bool,
    ) -> DraftResult<crate::task::TaskDropOutcome> {
        let ws = self.open(cwd)?;
        let recovery = crate::operation::RecoveryStore::for_root(&ws.root);
        let entry = recovery.start(
            if hard { "task.drop_hard" } else { "task.drop" },
            Some(id_or_name.to_string()),
            serde_json::json!({ "task": id_or_name, "hard": hard }),
        )?;
        let entry = recovery.mark_in_progress(entry, None)?;
        let outcome = match crate::task::TaskStore::for_root(&ws.root).drop_task(id_or_name, hard) {
            Ok(outcome) => outcome,
            Err(err) => {
                let _ = recovery.fail(entry, err.message.clone());
                return Err(err);
            }
        };
        let _ = recovery.complete(
            entry,
            serde_json::json!({
                "task_id": outcome.task_id.clone(),
                "definition_removed": outcome.definition_removed,
                "removed_executions": outcome.removed_executions.clone(),
            }),
        )?;
        ws.events()?.append(
            if hard {
                "task.dropped_hard"
            } else {
                "task.dropped"
            },
            Some(outcome.task_id.clone()),
            serde_json::to_value(&outcome).expect("Draft-owned records must serialize"),
        )?;
        Ok(outcome)
    }

    pub fn task_export(
        &self,
        cwd: &Path,
        id_or_name: &str,
        output: Option<&Path>,
    ) -> DraftResult<TaskExportReport> {
        let ws = self.open(cwd)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        let task = store
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        let output = output
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(format!("{}.task.json", task.name)));
        let exported = store.export_to(task.id.as_str(), &output)?;
        ws.events()?.append(
            "task.exported",
            Some(exported.id.to_string()),
            serde_json::json!({
                "task_id": exported.id.to_string(),
                "task_name": exported.name,
                "output": output.display().to_string(),
            }),
        )?;
        Ok(TaskExportReport {
            task_id: exported.id.to_string(),
            task_name: exported.name,
            output: output.display().to_string(),
            next_action: "import with `draft task import <path>` in another Draft workspace"
                .to_string(),
        })
    }

    pub fn task_import(
        &self,
        cwd: &Path,
        source: &Path,
        name: Option<String>,
    ) -> DraftResult<TaskImportReport> {
        let ws = self.open(cwd)?;
        let mut task: crate::task::TaskDefinition =
            crate::contracts::decode_wire(&fs::read(source)?)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        if let Some(name) = name {
            task.name = name;
        }
        if store.resolve(&task.name)?.is_some() {
            return Err(DraftError::new(
                DraftErrorKind::TaskDefinitionConflict,
                format!("task '{}' already exists", task.name),
            )
            .with_suggestion("pass `--name <new-name>` or drop the existing task first"));
        }
        let stable = self
            .stable_head_ref(&ws)
            .unwrap_or_else(|_| "uninitialized".to_string());
        let actor = format!("{:?}", resolve_actor(&ws.layout.draft_dir)?);
        let at = now();
        task.schema_version = current_version(ContractId::TaskDefinition);
        task.id = TaskId::generate();
        task.kind = crate::task::TaskKind::Imported;
        task.created_at = at;
        task.updated_at = at;
        task.created_by = actor;
        task.base_stable_head = stable;
        task.source_context = Some(crate::task::TaskSourceContext {
            path: source.display().to_string(),
            start_line: None,
            end_line: None,
            symbol: None,
            reason: Some("task import".to_string()),
        });
        store.import(&task)?;
        ws.events()?.append(
            "task.imported",
            Some(task.id.to_string()),
            serde_json::json!({
                "task_id": task.id.to_string(),
                "task_name": task.name,
                "source": source.display().to_string(),
            }),
        )?;
        Ok(TaskImportReport {
            task_id: task.id.to_string(),
            task_name: task.name,
            source: source.display().to_string(),
            next_action: format!("draft task spawn {}", task.id),
        })
    }

    pub fn task_retry_execution(
        &self,
        cwd: &Path,
        execution_id: &str,
    ) -> DraftResult<crate::task::Execution> {
        let ws = self.open(cwd)?;
        let execution = crate::task::ExecutionStore::for_root(&ws.root).retry(execution_id)?;
        ws.events()?.append(
            "execution.retry_queued",
            Some(execution.id.to_string()),
            serde_json::to_value(&execution).expect("Draft-owned records must serialize"),
        )?;
        Ok(execution)
    }

    pub fn task_cancel_execution(
        &self,
        cwd: &Path,
        execution_id: &str,
        reason: Option<String>,
    ) -> DraftResult<crate::task::Execution> {
        let ws = self.open(cwd)?;
        let reason = reason.unwrap_or_else(|| "cancelled by user".to_string());
        let store = crate::task::ExecutionStore::for_root(&ws.root);
        let before = store.read(execution_id)?;
        if let Some(pid) = before.pid {
            let _ = terminate_process(pid);
        }
        let execution = store.mark_cancelled(execution_id, &reason)?;
        ws.events()?.append(
            "execution.cancelled",
            Some(execution.id.to_string()),
            serde_json::json!({ "reason": reason }),
        )?;
        Ok(execution)
    }

    pub fn task_resume_execution(
        &self,
        cwd: &Path,
        execution_id: &str,
    ) -> DraftResult<crate::task::Execution> {
        let ws = self.open(cwd)?;
        let store = crate::task::ExecutionStore::for_root(&ws.root);
        let execution = store.read(execution_id)?;
        if !execution.is_resumable() {
            return Err(DraftError::invalid_config(format!(
                "execution {execution_id} is not resumable"
            )));
        }
        let registry = self.candidate_registry_for(&ws)?;
        registry
            .profile(&execution.candidate)?
            .ensure_capability("resume")?;
        let resumed = store.update(execution_id, |e| {
            e.status = crate::task::ExecutionStatus::Queued;
            e.finished_at = None;
            e.failure_reason = None;
            e.cancellation_reason = None;
        })?;
        ws.events()?.append(
            "execution.resume_queued",
            Some(resumed.id.to_string()),
            serde_json::to_value(&resumed).expect("Draft-owned records must serialize"),
        )?;
        Ok(resumed)
    }

    pub fn inbox(&self, cwd: &Path) -> DraftResult<Vec<crate::review::workflow::InboxItem>> {
        let ws = self.open(cwd)?;
        let workflow = crate::review::workflow::WorkflowStore::for_root(&ws.root);
        let mut by_id = BTreeMap::<String, crate::review::workflow::InboxItem>::new();
        for item in workflow.inbox()? {
            by_id.insert(item.id.clone(), item);
        }

        for pack in self.pack_list_for_workspace(&ws)? {
            let lifecycle = pack_lifecycle(&ws, &pack.id)?;
            if matches!(lifecycle, PackLifecycle::Draft | PackLifecycle::Verified) {
                insert_inbox(
                    &mut by_id,
                    format!("inbox:pack_review:{}", pack.id),
                    "pack_review",
                    pack.id.to_string(),
                    "review_needed",
                    format!(
                        "pack {} needs review",
                        pack.name.clone().unwrap_or_else(|| pack.id.to_string())
                    ),
                    format!("draft review -p {}", pack.id),
                );
            }
            {
                let patch = load_patch(&ws, &pack)?;
                let paths = patch
                    .files
                    .iter()
                    .map(|file| file.path.as_str().to_string())
                    .collect::<Vec<_>>();
                let reviewers = workflow
                    .decisions()?
                    .into_iter()
                    .filter(|decision| decision.pack_id.as_deref() == Some(pack.id.as_str()))
                    .map(|decision| decision.author)
                    .collect::<Vec<_>>();
                let ownership =
                    crate::workspace::ownership::evaluate(&ws.root, &paths, &reviewers)?;
                if ownership.missing_owner_review {
                    insert_inbox(
                        &mut by_id,
                        format!("inbox:owner_review:{}", pack.id),
                        "owner_review",
                        pack.id.to_string(),
                        "missing_owner_review",
                        format!("owner review needed for {}", ownership.domains.join(", ")),
                        format!("draft approve -p {} --reason <reason>", pack.id),
                    );
                }
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

        let renewal_cutoff = now() + chrono::Duration::hours(72);
        for waiver in workflow.waivers()? {
            if waiver.expires_at <= renewal_cutoff {
                insert_inbox(
                    &mut by_id,
                    format!("inbox:waiver_renewal:{}", waiver.id),
                    "waiver_renewal",
                    waiver.pack_id.clone(),
                    "expires_soon",
                    format!("waiver {} expires soon", waiver.id),
                    format!(
                        "draft waive {} {} --reason <reason> --expires 7d",
                        waiver.pack_id, waiver.finding_id
                    ),
                );
            }
        }

        for entry in crate::operation::RecoveryStore::for_root(&ws.root).recoverable()? {
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

        let pending_editor_dir = crate::workspace::layout::DraftLayout::for_root(&ws.root)
            .editor_dir()
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
                    "draft console".to_string(),
                );
            }
        }

        Ok(by_id.into_values().collect())
    }

    pub fn editor_tree(&self, cwd: &Path) -> DraftResult<Vec<EditorFileEntry>> {
        let ws = self.open(cwd)?;
        let mut out = Vec::new();
        collect_editor_entries(&ws.root, &ws.root, &mut out)?;
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    pub fn editor_session_commit(
        &self,
        cwd: &Path,
        session_id: &str,
        operation_id: crate::support::common::OperationId,
    ) -> DraftResult<crate::operation::editor::EditCommitResult> {
        let ws = self.open(cwd)?;
        let store = crate::operation::editor::EditSessionStore::for_workspace(&ws.root);
        store.commit_with_validation(session_id, operation_id, |session, _revision| {
            if let crate::operation::editor::EditAttribution::Pack { id }
            | crate::operation::editor::EditAttribution::Review { id } = &session.attribution
            {
                let workflow = crate::review::workflow::WorkflowStore::for_root(&ws.root);
                workflow.mark_pack_evidence_stale(
                    id,
                    "editor commit changed the canonical subject digest",
                    "editor.session.commit",
                )?;
                workflow.invalidate_approvals(
                    id,
                    "editor commit changed the canonical subject digest",
                )?;
            }
            Ok(())
        })
    }

    pub fn editor_workspace(&self, cwd: &Path) -> DraftResult<EditorWorkspaceReport> {
        let ws = self.open(cwd)?;
        let files = self.editor_tree(cwd)?.len();
        let pending_dir = crate::workspace::layout::DraftLayout::for_root(&ws.root)
            .editor_dir()
            .join("pending");
        let pending_edits = if pending_dir.exists() {
            list_with_extension(&pending_dir, "json")?.len()
        } else {
            0
        };
        Ok(EditorWorkspaceReport {
            mode: if pending_edits > 0 {
                "task_edit".to_string()
            } else {
                "browse".to_string()
            },
            workspace_hash: crate::workspace::source_view::workspace_hash(&ws.root)?,
            pending_edits,
            files,
            status: if pending_edits > 0 {
                "needs_review"
            } else {
                "ready"
            }
            .to_string(),
        })
    }

    pub fn editor_read(&self, cwd: &Path, path: &str) -> DraftResult<EditorFileView> {
        let ws = self.open(cwd)?;
        let rel = WorkspacePath::new(crate::support::pathguard::check_relative(path).map_err(
            |e| {
                DraftError::new(
                    DraftErrorKind::ProtectedFileAccess,
                    format!("unsafe editor path '{path}': {e}"),
                )
            },
        )?);
        crate::workspace::protected::ensure_allowed(&ws.root, &rel)?;
        let fs_path = safe_workspace_dest(&ws.root, &rel)?;
        let bytes = std::fs::read(&fs_path)
            .map_err(|e| DraftError::not_found(format!("cannot read {}: {e}", rel.as_str())))?;
        let content = String::from_utf8(bytes).map_err(|_| {
            DraftError::new(
                DraftErrorKind::ProtectedFileAccess,
                format!("editor can only open UTF-8 text files: {}", rel.as_str()),
            )
        })?;
        Ok(EditorFileView {
            path: rel.to_string(),
            content,
            protected: false,
            workspace_hash: crate::workspace::source_view::workspace_hash(&ws.root)?,
        })
    }

    pub fn editor_create_file(
        &self,
        cwd: &Path,
        path: &str,
        content: &str,
    ) -> DraftResult<EditorMutationReport> {
        let ws = self.open(cwd)?;
        let rel = checked_editor_path(&ws.root, path)?;
        let dest = safe_workspace_dest(&ws.root, &rel)?;
        if dest.exists() {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!("editor path already exists: {}", rel.as_str()),
            ));
        }
        if let Some(parent) = dest.parent() {
            ensure_dir(parent)?;
        }
        write_atomic(&dest, content.as_bytes())?;
        ws.events()?.append(
            "editor.file_created",
            Some(rel.to_string()),
            serde_json::json!({ "path": rel.to_string() }),
        )?;
        Ok(EditorMutationReport {
            path: rel.to_string(),
            old_path: None,
            backup_path: None,
            workspace_hash: crate::workspace::source_view::workspace_hash(&ws.root)?,
            action: "created".to_string(),
        })
    }

    pub fn editor_rename_file(
        &self,
        cwd: &Path,
        from: &str,
        to: &str,
    ) -> DraftResult<EditorMutationReport> {
        let ws = self.open(cwd)?;
        let from_rel = checked_editor_path(&ws.root, from)?;
        let to_rel = checked_editor_path(&ws.root, to)?;
        let from_path = safe_workspace_dest(&ws.root, &from_rel)?;
        let to_path = safe_workspace_dest(&ws.root, &to_rel)?;
        if !from_path.is_file() {
            return Err(DraftError::not_found(format!(
                "editor source does not exist: {}",
                from_rel.as_str()
            )));
        }
        if to_path.exists() {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!("editor destination already exists: {}", to_rel.as_str()),
            ));
        }
        if let Some(parent) = to_path.parent() {
            ensure_dir(parent)?;
        }
        fs::rename(&from_path, &to_path).map_err(|e| {
            DraftError::storage(format!(
                "failed to rename {} to {}: {e}",
                from_rel.as_str(),
                to_rel.as_str()
            ))
        })?;
        ws.events()?.append(
            "editor.file_renamed",
            Some(to_rel.to_string()),
            serde_json::json!({ "from": from_rel.to_string(), "to": to_rel.to_string() }),
        )?;
        Ok(EditorMutationReport {
            path: to_rel.to_string(),
            old_path: Some(from_rel.to_string()),
            backup_path: None,
            workspace_hash: crate::workspace::source_view::workspace_hash(&ws.root)?,
            action: "renamed".to_string(),
        })
    }

    pub fn editor_delete_file(&self, cwd: &Path, path: &str) -> DraftResult<EditorMutationReport> {
        let ws = self.open(cwd)?;
        let rel = checked_editor_path(&ws.root, path)?;
        let dest = safe_workspace_dest(&ws.root, &rel)?;
        if !dest.is_file() {
            return Err(DraftError::not_found(format!(
                "editor file does not exist: {}",
                rel.as_str()
            )));
        }
        let backup = editor_backup_path(&ws.root, &rel)?;
        if let Some(parent) = backup.parent() {
            ensure_dir(parent)?;
        }
        fs::copy(&dest, &backup)
            .map_err(|e| DraftError::storage(format!("failed to back up {}: {e}", rel.as_str())))?;
        fs::remove_file(&dest)
            .map_err(|e| DraftError::storage(format!("failed to delete {}: {e}", rel.as_str())))?;
        ws.events()?.append(
            "editor.file_deleted",
            Some(rel.to_string()),
            serde_json::json!({ "path": rel.to_string(), "backup": backup.display().to_string() }),
        )?;
        Ok(EditorMutationReport {
            path: rel.to_string(),
            old_path: None,
            backup_path: Some(backup.display().to_string()),
            workspace_hash: crate::workspace::source_view::workspace_hash(&ws.root)?,
            action: "deleted".to_string(),
        })
    }

    pub fn editor_search(
        &self,
        cwd: &Path,
        query: &str,
        limit: usize,
    ) -> DraftResult<Vec<EditorSearchHit>> {
        let ws = self.open(cwd)?;
        let query = query.trim();
        if query.is_empty() {
            return Ok(vec![]);
        }
        let mut hits = Vec::new();
        for file in self.editor_tree(cwd)? {
            if file.protected {
                continue;
            }
            let path = safe_workspace_dest(&ws.root, &WorkspacePath::new(&file.path))?;
            let bytes = fs::read(&path).map_err(|error| {
                DraftError::storage(format!("cannot read {}: {error}", path.display()))
            })?;
            let Ok(content) = String::from_utf8(bytes) else {
                continue;
            };
            for (idx, line) in content.lines().enumerate() {
                if line.contains(query) {
                    hits.push(EditorSearchHit {
                        path: file.path.clone(),
                        line: (idx + 1) as u32,
                        preview: crate::support::redaction::redact(line.trim()),
                    });
                    if hits.len() >= limit.max(1) {
                        return Ok(hits);
                    }
                }
            }
        }
        Ok(hits)
    }

    pub fn editor_diff(
        &self,
        cwd: &Path,
        path: &str,
        pack_ref: Option<&str>,
    ) -> DraftResult<EditorDiffReport> {
        let ws = self.open(cwd)?;
        let rel = checked_editor_path(&ws.root, path)?;
        let current_path = safe_workspace_dest(&ws.root, &rel)?;
        let current = if current_path.exists() {
            fs::read_to_string(&current_path).map_err(|error| {
                DraftError::storage(format!("cannot read {}: {error}", current_path.display()))
            })?
        } else {
            String::new()
        };
        let (base_name, base_content) = if let Some(pack_ref) = pack_ref {
            (
                format!("pack-base:{pack_ref}"),
                self.editor_pack_base_content(&ws, &rel, pack_ref)?,
            )
        } else {
            return Err(DraftError::not_found(
                "stable file content is not available in the current stable_head record; pass a pack id to diff against pack base",
            ));
        };
        Ok(EditorDiffReport {
            path: rel.to_string(),
            base: base_name,
            unified_diff: simple_unified_diff(rel.as_str(), &base_content, &current),
            workspace_hash: crate::workspace::source_view::workspace_hash(&ws.root)?,
        })
    }

    pub fn editor_restore_from_pack_base(
        &self,
        cwd: &Path,
        path: &str,
        pack_ref: &str,
    ) -> DraftResult<EditorMutationReport> {
        let ws = self.open(cwd)?;
        let rel = checked_editor_path(&ws.root, path)?;
        let dest = safe_workspace_dest(&ws.root, &rel)?;
        let backup = if dest.exists() {
            let backup = editor_backup_path(&ws.root, &rel)?;
            if let Some(parent) = backup.parent() {
                ensure_dir(parent)?;
            }
            fs::copy(&dest, &backup).map_err(|e| {
                DraftError::storage(format!("failed to back up {}: {e}", rel.as_str()))
            })?;
            Some(backup)
        } else {
            None
        };
        let base = self.editor_pack_base_content(&ws, &rel, pack_ref)?;
        if let Some(parent) = dest.parent() {
            ensure_dir(parent)?;
        }
        write_atomic(&dest, base.as_bytes())?;
        ws.events()?.append(
            "editor.file_restored",
            Some(rel.to_string()),
            serde_json::json!({ "path": rel.to_string(), "pack": pack_ref }),
        )?;
        Ok(EditorMutationReport {
            path: rel.to_string(),
            old_path: None,
            backup_path: backup.map(|p| p.display().to_string()),
            workspace_hash: crate::workspace::source_view::workspace_hash(&ws.root)?,
            action: "restored".to_string(),
        })
    }

    fn editor_pack_base_content(
        &self,
        ws: &Workspace,
        rel: &WorkspacePath,
        pack_ref: &str,
    ) -> DraftResult<String> {
        let pack_id = self.resolve_canonical_pack_ref(ws, pack_ref)?;
        let store =
            crate::pack::PackStore::new(crate::workspace::layout::DraftLayout::for_root(&ws.root));
        let loc = store
            .locate(&pack_id)
            .unwrap_or(crate::pack::PackLocation::Store);
        let path = store.dir_for(loc, &pack_id).join("changes.patch");
        let bytes = fs::read(&path)
            .map_err(|e| DraftError::not_found(format!("cannot read pack diff {pack_id}: {e}")))?;
        let patch: PatchSet = crate::contracts::decode_persisted(&bytes)?;
        let file = patch
            .files
            .iter()
            .find(|f| f.path.as_str() == rel.as_str())
            .ok_or_else(|| {
                DraftError::not_found(format!("pack {pack_id} does not include {}", rel.as_str()))
            })?;
        let Some(old_hash) = &file.old_hash else {
            return Ok(String::new());
        };
        let content = ObjectStore::new(ws.layout.clone()).get_bytes(old_hash)?;
        String::from_utf8(content).map_err(|_| {
            DraftError::new(
                DraftErrorKind::ProtectedFileAccess,
                format!("pack base for {} is not UTF-8 text", rel.as_str()),
            )
        })
    }

    pub fn editor_save_to_pack(
        &self,
        cwd: &Path,
        path: &str,
        content: &str,
        pack_name: Option<String>,
    ) -> DraftResult<EditorSaveReport> {
        let ws = self.open(cwd)?;
        let rel = WorkspacePath::new(crate::support::pathguard::check_relative(path).map_err(
            |e| {
                DraftError::new(
                    DraftErrorKind::ProtectedFileAccess,
                    format!("unsafe editor path '{path}': {e}"),
                )
            },
        )?);
        crate::workspace::protected::ensure_allowed(&ws.root, &rel)?;
        let dest = safe_workspace_dest(&ws.root, &rel)?;
        let project_paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let backup_path = if dest.exists() {
            let backup = project_paths.editor_dir().join("backups").join(format!(
                "{}-{}",
                now().timestamp_millis(),
                rel.as_str().replace('/', "__")
            ));
            if let Some(parent) = backup.parent() {
                ensure_dir(parent)?;
            }
            std::fs::copy(&dest, &backup).map_err(|e| {
                DraftError::storage(format!("failed to back up {}: {e}", rel.as_str()))
            })?;
            Some(backup.display().to_string())
        } else {
            None
        };
        if let Some(parent) = dest.parent() {
            ensure_dir(parent)?;
        }
        write_atomic(&dest, content.as_bytes())?;
        let pack = self.pack_create(
            cwd,
            pack_name.or_else(|| Some(format!("editor-{}", rel.as_str().replace('/', "-")))),
            None,
            true,
        )?;
        let store = crate::review::workflow::WorkflowStore::for_root(&ws.root);
        let _ = store.mark_pack_evidence_stale(
            pack.id.as_str(),
            "editor saved new content into pack",
            "editor.save",
        );
        let _ = store.invalidate_approvals(pack.id.as_str(), "editor saved new content into pack");
        Ok(EditorSaveReport {
            path: rel.to_string(),
            pack_id: pack.id.to_string(),
            backup_path,
            workspace_hash: crate::workspace::source_view::workspace_hash(&ws.root)?,
            protected: false,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn task_create_from_selection(
        &self,
        cwd: &Path,
        path: &str,
        start_line: u32,
        end_line: u32,
        selected_text: &str,
        reason: Option<String>,
        workspace_hash: Option<String>,
    ) -> DraftResult<EditorSelectionTaskReport> {
        let ws = self.open(cwd)?;
        let rel = WorkspacePath::new(crate::support::pathguard::check_relative(path).map_err(
            |e| {
                DraftError::new(
                    DraftErrorKind::ProtectedFileAccess,
                    format!("unsafe editor selection path '{path}': {e}"),
                )
            },
        )?);
        crate::workspace::protected::ensure_allowed(&ws.root, &rel)?;
        let current_hash = crate::workspace::source_view::workspace_hash(&ws.root)?;
        if let Some(expected) = workspace_hash {
            if !expected.is_empty() && expected != current_hash {
                return Err(DraftError::new(
                    DraftErrorKind::DirtyWorkspace,
                    "workspace changed since the editor selection was read",
                )
                .with_suggestion("reload the file before creating a task from selection"));
            }
        }
        let stable_store = crate::workspace::stable::StableHeadStore::new(
            crate::workspace::layout::DraftLayout::for_root(&ws.root),
        );
        let stable = if stable_store.exists() {
            stable_store.read()?.stable_head_hash
        } else {
            "uninitialized".to_string()
        };
        let actor = format!("{:?}", resolve_actor(&ws.layout.draft_dir)?);
        let mut task = crate::task::TaskDefinition::new(
            format!("Review {}", rel.as_str()),
            reason
                .clone()
                .unwrap_or_else(|| format!("Review selected code in {}", rel.as_str())),
            stable,
            actor,
        )?;
        task.kind = crate::task::TaskKind::Defined;
        task.source_context = Some(crate::task::TaskSourceContext {
            path: rel.to_string(),
            start_line: Some(start_line),
            end_line: Some(end_line),
            symbol: None,
            reason,
        });
        task.allowed_zones = vec![rel.to_string()];
        task.success_criteria = vec!["Selected code has been reviewed and addressed".to_string()];
        task.metadata.insert(
            "selected_text".to_string(),
            serde_json::Value::String(crate::support::redaction::redact(selected_text)),
        );
        task.metadata.insert(
            "selection_workspace_hash".to_string(),
            serde_json::Value::String(current_hash),
        );
        crate::task::TaskStore::for_root(&ws.root).create(&task)?;
        ws.events()?.append(
            "task.created_from_selection",
            Some(task.id.to_string()),
            serde_json::to_value(&task).expect("Draft-owned records must serialize"),
        )?;
        Ok(EditorSelectionTaskReport {
            task_id: task.id.to_string(),
            path: rel.to_string(),
            start_line,
            end_line,
        })
    }

    pub fn waive(
        &self,
        cwd: &Path,
        pack_id: &str,
        finding_id: &str,
        reason: &str,
        expires: &str,
    ) -> DraftResult<crate::review::workflow::Waiver> {
        let ws = self.open(cwd)?;
        self.resolve_pack_ref(&ws, pack_id)?;
        let seconds = parse_duration_seconds(expires)?;
        let created_at = now();
        let waiver = crate::review::workflow::Waiver {
            schema_version: current_version(ContractId::Waiver),
            id: crate::review::workflow::WaiverId::generate(),
            pack_id: pack_id.into(),
            finding_id: finding_id.into(),
            author: format!("{:?}", resolve_actor(&ws.layout.draft_dir)?),
            reason: reason.into(),
            created_at,
            expires_at: created_at + chrono::Duration::seconds(seconds),
            receipt_id: None,
        };
        crate::review::workflow::WorkflowStore::for_root(&ws.root).write_waiver(&waiver)?;
        ws.events()?.append(
            "waiver.created",
            Some(pack_id.into()),
            serde_json::to_value(&waiver).expect("Draft-owned records must serialize"),
        )?;
        Ok(waiver)
    }

    /// The canonical spawn engine (Blueprint §3.9, TDD §10.1).
    ///
    /// Resolves a stored task (or creates an inline one), resolves the
    /// candidate list or preset, validates capabilities, and runs one real
    /// execution per candidate in an isolated workspace. Each successful
    /// execution produces a pack diffed against the same pre-spawn baseline;
    /// the working tree is left exactly as it was before the spawn.
    #[allow(clippy::too_many_arguments)]
    pub fn task_spawn(
        &self,
        cwd: &Path,
        name: &str,
        pack_id: Option<&str>,
        candidates: Vec<String>,
        cron: Option<String>,
        instruction: Vec<String>,
    ) -> DraftResult<TaskSpawnReport> {
        self.task_spawn_with_preset(cwd, name, pack_id, candidates, None, cron, instruction)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn task_spawn_with_preset(
        &self,
        cwd: &Path,
        name: &str,
        pack_id: Option<&str>,
        mut candidates: Vec<String>,
        preset: Option<String>,
        cron: Option<String>,
        instruction: Vec<String>,
    ) -> DraftResult<TaskSpawnReport> {
        let ws = self.open(cwd)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        let exec_store = crate::task::ExecutionStore::for_root(&ws.root);
        let instruction = instruction.join(" ");

        // Stored-vs-inline instruction rules.
        let mut task = match store.resolve(name)? {
            Some(stored) => {
                if !instruction.trim().is_empty() {
                    return Err(DraftError::new(
                        DraftErrorKind::TaskDefinitionConflict,
                        format!(
                            "task '{name}' already has a stored definition; spawn it without an inline instruction"
                        ),
                    )
                    .with_suggestion(format!(
                        "run `draft task spawn {name}` to use the stored goal, or `draft task {name}` to inspect it"
                    )));
                }
                stored
            }
            None => {
                if instruction.trim().is_empty() {
                    return Err(DraftError::invalid_config(format!(
                        "no stored task named '{name}'; an inline instruction is required"
                    ))
                    .with_suggestion(format!(
                        "run `draft task spawn {name} -- <instruction>` or create it first with `draft task create {name} --goal <goal>`"
                    )));
                }
                let stable = self.stable_head_ref(&ws)?;
                let actor = format!("{:?}", resolve_actor(&ws.layout.draft_dir)?);
                let task_name = inline_task_name(name);
                let mut t = crate::task::TaskDefinition::new(
                    task_name,
                    instruction.clone(),
                    stable,
                    actor,
                )?;
                t.kind = crate::task::TaskKind::Inline;
                store.create(&t)?;
                ws.events()?.append(
                    "task.created",
                    Some(t.id.to_string()),
                    serde_json::to_value(&t).expect("Draft-owned records must serialize"),
                )?;
                t
            }
        };

        if let Some(cron) = cron {
            task.schedule = Some(crate::task::TaskSchedule {
                cron: Some(cron),
                note: None,
            });
        }

        // Candidate list / preset resolution.
        let registry = self.candidate_registry_for(&ws)?;
        let mut preset_used = None;
        if candidates.is_empty() {
            let preset_name = preset.clone().or_else(|| task.candidate_preset.clone());
            if let Some(preset_name) = preset_name {
                let p = registry.preset(&preset_name)?;
                candidates = p.candidates.clone();
                preset_used = Some(p);
            } else {
                candidates.push("manual".to_string());
            }
        } else if let Some(preset_name) = preset {
            // Explicit candidates win, but a named preset still applies its policy.
            preset_used = Some(registry.preset(&preset_name)?);
        }
        if let Some(p) = &preset_used {
            if p.plan_first && task.mode == crate::task::TaskMode::Normal {
                task.mode = crate::task::TaskMode::PlanFirst;
            }
            if p.require_full_evidence && !task.required_evidence.iter().any(|e| e == "full_tests")
            {
                task.required_evidence.push("full_tests".to_string());
            }
            if p.prefer_smallest_valid_pack {
                task.metadata
                    .insert("prefer_smallest_valid_pack".to_string(), Value::Bool(true));
            }
        }
        task.updated_at = now();
        store.update(&task)?;

        // Validate every candidate profile before starting any execution.
        let mut profiles = Vec::new();
        for candidate in &candidates {
            let profile = registry.profile(candidate)?;
            if profile.kind.runs_command() {
                profile.ensure_capability("edit")?;
                if task.mode == crate::task::TaskMode::PlanFirst {
                    profile.ensure_capability("plan")?;
                }
                if profile.command.is_none() {
                    return Err(DraftError::new(
                        DraftErrorKind::CandidateNotConfigured,
                        format!("candidate '{candidate}' has no command configured"),
                    )
                    .with_suggestion(format!(
                        "set `command` under [candidates.{candidate}] in .draft/config.toml"
                    )));
                }
            }
            profiles.push(profile);
        }
        if profiles.iter().any(|profile| profile.kind.runs_command()) {
            let stable = crate::workspace::stable::StableHeadStore::new(
                crate::workspace::layout::DraftLayout::for_root(&ws.root),
            )
            .read()?;
            ensure_workspace_matches_hash(
                &ws,
                &stable.workspace_hash,
                "task spawn",
                "create a pack from the current edits, discard them, or run the task from a clean stable head",
            )?;
        }

        let parent_pack = Some(match pack_id {
            Some(pack_id) => pack_id.to_string(),
            None => self.selected_pack_id(cwd)?,
        });
        ws.events()?.append(
            "task.spawned",
            Some(task.id.to_string()),
            serde_json::json!({
                "pack_id": parent_pack,
                "candidates": candidates,
                "preset": preset_used.as_ref().map(|p| p.name.clone()),
                "instruction": redact_secrets(&task.goal),
            }),
        )?;

        // One shared pre-spawn baseline: every candidate pack diffs against it.
        let baseline =
            Snapshotter::new(&ws)?.create_snapshot(resolve_actor(&ws.layout.draft_dir)?)?;
        let mut executions = Vec::new();
        for profile in &profiles {
            let command = profile
                .command
                .as_deref()
                .map(|t| crate::task::candidate::render_command(t, &task.goal))
                .unwrap_or_default();
            let mut execution =
                crate::task::Execution::queued(&task, profile.name.clone(), command);
            execution.workspace_id = Some(ws.workspace_id.to_string());
            execution.parent_pack = parent_pack.clone();
            exec_store.write(&execution)?;
            ws.events()?.append(
                "execution.queued",
                Some(execution.id.to_string()),
                serde_json::json!({
                    "task_id": task.id.to_string(),
                    "candidate": profile.name,
                }),
            )?;
            if !profile.kind.runs_command() {
                executions.push(ExecutionSummary {
                    execution_id: execution.id.to_string(),
                    candidate: profile.name.clone(),
                    status: "queued".to_string(),
                    produced_pack: None,
                    error: None,
                    note: Some(
                        "human execution: make edits in the editor or workspace, then create a pack"
                            .to_string(),
                    ),
                });
                continue;
            }
            match self.run_candidate_execution(&ws, &task, &execution, profile, &baseline) {
                Ok(produced_pack) => {
                    let refreshed = exec_store.read(execution.id.as_str())?;
                    executions.push(ExecutionSummary {
                        execution_id: execution.id.to_string(),
                        candidate: profile.name.clone(),
                        status: execution_status_label(refreshed.status).to_string(),
                        produced_pack,
                        error: refreshed.failure_reason,
                        note: None,
                    });
                }
                Err(e) => {
                    let _ = exec_store.mark_failed(execution.id.as_str(), &e.to_string());
                    let _ = ws.events()?.append(
                        "execution.failed",
                        Some(execution.id.to_string()),
                        serde_json::json!({
                            "task_id": task.id.to_string(),
                            "candidate": profile.name,
                            "reason": redact_secrets(&e.to_string()),
                        }),
                    );
                    executions.push(ExecutionSummary {
                        execution_id: execution.id.to_string(),
                        candidate: profile.name.clone(),
                        status: "failed".to_string(),
                        produced_pack: None,
                        error: Some(e.to_string()),
                        note: None,
                    });
                }
            }
        }
        store.rebuild_index()?;

        let next_action = if let Some(done) = executions
            .iter()
            .find(|e| e.status == "completed" && e.produced_pack.is_some())
        {
            format!(
                "draft review {}",
                done.produced_pack.clone().unwrap_or_default()
            )
        } else if executions.iter().any(|e| e.status == "queued") {
            "make the edits, then run `draft pack new` to capture them".to_string()
        } else {
            format!("draft task {} --executions", task.name)
        };
        Ok(TaskSpawnReport {
            task_id: task.id.to_string(),
            task_name: task.name.clone(),
            task_kind: format!("{:?}", task.kind).to_lowercase(),
            preset: preset_used.map(|p| p.name),
            parent_pack,
            executions,
            next_action,
        })
    }

    /// Run one candidate command in an isolated copy of the workspace,
    /// enforce candidate limits and file guards, and turn accepted changes
    /// into a pack against `baseline`. The working tree is restored to its
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
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let exe_id = execution.id.as_str();
        let runtime_dir = paths.execution_runtime_dir(exe_id);
        ensure_dir(&runtime_dir)?;

        // Deterministic task contract for the candidate (TDD §10.2).
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
                "base_stable_head": task.base_stable_head,
                "required_evidence": task.required_evidence,
                "protected_files": crate::workspace::protected::rules_for_project(&ws.root)?,
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
            for entry in baseline.files.iter() {
                let src = safe_workspace_dest(&ws.root, &entry.path)?;
                let dst = dir.join(entry.path.as_str());
                if let Some(parent) = dst.parent() {
                    ensure_dir(parent)?;
                }
                if src.exists() && !src.is_dir() {
                    fs::copy(&src, &dst).map_err(|e| {
                        DraftError::storage(format!(
                            "failed to copy {} into execution workspace: {e}",
                            entry.path.as_str()
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
            "execution.started",
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
                "execution.failed",
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
            let created_pack =
                self.create_empty_execution_pack(ws, task, execution, profile, baseline)?;
            exec_store.mark_completed(exe_id)?;
            exec_store.update(exe_id, |e| {
                e.produced_pack = Some(created_pack.clone());
            })?;
            ws.events()?.append(
                "execution.completed",
                Some(exe_id.to_string()),
                serde_json::json!({
                    "task_id": task.id.to_string(),
                    "candidate": profile.name,
                    "changed_files": 0,
                    "produced_pack": created_pack,
                }),
            )?;
            return Ok(Some(created_pack));
        }

        // Apply accepted changes, capture the pack, restore the tree.
        let produced_pack = if isolated {
            let stash = stash_workspace_files(&ws.root, &changes)?;
            let apply = apply_isolated_changes(&ws.root, &work_dir, &changes);
            let pack = match apply {
                Ok(()) => self.pack_create_with_base(
                    &ws.root,
                    Some(pack_name_for(task, profile, exe_id)),
                    Some(task.id.to_string()),
                    baseline.clone(),
                ),
                Err(e) => Err(e),
            };
            restore_workspace_files(&ws.root, stash)?;
            Some(pack?)
        } else {
            Some(self.pack_create_with_base(
                &ws.root,
                Some(pack_name_for(task, profile, exe_id)),
                Some(task.id.to_string()),
                baseline.clone(),
            )?)
        };
        let pack_id = produced_pack.as_ref().map(|p| p.id.to_string());
        exec_store.update(exe_id, |e| {
            e.produced_pack = pack_id.clone();
        })?;
        exec_store.mark_completed(exe_id)?;
        ws.events()?.append(
            "execution.completed",
            Some(exe_id.to_string()),
            serde_json::json!({
                "task_id": task.id.to_string(),
                "candidate": profile.name,
                "changed_files": changes.len(),
                "produced_pack": pack_id,
            }),
        )?;
        // Isolated work dir is no longer needed after a completed run.
        if isolated {
            let _ = fs::remove_dir_all(paths.execution_work_dir(exe_id));
        }
        Ok(pack_id)
    }

    fn create_empty_execution_pack(
        &self,
        ws: &Workspace,
        task: &crate::task::TaskDefinition,
        execution: &crate::task::Execution,
        profile: &crate::task::candidate::CandidateProfile,
        baseline: &Snapshot,
    ) -> DraftResult<String> {
        let mut pack = self.pack_create_with_base(
            &ws.root,
            Some(profile.name.clone()),
            Some(task.id.to_string()),
            baseline.clone(),
        )?;
        pack.execution_id = Some(execution.id.clone());
        save_pack_staging(ws, &mut pack)?;
        Ok(pack.id.to_string())
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
        let rules = crate::workspace::protected::rules_for_project(&ws.root)?;
        for change in changes {
            let path = change.path.as_str();
            if crate::workspace::protected::matches_rules(&rules, path) {
                violations.push(format!("protected file '{path}'"));
                let _ = ws.events()?.append(
                    "protected.access_attempt",
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
        if let Some(max) = profile.limits.max_changed_lines {
            let total: u64 = changes.iter().map(|c| c.changed_lines).sum();
            if total > u64::from(max) {
                violations.push(format!(
                    "{total} changed lines exceeds the candidate limit of {max}"
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
            "execution.blocked",
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

    /// Resolve the current stable head reference, or "uninitialized".
    fn stable_head_ref(&self, ws: &Workspace) -> DraftResult<String> {
        let store = crate::workspace::stable::StableHeadStore::new(
            crate::workspace::layout::DraftLayout::for_root(&ws.root),
        );
        if store.exists() {
            Ok(store.read()?.stable_head_hash)
        } else {
            Ok("uninitialized".to_string())
        }
    }

    fn candidate_registry_for(
        &self,
        ws: &Workspace,
    ) -> DraftResult<crate::task::candidate::CandidateRegistry> {
        let project_config = ws.layout.config_toml();
        let global_config = Some(crate::workspace::home::DraftGlobalStore::locate()?.config_toml());
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

    pub fn task_current(&self, cwd: &Path) -> DraftResult<Value> {
        let tasks = self.task_list(cwd)?;
        if let Some(task) = tasks.last() {
            Ok(serde_json::to_value(task).expect("Draft-owned records must serialize"))
        } else {
            Ok(serde_json::json!({ "message": "No running tasks." }))
        }
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
            "candidate.added",
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
            "candidate.updated",
        )
    }

    pub fn candidate_remove(
        &self,
        cwd: &Path,
        name: &str,
    ) -> DraftResult<crate::task::candidate::CandidateProfile> {
        let ws = self.open(cwd)?;
        let record = self.candidate_show(cwd, name)?;
        crate::workspace::config::remove_table(
            &ws.layout.config_toml(),
            &format!("candidates.{name}"),
        )?;
        ws.events()?.append(
            "candidate.removed",
            Some(name.to_string()),
            serde_json::json!({}),
        )?;
        Ok(record)
    }

    pub fn candidate_packs(
        &self,
        cwd: &Path,
        pack: Option<&str>,
        candidate: Option<&str>,
    ) -> DraftResult<Vec<CandidatePackAssignment>> {
        let ws = self.open(cwd)?;
        let packs = self.pack_list(cwd)?;
        let mut out = Vec::new();
        for p in packs {
            if let Some(filter) = pack {
                if p.id.as_str() != filter && p.name.as_deref() != Some(filter) {
                    continue;
                }
            }
            let execution = p
                .execution_id
                .as_ref()
                .map(|execution_id| {
                    crate::task::ExecutionStore::for_root(&ws.root).read(execution_id.as_str())
                })
                .transpose()?;
            let name = execution
                .as_ref()
                .map(|execution| execution.candidate.clone())
                .unwrap_or_else(|| {
                    p.execution_id
                        .as_ref()
                        .map(|_| "unknown".to_string())
                        .unwrap_or_else(|| "manual".to_string())
                });
            if candidate.map(|c| c != name).unwrap_or(false) {
                continue;
            }
            out.push(CandidatePackAssignment {
                pack_id: p.id.to_string(),
                candidate: name,
                task_id: p.task_id.as_ref().map(ToString::to_string),
                execution_id: p.execution_id.as_ref().map(ToString::to_string),
            });
        }
        Ok(out)
    }

    fn write_candidate(
        &self,
        cwd: &Path,
        name: &str,
        kind: &str,
        source: &str,
        template: Vec<String>,
        event: &str,
    ) -> DraftResult<crate::task::candidate::CandidateProfile> {
        let ws = self.open(cwd)?;
        let _ = crate::task::candidate::CandidateKind::parse(kind)?;
        crate::workspace::config::set_value(
            &ws.layout.config_toml(),
            &format!("candidates.{name}.kind"),
            kind,
        )?;
        if !template.is_empty() {
            crate::workspace::config::set_value(
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

    pub fn pack_create(
        &self,
        cwd: &Path,
        name: Option<String>,
        task_id: Option<String>,
        from_working_tree: bool,
    ) -> DraftResult<PackWorkspace> {
        let ws = self.open(cwd)?;
        if let Some(name) = name.as_deref() {
            self.ensure_unique_pack_name(&ws, name)?;
        }
        let base = latest_snapshot(&ws)?.unwrap_or_else(|| empty_snapshot(&ws));
        let result =
            Snapshotter::new(&ws)?.create_snapshot(resolve_actor(&ws.layout.draft_dir)?)?;
        let patch = diff_snapshots(&ws, &base, &result)?;
        let evidence = Evidence {
            schema_version: current_version(ContractId::PackEvidence),
            id: EvidenceId::generate(),
            pack_id: PackId::new("pending"),
            command_logs: vec![],
            files_touched: patch.files.iter().map(|f| f.path.clone()).collect(),
            generated_diff_ref: None,
            test_results: vec![],
            lint_results: vec![],
            risk_summary_ref: None,
            agent_plan_ref: None,
            agent_transcript_ref: None,
            warnings: if from_working_tree {
                vec![]
            } else {
                vec!["created from current workspace snapshot".to_string()]
            },
            created_at: now(),
        };
        let mut pack = PackWorkspace::new(
            ws.workspace_id.clone(),
            task_id.map(TaskId::new),
            None,
            base.id.clone(),
            result.id.clone(),
            name,
        );
        let mut evidence = evidence;
        evidence.pack_id = pack.id.clone();
        let pack_dir = ws.layout.pack_workspace_dir(&pack.id);
        ensure_dir(&pack_dir)?;
        write_json(&pack_dir.join("staging.json"), &pack)?;
        write_json(&pack_dir.join("patch.json"), &patch)?;
        write_json(&pack_dir.join("evidence.json"), &evidence)?;
        pack.patch_refs.push(patch.id.to_string());
        pack.evidence_refs.push(evidence.id.to_string());
        pack.manifest_hash = hash_json(&pack)?;
        write_json(&pack_dir.join("staging.json"), &pack)?;
        ws.events()?.append(
            "pack.created",
            Some(pack.id.to_string()),
            serde_json::to_value(&pack).expect("Draft-owned records must serialize"),
        )?;
        write_atomic(
            ws.layout.selected_pack_file().as_path(),
            pack.id.to_string().as_bytes(),
        )?;
        ws.events()?.append(
            "pack.selected",
            Some(pack.id.to_string()),
            serde_json::json!({}),
        )?;
        // Materialize the immutable manifest/revision and signed creation
        // receipt so every pack is inspectable and exportable immediately.
        let created_patch = load_patch(&ws, &pack)?;
        self.sync_canonical_pack(
            &ws,
            &pack,
            Some(&created_patch),
            PackSyncSpec {
                kind: crate::trust::event::EventKind::PackCreated,
                intent: crate::pack::PackIntent::Feature,
                lifecycle: crate::pack::lifecycle::PackLifecycle::Draft,
                metadata: serde_json::json!({ "name": pack.name }),
            },
        )?;
        Ok(pack)
    }

    fn pack_create_with_base(
        &self,
        cwd: &Path,
        name: Option<String>,
        task_id: Option<String>,
        base: Snapshot,
    ) -> DraftResult<PackWorkspace> {
        let ws = self.open(cwd)?;
        if let Some(name) = name.as_deref() {
            self.ensure_unique_pack_name(&ws, name)?;
        }
        let result =
            Snapshotter::new(&ws)?.create_snapshot(resolve_actor(&ws.layout.draft_dir)?)?;
        let patch = diff_snapshots(&ws, &base, &result)?;
        let evidence = Evidence {
            schema_version: current_version(ContractId::PackEvidence),
            id: EvidenceId::generate(),
            pack_id: PackId::new("pending"),
            command_logs: vec![],
            files_touched: patch.files.iter().map(|f| f.path.clone()).collect(),
            generated_diff_ref: None,
            test_results: vec![],
            lint_results: vec![],
            risk_summary_ref: None,
            agent_plan_ref: None,
            agent_transcript_ref: None,
            warnings: vec!["created from task execution baseline".to_string()],
            created_at: now(),
        };
        let mut pack = PackWorkspace::new(
            ws.workspace_id.clone(),
            task_id.map(TaskId::new),
            None,
            base.id.clone(),
            result.id.clone(),
            name,
        );
        let mut evidence = evidence;
        evidence.pack_id = pack.id.clone();
        let pack_dir = ws.layout.pack_workspace_dir(&pack.id);
        ensure_dir(&pack_dir)?;
        write_json(&pack_dir.join("staging.json"), &pack)?;
        write_json(&pack_dir.join("patch.json"), &patch)?;
        write_json(&pack_dir.join("evidence.json"), &evidence)?;
        pack.patch_refs.push(patch.id.to_string());
        pack.evidence_refs.push(evidence.id.to_string());
        pack.manifest_hash = hash_json(&pack)?;
        write_json(&pack_dir.join("staging.json"), &pack)?;
        ws.events()?.append(
            "pack.created",
            Some(pack.id.to_string()),
            serde_json::to_value(&pack).expect("Draft-owned records must serialize"),
        )?;
        write_atomic(
            ws.layout.selected_pack_file().as_path(),
            pack.id.to_string().as_bytes(),
        )?;
        ws.events()?.append(
            "pack.selected",
            Some(pack.id.to_string()),
            serde_json::json!({}),
        )?;
        self.sync_canonical_pack(
            &ws,
            &pack,
            Some(&patch),
            PackSyncSpec {
                kind: crate::trust::event::EventKind::PackCreated,
                intent: crate::pack::PackIntent::Feature,
                lifecycle: crate::pack::lifecycle::PackLifecycle::Draft,
                metadata: serde_json::json!({ "name": pack.name, "base": "task_execution" }),
            },
        )?;
        Ok(pack)
    }

    pub fn pack_create_from_base(
        &self,
        cwd: &Path,
        name: String,
        base_pack_ref: Option<String>,
    ) -> DraftResult<PackWorkspace> {
        let ws = self.open(cwd)?;
        let base_ref = base_pack_ref.unwrap_or(self.selected_pack_id(cwd)?);
        let base_pack = self.resolve_pack_ref(&ws, &base_ref)?;
        let base = load_snapshot(&ws, &base_pack.result_snapshot_id)?;
        self.pack_create_with_base(cwd, Some(name), None, base)
    }

    pub fn pack_select(&self, cwd: &Path, id: &str) -> DraftResult<PackWorkspace> {
        self.pack_select_ref(cwd, id)
    }

    pub fn pack_select_ref(&self, cwd: &Path, reference: &str) -> DraftResult<PackWorkspace> {
        let ws = self.open(cwd)?;
        let pack = self.resolve_pack_ref(&ws, reference)?;
        write_atomic(
            ws.layout.selected_pack_file().as_path(),
            pack.id.to_string().as_bytes(),
        )?;
        ws.events()?.append(
            "pack.selected",
            Some(pack.id.to_string()),
            serde_json::json!({}),
        )?;
        Ok(pack)
    }

    pub fn pack_show_selected(&self, cwd: &Path) -> DraftResult<PackReport> {
        let id = self.selected_pack_id(cwd)?;
        self.pack_show(cwd, &id)
    }

    pub fn pack_delete_ref(&self, cwd: &Path, reference: &str) -> DraftResult<PackDeleteReport> {
        let ws = self.open(cwd)?;
        let pack = self.resolve_pack_ref(&ws, reference)?;
        ensure_pack_not_locked(&ws, &pack)?;
        if pack.base_snapshot_id.as_str() == "chk_empty"
            && pack.result_snapshot_id.as_str() == "chk_empty"
        {
            return Err(DraftError::invalid_config("cannot delete the base pack"));
        }
        let active = self.pack_list(cwd)?;
        if active.len() <= 1 {
            return Err(DraftError::invalid_config(
                "cannot delete the last active pack",
            ));
        }
        let selected = Some(self.selected_pack_id(cwd)?);
        let replacement = if selected.as_deref() == Some(pack.id.as_str()) {
            active
                .iter()
                .filter(|p| p.id != pack.id)
                .max_by_key(|p| p.created_at)
                .map(|p| p.id.to_string())
        } else {
            selected
        };
        let Some(replacement_id) = replacement else {
            return Err(DraftError::invalid_config(
                "cannot delete selected pack without a replacement",
            ));
        };
        let pack_dir = ws.layout.pack_workspace_dir(&pack.id);
        let deleted_files = count_files(&pack_dir)?;
        // Task definitions and execution records are independent durable
        // history and are never deleted as a side effect of pack disposal.
        let deleted_executions = 0usize;
        let deleted_tasks = 0usize;
        ws.events()?.append(
            "pack.deleted",
            Some(pack.id.to_string()),
            serde_json::json!({
                "name": pack.name,
                "replacement_selected_pack": replacement_id,
                "deleted_files": deleted_files,
                "deleted_executions": deleted_executions,
                "deleted_tasks": deleted_tasks
            }),
        )?;
        fs::remove_dir_all(&pack_dir)
            .map_err(|e| DraftError::storage(format!("failed to delete pack {}: {e}", pack.id)))?;
        write_atomic(
            ws.layout.selected_pack_file().as_path(),
            replacement_id.as_bytes(),
        )?;
        let deleted_objects = garbage_collect_objects(&ws)?;
        Ok(PackDeleteReport {
            deleted_pack_id: pack.id.to_string(),
            deleted_pack_name: pack.name,
            replacement_selected_pack: replacement_id,
            deleted_files: deleted_files + deleted_objects,
        })
    }

    pub fn selected_pack_id(&self, cwd: &Path) -> DraftResult<String> {
        let ws = self.open(cwd)?;
        let raw = fs::read_to_string(ws.layout.selected_pack_file()).map_err(|e| {
            DraftError::not_found(format!(
                "no selected pack: {e}; run `draft pack -s <pck-id/name>`"
            ))
        })?;
        Ok(raw.trim().to_string())
    }

    pub fn resolve_pack_arg(&self, cwd: &Path, pack_id: Option<&str>) -> DraftResult<String> {
        match pack_id {
            Some(id) if !id.trim().is_empty() => {
                let ws = self.open(cwd)?;
                match self.resolve_pack_ref(&ws, id) {
                    Ok(pack) => Ok(pack.id.to_string()),
                    Err(staging_error) => {
                        let store = crate::pack::PackStore::new(
                            crate::workspace::layout::DraftLayout::for_root(&ws.root),
                        );
                        let candidate = if id.starts_with("pck_") {
                            Some(id.to_string())
                        } else {
                            store
                                .list()?
                                .into_iter()
                                .chain(store.list_quarantined()?)
                                .find(|m| m.name == id)
                                .map(|m| m.pack_id)
                        };
                        match candidate {
                            Some(cid) if store.locate(&cid).is_some() => Ok(cid),
                            _ => Err(staging_error),
                        }
                    }
                }
            }
            _ => self.selected_pack_id(cwd),
        }
    }

    pub fn pack_list(&self, cwd: &Path) -> DraftResult<Vec<PackWorkspace>> {
        let ws = self.open(cwd)?;
        self.pack_list_for_workspace(&ws)
    }

    fn pack_list_for_workspace(&self, ws: &Workspace) -> DraftResult<Vec<PackWorkspace>> {
        let mut packs = Vec::new();
        if ws.layout.pack_workspaces_dir().exists() {
            for entry in fs::read_dir(ws.layout.pack_workspaces_dir())? {
                let p = entry?.path().join("staging.json");
                if p.exists() {
                    let pack: PackWorkspace = crate::contracts::read_persisted(&p)?;
                    pack.validate()?;
                    packs.push(pack);
                }
            }
        }
        packs.sort_by_key(|a: &PackWorkspace| a.created_at);
        Ok(packs)
    }

    pub fn pack_show(&self, cwd: &Path, id: &str) -> DraftResult<PackReport> {
        let ws = self.open(cwd)?;
        let pack = self.resolve_pack_ref(&ws, id)?;
        let patch = load_patch(&ws, &pack)?;
        let evidence = Some(load_evidence(&ws, &pack)?);
        Ok(PackReport {
            lifecycle: pack_lifecycle(&ws, &pack.id)?,
            pack,
            patch,
            evidence,
        })
    }

    fn ensure_unique_pack_name(&self, ws: &Workspace, name: &str) -> DraftResult<()> {
        if name.trim().is_empty() {
            return Err(DraftError::invalid_config("pack name cannot be empty"));
        }
        if self
            .pack_list_for_workspace(ws)?
            .iter()
            .any(|p| p.name.as_deref() == Some(name))
        {
            return Err(DraftError::invalid_config(format!(
                "pack name '{name}' already exists"
            )));
        }
        Ok(())
    }

    fn resolve_pack_ref(&self, ws: &Workspace, reference: &str) -> DraftResult<PackWorkspace> {
        if reference.starts_with("pck_") {
            validate_pack_id(reference)?;
            let pack = load_pack(ws, reference)?;
            return Ok(pack);
        }
        let matches: Vec<_> = self
            .pack_list_for_workspace(ws)?
            .into_iter()
            .filter(|p| p.name.as_deref() == Some(reference))
            .collect();
        match matches.len() {
            1 => Ok(matches.into_iter().next().unwrap()),
            0 => Err(DraftError::not_found(format!("unknown pack '{reference}'"))),
            _ => Err(DraftError::invalid_config(format!(
                "pack name '{reference}' is ambiguous"
            ))),
        }
    }

    pub fn risk(&self, cwd: &Path, pack_id: &str) -> DraftResult<RiskSummary> {
        self.risk_inner(cwd, pack_id, true)
    }

    fn risk_preview(&self, cwd: &Path, pack_id: &str) -> DraftResult<RiskSummary> {
        self.risk_inner(cwd, pack_id, false)
    }

    fn risk_inner(&self, cwd: &Path, pack_id: &str, persist: bool) -> DraftResult<RiskSummary> {
        let ws = self.open(cwd)?;
        validate_pack_id(pack_id)?;
        let pack = load_pack(&ws, pack_id)?;
        let patch = load_patch(&ws, &pack)?;
        let risk_config = read_or_default::<RiskConfig>(&ws.layout.risk_toml())?;
        let mut score = patch.files.len() as u32;
        let mut factors = Vec::new();
        let mut reason_codes = Vec::new();
        let mut hotspots = Vec::new();
        let mut evidence_gaps = Vec::new();
        let mut evidence_summary = Vec::new();
        if patch.files.iter().any(|f| f.binary) {
            score += 3;
            factors.push("binary files".to_string());
            reason_codes.push("binary_change".to_string());
            hotspots.extend(
                patch
                    .files
                    .iter()
                    .filter(|f| f.binary)
                    .map(|f| f.path.clone()),
            );
        }
        if patch
            .files
            .iter()
            .any(|f| matches!(f.change_kind, FileChangeKind::Deleted))
        {
            score += 2;
            factors.push("deletions".to_string());
            reason_codes.push("deletion".to_string());
            hotspots.extend(
                patch
                    .files
                    .iter()
                    .filter(|f| matches!(f.change_kind, FileChangeKind::Deleted))
                    .map(|f| f.path.clone()),
            );
        }
        if patch
            .files
            .iter()
            .any(|f| f.path.0.contains("secret") || f.path.0.contains(".env"))
        {
            score += 5;
            factors.push("sensitive paths".to_string());
            reason_codes.push("sensitive_path".to_string());
            hotspots.extend(
                patch
                    .files
                    .iter()
                    .filter(|f| f.path.0.contains("secret") || f.path.0.contains(".env"))
                    .map(|f| f.path.clone()),
            );
        }
        for rule in risk_config.path_rules.iter() {
            let matched: Vec<_> = patch
                .files
                .iter()
                .filter(|f| {
                    let lower = f.path.0.to_ascii_lowercase();
                    rule.patterns
                        .iter()
                        .any(|needle| lower.contains(&needle.to_ascii_lowercase()))
                })
                .map(|f| f.path.clone())
                .collect();
            if !matched.is_empty() {
                score += rule.weight;
                factors.push(rule.code.replace('_', " "));
                reason_codes.push(rule.code.clone());
                hotspots.extend(matched);
            }
        }
        let deleted_tests: Vec<_> = patch
            .files
            .iter()
            .filter(|f| {
                matches!(f.change_kind, FileChangeKind::Deleted)
                    && f.path.0.to_ascii_lowercase().contains("test")
            })
            .map(|f| f.path.clone())
            .collect();
        if !deleted_tests.is_empty() {
            score += 5;
            factors.push("deleted tests".to_string());
            reason_codes.push("deleted_tests".to_string());
            hotspots.extend(deleted_tests);
        }
        if patch.files.len() >= 20 {
            score += 4;
            factors.push("large change set".to_string());
            reason_codes.push("large_change_set".to_string());
        }
        if pack.verification_refs.is_empty() {
            score += 2;
            factors.push("missing verification".to_string());
            reason_codes.push("missing_verification".to_string());
            evidence_gaps.push("verification receipt missing".to_string());
        } else {
            evidence_summary.push(format!(
                "{} verification receipt(s)",
                pack.verification_refs.len()
            ));
        }
        if evidence_summary.is_empty() {
            evidence_summary.push("no verification evidence recorded".to_string());
        }
        if factors.is_empty() {
            factors.push("small text-only change".to_string());
            reason_codes.push("low_complexity".to_string());
        }
        let level = if score >= risk_config.critical_threshold {
            RiskLevel::Critical
        } else if score >= risk_config.high_threshold {
            RiskLevel::High
        } else if score >= risk_config.medium_threshold {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        };
        hotspots.sort();
        hotspots.dedup();
        let policy_decision = if matches!(level, RiskLevel::Critical | RiskLevel::High)
            && pack.verification_refs.is_empty()
        {
            "blocked_until_verified".to_string()
        } else {
            "allowed_for_review".to_string()
        };
        let mut receipt_id = "preview".to_string();
        let mut receipt = ActionReceiptDraft::new(
            "risk",
            level.as_str(),
            Some(pack.id.to_string()),
            Value::Null,
        );
        if persist {
            receipt_id = receipt.id.to_string();
        }
        let summary = RiskSummary {
            pack_id: pack.id.to_string(),
            receipt_id,
            level,
            score,
            factors,
            reason_codes,
            hotspots,
            evidence_gaps,
            evidence_summary,
            policy_decision,
            files_changed: patch.files.len(),
        };
        if persist {
            receipt.payload =
                serde_json::to_value(&summary).expect("Draft-owned records must serialize");
            write_receipt(&ws, &receipt)?;
            ws.events()?.append(
                "risk.completed",
                Some(pack.id.to_string()),
                serde_json::to_value(&summary).expect("Draft-owned records must serialize"),
            )?;
        }
        Ok(summary)
    }

    pub fn risk_selected(&self, cwd: &Path, pack_id: Option<&str>) -> DraftResult<RiskSummary> {
        let pack_id = self.resolve_pack_arg(cwd, pack_id)?;
        self.risk(cwd, &pack_id)
    }

    pub fn risk_selected_with_options(
        &self,
        cwd: &Path,
        pack_id: Option<&str>,
        explain: bool,
        include_evidence: bool,
    ) -> DraftResult<RiskSummary> {
        let mut summary = self.risk_selected(cwd, pack_id)?;
        if !explain {
            summary.factors.clear();
        }
        if !include_evidence {
            summary.evidence_summary.clear();
        }
        Ok(summary)
    }

    pub fn risk_preview_selected_with_options(
        &self,
        cwd: &Path,
        pack_id: Option<&str>,
        explain: bool,
        include_evidence: bool,
    ) -> DraftResult<RiskSummary> {
        let pack_id = self.resolve_pack_arg(cwd, pack_id)?;
        let mut summary = self.risk_preview(cwd, &pack_id)?;
        if !explain {
            summary.factors.clear();
        }
        if !include_evidence {
            summary.evidence_summary.clear();
        }
        Ok(summary)
    }

    pub fn review(
        &self,
        cwd: &Path,
        pack_id: &str,
        comment: Option<String>,
    ) -> DraftResult<ReviewReport> {
        let ws = self.open(cwd)?;
        validate_pack_id(pack_id)?;
        let mut pack = load_pack(&ws, pack_id)?;
        ensure_pack_workspace_matches_target(
            &ws,
            &pack,
            "review",
            "update the pack from the current edits or restore the workspace to the pack target before review",
        )?;
        let store = crate::pack::PackStore::new(ws.layout.clone());
        let location = store.locate(pack_id).ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                "pack staging state references a missing canonical pack",
            )
        })?;
        let mut lifecycle = store.read_lifecycle_in(location, pack_id)?;
        if lifecycle.lifecycle == PackLifecycle::Draft {
            return Err(DraftError::new(
                DraftErrorKind::ReviewRequired,
                "pack must be verified before review can start",
            ));
        }
        let mut comments = load_review_file(&ws, &pack.id)?;
        let risk = Some(self.risk_preview(cwd, pack_id)?);
        if let Some(body) = comment {
            comments.comments.push(ReviewComment {
                id: ReviewCommentId::generate(),
                pack_id: pack.id.clone(),
                path: None,
                hunk_id: None,
                actor: resolve_actor(&ws.layout.draft_dir)?,
                body,
                created_at: now(),
            });
            ws.events()?.append(
                "review.comment_added",
                Some(pack.id.to_string()),
                serde_json::json!({ "count": comments.comments.len() }),
            )?;
        } else {
            ws.events()?.append(
                "review.started",
                Some(pack.id.to_string()),
                serde_json::json!({}),
            )?;
        }
        if lifecycle.lifecycle == PackLifecycle::Verified {
            lifecycle.transition(crate::pack::lifecycle::PackTransitionRequest {
                operation_id: crate::support::common::OperationId::new(format!(
                    "op_review_{}",
                    pack.id
                )),
                expected_revision_id: lifecycle.revision_id.clone(),
                expected_revision_digest: lifecycle.revision_digest.clone(),
                target: PackLifecycle::Reviewing,
            })?;
            store.write_lifecycle_in(location, &lifecycle)?;
        }
        write_json(
            &ws.layout
                .pack_workspace_dir(&pack.id)
                .join("review.lock.json"),
            &serde_json::json!({
                "schema_version": current_version(ContractId::ReviewFile),
                "pack_id": pack.id,
                "actor": resolve_actor(&ws.layout.draft_dir)?,
                "updated_at": now()
            }),
        )?;
        save_review_file(&ws, &pack.id, &comments)?;
        let review_units = build_review_units(&ws, &pack, risk.as_ref())?;
        let risk_receipt_id = risk
            .as_ref()
            .and_then(|risk| (risk.receipt_id != "preview").then(|| risk.receipt_id.clone()));
        let receipt = ActionReceiptDraft::new(
            "review",
            "completed",
            Some(pack.id.to_string()),
            serde_json::json!({
                "review_units": review_units,
                "risk_receipt_id": risk_receipt_id,
                "comments": comments.comments.len()
            }),
        );
        let receipt_id = receipt.id.to_string();
        write_receipt(&ws, &receipt)?;
        pack.review_refs.push(receipt_id.clone());
        save_pack_staging(&ws, &mut pack)?;
        Ok(ReviewReport {
            pack_id: pack.id.to_string(),
            review_receipt_id: Some(receipt_id),
            comments: comments.comments.len(),
            decisions: comments.decisions.len(),
            status: lifecycle.lifecycle,
            review_units,
            risk_receipt_id,
        })
    }

    pub fn review_selected(
        &self,
        cwd: &Path,
        pack_id: Option<&str>,
        comment: Option<String>,
    ) -> DraftResult<ReviewReport> {
        let pack_id = self.resolve_pack_arg(cwd, pack_id)?;
        self.review(cwd, &pack_id, comment)
    }

    pub fn decide(
        &self,
        cwd: &Path,
        pack_id: &str,
        kind: DecisionKind,
        reason: Option<String>,
    ) -> DraftResult<Decision> {
        let ws = self.open(cwd)?;
        validate_pack_id(pack_id)?;
        let mut pack = load_pack(&ws, pack_id)?;
        ensure_pack_workspace_matches_target(
            &ws,
            &pack,
            decision_dirty_action(kind),
            "update the pack from the current edits or restore the workspace to the pack target before deciding",
        )?;
        let store = crate::pack::PackStore::new(ws.layout.clone());
        let location = store.locate(pack_id).ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                "pack staging state references a missing canonical pack",
            )
        })?;
        let mut lifecycle = store.read_lifecycle_in(location, pack_id)?;
        if matches!(kind, DecisionKind::Approve | DecisionKind::Reject)
            && !matches!(
                lifecycle.lifecycle,
                PackLifecycle::Reviewing | PackLifecycle::Approved | PackLifecycle::Rejected
            )
        {
            return Err(DraftError::new(
                DraftErrorKind::ReviewRequired,
                "review is required before approve/reject",
            ));
        }
        let actor = resolve_actor(&ws.layout.draft_dir)?;
        if matches!(kind, DecisionKind::Approve | DecisionKind::Reject)
            && actor.kind != ActorKind::Human
        {
            return Err(DraftError::new(
                DraftErrorKind::ReviewRequired,
                "final approve/reject requires a human actor",
            ));
        }
        let decision = Decision {
            id: DecisionId::generate(),
            pack_id: pack.id.clone(),
            actor,
            kind,
            reason,
            created_at: now(),
        };
        let mut file = load_review_file(&ws, &pack.id)?;
        file.decisions.push(decision.clone());
        save_review_file(&ws, &pack.id, &file)?;
        pack.decision_refs.push(decision.id.to_string());
        save_pack_staging(&ws, &mut pack)?;
        let review_lock = ws
            .layout
            .pack_workspace_dir(&pack.id)
            .join("review.lock.json");
        if matches!(decision.kind, DecisionKind::Approve | DecisionKind::Reject)
            && review_lock.exists()
        {
            fs::remove_file(review_lock)?;
        }
        let event = if decision.kind == DecisionKind::Approve {
            "pack.approved"
        } else if decision.kind == DecisionKind::Reject {
            "pack.rejected"
        } else {
            "review.completed"
        };
        let receipt_kind = if decision.kind == DecisionKind::Approve {
            "approval"
        } else {
            "review"
        };
        let receipt = ActionReceiptDraft::new(
            receipt_kind,
            decision.kind.label(),
            Some(pack.id.to_string()),
            serde_json::json!({
                "decision": decision,
                "review_refs": pack.review_refs,
                "verification_refs": pack.verification_refs,
            }),
        );
        write_receipt(&ws, &receipt)?;
        if matches!(decision.kind, DecisionKind::Approve | DecisionKind::Reject) {
            lifecycle.transition(crate::pack::lifecycle::PackTransitionRequest {
                operation_id: crate::support::common::OperationId::new(decision.id.as_str()),
                expected_revision_id: lifecycle.revision_id.clone(),
                expected_revision_digest: lifecycle.revision_digest.clone(),
                target: if decision.kind == DecisionKind::Approve {
                    PackLifecycle::Approved
                } else {
                    PackLifecycle::Rejected
                },
            })?;
            store.write_lifecycle_in(location, &lifecycle)?;
        }
        if matches!(decision.kind, DecisionKind::Approve | DecisionKind::Reject) {
            let base = crate::workspace::stable::StableHeadStore::new(
                crate::workspace::layout::DraftLayout::for_root(&ws.root),
            )
            .read()?
            .stable_head_hash;
            let kind = if decision.kind == DecisionKind::Approve {
                crate::review::workflow::DecisionType::Approve
            } else {
                crate::review::workflow::DecisionType::Reject
            };
            let mut record = crate::review::workflow::new_decision(
                kind,
                pack.id.to_string(),
                base,
                format!("{:?}", decision.actor),
                decision
                    .reason
                    .clone()
                    .unwrap_or_else(|| decision.kind.label().to_string()),
            )?;
            record.receipt_id = Some(receipt.id.to_string());
            crate::review::workflow::WorkflowStore::for_root(&ws.root).write_decision(&record)?;
        }
        ws.events()?.append(
            event,
            Some(pack.id.to_string()),
            serde_json::to_value(&decision).expect("Draft-owned records must serialize"),
        )?;
        Ok(decision)
    }

    pub fn decide_selected(
        &self,
        cwd: &Path,
        pack_id: Option<&str>,
        kind: DecisionKind,
        reason: Option<String>,
    ) -> DraftResult<Decision> {
        let pack_id = self.resolve_pack_arg(cwd, pack_id)?;
        self.decide(cwd, &pack_id, kind, reason)
    }

    pub fn compare(&self, cwd: &Path, left: &str, right: &str) -> DraftResult<CompareReport> {
        let ws = self.open(cwd)?;
        let l = self.resolve_pack_ref(&ws, left)?;
        let r = self.resolve_pack_ref(&ws, right)?;
        let lp = load_patch(&ws, &l)?;
        let rp = load_patch(&ws, &r)?;
        let lf: BTreeSet<_> = lp.files.iter().map(|f| f.path.clone()).collect();
        let rf: BTreeSet<_> = rp.files.iter().map(|f| f.path.clone()).collect();
        let overlapping_files: Vec<_> = lf.intersection(&rf).cloned().collect();
        let overlapping_hunks = hunk_overlaps(&lp, &rp);
        let mut warnings = Vec::new();
        for path in &overlapping_files {
            let left_file = lp.files.iter().find(|f| &f.path == path);
            let right_file = rp.files.iter().find(|f| &f.path == path);
            if let (Some(lf), Some(rf)) = (left_file, right_file) {
                if file_level_conflict(lf, rf) {
                    warnings.push(format!("{path}: non-text or whole-file overlap"));
                }
            }
        }
        if !overlapping_hunks.is_empty() {
            warnings.push(format!(
                "{} overlapping text hunk(s)",
                overlapping_hunks.len()
            ));
        }
        let compatible = warnings.is_empty();
        let report = CompareReport {
            id: format!("cmp_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
            left_pack: l.id.to_string(),
            right_pack: r.id.to_string(),
            overlapping_files,
            overlapping_hunks,
            unique_left_files: lf.difference(&rf).cloned().collect(),
            unique_right_files: rf.difference(&lf).cloned().collect(),
            compatible,
            warnings,
            recommendation: Some(if compatible {
                "compose is allowed".to_string()
            } else {
                "resolve overlaps before compose".to_string()
            }),
        };
        ws.events()?.append(
            "compare.completed",
            None,
            serde_json::to_value(&report).expect("Draft-owned records must serialize"),
        )?;
        Ok(report)
    }

    pub fn compose(
        &self,
        cwd: &Path,
        left: &str,
        right: &str,
        output: &str,
    ) -> DraftResult<ComposeResult> {
        let ws = self.open(cwd)?;
        let l = self.resolve_pack_ref(&ws, left)?;
        let r = self.resolve_pack_ref(&ws, right)?;
        ensure_pack_not_locked(&ws, &l)?;
        ensure_pack_not_locked(&ws, &r)?;
        let project_paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        let compose_wsh = crate::workspace::source_view::workspace_hash_cached(
            &ws.root,
            &project_paths.workspace_hash_cache(),
        )?;
        ledger.record(
            crate::trust::event::EventKind::CompositionCreated,
            Some(format!("{}+{}", l.id, r.id)),
            None,
            compose_wsh.clone(),
            serde_json::json!({ "sources": [l.id.to_string(), r.id.to_string()] }),
        )?;
        let composition_failed = |reason: &str| -> DraftResult<()> {
            ledger.record(
                crate::trust::event::EventKind::CompositionFailed,
                Some(format!("{}+{}", l.id, r.id)),
                None,
                compose_wsh.clone(),
                serde_json::json!({
                    "sources": [l.id.to_string(), r.id.to_string()],
                    "reason": reason,
                }),
            )?;
            Ok(())
        };
        let l_base = load_snapshot(&ws, &l.base_snapshot_id)?;
        let r_base = load_snapshot(&ws, &r.base_snapshot_id)?;
        if snapshot_file_fingerprint(&l_base) != snapshot_file_fingerprint(&r_base) {
            composition_failed("compose requires packs with the same base content")?;
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "compose requires packs with the same base content",
            ));
        }
        let lp = load_patch(&ws, &l)?;
        let rp = load_patch(&ws, &r)?;
        let cmp = self.compare(cwd, left, right)?;
        if !cmp.compatible {
            composition_failed("compose has overlapping changes")?;
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "compose has overlapping changes",
            )
            .with_context(format!("{:?}", cmp.warnings)));
        }
        ledger.record(
            crate::trust::event::EventKind::CompositionVerified,
            Some(format!("{}+{}", l.id, r.id)),
            None,
            compose_wsh.clone(),
            serde_json::json!({
                "sources": [l.id.to_string(), r.id.to_string()],
                "compare": cmp.id,
            }),
        )?;
        let mut files = lp.files.clone();
        files.extend(rp.files.clone());
        files.sort_by(|a, b| a.path.cmp(&b.path).then(a.old_path.cmp(&b.old_path)));
        let mut patch = PatchSet {
            schema_version: current_version(ContractId::PatchSet),
            id: PatchSetId::generate(),
            base_snapshot_id: l.base_snapshot_id.clone(),
            result_snapshot_id: r.result_snapshot_id.clone(),
            files,
            patch_graph_hash: String::new(),
        };
        patch.patch_graph_hash = hash_json(&patch)?;
        let evidence = Evidence {
            schema_version: current_version(ContractId::PackEvidence),
            id: EvidenceId::generate(),
            pack_id: PackId::new("pending"),
            command_logs: vec![],
            files_touched: patch.files.iter().map(|f| f.path.clone()).collect(),
            generated_diff_ref: None,
            test_results: vec![],
            lint_results: vec![],
            risk_summary_ref: None,
            agent_plan_ref: None,
            agent_transcript_ref: None,
            warnings: vec!["composed from compatible packs".to_string()],
            created_at: now(),
        };
        let mut pack = PackWorkspace::new(
            ws.workspace_id.clone(),
            l.task_id.clone().or_else(|| r.task_id.clone()),
            None,
            l.base_snapshot_id.clone(),
            r.result_snapshot_id.clone(),
            Some(output.to_string()),
        );
        let mut evidence = evidence;
        evidence.pack_id = pack.id.clone();
        pack.source_pack_ids = vec![l.id.to_string(), r.id.to_string()];
        pack.patch_refs.push(patch.id.to_string());
        pack.evidence_refs.push(evidence.id.to_string());
        let pack_dir = ws.layout.pack_workspace_dir(&pack.id);
        ensure_dir(&pack_dir)?;
        write_json(&pack_dir.join("patch.json"), &patch)?;
        write_json(&pack_dir.join("evidence.json"), &evidence)?;
        save_pack_staging(&ws, &mut pack)?;
        self.sync_canonical_pack(
            &ws,
            &pack,
            Some(&patch),
            PackSyncSpec {
                kind: crate::trust::event::EventKind::PackComposed,
                intent: crate::pack::PackIntent::Feature,
                lifecycle: crate::pack::lifecycle::PackLifecycle::Draft,
                metadata: serde_json::json!({
                    "sources": pack.source_pack_ids,
                    "compare": cmp.id,
                }),
            },
        )?;
        let receipt = ActionReceiptDraft::new(
            "compose",
            "completed",
            Some(pack.id.to_string()),
            serde_json::json!({
                "sources": pack.source_pack_ids,
                "files": patch.files.len(),
                "compare": cmp.id
            }),
        );
        write_receipt(&ws, &receipt)?;
        ws.events()?.append(
            "compose.completed",
            Some(pack.id.to_string()),
            serde_json::json!({ "receipt_id": receipt.id.to_string() }),
        )?;
        Ok(ComposeResult {
            output_pack_id: pack.id.to_string(),
            source_packs: pack.source_pack_ids,
            receipt_id: receipt.id.to_string(),
            files: patch.files.len(),
            compatible: true,
            requires_verification: true,
            requires_review: true,
            final_success: false,
        })
    }

    pub fn disperse(
        &self,
        cwd: &Path,
        pack_id: &str,
        output_a: &str,
        output_b: &str,
    ) -> DraftResult<DisperseResult> {
        let ws = self.open(cwd)?;
        let source = self.resolve_pack_ref(&ws, pack_id)?;
        ensure_pack_not_locked(&ws, &source)?;
        let mut left = PackWorkspace::new(
            ws.workspace_id.clone(),
            source.task_id.clone(),
            source.execution_id.clone(),
            source.base_snapshot_id.clone(),
            source.result_snapshot_id.clone(),
            Some(output_a.to_string()),
        );
        left.source_pack_ids = vec![source.id.to_string()];
        let mut right = PackWorkspace::new(
            ws.workspace_id.clone(),
            source.task_id.clone(),
            source.execution_id.clone(),
            source.base_snapshot_id.clone(),
            source.result_snapshot_id.clone(),
            Some(output_b.to_string()),
        );
        right.source_pack_ids = vec![source.id.to_string()];
        ensure_dir(&ws.layout.pack_workspace_dir(&left.id))?;
        ensure_dir(&ws.layout.pack_workspace_dir(&right.id))?;
        let patch = load_patch(&ws, &source)?;
        let mut left_files = Vec::new();
        let mut right_files = Vec::new();
        for (idx, file) in patch.files.into_iter().enumerate() {
            if idx % 2 == 0 {
                left_files.push(file);
            } else {
                right_files.push(file);
            }
        }
        if right_files.is_empty() && left_files.len() > 1 {
            if let Some(file) = left_files.pop() {
                right_files.push(file);
            }
        }
        let left_patch = split_patch(&source, left_files)?;
        let right_patch = split_patch(&source, right_files)?;
        let left_evidence = split_evidence(&left, &left_patch, "dispersed output A");
        let right_evidence = split_evidence(&right, &right_patch, "dispersed output B");
        left.patch_refs.push(left_patch.id.to_string());
        right.patch_refs.push(right_patch.id.to_string());
        left.evidence_refs.push(left_evidence.id.to_string());
        right.evidence_refs.push(right_evidence.id.to_string());
        write_json(
            &ws.layout.pack_workspace_dir(&left.id).join("patch.json"),
            &left_patch,
        )?;
        write_json(
            &ws.layout.pack_workspace_dir(&right.id).join("patch.json"),
            &right_patch,
        )?;
        write_json(
            &ws.layout.pack_workspace_dir(&left.id).join("evidence.json"),
            &left_evidence,
        )?;
        write_json(
            &ws.layout
                .pack_workspace_dir(&right.id)
                .join("evidence.json"),
            &right_evidence,
        )?;
        save_pack_staging(&ws, &mut left)?;
        save_pack_staging(&ws, &mut right)?;
        for (output, output_patch) in [(&left, &left_patch), (&right, &right_patch)] {
            self.sync_canonical_pack(
                &ws,
                output,
                Some(output_patch),
                PackSyncSpec {
                    kind: crate::trust::event::EventKind::PackDispersed,
                    intent: crate::pack::PackIntent::Feature,
                    lifecycle: crate::pack::lifecycle::PackLifecycle::Draft,
                    metadata: serde_json::json!({
                        "source": source.id,
                        "output": output.id,
                    }),
                },
            )?;
        }
        let receipt = ActionReceiptDraft::new(
            "disperse",
            "completed",
            Some(source.id.to_string()),
            serde_json::json!({ "outputs": [left.id.to_string(), right.id.to_string()] }),
        );
        write_receipt(&ws, &receipt)?;
        ws.events()?.append(
            "disperse.completed",
            Some(source.id.to_string()),
            serde_json::json!({ "receipt_id": receipt.id.to_string() }),
        )?;
        Ok(DisperseResult {
            source_pack_id: source.id.to_string(),
            output_pack_ids: vec![left.id.to_string(), right.id.to_string()],
            receipt_id: receipt.id.to_string(),
            requires_verification: true,
            requires_review: true,
            final_success: false,
        })
    }

    pub fn submit(
        &self,
        cwd: &Path,
        pack_id: &str,
        vars: BTreeMap<String, String>,
    ) -> DraftResult<SubmitRecord> {
        let ws = self.open(cwd)?;
        let project_paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let _submit_lock =
            FileGuard::acquire(&project_paths.lock_file("submit"), Duration::from_secs(30))?;
        validate_pack_id(pack_id)?;
        // Imported packs take the canonical import-submit path: gates, content
        // application from embedded objects, and promotion out of quarantine.
        {
            let store = crate::pack::PackStore::new(
                crate::workspace::layout::DraftLayout::for_root(&ws.root),
            );
            if let Some(loc) = store.locate(pack_id) {
                let manifest = store.read_manifest_in(loc, pack_id)?;
                if store.quarantine_record(pack_id)?.is_some() {
                    return self.submit_imported_pack(&ws, &store, loc, manifest);
                }
            }
        }
        let mut pack = load_pack(&ws, pack_id)?;
        ensure_pack_not_locked(&ws, &pack)?;
        ensure_pack_workspace_matches_target(
            &ws,
            &pack,
            "submit",
            "update the pack from the current edits or restore the workspace to the approved pack target before submit",
        )?;
        validate_canonical_submit_gate(&ws, pack_id)?;
        let started = now();
        let submit_started_event_id = ws.events()?.append(
            "submit.started",
            Some(pack.id.to_string()),
            serde_json::json!({}),
        )?;
        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        ledger.record(
            crate::trust::event::EventKind::SubmitStarted,
            Some(pack.id.to_string()),
            None,
            crate::workspace::source_view::workspace_hash_cached(
                &ws.root,
                &project_paths.workspace_hash_cache(),
            )?,
            serde_json::json!({ "orchestration_event_id": submit_started_event_id.to_string() }),
        )?;
        let cfg = ResolvedConfig::load(&ws)?;
        let policy = effective_policy(&ws)?;
        let patch = load_patch(&ws, &pack)?;
        if patch.files.iter().any(|f| is_draft_path(f.path.as_str())) {
            let receipt = failed_submit(
                &ws,
                &pack,
                started,
                "Warning: .draft/ is included in the submit candidate.",
            )?;
            ws.events()?.append(
                "submit.completed",
                Some(pack.id.to_string()),
                serde_json::to_value(&receipt).expect("Draft-owned records must serialize"),
            )?;
            return Err(DraftError::new(DraftErrorKind::SubmitFailed, ".draft/ is included in the submit candidate.\n\nDraft metadata must never be submitted into an external repository or external system.\n\nSubmit aborted."));
        }
        let readiness = submit_readiness(&ws, &pack, &patch, &policy)?;
        if readiness.verification_receipt_id.is_none() {
            let reason = readiness
                .blockers
                .first()
                .cloned()
                .unwrap_or_else(|| "verification is required before submit".to_string());
            let receipt = failed_submit(&ws, &pack, started, &reason)?;
            ws.events()?.append(
                "submit.completed",
                Some(pack.id.to_string()),
                serde_json::to_value(&receipt).expect("Draft-owned records must serialize"),
            )?;
            return Err(DraftError::new(DraftErrorKind::VerificationFailed, reason));
        }
        if policy.require_approval_for_submit && readiness.approval_ref.is_none() {
            let reason = readiness
                .blockers
                .iter()
                .find(|blocker| blocker.contains("approval") || blocker.contains("review"))
                .cloned()
                .unwrap_or_else(|| "approval is required before submit".to_string());
            let receipt = failed_submit(&ws, &pack, started, &reason)?;
            ws.events()?.append(
                "submit.completed",
                Some(pack.id.to_string()),
                serde_json::to_value(&receipt).expect("Draft-owned records must serialize"),
            )?;
            return Err(DraftError::new(DraftErrorKind::ReviewRequired, reason));
        }
        let risk_summary = Some(self.risk(cwd, pack_id).map_err(|e| {
            DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                format!("risk evaluation failed before submit: {e}"),
            )
        })?);
        if risk_summary
            .as_ref()
            .map(|risk| risk.policy_decision.starts_with("blocked"))
            .unwrap_or(false)
        {
            let receipt = failed_submit(&ws, &pack, started, "risk policy blocks submit")?;
            ws.events()?.append(
                "submit.completed",
                Some(pack.id.to_string()),
                serde_json::to_value(&receipt).expect("Draft-owned records must serialize"),
            )?;
            return Err(DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                "risk policy blocks submit",
            ));
        }
        let receipt_id = ReceiptId::generate();
        let rendered_message = render_message(&ws, &cfg, &pack, &patch, &receipt_id)?;
        let store = ObjectStore::new(ws.layout.clone());
        let message_ref = store.put_bytes(rendered_message.as_bytes())?;
        let mut receipt = SubmitRecord {
            schema_version: current_version(ContractId::SubmitRecord),
            id: receipt_id,
            pack_id: pack.id.clone(),
            actor_id: resolve_actor(&ws.layout.draft_dir)?.id,
            native_submit_status: NativeSubmitStatus::Submitted,
            hook_status: HookStatus::NotConfigured,
            overall_status: SubmitOverallStatus::Submitted,
            message_ref: message_ref.clone(),
            hook_results: Vec::new(),
            hook_receipt_refs: Vec::new(),
            object_refs: vec![message_ref.clone()],
            event_refs: vec![submit_started_event_id.to_string()],
            risk_level: risk_summary
                .as_ref()
                .map(|risk| risk.level.as_str().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            risk_receipt_id: risk_summary.as_ref().map(|risk| risk.receipt_id.clone()),
            started_at: started,
            ended_at: now(),
            record_digest: String::new(),
            failure_reason: None,
        };
        for hook in cfg.submit_hooks(SubmitHookPhase::Before) {
            let ctx = HookContext {
                message: rendered_message.clone(),
                title: pack.name.clone().unwrap_or_else(|| pack.id.to_string()),
                description: String::new(),
                task_id: pack
                    .task_id
                    .as_ref()
                    .map(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                execution_id: pack
                    .execution_id
                    .as_ref()
                    .map(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                pack_id: pack.id.to_string(),
                receipt_id: receipt.id.to_string(),
                actor_name: resolve_actor(&ws.layout.draft_dir)?.id.to_string(),
                timestamp: now().to_rfc3339(),
                verified: (!pack.verification_refs.is_empty()).to_string(),
                risk_level: risk_summary
                    .as_ref()
                    .map(|risk| risk.level.as_str().to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                files_changed: patch.files.len().to_string(),
                workspace_root: ws.root.display().to_string(),
                hook_name: "submit.before".to_string(),
                hook_phase: SubmitHookPhase::Before.as_str().to_string(),
                vars: vars.clone(),
            };
            ledger.record(
                crate::trust::event::EventKind::SubmitHookStarted,
                Some(pack.id.to_string()),
                None,
                crate::workspace::source_view::workspace_hash_cached(
                    &ws.root,
                    &project_paths.workspace_hash_cache(),
                )?,
                serde_json::json!({ "phase": "before", "command": hook.command }),
            )?;
            match run_hook(&ws, &store, "submit.before", &hook, &ctx) {
                Ok(result) => {
                    let failed = result.exit_code != 0;
                    ledger.record(
                        if failed {
                            crate::trust::event::EventKind::SubmitHookFailed
                        } else {
                            crate::trust::event::EventKind::SubmitHookCompleted
                        },
                        Some(pack.id.to_string()),
                        None,
                        crate::workspace::source_view::workspace_hash_cached(
                            &ws.root,
                            &project_paths.workspace_hash_cache(),
                        )?,
                        serde_json::json!({
                            "phase": "before",
                            "command": hook.command,
                            "exit_code": result.exit_code,
                        }),
                    )?;
                    let hook_receipt = ActionReceiptDraft::new(
                        "hook",
                        if failed { "failed" } else { "succeeded" },
                        Some(pack.id.to_string()),
                        serde_json::to_value(&result).expect("Draft-owned records must serialize"),
                    );
                    let hook_receipt_id = hook_receipt.id.to_string();
                    write_receipt(&ws, &hook_receipt)?;
                    receipt.hook_receipt_refs.push(hook_receipt_id);
                    receipt.hook_results.push(result);
                    if failed {
                        receipt.hook_status = HookStatus::Failed;
                        if hook.continue_on_error {
                            receipt.overall_status = SubmitOverallStatus::SubmittedWithHookFailure;
                        } else {
                            receipt.overall_status = SubmitOverallStatus::Failed;
                            receipt.failure_reason = Some("hooks.submit.before failed".to_string());
                            receipt.ended_at = now();
                            receipt.record_digest = hash_json(&receipt)?;
                            write_submit_record(&ws, &receipt)?;
                            ws.events()?.append(
                                "submit.completed",
                                Some(pack.id.to_string()),
                                serde_json::to_value(&receipt)
                                    .expect("Draft-owned records must serialize"),
                            )?;
                            return Err(DraftError::new(
                                DraftErrorKind::SubmitFailed,
                                "hooks.submit.before failed",
                            ));
                        }
                    } else {
                        receipt.hook_status = HookStatus::Succeeded;
                    }
                }
                Err(e) => {
                    receipt.hook_status = HookStatus::Failed;
                    ledger.record(
                        crate::trust::event::EventKind::SubmitHookFailed,
                        Some(pack.id.to_string()),
                        None,
                        crate::workspace::source_view::workspace_hash_cached(
                            &ws.root,
                            &project_paths.workspace_hash_cache(),
                        )?,
                        serde_json::json!({
                            "phase": "before",
                            "command": hook.command,
                            "error": e.message,
                        }),
                    )?;
                    let hook_receipt = ActionReceiptDraft::new(
                        "hook",
                        "failed",
                        Some(pack.id.to_string()),
                        serde_json::json!({
                            "hook_name": "submit",
                            "hook_phase": hook.phase,
                            "error": e.message
                        }),
                    );
                    let hook_receipt_id = hook_receipt.id.to_string();
                    write_receipt(&ws, &hook_receipt)?;
                    receipt.hook_receipt_refs.push(hook_receipt_id);
                    if hook.continue_on_error {
                        receipt.overall_status = SubmitOverallStatus::SubmittedWithHookFailure;
                        receipt.failure_reason = Some(e.message);
                    } else {
                        receipt.overall_status = SubmitOverallStatus::Failed;
                        receipt.failure_reason = Some(e.message.clone());
                        receipt.ended_at = now();
                        receipt.record_digest = hash_json(&receipt)?;
                        write_submit_record(&ws, &receipt)?;
                        ws.events()?.append(
                            "submit.completed",
                            Some(pack.id.to_string()),
                            serde_json::to_value(&receipt)
                                .expect("Draft-owned records must serialize"),
                        )?;
                        return Err(DraftError::new(DraftErrorKind::SubmitFailed, e.message));
                    }
                }
            }
        }
        let stable_store = crate::workspace::stable::StableHeadStore::new(project_paths.clone());
        let previous_head = if stable_store.exists() {
            Some(stable_store.read()?)
        } else {
            None
        };
        let submit_mode = cfg.pack_disposal;
        let pack_digest = Some(hash_json(&serde_json::json!({
            "pack": pack,
            "patch": patch,
            "submit_receipt": receipt.id.to_string()
        }))?);
        let affected_paths = patch
            .files
            .iter()
            .map(|f| f.path.as_str().to_string())
            .filter(|p| !is_draft_path(p.as_str()))
            .collect::<Vec<_>>();
        let pack_summary = Some(crate::workspace::stable::PackSummary {
            pack_id: pack_id.to_string(),
            name: pack.name.clone(),
            affected_paths: affected_paths.clone(),
        });
        ledger.record(
            crate::trust::event::EventKind::ProjectStateVerificationStarted,
            Some(pack_id.to_string()),
            None,
            crate::workspace::source_view::workspace_hash_cached(
                &ws.root,
                &project_paths.workspace_hash_cache(),
            )?,
            serde_json::json!({ "submit_receipt": receipt.id.to_string() }),
        )?;
        // Project-state verification (SRS-FR-083–086): pack validity is not
        // project stability — re-verify the composed final state before any
        // receipt is written or stable_head advances. Failure preserves the
        // pack and leaves stable_head unchanged.
        let ps_report = crate::review::verification::verify_project_state(
            &ws.root,
            &project_paths,
            ws.workspace_id.as_str(),
            &affected_paths,
            true,
        )?;
        if !ps_report.passed {
            let failed_names = ps_report.failed_checks().join(", ");
            ledger.record(
                crate::trust::event::EventKind::ProjectStateVerificationFailed,
                Some(pack_id.to_string()),
                None,
                ps_report.workspace_hash.clone(),
                serde_json::json!({
                    "submit_receipt": receipt.id.to_string(),
                    "checks": ps_report.checks,
                    "failed": ps_report.failed_checks(),
                }),
            )?;
            receipt.overall_status = SubmitOverallStatus::Failed;
            receipt.failure_reason =
                Some(format!("project-state verification failed: {failed_names}"));
            receipt.ended_at = now();
            receipt.record_digest = hash_json(&receipt)?;
            write_submit_record(&ws, &receipt)?;
            ws.events()?.append(
                "submit.completed",
                Some(pack.id.to_string()),
                serde_json::to_value(&receipt).expect("Draft-owned records must serialize"),
            )?;
            return Err(DraftError::new(
                DraftErrorKind::VerificationFailed,
                format!(
                    "project-state verification failed: {failed_names}\n\nThe pack was preserved and stable_head was not advanced."
                ),
            ));
        }
        let project_verified = ledger.record(
            crate::trust::event::EventKind::ProjectStateVerified,
            Some(pack_id.to_string()),
            None,
            ps_report.workspace_hash.clone(),
            serde_json::json!({
                "submit_receipt": receipt.id.to_string(),
                "submit_mode": submit_mode.as_str(),
                "checks": ps_report.checks,
            }),
        )?;
        // Advance stable_head only after successful project-state verification
        // (SRS-FR-050). After-submit hooks run post-advance but pre-disposal
        // (TDD §13.1/§14.3), so a failed after hook preserves pack metadata.
        if submit_mode == crate::workspace::stable::SubmitMode::MergeAndDispose {
            let stable_head = stable_store.advance(
                &ws.root,
                project_verified.receipt.receipt_id.clone(),
                previous_head,
                pack_digest,
                pack_summary,
                submit_mode,
            )?;
            ledger.record(
                crate::trust::event::EventKind::StableHeadAdvanced,
                Some(stable_head.id.clone()),
                None,
                stable_head.workspace_hash.clone(),
                serde_json::json!({
                    "stable_head": stable_head.id,
                    "project_state_receipt": project_verified.receipt.receipt_id,
                    "submit_receipt": receipt.id.to_string()
                }),
            )?;
        }
        for hook in cfg.submit_hooks(SubmitHookPhase::After) {
            let ctx = HookContext {
                message: rendered_message.clone(),
                title: pack.name.clone().unwrap_or_else(|| pack.id.to_string()),
                description: String::new(),
                task_id: pack
                    .task_id
                    .as_ref()
                    .map(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                execution_id: pack
                    .execution_id
                    .as_ref()
                    .map(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                pack_id: pack.id.to_string(),
                receipt_id: receipt.id.to_string(),
                actor_name: resolve_actor(&ws.layout.draft_dir)?.id.to_string(),
                timestamp: now().to_rfc3339(),
                verified: (!pack.verification_refs.is_empty()).to_string(),
                risk_level: risk_summary
                    .as_ref()
                    .map(|risk| risk.level.as_str().to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                files_changed: patch.files.len().to_string(),
                workspace_root: ws.root.display().to_string(),
                hook_name: "submit.after".to_string(),
                hook_phase: SubmitHookPhase::After.as_str().to_string(),
                vars: vars.clone(),
            };
            ledger.record(
                crate::trust::event::EventKind::SubmitHookStarted,
                Some(pack.id.to_string()),
                None,
                crate::workspace::source_view::workspace_hash_cached(
                    &ws.root,
                    &project_paths.workspace_hash_cache(),
                )?,
                serde_json::json!({ "phase": "after", "command": hook.command }),
            )?;
            match run_hook(&ws, &store, "submit.after", &hook, &ctx) {
                Ok(result) => {
                    let failed = result.exit_code != 0;
                    ledger.record(
                        if failed {
                            crate::trust::event::EventKind::SubmitHookFailed
                        } else {
                            crate::trust::event::EventKind::SubmitHookCompleted
                        },
                        Some(pack.id.to_string()),
                        None,
                        crate::workspace::source_view::workspace_hash_cached(
                            &ws.root,
                            &project_paths.workspace_hash_cache(),
                        )?,
                        serde_json::json!({
                            "phase": "after",
                            "command": hook.command,
                            "exit_code": result.exit_code,
                        }),
                    )?;
                    let hook_receipt = ActionReceiptDraft::new(
                        "hook",
                        if failed { "failed" } else { "succeeded" },
                        Some(pack.id.to_string()),
                        serde_json::to_value(&result).expect("Draft-owned records must serialize"),
                    );
                    let hook_receipt_id = hook_receipt.id.to_string();
                    write_receipt(&ws, &hook_receipt)?;
                    receipt.hook_receipt_refs.push(hook_receipt_id);
                    receipt.hook_results.push(result);
                    if failed {
                        receipt.hook_status = HookStatus::Failed;
                        if hook.continue_on_error {
                            receipt.overall_status = SubmitOverallStatus::SubmittedWithHookFailure;
                        } else {
                            receipt.overall_status = SubmitOverallStatus::Failed;
                            receipt.failure_reason = Some("hooks.submit.after failed".to_string());
                            receipt.ended_at = now();
                            receipt.record_digest = hash_json(&receipt)?;
                            write_submit_record(&ws, &receipt)?;
                            ws.events()?.append(
                                "submit.completed",
                                Some(pack.id.to_string()),
                                serde_json::to_value(&receipt)
                                    .expect("Draft-owned records must serialize"),
                            )?;
                            return Err(DraftError::new(
                                DraftErrorKind::SubmitFailed,
                                "hooks.submit.after failed",
                            ));
                        }
                    } else {
                        receipt.hook_status = HookStatus::Succeeded;
                    }
                }
                Err(e) => {
                    receipt.hook_status = HookStatus::Failed;
                    ledger.record(
                        crate::trust::event::EventKind::SubmitHookFailed,
                        Some(pack.id.to_string()),
                        None,
                        crate::workspace::source_view::workspace_hash_cached(
                            &ws.root,
                            &project_paths.workspace_hash_cache(),
                        )?,
                        serde_json::json!({
                            "phase": "after",
                            "command": hook.command,
                            "error": e.message,
                        }),
                    )?;
                    let hook_receipt = ActionReceiptDraft::new(
                        "hook",
                        "failed",
                        Some(pack.id.to_string()),
                        serde_json::json!({
                            "hook_name": "submit.after",
                            "hook_phase": SubmitHookPhase::After.as_str(),
                            "error": e.message
                        }),
                    );
                    let hook_receipt_id = hook_receipt.id.to_string();
                    write_receipt(&ws, &hook_receipt)?;
                    receipt.hook_receipt_refs.push(hook_receipt_id);
                    if hook.continue_on_error {
                        receipt.overall_status = SubmitOverallStatus::SubmittedWithHookFailure;
                        receipt.failure_reason = Some(e.message);
                    } else {
                        receipt.overall_status = SubmitOverallStatus::Failed;
                        receipt.failure_reason = Some(e.message.clone());
                        receipt.ended_at = now();
                        receipt.record_digest = hash_json(&receipt)?;
                        write_submit_record(&ws, &receipt)?;
                        ws.events()?.append(
                            "submit.completed",
                            Some(pack.id.to_string()),
                            serde_json::to_value(&receipt)
                                .expect("Draft-owned records must serialize"),
                        )?;
                        return Err(DraftError::new(DraftErrorKind::SubmitFailed, e.message));
                    }
                }
            }
        }
        receipt.ended_at = now();
        receipt.record_digest = hash_json(&receipt)?;
        write_submit_record(&ws, &receipt)?;
        pack.receipt_refs.push(receipt.id.to_string());
        save_pack_staging(&ws, &mut pack)?;
        ws.events()?.append(
            "submit.completed",
            Some(pack.id.to_string()),
            serde_json::to_value(&receipt).expect("Draft-owned records must serialize"),
        )?;
        self.finalize_canonical_pack(&ws, &pack, &receipt)?;
        ledger.record(
            crate::trust::event::EventKind::SubmitFinalized,
            Some(pack_id.to_string()),
            None,
            crate::workspace::source_view::workspace_hash_cached(
                &ws.root,
                &project_paths.workspace_hash_cache(),
            )?,
            serde_json::json!({ "submit_receipt": receipt.id.to_string() }),
        )?;
        match dispose_pack_metadata(&ws, &project_paths, pack_id) {
            Ok(removed) => {
                ledger.record(
                    crate::trust::event::EventKind::PackDisposed,
                    Some(pack_id.to_string()),
                    None,
                    crate::workspace::source_view::workspace_hash_cached(
                        &ws.root,
                        &project_paths.workspace_hash_cache(),
                    )?,
                    serde_json::json!({ "removed_entries": removed }),
                )?;
            }
            Err(e) => {
                ledger.record(
                    crate::trust::event::EventKind::PackDisposalFailed,
                    Some(pack_id.to_string()),
                    None,
                    crate::workspace::source_view::workspace_hash_cached(
                        &ws.root,
                        &project_paths.workspace_hash_cache(),
                    )?,
                    serde_json::json!({ "error": e.message }),
                )?;
                return Err(e);
            }
        }
        Ok(receipt)
    }

    /// Bind submit to the already-approved immutable revision. Submit hooks
    /// may create workspace files, but those post-decision side effects must
    /// never derive or silently replace the revision that was reviewed.
    fn finalize_canonical_pack(
        &self,
        ws: &Workspace,
        pack: &PackWorkspace,
        receipt: &SubmitRecord,
    ) -> DraftResult<()> {
        use crate::pack::lifecycle::{PackLifecycle, PackTransitionRequest};

        let store = crate::pack::PackStore::new(ws.layout.clone());
        let location = store.locate(pack.id.as_str()).ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                "pack staging state references a missing canonical pack",
            )
        })?;
        let revision = store.current_revision_in(location, pack.id.as_str())?;
        let mut lifecycle = store.read_lifecycle_in(location, pack.id.as_str())?;
        if lifecycle.lifecycle != PackLifecycle::Approved {
            return Err(DraftError::new(
                DraftErrorKind::ReviewRequired,
                "the exact immutable pack revision is not approved for submit",
            ));
        }
        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        let outcome = ledger.record(
            crate::trust::event::EventKind::PackSubmitted,
            Some(pack.id.to_string()),
            None,
            crate::workspace::source_view::workspace_hash(&ws.root)?,
            serde_json::json!({ "submit_receipt": receipt.id.to_string() }),
        )?;
        lifecycle.transition(PackTransitionRequest {
            operation_id: crate::support::common::OperationId::new(&outcome.event.event_id),
            expected_revision_id: revision.revision_id,
            expected_revision_digest: revision.revision_digest,
            target: PackLifecycle::Submitted,
        })?;
        store.write_lifecycle_in(location, &lifecycle)
    }

    /// Materialize or advance the canonical immutable pack/revision and its
    /// separate lifecycle record, then record the signed trust event.
    fn sync_canonical_pack(
        &self,
        ws: &Workspace,
        pack: &PackWorkspace,
        patch: Option<&PatchSet>,
        spec: PackSyncSpec,
    ) -> DraftResult<String> {
        use crate::pack::lifecycle::{PackLifecycle, PackLifecycleRecord, PackTransitionRequest};
        use crate::pack::{PackLockfile, PackManifest, PackRevision, PackStore};
        let PackSyncSpec {
            kind,
            intent,
            lifecycle,
            metadata,
        } = spec;
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let store = PackStore::new(paths.clone());
        let workspace_hash = crate::workspace::source_view::workspace_hash(&ws.root)?;
        let changes_bytes = patch.map(to_pretty).transpose()?;
        if let Some(p) = patch {
            let mut paths_to_check = Vec::new();
            for file in &p.files {
                paths_to_check.push(&file.path);
                if let Some(old_path) = &file.old_path {
                    paths_to_check.push(old_path);
                }
            }
            let violations = crate::workspace::protected::violations(&ws.root, paths_to_check)?;
            if let Some(v) = violations.first() {
                return Err(DraftError::new(
                    DraftErrorKind::ProtectedFileAccess,
                    format!("protected file cannot be packed: {}", v.path),
                )
                .with_context(format!("matched protected pattern '{}'", v.pattern))
                .with_suggestion("remove the protected file from the pack scope"));
            }
        }

        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        let manifest = if store.exists(pack.id.as_str()) {
            store.read_manifest(pack.id.as_str())?
        } else {
            let mut manifest = PackManifest {
                schema_version: current_version(ContractId::PackManifest),
                pack_id: pack.id.to_string(),
                manifest_digest: String::new(),
                name: pack.name.clone().unwrap_or_else(|| pack.id.to_string()),
                description: String::new(),
                intent,
                provenance: serde_json::json!({"kind": "local_workspace"}),
                author_id: ledger.actor_id().to_string(),
                candidate_id: None,
                declared_dependencies: Vec::new(),
                created_at: now().to_rfc3339(),
            };
            manifest.refresh_manifest_digest();
            store.write_manifest(&manifest)?;
            manifest
        };
        manifest.ensure_supported()?;

        let existing_revisions = store.revisions(pack.id.as_str())?;
        let current = if existing_revisions.is_empty() {
            None
        } else {
            Some(store.current_revision_in(crate::pack::PackLocation::Store, pack.id.as_str())?)
        };
        let diff_digest = changes_bytes
            .as_ref()
            .map(|bytes| sha256_hex(bytes))
            .or_else(|| {
                current
                    .as_ref()
                    .map(|revision| revision.diff_digest.clone())
            })
            .unwrap_or_else(|| sha256_hex(b""));
        let base_digest = hash_json(&load_snapshot(ws, &pack.base_snapshot_id)?)?;
        let content_digest = hash_json(&load_snapshot(ws, &pack.result_snapshot_id)?)?;
        let revision_changed = current.as_ref().is_none_or(|revision| {
            revision.diff_digest != diff_digest
                || revision.content_digest != content_digest
                || revision.target_digest != workspace_hash
        });
        let revision = if revision_changed {
            let mut revision = PackRevision {
                schema_version: current_version(ContractId::PackRevision),
                pack_id: manifest.pack_id.clone(),
                manifest_digest: manifest.manifest_digest.clone(),
                revision_id: format!("rev_{}", uuid::Uuid::new_v4().simple()),
                revision_number: existing_revisions.len() as u64 + 1,
                revision_digest: String::new(),
                base_digest,
                content_digest,
                diff_digest,
                target_digest: workspace_hash.clone(),
                resolved_dependency_digests: manifest.declared_dependencies.clone(),
                created_at: now().to_rfc3339(),
            };
            revision.refresh_revision_digest();
            store.write_revision(&revision)?;
            if let Some(bytes) = &changes_bytes {
                write_atomic(&paths.pack_changes(pack.id.as_str()), bytes)?;
                write_atomic(
                    &paths
                        .pack_dir(pack.id.as_str())
                        .join("revisions")
                        .join(format!("{}.patch", revision.revision_id)),
                    bytes,
                )?;
            }
            revision
        } else {
            current.expect("a non-changing revision must have a current revision")
        };

        let outcome = ledger.record(
            kind,
            Some(pack.id.to_string()),
            None,
            workspace_hash.clone(),
            metadata,
        )?;
        let operation_id = crate::support::common::OperationId::new(&outcome.event.event_id);
        let mut lifecycle_record = if revision_changed {
            PackLifecycleRecord {
                schema_version: current_version(ContractId::PackLifecycle),
                pack_id: pack.id.to_string(),
                revision_id: revision.revision_id.clone(),
                revision_digest: revision.revision_digest.clone(),
                lifecycle: PackLifecycle::Draft,
                updated_at: now(),
                last_operation_id: operation_id.clone(),
            }
        } else {
            store.read_lifecycle_in(crate::pack::PackLocation::Store, pack.id.as_str())?
        };
        if lifecycle_record.lifecycle != lifecycle {
            if matches!(lifecycle, PackLifecycle::Approved | PackLifecycle::Rejected)
                && lifecycle_record.lifecycle == PackLifecycle::Verified
            {
                lifecycle_record.transition(PackTransitionRequest {
                    operation_id: operation_id.clone(),
                    expected_revision_id: revision.revision_id.clone(),
                    expected_revision_digest: revision.revision_digest.clone(),
                    target: PackLifecycle::Reviewing,
                })?;
            }
            lifecycle_record.transition(PackTransitionRequest {
                operation_id,
                expected_revision_id: revision.revision_id.clone(),
                expected_revision_digest: revision.revision_digest.clone(),
                target: lifecycle,
            })?;
        }
        store.write_lifecycle_in(crate::pack::PackLocation::Store, &lifecycle_record)?;

        if let Some(p) = patch {
            let mut file_hashes = std::collections::BTreeMap::new();
            for f in &p.files {
                if matches!(f.change_kind, FileChangeKind::Deleted) {
                    continue;
                }
                let fp = ws.root.join(f.path.as_str());
                let bytes = std::fs::read(&fp).map_err(|error| {
                    DraftError::storage(format!(
                        "cannot read changed file {} while locking pack {}: {error}",
                        fp.display(),
                        pack.id
                    ))
                })?;
                file_hashes.insert(f.path.to_string(), sha256_hex(&bytes));
            }
            let lock = PackLockfile {
                schema_version: current_version(ContractId::PackLock),
                pack_id: pack.id.to_string(),
                workspace_hash,
                file_hashes,
                policy_version: crate::DRAFT_VERSION.to_string(),
                risk_engine_version: crate::DRAFT_VERSION.to_string(),
                verification_commands: Vec::new(),
                lsif_version: crate::DRAFT_VERSION.to_string(),
                test_selector_version: crate::DRAFT_VERSION.to_string(),
                fuzz_selector_version: crate::DRAFT_VERSION.to_string(),
                dependency_pack_hashes: Vec::new(),
                receipt_digests: vec![sha256_hex(outcome.receipt.receipt_id.as_bytes())],
            };
            store.write_lockfile(&lock)?;
        }
        Ok(outcome.receipt.receipt_id)
    }

    /// Submit an imported pack: enforce the import gates, apply the embedded
    /// content to the workspace (fail closed, nothing written on any
    /// conflict), and promote the pack out of quarantine.
    ///
    /// Submit hooks do not run for import submissions — there is no rendered submit
    /// message/diff context for an imported pack.
    fn submit_imported_pack(
        &self,
        ws: &Workspace,
        store: &crate::pack::PackStore,
        loc: crate::pack::PackLocation,
        manifest: crate::pack::PackManifest,
    ) -> DraftResult<SubmitRecord> {
        use crate::pack::lifecycle::{PackLifecycle, PackTransitionRequest};
        use crate::pack::QuarantineState;
        let started = now();
        let pack_id = manifest.pack_id.clone();
        let dir = store.dir_for(loc, &pack_id);
        let quarantine = store.read_quarantine(&pack_id)?;
        let mut lifecycle = store.read_lifecycle_in(loc, &pack_id)?;
        let revision = store.current_revision_in(loc, &pack_id)?;

        // State gate with actionable, state-specific errors.
        match quarantine.trust_evaluation {
            QuarantineState::Approved => {}
            QuarantineState::Quarantined => {
                return Err(DraftError::new(
                    DraftErrorKind::ReviewRequired,
                    "imported packs must be locally verified and approved before submit",
                )
                .with_suggestion("run `draft verify <pck_id>`, then approve it"));
            }
            QuarantineState::Verified => {
                return Err(DraftError::new(
                    DraftErrorKind::ReviewRequired,
                    "imported packs must be approved before submit",
                ));
            }
            QuarantineState::Rejected => {
                return Err(DraftError::invalid_config(
                    "a rejected import cannot be submitted",
                ));
            }
            QuarantineState::Promoted => {
                return Err(DraftError::invalid_config(
                    "this imported pack is already submitted",
                ));
            }
        }
        let verification: crate::review::verification::VerifyEvidence =
            crate::contracts::read_persisted(&dir.join("verify.json"))?;
        verification.validate_binding(&revision)?;
        if lifecycle.lifecycle != PackLifecycle::Approved {
            return Err(DraftError::new(
                DraftErrorKind::ReviewRequired,
                "the current immutable revision is not approved for submit",
            ));
        }

        // Policy gates.
        let policy = effective_policy(ws)?;
        if policy.require_local_verify_for_imports && !verification.passed() {
            return Err(DraftError::new(
                DraftErrorKind::VerificationFailed,
                "imported packs must be locally re-verified before submit",
            )
            .with_suggestion("run `draft verify <pck_id>` first"));
        }
        let risk_path = dir.join("risk.json");
        let mut risk_level = "unknown".to_string();
        if risk_path.exists() {
            let risk: crate::review::risk::RiskReport =
                crate::contracts::read_persisted(&risk_path)?;
            risk.validate_binding(&revision)?;
            risk_level = risk.risk_level.as_str().to_string();
            if policy.block_on_critical_risk
                && risk.risk_level == crate::review::risk::RiskLevel::Critical
            {
                return Err(DraftError::new(
                    DraftErrorKind::RiskPolicyBlocked,
                    "unresolved critical risk blocks submit",
                ));
            }
        } else if policy.block_on_critical_risk {
            return Err(DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                "no local risk report exists for this imported pack",
            )
            .with_suggestion("run `draft verify <pck_id>` before submit"));
        }
        if policy.require_reverify_on_workspace_change {
            let current = crate::workspace::source_view::workspace_hash(&ws.root)?;
            if verification
                .verification_key
                .as_ref()
                .map(|key| key.workspace_hash.as_str())
                != Some(current.as_str())
            {
                return Err(DraftError::new(
                    DraftErrorKind::VerificationFailed,
                    "workspace content changed after the import was verified",
                )
                .with_suggestion("run `draft verify <pck_id>` again before submit"));
            }
        }
        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        if !ledger.verify_all()?.all_ok {
            return Err(DraftError::new(
                DraftErrorKind::OperationLogCorrupt,
                "canonical event, receipt, or transparency ledger failed verification",
            )
            .with_suggestion("run `draft receipt verify --all` or `draft doctor`"));
        }

        // Integrity + application plan (validate everything before writing).
        let changes_bytes = fs::read(dir.join("changes.patch"))?;
        if sha256_hex(&changes_bytes) != revision.diff_digest {
            return Err(DraftError::new(
                DraftErrorKind::VerificationFailed,
                "imported changes.patch does not match the manifest changes_hash (tampering?)",
            ));
        }
        let patch: PatchSet = serde_json::from_slice(&changes_bytes).map_err(|e| {
            DraftError::new(
                DraftErrorKind::VerificationFailed,
                format!("imported changes.patch is corrupt: {e}"),
            )
        })?;
        let plan = plan_import_apply(ws, &dir, &patch)?;

        // The apply is rollback-safe: checkpoint the workspace first.
        self.checkpoint(&ws.root, &format!("pre-import-submit {pack_id}"))?;

        ws.events()?.append(
            "submit.started",
            Some(pack_id.clone()),
            serde_json::json!({ "imported": true }),
        )?;
        for (dest, bytes) in &plan.writes {
            if let Some(parent) = dest.parent() {
                ensure_dir(parent)?;
            }
            write_atomic(dest, bytes)?;
        }
        for dest in &plan.deletes {
            if dest.is_file() {
                fs::remove_file(dest)?;
            }
        }

        // Project-state verification (SRS-FR-083–086), symmetric with the
        // local submit path: the applied state must verify before promotion,
        // receipts, or stable_head advancement.
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let applied_paths: Vec<String> = {
            let mut rels = Vec::new();
            for (p, _) in &plan.writes {
                if let Ok(rel) = p.strip_prefix(&ws.root) {
                    rels.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
            for p in &plan.deletes {
                if let Ok(rel) = p.strip_prefix(&ws.root) {
                    rels.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
            rels.sort();
            rels.dedup();
            rels
        };
        ledger.record(
            crate::trust::event::EventKind::ProjectStateVerificationStarted,
            Some(pack_id.clone()),
            None,
            crate::workspace::source_view::workspace_hash_cached(
                &ws.root,
                &paths.workspace_hash_cache(),
            )?,
            serde_json::json!({ "imported": true }),
        )?;
        let ps_report = crate::review::verification::verify_project_state(
            &ws.root,
            &paths,
            ws.workspace_id.as_str(),
            &applied_paths,
            true,
        )?;
        if !ps_report.passed {
            let failed_names = ps_report.failed_checks().join(", ");
            ledger.record(
                crate::trust::event::EventKind::ProjectStateVerificationFailed,
                Some(pack_id.clone()),
                None,
                ps_report.workspace_hash.clone(),
                serde_json::json!({
                    "imported": true,
                    "checks": ps_report.checks,
                    "failed": ps_report.failed_checks(),
                }),
            )?;
            return Err(DraftError::new(
                DraftErrorKind::VerificationFailed,
                format!(
                    "project-state verification failed after applying the imported pack: {failed_names}"
                ),
            )
            .with_suggestion(
                "the workspace was checkpointed before apply; run `draft rollback <chk_id>` to restore it",
            ));
        }

        // Advance the lifecycle of this exact immutable revision, then promote
        // the separately approved quarantine record.
        let wsh_after = crate::workspace::source_view::workspace_hash(&ws.root)?;
        let submitted = ledger.record(
            crate::trust::event::EventKind::PackSubmitted,
            Some(pack_id.clone()),
            None,
            wsh_after,
            serde_json::json!({
                "imported": true,
                "applied": true,
                "files_written": plan.writes.len(),
                "files_deleted": plan.deletes.len(),
            }),
        )?;
        lifecycle.transition(PackTransitionRequest {
            operation_id: crate::support::common::OperationId::new(&submitted.event.event_id),
            expected_revision_id: revision.revision_id.clone(),
            expected_revision_digest: revision.revision_digest.clone(),
            target: PackLifecycle::Submitted,
        })?;
        store.write_lifecycle_in(loc, &lifecycle)?;
        if loc == crate::pack::PackLocation::Quarantine {
            store.promote_from_quarantine(&pack_id)?;
        }

        // Return the same submit operation record to every caller.
        let object_store = ObjectStore::new(ws.layout.clone());
        let mut receipt = SubmitRecord {
            schema_version: current_version(ContractId::SubmitRecord),
            id: ReceiptId::generate(),
            pack_id: PackId::new(pack_id.clone()),
            actor_id: resolve_actor(&ws.layout.draft_dir)?.id,
            native_submit_status: NativeSubmitStatus::Submitted,
            hook_status: HookStatus::NotConfigured,
            overall_status: SubmitOverallStatus::Submitted,
            message_ref: object_store.put_bytes(manifest.name.as_bytes())?,
            hook_results: Vec::new(),
            hook_receipt_refs: Vec::new(),
            object_refs: Vec::new(),
            event_refs: Vec::new(),
            risk_level,
            risk_receipt_id: None,
            started_at: started,
            ended_at: now(),
            record_digest: String::new(),
            failure_reason: None,
        };
        receipt.record_digest = hash_json(&receipt)?;
        write_submit_record(ws, &receipt)?;
        ws.events()?.append(
            "submit.completed",
            Some(pack_id.clone()),
            serde_json::to_value(&receipt).expect("Draft-owned records must serialize"),
        )?;
        let stable_store = crate::workspace::stable::StableHeadStore::new(paths.clone());
        let previous_head = if stable_store.exists() {
            Some(stable_store.read()?)
        } else {
            None
        };
        let submit_mode = ResolvedConfig::load(ws)?.pack_disposal;
        let project_verified = ledger.record(
            crate::trust::event::EventKind::ProjectStateVerified,
            Some(pack_id.clone()),
            None,
            ps_report.workspace_hash.clone(),
            serde_json::json!({
                "imported": true,
                "submit_receipt": receipt.id.to_string(),
                "submit_mode": submit_mode.as_str(),
                "checks": ps_report.checks,
            }),
        )?;
        if submit_mode == crate::workspace::stable::SubmitMode::MergeAndDispose {
            let stable_head = stable_store.advance(
                &ws.root,
                project_verified.receipt.receipt_id.clone(),
                previous_head,
                Some(hash_json(&serde_json::json!({
                    "manifest": manifest,
                    "patch": patch,
                    "submit_receipt": receipt.id.to_string()
                }))?),
                Some(crate::workspace::stable::PackSummary {
                    pack_id: pack_id.clone(),
                    name: Some(manifest.name.clone()),
                    affected_paths: {
                        let mut paths = Vec::new();
                        for (p, _) in &plan.writes {
                            if let Ok(rel) = p.strip_prefix(&ws.root) {
                                paths.push(rel.to_string_lossy().replace('\\', "/"));
                            }
                        }
                        for p in &plan.deletes {
                            if let Ok(rel) = p.strip_prefix(&ws.root) {
                                paths.push(rel.to_string_lossy().replace('\\', "/"));
                            }
                        }
                        paths.sort();
                        paths.dedup();
                        paths
                    },
                }),
                submit_mode,
            )?;
            ledger.record(
                crate::trust::event::EventKind::StableHeadAdvanced,
                Some(stable_head.id.clone()),
                None,
                stable_head.workspace_hash.clone(),
                serde_json::json!({
                    "stable_head": stable_head.id,
                    "project_state_receipt": project_verified.receipt.receipt_id,
                    "submit_receipt": receipt.id.to_string()
                }),
            )?;
        }
        ledger.record(
            crate::trust::event::EventKind::SubmitFinalized,
            Some(pack_id.clone()),
            None,
            crate::workspace::source_view::workspace_hash(&ws.root)?,
            serde_json::json!({ "submit_receipt": receipt.id.to_string(), "imported": true }),
        )?;
        let removed = dispose_pack_metadata(ws, &paths, &pack_id)?;
        ledger.record(
            crate::trust::event::EventKind::PackDisposed,
            Some(pack_id),
            None,
            crate::workspace::source_view::workspace_hash(&ws.root)?,
            serde_json::json!({ "removed_entries": removed, "imported": true }),
        )?;
        Ok(receipt)
    }

    pub fn submit_selected(
        &self,
        cwd: &Path,
        pack_id: Option<&str>,
        vars: BTreeMap<String, String>,
    ) -> DraftResult<SubmitRecord> {
        let pack_id = self.resolve_pack_arg(cwd, pack_id)?;
        self.submit(cwd, &pack_id, vars)
    }

    pub fn submit_readiness_selected(
        &self,
        cwd: &Path,
        pack_id: Option<&str>,
    ) -> DraftResult<SubmitReadinessReport> {
        let pack_id = self.resolve_pack_arg(cwd, pack_id)?;
        let ws = self.open(cwd)?;
        let pack = load_pack(&ws, &pack_id)?;
        let patch = load_patch(&ws, &pack)?;
        let policy = effective_policy(&ws)?;
        submit_readiness(&ws, &pack, &patch, &policy)
    }

    pub fn rollback_plan(&self, cwd: &Path, reference: &str) -> DraftResult<RollbackPlan> {
        let ws = self.open(cwd)?;
        let snapshot = resolve_snapshot_reference(&ws, reference)?;
        let current =
            Snapshotter::new(&ws)?.create_snapshot(resolve_actor(&ws.layout.draft_dir)?)?;
        let patch = diff_snapshot_values(&snapshot, &current)?;
        Ok(RollbackPlan {
            schema_version: current_version(ContractId::RollbackPlan),
            id: RollbackPlanId::generate(),
            rollback_snapshot_id: snapshot.id,
            affected_files: patch
                .files
                .into_iter()
                .map(|f| f.path)
                .filter(|p| !is_draft_path(p.as_str()))
                .collect(),
            destructive: true,
            warnings: vec!["rollback will overwrite affected workspace files".to_string()],
        })
    }

    pub fn rollback(&self, cwd: &Path, reference: &str, yes: bool) -> DraftResult<RollbackRecord> {
        let ws = self.open(cwd)?;
        let plan = self.rollback_plan(cwd, reference)?;
        if plan.destructive && !yes {
            return Err(DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                "rollback is destructive and requires explicit CLI invocation",
            ));
        }
        ws.events()?.append(
            "rollback.started",
            Some(plan.id.to_string()),
            serde_json::to_value(&plan).expect("Draft-owned records must serialize"),
        )?;
        let snap = load_snapshot(&ws, &plan.rollback_snapshot_id)?;
        restore_snapshot(&ws, &snap)?;
        let mut receipt = RollbackRecord {
            schema_version: current_version(ContractId::RollbackRecord),
            id: ReceiptId::generate(),
            rollback_plan_id: plan.id.clone(),
            actor_id: resolve_actor(&ws.layout.draft_dir)?.id,
            status: "completed".to_string(),
            started_at: now(),
            ended_at: now(),
            record_digest: String::new(),
        };
        write_rollback_record(&ws, &mut receipt)?;
        ws.events()?.append(
            "rollback.completed",
            Some(receipt.id.to_string()),
            serde_json::to_value(&receipt).expect("Draft-owned records must serialize"),
        )?;
        let workspace_hash = crate::workspace::source_view::workspace_hash(&ws.root)?;
        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        ledger.record_with_receipt_id(
            crate::trust::event::EventKind::RollbackPerformed,
            Some(reference.to_string()),
            None,
            workspace_hash,
            serde_json::json!({
                "rollback_record_id": receipt.id.to_string(),
                "snapshot": snap.id.to_string(),
                "rollback_plan": plan,
            }),
            receipt.id.to_string(),
        )?;
        Ok(receipt)
    }

    /// `draft rollback <target> --dry-run`: resolve the target and report what
    /// would change and which safety checks pass, without mutating anything.
    pub fn rollback_dry_run(&self, cwd: &Path, reference: &str) -> DraftResult<DryRunReport> {
        let ws = self.open(cwd)?;
        let mut checks = Vec::new();
        // Target id prefix must be chk_/pck_/rcp_ (validated by the resolver).
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
            .map(|p| {
                p.affected_files
                    .iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
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
                "workspace restored to target".to_string()
            } else {
                "blocked".to_string()
            },
            affected_files: affected,
            checks,
        })
    }

    /// `draft submit --dry-run`: report whether the pack would submit and why,
    /// without writing anything.
    pub fn submit_dry_run(&self, cwd: &Path, pack_id: Option<&str>) -> DraftResult<DryRunReport> {
        let pack_id = self.resolve_pack_arg(cwd, pack_id)?;
        let ws = self.open(cwd)?;
        {
            let store = crate::pack::PackStore::new(
                crate::workspace::layout::DraftLayout::for_root(&ws.root),
            );
            if let Some(loc) = store.locate(&pack_id) {
                let manifest = store.read_manifest_in(loc, &pack_id)?;
                if store.quarantine_record(&pack_id)?.is_some() {
                    return self.import_submit_dry_run(&ws, &store, loc, manifest);
                }
            }
        }
        let pack = load_pack(&ws, &pack_id)?;
        let patch = load_patch(&ws, &pack)?;
        let policy = effective_policy(&ws)?;
        let readiness = submit_readiness(&ws, &pack, &patch, &policy)?;
        let mut checks = Vec::new();
        let draft_touch = patch.files.iter().any(|f| is_draft_path(f.path.as_str()));
        checks.push(bool_check(
            "draft-exclusion",
            !draft_touch,
            ".draft/ not in candidate",
            ".draft/ present in submit candidate",
        ));
        checks.push(bool_check(
            "verified",
            readiness.verification_receipt_id.is_some(),
            "verification receipt present",
            "not verified",
        ));
        checks.push(bool_check(
            "approved",
            readiness.approval_ref.is_some(),
            "approval present",
            "not approved",
        ));
        checks.push(match self.verify_events(&ws.root) {
            Ok(_) => DoctorCheck::ok("event-chain", "intact"),
            Err(e) => DoctorCheck::fail("event-chain", e.message),
        });
        let would = checks.iter().all(|c| c.ok);
        Ok(DryRunReport {
            action: "submit".to_string(),
            target: pack_id,
            would_proceed: would,
            resulting_state: if would {
                "submitted".to_string()
            } else {
                "blocked".to_string()
            },
            affected_files: patch.files.iter().map(|f| f.path.to_string()).collect(),
            checks,
        })
    }

    /// `draft submit --dry-run` for an imported pack: report the import gates
    /// and whether the embedded content would apply cleanly.
    fn import_submit_dry_run(
        &self,
        ws: &Workspace,
        store: &crate::pack::PackStore,
        loc: crate::pack::PackLocation,
        manifest: crate::pack::PackManifest,
    ) -> DraftResult<DryRunReport> {
        let dir = store.dir_for(loc, &manifest.pack_id);
        let lifecycle = store.read_lifecycle_in(loc, &manifest.pack_id)?;
        let revision = store.current_revision_in(loc, &manifest.pack_id)?;
        let quarantine = store.read_quarantine(&manifest.pack_id)?;
        let verification: crate::review::verification::VerifyEvidence =
            crate::contracts::read_persisted(&dir.join("verify.json"))?;
        verification.validate_binding(&revision)?;
        let mut checks = Vec::new();
        checks.push(bool_check(
            "locally-verified",
            verification.passed(),
            "local verification evidence present",
            "imported pack is not locally verified",
        ));
        checks.push(bool_check(
            "approved",
            quarantine.trust_evaluation == crate::pack::QuarantineState::Approved
                && lifecycle.lifecycle == crate::pack::lifecycle::PackLifecycle::Approved,
            "import approved",
            "imported pack is not approved",
        ));
        let workspace_unchanged = crate::workspace::source_view::workspace_hash(&ws.root)
            .map(|hash| {
                verification
                    .verification_key
                    .as_ref()
                    .map(|key| key.workspace_hash.as_str())
                    == Some(hash.as_str())
            })
            .unwrap_or(false);
        checks.push(bool_check(
            "workspace-unchanged",
            workspace_unchanged,
            "workspace matches verification state",
            "workspace changed since local verification",
        ));
        let (applies, affected) = match fs::read(dir.join("changes.patch"))
            .map_err(DraftError::from)
            .and_then(|b| {
                if sha256_hex(&b) != revision.diff_digest {
                    return Err(DraftError::new(
                        DraftErrorKind::VerificationFailed,
                        "changes hash mismatch",
                    ));
                }
                serde_json::from_slice::<PatchSet>(&b)
                    .map_err(|e| DraftError::invalid_config(e.to_string()))
            })
            .and_then(|patch| plan_import_apply(ws, &dir, &patch).map(|plan| (patch, plan)))
        {
            Ok((patch, _plan)) => (
                DoctorCheck::ok("applies-cleanly", "embedded content applies cleanly"),
                patch.files.iter().map(|f| f.path.to_string()).collect(),
            ),
            Err(e) => (DoctorCheck::fail("applies-cleanly", e.message), Vec::new()),
        };
        checks.push(applies);
        let would = checks.iter().all(|c| c.ok);
        Ok(DryRunReport {
            action: "submit".to_string(),
            target: manifest.pack_id,
            would_proceed: would,
            resulting_state: if would {
                "import applied and submitted".to_string()
            } else {
                "blocked".to_string()
            },
            affected_files: affected,
            checks,
        })
    }

    /// Resolve a pack reference (pck_id or unique name) to a canonical pack id.
    fn resolve_canonical_pack_ref(&self, ws: &Workspace, reference: &str) -> DraftResult<String> {
        if reference.starts_with("pck_") {
            return Ok(reference.to_string());
        }
        let store =
            crate::pack::PackStore::new(crate::workspace::layout::DraftLayout::for_root(&ws.root));
        if let Some(m) = store.list()?.into_iter().find(|m| m.name == reference) {
            return Ok(m.pack_id);
        }
        // Quarantined imports are addressable by name too.
        if let Some(m) = store
            .list_quarantined()?
            .into_iter()
            .find(|m| m.name == reference)
        {
            return Ok(m.pack_id);
        }
        Err(DraftError::not_found(format!(
            "canonical pack '{reference}' was not found"
        )))
    }

    /// `draft pack --export <pck_id|name> [--output <path>]`.
    pub fn pack_export(
        &self,
        cwd: &Path,
        reference: &str,
        output: Option<&Path>,
    ) -> DraftResult<PackExportReport> {
        use crate::pack::archive::{
            archive_content_digest, DraftpackHeader, Provenance, DRAFTPACK_FORMAT,
        };
        let ws = self.open(cwd)?;
        let pack_id = self.resolve_canonical_pack_ref(&ws, reference)?;
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let store = crate::pack::PackStore::new(paths.clone());
        let manifest = store.read_manifest(&pack_id)?;
        let revision = store.current_revision_in(crate::pack::PackLocation::Store, &pack_id)?;
        let lifecycle = store.read_lifecycle_in(crate::pack::PackLocation::Store, &pack_id)?;

        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        entries.push(("manifest.json".into(), to_pretty(&manifest)?));
        entries.push(("revision.json".into(), to_pretty(&revision)?));
        entries.push(("lifecycle.json".into(), to_pretty(&lifecycle)?));
        let lock = store.read_lockfile(&pack_id)?;
        entries.push(("pack.lock.json".into(), to_pretty(&lock)?));
        if paths.pack_changes(&pack_id).exists() {
            let changes_bytes = fs::read(paths.pack_changes(&pack_id))?;
            if sha256_hex(&changes_bytes) != revision.diff_digest {
                return Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    "changes.patch does not match the immutable revision",
                ));
            }
            // Embed the content-addressed objects referenced by the patch
            // (new file contents + hunk bodies) so the pack is portable: an
            // importing workspace can re-verify content and apply it on submit.
            let patch: PatchSet = crate::contracts::decode_persisted(&changes_bytes)?;
            let object_store = ObjectStore::new(ws.layout.clone());
            let mut refs = std::collections::BTreeSet::new();
            for f in &patch.files {
                if let Some(h) = &f.old_hash {
                    refs.insert(h.clone());
                }
                if let Some(h) = &f.new_hash {
                    refs.insert(h.clone());
                }
                for hunk in &f.hunks {
                    if !hunk.content_ref.is_empty() {
                        refs.insert(hunk.content_ref.clone());
                    }
                }
            }
            for object_ref in refs {
                let hex = object_ref.strip_prefix("b3:").ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::CorruptData,
                        format!("unsupported object ref '{object_ref}'"),
                    )
                })?;
                let source_dir = store.dir_for(crate::pack::PackLocation::Store, &pack_id);
                let embedded = source_dir.join("objects").join(hex);
                let bytes = if embedded.exists() {
                    read_imported_object(&source_dir, &object_ref)?
                } else {
                    object_store.get_bytes(&object_ref)?
                };
                entries.push((format!("objects/{hex}"), bytes));
            }
            entries.push(("changes.patch".into(), changes_bytes));
        } else if revision.diff_digest != sha256_hex(b"") {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "pack revision references missing changes.patch",
            ));
        }
        for (name, p) in [
            ("risk.json", paths.pack_risk(&pack_id)),
            ("verify.json", paths.pack_verify(&pack_id)),
            ("lsif.json", paths.pack_lsif(&pack_id)),
        ] {
            if p.exists() {
                entries.push((name.to_string(), fs::read(&p)?));
            }
        }
        // Signed receipts referencing this pack are preserved as provenance.
        let rstore = crate::trust::receipt::ReceiptStore::new(paths.clone());
        let mut external_receipt_ids = Vec::new();
        for r in rstore.list()? {
            if r.subject_id.as_deref() == Some(pack_id.as_str()) {
                entries.push((format!("receipts/{}.json", r.receipt_id), to_pretty(&r)?));
                external_receipt_ids.push(r.receipt_id.clone());
            }
        }
        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        let provenance = Provenance {
            schema_version: current_version(ContractId::DraftpackProvenance),
            origin: "local".to_string(),
            exported_by_actor: ledger.actor_id().to_string(),
            source_workspace_hash: revision.target_digest.clone(),
            external_receipt_ids,
        };
        entries.push(("provenance.json".into(), to_pretty(&provenance)?));
        let header = DraftpackHeader {
            schema_version: current_version(ContractId::Draftpack),
            format: DRAFTPACK_FORMAT.to_string(),
            draft_version: crate::DRAFT_VERSION.to_string(),
            artifact_digest: archive_content_digest(&entries),
            pack_id: manifest.pack_id.clone(),
            name: manifest.name.clone(),
            exported_at: now().to_rfc3339(),
        };
        entries.push(("draftpack.json".into(), to_pretty(&header)?));

        let out = match output {
            Some(p) => p.to_path_buf(),
            None => {
                ensure_dir(&paths.exports_dir())?;
                paths
                    .exports_dir()
                    .join(format!("{}.draftpack", manifest.name))
            }
        };
        crate::pack::archive::write_archive(&out, &entries)?;
        let wsh = crate::workspace::source_view::workspace_hash(&ws.root)?;
        ledger.record(
            crate::trust::event::EventKind::PackExported,
            Some(pack_id.clone()),
            None,
            wsh,
            serde_json::json!({ "output": out.display().to_string() }),
        )?;
        Ok(PackExportReport {
            pack_id,
            name: manifest.name,
            output: out.display().to_string(),
            bytes: fs::metadata(&out)?.len(),
        })
    }

    /// `draft pack --import <path> [--name <unique>] [--dry-run]`.
    pub fn pack_import(
        &self,
        cwd: &Path,
        path: &Path,
        new_name: Option<&str>,
        dry_run: bool,
    ) -> DraftResult<PackImportReport> {
        use crate::pack::archive::{DraftpackHeader, DRAFTPACK_FORMAT};
        use crate::pack::lifecycle::{PackLifecycle, PackLifecycleRecord};
        use crate::pack::{
            PackManifest, PackQuarantineRecord, PackRevision, PackStore, QuarantineState,
        };
        let ws = self.open(cwd)?;
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let store = PackStore::new(paths.clone());

        // Full security validation happens inside read_archive (fail closed).
        let archive = crate::pack::archive::read_archive(path)?;
        let header: DraftpackHeader = crate::contracts::decode_wire(
            archive
                .get("draftpack.json")
                .ok_or_else(|| DraftError::invalid_config("archive is missing draftpack.json"))?,
        )?;
        if header.format != DRAFTPACK_FORMAT {
            return Err(DraftError::invalid_config(format!(
                "unsupported .draftpack format '{}'",
                header.format
            )));
        }
        let source_manifest: PackManifest = crate::contracts::decode_wire(
            archive
                .get("manifest.json")
                .ok_or_else(|| DraftError::invalid_config("archive is missing manifest.json"))?,
        )?;
        source_manifest.ensure_supported()?;
        let source_revision: PackRevision = crate::contracts::decode_wire(
            archive
                .get("revision.json")
                .ok_or_else(|| DraftError::invalid_config("archive is missing revision.json"))?,
        )?;
        source_revision.validate(&source_manifest)?;
        if let Some(bytes) = archive.get("risk.json") {
            let risk: crate::review::risk::RiskReport = crate::contracts::decode_wire(bytes)?;
            risk.validate_binding(&source_revision)
                .map_err(|error| wire_binding_error("risk evidence", error))?;
        }
        if let Some(bytes) = archive.get("verify.json") {
            let verification: crate::review::verification::VerifyEvidence =
                crate::contracts::decode_wire(bytes)?;
            verification
                .validate_binding(&source_revision)
                .map_err(|error| wire_binding_error("verification evidence", error))?;
        }
        // Content integrity: changes.patch is bound to the immutable revision.
        if let Some(changes) = archive.get("changes.patch") {
            let recomputed = sha256_hex(changes);
            if recomputed != source_revision.diff_digest {
                return Err(DraftError::invalid_config(
                    "changes hash mismatch: revision.diff_digest does not match changes.patch",
                ));
            }
        } else if source_revision.diff_digest != sha256_hex(b"") {
            return Err(DraftError::invalid_config(
                "archive is missing changes.patch for its declared revision",
            ));
        }

        let target_name = new_name
            .map(|s| s.to_string())
            .unwrap_or_else(|| source_manifest.name.clone());
        // Uniqueness spans both saved packs and already-quarantined imports.
        let name_taken =
            store.name_taken(&target_name)? || quarantine_names(&paths)?.contains(&target_name);
        if name_taken {
            let hint = if new_name.is_some() {
                format!("name '{target_name}' already exists; choose another --name")
            } else {
                format!("duplicate pack name '{target_name}'; import with --name <unique>")
            };
            return Err(DraftError::invalid_config(hint));
        }

        let mut target_id = source_manifest.pack_id.clone();
        let mut remapped = false;
        if store.exists(&target_id) || paths.quarantine_dir().join(&target_id).exists() {
            target_id = format!("pck_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
            remapped = true;
        }
        // Embedded objects are content-addressed: a byte payload that does not
        // hash to its own entry name is tampering (fail closed).
        for (entry_name, bytes) in archive
            .entries
            .iter()
            .filter(|(k, _)| k.starts_with("objects/"))
        {
            let expected = entry_name.trim_start_matches("objects/");
            let actual = blake3_hex(bytes);
            if actual != expected {
                return Err(DraftError::invalid_config(format!(
                    "corrupt embedded object '{entry_name}': content hash mismatch"
                )));
            }
        }

        // External receipts are provenance only and never granted local trust,
        // but a corrupt or wrong-schema receipt still rejects the artifact
        // (fail closed on every embedded document).
        let mut external_receipts = 0usize;
        for (entry_name, bytes) in archive
            .entries
            .iter()
            .filter(|(k, _)| k.starts_with("receipts/"))
        {
            crate::contracts::decode_wire::<crate::trust::receipt::ReceiptRecord>(bytes)
                .map_err(|error| error.with_context(entry_name.clone()))?;
            external_receipts += 1;
        }

        if dry_run {
            return Ok(PackImportReport {
                pack_id: target_id,
                name: target_name,
                quarantined: true,
                remapped,
                external_receipts,
                applied: false,
            });
        }

        // Derive a new local immutable identity from the origin manifest. A
        // rename or id collision never mutates the origin contract in place.
        let mut manifest = PackManifest {
            schema_version: current_version(ContractId::PackManifest),
            pack_id: target_id.clone(),
            manifest_digest: String::new(),
            name: target_name.clone(),
            description: source_manifest.description.clone(),
            intent: source_manifest.intent,
            provenance: serde_json::json!({
                "kind": "draftpack_import",
                "source_pack_id": source_manifest.pack_id,
                "source_manifest_digest": source_manifest.manifest_digest,
                "artifact_digest": header.artifact_digest,
            }),
            author_id: source_manifest.author_id.clone(),
            candidate_id: source_manifest.candidate_id.clone(),
            declared_dependencies: source_manifest.declared_dependencies.clone(),
            created_at: now().to_rfc3339(),
        };
        manifest.refresh_manifest_digest();
        let mut revision = source_revision.clone();
        revision.pack_id = target_id.clone();
        revision.manifest_digest = manifest.manifest_digest.clone();
        revision.revision_id = format!("rev_{}", uuid::Uuid::new_v4().simple());
        revision.revision_number = 1;
        revision.created_at = now().to_rfc3339();
        revision.refresh_revision_digest();

        // Extract canonical content into quarantine. Origin decisions and
        // evidence remain provenance and cannot satisfy local gates.
        let qdir = paths.quarantine_dir().join(&target_id);
        ensure_dir(&qdir)?;
        for (name, bytes) in &archive.entries {
            if matches!(
                name.as_str(),
                "manifest.json" | "revision.json" | "lifecycle.json"
            ) {
                continue;
            }
            let local_name = if matches!(
                name.as_str(),
                "risk.json" | "verify.json" | "lsif.json" | "provenance.json" | "draftpack.json"
            ) || name.starts_with("receipts/")
            {
                format!("origin/{name}")
            } else {
                name.clone()
            };
            let dest = crate::support::pathguard::safe_join(&qdir, &local_name)
                .map_err(|v| DraftError::invalid_config(format!("unsafe entry {name}: {v}")))?;
            if let Some(parent) = dest.parent() {
                ensure_dir(parent)?;
            }
            write_atomic(&dest, bytes)?;
        }
        store.write_manifest_in(crate::pack::PackLocation::Quarantine, &manifest)?;
        store.write_revision_in(crate::pack::PackLocation::Quarantine, &revision)?;
        store.write_lifecycle_in(
            crate::pack::PackLocation::Quarantine,
            &PackLifecycleRecord {
                schema_version: current_version(ContractId::PackLifecycle),
                pack_id: target_id.clone(),
                revision_id: revision.revision_id.clone(),
                revision_digest: revision.revision_digest.clone(),
                lifecycle: PackLifecycle::Draft,
                updated_at: now(),
                last_operation_id: crate::support::common::OperationId::new("op_import"),
            },
        )?;
        store.write_quarantine(&PackQuarantineRecord {
            schema_version: current_version(ContractId::PackQuarantine),
            pack_id: target_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_digest: revision.revision_digest.clone(),
            storage_location: "quarantine".into(),
            source: path.display().to_string(),
            artifact_digest: header.artifact_digest.clone(),
            trust_evaluation: QuarantineState::Quarantined,
            quarantined_at: now().to_rfc3339(),
            promoted_at: None,
        })?;

        let wsh = crate::workspace::source_view::workspace_hash(&ws.root)?;
        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        ledger.record(
            crate::trust::event::EventKind::PackImported,
            Some(target_id.clone()),
            None,
            wsh,
            serde_json::json!({
                "source": path.display().to_string(),
                "remapped": remapped,
                "external_receipts": external_receipts,
            }),
        )?;
        Ok(PackImportReport {
            pack_id: target_id,
            name: target_name,
            quarantined: true,
            remapped,
            external_receipts,
            applied: true,
        })
    }

    /// `draft verify pck_<id> [--explain|--full|--fuzz]`: LSIF-backed risk +
    /// evidence-based test/fuzz selection. Writes lsif.json/risk.json/verify.json
    /// and records a signed PackVerified receipt.
    pub fn verify_pack(
        &self,
        cwd: &Path,
        pack_ref: &str,
        full: bool,
        fuzz: bool,
    ) -> DraftResult<VerifyReport> {
        use crate::pack::PackStore;
        use crate::review::lsif::{LsifIndex, LSIF_BACKEND};
        let ws = self.open(cwd)?;
        let pack_id = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let store = PackStore::new(paths.clone());
        let loc = store
            .locate(&pack_id)
            .unwrap_or(crate::pack::PackLocation::Store);
        let manifest = store.read_manifest_in(loc, &pack_id)?;
        let revision = store.current_revision_in(loc, &pack_id)?;
        let mut lifecycle = store.read_lifecycle_in(loc, &pack_id)?;
        if matches!(
            lifecycle.lifecycle,
            crate::pack::lifecycle::PackLifecycle::Rejected
                | crate::pack::lifecycle::PackLifecycle::Submitted
        ) {
            return Err(DraftError::invalid_config(
                "rejected or submitted revisions cannot be re-verified; reopen or create a successor",
            ));
        }

        // Policy may escalate verification scope for sensitive intents
        // (e.g. `security` requires the full suite and fuzzing).
        let policy = effective_policy(&ws)?;
        let intent_label = manifest.intent.as_str();
        let full = full || policy.intent_requires_full_verify(intent_label);
        let fuzz = fuzz || policy.intent_requires_fuzz(intent_label);

        // Imported packs are verified from their embedded, content-addressed
        // artifacts, never from origin evidence.
        if store.quarantine_record(&pack_id)?.is_some() {
            return self.verify_imported_pack(&ws, &paths, &store, loc, manifest, full, fuzz);
        }
        let pack = self.resolve_pack_ref(&ws, &pack_id)?;

        // Changed files (content) — never include `.draft/`.
        let patch = load_patch(&ws, &pack)?;
        if sha256_hex(&to_pretty(&patch)?) != revision.diff_digest {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "current pack diff does not match its immutable revision",
            ));
        }
        let mut changed = Vec::new();
        for file in patch
            .files
            .iter()
            .filter(|file| !is_draft_path(file.path.as_str()) && !file.binary)
        {
            let content = if matches!(file.change_kind, FileChangeKind::Deleted) {
                String::new()
            } else {
                fs::read_to_string(ws.root.join(file.path.as_str())).map_err(|error| {
                    DraftError::storage(format!(
                        "cannot read changed text file {}: {error}",
                        file.path
                    ))
                })?
            };
            changed.push((file.path.to_string(), content));
        }
        let changed_paths: Vec<String> = changed.iter().map(|(p, _)| p.clone()).collect();

        // LSIF impact.
        let lsif = LsifIndex::open(&paths)?;
        lsif.index_pack(&pack_id, &changed)?;
        let changed_symbols = lsif.symbols_touched_by_pack(&pack_id)?;
        let public_api = lsif.public_api_symbols_changed(&pack_id)?;
        let known: std::collections::BTreeSet<String> = changed_symbols.iter().cloned().collect();
        for (rel, content) in scan_test_files(&ws.root)? {
            lsif.record_refs(&rel, &content, &known)?;
        }
        let test_files = lsif.files_referencing_symbols(&changed_symbols)?;
        let fuzz_targets = scan_fuzz_targets(&ws.root)?;

        // Risk.
        let ledger_events =
            crate::trust::event::EventLog::workspace(paths.clone(), ws.workspace_id.to_string())
                .read_all()?;
        let all_manifests = store.list()?;
        let risk_inputs = crate::review::risk::RiskInputs {
            pack_id: pack_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_digest: revision.revision_digest.clone(),
            dependency_digests: revision.resolved_dependency_digests.clone(),
            intent: manifest.intent,
            files_touched: changed.len(),
            lines_changed: changed.iter().map(|(_, c)| c.lines().count()).sum(),
            high_risk_paths: crate::review::risk::high_risk_paths(&changed_paths),
            has_tests: !test_files.is_empty(),
            has_fuzz: fuzz && !fuzz_targets.is_empty(),
            public_api_changes: public_api.len(),
            imported: store.quarantine_record(&pack_id)?.is_some(),
            dependency_count: store.read_lockfile(&pack_id)?.dependency_pack_hashes.len(),
            semantic_impact: changed_symbols.len(),
            candidate_rollback_rate: manifest
                .candidate_id
                .as_deref()
                .map(|c| candidate_rollback_rate(&ledger_events, &all_manifests, c))
                .unwrap_or(0.0),
        };
        let risk = crate::review::risk::assess(&risk_inputs);

        // Selection evidence.
        let selection = crate::review::verification::SelectionInput {
            pack_id: pack_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_digest: revision.revision_digest.clone(),
            dependency_digests: revision.resolved_dependency_digests.clone(),
            changed_files: changed_paths.clone(),
            changed_symbols: changed_symbols.clone(),
            test_files: test_files.clone(),
            fuzz_targets: fuzz_targets.clone(),
            full,
            fuzz,
        };
        let mut evidence = crate::review::verification::plan(&selection);
        let commands = verification_commands(&ws, &evidence)?;
        crate::review::verification::execute(&mut evidence, &commands, &ws.root);

        // Deterministic verification cache key (SRS-FR-144): associates this
        // result with the exact workspace, config, toolchain, command set, and
        // environment that produced it.
        let wsh = crate::workspace::source_view::workspace_hash_cached(
            &ws.root,
            &paths.workspace_hash_cache(),
        )?;
        let verification_key = crate::review::verification::VerificationKey::compose(
            wsh.clone(),
            crate::workspace::config::config_hash(&paths)?,
            crate::review::verification::toolchain_hash(&ws.root),
            crate::review::verification::verification_command_hash(&commands),
            crate::review::verification::environment_hash(),
        );
        evidence.verification_key = Some(verification_key.clone());

        // Persist canonical evidence.
        write_json(&paths.pack_risk(&pack_id), &risk)?;
        write_json(&paths.pack_verify(&pack_id), &evidence)?;
        crate::review::index::VerificationCacheManifest::record(
            &paths,
            crate::review::index::VerificationCacheEntry {
                verification_key: verification_key.key.clone(),
                pack_id: pack_id.clone(),
                result_hash: evidence.result_hash.clone(),
                passed: evidence.passed(),
                recorded_at: now().to_rfc3339(),
            },
        )?;
        crate::review::index::AffectedPathIndex::upsert(&paths, &pack_id, changed_paths.clone())?;
        let lsif_summary = serde_json::json!({
            "backend": LSIF_BACKEND,
            "symbols_touched": changed_symbols,
            "public_api_changed": public_api,
            "tests_referencing": test_files,
            "semantic_impact": changed_symbols.len(),
        });
        write_json(&paths.pack_lsif(&pack_id), &lsif_summary)?;

        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        let verification_outcome = ledger.record(
            crate::trust::event::EventKind::PackVerified,
            Some(pack_id.clone()),
            None,
            wsh,
            serde_json::json!({
                "risk_level": risk.risk_level.as_str(),
                "result_hash": evidence.result_hash,
            }),
        )?;
        if lifecycle.lifecycle == crate::pack::lifecycle::PackLifecycle::Draft {
            lifecycle.transition(crate::pack::lifecycle::PackTransitionRequest {
                operation_id: crate::support::common::OperationId::new(
                    &verification_outcome.event.event_id,
                ),
                expected_revision_id: revision.revision_id.clone(),
                expected_revision_digest: revision.revision_digest.clone(),
                target: crate::pack::lifecycle::PackLifecycle::Verified,
            })?;
            store.write_lifecycle_in(loc, &lifecycle)?;
        }
        crate::review::workflow::WorkflowStore::for_root(&ws.root).write_evidence(
            &crate::review::workflow::EvidenceRecord {
                schema_version: current_version(ContractId::WorkflowEvidence),
                id: EvidenceId::generate(),
                task_id: None,
                pack_id: Some(pack_id.clone()),
                execution_id: None,
                kind: "verification".to_string(),
                state: if evidence.passed() {
                    crate::review::workflow::EvidenceState::Fresh
                } else {
                    crate::review::workflow::EvidenceState::Failed
                },
                result: serde_json::json!({
                    "risk_level": risk.risk_level.as_str(),
                    "risk_score": risk.risk_score,
                    "result_hash": evidence.result_hash,
                    "selected_tests": evidence.selected_tests.len(),
                    "selected_fuzz_targets": evidence.selected_fuzz_targets.len(),
                }),
                produced_at: now(),
                stale_reason: None,
                invalidated_by: vec![],
                receipt_id: Some(verification_outcome.receipt.receipt_id),
            },
        )?;

        Ok(VerifyReport {
            pack_id,
            risk_level: risk.risk_level.as_str().to_string(),
            risk_score: risk.risk_score,
            explanations: risk.explanations,
            required_actions: risk.required_actions,
            selected_tests: evidence.selected_tests,
            selected_fuzz_targets: evidence.selected_fuzz_targets,
            selection_reason: evidence.selection_reason,
            coverage_basis: evidence.coverage_basis,
            symbols_touched: changed_symbols.len(),
            public_api_changed: public_api.len(),
            result_hash: evidence.result_hash,
        })
    }

    /// Locally re-verify an imported pack from its quarantined artifacts.
    ///
    /// Fail closed on any integrity violation: the embedded `changes.patch`
    /// must match the revision's `diff_digest`, and every referenced content
    /// object must hash to its own name. Evidence (risk/verify/lsif) is then
    /// produced by the same pipeline as local packs, with file contents read
    /// from the embedded objects instead of the workspace.
    #[allow(clippy::too_many_arguments)]
    fn verify_imported_pack(
        &self,
        ws: &Workspace,
        paths: &crate::workspace::layout::DraftLayout,
        store: &crate::pack::PackStore,
        loc: crate::pack::PackLocation,
        manifest: crate::pack::PackManifest,
        full: bool,
        fuzz: bool,
    ) -> DraftResult<VerifyReport> {
        use crate::review::lsif::{LsifIndex, LSIF_BACKEND};
        let pack_id = manifest.pack_id.clone();
        let mut quarantine = store.read_quarantine(&pack_id)?;
        let revision = store.current_revision_in(loc, &pack_id)?;
        let mut lifecycle = store.read_lifecycle_in(loc, &pack_id)?;
        if !crate::pack::can_quarantine_transition(
            quarantine.trust_evaluation,
            crate::pack::QuarantineState::Verified,
        ) {
            return Err(DraftError::invalid_config(format!(
                "imported pack trust state '{:?}' cannot be verified",
                quarantine.trust_evaluation
            )));
        }
        if !matches!(
            lifecycle.lifecycle,
            crate::pack::lifecycle::PackLifecycle::Draft
                | crate::pack::lifecycle::PackLifecycle::Verified
        ) {
            return Err(DraftError::invalid_config(
                "reviewed, rejected, or submitted packs require an explicit reopen or successor before verification",
            ));
        }
        let dir = store.dir_for(loc, &pack_id);

        // Integrity gate: changes.patch must exist, match the manifest hash,
        // and parse.
        let changes_path = dir.join("changes.patch");
        if !changes_path.exists() {
            return Err(DraftError::new(
                DraftErrorKind::VerificationFailed,
                "imported pack has no changes.patch to verify",
            ));
        }
        let changes_bytes = fs::read(&changes_path)?;
        if sha256_hex(&changes_bytes) != revision.diff_digest {
            return Err(DraftError::new(
                DraftErrorKind::VerificationFailed,
                "imported changes.patch does not match the immutable revision (tampering?)",
            ));
        }
        let patch: PatchSet = serde_json::from_slice(&changes_bytes).map_err(|e| {
            DraftError::new(
                DraftErrorKind::VerificationFailed,
                format!("imported changes.patch is corrupt: {e}"),
            )
        })?;

        // Reconstruct changed-file contents from the embedded objects
        // (content-addressed; re-checked here against post-import tampering).
        let mut changed: Vec<(String, String)> = Vec::new();
        for f in &patch.files {
            if is_draft_path(f.path.as_str()) {
                continue;
            }
            let content = match &f.new_hash {
                Some(h) => String::from_utf8_lossy(&read_imported_object(&dir, h)?).into_owned(),
                None => String::new(),
            };
            changed.push((f.path.to_string(), content));
        }
        let changed_paths: Vec<String> = changed.iter().map(|(p, _)| p.clone()).collect();

        // Same evidence pipeline as local packs.
        let lsif = LsifIndex::open(paths)?;
        lsif.index_pack(&pack_id, &changed)?;
        let changed_symbols = lsif.symbols_touched_by_pack(&pack_id)?;
        let public_api = lsif.public_api_symbols_changed(&pack_id)?;
        let known: std::collections::BTreeSet<String> = changed_symbols.iter().cloned().collect();
        for (rel, content) in scan_test_files(&ws.root)? {
            lsif.record_refs(&rel, &content, &known)?;
        }
        let test_files = lsif.files_referencing_symbols(&changed_symbols)?;
        let fuzz_targets = scan_fuzz_targets(&ws.root)?;

        let dependency_count = revision.resolved_dependency_digests.len();
        let risk_inputs = crate::review::risk::RiskInputs {
            pack_id: pack_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_digest: revision.revision_digest.clone(),
            dependency_digests: revision.resolved_dependency_digests.clone(),
            intent: manifest.intent,
            files_touched: changed.len(),
            lines_changed: changed.iter().map(|(_, c)| c.lines().count()).sum(),
            high_risk_paths: crate::review::risk::high_risk_paths(&changed_paths),
            has_tests: !test_files.is_empty(),
            has_fuzz: fuzz && !fuzz_targets.is_empty(),
            public_api_changes: public_api.len(),
            imported: true,
            dependency_count,
            semantic_impact: changed_symbols.len(),
            candidate_rollback_rate: 0.0,
        };
        let risk = crate::review::risk::assess(&risk_inputs);

        let selection = crate::review::verification::SelectionInput {
            pack_id: pack_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_digest: revision.revision_digest.clone(),
            dependency_digests: revision.resolved_dependency_digests.clone(),
            changed_files: changed_paths.clone(),
            changed_symbols: changed_symbols.clone(),
            test_files: test_files.clone(),
            fuzz_targets: fuzz_targets.clone(),
            full,
            fuzz,
        };
        let mut evidence = crate::review::verification::plan(&selection);
        let commands = verification_commands(ws, &evidence)?;
        crate::review::verification::execute(&mut evidence, &commands, &ws.root);
        let wsh = crate::workspace::source_view::workspace_hash_cached(
            &ws.root,
            &paths.workspace_hash_cache(),
        )?;
        let verification_key = crate::review::verification::VerificationKey::compose(
            wsh.clone(),
            crate::workspace::config::config_hash(paths)?,
            crate::review::verification::toolchain_hash(&ws.root),
            crate::review::verification::verification_command_hash(&commands),
            crate::review::verification::environment_hash(),
        );
        evidence.verification_key = Some(verification_key.clone());

        // Persist local evidence beside the pack (replacing origin evidence;
        // the origin's signed receipts remain as provenance).
        write_json(&dir.join("risk.json"), &risk)?;
        write_json(&dir.join("verify.json"), &evidence)?;
        crate::review::index::VerificationCacheManifest::record(
            paths,
            crate::review::index::VerificationCacheEntry {
                verification_key: verification_key.key.clone(),
                pack_id: pack_id.clone(),
                result_hash: evidence.result_hash.clone(),
                passed: evidence.passed(),
                recorded_at: now().to_rfc3339(),
            },
        )?;
        crate::review::index::AffectedPathIndex::upsert(paths, &pack_id, changed_paths.clone())?;
        let lsif_summary = serde_json::json!({
            "backend": LSIF_BACKEND,
            "symbols_touched": changed_symbols,
            "public_api_changed": public_api,
            "tests_referencing": test_files,
            "semantic_impact": changed_symbols.len(),
        });
        write_json(&dir.join("lsif.json"), &lsif_summary)?;

        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        let verification_outcome = ledger.record(
            crate::trust::event::EventKind::PackVerified,
            Some(pack_id.clone()),
            None,
            wsh,
            serde_json::json!({
                "imported": true,
                "risk_level": risk.risk_level.as_str(),
                "result_hash": evidence.result_hash,
            }),
        )?;
        if lifecycle.lifecycle == crate::pack::lifecycle::PackLifecycle::Draft {
            lifecycle.transition(crate::pack::lifecycle::PackTransitionRequest {
                operation_id: crate::support::common::OperationId::new(
                    &verification_outcome.event.event_id,
                ),
                expected_revision_id: revision.revision_id.clone(),
                expected_revision_digest: revision.revision_digest.clone(),
                target: crate::pack::lifecycle::PackLifecycle::Verified,
            })?;
            store.write_lifecycle_in(loc, &lifecycle)?;
        }
        quarantine.trust_evaluation = crate::pack::QuarantineState::Verified;
        store.write_quarantine(&quarantine)?;
        crate::review::workflow::WorkflowStore::for_root(&ws.root).write_evidence(
            &crate::review::workflow::EvidenceRecord {
                schema_version: current_version(ContractId::WorkflowEvidence),
                id: EvidenceId::generate(),
                task_id: None,
                pack_id: Some(pack_id.clone()),
                execution_id: None,
                kind: "verification".to_string(),
                state: if evidence.passed() {
                    crate::review::workflow::EvidenceState::Fresh
                } else {
                    crate::review::workflow::EvidenceState::Failed
                },
                result: serde_json::json!({
                    "imported": true,
                    "risk_level": risk.risk_level.as_str(),
                    "risk_score": risk.risk_score,
                    "result_hash": evidence.result_hash,
                    "selected_tests": evidence.selected_tests.len(),
                    "selected_fuzz_targets": evidence.selected_fuzz_targets.len(),
                }),
                produced_at: now(),
                stale_reason: None,
                invalidated_by: vec![],
                receipt_id: Some(verification_outcome.receipt.receipt_id),
            },
        )?;

        Ok(VerifyReport {
            pack_id,
            risk_level: risk.risk_level.as_str().to_string(),
            risk_score: risk.risk_score,
            explanations: risk.explanations,
            required_actions: risk.required_actions,
            selected_tests: evidence.selected_tests,
            selected_fuzz_targets: evidence.selected_fuzz_targets,
            selection_reason: evidence.selection_reason,
            coverage_basis: evidence.coverage_basis,
            symbols_touched: changed_symbols.len(),
            public_api_changed: public_api.len(),
            result_hash: evidence.result_hash,
        })
    }

    /// Index a pack into LSIF from the immutable content objects referenced by
    /// its canonical patch. Workspace files are never used here: they may have
    /// moved on, been deleted, or contain another pack's candidate content.
    fn ensure_pack_indexed(
        &self,
        ws: &Workspace,
        lsif: &crate::review::lsif::LsifIndex,
        pack_id: &str,
    ) -> DraftResult<Vec<String>> {
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let store = crate::pack::PackStore::new(paths.clone());
        let location = store.locate(pack_id).ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!("canonical pack {pack_id} has no storage location"),
            )
        })?;
        let pack_dir = store.dir_for(location, pack_id);
        let patch_path = pack_dir.join("changes.patch");
        let patch: PatchSet = crate::contracts::read_persisted(&patch_path)?;
        let mut canonical = patch.clone();
        canonical.patch_graph_hash.clear();
        if patch.patch_graph_hash != hash_json(&canonical)? {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("canonical pack {pack_id} patch graph digest mismatch"),
            ));
        }
        let object_store = ObjectStore::new(ws.layout.clone());
        let mut content = Vec::with_capacity(patch.files.len());
        for file in &patch.files {
            let object_ref = if matches!(&file.change_kind, FileChangeKind::Deleted) {
                file.old_hash.as_ref()
            } else {
                file.new_hash.as_ref()
            };
            let Some(object_ref) = object_ref else {
                continue;
            };
            let embedded_path = object_ref
                .strip_prefix("b3:")
                .map(|digest| pack_dir.join("objects").join(digest));
            let bytes = if embedded_path.as_ref().is_some_and(|path| path.exists()) {
                read_imported_object(&pack_dir, object_ref)?
            } else {
                object_store.get_bytes(object_ref)?
            };
            content.push((
                file.path.to_string(),
                String::from_utf8_lossy(&bytes).into_owned(),
            ));
        }
        lsif.index_pack(pack_id, &content)?;
        lsif.symbols_touched_by_pack(pack_id)
    }

    /// `draft pack inspect <pck_id>`.
    pub fn pack_inspect(&self, cwd: &Path, pack_ref: &str) -> DraftResult<PackInspectReport> {
        let ws = self.open(cwd)?;
        let pack_id = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let store = crate::pack::PackStore::new(paths.clone());
        let loc = store
            .locate(&pack_id)
            .unwrap_or(crate::pack::PackLocation::Store);
        let manifest = store.read_manifest_in(loc, &pack_id)?;
        let lsif = crate::review::lsif::LsifIndex::open(&paths)?;
        if store.quarantine_record(&pack_id)?.is_none() {
            self.ensure_pack_indexed(&ws, &lsif, &pack_id)?;
        }
        // Imported packs were indexed from their embedded content at local
        // verification; re-indexing from the workspace would erase that.
        let symbols_touched = lsif.symbols_touched_by_pack(&pack_id)?;
        let public_api_changed = lsif.public_api_symbols_changed(&pack_id)?;
        let receipts = crate::trust::receipt::ReceiptStore::new(paths)
            .list()?
            .into_iter()
            .filter(|r| r.subject_id.as_deref() == Some(pack_id.as_str()))
            .map(|r| r.receipt_id)
            .collect();
        let lifecycle_record = store.read_lifecycle_in(loc, &pack_id)?;
        let revision = store.current_revision_in(loc, &pack_id)?;
        let quarantine = store.quarantine_record(&pack_id)?;
        let verified = if matches!(
            lifecycle_record.lifecycle,
            crate::pack::lifecycle::PackLifecycle::Draft
        ) {
            false
        } else {
            let evidence_path = store.dir_for(loc, &pack_id).join("verify.json");
            if !evidence_path.exists() {
                false
            } else {
                let evidence: crate::review::verification::VerifyEvidence =
                    crate::contracts::read_persisted(&evidence_path)?;
                evidence.validate_binding(&revision)?;
                evidence.passed()
            }
        };
        Ok(PackInspectReport {
            valid_actions: lifecycle_record
                .lifecycle
                .valid_actions()
                .iter()
                .map(|action| (*action).to_string())
                .collect(),
            lifecycle: lifecycle_record.lifecycle,
            quarantine,
            verified,
            revision_id: lifecycle_record.revision_id,
            symbols_touched,
            public_api_changed,
            receipts,
            manifest,
        })
    }

    /// Reopen a non-submitted pack as a new mutable revision. Existing
    /// evidence remains on disk as history while its manifest bindings are
    /// cleared so no prior verification or decision can authorize the new
    /// revision.
    pub fn pack_reopen(
        &self,
        cwd: &Path,
        pack_ref: &str,
        operation_id: &str,
    ) -> DraftResult<PackReopenReport> {
        let ws = self.open(cwd)?;
        let pack_id = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let store = crate::pack::PackStore::new(paths);
        let loc = store
            .locate(&pack_id)
            .ok_or_else(|| DraftError::not_found(format!("pack {pack_id} not found")))?;
        let manifest = store.read_manifest_in(loc, &pack_id)?;
        let operation_id = crate::support::common::OperationId::new(operation_id);
        let envelope = store.read_lifecycle_in(loc, &pack_id)?;
        let selected = store.current_revision_in(loc, &pack_id)?;
        let new_revision_id = format!("rev_{}", uuid::Uuid::new_v4().simple());
        let mut new_revision = crate::pack::PackRevision {
            schema_version: current_version(ContractId::PackRevision),
            pack_id: pack_id.clone(),
            manifest_digest: manifest.manifest_digest.clone(),
            revision_id: new_revision_id.clone(),
            revision_number: store.revisions_in(loc, &pack_id)?.len() as u64 + 1,
            revision_digest: String::new(),
            base_digest: selected.base_digest.clone(),
            content_digest: selected.content_digest.clone(),
            diff_digest: selected.diff_digest.clone(),
            target_digest: selected.target_digest.clone(),
            resolved_dependency_digests: selected.resolved_dependency_digests.clone(),
            created_at: now().to_rfc3339(),
        };
        new_revision.refresh_revision_digest();
        store.write_revision_in(loc, &new_revision)?;
        let reopened = envelope.reopen(
            operation_id.clone(),
            new_revision_id,
            new_revision.revision_digest.clone(),
        )?;

        store.write_lifecycle_in(loc, &reopened)?;
        if let Some(mut quarantine) = store.quarantine_record(&pack_id)? {
            quarantine.revision_id = new_revision.revision_id.clone();
            quarantine.revision_digest = new_revision.revision_digest.clone();
            quarantine.trust_evaluation = crate::pack::QuarantineState::Quarantined;
            store.write_quarantine(&quarantine)?;
        }
        let review_lock = store.dir_for(loc, &pack_id).join("review.lock.json");
        if review_lock.exists() {
            std::fs::remove_file(review_lock)?;
        }
        let revision = crate::workspace::source_view::WorkspaceRevision::derive(&ws.root)?;
        crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?.record(
            crate::trust::event::EventKind::PackReopened,
            Some(pack_id.clone()),
            None,
            revision.content_digest,
            serde_json::json!({
                "operation_id": operation_id,
                "revision_id": reopened.revision_id,
                "revision_digest": reopened.revision_digest,
                "evidence_invalidated": true,
            }),
        )?;
        Ok(PackReopenReport {
            pack_id,
            lifecycle: PackLifecycle::Draft,
            revision_id: reopened.revision_id,
            operation_id: operation_id.to_string(),
        })
    }

    /// `draft pack depends <pck_id>`.
    pub fn pack_depends(&self, cwd: &Path, pack_ref: &str) -> DraftResult<PackDependsReport> {
        let ws = self.open(cwd)?;
        let pack_id = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let store = crate::pack::PackStore::new(paths.clone());
        store.read_manifest(&pack_id)?;
        let revision = store.current_revision_in(crate::pack::PackLocation::Store, &pack_id)?;
        let lsif = crate::review::lsif::LsifIndex::open(&paths)?;
        // Index this pack and every other pack so shared-symbol analysis is real.
        let my_symbols = self.ensure_pack_indexed(&ws, &lsif, &pack_id)?;
        for other in store.list()? {
            if other.pack_id != pack_id {
                self.ensure_pack_indexed(&ws, &lsif, &other.pack_id)?;
            }
        }
        // Shortlist packs that touch any of this pack's symbols, then compute
        // the exact shared-symbol overlap only for those.
        let mut shared_symbol_packs = std::collections::BTreeMap::new();
        for other_id in lsif.packs_touching_symbols(&my_symbols)? {
            if other_id == pack_id {
                continue;
            }
            let shared = lsif.possible_semantic_conflicts(&pack_id, &other_id)?;
            if !shared.is_empty() {
                shared_symbol_packs.insert(other_id, shared);
            }
        }
        let lock = store.read_lockfile(&pack_id)?;
        Ok(PackDependsReport {
            pack_id,
            base_workspace_hash: revision.base_digest,
            changed_files: lock.file_hashes.keys().cloned().collect(),
            shared_symbol_packs,
            declared_dependencies: revision.resolved_dependency_digests,
        })
    }

    /// `draft pack conflicts <a> <b>`: textual, semantic, policy, verification,
    /// and dependency conflicts.
    pub fn pack_conflicts(
        &self,
        cwd: &Path,
        a_ref: &str,
        b_ref: &str,
    ) -> DraftResult<PackConflictsReport> {
        let ws = self.open(cwd)?;
        let a = self.resolve_canonical_pack_ref(&ws, a_ref)?;
        let b = self.resolve_canonical_pack_ref(&ws, b_ref)?;
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let store = crate::pack::PackStore::new(paths.clone());
        let ma = store.read_manifest(&a)?;
        let mb = store.read_manifest(&b)?;
        let lock_a = store.read_lockfile(&a)?;
        let lock_b = store.read_lockfile(&b)?;
        let mut conflicts = Vec::new();

        // Textual: same file changed with different content.
        for (path, ha) in &lock_a.file_hashes {
            if let Some(hb) = lock_b.file_hashes.get(path) {
                if ha != hb {
                    conflicts.push(ConflictFinding {
                        kind: "textual".to_string(),
                        detail: format!("both change '{path}' with different content"),
                        blocking: true,
                    });
                }
            }
        }

        // Semantic: both touch the same symbols (via LSIF).
        let lsif = crate::review::lsif::LsifIndex::open(&paths)?;
        self.ensure_pack_indexed(&ws, &lsif, &a)?;
        self.ensure_pack_indexed(&ws, &lsif, &b)?;
        let shared = lsif.possible_semantic_conflicts(&a, &b)?;
        if !shared.is_empty() {
            conflicts.push(ConflictFinding {
                kind: "semantic".to_string(),
                detail: format!("both touch symbols: {}", shared.join(", ")),
                blocking: true,
            });
        }

        // Policy: intent mismatch that policy would treat as incompatible.
        if ma.intent != mb.intent
            && (ma.intent == crate::pack::PackIntent::Security
                || mb.intent == crate::pack::PackIntent::Security)
        {
            conflicts.push(ConflictFinding {
                kind: "policy".to_string(),
                detail: format!(
                    "composing '{}' with '{}' intent requires stronger verification",
                    ma.intent.as_str(),
                    mb.intent.as_str()
                ),
                blocking: false,
            });
        }

        // Verification: an unverified pack cannot be trusted for composition.
        for m in [&ma, &mb] {
            let lifecycle = store
                .read_lifecycle_in(crate::pack::PackLocation::Store, &m.pack_id)?
                .lifecycle;
            if lifecycle == crate::pack::lifecycle::PackLifecycle::Draft {
                conflicts.push(ConflictFinding {
                    kind: "verification".to_string(),
                    detail: format!("pack '{}' is not verified", m.pack_id),
                    blocking: false,
                });
            }
        }

        // Dependency: one pack already declares the other as a dependency.
        for (m, other, lock) in [(&ma, &b, &lock_a), (&mb, &a, &lock_b)] {
            if lock.dependency_pack_hashes.iter().any(|d| d == other) {
                conflicts.push(ConflictFinding {
                    kind: "dependency".to_string(),
                    detail: format!("'{}' depends on '{other}'", m.pack_id),
                    blocking: false,
                });
            }
        }

        let blocking = conflicts.iter().any(|c| c.blocking);
        Ok(PackConflictsReport {
            pack_a: a,
            pack_b: b,
            conflicts,
            blocking,
        })
    }

    /// `draft pack compose <a> <b> --name <name>`: create a new pack combining
    /// two others. Blocking conflicts prevent composition; the result is marked
    /// unverified and must be re-verified.
    pub fn pack_compose(
        &self,
        cwd: &Path,
        a_ref: &str,
        b_ref: &str,
        name: &str,
    ) -> DraftResult<PackComposeReport> {
        use crate::pack::lifecycle::{PackLifecycle, PackLifecycleRecord};
        use crate::pack::{PackLockfile, PackManifest, PackRevision, PackStore};
        let ws = self.open(cwd)?;
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        let wsh = crate::workspace::source_view::workspace_hash_cached(
            &ws.root,
            &paths.workspace_hash_cache(),
        )?;
        let base_stable_head = crate::workspace::stable::StableHeadStore::new(paths.clone())
            .read()?
            .id;
        let conflict_report = self.pack_conflicts(cwd, a_ref, b_ref)?;
        let a = conflict_report.pack_a.clone();
        let b = conflict_report.pack_b.clone();
        let store = PackStore::new(paths.clone());
        // Canonical composition validation (SRS-FR-028–036): the hunk-aware
        // conflict report is authoritative; the composition object carries
        // dependency order and a deterministic composition_hash.
        let manifests = vec![store.read_manifest(&a)?, store.read_manifest(&b)?];
        let locks: Vec<PackLockfile> = [&a, &b]
            .iter()
            .map(|id| store.read_lockfile(id))
            .collect::<DraftResult<_>>()?;
        let blocking_conflicts: Vec<String> = conflict_report
            .conflicts
            .iter()
            .filter(|c| c.blocking)
            .map(|c| format!("{}: {}", c.kind, c.detail))
            .collect();
        ledger.record(
            crate::trust::event::EventKind::CompositionCreated,
            Some(format!("{a}+{b}")),
            None,
            wsh.clone(),
            serde_json::json!({ "sources": [a.clone(), b.clone()], "base_stable_head": base_stable_head }),
        )?;
        let composition = crate::pack::composition::validate(
            &base_stable_head,
            &manifests,
            &locks,
            Some(blocking_conflicts.clone()),
        )?;
        if composition.status == crate::pack::composition::CompositionStatus::Failed {
            ledger.record(
                crate::trust::event::EventKind::CompositionFailed,
                Some(composition.id.clone()),
                None,
                wsh.clone(),
                serde_json::to_value(&composition).expect("Draft-owned records must serialize"),
            )?;
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "cannot compose — blocking conflicts: {}",
                    composition.conflicts.join("; ")
                ),
            ));
        }
        ledger.record(
            crate::trust::event::EventKind::CompositionVerified,
            Some(composition.id.clone()),
            None,
            wsh.clone(),
            serde_json::to_value(&composition).expect("Draft-owned records must serialize"),
        )?;
        if store.name_taken(name)? {
            return Err(DraftError::invalid_config(format!(
                "pack name '{name}' already exists"
            )));
        }
        let ma = store.read_manifest(&a)?;
        let mb = store.read_manifest(&b)?;
        let ra = store.current_revision_in(crate::pack::PackLocation::Store, &a)?;
        let rb = store.current_revision_in(crate::pack::PackLocation::Store, &b)?;
        let read_source_patch =
            |pack_id: &str, revision: &crate::pack::PackRevision| -> DraftResult<PatchSet> {
                let path = paths.pack_changes(pack_id);
                let bytes = fs::read(&path).map_err(|error| {
                    DraftError::new(
                        DraftErrorKind::CorruptData,
                        format!(
                            "canonical source pack {pack_id} is missing {}: {error}",
                            path.display()
                        ),
                    )
                })?;
                if sha256_hex(&bytes) != revision.diff_digest {
                    return Err(DraftError::new(
                        DraftErrorKind::CorruptData,
                        format!("canonical source pack {pack_id} changes digest mismatch"),
                    ));
                }
                let patch: PatchSet = crate::contracts::decode_persisted(&bytes)?;
                let mut canonical = patch.clone();
                canonical.patch_graph_hash.clear();
                if patch.patch_graph_hash != hash_json(&canonical)? {
                    return Err(DraftError::new(
                        DraftErrorKind::CorruptData,
                        format!("canonical source pack {pack_id} patch graph digest mismatch"),
                    ));
                }
                Ok(patch)
            };
        let patch_a = read_source_patch(&a, &ra)?;
        let patch_b = read_source_patch(&b, &rb)?;
        let nonempty_patches = [&patch_a, &patch_b]
            .into_iter()
            .filter(|patch| !patch.files.is_empty())
            .collect::<Vec<_>>();
        if let Some(first) = nonempty_patches.first() {
            if nonempty_patches
                .iter()
                .any(|patch| patch.base_snapshot_id != first.base_snapshot_id)
            {
                return Err(DraftError::new(
                    DraftErrorKind::ConflictDetected,
                    "canonical composition requires source changes with the same immutable base",
                ));
            }
        }
        let base_snapshot_id = nonempty_patches
            .first()
            .map(|patch| patch.base_snapshot_id.clone())
            .unwrap_or_else(|| patch_a.base_snapshot_id.clone());
        let result_snapshot_id = nonempty_patches
            .last()
            .map(|patch| patch.result_snapshot_id.clone())
            .unwrap_or_else(|| patch_a.result_snapshot_id.clone());
        let mut files_by_path: BTreeMap<(WorkspacePath, Option<WorkspacePath>), FilePatch> =
            BTreeMap::new();
        for file in patch_a.files.iter().chain(&patch_b.files) {
            let key = (file.path.clone(), file.old_path.clone());
            if let Some(existing) = files_by_path.get(&key) {
                if hash_json(existing)? != hash_json(file)? {
                    return Err(DraftError::new(
                        DraftErrorKind::ConflictDetected,
                        format!(
                            "canonical composition contains incompatible changes for '{}'",
                            file.path
                        ),
                    ));
                }
            } else {
                files_by_path.insert(key, file.clone());
            }
        }
        let mut combined_patch = PatchSet {
            schema_version: current_version(ContractId::PatchSet),
            id: PatchSetId::generate(),
            base_snapshot_id,
            result_snapshot_id,
            files: files_by_path.into_values().collect(),
            patch_graph_hash: String::new(),
        };
        combined_patch.patch_graph_hash = hash_json(&combined_patch)?;
        let changes_bytes = to_pretty(&combined_patch)?;

        // Make the composed pack self-contained even when a source was a
        // promoted import whose objects live inside that source pack.
        let object_store = ObjectStore::new(ws.layout.clone());
        let mut composed_objects = BTreeMap::new();
        for (source_id, patch) in [(&a, &patch_a), (&b, &patch_b)] {
            let source_dir = store.dir_for(crate::pack::PackLocation::Store, source_id);
            let mut refs = BTreeSet::new();
            for file in &patch.files {
                refs.extend(file.old_hash.iter().cloned());
                refs.extend(file.new_hash.iter().cloned());
                refs.extend(
                    file.hunks
                        .iter()
                        .filter(|hunk| !hunk.content_ref.is_empty())
                        .map(|hunk| hunk.content_ref.clone()),
                );
            }
            for object_ref in refs {
                let hex = object_ref.strip_prefix("b3:").ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::CorruptData,
                        format!("unsupported object ref '{object_ref}'"),
                    )
                })?;
                let embedded = source_dir.join("objects").join(hex);
                let bytes = if embedded.exists() {
                    read_imported_object(&source_dir, &object_ref)?
                } else {
                    object_store.get_bytes(&object_ref)?
                };
                composed_objects.insert(hex.to_string(), bytes);
            }
        }
        let new_id = format!("pck_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);

        // Combined change set from both lockfiles.
        let mut file_hashes = std::collections::BTreeMap::new();
        for id in [&a, &b] {
            let lock = store.read_lockfile(id)?;
            for (f, h) in lock.file_hashes {
                file_hashes.insert(f, h);
            }
        }
        let outcome = ledger.record(
            crate::trust::event::EventKind::PackComposed,
            Some(new_id.clone()),
            None,
            wsh.clone(),
            serde_json::json!({
                "sources": [a, b],
                "name": name,
                "composition_id": composition.id,
                "composition_hash": composition.composition_hash,
                "dependency_order": composition.dependency_order,
            }),
        )?;
        let mut manifest = PackManifest {
            schema_version: current_version(ContractId::PackManifest),
            pack_id: new_id.clone(),
            manifest_digest: String::new(),
            name: name.to_string(),
            description: format!("composed from {a} + {b}"),
            intent: ma.intent,
            provenance: serde_json::json!({"composed_from": [a, b]}),
            author_id: ledger.actor_id().to_string(),
            candidate_id: None,
            declared_dependencies: vec![ma.manifest_digest.clone(), mb.manifest_digest.clone()],
            created_at: now().to_rfc3339(),
        };
        manifest.refresh_manifest_digest();
        store.write_manifest(&manifest)?;
        let composed_dir = store.dir_for(crate::pack::PackLocation::Store, &new_id);
        for (digest, bytes) in composed_objects {
            write_atomic(&composed_dir.join("objects").join(digest), &bytes)?;
        }
        let mut revision = PackRevision {
            schema_version: current_version(ContractId::PackRevision),
            pack_id: new_id.clone(),
            manifest_digest: manifest.manifest_digest.clone(),
            revision_id: format!("rev_{}", uuid::Uuid::new_v4().simple()),
            revision_number: 1,
            revision_digest: String::new(),
            base_digest: crate::support::hashing::domain_hash(
                "draft-pack-composition-base",
                [ra.base_digest.as_bytes(), rb.base_digest.as_bytes()],
            ),
            content_digest: crate::support::hashing::domain_hash(
                "draft-pack-composition-content",
                [ra.content_digest.as_bytes(), rb.content_digest.as_bytes()],
            ),
            diff_digest: sha256_hex(&changes_bytes),
            target_digest: wsh.clone(),
            resolved_dependency_digests: vec![
                ra.revision_digest.clone(),
                rb.revision_digest.clone(),
            ],
            created_at: now().to_rfc3339(),
        };
        revision.refresh_revision_digest();
        store.write_revision(&revision)?;
        write_atomic(&paths.pack_changes(&new_id), &changes_bytes)?;
        store.write_lifecycle_in(
            crate::pack::PackLocation::Store,
            &PackLifecycleRecord {
                schema_version: current_version(ContractId::PackLifecycle),
                pack_id: new_id.clone(),
                revision_id: revision.revision_id.clone(),
                revision_digest: revision.revision_digest.clone(),
                lifecycle: PackLifecycle::Draft,
                updated_at: now(),
                last_operation_id: crate::support::common::OperationId::new(
                    &outcome.event.event_id,
                ),
            },
        )?;
        let lock = PackLockfile {
            schema_version: current_version(ContractId::PackLock),
            pack_id: new_id.clone(),
            workspace_hash: wsh,
            file_hashes,
            policy_version: crate::DRAFT_VERSION.to_string(),
            risk_engine_version: crate::DRAFT_VERSION.to_string(),
            verification_commands: Vec::new(),
            lsif_version: crate::DRAFT_VERSION.to_string(),
            test_selector_version: crate::DRAFT_VERSION.to_string(),
            fuzz_selector_version: crate::DRAFT_VERSION.to_string(),
            dependency_pack_hashes: vec![a.clone(), b.clone()],
            receipt_digests: vec![sha256_hex(outcome.receipt.receipt_id.as_bytes())],
        };
        store.write_lockfile(&lock)?;
        Ok(PackComposeReport {
            pack_id: new_id,
            name: name.to_string(),
            dependencies: vec![a, b],
            requires_reverification: true,
            composition_hash: composition.composition_hash,
        })
    }

    // ---- Thin read/act methods for local review surfaces ----------------

    /// All canonical pack manifests, including quarantined imports (for the
    /// Console pack list and other local review surfaces).
    pub fn list_canonical_packs(&self, cwd: &Path) -> DraftResult<Vec<crate::pack::PackManifest>> {
        let ws = self.open(cwd)?;
        let store =
            crate::pack::PackStore::new(crate::workspace::layout::DraftLayout::for_root(&ws.root));
        let mut packs = store.list()?;
        packs.extend(store.list_quarantined()?);
        packs.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(packs)
    }

    /// A pack's canonical stored diff.
    pub fn pack_diff_text(&self, cwd: &Path, pack_ref: &str) -> DraftResult<String> {
        let ws = self.open(cwd)?;
        let pid = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        let store =
            crate::pack::PackStore::new(crate::workspace::layout::DraftLayout::for_root(&ws.root));
        let loc = store.locate(&pid).ok_or_else(|| {
            DraftError::not_found(format!("canonical pack '{pid}' was not found"))
        })?;
        let p = store.dir_for(loc, &pid).join("changes.patch");
        fs::read_to_string(&p)
            .map_err(|error| DraftError::storage(format!("cannot read {}: {error}", p.display())))
    }

    /// A pack's stored risk report (or null).
    pub fn pack_risk_json(&self, cwd: &Path, pack_ref: &str) -> DraftResult<Value> {
        let ws = self.open(cwd)?;
        let pid = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        let store =
            crate::pack::PackStore::new(crate::workspace::layout::DraftLayout::for_root(&ws.root));
        let loc = store.locate(&pid).ok_or_else(|| {
            DraftError::not_found(format!("canonical pack '{pid}' was not found"))
        })?;
        let p = store.dir_for(loc, &pid).join("risk.json");
        if p.exists() {
            let risk: crate::review::risk::RiskReport = crate::contracts::read_persisted(&p)?;
            serde_json::to_value(risk).map_err(|error| {
                DraftError::new(
                    DraftErrorKind::Internal,
                    format!("serialize risk report: {error}"),
                )
            })
        } else {
            Ok(Value::Null)
        }
    }

    /// Signed receipts referencing a pack.
    pub fn pack_receipts(
        &self,
        cwd: &Path,
        pack_ref: &str,
    ) -> DraftResult<Vec<crate::trust::receipt::ReceiptRecord>> {
        let ws = self.open(cwd)?;
        let pid = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        Ok(crate::trust::receipt::ReceiptStore::new(
            crate::workspace::layout::DraftLayout::for_root(&ws.root),
        )
        .list()?
        .into_iter()
        .filter(|r| r.subject_id.as_deref() == Some(pid.as_str()))
        .collect())
    }

    /// The canonical hash-chained event log.
    pub fn canonical_events(
        &self,
        cwd: &Path,
    ) -> DraftResult<Vec<crate::trust::event::EventRecord>> {
        let ws = self.open(cwd)?;
        crate::trust::event::EventLog::workspace(
            crate::workspace::layout::DraftLayout::for_root(&ws.root),
            ws.workspace_id.to_string(),
        )
        .read_all()
    }

    /// Approve or reject a pack and record the canonical signed decision.
    pub fn decide_pack(
        &self,
        cwd: &Path,
        pack_ref: &str,
        approve: bool,
        reason: Option<String>,
    ) -> DraftResult<String> {
        let ws = self.open(cwd)?;
        // Imported packs also advance their separate quarantine trust record.
        match self.resolve_canonical_pack_ref(&ws, pack_ref) {
            Ok(pack_id) => {
                let store = crate::pack::PackStore::new(
                    crate::workspace::layout::DraftLayout::for_root(&ws.root),
                );
                if let Some(loc) = store.locate(&pack_id) {
                    let manifest = store.read_manifest_in(loc, &pack_id)?;
                    if store.quarantine_record(&pack_id)?.is_some() {
                        return self
                            .decide_imported_pack(&ws, &store, loc, manifest, approve, reason);
                    }
                }
            }
            Err(error) if error.kind == DraftErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let decision = if approve {
            DecisionKind::Approve
        } else {
            DecisionKind::Reject
        };
        self.decide_selected(cwd, Some(pack_ref), decision, reason)?;
        let pack = self.resolve_pack_ref(&ws, pack_ref)?;
        self.sync_canonical_pack(
            &ws,
            &pack,
            None,
            PackSyncSpec {
                kind: if approve {
                    crate::trust::event::EventKind::PackApproved
                } else {
                    crate::trust::event::EventKind::PackRejected
                },
                intent: crate::pack::PackIntent::Feature,
                lifecycle: if approve {
                    crate::pack::lifecycle::PackLifecycle::Approved
                } else {
                    crate::pack::lifecycle::PackLifecycle::Rejected
                },
                metadata: serde_json::json!({ "via": "console" }),
            },
        )?;
        Ok(pack.id.to_string())
    }

    /// Approve or reject an imported pack while keeping quarantine trust state
    /// separate from the canonical review lifecycle.
    fn decide_imported_pack(
        &self,
        ws: &Workspace,
        store: &crate::pack::PackStore,
        loc: crate::pack::PackLocation,
        manifest: crate::pack::PackManifest,
        approve: bool,
        reason: Option<String>,
    ) -> DraftResult<String> {
        use crate::pack::lifecycle::{PackLifecycle, PackTransitionRequest};
        use crate::pack::QuarantineState;
        let mut quarantine = store.read_quarantine(&manifest.pack_id)?;
        let revision = store.current_revision_in(loc, &manifest.pack_id)?;
        let mut lifecycle = store.read_lifecycle_in(loc, &manifest.pack_id)?;
        let target = if approve {
            QuarantineState::Approved
        } else {
            QuarantineState::Rejected
        };
        if approve && quarantine.trust_evaluation == QuarantineState::Quarantined {
            return Err(DraftError::new(
                DraftErrorKind::ReviewRequired,
                "imported packs must be locally verified before approval",
            )
            .with_suggestion("run `draft verify <pck_id>` first"));
        }
        if !crate::pack::can_quarantine_transition(quarantine.trust_evaluation, target) {
            return Err(DraftError::invalid_config(format!(
                "imported pack in trust state '{:?}' cannot be {}",
                quarantine.trust_evaluation,
                if approve { "approved" } else { "rejected" }
            )));
        }
        if approve && lifecycle.lifecycle != PackLifecycle::Verified {
            return Err(DraftError::new(
                DraftErrorKind::ReviewRequired,
                "only the currently verified immutable revision may be approved",
            ));
        }

        let wsh = crate::workspace::source_view::workspace_hash(&ws.root)?;
        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        let outcome = ledger.record(
            if approve {
                crate::trust::event::EventKind::PackApproved
            } else {
                crate::trust::event::EventKind::PackRejected
            },
            Some(manifest.pack_id.clone()),
            None,
            wsh,
            serde_json::json!({
                "via": "console",
                "imported": true,
                "reason": reason,
            }),
        )?;
        if lifecycle.lifecycle == PackLifecycle::Verified {
            let operation_id = crate::support::common::OperationId::new(&outcome.event.event_id);
            lifecycle.transition(PackTransitionRequest {
                operation_id: operation_id.clone(),
                expected_revision_id: revision.revision_id.clone(),
                expected_revision_digest: revision.revision_digest.clone(),
                target: PackLifecycle::Reviewing,
            })?;
            lifecycle.transition(PackTransitionRequest {
                operation_id,
                expected_revision_id: revision.revision_id.clone(),
                expected_revision_digest: revision.revision_digest.clone(),
                target: if approve {
                    PackLifecycle::Approved
                } else {
                    PackLifecycle::Rejected
                },
            })?;
            store.write_lifecycle_in(loc, &lifecycle)?;
        }
        quarantine.trust_evaluation = target;
        store.write_quarantine(&quarantine)?;
        Ok(manifest.pack_id)
    }

    /// Import a `.draftpack` provided as bytes (Console upload).
    pub fn pack_import_bytes(
        &self,
        cwd: &Path,
        bytes: &[u8],
        name: Option<&str>,
    ) -> DraftResult<PackImportReport> {
        let tmp = std::env::temp_dir().join(format!(
            "draft-import-{}.draftpack",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&tmp, bytes)
            .map_err(|e| DraftError::storage(format!("write temp import: {e}")))?;
        let result = self.pack_import(cwd, &tmp, name, false);
        let _ = std::fs::remove_file(&tmp);
        result
    }

    pub fn receipts(&self, cwd: &Path) -> DraftResult<Vec<Value>> {
        let ws = self.open(cwd)?;
        crate::trust::receipt::ReceiptStore::new(ws.layout)
            .list()?
            .into_iter()
            .map(|receipt| serde_json::to_value(receipt).map_err(DraftError::from))
            .collect()
    }

    pub fn storage_stats(&self, cwd: &Path) -> DraftResult<StorageStats> {
        let ws = self.open(cwd)?;
        Ok(StorageStats {
            draft_size_bytes: dir_size(&ws.layout.draft_dir)?,
            repo_size_bytes: dir_size_excluding_draft(&ws.root)?,
            objects_size_bytes: dir_size(&ws.layout.objects_dir())?,
            packs_size_bytes: dir_size(&ws.layout.packs_dir())?,
            receipts_size_bytes: dir_size(&ws.layout.receipts_dir())?,
            events_size_bytes: fs::metadata(ws.layout.event_log())
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

    pub fn doctor_index(&self, cwd: &Path, refresh: bool) -> DraftResult<Value> {
        let ws = self.open(cwd)?;
        if refresh || !ws.layout.index_file().exists() {
            rebuild_index(&ws)?;
            crate::task::TaskStore::for_root(&ws.root).rebuild_index()?;
        }
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let indexes = vec![
            index_status("file", ws.layout.index_file(), &[ws.layout.snapshots_dir()])?,
            index_status(
                "symbol",
                paths.lsif_index_db(),
                std::slice::from_ref(&ws.root),
            )?,
            index_status("task", paths.task_name_index(), &[paths.tasks_dir()])?,
            index_status("pack", paths.stable_graph_index(), &[paths.packs_dir()])?,
            index_status(
                "receipt",
                paths.receipts_dir().join("index.json"),
                &[paths.receipts_dir()],
            )?,
            index_status(
                "search",
                paths.indexes_dir().join("search.json"),
                std::slice::from_ref(&ws.root),
            )?,
            index_status("activity", paths.event_index(), &[paths.event_log()])?,
        ];
        let state = if indexes.iter().any(|i| i["state"] == "failed") {
            "failed"
        } else if indexes.iter().any(|i| i["state"] == "missing") {
            "missing"
        } else if indexes.iter().any(|i| i["state"] == "stale") {
            "stale"
        } else {
            "fresh"
        };
        Ok(serde_json::json!({
            "state": state,
            "scope": "project",
            "refreshed": refresh,
            "indexes": indexes,
        }))
    }

    pub fn doctor_index_global(&self, refresh: bool) -> DraftResult<Value> {
        let home = crate::workspace::home::DraftGlobalStore::locate()?;
        if refresh {
            home.create_all()?;
        }
        let indexes = vec![
            index_status(
                "registry",
                home.registry_dir().join("projects.index"),
                &[home.registry_dir().join("projects.jsonl")],
            )?,
            index_status(
                "receipt",
                home.global_receipt_index(),
                &[home.receipts_dir()],
            )?,
            index_status(
                "candidate",
                home.indexes_dir().join("candidates.json"),
                &[home.candidates_json()],
            )?,
            index_status(
                "activity",
                home.indexes_dir().join("activity.json"),
                &[home.logs_dir()],
            )?,
        ];
        let state = if indexes.iter().any(|i| i["state"] == "failed") {
            "failed"
        } else if indexes.iter().any(|i| i["state"] == "missing") {
            "missing"
        } else if indexes.iter().any(|i| i["state"] == "stale") {
            "stale"
        } else {
            "fresh"
        };
        Ok(serde_json::json!({
            "state": state,
            "scope": "global",
            "root": home.root(),
            "refreshed": refresh,
            "indexes": indexes,
        }))
    }

    pub fn storage_gc(&self, cwd: &Path) -> DraftResult<StorageMaintenanceReport> {
        let ws = self.open(cwd)?;
        let removed = garbage_collect_objects(&ws)?;
        ws.events()?.append(
            "storage.gc_completed",
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
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let _lock = FileGuard::acquire(&paths.lock_file("gc"), Duration::from_secs(30))?;
        let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
        ledger.record(
            crate::trust::event::EventKind::GcStarted,
            None,
            None,
            crate::workspace::source_view::workspace_hash(&ws.root)?,
            serde_json::json!({}),
        )?;
        match crate::app::maintenance::run(&paths) {
            Ok(report) => {
                ledger.record(
                    crate::trust::event::EventKind::GcCompleted,
                    None,
                    None,
                    crate::workspace::source_view::workspace_hash(&ws.root)?,
                    serde_json::to_value(&report).expect("GC report is serializable"),
                )?;
                Ok(report)
            }
            Err(e) => Err(e),
        }
    }

    pub fn close(&self, cwd: &Path, force: bool) -> DraftResult<CloseReport> {
        // Recovery must remain possible when obsolete profile state blocks all
        // normal operations. Close never reads or applies that state.
        let ws = self.open_workspace(cwd, true)?;
        let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
        let _lock = FileGuard::acquire(&paths.lock_file("close"), Duration::from_secs(30))?;
        let home = crate::workspace::home::DraftGlobalStore::locate()?;
        let retired_profile_present =
            crate::trust::identity::reject_retired_profile_state(Some(&paths.draft_dir)).is_err()
                || crate::trust::identity::global::reject_retired_actor_profile(&home).is_err()
                || crate::workspace::config::reject_retired_profile_config(&paths.config_toml())
                    .is_err()
                || crate::workspace::config::reject_retired_profile_config(&home.config_toml())
                    .is_err();
        let pending_packs = unsafe_pending_pack_count(&paths)?;
        if pending_packs > 0 && !force {
            if !retired_profile_present {
                let ledger =
                    crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
                let _ = ledger.record(
                    crate::trust::event::EventKind::CloseFailed,
                    None,
                    None,
                    crate::workspace::source_view::workspace_hash(&ws.root)?,
                    serde_json::json!({
                        "reason": "pending packs",
                        "pending_packs": pending_packs
                    }),
                );
            }
            return Err(DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                format!("draft close refused: {pending_packs} pending pack(s) remain"),
            )
            .with_suggestion("submit, delete, or export pending packs first; use --force only when you intend to discard Draft metadata"));
        }
        if !retired_profile_present {
            let ledger =
                crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
            ledger.record(
                crate::trust::event::EventKind::CloseStarted,
                None,
                None,
                crate::workspace::source_view::workspace_hash(&ws.root)?,
                serde_json::json!({ "forced": force, "pending_packs": pending_packs }),
            )?;
            ledger.record(
                crate::trust::event::EventKind::CloseCompleted,
                None,
                None,
                crate::workspace::source_view::workspace_hash(&ws.root)?,
                serde_json::json!({ "forced": force, "pending_packs": pending_packs }),
            )?;
        }
        let draft_dir = ws.layout.draft_dir.display().to_string();
        crate::workspace::registry::ProjectRegistry::global()?.remove(ws.workspace_id.as_str())?;
        std::fs::remove_dir_all(&ws.layout.draft_dir)
            .map_err(|e| DraftError::storage(format!("remove .draft: {e}")))?;
        Ok(CloseReport {
            closed: true,
            forced: force,
            draft_dir,
            pending_packs,
        })
    }

    pub fn storage_compact(&self, cwd: &Path) -> DraftResult<StorageMaintenanceReport> {
        let ws = self.open(cwd)?;
        let compacted = compact_loose_objects(&ws)?;
        ws.events()?.append(
            "storage.compacted",
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
            "storage.pruned",
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
        let chain = ws.events()?.verify_chain()?;
        let object_errors = verify_objects(&ws)?;
        let receipt_errors = verify_receipts(&ws)?;
        let draft_exclusion_errors = verify_draft_hard_exclusion(&ws)?;
        Ok(StorageDoctorReport {
            event_chain_ok: chain.ok,
            event_chain_error: chain.error,
            draft_hard_excluded: draft_exclusion_errors.is_empty(),
            draft_exclusion_errors,
            objects_ok: object_errors.is_empty(),
            object_errors,
            receipts_ok: receipt_errors.is_empty(),
            receipt_errors,
            receipts: list_with_extension(&ws.layout.receipts_dir(), "json")?.len(),
            packs: self.pack_list(cwd)?.len(),
        })
    }

    pub fn receipt_show(&self, cwd: &Path, id: &str) -> DraftResult<Value> {
        validate_receipt_id(id)?;
        let ws = self.open(cwd)?;
        let p = ws.layout.receipts_dir().join(format!("{}.json", id));
        Ok(serde_json::from_str(&fs::read_to_string(&p).map_err(
            |e| DraftError::not_found(format!("cannot read receipt {id}: {e}")),
        )?)?)
    }

    pub fn events(&self, cwd: &Path) -> DraftResult<Vec<crate::trust::event::EventRecord>> {
        self.open(cwd)?.events()?.read_all()
    }

    pub fn events_page(
        &self,
        cwd: &Path,
        top: bool,
        bottom: bool,
        page: Option<usize>,
        limit: Option<usize>,
        filter: Option<&str>,
    ) -> DraftResult<Vec<crate::trust::event::EventRecord>> {
        self.open(cwd)?
            .events()?
            .read_page(top, bottom, page, limit, filter)
    }

    pub fn verify_events(&self, cwd: &Path) -> DraftResult<HashChainStatus> {
        self.open(cwd)?.events()?.verify_chain()
    }

    pub fn replay_events(&self, cwd: &Path) -> DraftResult<EventReplayReport> {
        let ws = self.open(cwd)?;
        let events = ws.events()?.read_all()?;
        let mut by_type = BTreeMap::new();
        for event in &events {
            *by_type.entry(event.event_type.clone()).or_insert(0usize) += 1;
        }
        let chain = ws.events()?.verify_chain()?;
        Ok(EventReplayReport {
            workspace_id: ws.workspace_id.to_string(),
            events: events.len(),
            by_type,
            chain_ok: chain.ok,
            error: chain.error,
        })
    }

    pub fn index_rebuild(&self, cwd: &Path) -> DraftResult<IndexReport> {
        let ws = self.open(cwd)?;
        rebuild_index(&ws)
    }
}

fn append_project_config_event(ws: &Workspace, key: &str, operation: &str) -> DraftResult<()> {
    let bytes = std::fs::read(ws.layout.config_toml())?;
    ws.events()?.append(
        if key.starts_with("user.") {
            "user.profile.updated"
        } else {
            "config.updated"
        },
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
    fn events(&self) -> DraftResult<WorkspaceEventLog> {
        Ok(WorkspaceEventLog::new(
            self.layout.clone(),
            self.workspace_id.clone(),
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitReport {
    pub workspace_id: String,
    pub root: String,
    pub created: bool,
    pub draft_dir: String,
    pub stable_head_id: String,
    pub stable_head_receipt_id: String,
    pub workspace_hash: String,
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
    pub pending_packs: usize,
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
    pub packs_size_bytes: u64,
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
    pub event_chain_ok: bool,
    pub event_chain_error: Option<String>,
    pub draft_hard_excluded: bool,
    #[serde(default)]
    pub draft_exclusion_errors: Vec<String>,
    pub objects_ok: bool,
    pub object_errors: Vec<String>,
    pub receipts_ok: bool,
    pub receipt_errors: Vec<String>,
    pub receipts: usize,
    pub packs: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointReport {
    pub snapshot_id: String,
    pub receipt_id: String,
    pub files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpawnReport {
    pub task_id: String,
    pub task_name: String,
    pub task_kind: String,
    pub preset: Option<String>,
    pub parent_pack: Option<String>,
    pub executions: Vec<ExecutionSummary>,
    pub next_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSummary {
    pub execution_id: String,
    pub candidate: String,
    pub status: String,
    pub produced_pack: Option<String>,
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
    pub packs: bool,
    pub conflicts: bool,
    pub lanes: bool,
    pub evidence: bool,
    pub timeline: bool,
    pub explain: bool,
    pub decompose: bool,
    pub diff_stable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidatePackAssignment {
    pub pack_id: String,
    pub candidate: String,
    pub task_id: Option<String>,
    pub execution_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackReport {
    pub lifecycle: PackLifecycle,
    pub pack: PackWorkspace,
    pub patch: PatchSet,
    pub evidence: Option<Evidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackDeleteReport {
    pub deleted_pack_id: String,
    pub deleted_pack_name: Option<String>,
    pub replacement_selected_pack: String,
    pub deleted_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitReadinessReport {
    pub ok: bool,
    pub blockers: Vec<String>,
    #[serde(default)]
    pub ownership: Option<crate::workspace::ownership::OwnershipReport>,
    #[serde(default)]
    pub reviewability: Option<crate::review::reviewability::ReviewabilityReport>,
    #[serde(default)]
    pub verification_receipt_id: Option<String>,
    #[serde(default)]
    pub review_receipt_id: Option<String>,
    #[serde(default)]
    pub approval_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareReport {
    pub id: String,
    pub left_pack: String,
    pub right_pack: String,
    pub overlapping_files: Vec<WorkspacePath>,
    #[serde(default)]
    pub overlapping_hunks: Vec<HunkOverlap>,
    pub unique_left_files: Vec<WorkspacePath>,
    pub unique_right_files: Vec<WorkspacePath>,
    #[serde(default)]
    pub compatible: bool,
    pub warnings: Vec<String>,
    pub recommendation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeResult {
    pub output_pack_id: String,
    pub source_packs: Vec<String>,
    pub receipt_id: String,
    #[serde(default)]
    pub files: usize,
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
    pub source_pack_id: String,
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
    pub packs: usize,
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

fn verification_commands(
    ws: &Workspace,
    evidence: &crate::review::verification::VerifyEvidence,
) -> DraftResult<Vec<String>> {
    let mut commands = std::collections::BTreeSet::new();
    commands.extend(
        evidence
            .selected_tests
            .iter()
            .map(|test| test.command.clone()),
    );
    commands.extend(
        evidence
            .selected_fuzz_targets
            .iter()
            .map(|target| target.command.clone()),
    );
    if ws.layout.verify_toml().exists() {
        let config: VerificationConfig = read_toml(&ws.layout.verify_toml())?;
        if !crate::contracts::supports_version(
            ContractId::VerificationConfig,
            config.schema_version,
        ) {
            return Err(DraftError::new(
                DraftErrorKind::UnsupportedSchema,
                format!(
                    "verification configuration schema {} is unsupported",
                    config.schema_version
                ),
            ));
        }
        commands.extend(
            config
                .checks
                .into_iter()
                .filter(|check| check.enabled && !check.command.trim().is_empty())
                .map(|check| check.command),
        );
    }
    Ok(commands.into_iter().collect())
}

fn wire_binding_error(contract: &str, error: DraftError) -> DraftError {
    if error.kind == DraftErrorKind::UnsupportedSchema {
        error
    } else {
        DraftError::new(
            DraftErrorKind::Validation,
            format!("invalid {contract}: {}", error.message),
        )
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

fn validate_task_definition(ws: &Workspace, task: &crate::task::TaskDefinition) -> DraftResult<()> {
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
    let protected_rules = crate::workspace::protected::rules_for_project(&ws.root)?;
    if let Some(zone) = task
        .allowed_zones
        .iter()
        .find(|zone| crate::workspace::protected::matches_rules(&protected_rules, zone))
    {
        return Err(DraftError::new(
            DraftErrorKind::ProtectedFileAccess,
            format!("task allowed zone '{zone}' conflicts with protected-file rules"),
        )
        .with_suggestion("remove protected paths from --allow and keep them in --forbid"));
    }
    serde_json::to_value(task)
        .and_then(serde_json::from_value::<crate::task::TaskDefinition>)
        .map_err(|err| DraftError::storage(format!("task schema round-trip failed: {err}")))?;
    Ok(())
}

fn empty_snapshot(ws: &Workspace) -> Snapshot {
    Snapshot {
        schema_version: current_version(ContractId::WorkspaceSnapshot),
        id: SnapshotId::new("chk_empty"),
        workspace_id: ws.workspace_id.clone(),
        manifest_hash: sha256_hex(b"empty"),
        files: vec![],
        content_object_refs: vec![],
        ignored_patterns_hash: sha256_hex(b""),
        created_at: now(),
        created_by: ActorRef {
            id: ActorId::new("act_system"),
            kind: ActorKind::Service,
            display_name: "draft".to_string(),
        },
    }
}

fn load_snapshot(ws: &Workspace, id: &SnapshotId) -> DraftResult<Snapshot> {
    if id.as_str() == "chk_empty" {
        return Ok(empty_snapshot(ws));
    }
    crate::contracts::read_persisted(&ws.layout.snapshots_dir().join(format!("{}.json", id)))
}

fn snapshot_file_fingerprint(snapshot: &Snapshot) -> String {
    let mut files = snapshot.files.clone();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let stable = files
        .into_iter()
        .map(|f| {
            serde_json::json!({
                "path": f.path,
                "file_kind": f.file_kind,
                "content_hash": f.content_hash,
                "size_bytes": f.size_bytes,
                "executable": f.executable,
            })
        })
        .collect::<Vec<_>>();
    sha256_hex(canonical_json(&Value::Array(stable)).as_bytes())
}

fn diff_snapshots(ws: &Workspace, base: &Snapshot, result: &Snapshot) -> DraftResult<PatchSet> {
    let mut patch = diff_snapshot_values(base, result)?;
    enrich_patch_hunks(ws, base, result, &mut patch)?;
    patch.patch_graph_hash.clear();
    patch.patch_graph_hash = hash_json(&patch)?;
    write_json(
        &ws.layout.tmp_dir().join(format!("{}.json", patch.id)),
        &patch,
    )?;
    Ok(patch)
}

fn diff_snapshot_values(base: &Snapshot, result: &Snapshot) -> DraftResult<PatchSet> {
    let old: BTreeMap<_, _> = base
        .files
        .iter()
        .map(|f| (f.path.clone(), f.clone()))
        .collect();
    let new: BTreeMap<_, _> = result
        .files
        .iter()
        .map(|f| (f.path.clone(), f.clone()))
        .collect();
    let files = diff_manifests(&old, &new)
        .into_iter()
        .map(|c| FilePatch {
            path: c.path,
            old_path: match &c.change_kind {
                FileChangeKind::Renamed { from } => Some(from.clone()),
                _ => None,
            },
            change_kind: c.change_kind,
            hunks: vec![],
            binary: matches!(c.file_kind, FileKind::Binary),
            old_hash: c.old_hash,
            new_hash: c.new_hash,
        })
        .collect::<Vec<_>>();
    let mut patch = PatchSet {
        schema_version: current_version(ContractId::PatchSet),
        id: PatchSetId::generate(),
        base_snapshot_id: base.id.clone(),
        result_snapshot_id: result.id.clone(),
        files,
        patch_graph_hash: String::new(),
    };
    patch.patch_graph_hash = hash_json(&patch)?;
    Ok(patch)
}

fn enrich_patch_hunks(
    ws: &Workspace,
    base: &Snapshot,
    result: &Snapshot,
    patch: &mut PatchSet,
) -> DraftResult<()> {
    let store = ObjectStore::new(ws.layout.clone());
    let old_by_path: BTreeMap<_, _> = base
        .files
        .iter()
        .map(|f| (f.path.clone(), f.clone()))
        .collect();
    let new_by_path: BTreeMap<_, _> = result
        .files
        .iter()
        .map(|f| (f.path.clone(), f.clone()))
        .collect();
    for file in &mut patch.files {
        if file.binary {
            continue;
        }
        let old_entry = file
            .old_path
            .as_ref()
            .and_then(|p| old_by_path.get(p))
            .or_else(|| old_by_path.get(&file.path));
        let new_entry = new_by_path.get(&file.path);
        let old_text = read_text_object(&store, old_entry.and_then(|e| e.content_hash.as_ref()))?;
        let new_text = read_text_object(&store, new_entry.and_then(|e| e.content_hash.as_ref()))?;
        if old_text.is_none() && new_text.is_none() {
            continue;
        }
        file.hunks = build_text_hunks(
            &store,
            &file.path,
            old_text.as_deref().unwrap_or(""),
            new_text.as_deref().unwrap_or(""),
        )?;
    }
    Ok(())
}

fn read_text_object(
    store: &ObjectStore,
    object_ref: Option<&String>,
) -> DraftResult<Option<String>> {
    let Some(object_ref) = object_ref else {
        return Ok(None);
    };
    let bytes = store.get_bytes(object_ref)?;
    match String::from_utf8(bytes) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Ok(None),
    }
}

fn build_text_hunks(
    store: &ObjectStore,
    path: &WorkspacePath,
    old_text: &str,
    new_text: &str,
) -> DraftResult<Vec<PatchHunk>> {
    if old_text == new_text {
        return Ok(Vec::new());
    }
    let old_lines = split_lines_preserve(old_text);
    let new_lines = split_lines_preserve(new_text);
    let mut prefix = 0usize;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix + prefix < old_lines.len()
        && suffix + prefix < new_lines.len()
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let old_changed = &old_lines[prefix..old_lines.len().saturating_sub(suffix)];
    let new_changed = &new_lines[prefix..new_lines.len().saturating_sub(suffix)];
    let old_start = prefix as u32 + 1;
    let new_start = prefix as u32 + 1;
    let old_joined = old_changed.concat();
    let new_joined = new_changed.concat();
    let hunk_body = format!(
        "--- {}\n+++ {}\n@@ -{},{} +{},{} @@\n{}{}",
        path,
        path,
        old_start,
        old_changed.len(),
        new_start,
        new_changed.len(),
        old_changed
            .iter()
            .map(|l| format!("-{l}"))
            .collect::<String>(),
        new_changed
            .iter()
            .map(|l| format!("+{l}"))
            .collect::<String>(),
    );
    let old_hash = if old_joined.is_empty() {
        None
    } else {
        Some(format!("b3:{}", blake3_hex(old_joined.as_bytes())))
    };
    let new_hash = if new_joined.is_empty() {
        None
    } else {
        Some(format!("b3:{}", blake3_hex(new_joined.as_bytes())))
    };
    let id_input = format!(
        "{}:{}:{}:{}:{}:{}",
        path,
        old_start,
        old_changed.len(),
        new_start,
        new_changed.len(),
        sha256_hex(hunk_body.as_bytes())
    );
    let hunk_digest = sha256_hex(id_input.as_bytes());
    let hunk_digest = hunk_digest
        .strip_prefix("sha256:")
        .expect("canonical SHA-256 digests carry their algorithm prefix");
    Ok(vec![PatchHunk {
        id: format!("hunk_{}", &hunk_digest[..12]),
        old_start,
        old_lines: old_changed.len() as u32,
        new_start,
        new_lines: new_changed.len() as u32,
        content_ref: store.put_bytes(hunk_body.as_bytes())?,
        old_content_hash: old_hash,
        new_content_hash: new_hash,
    }])
}

fn split_lines_preserve(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split_inclusive('\n')
        .map(ToString::to_string)
        .collect()
}

fn file_level_conflict(left: &FilePatch, right: &FilePatch) -> bool {
    left.hunks.is_empty()
        || right.hunks.is_empty()
        || left.binary
        || right.binary
        || !matches!(
            (&left.change_kind, &right.change_kind),
            (FileChangeKind::Modified, FileChangeKind::Modified)
        )
}

fn hunk_overlaps(left: &PatchSet, right: &PatchSet) -> Vec<HunkOverlap> {
    let mut out = Vec::new();
    for lf in &left.files {
        for rf in right.files.iter().filter(|rf| rf.path == lf.path) {
            if file_level_conflict(lf, rf) {
                continue;
            }
            for lh in &lf.hunks {
                for rh in &rf.hunks {
                    if ranges_overlap(lh.old_start, lh.old_lines, rh.old_start, rh.old_lines)
                        || ranges_overlap(lh.new_start, lh.new_lines, rh.new_start, rh.new_lines)
                    {
                        out.push(HunkOverlap {
                            path: lf.path.clone(),
                            left_hunk_id: lh.id.clone(),
                            right_hunk_id: rh.id.clone(),
                            old_start: lh.old_start.min(rh.old_start),
                            old_end: range_end(lh.old_start, lh.old_lines)
                                .max(range_end(rh.old_start, rh.old_lines)),
                            new_start: lh.new_start.min(rh.new_start),
                            new_end: range_end(lh.new_start, lh.new_lines)
                                .max(range_end(rh.new_start, rh.new_lines)),
                        });
                    }
                }
            }
        }
    }
    out
}

fn ranges_overlap(a_start: u32, a_len: u32, b_start: u32, b_len: u32) -> bool {
    let a_end = range_end(a_start, a_len);
    let b_end = range_end(b_start, b_len);
    a_start <= b_end && b_start <= a_end
}

fn range_end(start: u32, len: u32) -> u32 {
    if len == 0 {
        start
    } else {
        start + len - 1
    }
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

fn load_pack(ws: &Workspace, id: &str) -> DraftResult<PackWorkspace> {
    let pack: PackWorkspace = crate::contracts::read_persisted(
        &ws.layout
            .pack_workspaces_dir()
            .join(id)
            .join("staging.json"),
    )?;
    pack.validate()?;
    Ok(pack)
}

fn pack_lifecycle(ws: &Workspace, id: &PackId) -> DraftResult<PackLifecycle> {
    let store = crate::pack::PackStore::new(ws.layout.clone());
    let location = store.locate(id.as_str()).ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!("pack staging state {} has no canonical pack", id),
        )
    })?;
    Ok(store.read_lifecycle_in(location, id.as_str())?.lifecycle)
}

fn save_pack_staging(ws: &Workspace, pack: &mut PackWorkspace) -> DraftResult<()> {
    pack.updated_at = now();
    pack.manifest_hash.clear();
    pack.manifest_hash = hash_json(pack)?;
    write_json(
        &ws.layout.pack_workspace_dir(&pack.id).join("staging.json"),
        pack,
    )
}

fn dispose_pack_metadata(
    ws: &Workspace,
    paths: &crate::workspace::layout::DraftLayout,
    pack_id: &str,
) -> DraftResult<usize> {
    validate_pack_id(pack_id)?;
    let mut removed = 0;
    let staging_dir = ws.layout.pack_workspaces_dir().join(pack_id);
    if staging_dir.exists() {
        std::fs::remove_dir_all(&staging_dir)
            .map_err(|e| DraftError::storage(format!("dispose pack {pack_id}: {e}")))?;
        removed += 1;
    }
    // Canonical manifests, revisions, lifecycle, evidence, and receipts are
    // immutable history. Disposal removes only mutable staging state.
    let selected = ws.layout.selected_pack_file();
    if selected.exists()
        && std::fs::read_to_string(&selected)
            .map(|s| s.trim() == pack_id)
            .unwrap_or(false)
    {
        std::fs::remove_file(&selected)
            .map_err(|e| DraftError::storage(format!("clear selected pack: {e}")))?;
        removed += 1;
    }
    crate::review::index::AffectedPathIndex::remove(paths, pack_id)?;
    Ok(removed)
}

fn unsafe_pending_pack_count(paths: &crate::workspace::layout::DraftLayout) -> DraftResult<usize> {
    if !paths.packs_dir().exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in std::fs::read_dir(paths.packs_dir())? {
        let manifest_path = entry?.path().join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        let manifest: crate::pack::PackManifest = crate::contracts::read_persisted(&manifest_path)?;
        if manifest.description != "base pack" {
            count += 1;
        }
    }
    Ok(count)
}

fn load_patch(ws: &Workspace, pack: &PackWorkspace) -> DraftResult<PatchSet> {
    let patch: PatchSet = crate::contracts::read_persisted(
        &ws.layout.pack_workspace_dir(&pack.id).join("patch.json"),
    )?;
    let mut canonical = patch.clone();
    canonical.patch_graph_hash.clear();
    if patch.patch_graph_hash != hash_json(&canonical)? {
        return Err(DraftError::new(
            DraftErrorKind::CorruptData,
            format!("pack {} patch graph digest mismatch", pack.id),
        ));
    }
    Ok(patch)
}

fn load_evidence(ws: &Workspace, pack: &PackWorkspace) -> DraftResult<Evidence> {
    crate::contracts::read_persisted(&ws.layout.pack_workspace_dir(&pack.id).join("evidence.json"))
}

fn pack_has_path_conflicts(
    ws: &Workspace,
    pack: &PackWorkspace,
    patch: &PatchSet,
) -> DraftResult<bool> {
    let paths: BTreeSet<String> = patch
        .files
        .iter()
        .map(|f| f.path.as_str().to_string())
        .collect();
    if paths.is_empty() {
        return Ok(false);
    }
    if !ws.layout.pack_workspaces_dir().exists() {
        return Ok(false);
    }
    for entry in fs::read_dir(ws.layout.pack_workspaces_dir())? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let manifest = path.join("staging.json");
        if !manifest.exists() {
            continue;
        }
        let other: PackWorkspace = crate::contracts::read_persisted(&manifest)?;
        let lifecycle = pack_lifecycle(ws, &other.id)?;
        if other.id == pack.id
            || matches!(
                lifecycle,
                PackLifecycle::Submitted | PackLifecycle::Rejected
            )
        {
            continue;
        }
        let other_patch = load_patch(ws, &other)?;
        if other_patch
            .files
            .iter()
            .any(|f| paths.contains(f.path.as_str()))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn patch_changed_lines(patch: &PatchSet) -> u64 {
    patch
        .files
        .iter()
        .flat_map(|file| file.hunks.iter())
        .map(|hunk| u64::from(hunk.old_lines.max(hunk.new_lines)))
        .sum()
}

fn patch_zones(patch: &PatchSet) -> usize {
    patch
        .files
        .iter()
        .filter_map(|file| file.path.as_str().split('/').next())
        .filter(|zone| !zone.is_empty())
        .collect::<BTreeSet<_>>()
        .len()
}

fn hooks_submit_ready(ws: &Workspace) -> DraftResult<bool> {
    let cfg = ResolvedConfig::load(ws)?;
    let hooks = cfg
        .submit_hooks(SubmitHookPhase::Before)
        .into_iter()
        .chain(cfg.submit_hooks(SubmitHookPhase::After));
    Ok(hooks
        .filter(|hook| hook.enabled)
        .all(|hook| !hook.command.trim().is_empty()))
}

fn ensure_workspace_matches_hash(
    ws: &Workspace,
    expected_hash: &str,
    action: &str,
    suggestion: &str,
) -> DraftResult<()> {
    let current_hash = crate::workspace::source_view::workspace_hash(&ws.root)?;
    if current_hash == expected_hash {
        return Ok(());
    }
    Err(DraftError::new(
        DraftErrorKind::DirtyWorkspace,
        format!("workspace has Draft-visible edits that are not part of the {action} baseline"),
    )
    .with_context(format!(
        "expected workspace hash {expected_hash}, found {current_hash}"
    ))
    .with_suggestion(suggestion))
}

fn ensure_pack_workspace_matches_target(
    ws: &Workspace,
    pack: &PackWorkspace,
    action: &str,
    suggestion: &str,
) -> DraftResult<()> {
    let store =
        crate::pack::PackStore::new(crate::workspace::layout::DraftLayout::for_root(&ws.root));
    let Some(location) = store.locate(pack.id.as_str()) else {
        return Ok(());
    };
    let revision = store.current_revision_in(location, pack.id.as_str())?;
    ensure_workspace_matches_hash(ws, &revision.target_digest, action, suggestion)
}

fn decision_dirty_action(kind: DecisionKind) -> &'static str {
    match kind {
        DecisionKind::Approve => "approve",
        DecisionKind::Reject => "reject",
        DecisionKind::NeedsChanges => "request changes",
        DecisionKind::AcceptFile => "accept file",
        DecisionKind::RejectFile => "reject file",
        DecisionKind::AcceptCandidate => "accept candidate",
    }
}

fn insert_inbox(
    by_id: &mut BTreeMap<String, crate::review::workflow::InboxItem>,
    id: String,
    kind: &str,
    subject_id: String,
    status: &str,
    summary: String,
    next_action: String,
) {
    by_id
        .entry(id.clone())
        .or_insert(crate::review::workflow::InboxItem {
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

fn submit_readiness(
    ws: &Workspace,
    pack: &PackWorkspace,
    patch: &PatchSet,
    policy: &crate::review::policy::Policy,
) -> DraftResult<SubmitReadinessReport> {
    let mut blockers = Vec::new();
    let verification = latest_current_passed_verification(ws, pack)?;
    if verification.is_none() {
        blockers.push("current passed verification receipt is required before submit".to_string());
    }
    let review =
        latest_completed_review_after(ws, pack, verification.as_ref().map(|v| v.created_at))?;
    let approval = latest_human_approval_after(ws, pack, review.as_ref().map(|r| r.created_at))?;
    let lifecycle = pack_lifecycle(ws, &pack.id)?;
    if policy.require_approval_for_submit {
        if review.is_none() {
            blockers.push("current review receipt is required before submit".to_string());
        }
        if approval.is_none() || lifecycle != PackLifecycle::Approved {
            blockers
                .push("human approval is required after current review before submit".to_string());
        }
    }
    let workflow = crate::review::workflow::WorkflowStore::for_root(&ws.root);
    let mut paths_to_check = Vec::new();
    for file in &patch.files {
        paths_to_check.push(&file.path);
        if let Some(old_path) = &file.old_path {
            paths_to_check.push(old_path);
        }
    }
    let protected_violations =
        crate::workspace::protected::violations(&ws.root, paths_to_check.clone())?;
    let no_protected = protected_violations.is_empty();
    let no_forbidden = paths_to_check
        .iter()
        .all(|path| !path.as_str().starts_with(".draft/") && path.as_str() != ".draft");
    let stable_store = crate::workspace::stable::StableHeadStore::new(
        crate::workspace::layout::DraftLayout::for_root(&ws.root),
    );
    let base_valid = if stable_store.exists() {
        stable_store.read()?;
        true
    } else {
        false
    };
    let no_conflicts = !pack_has_path_conflicts(ws, pack, patch)?;
    let hooks_ready = hooks_submit_ready(ws)?;
    let patch_paths = paths_to_check
        .iter()
        .map(|path| path.as_str().to_string())
        .collect::<Vec<_>>();
    let decision_authors = workflow
        .decisions()?
        .into_iter()
        .filter(|decision| decision.pack_id.as_deref() == Some(pack.id.as_str()))
        .map(|decision| decision.author)
        .collect::<Vec<_>>();
    let ownership =
        crate::workspace::ownership::evaluate(&ws.root, &patch_paths, &decision_authors)?;
    if ownership.missing_owner_review {
        blockers.push(format!(
            "owner_review: owner review required for {}",
            ownership.domains.join(", ")
        ));
    }
    let changed_lines = patch_changed_lines(patch);
    let zones = patch_zones(patch);
    let unresolved_warnings = load_evidence(ws, pack)?.warnings.len();
    let reviewability = crate::review::reviewability::evaluate(
        &crate::review::reviewability::budget(&ws.root)?,
        patch.files.len(),
        changed_lines,
        zones,
        ownership.domains.len(),
        unresolved_warnings,
    );
    if reviewability.status == "poor" {
        blockers.push(format!(
            "reviewability: {}",
            reviewability.reasons.join("; ")
        ));
    }
    let rollback_available = pack.base_snapshot_id.as_str() != "chk_empty"
        || latest_snapshot(ws)?.is_some()
        || crate::workspace::stable::StableHeadStore::new(
            crate::workspace::layout::DraftLayout::for_root(&ws.root),
        )
        .exists();
    let view = workflow.readiness(
        pack.id.as_str(),
        true,
        base_valid,
        no_conflicts,
        no_protected,
        no_forbidden,
        hooks_ready,
        rollback_available,
    )?;
    for check in view.checks.into_iter().filter(|check| !check.passed) {
        if check.id == "protected_files" {
            for violation in &protected_violations {
                blockers.push(format!(
                    "{}: {} matched {} ({})",
                    check.id, violation.path, violation.pattern, violation.reason
                ));
            }
            if protected_violations.is_empty() {
                blockers.push(format!("{}: {}", check.id, check.reason));
            }
        } else {
            blockers.push(format!("{}: {}", check.id, check.reason));
        }
    }
    Ok(SubmitReadinessReport {
        ok: blockers.is_empty(),
        blockers,
        ownership: Some(ownership),
        reviewability: Some(reviewability),
        verification_receipt_id: verification.map(|v| v.id),
        review_receipt_id: review.map(|r| r.id),
        approval_ref: approval,
    })
}

/// Resolve the effective policy for a workspace: project `.draft/policy.toml`
/// over the global default policy over the built-in safe default. Fails closed
/// on an unreadable or malformed policy file.
fn effective_policy(ws: &Workspace) -> DraftResult<crate::review::policy::Policy> {
    let project = crate::workspace::layout::DraftLayout::for_root(&ws.root).policy_toml();
    let global = Some(crate::workspace::home::DraftGlobalStore::locate()?.default_policy_toml());
    crate::review::policy::Policy::resolve(Some(&project), global.as_deref())
}

fn validate_canonical_submit_gate(ws: &Workspace, pack_id: &str) -> DraftResult<()> {
    let policy = effective_policy(ws)?;
    let paths = crate::workspace::layout::DraftLayout::for_root(&ws.root);
    let store = crate::pack::PackStore::new(paths.clone());
    store.read_manifest(pack_id)?;
    let revision = store.current_revision_in(crate::pack::PackLocation::Store, pack_id)?;
    let lifecycle = store.read_lifecycle_in(crate::pack::PackLocation::Store, pack_id)?;
    let verification_path = paths.pack_verify(pack_id);
    if !verification_path.exists() {
        return Err(DraftError::new(
            DraftErrorKind::VerificationFailed,
            "canonical verification evidence is required before submit",
        )
        .with_suggestion("run `draft verify <pck_id>` before submit"));
    }
    let evidence: crate::review::verification::VerifyEvidence =
        crate::contracts::read_persisted(&verification_path)?;
    evidence.validate_binding(&revision)?;
    if !evidence.passed() {
        return Err(DraftError::new(
            DraftErrorKind::VerificationFailed,
            "canonical verification receipt is required before submit",
        ));
    }
    if policy.require_approval_for_submit
        && lifecycle.lifecycle != crate::pack::lifecycle::PackLifecycle::Approved
    {
        return Err(DraftError::new(
            DraftErrorKind::ReviewRequired,
            "canonical approval receipt is required before submit",
        ));
    }
    if store.is_quarantined(pack_id) {
        return Err(DraftError::new(
            DraftErrorKind::ReviewRequired,
            "imported packs must be locally verified and approved before submit",
        ));
    }
    validate_canonical_risk_gate(
        &policy,
        &paths,
        pack_id,
        &revision,
        lifecycle.lifecycle == crate::pack::lifecycle::PackLifecycle::Approved,
    )?;
    if policy.require_reverify_on_workspace_change {
        let current_hash = crate::workspace::source_view::workspace_hash(&ws.root)?;
        if evidence
            .verification_key
            .as_ref()
            .map(|key| key.workspace_hash.as_str())
            != Some(current_hash.as_str())
        {
            return Err(DraftError::new(
                DraftErrorKind::VerificationFailed,
                "workspace content changed after canonical verification",
            )
            .with_suggestion("run `draft verify <pck_id>` again before submit"));
        }
    }
    let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
    let verification = ledger.verify_all()?;
    if !verification.all_ok {
        return Err(DraftError::new(
            DraftErrorKind::OperationLogCorrupt,
            "canonical event, receipt, or transparency ledger failed verification",
        )
        .with_suggestion("run `draft receipt verify --all` or `draft doctor`"));
    }
    Ok(())
}

/// Enforce the canonical risk report (`risk.json`) against the effective
/// policy: an unresolved critical risk blocks submit, and high/critical risk
/// requires explicit approval. A missing risk report fails closed when the
/// policy blocks on critical risk.
fn validate_canonical_risk_gate(
    policy: &crate::review::policy::Policy,
    paths: &crate::workspace::layout::DraftLayout,
    pack_id: &str,
    revision: &crate::pack::PackRevision,
    approved: bool,
) -> DraftResult<()> {
    let risk_path = paths.pack_risk(pack_id);
    if !risk_path.exists() {
        if policy.block_on_critical_risk {
            return Err(DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                "no canonical risk report exists for this pack",
            )
            .with_suggestion("run `draft verify <pck_id>` before submit"));
        }
        return Ok(());
    }
    let risk: crate::review::risk::RiskReport = crate::contracts::read_persisted(&risk_path)?;
    risk.validate_binding(revision)?;
    if policy.block_on_critical_risk && risk.risk_level == crate::review::risk::RiskLevel::Critical
    {
        return Err(DraftError::new(
            DraftErrorKind::RiskPolicyBlocked,
            "unresolved critical risk blocks submit",
        )
        .with_suggestion("resolve the required actions in risk.json and re-verify"));
    }
    if policy.require_approval_on_high_risk
        && matches!(
            risk.risk_level,
            crate::review::risk::RiskLevel::High | crate::review::risk::RiskLevel::Critical
        )
        && !approved
    {
        return Err(DraftError::new(
            DraftErrorKind::ReviewRequired,
            "high-risk pack requires explicit approval before submit",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ReceiptRef {
    id: String,
    created_at: DateTime<Utc>,
}

fn latest_current_passed_verification(
    ws: &Workspace,
    pack: &PackWorkspace,
) -> DraftResult<Option<ReceiptRef>> {
    let store = crate::pack::PackStore::new(ws.layout.clone());
    let location = store.locate(pack.id.as_str()).ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!("pack staging state {} has no canonical pack", pack.id),
        )
    })?;
    let revision = store.current_revision_in(location, pack.id.as_str())?;
    let evidence_path = store
        .dir_for(location, pack.id.as_str())
        .join("verify.json");
    if !evidence_path.exists() {
        return Ok(None);
    }
    let evidence: crate::review::verification::VerifyEvidence =
        crate::contracts::read_persisted(&evidence_path)?;
    evidence.validate_binding(&revision)?;
    if !evidence.passed() {
        return Ok(None);
    }

    let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
    let record = crate::review::workflow::WorkflowStore::for_root(&ws.root)
        .evidence()?
        .into_iter()
        .filter(|record| {
            record.pack_id.as_deref() == Some(pack.id.as_str())
                && record.kind == "verification"
                && record.state == crate::review::workflow::EvidenceState::Fresh
                && record.result.get("result_hash").and_then(Value::as_str)
                    == Some(evidence.result_hash.as_str())
        })
        .max_by_key(|record| record.produced_at)
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                "canonical verification evidence has no linked workflow evidence record",
            )
        })?;
    let receipt_id = record.receipt_id.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            "canonical verification evidence has no linked receipt",
        )
    })?;
    if !ledger.verify_receipt(&receipt_id)?.ok {
        return Err(DraftError::new(
            DraftErrorKind::CorruptData,
            "canonical verification receipt failed validation",
        ));
    }
    Ok(Some(ReceiptRef {
        id: receipt_id,
        created_at: record.produced_at,
    }))
}

fn latest_completed_review_after(
    ws: &Workspace,
    pack: &PackWorkspace,
    after: Option<DateTime<Utc>>,
) -> DraftResult<Option<ReceiptRef>> {
    let mut latest = None;
    let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
    let receipts = crate::trust::receipt::ReceiptStore::new(ws.layout.clone());
    let events =
        crate::trust::event::EventLog::workspace(ws.layout.clone(), ws.workspace_id.to_string())
            .read_all()?;
    for receipt_id in &pack.review_refs {
        let receipt = receipts.read(receipt_id)?;
        if receipt.event_type != "ReviewCompleted"
            || receipt.subject_id.as_deref() != Some(pack.id.as_str())
            || !ledger.verify_receipt(receipt_id)?.ok
        {
            continue;
        }
        let event = events
            .iter()
            .find(|event| event.event_hash == receipt.event_hash)
            .ok_or_else(|| {
                DraftError::new(DraftErrorKind::CorruptData, "receipt event is missing")
            })?;
        if event.metadata.get("status").and_then(Value::as_str) != Some("completed") {
            continue;
        }
        let created_at = DateTime::parse_from_rfc3339(&receipt.timestamp)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?
            .with_timezone(&Utc);
        if after.map(|minimum| created_at < minimum).unwrap_or(false) {
            continue;
        }
        if latest
            .as_ref()
            .map(|current: &ReceiptRef| created_at > current.created_at)
            .unwrap_or(true)
        {
            latest = Some(ReceiptRef {
                id: receipt_id.clone(),
                created_at,
            });
        }
    }
    Ok(latest)
}

fn latest_human_approval_after(
    ws: &Workspace,
    pack: &PackWorkspace,
    after: Option<DateTime<Utc>>,
) -> DraftResult<Option<String>> {
    let review = load_review_file(ws, &pack.id)?;
    Ok(review
        .decisions
        .into_iter()
        .filter(|decision| {
            decision.kind == DecisionKind::Approve
                && decision.actor.kind == ActorKind::Human
                && after
                    .map(|minimum| decision.created_at >= minimum)
                    .unwrap_or(true)
        })
        .max_by_key(|decision| decision.created_at)
        .map(|decision| decision.id.to_string()))
}

fn load_review_file(ws: &Workspace, id: &PackId) -> DraftResult<ReviewFile> {
    let path = ws.layout.pack_workspace_dir(id).join("review.json");
    if !path.exists() {
        return Ok(ReviewFile::default());
    }
    crate::contracts::read_persisted(&path)
}

fn save_review_file(ws: &Workspace, id: &PackId, file: &ReviewFile) -> DraftResult<()> {
    write_json(&ws.layout.pack_workspace_dir(id).join("review.json"), file)
}

fn build_review_units(
    ws: &Workspace,
    pack: &PackWorkspace,
    risk: Option<&RiskSummary>,
) -> DraftResult<Vec<ReviewUnit>> {
    let patch = load_patch(ws, pack)?;
    let hotspots: HashSet<_> = risk
        .map(|summary| summary.hotspots.iter().cloned().collect())
        .unwrap_or_default();
    Ok(patch
        .files
        .into_iter()
        .enumerate()
        .map(|(idx, file)| {
            let risk_contribution = if hotspots.contains(&file.path) { 10 } else { 1 };
            ReviewUnit {
                id: format!("rvu_{:04}", idx + 1),
                path: file.path,
                change_kind: format!("{:?}", file.change_kind),
                risk_contribution,
                evidence_refs: pack.evidence_refs.clone(),
                provenance_refs: pack
                    .task_id
                    .as_ref()
                    .map(|id| vec![id.to_string()])
                    .unwrap_or_default(),
                status: "pending".to_string(),
            }
        })
        .collect())
}

fn split_patch(source: &PackWorkspace, files: Vec<FilePatch>) -> DraftResult<PatchSet> {
    let mut patch = PatchSet {
        schema_version: current_version(ContractId::PatchSet),
        id: PatchSetId::generate(),
        base_snapshot_id: source.base_snapshot_id.clone(),
        result_snapshot_id: source.result_snapshot_id.clone(),
        files,
        patch_graph_hash: String::new(),
    };
    patch.patch_graph_hash = hash_json(&patch)?;
    Ok(patch)
}

fn split_evidence(pack: &PackWorkspace, patch: &PatchSet, warning: &str) -> Evidence {
    Evidence {
        schema_version: current_version(ContractId::PackEvidence),
        id: EvidenceId::generate(),
        pack_id: pack.id.clone(),
        command_logs: vec![],
        files_touched: patch.files.iter().map(|f| f.path.clone()).collect(),
        generated_diff_ref: None,
        test_results: vec![],
        lint_results: vec![],
        risk_summary_ref: None,
        agent_plan_ref: None,
        agent_transcript_ref: None,
        warnings: vec![warning.to_string()],
        created_at: now(),
    }
}

fn write_receipt(ws: &Workspace, receipt: &ActionReceiptDraft) -> DraftResult<()> {
    let kind = match receipt.kind.as_str() {
        "checkpoint" => crate::trust::event::EventKind::CheckpointCreated,
        "verification" => crate::trust::event::EventKind::PackVerified,
        "risk" => crate::trust::event::EventKind::RiskAssessed,
        "review" => crate::trust::event::EventKind::ReviewCompleted,
        "approval" => crate::trust::event::EventKind::PackApproved,
        "compose" => crate::trust::event::EventKind::PackComposed,
        "disperse" => crate::trust::event::EventKind::PackDispersed,
        "hook" if receipt.status == "failed" => crate::trust::event::EventKind::SubmitHookFailed,
        "hook" => crate::trust::event::EventKind::SubmitHookCompleted,
        other => {
            return Err(DraftError::invalid_config(format!(
                "unsupported action receipt kind '{other}'"
            )))
        }
    };
    crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?
        .record_with_receipt_id(
            kind,
            receipt.subject_id.clone(),
            None,
            crate::workspace::source_view::workspace_hash(&ws.root)?,
            serde_json::json!({
                "status": receipt.status,
                "payload": redact_value(receipt.payload.clone()),
                "rollback_target": receipt.rollback_target,
            }),
            receipt.id.to_string(),
        )?;
    Ok(())
}

fn write_submit_record(ws: &Workspace, receipt: &SubmitRecord) -> DraftResult<()> {
    let mut receipt = receipt.clone();
    collect_object_refs_into_vec(
        &serde_json::to_value(&receipt.hook_results).expect("Draft-owned records must serialize"),
        &mut receipt.object_refs,
    );
    receipt.object_refs.sort();
    receipt.object_refs.dedup();
    receipt.failure_reason = receipt
        .failure_reason
        .as_ref()
        .map(|reason| redact_secrets(reason));
    receipt.record_digest.clear();
    receipt.record_digest = hash_json(&receipt)?;
    let kind = if receipt.overall_status == SubmitOverallStatus::Failed {
        crate::trust::event::EventKind::SubmitCompleted
    } else {
        crate::trust::event::EventKind::PackSubmitted
    };
    let rollback_snapshot_id = if ws
        .layout
        .pack_workspace_dir(&receipt.pack_id)
        .join("staging.json")
        .exists()
    {
        Some(
            load_pack(ws, receipt.pack_id.as_str())?
                .base_snapshot_id
                .to_string(),
        )
    } else {
        None
    };
    crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?
        .record_with_receipt_id(
            kind,
            Some(receipt.pack_id.to_string()),
            None,
            crate::workspace::source_view::workspace_hash(&ws.root)?,
            redact_value(serde_json::json!({
                "submit": receipt,
                "rollback_snapshot_id": rollback_snapshot_id,
            })),
            receipt.id.to_string(),
        )?;
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
    conn.execute("DELETE FROM packs", []).map_err(sql_err)?;
    conn.execute("DELETE FROM receipts", []).map_err(sql_err)?;
    conn.execute("DELETE FROM snapshots", []).map_err(sql_err)?;

    let events = ws.events()?.read_all()?;
    for event in &events {
        conn.execute(
            "INSERT INTO events (id, event_type, subject_id, time, event_hash) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.event_id,
                event.event_type,
                event.subject_id,
                event.time,
                event.event_hash
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

    let packs = App::new().pack_list(&ws.root)?;
    for pack in &packs {
        let lifecycle = pack_lifecycle(ws, &pack.id)?;
        conn.execute(
            "INSERT INTO packs (id, name, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                pack.id.to_string(),
                pack.name.clone().unwrap_or_default(),
                format!("{lifecycle:?}"),
                pack.created_at.to_rfc3339(),
                pack.updated_at.to_rfc3339()
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
            "INSERT INTO snapshots (id, manifest_hash, created_at, file_count) VALUES (?1, ?2, ?3, ?4)",
            params![
                snapshot.id.to_string(),
                snapshot.manifest_hash,
                snapshot.created_at.to_rfc3339(),
                snapshot.files.len() as i64
            ],
        )
        .map_err(sql_err)?;
    }

    Ok(IndexReport {
        path: ws.layout.index_file().display().to_string(),
        events: events.len(),
        tasks: tasks.len(),
        executions: executions.len(),
        packs: packs.len(),
        receipts: receipts.len(),
        snapshots: snapshots.len(),
    })
}

fn rebuild_index_for_layout(layout: &DraftLayout) -> DraftResult<()> {
    let conn = open_index(layout)?;
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
        CREATE TABLE IF NOT EXISTS packs (
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
            manifest_hash TEXT NOT NULL,
            created_at TEXT NOT NULL,
            file_count INTEGER NOT NULL
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

fn failed_submit(
    ws: &Workspace,
    pack: &PackWorkspace,
    started: DateTime<Utc>,
    reason: &str,
) -> DraftResult<SubmitRecord> {
    let store = ObjectStore::new(ws.layout.clone());
    let mut receipt = SubmitRecord {
        schema_version: current_version(ContractId::SubmitRecord),
        id: ReceiptId::generate(),
        pack_id: pack.id.clone(),
        actor_id: resolve_actor(&ws.layout.draft_dir)?.id,
        native_submit_status: NativeSubmitStatus::Failed,
        hook_status: HookStatus::Skipped,
        overall_status: SubmitOverallStatus::Failed,
        message_ref: store.put_bytes(b"")?,
        hook_results: Vec::new(),
        hook_receipt_refs: Vec::new(),
        object_refs: Vec::new(),
        event_refs: Vec::new(),
        risk_level: "unknown".to_string(),
        risk_receipt_id: None,
        started_at: started,
        ended_at: now(),
        record_digest: String::new(),
        failure_reason: Some(reason.to_string()),
    };
    receipt.record_digest = hash_json(&receipt)?;
    write_submit_record(ws, &receipt)?;
    Ok(receipt)
}

fn render_message(
    ws: &Workspace,
    cfg: &ResolvedConfig,
    pack: &PackWorkspace,
    patch: &PatchSet,
    receipt_id: &ReceiptId,
) -> DraftResult<String> {
    let title = pack.name.clone().unwrap_or_else(|| pack.id.to_string());
    let mut values = BTreeMap::new();
    values.insert("message".to_string(), title.clone());
    values.insert("title".to_string(), title);
    values.insert("description".to_string(), String::new());
    values.insert(
        "task_id".to_string(),
        pack.task_id
            .as_ref()
            .map(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    );
    values.insert(
        "execution_id".to_string(),
        pack.execution_id
            .as_ref()
            .map(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    );
    values.insert("pack_id".to_string(), pack.id.to_string());
    values.insert("receipt_id".to_string(), receipt_id.to_string());
    let actor_id = resolve_actor(&ws.layout.draft_dir)?.id.to_string();
    values.insert("actor_name".to_string(), actor_id);
    values.insert("timestamp".to_string(), now().to_rfc3339());
    values.insert(
        "verified".to_string(),
        (!pack.verification_refs.is_empty()).to_string(),
    );
    values.insert("risk_level".to_string(), "unknown".to_string());
    values.insert("files_changed".to_string(), patch.files.len().to_string());
    Ok(interpolate_lenient(&cfg.submit_message_template, &values))
}

/// The fraction (0.0–1.0) of a candidate's packs that were later rolled back —
/// a risk signal, never a verdict on its own. Returns 0.0 when the candidate
/// has produced no packs. Live values stay 0.0 until candidate attribution is
/// recorded on manifests.
fn candidate_rollback_rate(
    events: &[crate::trust::event::EventRecord],
    manifests: &[crate::pack::PackManifest],
    candidate: &str,
) -> f64 {
    let candidate_packs: Vec<&str> = manifests
        .iter()
        .filter(|m| m.candidate_id.as_deref() == Some(candidate))
        .map(|m| m.pack_id.as_str())
        .collect();
    if candidate_packs.is_empty() {
        return 0.0;
    }
    let rolled_back = candidate_packs
        .iter()
        .filter(|pack_id| {
            events.iter().any(|e| {
                e.event_type == "RollbackPerformed" && e.subject_id.as_deref() == Some(**pack_id)
            })
        })
        .count();
    rolled_back as f64 / candidate_packs.len() as f64
}

/// A fully validated import application plan: every write has its bytes in
/// hand and every precondition was checked before anything touches the
/// workspace.
struct ImportApplyPlan {
    writes: Vec<(PathBuf, Vec<u8>)>,
    deletes: Vec<PathBuf>,
}

/// Validate that an imported patch applies cleanly to the current workspace
/// and assemble the plan. Fail closed on the first conflict: a file whose
/// current content does not match the patch's recorded `old_hash`, a missing
/// content object, or an unsafe path. Already-applied entries are skipped so
/// the apply is idempotent.
fn plan_import_apply(
    ws: &Workspace,
    pack_dir: &Path,
    patch: &PatchSet,
) -> DraftResult<ImportApplyPlan> {
    let current_hash = |p: &Path| -> DraftResult<Option<String>> {
        if !p.is_file() {
            return Ok(None);
        }
        Ok(Some(format!("b3:{}", blake3_hex(&fs::read(p)?))))
    };
    let conflict = |path: &WorkspacePath, why: &str| {
        DraftError::new(
            DraftErrorKind::SubmitFailed,
            format!("cannot apply imported change to '{path}': {why}"),
        )
        .with_suggestion("resolve the local conflict, then re-verify and submit again")
    };

    let mut writes = Vec::new();
    let mut deletes = Vec::new();
    for f in &patch.files {
        if is_draft_path(f.path.as_str()) {
            continue;
        }
        let dest = safe_workspace_dest(&ws.root, &f.path)?;
        let current = current_hash(&dest)?;
        match &f.change_kind {
            FileChangeKind::Added => {
                let new_hash = f
                    .new_hash
                    .as_ref()
                    .ok_or_else(|| conflict(&f.path, "added file has no recorded content hash"))?;
                if current.as_deref() == Some(new_hash.as_str()) {
                    continue; // already applied
                }
                if current.is_some() {
                    return Err(conflict(
                        &f.path,
                        "a different local file already exists at this path",
                    ));
                }
                writes.push((dest, read_imported_object(pack_dir, new_hash)?));
            }
            FileChangeKind::Modified
            | FileChangeKind::TypeChanged
            | FileChangeKind::PermissionChanged => {
                let new_hash = f.new_hash.as_ref().ok_or_else(|| {
                    conflict(&f.path, "modified file has no recorded content hash")
                })?;
                if current.as_deref() == Some(new_hash.as_str()) {
                    continue; // already applied
                }
                if current.as_deref() != f.old_hash.as_deref() {
                    return Err(conflict(
                        &f.path,
                        "local content differs from the change's base version",
                    ));
                }
                writes.push((dest, read_imported_object(pack_dir, new_hash)?));
            }
            FileChangeKind::Deleted => {
                match current {
                    None => continue, // already applied
                    Some(h) if Some(h.as_str()) == f.old_hash.as_deref() => deletes.push(dest),
                    Some(_) => {
                        return Err(conflict(
                            &f.path,
                            "local content differs from the change's base version",
                        ))
                    }
                }
            }
            FileChangeKind::Renamed { from } => {
                let new_hash = f.new_hash.as_ref().ok_or_else(|| {
                    conflict(&f.path, "renamed file has no recorded content hash")
                })?;
                let source = safe_workspace_dest(&ws.root, from)?;
                let source_hash = current_hash(&source)?;
                if current.as_deref() == Some(new_hash.as_str()) && source_hash.is_none() {
                    continue; // already applied
                }
                if source_hash.as_deref() != f.old_hash.as_deref() {
                    return Err(conflict(
                        from,
                        "rename source differs from the change's base version",
                    ));
                }
                if current.is_some() {
                    return Err(conflict(
                        &f.path,
                        "a different local file already exists at the rename target",
                    ));
                }
                writes.push((dest, read_imported_object(pack_dir, new_hash)?));
                deletes.push(source);
            }
        }
    }
    Ok(ImportApplyPlan { writes, deletes })
}

/// Read a content object embedded in an imported pack directory, re-checking
/// its content address (fail closed on post-import tampering or absence).
fn read_imported_object(pack_dir: &Path, object_ref: &str) -> DraftResult<Vec<u8>> {
    let hex = object_ref.strip_prefix("b3:").ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::VerificationFailed,
            format!("imported pack references unsupported object '{object_ref}'"),
        )
    })?;
    let path = pack_dir.join("objects").join(hex);
    if !path.exists() {
        return Err(DraftError::new(
            DraftErrorKind::VerificationFailed,
            format!("imported pack is missing content object '{hex}'"),
        )
        .with_suggestion("re-export the pack with the current Draft release"));
    }
    let bytes = fs::read(&path)?;
    if blake3_hex(&bytes) != hex {
        return Err(DraftError::new(
            DraftErrorKind::VerificationFailed,
            format!("imported content object '{hex}' failed its content-address check"),
        ));
    }
    Ok(bytes)
}

fn resolve_snapshot_reference(ws: &Workspace, reference: &str) -> DraftResult<Snapshot> {
    if reference.starts_with("chk_") {
        validate_checkpoint_id(reference)?;
        return load_snapshot(ws, &SnapshotId::new(reference));
    }
    if reference.starts_with("pck_") {
        validate_pack_id(reference)?;
        let staging = ws.layout.pack_workspace_dir(PackId::new(reference));
        if !staging.join("staging.json").exists()
            && crate::pack::PackStore::new(ws.layout.clone()).exists(reference)
        {
            let receipt = crate::trust::receipt::ReceiptStore::new(ws.layout.clone())
                .list()?
                .into_iter()
                .rev()
                .find(|receipt| {
                    receipt.subject_id.as_deref() == Some(reference)
                        && receipt.event_type == "PackSubmitted"
                })
                .map(|receipt| receipt.receipt_id)
                .unwrap_or_else(|| "the signed submit receipt".to_string());
            return Err(DraftError::invalid_config(format!(
                "submitted pack '{reference}' is immutable and its mutable staging snapshot was disposed; use rollback receipt '{receipt}'"
            )));
        }
        let pack = load_pack(ws, reference)?;
        return load_snapshot(ws, &pack.base_snapshot_id);
    }
    if reference.starts_with("rcp_") {
        validate_receipt_id(reference)?;
        let receipt_path = ws.layout.receipts_dir().join(format!("{reference}.json"));
        if !receipt_path.exists() {
            return Err(DraftError::not_found(format!(
                "unknown rollback receipt '{reference}'"
            )));
        }
        let record: crate::trust::receipt::ReceiptRecord =
            crate::contracts::read_persisted(&receipt_path)?;
        return resolve_canonical_receipt_target(ws, &record);
    }
    Err(DraftError::invalid_config(format!(
        "rollback reference '{reference}' must start with chk_, pck_, or rcp_"
    )))
}

/// Canonical receipt event types whose subject is a meaningful local rollback
/// anchor. Deliberately excluded: PackImported (no local snapshot precedes
/// it), PackComposed (no base snapshot), PackExported (no state change), and
/// RollbackPerformed (aliases the original reference).
const ROLLBACK_ELIGIBLE_EVENTS: &[&str] = &[
    "CheckpointCreated",
    "PackCreated",
    "PackVerified",
    "PackApproved",
    "PackSubmitted",
];

/// Resolve a canonical signed receipt to a rollback snapshot via its subject.
/// The receipt must verify (fail closed: an unverifiable receipt is not a
/// trustworthy rollback anchor) and its event type must be rollback-eligible.
fn resolve_canonical_receipt_target(
    ws: &Workspace,
    record: &crate::trust::receipt::ReceiptRecord,
) -> DraftResult<Snapshot> {
    let ledger = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?;
    let verification = ledger.verify_receipt(&record.receipt_id)?;
    if !verification.ok {
        return Err(DraftError::new(
            DraftErrorKind::OperationLogCorrupt,
            format!(
                "receipt '{}' failed verification and cannot anchor a rollback",
                record.receipt_id
            ),
        )
        .with_suggestion("run `draft receipt verify --all` or `draft doctor`"));
    }
    if !ROLLBACK_ELIGIBLE_EVENTS.contains(&record.event_type.as_str()) {
        return Err(DraftError::invalid_config(format!(
            "receipt '{}' ({}) is not rollback-eligible",
            record.receipt_id, record.event_type
        )));
    }
    let event =
        crate::trust::event::EventLog::workspace(ws.layout.clone(), ws.workspace_id.to_string())
            .read_all()?
            .into_iter()
            .find(|event| event.event_hash == record.event_hash)
            .ok_or_else(|| {
                DraftError::new(DraftErrorKind::CorruptData, "receipt event is missing")
            })?;
    if let Some(snapshot_id) = event
        .metadata
        .get("rollback_snapshot_id")
        .and_then(Value::as_str)
    {
        if snapshot_id != "chk_empty" {
            validate_checkpoint_id(snapshot_id)?;
        }
        return load_snapshot(ws, &SnapshotId::new(snapshot_id));
    }
    match record.subject_id.as_deref() {
        Some(subject) if subject.starts_with("chk_") => {
            validate_checkpoint_id(subject)?;
            load_snapshot(ws, &SnapshotId::new(subject))
        }
        Some(subject) if subject.starts_with("pck_") => {
            validate_pack_id(subject)?;
            let pack = load_pack(ws, subject).map_err(|_| {
                DraftError::invalid_config(format!(
                    "pack '{subject}' has no local snapshot to roll back to"
                ))
            })?;
            load_snapshot(ws, &pack.base_snapshot_id)
        }
        other => Err(DraftError::invalid_config(format!(
            "receipt '{}' subject '{}' is not a rollback target",
            record.receipt_id,
            other.unwrap_or("<none>")
        ))),
    }
}

fn validate_checkpoint_id(id: &str) -> DraftResult<()> {
    validate_prefixed_id(id, "chk_", "checkpoint")
}

fn validate_pack_id(id: &str) -> DraftResult<()> {
    validate_prefixed_id(id, "pck_", "pack")
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

fn ensure_pack_not_locked(ws: &Workspace, pack: &PackWorkspace) -> DraftResult<()> {
    let lock = ws
        .layout
        .pack_workspace_dir(&pack.id)
        .join("review.lock.json");
    if lock.exists() {
        return Err(DraftError::new(
            DraftErrorKind::ReviewRequired,
            format!(
                "Pack {} is locked for review; approve or reject it before mutating it",
                pack.id
            ),
        ));
    }
    Ok(())
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
    let index = read_object_pack_index(&ws.layout)?;
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
        entries.push(ObjectPackEntry {
            object_ref,
            compressed_hex: hex_encode(&compressed),
        });
        loose_paths.push(path);
    }
    if entries.is_empty() {
        return Ok(0);
    }
    ensure_dir(&ws.layout.object_packs_dir())?;
    let pack_id = format!("opk_{}", uuid::Uuid::new_v4().simple());
    let pack_name = format!("{pack_id}.json.zst");
    let pack = ObjectPack {
        schema_version: current_version(ContractId::ObjectPack),
        id: pack_id,
        created_at: now(),
        entries,
    };
    let json = serde_json::to_vec(&pack).map_err(json_err)?;
    let compressed = zstd::stream::encode_all(json.as_slice(), 3)
        .map_err(|e| DraftError::storage(format!("object pack compression failed: {e}")))?;
    write_atomic(&ws.layout.object_packs_dir().join(&pack_name), &compressed)?;

    let mut index = read_object_pack_index(&ws.layout)?;
    for entry in &pack.entries {
        index
            .objects
            .insert(entry.object_ref.clone(), pack_name.clone());
    }
    write_object_pack_index(&ws.layout, &index)?;
    let store = ObjectStore::new(ws.layout.clone());
    for entry in &pack.entries {
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
    let verification = crate::trust::ledger::TrustLedger::open(&ws.root, ws.workspace_id.as_str())?
        .verify_all()?;
    let mut errors = Vec::new();
    if !verification.event_chain_ok {
        errors.push("event chain verification failed".into());
    }
    if !verification.transparency_ok {
        errors.push("receipt transparency chain verification failed".into());
    }
    for receipt in verification
        .receipts
        .into_iter()
        .filter(|receipt| !receipt.ok)
    {
        let failed = receipt
            .checks
            .into_iter()
            .filter(|check| !check.ok)
            .map(|check| check.name)
            .collect::<Vec<_>>()
            .join(", ");
        errors.push(format!("{}: {failed}", receipt.receipt_id));
    }
    Ok(errors)
}

fn verify_draft_hard_exclusion(ws: &Workspace) -> DraftResult<Vec<String>> {
    let mut errors = Vec::new();
    if !ws.layout.pack_workspaces_dir().exists() {
        return Ok(errors);
    }
    for entry in fs::read_dir(ws.layout.pack_workspaces_dir())? {
        let manifest = entry?.path().join("staging.json");
        if !manifest.exists() {
            continue;
        }
        let pack: PackWorkspace = crate::contracts::read_persisted(&manifest)?;
        pack.validate()?;
        let patch = load_patch(ws, &pack)?;
        for file in patch.files {
            if is_draft_path(file.path.as_str())
                || file
                    .old_path
                    .as_ref()
                    .map(|p| is_draft_path(p.as_str()))
                    .unwrap_or(false)
            {
                errors.push(format!(
                    "{} includes Draft metadata path {}",
                    pack.id, file.path
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

fn collect_object_refs_into_vec(value: &Value, refs: &mut Vec<String>) {
    let mut set: HashSet<String> = refs.iter().cloned().collect();
    collect_object_refs_from_value(value, &mut set);
    *refs = set.into_iter().collect();
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

fn count_files(path: &Path) -> DraftResult<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0;
    for entry in walkdir::WalkDir::new(path) {
        let entry = entry.map_err(|e| DraftError::storage(e.to_string()))?;
        if entry.file_type().is_file() {
            total += 1;
        }
    }
    Ok(total)
}

fn empty_patch_for_pack(pack: &PackWorkspace) -> DraftResult<PatchSet> {
    let mut patch = PatchSet {
        schema_version: current_version(ContractId::PatchSet),
        id: PatchSetId::generate(),
        base_snapshot_id: pack.base_snapshot_id.clone(),
        result_snapshot_id: pack.result_snapshot_id.clone(),
        files: Vec::new(),
        patch_graph_hash: String::new(),
    };
    patch.patch_graph_hash = hash_json(&patch)?;
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

fn pack_name_for(
    task: &crate::task::TaskDefinition,
    profile: &crate::task::candidate::CandidateProfile,
    exe_id: &str,
) -> String {
    let suffix: String = exe_id
        .chars()
        .rev()
        .take(6)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{}-{}-{}", task.name, profile.name, suffix)
}

/// One accepted change collected from an execution workspace.
#[derive(Debug, Clone)]
struct IsolatedChange {
    path: WorkspacePath,
    kind: IsolatedChangeKind,
    changed_lines: u64,
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
    let baseline_by_path: BTreeMap<&str, &FileManifestEntry> = baseline
        .files
        .iter()
        .map(|f| (f.path.as_str(), f))
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
            Some(entry) if entry.content_hash.as_deref() == Some(hash.as_str()) => {}
            Some(entry) => {
                let old_len = entry.size_bytes.max(1);
                let changed_lines = estimate_changed_lines(&data, old_len);
                changes.push(IsolatedChange {
                    path: rel,
                    kind: IsolatedChangeKind::Modified,
                    changed_lines,
                });
            }
            None => {
                let changed_lines = count_lines(&data);
                changes.push(IsolatedChange {
                    path: rel,
                    kind: IsolatedChangeKind::Added,
                    changed_lines,
                });
            }
        }
        Ok(())
    })?;
    for entry in &baseline.files {
        if !seen.contains(entry.path.as_str()) && !ignore.is_ignored(entry.path.as_str()) {
            changes.push(IsolatedChange {
                path: entry.path.clone(),
                kind: IsolatedChangeKind::Deleted,
                changed_lines: 0,
            });
        }
    }
    Ok(changes)
}

fn count_lines(data: &[u8]) -> u64 {
    if data.is_empty() {
        return 0;
    }
    data.iter().filter(|b| **b == b'\n').count() as u64 + 1
}

/// A cheap, conservative changed-line estimate: the larger of the new line
/// count and a byte-based estimate of the old size. Used only for candidate
/// change-budget enforcement.
fn estimate_changed_lines(new_data: &[u8], old_size_bytes: u64) -> u64 {
    let new_lines = count_lines(new_data);
    let old_estimate = old_size_bytes / 40; // ~40 bytes per line of code
    new_lines.max(old_estimate.max(1))
}

/// Contents of workspace files about to be overwritten by an execution's
/// changes, so the tree can be restored afterwards.
struct WorkspaceStash {
    /// path -> original bytes (None = file did not exist before).
    entries: Vec<(WorkspacePath, Option<Vec<u8>>)>,
}

fn stash_workspace_files(root: &Path, changes: &[IsolatedChange]) -> DraftResult<WorkspaceStash> {
    let mut entries = Vec::new();
    for change in changes {
        let dest = safe_workspace_dest(root, &change.path)?;
        let original = if dest.exists() {
            Some(fs::read(&dest).map_err(|e| {
                DraftError::storage(format!("failed to stash {}: {e}", change.path.as_str()))
            })?)
        } else {
            None
        };
        entries.push((change.path.clone(), original));
    }
    Ok(WorkspaceStash { entries })
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

fn restore_workspace_files(root: &Path, stash: WorkspaceStash) -> DraftResult<()> {
    for (path, original) in stash.entries {
        let dest = safe_workspace_dest(root, &path)?;
        match original {
            Some(bytes) => {
                if let Some(parent) = dest.parent() {
                    ensure_dir(parent)?;
                }
                write_atomic(&dest, &bytes)?;
            }
            None => {
                if dest.exists() {
                    fs::remove_file(&dest).map_err(|e| {
                        DraftError::storage(format!("failed to restore {}: {e}", path.as_str()))
                    })?;
                }
            }
        }
    }
    Ok(())
}

fn restore_snapshot(ws: &Workspace, snap: &Snapshot) -> DraftResult<()> {
    let store = ObjectStore::new(ws.layout.clone());
    let desired: BTreeSet<_> = snap.files.iter().map(|f| f.path.clone()).collect();
    let scanner = Scanner::new(ws)?;
    for path in scanner.current_manifest()?.keys() {
        if !desired.contains(path) && !is_draft_path(path.as_str()) {
            let fs_path = safe_workspace_dest(&ws.root, path)?;
            if fs_path.is_file() || fs_path.is_symlink() {
                fs::remove_file(fs_path)?;
            }
        }
    }
    for f in &snap.files {
        if is_draft_path(f.path.as_str()) {
            continue;
        }
        let dest = safe_workspace_dest(&ws.root, &f.path)?;
        if let Some(parent) = dest.parent() {
            ensure_dir(parent)?;
        }
        if let Some(hash) = &f.content_hash {
            let bytes = store.get_bytes(hash)?;
            write_atomic(&dest, &bytes)?;
        }
    }
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

fn checked_editor_path(root: &Path, path: &str) -> DraftResult<WorkspacePath> {
    let rel = WorkspacePath::new(
        crate::support::pathguard::check_relative(path).map_err(|e| {
            DraftError::new(
                DraftErrorKind::ProtectedFileAccess,
                format!("unsafe editor path '{path}': {e}"),
            )
        })?,
    );
    crate::workspace::protected::ensure_allowed(root, &rel)?;
    Ok(rel)
}

fn editor_backup_path(root: &Path, rel: &WorkspacePath) -> DraftResult<PathBuf> {
    let project_paths = crate::workspace::layout::DraftLayout::for_root(root);
    Ok(project_paths.editor_dir().join("backups").join(format!(
        "{}-{}",
        now().timestamp_millis(),
        rel.as_str().replace('/', "__")
    )))
}

fn simple_unified_diff(path: &str, old: &str, new: &str) -> String {
    if old == new {
        return format!("--- a/{path}\n+++ b/{path}\n");
    }
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    let old_lines = old.lines().collect::<Vec<_>>();
    let new_lines = new.lines().collect::<Vec<_>>();
    out.push_str(&format!(
        "@@ -1,{} +1,{} @@\n",
        old_lines.len(),
        new_lines.len()
    ));
    let max = old_lines.len().max(new_lines.len());
    for i in 0..max {
        match (old_lines.get(i), new_lines.get(i)) {
            (Some(a), Some(b)) if a == b => {
                out.push(' ');
                out.push_str(a);
                out.push('\n');
            }
            (Some(a), Some(b)) => {
                out.push('-');
                out.push_str(a);
                out.push('\n');
                out.push('+');
                out.push_str(b);
                out.push('\n');
            }
            (Some(a), None) => {
                out.push('-');
                out.push_str(a);
                out.push('\n');
            }
            (None, Some(b)) => {
                out.push('+');
                out.push_str(b);
                out.push('\n');
            }
            (None, None) => {}
        }
    }
    out
}

fn collect_editor_entries(
    root: &Path,
    dir: &Path,
    out: &mut Vec<EditorFileEntry>,
) -> DraftResult<()> {
    if crate::support::pathguard::is_draft_path(
        dir.strip_prefix(root)
            .unwrap_or(dir)
            .to_string_lossy()
            .as_ref(),
    ) {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)
        .map_err(|e| DraftError::storage(format!("cannot read {}: {e}", dir.display())))?
    {
        let entry = entry.map_err(|e| DraftError::storage(e.to_string()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| DraftError::storage(format!("cannot stat {}: {e}", path.display())))?;
        let rel_path = match path.strip_prefix(root) {
            Ok(path) => WorkspacePath::from_relative(path),
            Err(_) => continue,
        };
        if crate::support::pathguard::is_draft_path(rel_path.as_str()) {
            continue;
        }
        if file_type.is_dir() {
            if !file_type.is_symlink() {
                collect_editor_entries(root, &path, out)?;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|e| DraftError::storage(format!("cannot stat {}: {e}", path.display())))?;
        let protected = !crate::workspace::protected::violations(root, [&rel_path])?.is_empty();
        out.push(EditorFileEntry {
            path: rel_path.to_string(),
            kind: "file".to_string(),
            protected,
            bytes: metadata.len(),
        });
    }
    Ok(())
}

#[derive(Debug)]
struct HookContext {
    message: String,
    title: String,
    description: String,
    task_id: String,
    execution_id: String,
    pack_id: String,
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
        ("pack_id".to_string(), ctx.pack_id.clone()),
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
    env.insert("DRAFT_PACK_ID".to_string(), ctx.pack_id.clone());
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
        "pack_id",
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

fn interpolate_lenient(template: &str, values: &BTreeMap<String, String>) -> String {
    let mut out = template.to_string();
    for (k, v) in values {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
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

fn parse_duration_seconds(raw: &str) -> DraftResult<i64> {
    let (number, multiplier) = match raw.chars().last() {
        Some('s') => (&raw[..raw.len() - 1], 1),
        Some('m') => (&raw[..raw.len() - 1], 60),
        Some('h') => (&raw[..raw.len() - 1], 3_600),
        Some('d') => (&raw[..raw.len() - 1], 86_400),
        _ => {
            return Err(DraftError::invalid_config(
                "duration must end in s, m, h, or d",
            ))
        }
    };
    let value: i64 = number
        .parse()
        .map_err(|_| DraftError::invalid_config("invalid duration"))?;
    if value <= 0 {
        return Err(DraftError::invalid_config("duration must be positive"));
    }
    value
        .checked_mul(multiplier)
        .ok_or_else(|| DraftError::invalid_config("duration is too large"))
}

impl From<serde_json::Error> for DraftError {
    fn from(e: serde_json::Error) -> Self {
        json_err(e)
    }
}

#[cfg(test)]
mod app_tests {
    use super::*;

    struct GlobalHomeGuard(Option<std::ffi::OsString>);

    impl GlobalHomeGuard {
        fn set(path: &Path) -> Self {
            let previous = std::env::var_os("DRAFT_GLOBAL_HOME");
            std::env::set_var("DRAFT_GLOBAL_HOME", path);
            Self(previous)
        }
    }

    impl Drop for GlobalHomeGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                std::env::set_var("DRAFT_GLOBAL_HOME", previous);
            } else {
                std::env::remove_var("DRAFT_GLOBAL_HOME");
            }
        }
    }

    fn manifest_for(candidate: Option<&str>, pack_id: &str) -> crate::pack::PackManifest {
        crate::pack::PackManifest {
            schema_version: current_version(ContractId::PackManifest),
            pack_id: pack_id.to_string(),
            manifest_digest: String::new(),
            name: pack_id.to_string(),
            description: String::new(),
            intent: crate::pack::PackIntent::Feature,
            provenance: serde_json::json!({"origin": "test"}),
            author_id: "act_t".into(),
            candidate_id: candidate.map(|c| c.to_string()),
            declared_dependencies: Vec::new(),
            created_at: "2026-07-04T00:00:00+00:00".into(),
        }
    }

    fn rollback_event(subject: &str) -> crate::trust::event::EventRecord {
        crate::trust::event::EventRecord {
            schema_version: current_version(ContractId::EventRecord),
            ledger: crate::trust::event::LedgerIdentity::Workspace {
                workspace_id: "ws_t".into(),
            },
            event_id: "evt_t".into(),
            event_type: "RollbackPerformed".into(),
            time: "2026-07-04T00:00:00+00:00".into(),
            subject_id: Some(subject.to_string()),
            actor_id: "act_t".into(),
            candidate_id: None,
            previous_event_hash: String::new(),
            event_hash: String::new(),
            receipt_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn candidate_rollback_rate_counts_rolled_back_fraction() {
        let manifests = vec![
            manifest_for(Some("cand_a"), "pck_1"),
            manifest_for(Some("cand_a"), "pck_2"),
            manifest_for(Some("cand_b"), "pck_3"),
            manifest_for(None, "pck_4"),
        ];
        let events = vec![rollback_event("pck_1")];
        // One of cand_a's two packs was rolled back.
        assert_eq!(candidate_rollback_rate(&events, &manifests, "cand_a"), 0.5);
        // cand_b has packs but no rollbacks.
        assert_eq!(candidate_rollback_rate(&events, &manifests, "cand_b"), 0.0);
        // Unknown candidates never divide by zero.
        assert_eq!(candidate_rollback_rate(&events, &manifests, "cand_x"), 0.0);
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
        let project_paths = crate::workspace::layout::DraftLayout::for_root(tmp.path());
        project_paths.create_all().unwrap();
        let id = WorkspaceId::generate();
        write_json(
            &layout.workspace_json(),
            &WorkspaceMetadata {
                schema_version: current_version(ContractId::WorkspaceMetadata),
                workspace_id: id.clone(),
                draft_version: crate::DRAFT_VERSION.to_string(),
                created_at: now(),
            },
        )
        .unwrap();
        crate::operation::RecoveryStore::for_root(tmp.path())
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
        let project_paths = crate::workspace::layout::DraftLayout::for_root(tmp.path());
        project_paths.create_all().unwrap();
        write_json(
            &layout.workspace_json(),
            &WorkspaceMetadata {
                schema_version: current_version(ContractId::WorkspaceMetadata),
                workspace_id: WorkspaceId::generate(),
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
        crate::workspace::stable::StableHeadStore::new(project_paths)
            .initialize(tmp.path(), "rcp_test".to_string())
            .unwrap();

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
        let project_paths = crate::workspace::layout::DraftLayout::for_root(tmp.path());
        project_paths.create_all().unwrap();
        write_json(
            &layout.workspace_json(),
            &WorkspaceMetadata {
                schema_version: current_version(ContractId::WorkspaceMetadata),
                workspace_id: WorkspaceId::generate(),
                draft_version: crate::DRAFT_VERSION.to_string(),
                created_at: now(),
            },
        )
        .unwrap();
        crate::workspace::stable::StableHeadStore::new(project_paths)
            .initialize(tmp.path(), "rcp_test".to_string())
            .unwrap();
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
            vec!["cargo".into(), "test".into()],
        );
        let execution_store = crate::task::ExecutionStore::for_root(tmp.path());
        execution_store.write(&execution).unwrap();
        execution_store
            .mark_failed(execution.id.as_str(), "tests failed")
            .unwrap();
        crate::operation::RecoveryStore::for_root(tmp.path())
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
    fn editor_file_lifecycle_uses_guards_backups_and_search() {
        let tmp = tempfile::tempdir().unwrap();
        let app = App::new();
        let layout = DraftLayout::for_root(tmp.path());
        layout.create_all().unwrap();
        let project_paths = crate::workspace::layout::DraftLayout::for_root(tmp.path());
        project_paths.create_all().unwrap();
        write_json(
            &layout.workspace_json(),
            &WorkspaceMetadata {
                schema_version: current_version(ContractId::WorkspaceMetadata),
                workspace_id: WorkspaceId::generate(),
                draft_version: crate::DRAFT_VERSION.to_string(),
                created_at: now(),
            },
        )
        .unwrap();
        crate::workspace::stable::StableHeadStore::new(project_paths)
            .initialize(tmp.path(), "rcp_test".to_string())
            .unwrap();

        let created = app
            .editor_create_file(tmp.path(), "src/editor.txt", "needle\n")
            .unwrap();
        assert_eq!(created.action, "created");
        assert_eq!(
            app.editor_search(tmp.path(), "needle", 10).unwrap()[0].path,
            "src/editor.txt"
        );

        let renamed = app
            .editor_rename_file(tmp.path(), "src/editor.txt", "src/renamed.txt")
            .unwrap();
        assert_eq!(renamed.old_path.as_deref(), Some("src/editor.txt"));
        assert!(tmp.path().join("src/renamed.txt").exists());

        let deleted = app
            .editor_delete_file(tmp.path(), "src/renamed.txt")
            .unwrap();
        assert_eq!(deleted.action, "deleted");
        assert!(deleted.backup_path.is_some());
        assert!(!tmp.path().join("src/renamed.txt").exists());

        let err = app
            .editor_create_file(tmp.path(), ".draft/owned.txt", "")
            .unwrap_err();
        assert!(matches!(
            err.kind,
            DraftErrorKind::ProtectedFileAccess | DraftErrorKind::Storage
        ));
    }

    #[test]
    fn global_config_get_unset_are_scoped_to_global_home() {
        let _lock = crate::workspace::home::global_home_env_lock()
            .lock()
            .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join(".draft-global");
        let _global = GlobalHomeGuard::set(&global);
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

    #[test]
    fn pack_dirty_guard_rejects_edits_after_pack_target() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = DraftLayout::for_root(tmp.path());
        layout.create_all().unwrap();
        let project_paths = crate::workspace::layout::DraftLayout::for_root(tmp.path());
        project_paths.create_all().unwrap();
        let workspace_id = WorkspaceId::generate();
        write_json(
            &layout.workspace_json(),
            &WorkspaceMetadata {
                schema_version: current_version(ContractId::WorkspaceMetadata),
                workspace_id: workspace_id.clone(),
                draft_version: crate::DRAFT_VERSION.to_string(),
                created_at: now(),
            },
        )
        .unwrap();
        let stable = crate::workspace::stable::StableHeadStore::new(project_paths.clone())
            .initialize(tmp.path(), "rcp_test".to_string())
            .unwrap();
        let pack = PackWorkspace::new(
            workspace_id.clone(),
            None,
            None,
            SnapshotId::generate(),
            SnapshotId::generate(),
            Some("dirty-pack".into()),
        );
        let manifest = manifest_for(None, pack.id.as_str());
        let store = crate::pack::PackStore::new(project_paths);
        store.write_manifest(&manifest).unwrap();
        let manifest = store.read_manifest(pack.id.as_str()).unwrap();
        let mut revision = crate::pack::PackRevision {
            schema_version: current_version(ContractId::PackRevision),
            pack_id: manifest.pack_id.clone(),
            manifest_digest: manifest.manifest_digest.clone(),
            revision_id: "rev_dirty".into(),
            revision_number: 1,
            revision_digest: String::new(),
            base_digest: stable.workspace_hash.clone(),
            content_digest: stable.workspace_hash.clone(),
            diff_digest: sha256_hex(b""),
            target_digest: stable.workspace_hash,
            resolved_dependency_digests: Vec::new(),
            created_at: now().to_rfc3339(),
        };
        revision.refresh_revision_digest();
        store.write_revision(&revision).unwrap();
        store
            .write_lockfile(&crate::pack::PackLockfile {
                schema_version: current_version(ContractId::PackLock),
                pack_id: manifest.pack_id.clone(),
                workspace_hash: revision.target_digest.clone(),
                file_hashes: BTreeMap::new(),
                policy_version: crate::DRAFT_VERSION.into(),
                risk_engine_version: crate::DRAFT_VERSION.into(),
                verification_commands: Vec::new(),
                lsif_version: crate::DRAFT_VERSION.into(),
                test_selector_version: crate::DRAFT_VERSION.into(),
                fuzz_selector_version: crate::DRAFT_VERSION.into(),
                dependency_pack_hashes: Vec::new(),
                receipt_digests: Vec::new(),
            })
            .unwrap();
        store
            .write_lifecycle_in(
                crate::pack::PackLocation::Store,
                &crate::pack::lifecycle::PackLifecycleRecord {
                    schema_version: current_version(ContractId::PackLifecycle),
                    pack_id: manifest.pack_id,
                    revision_id: revision.revision_id.clone(),
                    revision_digest: revision.revision_digest.clone(),
                    lifecycle: crate::pack::lifecycle::PackLifecycle::Draft,
                    updated_at: now(),
                    last_operation_id: crate::support::common::OperationId::new("op_dirty"),
                },
            )
            .unwrap();
        std::fs::write(tmp.path().join("src.txt"), "manual edit\n").unwrap();
        let ws = Workspace {
            workspace_id,
            root: tmp.path().to_path_buf(),
            layout,
        };

        let err =
            ensure_pack_workspace_matches_target(&ws, &pack, "review", "restore the workspace")
                .unwrap_err();
        assert_eq!(err.kind, DraftErrorKind::DirtyWorkspace);
        assert!(err.message.contains("review baseline"));
    }

    #[test]
    fn pack_reopen_creates_audited_revision_and_invalidates_current_evidence() {
        let _lock = crate::workspace::home::global_home_env_lock()
            .lock()
            .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();
        let _global = GlobalHomeGuard::set(&global.path().join(".draft-global"));
        let layout = DraftLayout::for_root(tmp.path());
        layout.create_all().unwrap();
        let project_paths = crate::workspace::layout::DraftLayout::for_root(tmp.path());
        project_paths.create_all().unwrap();
        write_json(
            &layout.workspace_json(),
            &WorkspaceMetadata {
                schema_version: current_version(ContractId::WorkspaceMetadata),
                workspace_id: WorkspaceId::generate(),
                draft_version: crate::DRAFT_VERSION.to_string(),
                created_at: now(),
            },
        )
        .unwrap();
        crate::workspace::stable::StableHeadStore::new(project_paths.clone())
            .initialize(tmp.path(), "rcp_reopen_test".to_string())
            .unwrap();
        let manifest = manifest_for(None, "pck_reopen");
        let store = crate::pack::PackStore::new(project_paths);
        store.write_manifest(&manifest).unwrap();
        let manifest = store.read_manifest("pck_reopen").unwrap();
        let mut patch = PatchSet {
            schema_version: current_version(ContractId::PatchSet),
            id: PatchSetId::generate(),
            base_snapshot_id: SnapshotId::new("chk_empty"),
            result_snapshot_id: SnapshotId::new("chk_empty"),
            files: Vec::new(),
            patch_graph_hash: String::new(),
        };
        patch.patch_graph_hash = hash_json(&patch).unwrap();
        let patch_bytes = to_pretty(&patch).unwrap();
        let mut revision = crate::pack::PackRevision {
            schema_version: current_version(ContractId::PackRevision),
            pack_id: manifest.pack_id.clone(),
            manifest_digest: manifest.manifest_digest.clone(),
            revision_id: "rev_rejected".into(),
            revision_number: 1,
            revision_digest: String::new(),
            base_digest: "sha256:base".into(),
            content_digest: "sha256:content".into(),
            diff_digest: sha256_hex(&patch_bytes),
            target_digest: "sha256:target".into(),
            resolved_dependency_digests: Vec::new(),
            created_at: now().to_rfc3339(),
        };
        revision.refresh_revision_digest();
        store.write_revision(&revision).unwrap();
        write_atomic(
            &store
                .dir_for(crate::pack::PackLocation::Store, "pck_reopen")
                .join("changes.patch"),
            &patch_bytes,
        )
        .unwrap();
        store
            .write_lockfile(&crate::pack::PackLockfile {
                schema_version: current_version(ContractId::PackLock),
                pack_id: manifest.pack_id.clone(),
                workspace_hash: revision.target_digest.clone(),
                file_hashes: BTreeMap::new(),
                policy_version: crate::DRAFT_VERSION.into(),
                risk_engine_version: crate::DRAFT_VERSION.into(),
                verification_commands: Vec::new(),
                lsif_version: crate::DRAFT_VERSION.into(),
                test_selector_version: crate::DRAFT_VERSION.into(),
                fuzz_selector_version: crate::DRAFT_VERSION.into(),
                dependency_pack_hashes: Vec::new(),
                receipt_digests: Vec::new(),
            })
            .unwrap();
        store
            .write_lifecycle_in(
                crate::pack::PackLocation::Store,
                &crate::pack::lifecycle::PackLifecycleRecord {
                    schema_version: current_version(ContractId::PackLifecycle),
                    pack_id: manifest.pack_id,
                    revision_id: revision.revision_id.clone(),
                    revision_digest: revision.revision_digest.clone(),
                    lifecycle: crate::pack::lifecycle::PackLifecycle::Rejected,
                    updated_at: now(),
                    last_operation_id: crate::support::common::OperationId::new("op_rejected"),
                },
            )
            .unwrap();

        let report = App::new()
            .pack_reopen(tmp.path(), "pck_reopen", "op_reopen_test")
            .unwrap();
        assert!(report.revision_id.starts_with("rev_"));
        let inspected = App::new().pack_inspect(tmp.path(), "pck_reopen").unwrap();
        assert_eq!(inspected.lifecycle, PackLifecycle::Draft);
        assert_eq!(inspected.revision_id, report.revision_id);
        assert!(!inspected.verified);
        assert_eq!(inspected.valid_actions, vec!["verify"]);
        assert!(App::new()
            .canonical_events(tmp.path())
            .unwrap()
            .iter()
            .any(|event| event.event_type == "PackReopened"));
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
        let _lock = crate::workspace::home::global_home_env_lock()
            .lock()
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("source.txt"), "canonical source\n").unwrap();
        let global_root = temp.path().join("global");
        let _guard = GlobalHomeGuard::set(&global_root);
        let app = App::new();
        let initialized = app.init(&root).unwrap();
        let home = crate::workspace::home::DraftGlobalStore::at(&global_root);
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
        let mut manifest = manifest_for(Some(&candidate.candidate_id), "pck_profile_invariance");
        manifest.author_id = actor.actor_id.clone();
        manifest.refresh_manifest_digest();
        let pack_store = crate::pack::PackStore::new(workspace.layout.clone());
        pack_store.write_manifest(&manifest).unwrap();
        crate::support::fsutil::write_json(
            &home.revoked_keys_json(),
            &serde_json::json!({
                "schema_version": current_version(ContractId::RevokedKeyRegistry),
                "public_key_ids": [],
            }),
        )
        .unwrap();
        crate::trust::audit::GlobalAuditLog::global()
            .unwrap()
            .append(
                "baseline",
                Some(actor.actor_id.clone()),
                Some("profile-invariance".into()),
                None,
                serde_json::json!({}),
            )
            .unwrap();

        let ledger = crate::trust::ledger::TrustLedger::open_at(
            &root,
            &initialized.workspace_id,
            home.clone(),
        )
        .unwrap();
        assert!(ledger.verify_all().unwrap().all_ok);

        let layout = crate::workspace::layout::DraftLayout::for_root(&root);
        let actor_bytes = std::fs::read(home.actor_json()).unwrap();
        let signing_key_bytes = std::fs::read(home.signing_key()).unwrap();
        let public_keys_digest = tree_digest(&home.public_keys_dir());
        let trust_digest = tree_digest(&home.trust_dir());
        let candidate_registry_bytes = std::fs::read(home.candidates_json()).unwrap();
        let manifest_bytes =
            std::fs::read(layout.pack_manifest(manifest.pack_id.as_str())).unwrap();
        let events_before = app.events(&root).unwrap();
        let event_log_before = std::fs::read(layout.event_log()).unwrap();
        let receipts_before = crate::trust::receipt::ReceiptStore::new(layout.clone())
            .list()
            .unwrap();
        let receipts_digest = tree_digest(&layout.receipts_dir());
        let source_policy = crate::workspace::source_view::CanonicalSourcePolicy::default();
        let source_digest =
            crate::workspace::source_view::CanonicalSourceView::build(&root, &source_policy)
                .unwrap()
                .content_digest;
        let workspace_digest = crate::workspace::source_view::workspace_hash(&root).unwrap();
        let ownership_before = crate::workspace::ownership::evaluate(
            &root,
            &["source.txt".into()],
            &["@owner".into()],
        )
        .unwrap();
        let policy_before = crate::review::policy::Policy::resolve(
            Some(&layout.policy_toml()),
            Some(&home.default_policy_toml()),
        )
        .unwrap();
        let audit_before = crate::trust::audit::GlobalAuditLog::global()
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
            std::fs::read(layout.pack_manifest(manifest.pack_id.as_str())).unwrap(),
            manifest_bytes
        );
        assert_eq!(
            manifest.candidate_id.as_deref(),
            Some(candidate.candidate_id.as_str())
        );
        assert_eq!(tree_digest(&layout.receipts_dir()), receipts_digest);
        assert_eq!(
            crate::trust::receipt::ReceiptStore::new(layout.clone())
                .list()
                .unwrap(),
            receipts_before
        );
        assert_eq!(
            crate::workspace::source_view::CanonicalSourceView::build(&root, &source_policy)
                .unwrap()
                .content_digest,
            source_digest
        );
        assert_eq!(
            crate::workspace::source_view::workspace_hash(&root).unwrap(),
            workspace_digest
        );
        assert_eq!(
            crate::workspace::ownership::evaluate(
                &root,
                &["source.txt".into()],
                &["@owner".into()],
            )
            .unwrap(),
            ownership_before
        );
        assert_eq!(
            crate::review::policy::Policy::resolve(
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
        assert!(std::fs::read(layout.event_log())
            .unwrap()
            .starts_with(&event_log_before));
        for event in &events_after[events_before.len()..] {
            assert_eq!(event.actor_id, actor.actor_id);
            assert_eq!(event.event_type, "user.profile.updated");
            assert_eq!(event.metadata["scope"], "project");
            assert!(event.metadata.get("changed_keys").is_some());
            assert!(event.metadata.get("resulting_config_digest").is_some());
            let serialized = serde_json::to_string(event).unwrap();
            assert!(!serialized.contains("Project User"));
            assert!(!serialized.contains("project@example.test"));
        }
        assert!(
            crate::trust::ledger::TrustLedger::open_at(
                &root,
                &initialized.workspace_id,
                home.clone(),
            )
            .unwrap()
            .verify_all()
            .unwrap()
            .all_ok
        );

        let audit_after = crate::trust::audit::GlobalAuditLog::global()
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
