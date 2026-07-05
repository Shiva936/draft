use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::common::{now, ActorId, ReceiptId, WorkspaceId, WorkspacePath};
use crate::error::{DraftError, DraftErrorKind, DraftResult};
use crate::fsutil::{
    ensure_dir, list_with_extension, read_json, read_toml, write_atomic, write_json, write_toml,
};
use crate::identity::{resolve_actor, ActorKind, ActorRef};
use crate::lock::FileGuard;

const DRAFT_DIR: &str = ".draft";
const SCHEMA_VERSION: u32 = 5;

crate::id_newtype!(EventId, "evt_");
crate::id_newtype!(SnapshotId, "chk_");
crate::id_newtype!(TaskId, "tsk_");
crate::id_newtype!(RunId, "run_");
crate::id_newtype!(ChangepackId, "pck_");
crate::id_newtype!(EvidenceId, "evd_");
crate::id_newtype!(PatchSetId, "patch_");
crate::id_newtype!(DecisionId, "dec_");
crate::id_newtype!(ReviewCommentId, "rcom_");
crate::id_newtype!(RollbackPlanId, "rbp_");

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
    pub detail: String,
}

impl DoctorCheck {
    fn ok(name: &str, detail: impl Into<String>) -> Self {
        DoctorCheck {
            name: name.to_string(),
            ok: true,
            detail: detail.into(),
        }
    }
    fn fail(name: &str, detail: impl Into<String>) -> Self {
        DoctorCheck {
            name: name.to_string(),
            ok: false,
            detail: detail.into(),
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
    pub lifecycle: String,
    pub symbols_touched: Vec<String>,
    pub public_api_changed: Vec<String>,
    pub receipts: Vec<String>,
    pub verified: bool,
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
}

/// Report from `draft verify pck_<id>` (v0.3.2 evidence-based verification).
#[derive(Debug, Clone, Serialize)]
pub struct VerifyV2Report {
    pub pack_id: String,
    pub risk_level: String,
    pub risk_score: u32,
    pub explanations: Vec<String>,
    pub required_actions: Vec<String>,
    pub selected_tests: Vec<crate::verifyv2::SelectedTest>,
    pub selected_fuzz_targets: Vec<crate::verifyv2::SelectedFuzzTarget>,
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
    kind: crate::event::EventKind,
    intent: crate::pack::PackIntent,
    approval: crate::pack::ApprovalState,
    save: crate::pack::SaveState,
    metadata: Value,
}

/// Result of a `--dry-run` for save or rollback: what would happen and why.
#[derive(Debug, Clone, Serialize)]
pub struct DryRunReport {
    pub action: String,
    pub target: String,
    pub would_proceed: bool,
    pub resulting_state: String,
    pub affected_files: Vec<String>,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    /// True if every present scope is healthy.
    pub fn healthy(&self) -> bool {
        self.global.healthy() && self.project.as_ref().map(|p| p.healthy()).unwrap_or(true)
    }
}

/// Write a minimal canonical manifest for the implicit base pack (empty change).
fn write_base_canonical_manifest(root: &Path, pack_id: &ChangepackId, name: &str) {
    use crate::pack::{ApprovalState, ImportState, PackManifest, PackStore, SaveState};
    let empty = sha256_hex(b"");
    let manifest = PackManifest {
        schema_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
        pack_id: pack_id.to_string(),
        name: name.to_string(),
        description: "base pack".to_string(),
        intent: crate::pack::PackIntent::Feature,
        origin: "local".to_string(),
        actor: "actor_local".to_string(),
        candidate: None,
        created_at: now().to_rfc3339(),
        base_workspace_hash: empty.clone(),
        target_workspace_hash: empty.clone(),
        changes_hash: empty.clone(),
        risk_hash: String::new(),
        verify_hash: String::new(),
        lsif_hash: String::new(),
        receipt_hashes: Vec::new(),
        import_state: ImportState::None,
        approval_state: ApprovalState::Pending,
        save_state: SaveState::Unsaved,
    };
    let store = PackStore::new(crate::layout::ProjectPaths::for_root(root));
    let _ = store.write_manifest(&manifest);
    // Empty lockfile so conflicts/depends have a file set to read.
    let lock = crate::pack::PackLockfile {
        schema_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
        pack_id: pack_id.to_string(),
        workspace_hash: empty,
        file_hashes: std::collections::BTreeMap::new(),
        policy_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
        risk_engine_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
        verification_commands: Vec::new(),
        lsif_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
        test_selector_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
        fuzz_selector_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
        dependency_pack_hashes: Vec::new(),
        receipt_hashes: Vec::new(),
    };
    let _ = store.write_lockfile(&lock);
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
    for dent in walker.flatten() {
        let path = dent.path();
        if !path.is_file() || crate::pathguard::path_is_draft(path) {
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
            if let Ok(content) = fs::read_to_string(path) {
                out.push((rel, content));
            }
        }
    }
    Ok(out)
}

/// Discover available fuzz target names under a `fuzz/fuzz_targets/` directory.
fn scan_fuzz_targets(root: &Path) -> Vec<String> {
    let dir = root.join("fuzz/fuzz_targets");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    out.push(stem.to_string());
                }
            }
        }
    }
    out.sort();
    out
}

/// Names of packs currently sitting in the import quarantine.
fn quarantine_names(paths: &crate::layout::ProjectPaths) -> Vec<String> {
    let mut names = Vec::new();
    let qdir = paths.quarantine_dir();
    if let Ok(entries) = std::fs::read_dir(&qdir) {
        for entry in entries.flatten() {
            let manifest = entry.path().join("manifest.json");
            if let Ok(m) = read_json::<crate::pack::PackManifest>(&manifest) {
                names.push(m.name);
            }
        }
    }
    names
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
        let created = !layout.draft_dir.exists();
        if !created {
            return Err(DraftError::invalid_config(
                "Draft workspace is already initialized; refusing to reinitialize without an explicit repair mode",
            ));
        }
        layout.create_all()?;
        // v0.3.2 canonical project store (events/receipts/transparency/packs/
        // imports/quarantine/exports/lsif/cache/adapters) + hidden `.draft/`.
        let project_paths = crate::layout::ProjectPaths::for_root(root);
        let hidden_status = project_paths.create_all()?;
        if let crate::hidden::HiddenStatus::Failed(reason) = &hidden_status {
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
            write_toml(&layout.verify_toml(), &VerifyFile::default())?;
        }
        if !layout.risk_toml().exists() {
            write_toml(&layout.risk_toml(), &RiskConfig::default())?;
        }
        if !layout.policy_toml().exists() {
            write_toml(&layout.policy_toml(), &PolicyConfig::default())?;
        }
        rebuild_index_for_layout(&layout)?;
        let meta = if layout.workspace_json().exists() {
            read_json::<WorkspaceMetadata>(&layout.workspace_json())?
        } else {
            let meta = WorkspaceMetadata {
                schema_version: SCHEMA_VERSION,
                id: WorkspaceId::generate(),
                draft_version: crate::DRAFT_VERSION.to_string(),
                created_at: now(),
            };
            write_json(&layout.workspace_json(), &meta)?;
            meta
        };
        let store = EventStore::new(layout.clone(), meta.id.clone())?;
        if created {
            store.append(
                "repo.initialized",
                None,
                serde_json::json!({ "root": root.display().to_string() }),
            )?;
            let base = Changepack::new(
                meta.id.clone(),
                None,
                None,
                SnapshotId::new("chk_empty"),
                SnapshotId::new("chk_empty"),
                Some(base_pack_name.to_string()),
            );
            let pack_dir = layout.pack_dir(&base.id);
            ensure_dir(&pack_dir)?;
            write_json(&pack_dir.join("manifest.json"), &base)?;
            // Also write a canonical v0.3.2 manifest for the (empty) base pack so
            // it participates in inspect/depends/conflicts/compose. No trust
            // receipt is minted for the implicit base pack.
            write_base_canonical_manifest(root, &base.id, base_pack_name);
            write_atomic(
                layout.selected_pack_file().as_path(),
                base.id.to_string().as_bytes(),
            )?;
            store.append(
                "pack.created",
                Some(base.id.to_string()),
                serde_json::to_value(&base).unwrap_or(Value::Null),
            )?;
            store.append(
                "pack.selected",
                Some(base.id.to_string()),
                serde_json::json!({ "name": base_pack_name }),
            )?;
        }
        Ok(InitReport {
            workspace_id: meta.id.to_string(),
            root: root.display().to_string(),
            created,
            draft_dir: layout.draft_dir.display().to_string(),
        })
    }

    pub fn open(&self, cwd: &Path) -> DraftResult<Workspace> {
        let root = find_workspace_root(cwd).ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::WorkspaceNotFound,
                "not inside a Draft workspace",
            )
            .with_suggestion("run `draft init`")
        })?;
        let layout = DraftLayout::for_root(&root);
        let meta = read_json::<WorkspaceMetadata>(&layout.workspace_json())?;
        // One-time migration: a workspace created by v0.3.1 lacks the canonical
        // v0.3.2 stores (transparency/, imports/quarantine/, lsif/, ...). Detect
        // via the transparency dir and create the missing tree idempotently.
        let paths = crate::layout::ProjectPaths::for_root(&root);
        if !paths.transparency_dir().exists() {
            paths.create_all()?;
        }
        Ok(Workspace {
            id: meta.id,
            root,
            layout,
        })
    }

    pub fn config_set(&self, cwd: &Path, key: &str, value: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let ws = self.open(cwd)?;
        let mut cfg = read_or_default::<DraftConfig>(&ws.layout.config_toml());
        cfg.set(key, value)?;
        write_toml(&ws.layout.config_toml(), &cfg)?;
        ws.events()?
            .append("config.set", None, serde_json::json!({ "key": key }))?;
        Ok(ConfigReport::single(key, value))
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
        let mut cfg = read_or_default::<DraftConfig>(&ws.layout.config_toml());
        cfg.unset(key)?;
        write_toml(&ws.layout.config_toml(), &cfg)?;
        ws.events()?
            .append("config.unset", None, serde_json::json!({ "key": key }))?;
        Ok(ConfigReport::single(key, ""))
    }

    pub fn config_list(&self, cwd: &Path) -> DraftResult<ConfigReport> {
        let ws = self.open(cwd)?;
        Ok(ConfigReport {
            entries: ResolvedConfig::load(&ws)?.entries(),
        })
    }

    // ---- v0.3.2 global store, doctor, identity, layered config ----------

    /// `draft init --global`: create the hidden global `~/.draft/` store,
    /// provision the actor identity + Ed25519 signing key, and seed the
    /// default policy. Idempotent.
    pub fn init_global(&self) -> DraftResult<InitGlobalReport> {
        let home = crate::home::GlobalHome::locate()?;
        let created = !home.exists();
        let hidden = home.create_all()?;
        // Seed a default policy file if absent (safe default).
        if !home.default_policy_toml().exists() {
            write_toml(
                &home.default_policy_toml(),
                &crate::policy::Policy::safe_default(),
            )?;
        }
        if !home.config_toml().exists() {
            crate::fsutil::write_atomic(&home.config_toml(), b"# Draft global config\n")?;
        }
        let profile = crate::identity::global::ensure_actor(&home)?;
        Ok(InitGlobalReport {
            root: home.root().display().to_string(),
            created,
            hidden: hidden.is_ok(),
            actor_id: profile.actor_id,
            public_key_id: profile.public_key_id,
        })
    }

    /// `draft identity status`: show the active actor and signing-key state.
    pub fn identity_status(&self) -> DraftResult<crate::identity::IdentityStatus> {
        let home = crate::home::GlobalHome::locate()?;
        crate::identity::global::status(&home)
    }

    /// `draft receipt verify rcp_<id>`: verify a single signed receipt.
    pub fn receipt_verify(
        &self,
        cwd: &Path,
        receipt_id: &str,
    ) -> DraftResult<crate::receipt::ReceiptVerification> {
        validate_receipt_id(receipt_id)?;
        let ws = self.open(cwd)?;
        let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
        ledger.verify_receipt(receipt_id)
    }

    /// `draft receipt verify --all`: verify the event chain, transparency chain,
    /// and every receipt. Fails closed if anything does not verify.
    pub fn receipt_verify_all(&self, cwd: &Path) -> DraftResult<crate::ledger::LedgerVerification> {
        let ws = self.open(cwd)?;
        let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
        ledger.verify_all()
    }

    /// `draft config set --global <key> <value>`.
    pub fn config_set_global(&self, key: &str, value: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        let home = crate::home::GlobalHome::locate()?;
        home.create_all()?;
        crate::config::set_value(&home.config_toml(), key, value)?;
        Ok(ConfigReport::single(key, value))
    }

    /// `draft config get <key>` with full precedence: CLI > project > global >
    /// built-in default. Works outside a workspace (project layer is skipped).
    pub fn config_get_layered(&self, cwd: &Path, key: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        let home = crate::home::GlobalHome::locate().ok();
        let project_cfg = self.open(cwd).ok().map(|ws| ws.layout.config_toml());
        let resolver = crate::config::ConfigResolver::load(
            project_cfg.as_deref(),
            home.as_ref().map(|h| h.config_toml()).as_deref(),
        );
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
            Err(_) => None,
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
        let home = crate::home::GlobalHome::locate()?;
        let exists = home.exists();
        let mut checks = Vec::new();
        if exists {
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
            #[cfg(unix)]
            checks.push(key_perms_check(&home.signing_key()));
            // Adapter status is explicit: every protocol adapter is either
            // implemented or marked experimental (never a silent stub).
            for adapter in crate::adapters::protocol_adapters() {
                checks.push(DoctorCheck::ok(
                    &format!("adapter:{}", adapter.id),
                    format!("{} — {}", adapter.display_name, adapter.status),
                ));
            }
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
            hidden: crate::hidden::is_hidden(home.root()),
            checks,
        })
    }

    fn doctor_project_scope(&self, ws: &Workspace) -> DraftResult<DoctorScope> {
        let paths = crate::layout::ProjectPaths::for_root(&ws.root);
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
            Err(e) => checks.push(DoctorCheck::fail("event-chain", e.message)),
        }
        // v0.3.2 trust ledger: canonical event log, receipts, transparency chain.
        match crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str()) {
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
                Err(e) => checks.push(DoctorCheck::fail("trust-ledger", e.message)),
            },
            Err(e) => checks.push(DoctorCheck::fail("trust-ledger", e.message)),
        }
        for (name, dir) in [
            ("events-dir", paths.events_dir()),
            ("receipts-dir", paths.receipts_dir()),
            ("transparency-dir", paths.transparency_dir()),
            ("packs-dir", paths.packs_dir()),
            ("quarantine-dir", paths.quarantine_dir()),
        ] {
            checks.push(bool_check(
                name,
                dir.is_dir(),
                format!("{} present", dir.display()),
                format!("{} missing", dir.display()),
            ));
        }
        Ok(DoctorScope {
            label: "project".to_string(),
            root: ws.root.display().to_string(),
            exists: true,
            hidden: crate::hidden::is_hidden(paths.draft_dir()),
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
            run_id: String::new(),
            changepack_id: String::new(),
            receipt_id: ReceiptId::generate().to_string(),
            actor_name: cfg.identity_username.clone(),
            actor_email: cfg.identity_email.clone(),
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
            .map_err(|e| DraftError::new(DraftErrorKind::SaveFailed, e.message))?;
        ws.events()?.append(
            "hook.completed",
            Some(hook_name.to_string()),
            serde_json::to_value(&result).unwrap_or(Value::Null),
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

    pub fn status_v031(
        &self,
        cwd: &Path,
        _pack: Option<&str>,
        _component: Option<&str>,
        _full: bool,
    ) -> DraftResult<WorkspaceStatus> {
        self.status(cwd)
    }

    pub fn checkpoint(&self, cwd: &Path, message: &str) -> DraftResult<CheckpointReport> {
        let ws = self.open(cwd)?;
        let snapshot = Snapshotter::new(&ws)?.create_snapshot()?;
        let receipt = Receipt::new(
            "checkpoint",
            "completed",
            Some(snapshot.id.to_string()),
            serde_json::json!({ "message": message }),
        )
        .reversible_to(snapshot.id.to_string());
        write_receipt(&ws, &receipt)?;
        ws.events()?.append(
            "checkpoint.created",
            Some(snapshot.id.to_string()),
            serde_json::json!({ "message": message }),
        )?;
        // v0.3.2 trust ledger: a checkpoint is a receipt-producing event. Record
        // a canonical CheckpointCreated event with a signed receipt and a
        // transparency-chain entry bound to the current workspace hash.
        let workspace_hash = crate::hashing::workspace_hash(&ws.root)?;
        let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
        ledger.record(
            crate::event::EventKind::CheckpointCreated,
            Some(snapshot.id.to_string()),
            None,
            workspace_hash,
            serde_json::json!({ "message": message }),
        )?;
        Ok(CheckpointReport {
            snapshot_id: snapshot.id.to_string(),
            // The report's receipt_id is the reversible rollback target (legacy
            // receipt); the signed trust receipt lives in the ledger and is
            // verifiable via `draft receipt verify --all`.
            receipt_id: receipt.id.to_string(),
            files: snapshot.files.len(),
        })
    }

    pub fn task_create(
        &self,
        cwd: &Path,
        title: &str,
        description: Option<String>,
    ) -> DraftResult<Task> {
        let ws = self.open(cwd)?;
        let task = Task {
            schema_version: SCHEMA_VERSION,
            id: TaskId::generate(),
            title: title.to_string(),
            description,
            created_by: resolve_actor(&ws.layout.draft_dir),
            risk_profile: None,
            linked_issue: None,
            created_at: now(),
            status: TaskStatus::Open,
        };
        write_json(
            &ws.layout.tasks_dir().join(format!("{}.json", task.id)),
            &task,
        )?;
        ws.events()?.append(
            "task.created",
            Some(task.id.to_string()),
            serde_json::to_value(&task).unwrap_or(Value::Null),
        )?;
        Ok(task)
    }

    pub fn task_list(&self, cwd: &Path) -> DraftResult<Vec<Task>> {
        let ws = self.open(cwd)?;
        load_json_dir(&ws.layout.tasks_dir())
    }

    pub fn task_show(&self, cwd: &Path, id: &str) -> DraftResult<Task> {
        let ws = self.open(cwd)?;
        read_json(&ws.layout.tasks_dir().join(format!("{}.json", id)))
    }

    pub fn task_spawn(
        &self,
        cwd: &Path,
        name: &str,
        pack_id: Option<&str>,
        candidates: Vec<String>,
        cron: Option<String>,
        instruction: Vec<String>,
    ) -> DraftResult<TaskSpawnReport> {
        let instruction = instruction.join(" ");
        let task = self.task_create(cwd, name, Some(instruction.clone()))?;
        let ws = self.open(cwd)?;
        let pack_id = pack_id
            .map(ToString::to_string)
            .or_else(|| self.selected_pack_id(cwd).ok());
        ws.events()?.append(
            "task.spawned",
            Some(task.id.to_string()),
            serde_json::json!({
                "pack_id": pack_id,
                "candidates": candidates,
                "cron": cron,
                "instruction": redact_secrets(&instruction)
            }),
        )?;
        let mut runs = Vec::new();
        for candidate in candidates {
            let record = self.ensure_candidate(&ws, &candidate)?;
            let command = render_candidate_command(&record.template, &instruction);
            match self.spawn_run(cwd, task.id.as_str(), &candidate, command) {
                Ok(run) => runs.push(TaskRunSummary {
                    candidate: candidate.clone(),
                    run_id: Some(run.id.to_string()),
                    status: format!("{:?}", run.status),
                    error: None,
                }),
                Err(e) => runs.push(TaskRunSummary {
                    candidate: candidate.clone(),
                    run_id: None,
                    status: "failed".to_string(),
                    error: Some(e.to_string()),
                }),
            }
        }
        Ok(TaskSpawnReport {
            task,
            pack_id,
            cron,
            runs,
        })
    }

    pub fn task_current(&self, cwd: &Path) -> DraftResult<Value> {
        let tasks = self.task_list(cwd)?;
        if let Some(task) = tasks.last() {
            Ok(serde_json::to_value(task).unwrap_or(Value::Null))
        } else {
            Ok(serde_json::json!({ "message": "No running tasks." }))
        }
    }

    pub fn candidate_list(&self, cwd: &Path) -> DraftResult<Vec<CandidateRecord>> {
        let ws = self.open(cwd)?;
        let mut records: Vec<CandidateRecord> = load_json_dir(&ws.layout.candidates_dir())?;
        for builtin in builtin_candidates() {
            if !records.iter().any(|r| r.name == builtin.name) {
                records.push(builtin);
            }
        }
        records.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(records)
    }

    pub fn candidate_show(&self, cwd: &Path, name: &str) -> DraftResult<CandidateRecord> {
        let ws = self.open(cwd)?;
        if let Ok(record) = read_json(&ws.layout.candidates_dir().join(format!("{name}.json"))) {
            return Ok(record);
        }
        builtin_candidates()
            .into_iter()
            .find(|r| r.name == name)
            .ok_or_else(|| DraftError::not_found(format!("unknown candidate '{name}'")))
    }

    pub fn candidate_add(
        &self,
        cwd: &Path,
        name: &str,
        kind: Option<&str>,
        template: Vec<String>,
    ) -> DraftResult<CandidateRecord> {
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
    ) -> DraftResult<CandidateRecord> {
        let existing_kind = self.candidate_show(cwd, name).ok().map(|c| c.kind);
        self.write_candidate(
            cwd,
            name,
            kind.unwrap_or(existing_kind.as_deref().unwrap_or("command")),
            "custom",
            template,
            "candidate.updated",
        )
    }

    pub fn candidate_remove(&self, cwd: &Path, name: &str) -> DraftResult<CandidateRecord> {
        let ws = self.open(cwd)?;
        let mut record = self.candidate_show(cwd, name)?;
        record.active = false;
        write_json(
            &ws.layout.candidates_dir().join(format!("{name}.json")),
            &record,
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
            let run = p
                .run_id
                .as_ref()
                .and_then(|run_id| self.run_show(&ws.root, run_id.as_str()).ok());
            let name = run
                .as_ref()
                .map(|run| run.actor_name.clone())
                .unwrap_or_else(|| {
                    p.run_id
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
                run_id: p.run_id.as_ref().map(ToString::to_string),
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
    ) -> DraftResult<CandidateRecord> {
        let ws = self.open(cwd)?;
        let record = CandidateRecord {
            name: name.to_string(),
            kind: kind.to_string(),
            source: source.to_string(),
            template: template.join(" "),
            role: None,
            persona: None,
            active: true,
        };
        write_json(
            &ws.layout.candidates_dir().join(format!("{name}.json")),
            &record,
        )?;
        ws.events()?.append(
            event,
            Some(name.to_string()),
            serde_json::to_value(&record).unwrap_or(Value::Null),
        )?;
        Ok(record)
    }

    fn ensure_candidate(&self, ws: &Workspace, name: &str) -> DraftResult<CandidateRecord> {
        let path = ws.layout.candidates_dir().join(format!("{name}.json"));
        if path.exists() {
            return read_json(&path);
        }
        let record = builtin_candidates()
            .into_iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| CandidateRecord {
                name: name.to_string(),
                kind: "command".to_string(),
                source: "auto".to_string(),
                template: format!("{name} {{{{instruction}}}}"),
                role: None,
                persona: None,
                active: true,
            });
        write_json(&path, &record)?;
        ws.events()?.append(
            "candidate.auto_registered",
            Some(name.to_string()),
            serde_json::to_value(&record).unwrap_or(Value::Null),
        )?;
        Ok(record)
    }

    pub fn pack_create(
        &self,
        cwd: &Path,
        name: Option<String>,
        task_id: Option<String>,
        from_working_tree: bool,
    ) -> DraftResult<Changepack> {
        let ws = self.open(cwd)?;
        if let Some(name) = name.as_deref() {
            self.ensure_unique_pack_name(&ws, name)?;
        }
        let base = latest_snapshot(&ws)?.unwrap_or_else(|| empty_snapshot(&ws));
        let result = Snapshotter::new(&ws)?.create_snapshot()?;
        let patch = diff_snapshots(&ws, &base, &result)?;
        let evidence = Evidence {
            schema_version: SCHEMA_VERSION,
            id: EvidenceId::generate(),
            changepack_id: ChangepackId::new("pending"),
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
        let mut pack = Changepack::new(
            ws.id.clone(),
            task_id.map(TaskId::new),
            None,
            base.id.clone(),
            result.id.clone(),
            name,
        );
        let mut evidence = evidence;
        evidence.changepack_id = pack.id.clone();
        let pack_dir = ws.layout.pack_dir(&pack.id);
        ensure_dir(&pack_dir)?;
        write_json(&pack_dir.join("manifest.json"), &pack)?;
        write_json(&pack_dir.join("patch.json"), &patch)?;
        write_json(&pack_dir.join("evidence.json"), &evidence)?;
        pack.patch_refs.push(patch.id.to_string());
        pack.evidence_refs.push(evidence.id.to_string());
        pack.manifest_hash = hash_json(&pack)?;
        write_json(&pack_dir.join("manifest.json"), &pack)?;
        ws.events()?.append(
            "pack.created",
            Some(pack.id.to_string()),
            serde_json::to_value(&pack).unwrap_or(Value::Null),
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
        // v0.3.2: materialize the canonical manifest/lockfile/changes and a
        // signed PackCreated trust receipt so every pack is inspectable and
        // exportable from the moment it exists.
        let created_patch = load_patch(&ws, &pack).ok();
        self.sync_canonical_pack(
            &ws,
            &pack,
            created_patch.as_ref(),
            PackSyncSpec {
                kind: crate::event::EventKind::PackCreated,
                intent: crate::pack::PackIntent::Feature,
                approval: crate::pack::ApprovalState::Pending,
                save: crate::pack::SaveState::Unsaved,
                metadata: serde_json::json!({ "name": pack.name }),
            },
        )?;
        Ok(pack)
    }

    pub fn pack_create_from_base(
        &self,
        cwd: &Path,
        name: String,
        base_pack_ref: Option<String>,
    ) -> DraftResult<Changepack> {
        let ws = self.open(cwd)?;
        self.ensure_unique_pack_name(&ws, &name)?;
        let base_ref = match base_pack_ref {
            Some(base) => Some(base),
            None => Some(self.selected_pack_id(cwd)?),
        };
        let mut pack = self.pack_create(cwd, Some(name), None, true)?;
        if let Some(base_ref) = base_ref {
            let ws = self.open(cwd)?;
            let base = self.resolve_pack_ref(&ws, &base_ref)?;
            pack.base_snapshot_id = base.result_snapshot_id.clone();
            save_pack_manifest(&ws, &mut pack)?;
        }
        Ok(pack)
    }

    pub fn pack_select(&self, cwd: &Path, id: &str) -> DraftResult<Changepack> {
        self.pack_select_ref(cwd, id)
    }

    pub fn pack_select_ref(&self, cwd: &Path, reference: &str) -> DraftResult<Changepack> {
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
        let selected = self.selected_pack_id(cwd).ok();
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
        let pack_dir = ws.layout.pack_dir(&pack.id);
        let mut deleted_files = count_files(&pack_dir)?;
        let mut deleted_runs = 0usize;
        let mut deleted_tasks = 0usize;
        if let Some(run_id) = &pack.run_id {
            let run_path = ws.layout.runs_dir().join(format!("{run_id}.json"));
            if run_path.exists() {
                fs::remove_file(&run_path)?;
                deleted_files += 1;
                deleted_runs += 1;
            }
        }
        if let Some(task_id) = &pack.task_id {
            let task_is_still_referenced = active
                .iter()
                .filter(|p| p.id != pack.id)
                .any(|p| p.task_id.as_ref() == Some(task_id));
            let task_path = ws.layout.tasks_dir().join(format!("{task_id}.json"));
            if !task_is_still_referenced && task_path.exists() {
                fs::remove_file(&task_path)?;
                deleted_files += 1;
                deleted_tasks += 1;
            }
        }
        ws.events()?.append(
            "pack.deleted",
            Some(pack.id.to_string()),
            serde_json::json!({
                "name": pack.name,
                "replacement_selected_pack": replacement_id,
                "deleted_files": deleted_files,
                "deleted_runs": deleted_runs,
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

    /// True when `reference` resolves to a legacy changepack (canonical-only
    /// packs, e.g. imports, have none).
    pub fn is_legacy_pack_ref(&self, cwd: &Path, reference: &str) -> bool {
        self.open(cwd)
            .and_then(|ws| self.resolve_pack_ref(&ws, reference))
            .is_ok()
    }

    pub fn resolve_pack_arg(&self, cwd: &Path, pack_id: Option<&str>) -> DraftResult<String> {
        match pack_id {
            Some(id) if !id.trim().is_empty() => {
                let ws = self.open(cwd)?;
                match self.resolve_pack_ref(&ws, id) {
                    Ok(pack) => Ok(pack.id.to_string()),
                    // Canonical-only packs (e.g. quarantined or saved imports)
                    // have no legacy changepack; resolve them against the
                    // canonical pack store and quarantine by id or name.
                    Err(legacy_err) => {
                        let store = crate::pack::PackStore::new(
                            crate::layout::ProjectPaths::for_root(&ws.root),
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
                            _ => Err(legacy_err),
                        }
                    }
                }
            }
            _ => self.selected_pack_id(cwd),
        }
    }

    pub fn pack_list(&self, cwd: &Path) -> DraftResult<Vec<Changepack>> {
        let ws = self.open(cwd)?;
        self.pack_list_for_workspace(&ws)
    }

    fn pack_list_for_workspace(&self, ws: &Workspace) -> DraftResult<Vec<Changepack>> {
        let mut packs = Vec::new();
        if ws.layout.changepacks_dir().exists() {
            for entry in fs::read_dir(ws.layout.changepacks_dir())? {
                let p = entry?.path().join("manifest.json");
                if p.exists() {
                    let pack: Changepack = read_json(&p)?;
                    if pack.active {
                        packs.push(pack);
                    }
                }
            }
        }
        packs.sort_by_key(|a: &Changepack| a.created_at);
        Ok(packs)
    }

    pub fn pack_show(&self, cwd: &Path, id: &str) -> DraftResult<PackReport> {
        let ws = self.open(cwd)?;
        let pack = self.resolve_pack_ref(&ws, id)?;
        let patch = load_patch(&ws, &pack).unwrap_or_else(|_| empty_patch_for_pack(&pack));
        let evidence = load_evidence(&ws, &pack).ok();
        Ok(PackReport {
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

    fn resolve_pack_ref(&self, ws: &Workspace, reference: &str) -> DraftResult<Changepack> {
        if reference.starts_with("pck_") {
            validate_pack_id(reference)?;
            let pack = load_pack(ws, reference)?;
            if !pack.active {
                return Err(DraftError::not_found(format!(
                    "pack '{reference}' is not active"
                )));
            }
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

    pub fn spawn_run(
        &self,
        cwd: &Path,
        task_id: &str,
        name: &str,
        command: Vec<String>,
    ) -> DraftResult<Run> {
        let ws = self.open(cwd)?;
        let base = Snapshotter::new(&ws)?.create_snapshot()?;
        let run_id = RunId::generate();
        ws.events()?.append(
            "task.started",
            Some(run_id.to_string()),
            serde_json::json!({ "task_id": task_id, "name": name, "command": command }),
        )?;
        let started = now();
        let output = if command.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::InvalidConfig,
                "spawn command is empty",
            ));
        } else {
            Command::new(&command[0])
                .args(&command[1..])
                .current_dir(&ws.root)
                .output()
        };
        let (status, stdout_ref, stderr_ref, exit_code) = match output {
            Ok(out) => {
                let store = ObjectStore::new(ws.layout.clone());
                let stdout_ref = store.put_bytes(&out.stdout)?;
                let stderr_ref = store.put_bytes(&out.stderr)?;
                (
                    if out.status.success() {
                        RunStatus::Completed
                    } else {
                        RunStatus::Failed
                    },
                    Some(stdout_ref),
                    Some(stderr_ref),
                    out.status.code(),
                )
            }
            Err(e) => (
                RunStatus::Failed,
                None,
                None,
                Some(-1).filter(|_| {
                    let _ = e;
                    true
                }),
            ),
        };
        let result = Snapshotter::new(&ws)?.create_snapshot()?;
        let run = Run {
            schema_version: SCHEMA_VERSION,
            id: run_id,
            task_id: TaskId::new(task_id),
            workspace_id: ws.id.clone(),
            base_snapshot_id: base.id,
            actor_kind: ActorKind::Agent,
            actor_name: name.to_string(),
            command: Some(command.join(" ")),
            started_at: started,
            ended_at: Some(now()),
            status,
            stdout_ref,
            stderr_ref,
            exit_code,
            result_snapshot_id: Some(result.id),
        };
        write_json(&ws.layout.runs_dir().join(format!("{}.json", run.id)), &run)?;
        ws.events()?.append(
            "task.completed",
            Some(run.id.to_string()),
            serde_json::to_value(&run).unwrap_or(Value::Null),
        )?;
        let mut pack =
            self.pack_create(cwd, Some(name.to_string()), Some(task_id.to_string()), true)?;
        pack.run_id = Some(run.id.clone());
        save_pack_manifest(&ws, &mut pack)?;
        Ok(run)
    }

    pub fn runs(&self, cwd: &Path) -> DraftResult<Vec<Run>> {
        let ws = self.open(cwd)?;
        load_json_dir(&ws.layout.runs_dir())
    }

    pub fn run_show(&self, cwd: &Path, id: &str) -> DraftResult<Run> {
        let ws = self.open(cwd)?;
        read_json(&ws.layout.runs_dir().join(format!("{}.json", id)))
    }

    pub fn verify(&self, cwd: &Path, pack_id: &str) -> DraftResult<VerificationReport> {
        let ws = self.open(cwd)?;
        validate_pack_id(pack_id)?;
        let mut pack = load_pack(&ws, pack_id)?;
        let checks = read_or_default::<VerifyFile>(&ws.layout.verify_toml()).checks;
        ws.events()?.append(
            "verify.started",
            Some(pack.id.to_string()),
            serde_json::json!({ "checks": checks.len() }),
        )?;
        let store = ObjectStore::new(ws.layout.clone());
        let mut results = Vec::new();
        for check in checks
            .into_iter()
            .filter(|c| c.enabled && !c.command.trim().is_empty())
        {
            let start = Instant::now();
            let shell = default_hook_shell_runtime();
            let out = shell_with_env_timeout(
                &check.command,
                &ws.root,
                &BTreeMap::new(),
                check
                    .timeout_seconds
                    .map(|seconds| seconds.saturating_mul(1000)),
                &shell,
            );
            let (exit_code, stdout, stderr) = match out {
                Ok(o) => (o.status.code().unwrap_or(-1), o.stdout, o.stderr),
                Err(e) => (-1, Vec::new(), e.to_string().into_bytes()),
            };
            let stdout = sanitize_output_bytes(&stdout);
            let stderr = sanitize_output_bytes(&stderr);
            results.push(VerificationResult {
                check_name: check.name,
                command_hash: command_hash(&default_shell(), &ws.root, &check.command, ""),
                started_at: now(),
                ended_at: now(),
                duration_ms: start.elapsed().as_millis() as u64,
                exit_code,
                stdout_ref: store.put_bytes(&stdout)?,
                stderr_ref: store.put_bytes(&stderr)?,
                status: if exit_code == 0 {
                    VerificationStatus::Passed
                } else {
                    VerificationStatus::Failed
                },
            });
        }
        if results.is_empty() {
            results.push(VerificationResult::skipped(&store)?);
        }
        let failed = results
            .iter()
            .any(|r| r.status == VerificationStatus::Failed);
        let patch = load_patch(&ws, &pack)?;
        let receipt = Receipt::new(
            "verification",
            if failed { "failed" } else { "passed" },
            Some(pack.id.to_string()),
            serde_json::json!({
                "patch_graph_hash": patch.patch_graph_hash,
                "result_snapshot_id": pack.result_snapshot_id,
                "results": results.clone()
            }),
        );
        write_receipt(&ws, &receipt)?;
        let report = VerificationReport {
            changepack_id: pack.id.to_string(),
            receipt_id: receipt.id.to_string(),
            results,
        };
        pack.verification_refs.push(receipt.id.to_string());
        if !failed && matches!(pack.status, ChangepackStatus::Draft) {
            pack.status = pack.status.transition(ChangepackStatus::Verified)?;
        }
        save_pack_manifest(&ws, &mut pack)?;
        ws.events()?.append(
            "verify.completed",
            Some(pack.id.to_string()),
            serde_json::to_value(&report).unwrap_or(Value::Null),
        )?;
        Ok(report)
    }

    pub fn verify_selected(
        &self,
        cwd: &Path,
        pack_id: Option<&str>,
    ) -> DraftResult<VerificationReport> {
        let pack_id = self.resolve_pack_arg(cwd, pack_id)?;
        self.verify(cwd, &pack_id)
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
        let risk_config = read_or_default::<RiskConfig>(&ws.layout.risk_toml());
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
        let mut receipt = Receipt::new(
            "risk",
            level.label(),
            Some(pack.id.to_string()),
            Value::Null,
        );
        if persist {
            receipt_id = receipt.id.to_string();
        }
        let summary = RiskSummary {
            changepack_id: pack.id.to_string(),
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
            receipt.payload = serde_json::to_value(&summary).unwrap_or(Value::Null);
            receipt.receipt_hash.clear();
            receipt.receipt_hash = hash_json(&receipt)?;
            write_receipt(&ws, &receipt)?;
            ws.events()?.append(
                "risk.completed",
                Some(pack.id.to_string()),
                serde_json::to_value(&summary).unwrap_or(Value::Null),
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
        let mut comments = load_review_file(&ws, &pack.id).unwrap_or_default();
        let risk = self.risk_preview(cwd, pack_id).ok();
        if let Some(body) = comment {
            comments.comments.push(ReviewComment {
                id: ReviewCommentId::generate(),
                changepack_id: pack.id.clone(),
                path: None,
                hunk_id: None,
                actor: resolve_actor(&ws.layout.draft_dir),
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
        if matches!(
            pack.status,
            ChangepackStatus::Draft | ChangepackStatus::Verified
        ) {
            pack.status = pack.status.transition(ChangepackStatus::Reviewed)?;
            save_pack_manifest(&ws, &mut pack)?;
        }
        write_json(
            &ws.layout.pack_dir(&pack.id).join("review.lock.json"),
            &serde_json::json!({
                "pack_id": pack.id,
                "actor": resolve_actor(&ws.layout.draft_dir),
                "updated_at": now()
            }),
        )?;
        save_review_file(&ws, &pack.id, &comments)?;
        let review_units = build_review_units(&ws, &pack, risk.as_ref())?;
        let risk_receipt_id = risk
            .as_ref()
            .and_then(|risk| (risk.receipt_id != "preview").then(|| risk.receipt_id.clone()));
        let receipt = Receipt::new(
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
        save_pack_manifest(&ws, &mut pack)?;
        Ok(ReviewReport {
            changepack_id: pack.id.to_string(),
            review_receipt_id: Some(receipt_id),
            comments: comments.comments.len(),
            decisions: comments.decisions.len(),
            status: pack.status,
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
        if matches!(kind, DecisionKind::Approve | DecisionKind::Reject)
            && !matches!(
                pack.status,
                ChangepackStatus::Reviewed
                    | ChangepackStatus::Approved
                    | ChangepackStatus::Rejected
            )
        {
            return Err(DraftError::new(
                DraftErrorKind::ReviewRequired,
                "review is required before approve/reject",
            ));
        }
        let actor = resolve_actor(&ws.layout.draft_dir);
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
            changepack_id: pack.id.clone(),
            actor,
            kind,
            reason,
            created_at: now(),
        };
        let mut file = load_review_file(&ws, &pack.id).unwrap_or_default();
        file.decisions.push(decision.clone());
        save_review_file(&ws, &pack.id, &file)?;
        pack.decision_refs.push(decision.id.to_string());
        pack.status = match decision.kind {
            DecisionKind::Approve => pack.status.transition(ChangepackStatus::Approved)?,
            DecisionKind::Reject => pack.status.transition(ChangepackStatus::Rejected)?,
            _ => pack.status,
        };
        save_pack_manifest(&ws, &mut pack)?;
        let review_lock = ws.layout.pack_dir(&pack.id).join("review.lock.json");
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
        let receipt = Receipt::new(
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
        ws.events()?.append(
            event,
            Some(pack.id.to_string()),
            serde_json::to_value(&decision).unwrap_or(Value::Null),
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
            serde_json::to_value(&report).unwrap_or(Value::Null),
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
        let l_base = load_snapshot(&ws, &l.base_snapshot_id)?;
        let r_base = load_snapshot(&ws, &r.base_snapshot_id)?;
        if snapshot_file_fingerprint(&l_base) != snapshot_file_fingerprint(&r_base) {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "compose requires changepacks with the same base content",
            ));
        }
        let lp = load_patch(&ws, &l)?;
        let rp = load_patch(&ws, &r)?;
        let cmp = self.compare(cwd, left, right)?;
        if !cmp.compatible {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "compose has overlapping changes",
            )
            .with_context(format!("{:?}", cmp.warnings)));
        }
        let mut files = lp.files.clone();
        files.extend(rp.files.clone());
        files.sort_by(|a, b| a.path.cmp(&b.path).then(a.old_path.cmp(&b.old_path)));
        let mut patch = PatchSet {
            schema_version: SCHEMA_VERSION,
            id: PatchSetId::generate(),
            base_snapshot_id: l.base_snapshot_id.clone(),
            result_snapshot_id: r.result_snapshot_id.clone(),
            files,
            patch_graph_hash: String::new(),
        };
        patch.patch_graph_hash = hash_json(&patch)?;
        let evidence = Evidence {
            schema_version: SCHEMA_VERSION,
            id: EvidenceId::generate(),
            changepack_id: ChangepackId::new("pending"),
            command_logs: vec![],
            files_touched: patch.files.iter().map(|f| f.path.clone()).collect(),
            generated_diff_ref: None,
            test_results: vec![],
            lint_results: vec![],
            risk_summary_ref: None,
            agent_plan_ref: None,
            agent_transcript_ref: None,
            warnings: vec!["composed from compatible changepacks".to_string()],
            created_at: now(),
        };
        let mut pack = Changepack::new(
            ws.id.clone(),
            l.task_id.clone().or_else(|| r.task_id.clone()),
            None,
            l.base_snapshot_id.clone(),
            r.result_snapshot_id.clone(),
            Some(output.to_string()),
        );
        let mut evidence = evidence;
        evidence.changepack_id = pack.id.clone();
        pack.source_pack_ids = vec![l.id.to_string(), r.id.to_string()];
        pack.patch_refs.push(patch.id.to_string());
        pack.evidence_refs.push(evidence.id.to_string());
        let pack_dir = ws.layout.pack_dir(&pack.id);
        ensure_dir(&pack_dir)?;
        write_json(&pack_dir.join("patch.json"), &patch)?;
        write_json(&pack_dir.join("evidence.json"), &evidence)?;
        save_pack_manifest(&ws, &mut pack)?;
        let receipt = Receipt::new(
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
        let mut left = Changepack::new(
            ws.id.clone(),
            source.task_id.clone(),
            source.run_id.clone(),
            source.base_snapshot_id.clone(),
            source.result_snapshot_id.clone(),
            Some(output_a.to_string()),
        );
        left.source_pack_ids = vec![source.id.to_string()];
        let mut right = Changepack::new(
            ws.id.clone(),
            source.task_id.clone(),
            source.run_id.clone(),
            source.base_snapshot_id.clone(),
            source.result_snapshot_id.clone(),
            Some(output_b.to_string()),
        );
        right.source_pack_ids = vec![source.id.to_string()];
        ensure_dir(&ws.layout.pack_dir(&left.id))?;
        ensure_dir(&ws.layout.pack_dir(&right.id))?;
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
            &ws.layout.pack_dir(&left.id).join("patch.json"),
            &left_patch,
        )?;
        write_json(
            &ws.layout.pack_dir(&right.id).join("patch.json"),
            &right_patch,
        )?;
        write_json(
            &ws.layout.pack_dir(&left.id).join("evidence.json"),
            &left_evidence,
        )?;
        write_json(
            &ws.layout.pack_dir(&right.id).join("evidence.json"),
            &right_evidence,
        )?;
        save_pack_manifest(&ws, &mut left)?;
        save_pack_manifest(&ws, &mut right)?;
        let receipt = Receipt::new(
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

    pub fn save(
        &self,
        cwd: &Path,
        pack_id: &str,
        vars: BTreeMap<String, String>,
    ) -> DraftResult<SaveReceipt> {
        let ws = self.open(cwd)?;
        validate_pack_id(pack_id)?;
        // Imported packs take the canonical import-save path: gates, content
        // application from embedded objects, and promotion out of quarantine.
        {
            let store =
                crate::pack::PackStore::new(crate::layout::ProjectPaths::for_root(&ws.root));
            if let Some(loc) = store.locate(pack_id) {
                let manifest = store.read_manifest_in(loc, pack_id)?;
                if manifest.import_state != crate::pack::ImportState::None {
                    return self.save_imported_pack(&ws, &store, loc, manifest);
                }
            }
        }
        let mut pack = load_pack(&ws, pack_id)?;
        ensure_pack_not_locked(&ws, &pack)?;
        let started = now();
        let save_started_event_id = ws.events()?.append(
            "save.started",
            Some(pack.id.to_string()),
            serde_json::json!({}),
        )?;
        let cfg = ResolvedConfig::load(&ws)?;
        let policy = read_or_default::<PolicyConfig>(&ws.layout.policy_toml());
        let patch = load_patch(&ws, &pack)?;
        if patch.files.iter().any(|f| is_draft_path(f.path.as_str())) {
            let receipt = failed_save(
                &ws,
                &pack,
                started,
                "Warning: .draft/ is included in the save candidate.",
            )?;
            ws.events()?.append(
                "save.completed",
                Some(pack.id.to_string()),
                serde_json::to_value(&receipt).unwrap_or(Value::Null),
            )?;
            return Err(DraftError::new(DraftErrorKind::SaveFailed, "Warning: .draft/ is included in the save candidate.\n\nDraft metadata must never be saved into an external repository or external system.\n\nSave aborted."));
        }
        let readiness = save_readiness(&ws, &pack, &patch, &policy)?;
        if policy.save.block_if_tests_fail && readiness.verification_receipt_id.is_none() {
            let reason = readiness
                .blockers
                .first()
                .cloned()
                .unwrap_or_else(|| "verification is required before save".to_string());
            let receipt = failed_save(&ws, &pack, started, &reason)?;
            ws.events()?.append(
                "save.completed",
                Some(pack.id.to_string()),
                serde_json::to_value(&receipt).unwrap_or(Value::Null),
            )?;
            return Err(DraftError::new(DraftErrorKind::VerificationFailed, reason));
        }
        if policy.save.block_if_unreviewed_high_risk && readiness.approval_ref.is_none() {
            let reason = readiness
                .blockers
                .iter()
                .find(|blocker| blocker.contains("approval") || blocker.contains("review"))
                .cloned()
                .unwrap_or_else(|| "approval is required before save".to_string());
            let receipt = failed_save(&ws, &pack, started, &reason)?;
            ws.events()?.append(
                "save.completed",
                Some(pack.id.to_string()),
                serde_json::to_value(&receipt).unwrap_or(Value::Null),
            )?;
            return Err(DraftError::new(DraftErrorKind::ReviewRequired, reason));
        }
        let risk_summary = Some(self.risk(cwd, pack_id).map_err(|e| {
            DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                format!("risk evaluation failed before save: {e}"),
            )
        })?);
        if risk_summary
            .as_ref()
            .map(|risk| risk.policy_decision.starts_with("blocked"))
            .unwrap_or(false)
        {
            let receipt = failed_save(&ws, &pack, started, "risk policy blocks save")?;
            ws.events()?.append(
                "save.completed",
                Some(pack.id.to_string()),
                serde_json::to_value(&receipt).unwrap_or(Value::Null),
            )?;
            return Err(DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                "risk policy blocks save",
            ));
        }
        validate_canonical_save_gate(&ws, pack_id)?;
        let receipt_id = ReceiptId::generate();
        let rendered_message = render_message(&cfg, &pack, &patch, &receipt_id);
        let store = ObjectStore::new(ws.layout.clone());
        let message_ref = store.put_bytes(rendered_message.as_bytes())?;
        let mut receipt = SaveReceipt {
            schema_version: SCHEMA_VERSION,
            id: receipt_id,
            changepack_id: pack.id.clone(),
            actor: resolve_actor(&ws.layout.draft_dir),
            native_save_status: NativeSaveStatus::Saved,
            hook_status: HookStatus::NotConfigured,
            overall_status: SaveOverallStatus::Saved,
            message_ref: message_ref.clone(),
            hook_results: Vec::new(),
            hook_receipt_refs: Vec::new(),
            object_refs: vec![message_ref.clone()],
            event_refs: vec![save_started_event_id.to_string()],
            risk_level: risk_summary
                .as_ref()
                .map(|risk| risk.level.label().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            risk_receipt_id: risk_summary.as_ref().map(|risk| risk.receipt_id.clone()),
            started_at: started,
            ended_at: now(),
            receipt_hash: String::new(),
            failure_reason: None,
        };
        if let Some(hook) = cfg.hook("save") {
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
                run_id: pack
                    .run_id
                    .as_ref()
                    .map(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                changepack_id: pack.id.to_string(),
                receipt_id: receipt.id.to_string(),
                actor_name: cfg.identity_username.clone(),
                actor_email: cfg.identity_email.clone(),
                timestamp: now().to_rfc3339(),
                verified: (!pack.verification_refs.is_empty()).to_string(),
                risk_level: risk_summary
                    .as_ref()
                    .map(|risk| risk.level.label().to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                files_changed: patch.files.len().to_string(),
                workspace_root: ws.root.display().to_string(),
                hook_name: "save".to_string(),
                hook_phase: hook.phase.clone(),
                vars,
            };
            match run_hook(&ws, &store, "save", &hook, &ctx) {
                Ok(result) => {
                    let failed = result.exit_code != 0;
                    let hook_receipt = Receipt::new(
                        "hook",
                        if failed { "failed" } else { "succeeded" },
                        Some(pack.id.to_string()),
                        serde_json::to_value(&result).unwrap_or(Value::Null),
                    );
                    let hook_receipt_id = hook_receipt.id.to_string();
                    write_receipt(&ws, &hook_receipt)?;
                    receipt.hook_receipt_refs.push(hook_receipt_id);
                    receipt.hook_results.push(result);
                    if failed {
                        receipt.hook_status = HookStatus::Failed;
                        if hook.continue_on_error {
                            receipt.overall_status = SaveOverallStatus::SavedWithHookFailure;
                        } else {
                            receipt.overall_status = SaveOverallStatus::Failed;
                            receipt.failure_reason = Some("hooks.save failed".to_string());
                            receipt.ended_at = now();
                            receipt.receipt_hash = hash_json(&receipt)?;
                            write_save_receipt(&ws, &receipt)?;
                            ws.events()?.append(
                                "save.completed",
                                Some(pack.id.to_string()),
                                serde_json::to_value(&receipt).unwrap_or(Value::Null),
                            )?;
                            return Err(DraftError::new(
                                DraftErrorKind::SaveFailed,
                                "hooks.save failed",
                            ));
                        }
                    } else {
                        receipt.hook_status = HookStatus::Succeeded;
                    }
                }
                Err(e) => {
                    receipt.hook_status = HookStatus::Failed;
                    let hook_receipt = Receipt::new(
                        "hook",
                        "failed",
                        Some(pack.id.to_string()),
                        serde_json::json!({
                            "hook_name": "save",
                            "hook_phase": hook.phase,
                            "error": e.message
                        }),
                    );
                    let hook_receipt_id = hook_receipt.id.to_string();
                    write_receipt(&ws, &hook_receipt)?;
                    receipt.hook_receipt_refs.push(hook_receipt_id);
                    if hook.continue_on_error {
                        receipt.overall_status = SaveOverallStatus::SavedWithHookFailure;
                        receipt.failure_reason = Some(e.message);
                    } else {
                        receipt.overall_status = SaveOverallStatus::Failed;
                        receipt.failure_reason = Some(e.message.clone());
                        receipt.ended_at = now();
                        receipt.receipt_hash = hash_json(&receipt)?;
                        write_save_receipt(&ws, &receipt)?;
                        ws.events()?.append(
                            "save.completed",
                            Some(pack.id.to_string()),
                            serde_json::to_value(&receipt).unwrap_or(Value::Null),
                        )?;
                        return Err(DraftError::new(DraftErrorKind::SaveFailed, e.message));
                    }
                }
            }
        }
        receipt.ended_at = now();
        receipt.receipt_hash = hash_json(&receipt)?;
        write_save_receipt(&ws, &receipt)?;
        pack.receipt_refs.push(receipt.id.to_string());
        pack.status = pack.status.transition(ChangepackStatus::Saved)?;
        save_pack_manifest(&ws, &mut pack)?;
        ws.events()?.append(
            "save.completed",
            Some(pack.id.to_string()),
            serde_json::to_value(&receipt).unwrap_or(Value::Null),
        )?;
        // v0.3.2: emit the canonical pack manifest/lockfile and a signed
        // PackSaved trust receipt bound to the current workspace hash.
        self.sync_canonical_pack(
            &ws,
            &pack,
            Some(&patch),
            PackSyncSpec {
                kind: crate::event::EventKind::PackSaved,
                intent: crate::pack::PackIntent::Feature,
                approval: crate::pack::ApprovalState::Approved,
                save: crate::pack::SaveState::Saved,
                metadata: serde_json::json!({ "save_receipt": receipt.id.to_string() }),
            },
        )?;
        Ok(receipt)
    }

    /// Upsert the canonical v0.3.2 `manifest.json`/`pack.lock.json` for `pack`
    /// and record a signed trust receipt for `kind`. Purely additive to the
    /// legacy changepack store; never alters legacy return values.
    fn sync_canonical_pack(
        &self,
        ws: &Workspace,
        pack: &Changepack,
        patch: Option<&PatchSet>,
        spec: PackSyncSpec,
    ) -> DraftResult<String> {
        use crate::pack::{ImportState, PackLockfile, PackManifest, PackStore};
        let PackSyncSpec {
            kind,
            intent,
            approval,
            save,
            metadata,
        } = spec;
        let paths = crate::layout::ProjectPaths::for_root(&ws.root);
        let store = PackStore::new(paths);
        let workspace_hash = crate::hashing::workspace_hash(&ws.root)?;
        let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
        let outcome = ledger.record(
            kind,
            Some(pack.id.to_string()),
            None,
            workspace_hash.clone(),
            metadata,
        )?;
        let receipt_hash = crate::hashing::sha256_hex(outcome.receipt.receipt_id.as_bytes());

        let existing = store.read_manifest(pack.id.as_str()).ok();
        let created_at = existing
            .as_ref()
            .map(|m| m.created_at.clone())
            .unwrap_or_else(|| now().to_rfc3339());
        let base_ws = existing
            .as_ref()
            .map(|m| m.base_workspace_hash.clone())
            .unwrap_or_else(|| workspace_hash.clone());
        let target_ws = if patch.is_some() {
            workspace_hash.clone()
        } else {
            existing
                .as_ref()
                .map(|m| m.target_workspace_hash.clone())
                .unwrap_or_else(|| workspace_hash.clone())
        };
        let mut receipt_hashes = existing
            .as_ref()
            .map(|m| m.receipt_hashes.clone())
            .unwrap_or_default();
        receipt_hashes.push(receipt_hash);
        // Serialize the patch once; the manifest's changes_hash is the hash of
        // the exact bytes written to changes.patch so import can re-verify it.
        let changes_bytes: Option<Vec<u8>> =
            patch.map(|p| serde_json::to_vec_pretty(p).unwrap_or_default());
        let changes_hash = changes_bytes
            .as_ref()
            .map(|b| sha256_hex(b))
            .or_else(|| existing.as_ref().map(|m| m.changes_hash.clone()))
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| sha256_hex(b""));
        let verify_hash = if !pack.verification_refs.is_empty() {
            sha256_hex(pack.verification_refs.join(",").as_bytes())
        } else {
            existing
                .as_ref()
                .map(|m| m.verify_hash.clone())
                .unwrap_or_default()
        };
        let manifest = PackManifest {
            schema_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            pack_id: pack.id.to_string(),
            name: pack.name.clone().unwrap_or_else(|| pack.id.to_string()),
            description: existing
                .as_ref()
                .map(|m| m.description.clone())
                .unwrap_or_default(),
            intent,
            origin: "local".to_string(),
            actor: ledger.actor_id().to_string(),
            candidate: None,
            created_at,
            base_workspace_hash: base_ws,
            target_workspace_hash: target_ws,
            changes_hash,
            risk_hash: existing
                .as_ref()
                .map(|m| m.risk_hash.clone())
                .unwrap_or_default(),
            verify_hash,
            lsif_hash: existing
                .as_ref()
                .map(|m| m.lsif_hash.clone())
                .unwrap_or_default(),
            receipt_hashes: receipt_hashes.clone(),
            import_state: ImportState::None,
            approval_state: approval,
            save_state: save,
        };
        store.write_manifest(&manifest)?;
        if let Some(bytes) = &changes_bytes {
            let paths2 = crate::layout::ProjectPaths::for_root(&ws.root);
            write_atomic(&paths2.pack_changes(pack.id.as_str()), bytes)?;
        }

        if let Some(p) = patch {
            let mut file_hashes = std::collections::BTreeMap::new();
            for f in &p.files {
                let fp = ws.root.join(f.path.as_str());
                if let Ok(bytes) = std::fs::read(&fp) {
                    file_hashes.insert(f.path.to_string(), sha256_hex(&bytes));
                }
            }
            let lock = PackLockfile {
                schema_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
                pack_id: pack.id.to_string(),
                workspace_hash,
                file_hashes,
                policy_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
                risk_engine_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
                verification_commands: Vec::new(),
                lsif_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
                test_selector_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
                fuzz_selector_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
                dependency_pack_hashes: Vec::new(),
                receipt_hashes,
            };
            store.write_lockfile(&lock)?;
        }
        Ok(outcome.receipt.receipt_id)
    }

    /// Save an imported pack: enforce the import gates, apply the embedded
    /// content to the workspace (fail closed, nothing written on any
    /// conflict), and promote the pack out of quarantine.
    ///
    /// Save hooks do not run for import saves — there is no rendered save
    /// message/diff context for an imported pack.
    fn save_imported_pack(
        &self,
        ws: &Workspace,
        store: &crate::pack::PackStore,
        loc: crate::pack::PackLocation,
        manifest: crate::pack::PackManifest,
    ) -> DraftResult<SaveReceipt> {
        use crate::pack::ImportState;
        let started = now();
        let pack_id = manifest.pack_id.clone();
        let dir = store.dir_for(loc, &pack_id);

        // State gate with actionable, state-specific errors.
        match manifest.import_state {
            ImportState::ImportApproved => {}
            ImportState::ImportedQuarantined => {
                return Err(DraftError::new(
                    DraftErrorKind::ReviewRequired,
                    "imported packs must be locally verified and approved before save",
                )
                .with_suggestion("run `draft verify <pck_id>`, then approve it"));
            }
            ImportState::ImportVerified => {
                return Err(DraftError::new(
                    DraftErrorKind::ReviewRequired,
                    "imported packs must be approved before save",
                ));
            }
            ImportState::ImportRejected => {
                return Err(DraftError::invalid_config(
                    "a rejected import cannot be saved",
                ));
            }
            ImportState::ImportSaved => {
                return Err(DraftError::invalid_config(
                    "this imported pack is already saved",
                ));
            }
            ImportState::None => unreachable!("save_imported_pack requires an imported pack"),
        }

        // Policy gates.
        let policy = effective_policy(ws)?;
        if policy.require_local_verify_for_imports && !manifest.is_verified() {
            return Err(DraftError::new(
                DraftErrorKind::VerificationFailed,
                "imported packs must be locally re-verified before save",
            )
            .with_suggestion("run `draft verify <pck_id>` first"));
        }
        let risk_path = dir.join("risk.json");
        let mut risk_level = "unknown".to_string();
        if risk_path.exists() {
            let risk: crate::riskv2::RiskReport = read_json(&risk_path)?;
            risk_level = risk.risk_level.as_str().to_string();
            if policy.block_on_critical_risk
                && risk.risk_level == crate::riskv2::RiskLevel::Critical
            {
                return Err(DraftError::new(
                    DraftErrorKind::RiskPolicyBlocked,
                    "unresolved critical risk blocks save",
                ));
            }
        } else if policy.block_on_critical_risk {
            return Err(DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                "no local risk report exists for this imported pack",
            )
            .with_suggestion("run `draft verify <pck_id>` before save"));
        }
        if policy.require_reverify_on_workspace_change {
            let current = crate::hashing::workspace_hash(&ws.root)?;
            if manifest.target_workspace_hash != current {
                return Err(DraftError::new(
                    DraftErrorKind::VerificationFailed,
                    "workspace content changed after the import was verified",
                )
                .with_suggestion("run `draft verify <pck_id>` again before save"));
            }
        }
        let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
        if !ledger.verify_all()?.all_ok {
            return Err(DraftError::new(
                DraftErrorKind::OperationLogCorrupt,
                "canonical event, receipt, or transparency ledger failed verification",
            )
            .with_suggestion("run `draft receipt verify --all` or `draft doctor`"));
        }

        // Integrity + application plan (validate everything before writing).
        let changes_bytes = fs::read(dir.join("changes.patch"))?;
        if sha256_hex(&changes_bytes) != manifest.changes_hash {
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
        self.checkpoint(&ws.root, &format!("pre-import-save {pack_id}"))?;

        ws.events()?.append(
            "save.started",
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

        // Promote out of quarantine and finalize the manifest.
        if loc == crate::pack::PackLocation::Quarantine {
            store.promote_from_quarantine(&pack_id)?;
        }
        let wsh_after = crate::hashing::workspace_hash(&ws.root)?;
        let mut manifest = manifest;
        manifest.import_state = ImportState::ImportSaved;
        manifest.save_state = crate::pack::SaveState::Saved;
        manifest.target_workspace_hash = wsh_after.clone();
        store.write_manifest(&manifest)?;

        ledger.record(
            crate::event::EventKind::PackSaved,
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

        // Legacy-parity save receipt so every caller of the save surface
        // (CLI, daemon, cockpit) gets the same artifact shape.
        let object_store = ObjectStore::new(ws.layout.clone());
        let mut receipt = SaveReceipt {
            schema_version: SCHEMA_VERSION,
            id: ReceiptId::generate(),
            changepack_id: ChangepackId::new(pack_id.clone()),
            actor: resolve_actor(&ws.layout.draft_dir),
            native_save_status: NativeSaveStatus::Saved,
            hook_status: HookStatus::NotConfigured,
            overall_status: SaveOverallStatus::Saved,
            message_ref: object_store.put_bytes(manifest.name.as_bytes())?,
            hook_results: Vec::new(),
            hook_receipt_refs: Vec::new(),
            object_refs: Vec::new(),
            event_refs: Vec::new(),
            risk_level,
            risk_receipt_id: None,
            started_at: started,
            ended_at: now(),
            receipt_hash: String::new(),
            failure_reason: None,
        };
        receipt.receipt_hash = hash_json(&receipt)?;
        write_save_receipt(ws, &receipt)?;
        ws.events()?.append(
            "save.completed",
            Some(pack_id),
            serde_json::to_value(&receipt).unwrap_or(Value::Null),
        )?;
        Ok(receipt)
    }

    pub fn save_selected(
        &self,
        cwd: &Path,
        pack_id: Option<&str>,
        vars: BTreeMap<String, String>,
    ) -> DraftResult<SaveReceipt> {
        let pack_id = self.resolve_pack_arg(cwd, pack_id)?;
        self.save(cwd, &pack_id, vars)
    }

    pub fn save_readiness_selected(
        &self,
        cwd: &Path,
        pack_id: Option<&str>,
    ) -> DraftResult<SaveReadinessReport> {
        let pack_id = self.resolve_pack_arg(cwd, pack_id)?;
        let ws = self.open(cwd)?;
        let pack = load_pack(&ws, &pack_id)?;
        let patch = load_patch(&ws, &pack)?;
        let policy = read_or_default::<PolicyConfig>(&ws.layout.policy_toml());
        save_readiness(&ws, &pack, &patch, &policy)
    }

    pub fn rollback_plan(&self, cwd: &Path, reference: &str) -> DraftResult<RollbackPlan> {
        let ws = self.open(cwd)?;
        let snapshot = resolve_snapshot_reference(&ws, reference)?;
        let current = Snapshotter::new(&ws)?.create_snapshot()?;
        let patch = diff_snapshot_values(&snapshot, &current);
        Ok(RollbackPlan {
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

    pub fn rollback(&self, cwd: &Path, reference: &str, yes: bool) -> DraftResult<RollbackReceipt> {
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
            serde_json::to_value(&plan).unwrap_or(Value::Null),
        )?;
        let snap = load_snapshot(&ws, &plan.rollback_snapshot_id)?;
        restore_snapshot(&ws, &snap)?;
        let mut receipt = RollbackReceipt {
            schema_version: SCHEMA_VERSION,
            id: ReceiptId::generate(),
            rollback_plan_id: plan.id,
            actor: resolve_actor(&ws.layout.draft_dir),
            status: "completed".to_string(),
            started_at: now(),
            ended_at: now(),
            receipt_hash: String::new(),
        };
        write_rollback_receipt(&ws, &mut receipt)?;
        ws.events()?.append(
            "rollback.completed",
            Some(receipt.id.to_string()),
            serde_json::to_value(&receipt).unwrap_or(Value::Null),
        )?;
        // v0.3.2: record a signed RollbackPerformed trust receipt.
        let workspace_hash = crate::hashing::workspace_hash(&ws.root)?;
        let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
        ledger.record(
            crate::event::EventKind::RollbackPerformed,
            Some(reference.to_string()),
            None,
            workspace_hash,
            serde_json::json!({
                "rollback_receipt": receipt.id.to_string(),
                "snapshot": snap.id.to_string(),
            }),
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

    /// `draft save --dry-run`: report whether the pack would save and why,
    /// without writing anything.
    pub fn save_dry_run(&self, cwd: &Path, pack_id: Option<&str>) -> DraftResult<DryRunReport> {
        let pack_id = self.resolve_pack_arg(cwd, pack_id)?;
        let ws = self.open(cwd)?;
        {
            let store =
                crate::pack::PackStore::new(crate::layout::ProjectPaths::for_root(&ws.root));
            if let Some(loc) = store.locate(&pack_id) {
                let manifest = store.read_manifest_in(loc, &pack_id)?;
                if manifest.import_state != crate::pack::ImportState::None {
                    return self.import_save_dry_run(&ws, &store, loc, manifest);
                }
            }
        }
        let pack = load_pack(&ws, &pack_id)?;
        let patch = load_patch(&ws, &pack)?;
        let policy = read_or_default::<PolicyConfig>(&ws.layout.policy_toml());
        let readiness = save_readiness(&ws, &pack, &patch, &policy)?;
        let mut checks = Vec::new();
        let draft_touch = patch.files.iter().any(|f| is_draft_path(f.path.as_str()));
        checks.push(bool_check(
            "draft-exclusion",
            !draft_touch,
            ".draft/ not in candidate",
            ".draft/ present in save candidate",
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
            action: "save".to_string(),
            target: pack_id,
            would_proceed: would,
            resulting_state: if would {
                "saved".to_string()
            } else {
                "blocked".to_string()
            },
            affected_files: patch.files.iter().map(|f| f.path.to_string()).collect(),
            checks,
        })
    }

    /// `draft save --dry-run` for an imported pack: report the import gates
    /// and whether the embedded content would apply cleanly.
    fn import_save_dry_run(
        &self,
        ws: &Workspace,
        store: &crate::pack::PackStore,
        loc: crate::pack::PackLocation,
        manifest: crate::pack::PackManifest,
    ) -> DraftResult<DryRunReport> {
        use crate::pack::ImportState;
        let dir = store.dir_for(loc, &manifest.pack_id);
        let mut checks = Vec::new();
        checks.push(bool_check(
            "locally-verified",
            manifest.is_verified(),
            "local verification evidence present",
            "imported pack is not locally verified",
        ));
        checks.push(bool_check(
            "approved",
            manifest.import_state == ImportState::ImportApproved,
            "import approved",
            "imported pack is not approved",
        ));
        let workspace_unchanged = crate::hashing::workspace_hash(&ws.root)
            .map(|h| h == manifest.target_workspace_hash)
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
                if sha256_hex(&b) != manifest.changes_hash {
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
            action: "save".to_string(),
            target: manifest.pack_id,
            would_proceed: would,
            resulting_state: if would {
                "import applied and saved".to_string()
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
        let store = crate::pack::PackStore::new(crate::layout::ProjectPaths::for_root(&ws.root));
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
        // Fall back to the legacy resolver (name/selected).
        Ok(self.resolve_pack_ref(ws, reference)?.id.to_string())
    }

    /// `draft pack --export <pck_id|name> [--output <path>]`.
    pub fn pack_export(
        &self,
        cwd: &Path,
        reference: &str,
        output: Option<&Path>,
    ) -> DraftResult<PackExportReport> {
        use crate::importexport::{DraftpackHeader, Provenance, DRAFTPACK_FORMAT};
        let ws = self.open(cwd)?;
        let pack_id = self.resolve_canonical_pack_ref(&ws, reference)?;
        let paths = crate::layout::ProjectPaths::for_root(&ws.root);
        let store = crate::pack::PackStore::new(paths.clone());
        let manifest = store.read_manifest(&pack_id)?;

        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        let header = DraftpackHeader {
            format: DRAFTPACK_FORMAT.to_string(),
            draft_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            pack_id: manifest.pack_id.clone(),
            name: manifest.name.clone(),
            exported_at: now().to_rfc3339(),
        };
        entries.push(("draftpack.json".into(), to_pretty(&header)?));
        entries.push(("manifest.json".into(), to_pretty(&manifest)?));
        if let Ok(lock) = store.read_lockfile(&pack_id) {
            entries.push(("pack.lock.json".into(), to_pretty(&lock)?));
        }
        if paths.pack_changes(&pack_id).exists() {
            let changes_bytes = fs::read(paths.pack_changes(&pack_id))?;
            // Embed the content-addressed objects referenced by the patch
            // (new file contents + hunk bodies) so the pack is portable: an
            // importing workspace can re-verify content and apply it on save.
            if let Ok(patch) = serde_json::from_slice::<PatchSet>(&changes_bytes) {
                let object_store = ObjectStore::new(ws.layout.clone());
                let mut refs = std::collections::BTreeSet::new();
                for f in &patch.files {
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
                        DraftError::storage(format!("unsupported object ref '{object_ref}'"))
                    })?;
                    let bytes = object_store.get_bytes(&object_ref)?;
                    entries.push((format!("objects/{hex}"), bytes));
                }
            }
            entries.push(("changes.patch".into(), changes_bytes));
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
        let rstore = crate::receipt::ReceiptStore::new(paths.clone());
        let mut external_receipt_ids = Vec::new();
        for r in rstore.list()? {
            if r.subject_id.as_deref() == Some(pack_id.as_str()) {
                entries.push((format!("receipts/{}.json", r.receipt_id), to_pretty(&r)?));
                external_receipt_ids.push(r.receipt_id.clone());
            }
        }
        let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
        let provenance = Provenance {
            origin: "local".to_string(),
            exported_by_actor: ledger.actor_id().to_string(),
            source_workspace_hash: manifest.target_workspace_hash.clone(),
            external_receipt_ids,
        };
        entries.push(("provenance.json".into(), to_pretty(&provenance)?));

        let out = match output {
            Some(p) => p.to_path_buf(),
            None => {
                ensure_dir(&paths.exports_dir())?;
                paths
                    .exports_dir()
                    .join(format!("{}.draftpack", manifest.name))
            }
        };
        crate::importexport::write_archive(&out, &entries)?;
        let wsh = crate::hashing::workspace_hash(&ws.root)?;
        ledger.record(
            crate::event::EventKind::PackExported,
            Some(pack_id.clone()),
            None,
            wsh,
            serde_json::json!({ "output": out.display().to_string() }),
        )?;
        Ok(PackExportReport {
            pack_id,
            name: manifest.name,
            output: out.display().to_string(),
            bytes: fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
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
        use crate::importexport::{DraftpackHeader, DRAFTPACK_FORMAT};
        use crate::pack::{ImportState, PackManifest, PackStore};
        let ws = self.open(cwd)?;
        let paths = crate::layout::ProjectPaths::for_root(&ws.root);
        let store = PackStore::new(paths.clone());

        // Full security validation happens inside read_archive (fail closed).
        let archive = crate::importexport::read_archive(path)?;
        let header: DraftpackHeader = archive.read_json("draftpack.json")?;
        if header.format != DRAFTPACK_FORMAT {
            return Err(DraftError::invalid_config(format!(
                "unsupported .draftpack format '{}'",
                header.format
            )));
        }
        let mut manifest: PackManifest = archive.read_json("manifest.json")?;
        manifest.ensure_supported()?;
        // Content integrity: changes.patch must match the manifest changes_hash.
        if let Some(changes) = archive.get("changes.patch") {
            let recomputed = sha256_hex(changes);
            if recomputed != manifest.changes_hash {
                return Err(DraftError::invalid_config(
                    "changes hash mismatch: manifest.changes_hash does not match changes.patch",
                ));
            }
        }

        let target_name = new_name
            .map(|s| s.to_string())
            .unwrap_or_else(|| manifest.name.clone());
        // Uniqueness spans both saved packs and already-quarantined imports.
        let name_taken =
            store.name_taken(&target_name)? || quarantine_names(&paths).contains(&target_name);
        if name_taken {
            let hint = if new_name.is_some() {
                format!("name '{target_name}' already exists; choose another --name")
            } else {
                format!("duplicate pack name '{target_name}'; import with --name <unique>")
            };
            return Err(DraftError::invalid_config(hint));
        }

        let mut target_id = manifest.pack_id.clone();
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
            serde_json::from_slice::<crate::receipt::ReceiptRecord>(bytes).map_err(|e| {
                DraftError::invalid_config(format!(
                    "corrupt or wrong-schema receipt '{entry_name}' in artifact: {e}"
                ))
            })?;
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

        // Extract into the quarantine using the path guard for defense in depth.
        let qdir = paths.quarantine_dir().join(&target_id);
        ensure_dir(&qdir)?;
        for (name, bytes) in &archive.entries {
            if name == "manifest.json" {
                continue; // rewritten below with quarantine state
            }
            let dest = crate::pathguard::safe_join(&qdir, name)
                .map_err(|v| DraftError::invalid_config(format!("unsafe entry {name}: {v}")))?;
            if let Some(parent) = dest.parent() {
                ensure_dir(parent)?;
            }
            write_atomic(&dest, bytes)?;
        }
        manifest.pack_id = target_id.clone();
        manifest.name = target_name.clone();
        manifest.import_state = ImportState::ImportedQuarantined;
        // Origin trust marks never carry over: the pack must be locally
        // re-verified and re-approved. The origin's risk/verify/lsif files and
        // receipts stay on disk in quarantine as provenance only.
        manifest.verify_hash = String::new();
        manifest.risk_hash = String::new();
        manifest.lsif_hash = String::new();
        manifest.approval_state = crate::pack::ApprovalState::Pending;
        manifest.save_state = crate::pack::SaveState::Unsaved;
        write_json(&qdir.join("manifest.json"), &manifest)?;

        let wsh = crate::hashing::workspace_hash(&ws.root)?;
        let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
        ledger.record(
            crate::event::EventKind::PackImported,
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
    pub fn verify_pack_v2(
        &self,
        cwd: &Path,
        pack_ref: &str,
        full: bool,
        fuzz: bool,
    ) -> DraftResult<VerifyV2Report> {
        use crate::lsif::{LsifIndex, LSIF_BACKEND};
        use crate::pack::PackStore;
        let ws = self.open(cwd)?;
        let pack_id = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        let paths = crate::layout::ProjectPaths::for_root(&ws.root);
        let store = PackStore::new(paths.clone());
        let loc = store
            .locate(&pack_id)
            .unwrap_or(crate::pack::PackLocation::Store);
        let manifest = store.read_manifest_in(loc, &pack_id)?;

        // Policy may escalate verification scope for sensitive intents
        // (e.g. `security` requires the full suite and fuzzing).
        let policy = effective_policy(&ws)?;
        let intent_label = manifest.intent.as_str();
        let full = full || policy.intent_requires_full_verify(intent_label);
        let fuzz = fuzz || policy.intent_requires_fuzz(intent_label);

        // Imported packs are verified from their embedded, content-addressed
        // artifacts, never from origin evidence.
        if manifest.import_state != crate::pack::ImportState::None {
            return self.verify_imported_pack(&ws, &paths, &store, loc, manifest, full, fuzz);
        }
        let pack = self.resolve_pack_ref(&ws, &pack_id)?;

        // Changed files (content) — never include `.draft/`.
        let patch = load_patch(&ws, &pack)?;
        let changed: Vec<(String, String)> = patch
            .files
            .iter()
            .filter(|f| !is_draft_path(f.path.as_str()))
            .map(|f| {
                let content = fs::read_to_string(ws.root.join(f.path.as_str())).unwrap_or_default();
                (f.path.to_string(), content)
            })
            .collect();
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
        let fuzz_targets = scan_fuzz_targets(&ws.root);

        // Risk.
        let ledger_events = crate::event::EventLog::new(paths.clone())
            .read_all()
            .unwrap_or_default();
        let all_manifests = store.list()?;
        let risk_inputs = crate::riskv2::RiskInputs {
            intent: manifest.intent,
            files_touched: changed.len(),
            lines_changed: changed.iter().map(|(_, c)| c.lines().count()).sum(),
            high_risk_paths: crate::riskv2::high_risk_paths(&changed_paths),
            has_tests: !test_files.is_empty(),
            has_fuzz: fuzz && !fuzz_targets.is_empty(),
            public_api_changes: public_api.len(),
            imported: manifest.import_state != crate::pack::ImportState::None,
            dependency_count: store
                .read_lockfile(&pack_id)
                .map(|l| l.dependency_pack_hashes.len())
                .unwrap_or(0),
            semantic_impact: changed_symbols.len(),
            candidate_rollback_rate: manifest
                .candidate
                .as_deref()
                .map(|c| candidate_rollback_rate(&ledger_events, &all_manifests, c))
                .unwrap_or(0.0),
        };
        let risk = crate::riskv2::assess(&risk_inputs);

        // Selection evidence.
        let selection = crate::verifyv2::SelectionInput {
            changed_files: changed_paths.clone(),
            changed_symbols: changed_symbols.clone(),
            test_files: test_files.clone(),
            fuzz_targets: fuzz_targets.clone(),
            full,
            fuzz,
        };
        let evidence = crate::verifyv2::plan(&selection);

        // Persist canonical evidence.
        write_json(&paths.pack_risk(&pack_id), &risk)?;
        write_json(&paths.pack_verify(&pack_id), &evidence)?;
        let lsif_summary = serde_json::json!({
            "backend": LSIF_BACKEND,
            "symbols_touched": changed_symbols,
            "public_api_changed": public_api,
            "tests_referencing": test_files,
            "semantic_impact": changed_symbols.len(),
        });
        write_json(&paths.pack_lsif(&pack_id), &lsif_summary)?;

        // Update manifest hashes and record PackVerified.
        let wsh = crate::hashing::workspace_hash(&ws.root)?;
        let mut manifest = manifest;
        manifest.target_workspace_hash = wsh.clone();
        manifest.risk_hash = crate::hashing::canonical_hash(&risk);
        manifest.verify_hash = evidence.result_hash.clone();
        manifest.lsif_hash = crate::hashing::canonical_hash(&lsif_summary);
        store.write_manifest(&manifest)?;
        let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
        ledger.record(
            crate::event::EventKind::PackVerified,
            Some(pack_id.clone()),
            None,
            wsh,
            serde_json::json!({
                "risk_level": risk.risk_level.as_str(),
                "result_hash": evidence.result_hash,
            }),
        )?;

        Ok(VerifyV2Report {
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
    /// must match the manifest's `changes_hash`, and every referenced content
    /// object must hash to its own name. Evidence (risk/verify/lsif) is then
    /// produced by the same pipeline as local packs, with file contents read
    /// from the embedded objects instead of the workspace.
    #[allow(clippy::too_many_arguments)]
    fn verify_imported_pack(
        &self,
        ws: &Workspace,
        paths: &crate::layout::ProjectPaths,
        store: &crate::pack::PackStore,
        loc: crate::pack::PackLocation,
        manifest: crate::pack::PackManifest,
        full: bool,
        fuzz: bool,
    ) -> DraftResult<VerifyV2Report> {
        use crate::lsif::{LsifIndex, LSIF_BACKEND};
        use crate::pack::ImportState;

        if !crate::pack::can_import_transition(manifest.import_state, ImportState::ImportVerified) {
            return Err(DraftError::invalid_config(format!(
                "imported pack in state '{}' cannot be verified",
                manifest.lifecycle()
            )));
        }
        let pack_id = manifest.pack_id.clone();
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
        if sha256_hex(&changes_bytes) != manifest.changes_hash {
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
        let fuzz_targets = scan_fuzz_targets(&ws.root);

        let dependency_count = read_json::<crate::pack::PackLockfile>(&dir.join("pack.lock.json"))
            .map(|l| l.dependency_pack_hashes.len())
            .unwrap_or(0);
        let risk_inputs = crate::riskv2::RiskInputs {
            intent: manifest.intent,
            files_touched: changed.len(),
            lines_changed: changed.iter().map(|(_, c)| c.lines().count()).sum(),
            high_risk_paths: crate::riskv2::high_risk_paths(&changed_paths),
            has_tests: !test_files.is_empty(),
            has_fuzz: fuzz && !fuzz_targets.is_empty(),
            public_api_changes: public_api.len(),
            imported: true,
            dependency_count,
            semantic_impact: changed_symbols.len(),
            candidate_rollback_rate: 0.0,
        };
        let risk = crate::riskv2::assess(&risk_inputs);

        let selection = crate::verifyv2::SelectionInput {
            changed_files: changed_paths.clone(),
            changed_symbols: changed_symbols.clone(),
            test_files: test_files.clone(),
            fuzz_targets: fuzz_targets.clone(),
            full,
            fuzz,
        };
        let evidence = crate::verifyv2::plan(&selection);

        // Persist local evidence beside the pack (replacing origin evidence;
        // the origin's signed receipts remain as provenance).
        write_json(&dir.join("risk.json"), &risk)?;
        write_json(&dir.join("verify.json"), &evidence)?;
        let lsif_summary = serde_json::json!({
            "backend": LSIF_BACKEND,
            "symbols_touched": changed_symbols,
            "public_api_changed": public_api,
            "tests_referencing": test_files,
            "semantic_impact": changed_symbols.len(),
        });
        write_json(&dir.join("lsif.json"), &lsif_summary)?;

        let wsh = crate::hashing::workspace_hash(&ws.root)?;
        let mut manifest = manifest;
        manifest.target_workspace_hash = wsh.clone();
        manifest.risk_hash = crate::hashing::canonical_hash(&risk);
        manifest.verify_hash = evidence.result_hash.clone();
        manifest.lsif_hash = crate::hashing::canonical_hash(&lsif_summary);
        manifest.import_state = ImportState::ImportVerified;
        // Approval must follow the latest verification.
        manifest.approval_state = crate::pack::ApprovalState::Pending;
        store.write_manifest_in(loc, &manifest)?;

        let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
        ledger.record(
            crate::event::EventKind::PackVerified,
            Some(pack_id.clone()),
            None,
            wsh,
            serde_json::json!({
                "imported": true,
                "risk_level": risk.risk_level.as_str(),
                "result_hash": evidence.result_hash,
            }),
        )?;

        Ok(VerifyV2Report {
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

    /// Index a pack into LSIF from the current content of its changed files.
    fn ensure_pack_indexed(
        &self,
        ws: &Workspace,
        lsif: &crate::lsif::LsifIndex,
        pack_id: &str,
    ) -> DraftResult<Vec<String>> {
        let paths = crate::layout::ProjectPaths::for_root(&ws.root);
        let store = crate::pack::PackStore::new(paths);
        let files: Vec<String> = store
            .read_lockfile(pack_id)
            .map(|l| l.file_hashes.keys().cloned().collect())
            .unwrap_or_default();
        let content: Vec<(String, String)> = files
            .iter()
            .map(|f| {
                (
                    f.clone(),
                    fs::read_to_string(ws.root.join(f)).unwrap_or_default(),
                )
            })
            .collect();
        lsif.index_pack(pack_id, &content)?;
        lsif.symbols_touched_by_pack(pack_id)
    }

    /// `draft pack inspect <pck_id>`.
    pub fn pack_inspect(&self, cwd: &Path, pack_ref: &str) -> DraftResult<PackInspectReport> {
        let ws = self.open(cwd)?;
        let pack_id = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        let paths = crate::layout::ProjectPaths::for_root(&ws.root);
        let store = crate::pack::PackStore::new(paths.clone());
        let loc = store
            .locate(&pack_id)
            .unwrap_or(crate::pack::PackLocation::Store);
        let manifest = store.read_manifest_in(loc, &pack_id)?;
        let lsif = crate::lsif::LsifIndex::open(&paths)?;
        if manifest.import_state == crate::pack::ImportState::None {
            self.ensure_pack_indexed(&ws, &lsif, &pack_id)?;
        }
        // Imported packs were indexed from their embedded content at local
        // verification; re-indexing from the workspace would erase that.
        let symbols_touched = lsif.symbols_touched_by_pack(&pack_id)?;
        let public_api_changed = lsif.public_api_symbols_changed(&pack_id)?;
        let receipts = crate::receipt::ReceiptStore::new(paths)
            .list()?
            .into_iter()
            .filter(|r| r.subject_id.as_deref() == Some(pack_id.as_str()))
            .map(|r| r.receipt_id)
            .collect();
        Ok(PackInspectReport {
            lifecycle: manifest.lifecycle().to_string(),
            verified: manifest.is_verified(),
            symbols_touched,
            public_api_changed,
            receipts,
            manifest,
        })
    }

    /// `draft pack depends <pck_id>`.
    pub fn pack_depends(&self, cwd: &Path, pack_ref: &str) -> DraftResult<PackDependsReport> {
        let ws = self.open(cwd)?;
        let pack_id = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        let paths = crate::layout::ProjectPaths::for_root(&ws.root);
        let store = crate::pack::PackStore::new(paths.clone());
        let manifest = store.read_manifest(&pack_id)?;
        let lsif = crate::lsif::LsifIndex::open(&paths)?;
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
        let lock = store.read_lockfile(&pack_id).ok();
        Ok(PackDependsReport {
            pack_id,
            base_workspace_hash: manifest.base_workspace_hash,
            changed_files: lock
                .as_ref()
                .map(|l| l.file_hashes.keys().cloned().collect())
                .unwrap_or_default(),
            shared_symbol_packs,
            declared_dependencies: lock.map(|l| l.dependency_pack_hashes).unwrap_or_default(),
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
        let paths = crate::layout::ProjectPaths::for_root(&ws.root);
        let store = crate::pack::PackStore::new(paths.clone());
        let ma = store.read_manifest(&a)?;
        let mb = store.read_manifest(&b)?;
        let lock_a = store.read_lockfile(&a).ok();
        let lock_b = store.read_lockfile(&b).ok();
        let mut conflicts = Vec::new();

        // Textual: same file changed with different content.
        if let (Some(la), Some(lb)) = (&lock_a, &lock_b) {
            for (path, ha) in &la.file_hashes {
                if let Some(hb) = lb.file_hashes.get(path) {
                    if ha != hb {
                        conflicts.push(ConflictFinding {
                            kind: "textual".to_string(),
                            detail: format!("both change '{path}' with different content"),
                            blocking: true,
                        });
                    }
                }
            }
        }

        // Semantic: both touch the same symbols (via LSIF).
        let lsif = crate::lsif::LsifIndex::open(&paths)?;
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
            if !m.is_verified() {
                conflicts.push(ConflictFinding {
                    kind: "verification".to_string(),
                    detail: format!("pack '{}' is not verified", m.pack_id),
                    blocking: false,
                });
            }
        }

        // Dependency: one pack already declares the other as a dependency.
        for (m, other) in [(&ma, &b), (&mb, &a)] {
            if let Ok(lock) = store.read_lockfile(&m.pack_id) {
                if lock.dependency_pack_hashes.iter().any(|d| d == other) {
                    conflicts.push(ConflictFinding {
                        kind: "dependency".to_string(),
                        detail: format!("'{}' depends on '{other}'", m.pack_id),
                        blocking: false,
                    });
                }
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
        use crate::pack::{
            ApprovalState, ImportState, PackLockfile, PackManifest, PackStore, SaveState,
        };
        let ws = self.open(cwd)?;
        let conflict_report = self.pack_conflicts(cwd, a_ref, b_ref)?;
        if conflict_report.blocking {
            let details: Vec<String> = conflict_report
                .conflicts
                .iter()
                .filter(|c| c.blocking)
                .map(|c| format!("{}: {}", c.kind, c.detail))
                .collect();
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "cannot compose — blocking conflicts: {}",
                    details.join("; ")
                ),
            ));
        }
        let a = conflict_report.pack_a;
        let b = conflict_report.pack_b;
        let paths = crate::layout::ProjectPaths::for_root(&ws.root);
        let store = PackStore::new(paths.clone());
        if store.name_taken(name)? {
            return Err(DraftError::invalid_config(format!(
                "pack name '{name}' already exists"
            )));
        }
        let ma = store.read_manifest(&a)?;
        let new_id = format!("pck_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);

        // Combined change set from both lockfiles.
        let mut file_hashes = std::collections::BTreeMap::new();
        for id in [&a, &b] {
            if let Ok(lock) = store.read_lockfile(id) {
                for (f, h) in lock.file_hashes {
                    file_hashes.insert(f, h);
                }
            }
        }
        let wsh = crate::hashing::workspace_hash(&ws.root)?;
        let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
        let outcome = ledger.record(
            crate::event::EventKind::PackComposed,
            Some(new_id.clone()),
            None,
            wsh.clone(),
            serde_json::json!({ "sources": [a, b], "name": name }),
        )?;
        let manifest = PackManifest {
            schema_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            pack_id: new_id.clone(),
            name: name.to_string(),
            description: format!("composed from {a} + {b}"),
            intent: ma.intent,
            origin: "composed".to_string(),
            actor: ledger.actor_id().to_string(),
            candidate: None,
            created_at: now().to_rfc3339(),
            base_workspace_hash: ma.base_workspace_hash.clone(),
            target_workspace_hash: wsh.clone(),
            changes_hash: sha256_hex(format!("{a}+{b}").as_bytes()),
            risk_hash: String::new(),
            verify_hash: String::new(), // unverified — must be re-verified
            lsif_hash: String::new(),
            receipt_hashes: vec![crate::hashing::sha256_hex(
                outcome.receipt.receipt_id.as_bytes(),
            )],
            import_state: ImportState::None,
            approval_state: ApprovalState::Pending,
            save_state: SaveState::Unsaved,
        };
        store.write_manifest(&manifest)?;
        let lock = PackLockfile {
            schema_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            pack_id: new_id.clone(),
            workspace_hash: wsh,
            file_hashes,
            policy_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            risk_engine_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            verification_commands: Vec::new(),
            lsif_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            test_selector_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            fuzz_selector_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            dependency_pack_hashes: vec![a.clone(), b.clone()],
            receipt_hashes: manifest.receipt_hashes.clone(),
        };
        store.write_lockfile(&lock)?;
        Ok(PackComposeReport {
            pack_id: new_id,
            name: name.to_string(),
            dependencies: vec![a, b],
            requires_reverification: true,
        })
    }

    // ---- Cockpit / AG-UI support (thin read/act methods) ----------------

    /// All canonical pack manifests, including quarantined imports (for the
    /// cockpit pack list, MCP, and ACP pending review).
    pub fn list_canonical_packs(&self, cwd: &Path) -> DraftResult<Vec<crate::pack::PackManifest>> {
        let ws = self.open(cwd)?;
        let store = crate::pack::PackStore::new(crate::layout::ProjectPaths::for_root(&ws.root));
        let mut packs = store.list()?;
        packs.extend(store.list_quarantined()?);
        packs.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(packs)
    }

    /// A pack's stored diff (changes.patch text), empty if none.
    pub fn pack_diff_text(&self, cwd: &Path, pack_ref: &str) -> DraftResult<String> {
        let ws = self.open(cwd)?;
        let pid = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        let store = crate::pack::PackStore::new(crate::layout::ProjectPaths::for_root(&ws.root));
        let loc = store
            .locate(&pid)
            .unwrap_or(crate::pack::PackLocation::Store);
        let p = store.dir_for(loc, &pid).join("changes.patch");
        Ok(fs::read_to_string(p).unwrap_or_default())
    }

    /// A pack's stored risk report (or null).
    pub fn pack_risk_json(&self, cwd: &Path, pack_ref: &str) -> DraftResult<Value> {
        let ws = self.open(cwd)?;
        let pid = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        let store = crate::pack::PackStore::new(crate::layout::ProjectPaths::for_root(&ws.root));
        let loc = store
            .locate(&pid)
            .unwrap_or(crate::pack::PackLocation::Store);
        let p = store.dir_for(loc, &pid).join("risk.json");
        if p.exists() {
            read_json::<Value>(&p)
        } else {
            Ok(Value::Null)
        }
    }

    /// Signed receipts referencing a pack.
    pub fn pack_receipts_v2(
        &self,
        cwd: &Path,
        pack_ref: &str,
    ) -> DraftResult<Vec<crate::receipt::ReceiptRecord>> {
        let ws = self.open(cwd)?;
        let pid = self.resolve_canonical_pack_ref(&ws, pack_ref)?;
        Ok(
            crate::receipt::ReceiptStore::new(crate::layout::ProjectPaths::for_root(&ws.root))
                .list()?
                .into_iter()
                .filter(|r| r.subject_id.as_deref() == Some(pid.as_str()))
                .collect(),
        )
    }

    /// The canonical hash-chained event log.
    pub fn canonical_events(&self, cwd: &Path) -> DraftResult<Vec<crate::event::EventRecord>> {
        let ws = self.open(cwd)?;
        crate::event::EventLog::new(crate::layout::ProjectPaths::for_root(&ws.root)).read_all()
    }

    /// Approve or reject a pack through the legacy decision path and record the
    /// canonical PackApproved/PackRejected trust receipt.
    pub fn cockpit_decide(
        &self,
        cwd: &Path,
        pack_ref: &str,
        approve: bool,
        reason: Option<String>,
    ) -> DraftResult<String> {
        let ws = self.open(cwd)?;
        // Imported packs never take the legacy decision path: their approval
        // is a canonical import-state transition with a signed receipt.
        if let Ok(pack_id) = self.resolve_canonical_pack_ref(&ws, pack_ref) {
            let store =
                crate::pack::PackStore::new(crate::layout::ProjectPaths::for_root(&ws.root));
            if let Some(loc) = store.locate(&pack_id) {
                let manifest = store.read_manifest_in(loc, &pack_id)?;
                if manifest.import_state != crate::pack::ImportState::None {
                    return self.decide_imported_pack(&ws, &store, loc, manifest, approve, reason);
                }
            }
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
                    crate::event::EventKind::PackApproved
                } else {
                    crate::event::EventKind::PackRejected
                },
                intent: crate::pack::PackIntent::Feature,
                approval: if approve {
                    crate::pack::ApprovalState::Approved
                } else {
                    crate::pack::ApprovalState::Rejected
                },
                save: crate::pack::SaveState::Unsaved,
                metadata: serde_json::json!({ "via": "cockpit" }),
            },
        )?;
        Ok(pack.id.to_string())
    }

    /// Approve or reject an imported pack: a canonical import-state transition
    /// recorded with a signed PackApproved/PackRejected receipt. Approval
    /// requires prior local verification; rejection is terminal.
    fn decide_imported_pack(
        &self,
        ws: &Workspace,
        store: &crate::pack::PackStore,
        loc: crate::pack::PackLocation,
        mut manifest: crate::pack::PackManifest,
        approve: bool,
        reason: Option<String>,
    ) -> DraftResult<String> {
        use crate::pack::ImportState;
        let target = if approve {
            ImportState::ImportApproved
        } else {
            ImportState::ImportRejected
        };
        if approve && manifest.import_state == ImportState::ImportedQuarantined {
            return Err(DraftError::new(
                DraftErrorKind::ReviewRequired,
                "imported packs must be locally verified before approval",
            )
            .with_suggestion("run `draft verify <pck_id>` first"));
        }
        if !crate::pack::can_import_transition(manifest.import_state, target) {
            return Err(DraftError::invalid_config(format!(
                "imported pack in state '{}' cannot be {}",
                manifest.lifecycle(),
                if approve { "approved" } else { "rejected" }
            )));
        }
        manifest.import_state = target;
        manifest.approval_state = if approve {
            crate::pack::ApprovalState::Approved
        } else {
            crate::pack::ApprovalState::Rejected
        };
        store.write_manifest_in(loc, &manifest)?;

        let wsh = crate::hashing::workspace_hash(&ws.root)?;
        let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
        ledger.record(
            if approve {
                crate::event::EventKind::PackApproved
            } else {
                crate::event::EventKind::PackRejected
            },
            Some(manifest.pack_id.clone()),
            None,
            wsh,
            serde_json::json!({
                "via": "cockpit",
                "imported": true,
                "reason": reason,
            }),
        )?;
        Ok(manifest.pack_id)
    }

    /// Import a `.draftpack` provided as bytes (cockpit upload).
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
        let mut out = Vec::new();
        for p in list_with_extension(&ws.layout.receipts_dir(), "json")? {
            out.push(serde_json::from_str(&fs::read_to_string(p)?)?);
        }
        Ok(out)
    }

    pub fn storage_stats(&self, cwd: &Path) -> DraftResult<StorageStats> {
        let ws = self.open(cwd)?;
        Ok(StorageStats {
            draft_size_bytes: dir_size(&ws.layout.draft_dir)?,
            repo_size_bytes: dir_size_excluding_draft(&ws.root)?,
            objects_size_bytes: dir_size(&ws.layout.objects_dir())?,
            packs_size_bytes: dir_size(&ws.layout.changepacks_dir())?,
            receipts_size_bytes: dir_size(&ws.layout.receipts_dir())?,
            events_size_bytes: fs::metadata(ws.layout.events_file())
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

    pub fn events(&self, cwd: &Path) -> DraftResult<Vec<EventEnvelope>> {
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
    ) -> DraftResult<Vec<EventEnvelope>> {
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
            workspace_id: ws.id.to_string(),
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

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub root: PathBuf,
    pub layout: DraftLayout,
}

impl Workspace {
    fn events(&self) -> DraftResult<EventStore> {
        EventStore::new(self.layout.clone(), self.id.clone())
    }
}

#[derive(Debug, Clone)]
pub struct DraftLayout {
    pub draft_dir: PathBuf,
}

impl DraftLayout {
    pub fn for_root(root: &Path) -> Self {
        Self {
            draft_dir: root.join(DRAFT_DIR),
        }
    }
    pub fn create_all(&self) -> DraftResult<()> {
        for dir in [
            self.draft_dir.clone(),
            self.objects_dir(),
            self.object_packs_dir(),
            self.events_dir(),
            self.snapshots_dir(),
            self.tasks_dir(),
            self.runs_dir(),
            self.candidates_dir(),
            self.changepacks_dir(),
            self.receipts_dir(),
            self.indexes_dir(),
            self.cache_dir(),
            self.locks_dir(),
            self.tmp_dir(),
        ] {
            ensure_dir(&dir)?;
        }
        Ok(())
    }
    pub fn root(&self) -> PathBuf {
        self.draft_dir
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    }
    pub fn config_toml(&self) -> PathBuf {
        self.draft_dir.join("config.toml")
    }
    pub fn ignore_file(&self) -> PathBuf {
        self.draft_dir.join(".ignore")
    }
    pub fn verify_toml(&self) -> PathBuf {
        self.draft_dir.join("verify.toml")
    }
    pub fn policy_toml(&self) -> PathBuf {
        self.draft_dir.join("policy.toml")
    }
    pub fn risk_toml(&self) -> PathBuf {
        self.draft_dir.join("risk.toml")
    }
    pub fn workspace_json(&self) -> PathBuf {
        self.draft_dir.join("workspace.json")
    }
    pub fn selected_pack_file(&self) -> PathBuf {
        self.draft_dir.join("selected-pack")
    }
    pub fn objects_dir(&self) -> PathBuf {
        self.draft_dir.join("objects/blake3")
    }
    pub fn object_packs_dir(&self) -> PathBuf {
        self.draft_dir.join("objects/packs")
    }
    pub fn events_dir(&self) -> PathBuf {
        self.draft_dir.join("events")
    }
    pub fn events_file(&self) -> PathBuf {
        self.events_dir().join("events.jsonl")
    }
    pub fn snapshots_dir(&self) -> PathBuf {
        self.draft_dir.join("snapshots")
    }
    pub fn tasks_dir(&self) -> PathBuf {
        self.draft_dir.join("tasks")
    }
    pub fn runs_dir(&self) -> PathBuf {
        self.draft_dir.join("runs")
    }
    pub fn candidates_dir(&self) -> PathBuf {
        self.draft_dir.join("candidates")
    }
    pub fn changepacks_dir(&self) -> PathBuf {
        self.draft_dir.join("changepacks")
    }
    pub fn receipts_dir(&self) -> PathBuf {
        self.draft_dir.join("receipts")
    }
    pub fn indexes_dir(&self) -> PathBuf {
        self.draft_dir.join("indexes")
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.draft_dir.join("cache")
    }
    pub fn index_file(&self) -> PathBuf {
        self.indexes_dir().join("draft.sqlite")
    }
    pub fn locks_dir(&self) -> PathBuf {
        self.draft_dir.join("locks")
    }
    pub fn tmp_dir(&self) -> PathBuf {
        self.draft_dir.join("tmp")
    }
    pub fn pack_dir(&self, id: &ChangepackId) -> PathBuf {
        self.changepacks_dir().join(id.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMetadata {
    pub schema_version: u32,
    pub id: WorkspaceId,
    pub draft_version: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitReport {
    pub workspace_id: String,
    pub root: String,
    pub created: bool,
    pub draft_dir: String,
}

const DEFAULT_IGNORE: &str = "# Draft private metadata is always excluded.\n.draft/\n";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftConfig {
    pub identity: IdentityConfig,
    #[serde(default)]
    pub save: SaveConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
    pub verification: VerificationConfig,
    pub policy: PolicyConfigSection,
}

impl Default for DraftConfig {
    fn default() -> Self {
        Self {
            identity: IdentityConfig::default(),
            save: SaveConfig::default(),
            hooks: HooksConfig::default(),
            verification: VerificationConfig {
                default_profile: "standard".to_string(),
            },
            policy: PolicyConfigSection {
                require_verification: true,
                require_approval: true,
                require_human_approval_for_high_risk: true,
                block_if_tests_fail: true,
            },
        }
    }
}

impl DraftConfig {
    fn set(&mut self, key: &str, value: &str) -> DraftResult<()> {
        match key {
            "identity.username" => self.identity.username = value.to_string(),
            "identity.email" => self.identity.email = value.to_string(),
            "save.message_template" => self.save.message_template = value.to_string(),
            "hooks.save" => self.hooks.save = Some(HookConfig::Raw(value.to_string())),
            "hooks.verify" => self.hooks.verify = Some(HookConfig::Raw(value.to_string())),
            _ => {
                return Err(DraftError::invalid_config(format!(
                    "unsupported config key '{key}'"
                )))
            }
        }
        Ok(())
    }
    fn unset(&mut self, key: &str) -> DraftResult<()> {
        self.set(key, "")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdentityConfig {
    pub username: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveConfig {
    pub message_template: String,
}

impl Default for SaveConfig {
    fn default() -> Self {
        Self {
            message_template: "{{title}}\n\n{{description}}\n\nDraft-Task: {{task_id}}\nDraft-Run: {{run_id}}\nDraft-Changepack: {{changepack_id}}\nDraft-Verified: {{verified}}\nDraft-Risk: {{risk_level}}\nDraft-Receipt: {{receipt_id}}\nDraft-Actor: {{actor_name}} <{{actor_email}}>".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    pub save: Option<HookConfig>,
    pub verify: Option<HookConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HookConfig {
    Raw(String),
    Entry(HookEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntry {
    pub command: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_hook_phase")]
    pub phase: String,
    #[serde(default = "default_hook_shell")]
    pub shell: String,
    #[serde(default = "default_hook_cwd")]
    pub cwd: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub continue_on_error: bool,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl HookConfig {
    fn entry(&self) -> Option<HookEntry> {
        match self {
            HookConfig::Raw(command) => {
                if command.trim().is_empty() {
                    None
                } else {
                    Some(HookEntry {
                        command: command.clone(),
                        enabled: true,
                        phase: default_hook_phase(),
                        shell: default_hook_shell(),
                        cwd: default_hook_cwd(),
                        timeout_ms: None,
                        continue_on_error: false,
                        env: BTreeMap::new(),
                    })
                }
            }
            HookConfig::Entry(entry) if entry.enabled && !entry.command.trim().is_empty() => {
                Some(entry.clone())
            }
            HookConfig::Entry(_) => None,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_hook_phase() -> String {
    "after_success".to_string()
}
fn default_hook_shell() -> String {
    "default".to_string()
}
fn default_hook_cwd() -> String {
    "workspace".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationConfig {
    pub default_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfigSection {
    pub require_verification: bool,
    pub require_approval: bool,
    pub require_human_approval_for_high_risk: bool,
    pub block_if_tests_fail: bool,
}

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

fn hook_config_command(hook: &HookConfig) -> String {
    match hook {
        HookConfig::Raw(command) => command.clone(),
        HookConfig::Entry(entry) => entry.command.clone(),
    }
}

#[derive(Debug, Clone)]
struct ResolvedConfig {
    identity_username: String,
    identity_email: String,
    save_message_template: String,
    hooks: HooksConfig,
}

impl ResolvedConfig {
    fn load(ws: &Workspace) -> DraftResult<Self> {
        let mut cfg = DraftConfig::default();
        if let Some(home) = home_dir() {
            let global = home.join(".draft/config.toml");
            if global.exists() {
                cfg = merge_config(cfg, read_toml(&global)?);
            }
        }
        if ws.layout.config_toml().exists() {
            cfg = merge_config(cfg, read_toml(&ws.layout.config_toml())?);
        }
        if let Ok(v) = std::env::var("DRAFT_IDENTITY_USERNAME") {
            cfg.identity.username = v;
        }
        if let Ok(v) = std::env::var("DRAFT_IDENTITY_EMAIL") {
            cfg.identity.email = v;
        }
        Ok(Self {
            identity_username: cfg.identity.username,
            identity_email: cfg.identity.email,
            save_message_template: cfg.save.message_template,
            hooks: cfg.hooks,
        })
    }
    fn hook(&self, name: &str) -> Option<HookEntry> {
        match name {
            "save" => self.hooks.save.as_ref().and_then(HookConfig::entry),
            "verify" => self.hooks.verify.as_ref().and_then(HookConfig::entry),
            _ => None,
        }
    }
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "identity.username" => Some(self.identity_username.clone()),
            "identity.email" => Some(self.identity_email.clone()),
            "save.message_template" => Some(self.save_message_template.clone()),
            "hooks.save" => self.hooks.save.as_ref().map(hook_config_command),
            "hooks.verify" => self.hooks.verify.as_ref().map(hook_config_command),
            _ => None,
        }
    }
    fn entries(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        for k in [
            "identity.username",
            "identity.email",
            "save.message_template",
            "hooks.save",
            "hooks.verify",
        ] {
            m.insert(k.to_string(), self.get(k).unwrap_or_default());
        }
        m
    }
}

fn merge_config(mut base: DraftConfig, overlay: DraftConfig) -> DraftConfig {
    if !overlay.identity.username.is_empty() {
        base.identity.username = overlay.identity.username;
    }
    if !overlay.identity.email.is_empty() {
        base.identity.email = overlay.identity.email;
    }
    if !overlay.save.message_template.is_empty() {
        base.save.message_template = overlay.save.message_template;
    }
    if overlay.hooks.save.is_some() {
        base.hooks.save = overlay.hooks.save;
    }
    if overlay.hooks.verify.is_some() {
        base.hooks.verify = overlay.hooks.verify;
    }
    base
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IgnoreReport {
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub id: EventId,
    #[serde(rename = "type")]
    pub event_type: String,
    pub time: DateTime<Utc>,
    pub actor: ActorRef,
    pub workspace_id: WorkspaceId,
    pub subject_id: Option<String>,
    pub payload: Value,
    pub prev_event_hash: Option<String>,
    pub event_hash: String,
    pub schema_version: u32,
}

#[derive(Debug, Clone)]
struct EventStore {
    layout: DraftLayout,
    workspace_id: WorkspaceId,
}

impl EventStore {
    fn new(layout: DraftLayout, workspace_id: WorkspaceId) -> DraftResult<Self> {
        ensure_dir(&layout.events_dir())?;
        if !layout.events_file().exists() {
            write_atomic(&layout.events_file(), b"")?;
        }
        Ok(Self {
            layout,
            workspace_id,
        })
    }
    fn append(
        &self,
        event_type: &str,
        subject_id: Option<String>,
        payload: Value,
    ) -> DraftResult<EventId> {
        let _guard = FileGuard::acquire(
            &self.layout.locks_dir().join("events.lock"),
            Duration::from_secs(10),
        )?;
        let prev = self.read_last()?.map(|e| e.event_hash);
        let mut env = EventEnvelope {
            id: EventId::generate(),
            event_type: event_type.to_string(),
            time: now(),
            actor: resolve_actor(&self.layout.draft_dir),
            workspace_id: self.workspace_id.clone(),
            subject_id,
            payload: redact_value(payload),
            prev_event_hash: prev,
            event_hash: String::new(),
            schema_version: SCHEMA_VERSION,
        };
        env.event_hash = hash_json(&env)?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.layout.events_file())?;
        writeln!(f, "{}", serde_json::to_string(&env).map_err(json_err)?)?;
        f.sync_all()?;
        Ok(env.id)
    }
    fn read_all(&self) -> DraftResult<Vec<EventEnvelope>> {
        let content = fs::read_to_string(self.layout.events_file()).unwrap_or_default();
        let mut out = Vec::new();
        for (idx, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            out.push(serde_json::from_str(line).map_err(|e| {
                DraftError::storage(format!("event log parse failed at line {}: {e}", idx + 1))
            })?);
        }
        Ok(out)
    }
    fn read_last(&self) -> DraftResult<Option<EventEnvelope>> {
        let file = fs::File::open(self.layout.events_file())?;
        let reader = BufReader::new(file);
        let mut last = None;
        for (idx, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            last = Some(serde_json::from_str(&line).map_err(|e| {
                DraftError::storage(format!("event log parse failed at line {}: {e}", idx + 1))
            })?);
        }
        Ok(last)
    }
    fn read_page(
        &self,
        top: bool,
        bottom: bool,
        page: Option<usize>,
        limit: Option<usize>,
        filter: Option<&str>,
    ) -> DraftResult<Vec<EventEnvelope>> {
        let file = fs::File::open(self.layout.events_file())?;
        let reader = BufReader::new(file);
        let limit = limit.unwrap_or(5);
        let page = page.unwrap_or(1).saturating_sub(1);
        let skip = page * limit;
        let newest_first = bottom || !top;
        let mut selected = Vec::new();
        let mut seen = 0usize;
        for (idx, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let event: EventEnvelope = serde_json::from_str(&line).map_err(|e| {
                DraftError::storage(format!("event log parse failed at line {}: {e}", idx + 1))
            })?;
            if let Some(filter) = filter {
                if !event.event_type.contains(filter)
                    && !event
                        .subject_id
                        .as_deref()
                        .map(|s| s.contains(filter))
                        .unwrap_or(false)
                {
                    continue;
                }
            }
            if newest_first {
                selected.push(event);
                let max = skip + limit;
                if selected.len() > max {
                    selected.remove(0);
                }
            } else if seen >= skip && selected.len() < limit {
                selected.push(event);
            }
            seen += 1;
        }
        if newest_first {
            selected.reverse();
            Ok(selected.into_iter().take(limit).collect())
        } else {
            Ok(selected)
        }
    }
    fn verify_chain(&self) -> DraftResult<HashChainStatus> {
        let events = self.read_all()?;
        let mut prev = None;
        for e in &events {
            if e.prev_event_hash != prev {
                return Ok(HashChainStatus {
                    ok: false,
                    events: events.len(),
                    error: Some(format!("broken prev hash at {}", e.id)),
                });
            }
            let mut clone = e.clone();
            clone.event_hash.clear();
            let hash = hash_json(&clone)?;
            if hash != e.event_hash {
                return Ok(HashChainStatus {
                    ok: false,
                    events: events.len(),
                    error: Some(format!("broken event hash at {}", e.id)),
                });
            }
            prev = Some(e.event_hash.clone());
        }
        Ok(HashChainStatus {
            ok: true,
            events: events.len(),
            error: None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashChainStatus {
    pub ok: bool,
    pub events: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventReplayReport {
    pub workspace_id: String,
    pub events: usize,
    pub by_type: BTreeMap<String, usize>,
    pub chain_ok: bool,
    pub error: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ObjectPackIndex {
    objects: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObjectPack {
    schema_version: u32,
    id: String,
    created_at: DateTime<Utc>,
    entries: Vec<ObjectPackEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObjectPackEntry {
    object_ref: String,
    compressed_hex: String,
}

#[derive(Debug, Clone)]
struct ObjectStore {
    layout: DraftLayout,
}

impl ObjectStore {
    fn new(layout: DraftLayout) -> Self {
        Self { layout }
    }
    fn put_bytes(&self, data: &[u8]) -> DraftResult<String> {
        let hash = blake3_hex(data);
        let (a, rest) = hash.split_at(2);
        let path = self.layout.objects_dir().join(a).join(rest);
        if !path.exists() {
            let compressed = zstd::stream::encode_all(data, 3)
                .map_err(|e| DraftError::storage(format!("zstd compression failed: {e}")))?;
            write_atomic(&path, &compressed)?;
        }
        Ok(format!("b3:{hash}"))
    }
    fn get_bytes(&self, object_ref: &str) -> DraftResult<Vec<u8>> {
        let h = object_ref.strip_prefix("b3:").ok_or_else(|| {
            DraftError::storage(format!("unsupported object reference '{object_ref}'"))
        })?;
        let (a, rest) = h.split_at(2);
        let loose_path = self.layout.objects_dir().join(a).join(rest);
        let compressed = if loose_path.exists() {
            fs::read(loose_path)?
        } else {
            self.get_packed_bytes(object_ref)?
        };
        let data = zstd::stream::decode_all(compressed.as_slice())
            .map_err(|e| DraftError::storage(format!("zstd decompression failed: {e}")))?;
        let actual = blake3_hex(&data);
        if actual != h {
            return Err(DraftError::storage(format!(
                "object hash mismatch for {object_ref}: expected {h}, got {actual}"
            )));
        }
        Ok(data)
    }

    fn get_packed_bytes(&self, object_ref: &str) -> DraftResult<Vec<u8>> {
        let index = read_object_pack_index(&self.layout)?;
        let pack_name = index.objects.get(object_ref).ok_or_else(|| {
            DraftError::not_found(format!(
                "object {object_ref} not found in loose or packed store"
            ))
        })?;
        let pack_path = self.layout.object_packs_dir().join(pack_name);
        let pack_bytes = fs::read(&pack_path)?;
        let json = zstd::stream::decode_all(pack_bytes.as_slice()).map_err(|e| {
            DraftError::storage(format!(
                "object pack decompression failed for {}: {e}",
                pack_path.display()
            ))
        })?;
        let pack: ObjectPack = serde_json::from_slice(&json).map_err(json_err)?;
        let entry = pack
            .entries
            .into_iter()
            .find(|entry| entry.object_ref == object_ref)
            .ok_or_else(|| {
                DraftError::storage(format!(
                    "object {object_ref} missing from indexed pack {pack_name}"
                ))
            })?;
        hex_decode(&entry.compressed_hex)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStatus {
    pub workspace_id: WorkspaceId,
    pub root_path: String,
    pub scanned_at: DateTime<Utc>,
    pub changes: Vec<FileChange>,
    pub ignored_count: usize,
    pub has_draft_dir_violation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: WorkspacePath,
    pub change_kind: FileChangeKind,
    pub file_kind: FileKind,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
    pub size_bytes: Option<u64>,
    pub executable: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed { from: WorkspacePath },
    TypeChanged,
    PermissionChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Text,
    Binary,
    Symlink,
    Directory,
    Unknown,
}

struct Scanner<'a> {
    ws: &'a Workspace,
    ignore: IgnoreMatcher,
}

impl<'a> Scanner<'a> {
    fn new(ws: &'a Workspace) -> DraftResult<Self> {
        Ok(Self {
            ws,
            ignore: IgnoreMatcher::load(&ws.layout.ignore_file())?,
        })
    }
    fn status(&self) -> DraftResult<WorkspaceStatus> {
        let previous = latest_snapshot(self.ws)?;
        let current = self.current_manifest()?;
        let previous_map: BTreeMap<_, _> = previous
            .as_ref()
            .map(|s| {
                s.files
                    .iter()
                    .map(|f| (f.path.clone(), f.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut changes = diff_manifests(&previous_map, &current);
        detect_renames(&mut changes);
        Ok(WorkspaceStatus {
            workspace_id: self.ws.id.clone(),
            root_path: self.ws.root.display().to_string(),
            scanned_at: now(),
            ignored_count: self.ignore.ignored_count,
            has_draft_dir_violation: false,
            changes,
        })
    }
    fn current_manifest(&self) -> DraftResult<BTreeMap<WorkspacePath, FileManifestEntry>> {
        let mut out = BTreeMap::new();
        let store = ObjectStore::new(self.ws.layout.clone());
        walk_dir(&self.ws.root, &mut |path| {
            let rel = rel_path(&self.ws.root, path)?;
            if self.ignore.is_ignored(rel.as_str()) {
                return Ok(());
            }
            if path.is_dir() {
                return Ok(());
            }
            let meta = fs::symlink_metadata(path)?;
            let kind = file_kind(path, &meta)?;
            let (hash, size) = if matches!(kind, FileKind::Directory) {
                (None, 0)
            } else if matches!(kind, FileKind::Symlink) {
                let target = fs::read_link(path)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (
                    Some(store.put_bytes(target.as_bytes())?),
                    target.len() as u64,
                )
            } else {
                let data = fs::read(path)?;
                (Some(store.put_bytes(&data)?), data.len() as u64)
            };
            out.insert(
                rel.clone(),
                FileManifestEntry {
                    path: rel,
                    file_kind: kind,
                    content_hash: hash,
                    size_bytes: size,
                    modified_time: meta.modified().ok().map(DateTime::<Utc>::from),
                    executable: executable(&meta),
                },
            );
            Ok(())
        })?;
        Ok(out)
    }
}

#[derive(Debug, Clone)]
struct IgnoreMatcher {
    patterns: Vec<String>,
    ignored_count: usize,
}

impl IgnoreMatcher {
    fn load(path: &Path) -> DraftResult<Self> {
        Ok(Self {
            patterns: read_ignore_lines(path)?,
            ignored_count: 0,
        })
    }
    fn is_ignored(&self, path: &str) -> bool {
        if is_draft_path(path) {
            return true;
        }
        let mut ignored = false;
        for p in &self.patterns {
            let neg = p.starts_with('!');
            let pat = p.trim_start_matches('!');
            let matched = pattern_match(pat, path);
            if matched {
                ignored = !neg;
            }
        }
        ignored
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub id: SnapshotId,
    pub workspace_id: WorkspaceId,
    pub manifest_hash: String,
    pub files: Vec<FileManifestEntry>,
    pub content_object_refs: Vec<String>,
    pub ignored_patterns_hash: String,
    pub created_at: DateTime<Utc>,
    pub created_by: ActorRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManifestEntry {
    pub path: WorkspacePath,
    pub file_kind: FileKind,
    pub content_hash: Option<String>,
    pub size_bytes: u64,
    pub modified_time: Option<DateTime<Utc>>,
    pub executable: Option<bool>,
}

struct Snapshotter<'a> {
    ws: &'a Workspace,
}

impl<'a> Snapshotter<'a> {
    fn new(ws: &'a Workspace) -> DraftResult<Self> {
        Ok(Self { ws })
    }
    fn create_snapshot(&self) -> DraftResult<Snapshot> {
        let scanner = Scanner::new(self.ws)?;
        let mut files: Vec<_> = scanner.current_manifest()?.into_values().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let content_object_refs = files
            .iter()
            .filter_map(|f| f.content_hash.clone())
            .collect();
        let ignored_patterns_hash = sha256_hex(
            read_ignore_lines(&self.ws.layout.ignore_file())?
                .join("\n")
                .as_bytes(),
        );
        let mut snapshot = Snapshot {
            schema_version: SCHEMA_VERSION,
            id: SnapshotId::generate(),
            workspace_id: self.ws.id.clone(),
            manifest_hash: String::new(),
            files,
            content_object_refs,
            ignored_patterns_hash,
            created_at: now(),
            created_by: resolve_actor(&self.ws.layout.draft_dir),
        };
        snapshot.manifest_hash = hash_json(&snapshot)?;
        write_json(
            &self
                .ws
                .layout
                .snapshots_dir()
                .join(format!("{}.json", snapshot.id)),
            &snapshot,
        )?;
        Ok(snapshot)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointReport {
    pub snapshot_id: String,
    pub receipt_id: String,
    pub files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub schema_version: u32,
    pub id: TaskId,
    pub title: String,
    pub description: Option<String>,
    pub created_by: ActorRef,
    pub risk_profile: Option<String>,
    pub linked_issue: Option<String>,
    pub created_at: DateTime<Utc>,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpawnReport {
    pub task: Task,
    pub pack_id: Option<String>,
    pub cron: Option<String>,
    pub runs: Vec<TaskRunSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRunSummary {
    pub candidate: String,
    pub run_id: Option<String>,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub name: String,
    pub kind: String,
    pub source: String,
    pub template: String,
    pub role: Option<String>,
    pub persona: Option<String>,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidatePackAssignment {
    pub pack_id: String,
    pub candidate: String,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Open,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub schema_version: u32,
    pub id: RunId,
    pub task_id: TaskId,
    pub workspace_id: WorkspaceId,
    pub base_snapshot_id: SnapshotId,
    pub actor_kind: ActorKind,
    pub actor_name: String,
    pub command: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: RunStatus,
    pub stdout_ref: Option<String>,
    pub stderr_ref: Option<String>,
    pub exit_code: Option<i32>,
    pub result_snapshot_id: Option<SnapshotId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Started,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Changepack {
    pub schema_version: u32,
    pub id: ChangepackId,
    pub name: Option<String>,
    pub task_id: Option<TaskId>,
    pub run_id: Option<RunId>,
    pub workspace_id: WorkspaceId,
    pub base_snapshot_id: SnapshotId,
    pub result_snapshot_id: SnapshotId,
    pub patch_refs: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub verification_refs: Vec<String>,
    pub review_refs: Vec<String>,
    pub decision_refs: Vec<String>,
    pub receipt_refs: Vec<String>,
    pub source_pack_ids: Vec<String>,
    pub status: ChangepackStatus,
    #[serde(default = "default_true")]
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub manifest_hash: String,
}

impl Changepack {
    fn new(
        workspace_id: WorkspaceId,
        task_id: Option<TaskId>,
        run_id: Option<RunId>,
        base_snapshot_id: SnapshotId,
        result_snapshot_id: SnapshotId,
        name: Option<String>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: ChangepackId::generate(),
            name,
            task_id,
            run_id,
            workspace_id,
            base_snapshot_id,
            result_snapshot_id,
            patch_refs: vec![],
            evidence_refs: vec![],
            verification_refs: vec![],
            review_refs: vec![],
            decision_refs: vec![],
            receipt_refs: vec![],
            source_pack_ids: vec![],
            status: ChangepackStatus::Draft,
            active: true,
            created_at: now(),
            updated_at: now(),
            manifest_hash: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangepackStatus {
    Draft,
    Verified,
    Reviewed,
    Approved,
    Saved,
    Rejected,
    RolledBack,
}

impl ChangepackStatus {
    fn transition(self, next: ChangepackStatus) -> DraftResult<ChangepackStatus> {
        use ChangepackStatus::*;
        let ok = matches!(
            (self, next),
            (Draft, Verified)
                | (Draft, Reviewed)
                | (Draft, Rejected)
                | (Verified, Reviewed)
                | (Verified, Approved)
                | (Reviewed, Approved)
                | (Reviewed, Rejected)
                | (Approved, Saved)
                | (Saved, RolledBack)
                | (Approved, Approved)
                | (Saved, Saved)
        );
        if ok {
            Ok(next)
        } else {
            Err(DraftError::new(
                DraftErrorKind::InvalidConfig,
                format!(
                    "invalid changepack transition from {:?} to {:?}",
                    self, next
                ),
            ))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchSet {
    pub schema_version: u32,
    pub id: PatchSetId,
    pub base_snapshot_id: SnapshotId,
    pub result_snapshot_id: SnapshotId,
    pub files: Vec<FilePatch>,
    pub patch_graph_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePatch {
    pub path: WorkspacePath,
    pub old_path: Option<WorkspacePath>,
    pub change_kind: FileChangeKind,
    #[serde(default)]
    pub hunks: Vec<PatchHunk>,
    pub binary: bool,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchHunk {
    #[serde(default)]
    pub id: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub content_ref: String,
    #[serde(default)]
    pub old_content_hash: Option<String>,
    #[serde(default)]
    pub new_content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HunkOverlap {
    pub path: WorkspacePath,
    pub left_hunk_id: String,
    pub right_hunk_id: String,
    pub old_start: u32,
    pub old_end: u32,
    pub new_start: u32,
    pub new_end: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub schema_version: u32,
    pub id: EvidenceId,
    pub changepack_id: ChangepackId,
    pub command_logs: Vec<String>,
    pub files_touched: Vec<WorkspacePath>,
    pub generated_diff_ref: Option<String>,
    pub test_results: Vec<String>,
    pub lint_results: Vec<String>,
    pub risk_summary_ref: Option<String>,
    pub agent_plan_ref: Option<String>,
    pub agent_transcript_ref: Option<String>,
    pub warnings: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackReport {
    pub pack: Changepack,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerifyFile {
    pub checks: Vec<VerificationCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationCheck {
    pub name: String,
    pub command: String,
    pub risk: RiskLevel,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub check_name: String,
    pub command_hash: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub exit_code: i32,
    pub stdout_ref: String,
    pub stderr_ref: String,
    pub status: VerificationStatus,
}

impl VerificationResult {
    fn skipped(store: &ObjectStore) -> DraftResult<Self> {
        let empty_ref = store.put_bytes(b"")?;
        Ok(Self {
            check_name: "no enabled checks".to_string(),
            command_hash: sha256_hex(b"skipped"),
            started_at: now(),
            ended_at: now(),
            duration_ms: 0,
            exit_code: 0,
            stdout_ref: empty_ref.clone(),
            stderr_ref: empty_ref,
            status: VerificationStatus::Skipped,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    Failed,
    Skipped,
    TimedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReport {
    pub changepack_id: String,
    pub receipt_id: String,
    pub results: Vec<VerificationResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    pub fn label(self) -> &'static str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskSummary {
    pub changepack_id: String,
    pub receipt_id: String,
    pub level: RiskLevel,
    pub score: u32,
    pub factors: Vec<String>,
    pub reason_codes: Vec<String>,
    pub hotspots: Vec<WorkspacePath>,
    pub evidence_gaps: Vec<String>,
    #[serde(default)]
    pub evidence_summary: Vec<String>,
    pub policy_decision: String,
    pub files_changed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveReadinessReport {
    pub ok: bool,
    pub blockers: Vec<String>,
    #[serde(default)]
    pub verification_receipt_id: Option<String>,
    #[serde(default)]
    pub review_receipt_id: Option<String>,
    #[serde(default)]
    pub approval_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskConfig {
    pub medium_threshold: u32,
    pub high_threshold: u32,
    pub critical_threshold: u32,
    pub path_rules: Vec<RiskPathRule>,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            medium_threshold: 3,
            high_threshold: 6,
            critical_threshold: 10,
            path_rules: vec![
                RiskPathRule::new(
                    "auth_or_security_surface",
                    6,
                    ["auth", "oauth", "jwt", "session", "security"],
                ),
                RiskPathRule::new(
                    "payment_surface",
                    6,
                    ["payment", "billing", "stripe", "invoice"],
                ),
                RiskPathRule::new(
                    "database_migration",
                    5,
                    ["migration", "schema.sql", "database", "db/"],
                ),
                RiskPathRule::new(
                    "dependency_lockfile",
                    4,
                    [
                        "package-lock.json",
                        "pnpm-lock.yaml",
                        "yarn.lock",
                        "cargo.lock",
                        "go.sum",
                    ],
                ),
                RiskPathRule::new(
                    "ci_cd_change",
                    4,
                    [".github/", ".gitlab-ci", "circleci", "jenkins", "ci/"],
                ),
                RiskPathRule::new(
                    "container_change",
                    3,
                    ["dockerfile", "compose.yaml", "compose.yml"],
                ),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskPathRule {
    pub code: String,
    pub weight: u32,
    pub patterns: Vec<String>,
}

impl RiskPathRule {
    fn new<const N: usize>(code: &str, weight: u32, patterns: [&str; N]) -> Self {
        Self {
            code: code.to_string(),
            weight,
            patterns: patterns.into_iter().map(ToString::to_string).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    pub approval: ApprovalPolicy,
    pub agent: AgentPolicy,
    pub save: SavePolicy,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            approval: ApprovalPolicy {
                low_risk_requires: 0,
                medium_risk_requires: 1,
                high_risk_requires: 1,
            },
            agent: AgentPolicy {
                allow_network: false,
                allow_secrets: false,
                require_isolated_workspace: true,
            },
            save: SavePolicy {
                block_if_tests_fail: true,
                block_if_unreviewed_high_risk: true,
                block_if_draft_dir_in_candidate: true,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    pub low_risk_requires: u32,
    pub medium_risk_requires: u32,
    pub high_risk_requires: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPolicy {
    pub allow_network: bool,
    pub allow_secrets: bool,
    pub require_isolated_workspace: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavePolicy {
    pub block_if_tests_fail: bool,
    pub block_if_unreviewed_high_risk: bool,
    pub block_if_draft_dir_in_candidate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ReviewFile {
    comments: Vec<ReviewComment>,
    decisions: Vec<Decision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: ReviewCommentId,
    pub changepack_id: ChangepackId,
    pub path: Option<WorkspacePath>,
    pub hunk_id: Option<String>,
    pub actor: ActorRef,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Approve,
    Reject,
    NeedsChanges,
    AcceptFile,
    RejectFile,
    AcceptCandidate,
}

impl DecisionKind {
    pub fn label(self) -> &'static str {
        match self {
            DecisionKind::Approve => "approve",
            DecisionKind::Reject => "reject",
            DecisionKind::NeedsChanges => "needs_changes",
            DecisionKind::AcceptFile => "accept_file",
            DecisionKind::RejectFile => "reject_file",
            DecisionKind::AcceptCandidate => "accept_candidate",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: DecisionId,
    pub changepack_id: ChangepackId,
    pub actor: ActorRef,
    pub kind: DecisionKind,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewReport {
    pub changepack_id: String,
    #[serde(default)]
    pub review_receipt_id: Option<String>,
    pub comments: usize,
    pub decisions: usize,
    pub status: ChangepackStatus,
    #[serde(default)]
    pub review_units: Vec<ReviewUnit>,
    #[serde(default)]
    pub risk_receipt_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewUnit {
    pub id: String,
    pub path: WorkspacePath,
    pub change_kind: String,
    pub risk_contribution: u32,
    pub evidence_refs: Vec<String>,
    pub provenance_refs: Vec<String>,
    pub status: String,
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
pub struct SaveReceipt {
    pub schema_version: u32,
    pub id: ReceiptId,
    pub changepack_id: ChangepackId,
    pub actor: ActorRef,
    pub native_save_status: NativeSaveStatus,
    pub hook_status: HookStatus,
    pub overall_status: SaveOverallStatus,
    pub message_ref: String,
    pub hook_results: Vec<HookResult>,
    #[serde(default)]
    pub hook_receipt_refs: Vec<String>,
    #[serde(default)]
    pub object_refs: Vec<String>,
    #[serde(default)]
    pub event_refs: Vec<String>,
    #[serde(default)]
    pub risk_level: String,
    #[serde(default)]
    pub risk_receipt_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub receipt_hash: String,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeSaveStatus {
    Saved,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookStatus {
    NotConfigured,
    Skipped,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveOverallStatus {
    Saved,
    Failed,
    SavedWithHookFailure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    pub hook_name: String,
    pub hook_phase: String,
    pub shell: String,
    pub working_dir: String,
    pub command_hash: String,
    pub exit_code: i32,
    pub stdout_ref: String,
    pub stderr_ref: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub env_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackPlan {
    pub id: RollbackPlanId,
    pub rollback_snapshot_id: SnapshotId,
    pub affected_files: Vec<WorkspacePath>,
    pub destructive: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackReceipt {
    pub schema_version: u32,
    pub id: ReceiptId,
    pub rollback_plan_id: RollbackPlanId,
    pub actor: ActorRef,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub receipt_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexReport {
    pub path: String,
    pub events: usize,
    pub tasks: usize,
    pub runs: usize,
    pub changepacks: usize,
    pub receipts: usize,
    pub snapshots: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Receipt {
    schema_version: u32,
    id: ReceiptId,
    kind: String,
    status: String,
    subject_id: Option<String>,
    #[serde(default)]
    event_refs: Vec<String>,
    #[serde(default)]
    object_refs: Vec<String>,
    #[serde(default)]
    reversible: bool,
    #[serde(default)]
    rollback_target: Option<String>,
    payload: Value,
    created_at: DateTime<Utc>,
    receipt_hash: String,
}

impl Receipt {
    fn new(kind: &str, status: &str, subject_id: Option<String>, payload: Value) -> Self {
        let mut r = Self {
            schema_version: SCHEMA_VERSION,
            id: ReceiptId::generate(),
            kind: kind.to_string(),
            status: status.to_string(),
            subject_id,
            event_refs: vec![],
            object_refs: vec![],
            reversible: false,
            rollback_target: None,
            payload,
            created_at: now(),
            receipt_hash: String::new(),
        };
        r.receipt_hash = hash_json(&r).unwrap_or_default();
        r
    }

    fn reversible_to(mut self, target: impl Into<String>) -> Self {
        self.reversible = true;
        self.rollback_target = Some(target.into());
        self.receipt_hash.clear();
        self.receipt_hash = hash_json(&self).unwrap_or_default();
        self
    }
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
            "retired external-action config keys are not supported in Draft v0.3.2; use hooks.*",
        ));
    }
    Ok(())
}

fn validate_config_key(key: &str) -> DraftResult<()> {
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

fn read_or_default<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    if path.exists() {
        read_toml(path).unwrap_or_default()
    } else {
        T::default()
    }
}

fn read_ignore_lines(path: &Path) -> DraftResult<Vec<String>> {
    let content = fs::read_to_string(path).unwrap_or_default();
    Ok(content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(ToString::to_string)
        .collect())
}

fn is_draft_path(path: &str) -> bool {
    // Delegate to the central path guard so *every* `.draft` component (nested,
    // case-insensitive, backslash-separated) is hard-excluded (spec §9.2), not
    // just a top-level `.draft/`.
    crate::pathguard::is_draft_path(path)
}

fn pattern_match(pattern: &str, path: &str) -> bool {
    if pattern == ".draft/" {
        return is_draft_path(path);
    }
    if let Some(dir) = pattern.strip_suffix('/') {
        return path == dir || path.starts_with(&format!("{dir}/"));
    }
    if let Some(ext) = pattern.strip_prefix("*.") {
        return path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .ends_with(&format!(".{ext}"));
    }
    if pattern.contains('*') {
        let parts: Vec<_> = pattern.split('*').collect();
        let mut rem = path;
        for part in parts {
            if part.is_empty() {
                continue;
            }
            if let Some(idx) = rem.find(part) {
                rem = &rem[idx + part.len()..];
            } else {
                return false;
            }
        }
        return true;
    }
    path == pattern || path.starts_with(&format!("{pattern}/"))
}

fn rel_path(root: &Path, path: &Path) -> DraftResult<WorkspacePath> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| DraftError::storage("path escaped workspace root"))?;
    Ok(WorkspacePath::from_relative(rel))
}

fn walk_dir<F: FnMut(&Path) -> DraftResult<()>>(root: &Path, f: &mut F) -> DraftResult<()> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let rel = rel_path(root, &path)?;
        if is_draft_path(rel.as_str()) {
            continue;
        }
        f(&path)?;
        if path.is_dir() {
            walk_dir_inner(root, &path, f)?;
        }
    }
    Ok(())
}

fn walk_dir_inner<F: FnMut(&Path) -> DraftResult<()>>(
    root: &Path,
    dir: &Path,
    f: &mut F,
) -> DraftResult<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let rel = rel_path(root, &path)?;
        if is_draft_path(rel.as_str()) {
            continue;
        }
        f(&path)?;
        if path.is_dir() {
            walk_dir_inner(root, &path, f)?;
        }
    }
    Ok(())
}

fn file_kind(path: &Path, meta: &fs::Metadata) -> DraftResult<FileKind> {
    if meta.file_type().is_symlink() {
        return Ok(FileKind::Symlink);
    }
    if meta.is_dir() {
        return Ok(FileKind::Directory);
    }
    let mut buf = [0u8; 1024];
    let n = fs::File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    if buf[..n].contains(&0) {
        Ok(FileKind::Binary)
    } else {
        Ok(FileKind::Text)
    }
}

#[cfg(unix)]
fn executable(meta: &fs::Metadata) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(_meta: &fs::Metadata) -> Option<bool> {
    None
}

fn diff_manifests(
    old: &BTreeMap<WorkspacePath, FileManifestEntry>,
    new: &BTreeMap<WorkspacePath, FileManifestEntry>,
) -> Vec<FileChange> {
    let mut changes = Vec::new();
    for (path, n) in new {
        match old.get(path) {
            None => changes.push(FileChange {
                path: path.clone(),
                change_kind: FileChangeKind::Added,
                file_kind: n.file_kind.clone(),
                old_hash: None,
                new_hash: n.content_hash.clone(),
                size_bytes: Some(n.size_bytes),
                executable: n.executable,
            }),
            Some(o)
                if o.content_hash != n.content_hash
                    || o.file_kind != n.file_kind
                    || o.executable != n.executable =>
            {
                changes.push(FileChange {
                    path: path.clone(),
                    change_kind: if o.file_kind != n.file_kind {
                        FileChangeKind::TypeChanged
                    } else if o.executable != n.executable {
                        FileChangeKind::PermissionChanged
                    } else {
                        FileChangeKind::Modified
                    },
                    file_kind: n.file_kind.clone(),
                    old_hash: o.content_hash.clone(),
                    new_hash: n.content_hash.clone(),
                    size_bytes: Some(n.size_bytes),
                    executable: n.executable,
                })
            }
            _ => {}
        }
    }
    for (path, o) in old {
        if !new.contains_key(path) {
            changes.push(FileChange {
                path: path.clone(),
                change_kind: FileChangeKind::Deleted,
                file_kind: o.file_kind.clone(),
                old_hash: o.content_hash.clone(),
                new_hash: None,
                size_bytes: Some(o.size_bytes),
                executable: o.executable,
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes
}

fn detect_renames(changes: &mut [FileChange]) {
    let deleted: Vec<_> = changes
        .iter()
        .filter(|c| matches!(c.change_kind, FileChangeKind::Deleted))
        .map(|c| (c.old_hash.clone(), c.path.clone()))
        .collect();
    for c in changes
        .iter_mut()
        .filter(|c| matches!(c.change_kind, FileChangeKind::Added))
    {
        if let Some((_, from)) = deleted
            .iter()
            .find(|(h, _)| h.is_some() && h == &c.new_hash)
        {
            c.change_kind = FileChangeKind::Renamed { from: from.clone() };
        }
    }
}

fn latest_snapshot(ws: &Workspace) -> DraftResult<Option<Snapshot>> {
    let mut snaps: Vec<Snapshot> = load_json_dir(&ws.layout.snapshots_dir())?;
    snaps.sort_by_key(|a| a.created_at);
    Ok(snaps.pop())
}

fn empty_snapshot(ws: &Workspace) -> Snapshot {
    Snapshot {
        schema_version: SCHEMA_VERSION,
        id: SnapshotId::new("chk_empty"),
        workspace_id: ws.id.clone(),
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
    read_json(&ws.layout.snapshots_dir().join(format!("{}.json", id)))
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
    let mut patch = diff_snapshot_values(base, result);
    enrich_patch_hunks(ws, base, result, &mut patch)?;
    patch.patch_graph_hash.clear();
    patch.patch_graph_hash = hash_json(&patch)?;
    write_json(
        &ws.layout.tmp_dir().join(format!("{}.json", patch.id)),
        &patch,
    )?;
    Ok(patch)
}

fn diff_snapshot_values(base: &Snapshot, result: &Snapshot) -> PatchSet {
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
        schema_version: SCHEMA_VERSION,
        id: PatchSetId::generate(),
        base_snapshot_id: base.id.clone(),
        result_snapshot_id: result.id.clone(),
        files,
        patch_graph_hash: String::new(),
    };
    patch.patch_graph_hash = hash_json(&patch).unwrap_or_default();
    patch
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
    Ok(vec![PatchHunk {
        id: format!("hunk_{}", &sha256_hex(id_input.as_bytes())[..12]),
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

fn load_json_dir<T: for<'de> Deserialize<'de>>(dir: &Path) -> DraftResult<Vec<T>> {
    let mut out = Vec::new();
    for p in list_with_extension(dir, "json")? {
        out.push(read_json(&p)?);
    }
    Ok(out)
}

fn load_pack(ws: &Workspace, id: &str) -> DraftResult<Changepack> {
    read_json(&ws.layout.changepacks_dir().join(id).join("manifest.json"))
}

fn save_pack_manifest(ws: &Workspace, pack: &mut Changepack) -> DraftResult<()> {
    pack.updated_at = now();
    pack.manifest_hash.clear();
    pack.manifest_hash = hash_json(pack)?;
    write_json(&ws.layout.pack_dir(&pack.id).join("manifest.json"), pack)
}

fn load_patch(ws: &Workspace, pack: &Changepack) -> DraftResult<PatchSet> {
    read_json(&ws.layout.pack_dir(&pack.id).join("patch.json"))
}

fn load_evidence(ws: &Workspace, pack: &Changepack) -> DraftResult<Evidence> {
    read_json(&ws.layout.pack_dir(&pack.id).join("evidence.json"))
}

fn save_readiness(
    ws: &Workspace,
    pack: &Changepack,
    patch: &PatchSet,
    policy: &PolicyConfig,
) -> DraftResult<SaveReadinessReport> {
    let mut blockers = Vec::new();
    let verification = latest_current_passed_verification(ws, pack, patch)?;
    if policy.save.block_if_tests_fail && verification.is_none() {
        blockers.push("current passed verification receipt is required before save".to_string());
    }
    let review =
        latest_completed_review_after(ws, pack, verification.as_ref().map(|v| v.created_at))?;
    let approval = latest_human_approval_after(ws, pack, review.as_ref().map(|r| r.created_at))?;
    if policy.save.block_if_unreviewed_high_risk {
        if review.is_none() {
            blockers.push("current review receipt is required before save".to_string());
        }
        if approval.is_none() || !matches!(pack.status, ChangepackStatus::Approved) {
            blockers
                .push("human approval is required after current review before save".to_string());
        }
    }
    Ok(SaveReadinessReport {
        ok: blockers.is_empty(),
        blockers,
        verification_receipt_id: verification.map(|v| v.id),
        review_receipt_id: review.map(|r| r.id),
        approval_ref: approval,
    })
}

/// Resolve the effective policy for a workspace: project `.draft/policy.toml`
/// over the global default policy over the built-in safe default. Fails closed
/// on an unreadable or malformed policy file.
fn effective_policy(ws: &Workspace) -> DraftResult<crate::policy::Policy> {
    let project = crate::layout::ProjectPaths::for_root(&ws.root).policy_toml();
    let global = crate::home::GlobalHome::locate()
        .ok()
        .map(|h| h.default_policy_toml());
    crate::policy::Policy::resolve_checked(Some(&project), global.as_deref())
        .map_err(DraftError::invalid_config)
}

fn validate_canonical_save_gate(ws: &Workspace, pack_id: &str) -> DraftResult<()> {
    let policy = effective_policy(ws)?;
    let paths = crate::layout::ProjectPaths::for_root(&ws.root);
    let store = crate::pack::PackStore::new(paths.clone());
    let manifest = store.read_manifest(pack_id)?;
    if !manifest.is_verified() {
        return Err(DraftError::new(
            DraftErrorKind::VerificationFailed,
            "canonical verification receipt is required before save",
        ));
    }
    if policy.require_approval_for_save
        && manifest.approval_state != crate::pack::ApprovalState::Approved
    {
        return Err(DraftError::new(
            DraftErrorKind::ReviewRequired,
            "canonical approval receipt is required before save",
        ));
    }
    if manifest.import_state == crate::pack::ImportState::ImportedQuarantined {
        return Err(DraftError::new(
            DraftErrorKind::ReviewRequired,
            "imported packs must be locally verified and approved before save",
        ));
    }
    validate_canonical_risk_gate(&policy, &paths, pack_id, &manifest)?;
    if policy.require_reverify_on_workspace_change {
        let current_hash = crate::hashing::workspace_hash(&ws.root)?;
        if manifest.target_workspace_hash != current_hash {
            return Err(DraftError::new(
                DraftErrorKind::VerificationFailed,
                "workspace content changed after canonical verification",
            )
            .with_suggestion("run `draft verify <pck_id>` again before save"));
        }
    }
    let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
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
/// policy: an unresolved critical risk blocks save, and high/critical risk
/// requires explicit approval. A missing risk report fails closed when the
/// policy blocks on critical risk.
fn validate_canonical_risk_gate(
    policy: &crate::policy::Policy,
    paths: &crate::layout::ProjectPaths,
    pack_id: &str,
    manifest: &crate::pack::PackManifest,
) -> DraftResult<()> {
    let risk_path = paths.pack_risk(pack_id);
    if !risk_path.exists() {
        if policy.block_on_critical_risk {
            return Err(DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                "no canonical risk report exists for this pack",
            )
            .with_suggestion("run `draft verify <pck_id>` before save"));
        }
        return Ok(());
    }
    let risk: crate::riskv2::RiskReport = read_json(&risk_path)?;
    if policy.block_on_critical_risk && risk.risk_level == crate::riskv2::RiskLevel::Critical {
        return Err(DraftError::new(
            DraftErrorKind::RiskPolicyBlocked,
            "unresolved critical risk blocks save",
        )
        .with_suggestion("resolve the required actions in risk.json and re-verify"));
    }
    if policy.require_approval_on_high_risk
        && matches!(
            risk.risk_level,
            crate::riskv2::RiskLevel::High | crate::riskv2::RiskLevel::Critical
        )
        && manifest.approval_state != crate::pack::ApprovalState::Approved
    {
        return Err(DraftError::new(
            DraftErrorKind::ReviewRequired,
            "high-risk pack requires explicit approval before save",
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
    pack: &Changepack,
    patch: &PatchSet,
) -> DraftResult<Option<ReceiptRef>> {
    let mut latest = None;
    for receipt_id in &pack.verification_refs {
        let value = read_receipt_value(ws, receipt_id)?;
        if value.get("kind").and_then(Value::as_str) != Some("verification")
            || value.get("status").and_then(Value::as_str) != Some("passed")
            || value.get("subject_id").and_then(Value::as_str) != Some(pack.id.as_str())
        {
            continue;
        }
        let payload = value.get("payload").unwrap_or(&Value::Null);
        if payload.get("patch_graph_hash").and_then(Value::as_str)
            != Some(patch.patch_graph_hash.as_str())
        {
            continue;
        }
        let Some(created_at) = receipt_created_at(&value) else {
            continue;
        };
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

fn latest_completed_review_after(
    ws: &Workspace,
    pack: &Changepack,
    after: Option<DateTime<Utc>>,
) -> DraftResult<Option<ReceiptRef>> {
    let mut latest = None;
    for receipt_id in &pack.review_refs {
        let value = read_receipt_value(ws, receipt_id)?;
        if value.get("kind").and_then(Value::as_str) != Some("review")
            || value.get("status").and_then(Value::as_str) != Some("completed")
            || value.get("subject_id").and_then(Value::as_str) != Some(pack.id.as_str())
        {
            continue;
        }
        let Some(created_at) = receipt_created_at(&value) else {
            continue;
        };
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
    pack: &Changepack,
    after: Option<DateTime<Utc>>,
) -> DraftResult<Option<String>> {
    let review = load_review_file(ws, &pack.id).unwrap_or_default();
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

fn read_receipt_value(ws: &Workspace, id: &str) -> DraftResult<Value> {
    validate_receipt_id(id)?;
    read_json(&ws.layout.receipts_dir().join(format!("{id}.json")))
}

fn receipt_created_at(value: &Value) -> Option<DateTime<Utc>> {
    value
        .get("created_at")
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

fn load_review_file(ws: &Workspace, id: &ChangepackId) -> DraftResult<ReviewFile> {
    read_json(&ws.layout.pack_dir(id).join("review.json"))
}

fn save_review_file(ws: &Workspace, id: &ChangepackId, file: &ReviewFile) -> DraftResult<()> {
    write_json(&ws.layout.pack_dir(id).join("review.json"), file)
}

fn build_review_units(
    ws: &Workspace,
    pack: &Changepack,
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

fn split_patch(source: &Changepack, files: Vec<FilePatch>) -> DraftResult<PatchSet> {
    let mut patch = PatchSet {
        schema_version: SCHEMA_VERSION,
        id: PatchSetId::generate(),
        base_snapshot_id: source.base_snapshot_id.clone(),
        result_snapshot_id: source.result_snapshot_id.clone(),
        files,
        patch_graph_hash: String::new(),
    };
    patch.patch_graph_hash = hash_json(&patch)?;
    Ok(patch)
}

fn split_evidence(pack: &Changepack, patch: &PatchSet, warning: &str) -> Evidence {
    Evidence {
        schema_version: SCHEMA_VERSION,
        id: EvidenceId::generate(),
        changepack_id: pack.id.clone(),
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

fn write_receipt(ws: &Workspace, receipt: &Receipt) -> DraftResult<()> {
    let mut receipt = receipt.clone();
    receipt.payload = redact_value(receipt.payload);
    collect_object_refs_into_vec(&receipt.payload, &mut receipt.object_refs);
    receipt.object_refs.sort();
    receipt.object_refs.dedup();
    let event_id = ws.events()?.append(
        "receipt.created",
        Some(receipt.id.to_string()),
        serde_json::json!({
            "kind": receipt.kind,
            "status": receipt.status,
            "subject_id": receipt.subject_id,
            "reversible": receipt.reversible,
            "rollback_target": receipt.rollback_target
        }),
    )?;
    receipt.event_refs.push(event_id.to_string());
    receipt.event_refs.sort();
    receipt.event_refs.dedup();
    receipt.receipt_hash.clear();
    receipt.receipt_hash = hash_json(&receipt)?;
    write_json(
        &ws.layout
            .receipts_dir()
            .join(format!("{}.json", receipt.id)),
        &receipt,
    )?;
    Ok(())
}

fn write_save_receipt(ws: &Workspace, receipt: &SaveReceipt) -> DraftResult<()> {
    let mut receipt = receipt.clone();
    collect_object_refs_into_vec(
        &serde_json::to_value(&receipt.hook_results).unwrap_or(Value::Null),
        &mut receipt.object_refs,
    );
    receipt.object_refs.sort();
    receipt.object_refs.dedup();
    receipt.failure_reason = receipt
        .failure_reason
        .as_ref()
        .map(|reason| redact_secrets(reason));
    let event_id = ws.events()?.append(
        "receipt.created",
        Some(receipt.id.to_string()),
        serde_json::json!({
            "kind": "save",
            "status": receipt.overall_status,
            "subject_id": receipt.changepack_id,
            "hook_receipt_refs": receipt.hook_receipt_refs
        }),
    )?;
    receipt.event_refs.push(event_id.to_string());
    receipt.event_refs.sort();
    receipt.event_refs.dedup();
    receipt.receipt_hash.clear();
    receipt.receipt_hash = hash_json(&receipt)?;
    write_json(
        &ws.layout
            .receipts_dir()
            .join(format!("{}.json", receipt.id)),
        &receipt,
    )?;
    write_json(
        &ws.layout
            .pack_dir(&receipt.changepack_id)
            .join("receipts.json"),
        &receipt,
    )
}

fn write_rollback_receipt(ws: &Workspace, receipt: &mut RollbackReceipt) -> DraftResult<()> {
    ws.events()?.append(
        "receipt.created",
        Some(receipt.id.to_string()),
        serde_json::json!({
            "kind": "rollback",
            "status": receipt.status,
            "subject_id": receipt.rollback_plan_id
        }),
    )?;
    receipt.receipt_hash.clear();
    receipt.receipt_hash = hash_json(receipt)?;
    write_json(
        &ws.layout
            .receipts_dir()
            .join(format!("{}.json", receipt.id)),
        receipt,
    )?;
    Ok(())
}

fn rebuild_index(ws: &Workspace) -> DraftResult<IndexReport> {
    rebuild_index_for_layout(&ws.layout)?;
    let conn = open_index(&ws.layout)?;
    conn.execute("DELETE FROM events", []).map_err(sql_err)?;
    conn.execute("DELETE FROM tasks", []).map_err(sql_err)?;
    conn.execute("DELETE FROM runs", []).map_err(sql_err)?;
    conn.execute("DELETE FROM changepacks", [])
        .map_err(sql_err)?;
    conn.execute("DELETE FROM receipts", []).map_err(sql_err)?;
    conn.execute("DELETE FROM snapshots", []).map_err(sql_err)?;

    let events = ws.events()?.read_all()?;
    for event in &events {
        conn.execute(
            "INSERT INTO events (id, event_type, subject_id, time, event_hash) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.id.to_string(),
                event.event_type,
                event.subject_id,
                event.time.to_rfc3339(),
                event.event_hash
            ],
        )
        .map_err(sql_err)?;
    }

    let tasks: Vec<Task> = load_json_dir(&ws.layout.tasks_dir())?;
    for task in &tasks {
        conn.execute(
            "INSERT INTO tasks (id, title, status, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                task.id.to_string(),
                task.title,
                format!("{:?}", task.status),
                task.created_at.to_rfc3339()
            ],
        )
        .map_err(sql_err)?;
    }

    let runs: Vec<Run> = load_json_dir(&ws.layout.runs_dir())?;
    for run in &runs {
        conn.execute(
            "INSERT INTO runs (id, task_id, status, started_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                run.id.to_string(),
                run.task_id.to_string(),
                format!("{:?}", run.status),
                run.started_at.to_rfc3339()
            ],
        )
        .map_err(sql_err)?;
    }

    let packs = App::new().pack_list(&ws.root)?;
    for pack in &packs {
        conn.execute(
            "INSERT INTO changepacks (id, name, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                pack.id.to_string(),
                pack.name.clone().unwrap_or_default(),
                format!("{:?}", pack.status),
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
                // Legacy receipts use `id`; v0.3.2 signed receipts use `receipt_id`.
                receipt
                    .get("id")
                    .or_else(|| receipt.get("receipt_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                receipt.get("kind").and_then(Value::as_str).unwrap_or_else(|| {
                    if receipt.get("event_type").is_some() {
                        "signed"
                    } else if receipt.get("hook_results").is_some()
                        || receipt.get("overall_status").is_some()
                    {
                        "save"
                    } else {
                        "receipt"
                    }
                }),
                receipt
                    .get("status")
                    .and_then(Value::as_str)
                    .or_else(|| receipt.get("overall_status").and_then(Value::as_str))
                    .unwrap_or_default(),
                receipt
                    .get("subject_id")
                    .and_then(Value::as_str)
                    .or_else(|| receipt.get("changepack_id").and_then(Value::as_str))
                    .unwrap_or_default(),
                receipt
                    .get("created_at")
                    .or_else(|| receipt.get("started_at"))
                    .or_else(|| receipt.get("timestamp"))
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
        runs: runs.len(),
        changepacks: packs.len(),
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
        CREATE TABLE IF NOT EXISTS runs (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            status TEXT NOT NULL,
            started_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS changepacks (
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
        params![SCHEMA_VERSION.to_string()],
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

fn failed_save(
    ws: &Workspace,
    pack: &Changepack,
    started: DateTime<Utc>,
    reason: &str,
) -> DraftResult<SaveReceipt> {
    let store = ObjectStore::new(ws.layout.clone());
    let mut receipt = SaveReceipt {
        schema_version: SCHEMA_VERSION,
        id: ReceiptId::generate(),
        changepack_id: pack.id.clone(),
        actor: resolve_actor(&ws.layout.draft_dir),
        native_save_status: NativeSaveStatus::Failed,
        hook_status: HookStatus::Skipped,
        overall_status: SaveOverallStatus::Failed,
        message_ref: store.put_bytes(b"")?,
        hook_results: Vec::new(),
        hook_receipt_refs: Vec::new(),
        object_refs: Vec::new(),
        event_refs: Vec::new(),
        risk_level: "unknown".to_string(),
        risk_receipt_id: None,
        started_at: started,
        ended_at: now(),
        receipt_hash: String::new(),
        failure_reason: Some(reason.to_string()),
    };
    receipt.receipt_hash = hash_json(&receipt)?;
    write_save_receipt(ws, &receipt)?;
    Ok(receipt)
}

fn render_message(
    cfg: &ResolvedConfig,
    pack: &Changepack,
    patch: &PatchSet,
    receipt_id: &ReceiptId,
) -> String {
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
        "run_id".to_string(),
        pack.run_id
            .as_ref()
            .map(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    );
    values.insert("changepack_id".to_string(), pack.id.to_string());
    values.insert("receipt_id".to_string(), receipt_id.to_string());
    values.insert("actor_name".to_string(), cfg.identity_username.clone());
    values.insert("actor_email".to_string(), cfg.identity_email.clone());
    values.insert("timestamp".to_string(), now().to_rfc3339());
    values.insert(
        "verified".to_string(),
        (!pack.verification_refs.is_empty()).to_string(),
    );
    values.insert("risk_level".to_string(), "unknown".to_string());
    values.insert("files_changed".to_string(), patch.files.len().to_string());
    interpolate_lenient(&cfg.save_message_template, &values)
}

/// The fraction (0.0–1.0) of a candidate's packs that were later rolled back —
/// a risk signal, never a verdict on its own. Returns 0.0 when the candidate
/// has produced no packs. Live values stay 0.0 until candidate attribution is
/// recorded on manifests (e.g. via `draft a2a link`).
fn candidate_rollback_rate(
    events: &[crate::event::EventRecord],
    manifests: &[crate::pack::PackManifest],
    candidate: &str,
) -> f64 {
    let candidate_packs: Vec<&str> = manifests
        .iter()
        .filter(|m| m.candidate.as_deref() == Some(candidate))
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
            DraftErrorKind::SaveFailed,
            format!("cannot apply imported change to '{path}': {why}"),
        )
        .with_suggestion("resolve the local conflict, then re-verify and save again")
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
        .with_suggestion("re-export the pack with draft v0.3.2 or newer (format draftpack/2)"));
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
        let value: Value = read_json(&receipt_path)?;
        // Canonical signed receipts resolve through their subject; anything
        // else takes the legacy path, which requires an explicit rollback
        // target.
        if let Ok(record) = serde_json::from_value::<crate::receipt::ReceiptRecord>(value.clone()) {
            return resolve_canonical_receipt_target(ws, &record);
        }
        if !value
            .get("reversible")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(DraftError::invalid_config(format!(
                "receipt '{reference}' is not reversible"
            )));
        }
        if let Some(target) = value.get("rollback_target").and_then(Value::as_str) {
            if target.starts_with("chk_") {
                validate_checkpoint_id(target)?;
                return load_snapshot(ws, &SnapshotId::new(target));
            }
            if target.starts_with("pck_") {
                validate_pack_id(target)?;
                let pack = load_pack(ws, target)?;
                return load_snapshot(ws, &pack.base_snapshot_id);
            }
        }
        return Err(DraftError::invalid_config(format!(
            "receipt '{reference}' does not describe a reversible rollback target"
        )));
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
    "PackSaved",
];

/// Resolve a canonical signed receipt to a rollback snapshot via its subject.
/// The receipt must verify (fail closed: an unverifiable receipt is not a
/// trustworthy rollback anchor) and its event type must be rollback-eligible.
fn resolve_canonical_receipt_target(
    ws: &Workspace,
    record: &crate::receipt::ReceiptRecord,
) -> DraftResult<Snapshot> {
    let ledger = crate::ledger::TrustLedger::open(&ws.root, ws.id.as_str())?;
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

fn ensure_pack_not_locked(ws: &Workspace, pack: &Changepack) -> DraftResult<()> {
    let lock = ws.layout.pack_dir(&pack.id).join("review.lock.json");
    if lock.exists() {
        return Err(DraftError::new(
            DraftErrorKind::ReviewRequired,
            format!(
                "ChangePack {} is locked for review; approve or reject it before mutating it",
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
        schema_version: SCHEMA_VERSION,
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

fn read_object_pack_index(layout: &DraftLayout) -> DraftResult<ObjectPackIndex> {
    let path = layout.object_packs_dir().join("index.json");
    if !path.exists() {
        return Ok(ObjectPackIndex::default());
    }
    read_json(&path)
}

fn write_object_pack_index(layout: &DraftLayout, index: &ObjectPackIndex) -> DraftResult<()> {
    write_json(&layout.object_packs_dir().join("index.json"), index)
}

fn verify_receipts(ws: &Workspace) -> DraftResult<Vec<String>> {
    let mut errors = Vec::new();
    let event_ids: HashSet<String> = ws
        .events()?
        .read_all()?
        .into_iter()
        .map(|event| event.id.to_string())
        .collect();
    let store = ObjectStore::new(ws.layout.clone());
    for path in list_with_extension(&ws.layout.receipts_dir(), "json")? {
        let text = fs::read_to_string(&path)?;
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(e) => {
                errors.push(format!("{}: invalid JSON: {e}", path.display()));
                continue;
            }
        };
        let Some(expected) = value
            .get("receipt_hash")
            .and_then(Value::as_str)
            .map(ToString::to_string)
        else {
            errors.push(format!("{}: missing receipt_hash", path.display()));
            continue;
        };
        let mut hash_value = value.clone();
        if let Value::Object(map) = &mut hash_value {
            map.insert("receipt_hash".to_string(), Value::String(String::new()));
        }
        let actual = hash_json(&hash_value)?;
        if actual != expected {
            errors.push(format!(
                "{}: receipt hash mismatch: expected {expected}, got {actual}",
                path.display()
            ));
        }
        for object_ref in explicit_string_array(&value, "object_refs") {
            if let Err(e) = store.get_bytes(&object_ref) {
                errors.push(format!(
                    "{}: missing object ref {object_ref}: {e}",
                    path.display()
                ));
            }
        }
        for event_ref in explicit_string_array(&value, "event_refs") {
            if !event_ids.contains(&event_ref) {
                errors.push(format!("{}: missing event ref {event_ref}", path.display()));
            }
        }
        for receipt_ref in explicit_string_array(&value, "hook_receipt_refs") {
            if !ws
                .layout
                .receipts_dir()
                .join(format!("{receipt_ref}.json"))
                .exists()
            {
                errors.push(format!(
                    "{}: missing hook receipt ref {receipt_ref}",
                    path.display()
                ));
            }
        }
    }
    Ok(errors)
}

fn verify_draft_hard_exclusion(ws: &Workspace) -> DraftResult<Vec<String>> {
    let mut errors = Vec::new();
    if !ws.layout.changepacks_dir().exists() {
        return Ok(errors);
    }
    for entry in fs::read_dir(ws.layout.changepacks_dir())? {
        let manifest = entry?.path().join("manifest.json");
        if !manifest.exists() {
            continue;
        }
        let pack: Changepack = read_json(&manifest)?;
        if let Ok(patch) = load_patch(ws, &pack) {
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
    }
    Ok(errors)
}

fn explicit_string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
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
                for line in text.lines().filter(|line| !line.trim().is_empty()) {
                    if let Ok(value) = serde_json::from_str::<Value>(line) {
                        collect_object_refs_from_value(&value, refs);
                    }
                }
            } else if let Ok(value) = serde_json::from_str::<Value>(&text) {
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

fn empty_patch_for_pack(pack: &Changepack) -> PatchSet {
    PatchSet {
        schema_version: SCHEMA_VERSION,
        id: PatchSetId::generate(),
        base_snapshot_id: pack.base_snapshot_id.clone(),
        result_snapshot_id: pack.result_snapshot_id.clone(),
        files: Vec::new(),
        patch_graph_hash: sha256_hex(b"empty"),
    }
}

fn builtin_candidates() -> Vec<CandidateRecord> {
    vec![
        CandidateRecord {
            name: "manual".to_string(),
            kind: "manual".to_string(),
            source: "builtin".to_string(),
            template: "{{instruction}}".to_string(),
            role: None,
            persona: None,
            active: true,
        },
        CandidateRecord {
            name: "codex".to_string(),
            kind: "command".to_string(),
            source: "builtin".to_string(),
            template: "codex {{instruction}}".to_string(),
            role: None,
            persona: None,
            active: true,
        },
        CandidateRecord {
            name: "claude".to_string(),
            kind: "command".to_string(),
            source: "builtin".to_string(),
            template: "claude {{instruction}}".to_string(),
            role: None,
            persona: None,
            active: true,
        },
    ]
}

fn render_candidate_command(template: &str, instruction: &str) -> Vec<String> {
    let rendered = template.replace("{{instruction}}", instruction);
    rendered
        .split_whitespace()
        .map(ToString::to_string)
        .collect()
}

fn redact_secrets(input: &str) -> String {
    let input = redact_pem_blocks(input);
    input
        .lines()
        .map(redact_secret_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_secret_line(line: &str) -> String {
    let mut out = Vec::new();
    let mut redact_next = false;
    for token in line.split_whitespace() {
        if redact_next {
            out.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }
        let lower = token.to_ascii_lowercase();
        if looks_like_standalone_secret(token) {
            out.push("[REDACTED]".to_string());
        } else if let Some(redacted) = redact_assignment_token(token) {
            out.push(redacted);
        } else if sensitive_key_token(&lower) || lower == "bearer" {
            out.push(redact_key_token(token));
            redact_next = true;
        } else {
            out.push(redact_url_credentials(token));
        }
    }
    out.join(" ")
}

fn redact_assignment_token(token: &str) -> Option<String> {
    for sep in ['=', ':'] {
        if let Some((key, _)) = token.split_once(sep) {
            if sensitive_key_token(&key.to_ascii_lowercase()) {
                return Some(format!("{key}{sep}[REDACTED]"));
            }
        }
    }
    None
}

fn redact_key_token(token: &str) -> String {
    let trimmed = token.trim_end_matches([':', '=']);
    if trimmed.len() != token.len() {
        format!("{trimmed}:[REDACTED]")
    } else {
        "[REDACTED]".to_string()
    }
}

fn sensitive_key_token(lower: &str) -> bool {
    [
        "password",
        "passwd",
        "pwd",
        "token",
        "secret",
        "api_key",
        "apikey",
        "access_key",
        "private_key",
        "authorization",
    ]
    .iter()
    .any(|key| lower.contains(key))
}

fn looks_like_standalone_secret(token: &str) -> bool {
    let trimmed = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '_');
    (trimmed.starts_with("eyJ") && trimmed.matches('.').count() >= 2)
        || trimmed.starts_with("AKIA")
        || trimmed.starts_with("ASIA")
        || trimmed.starts_with("ghp_")
        || trimmed.starts_with("xoxb-")
}

fn redact_url_credentials(token: &str) -> String {
    let Some(scheme_idx) = token.find("://") else {
        return token.to_string();
    };
    let authority_start = scheme_idx + 3;
    let authority_end = token[authority_start..]
        .find('/')
        .map(|idx| authority_start + idx)
        .unwrap_or(token.len());
    let Some(at_offset) = token[authority_start..authority_end].find('@') else {
        return token.to_string();
    };
    let at = authority_start + at_offset;
    format!("{}[REDACTED]{}", &token[..authority_start], &token[at..])
}

fn redact_pem_blocks(input: &str) -> String {
    let mut out = Vec::new();
    let mut in_pem = false;
    for line in input.lines() {
        if line.contains("-----BEGIN ") && line.contains("PRIVATE KEY-----") {
            out.push("[REDACTED PEM PRIVATE KEY]".to_string());
            in_pem = true;
            continue;
        }
        if in_pem {
            if line.contains("-----END ") && line.contains("PRIVATE KEY-----") {
                in_pem = false;
            }
            continue;
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

fn redact_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(redact_secrets(&s)),
        Value::Array(values) => Value::Array(values.into_iter().map(redact_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    if [
                        "password",
                        "passwd",
                        "pwd",
                        "token",
                        "secret",
                        "api_key",
                        "apikey",
                        "access_key",
                        "private_key",
                    ]
                    .iter()
                    .any(|needle| lower.contains(needle))
                    {
                        (key, Value::String("[REDACTED]".to_string()))
                    } else {
                        (key, redact_value(value))
                    }
                })
                .collect(),
        ),
        other => other,
    }
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

#[derive(Debug)]
struct HookContext {
    message: String,
    title: String,
    description: String,
    task_id: String,
    run_id: String,
    changepack_id: String,
    receipt_id: String,
    actor_name: String,
    actor_email: String,
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
        ("run_id".to_string(), ctx.run_id.clone()),
        ("changepack_id".to_string(), ctx.changepack_id.clone()),
        ("receipt_id".to_string(), ctx.receipt_id.clone()),
        ("actor_name".to_string(), ctx.actor_name.clone()),
        ("actor_email".to_string(), ctx.actor_email.clone()),
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
    env.insert(
        "DRAFT_CHANGE_PACK_ID".to_string(),
        ctx.changepack_id.clone(),
    );
    env.insert("DRAFT_ACTOR_NAME".to_string(), ctx.actor_name.clone());
    env.insert("DRAFT_ACTOR_EMAIL".to_string(), ctx.actor_email.clone());
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
        "run_id",
        "changepack_id",
        "receipt_id",
        "actor_name",
        "actor_email",
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

fn default_shell() -> String {
    default_hook_shell_runtime().name
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

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
}

fn hash_json<T: Serialize>(value: &T) -> DraftResult<String> {
    let value = serde_json::to_value(value).map_err(json_err)?;
    Ok(sha256_hex(canonical_json(&value).as_bytes()))
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()),
        Value::Array(items) => {
            let inner = items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{inner}]")
        }
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let inner = entries
                .into_iter()
                .map(|(k, v)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_else(|_| "\"\"".to_string()),
                        canonical_json(v)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{inner}}}")
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(input: &str) -> DraftResult<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return Err(DraftError::storage("invalid hex length"));
    }
    let mut out = Vec::with_capacity(input.len() / 2);
    let bytes = input.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let high = hex_value(pair[0])?;
        let low = hex_value(pair[1])?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

fn hex_value(byte: u8) -> DraftResult<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(DraftError::storage("invalid hex byte")),
    }
}

fn json_err(e: serde_json::Error) -> DraftError {
    DraftError::storage(format!("JSON error: {e}"))
}

impl From<serde_json::Error> for DraftError {
    fn from(e: serde_json::Error) -> Self {
        json_err(e)
    }
}

#[cfg(test)]
mod app_tests {
    use super::*;

    fn manifest_for(candidate: Option<&str>, pack_id: &str) -> crate::pack::PackManifest {
        crate::pack::PackManifest {
            schema_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            pack_id: pack_id.to_string(),
            name: pack_id.to_string(),
            description: String::new(),
            intent: crate::pack::PackIntent::Feature,
            origin: "local".into(),
            actor: "act_t".into(),
            candidate: candidate.map(|c| c.to_string()),
            created_at: "2026-07-04T00:00:00+00:00".into(),
            base_workspace_hash: "sha256:a".into(),
            target_workspace_hash: "sha256:b".into(),
            changes_hash: "sha256:c".into(),
            risk_hash: String::new(),
            verify_hash: String::new(),
            lsif_hash: String::new(),
            receipt_hashes: vec![],
            import_state: crate::pack::ImportState::None,
            approval_state: crate::pack::ApprovalState::Pending,
            save_state: crate::pack::SaveState::Unsaved,
        }
    }

    fn rollback_event(subject: &str) -> crate::event::EventRecord {
        crate::event::EventRecord {
            event_id: "evt_t".into(),
            event_type: "RollbackPerformed".into(),
            time: "2026-07-04T00:00:00+00:00".into(),
            subject_id: Some(subject.to_string()),
            actor_id: "act_t".into(),
            candidate_id: None,
            workspace_id: "ws_t".into(),
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
}
