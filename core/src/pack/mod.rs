//! Canonical v0.3.3 pack model: manifest, lockfile, intents, and the pack state
//! machine (PRD §9.5/9.6/9.11/9.12, TDD §17–21).
//!
//! A pack lives at `.draft/packs/pck_<id>/` and is described by an immutable
//! `manifest.json` (identity, intent, provenance, content/risk/verify hashes)
//! plus a `pack.lock.json` (per-file hashes and the tool/policy versions used to
//! verify it). Lifecycle is tracked by three orthogonal states — import,
//! approval, and save — whose combination yields the PRD lifecycle label.

use crate::error::{DraftError, DraftResult};
use crate::fsutil;
use crate::layout::ProjectPaths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Declared intent of a pack; risk policy reasons over this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackIntent {
    Feature,
    Bugfix,
    Refactor,
    Security,
    Migration,
    Docs,
    TestOnly,
    DependencyUpdate,
    Cleanup,
    Generated,
}

impl PackIntent {
    pub fn as_str(&self) -> &'static str {
        match self {
            PackIntent::Feature => "feature",
            PackIntent::Bugfix => "bugfix",
            PackIntent::Refactor => "refactor",
            PackIntent::Security => "security",
            PackIntent::Migration => "migration",
            PackIntent::Docs => "docs",
            PackIntent::TestOnly => "test-only",
            PackIntent::DependencyUpdate => "dependency-update",
            PackIntent::Cleanup => "cleanup",
            PackIntent::Generated => "generated",
        }
    }

    pub fn parse(s: &str) -> DraftResult<Self> {
        let v = match s {
            "feature" => PackIntent::Feature,
            "bugfix" => PackIntent::Bugfix,
            "refactor" => PackIntent::Refactor,
            "security" => PackIntent::Security,
            "migration" => PackIntent::Migration,
            "docs" => PackIntent::Docs,
            "test-only" => PackIntent::TestOnly,
            "dependency-update" => PackIntent::DependencyUpdate,
            "cleanup" => PackIntent::Cleanup,
            "generated" => PackIntent::Generated,
            other => {
                return Err(DraftError::invalid_config(format!(
                    "unknown pack intent '{other}'"
                )))
            }
        };
        Ok(v)
    }
}

/// Import lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportState {
    /// Not an imported pack.
    None,
    ImportedQuarantined,
    ImportVerified,
    ImportApproved,
    ImportSaved,
    ImportRejected,
}

/// Approval lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    Approved,
    Rejected,
}

/// Save lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveState {
    Unsaved,
    Saved,
    RolledBack,
}

/// The canonical pack manifest (`manifest.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackManifest {
    pub schema_version: String,
    pub pack_id: String,
    pub name: String,
    pub description: String,
    pub intent: PackIntent,
    pub origin: String,
    pub actor: String,
    pub candidate: Option<String>,
    pub created_at: String,
    pub base_workspace_hash: String,
    pub target_workspace_hash: String,
    pub changes_hash: String,
    pub risk_hash: String,
    pub verify_hash: String,
    pub lsif_hash: String,
    pub receipt_hashes: Vec<String>,
    pub import_state: ImportState,
    pub approval_state: ApprovalState,
    pub save_state: SaveState,
}

impl PackManifest {
    /// True once a verification result has been recorded (verify_hash set).
    pub fn is_verified(&self) -> bool {
        !self.verify_hash.is_empty() && self.verify_hash != crate::hashing::sha256_hex(b"")
    }

    /// Derive the PRD lifecycle label from the orthogonal states.
    pub fn lifecycle(&self) -> &'static str {
        if self.import_state != ImportState::None {
            return match self.import_state {
                ImportState::ImportedQuarantined => "imported_quarantined",
                ImportState::ImportVerified => "import_verified",
                ImportState::ImportApproved => "import_approved",
                ImportState::ImportSaved => "import_saved",
                ImportState::ImportRejected => "import_rejected",
                ImportState::None => unreachable!(),
            };
        }
        match (self.approval_state, self.save_state) {
            (_, SaveState::RolledBack) => "rolled_back",
            (_, SaveState::Saved) => "saved",
            (ApprovalState::Rejected, _) => "rejected",
            (ApprovalState::Approved, _) => "approved",
            (ApprovalState::Pending, _) if self.is_verified() => "verified",
            _ => "created",
        }
    }

    /// Validate that this manifest's schema is supported (fail closed on drift).
    pub fn ensure_supported(&self) -> DraftResult<()> {
        if self.schema_version != crate::DRAFT_SCHEMA_VERSION {
            return Err(DraftError::invalid_config(format!(
                "pack manifest schema {} is unsupported (expected {})",
                self.schema_version,
                crate::DRAFT_SCHEMA_VERSION
            )));
        }
        Ok(())
    }
}

/// The pack lockfile (`pack.lock.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackLockfile {
    pub schema_version: String,
    pub pack_id: String,
    pub workspace_hash: String,
    pub file_hashes: BTreeMap<String, String>,
    pub policy_version: String,
    pub risk_engine_version: String,
    pub verification_commands: Vec<LockedCommand>,
    pub lsif_version: String,
    pub test_selector_version: String,
    pub fuzz_selector_version: String,
    pub dependency_pack_hashes: Vec<String>,
    pub receipt_hashes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedCommand {
    pub command: String,
    pub command_hash: String,
}

/// Where a canonical pack currently lives on disk: the trusted pack store or
/// the import quarantine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackLocation {
    Store,
    Quarantine,
}

/// Validate an imported-pack state transition (PRD lifecycle):
/// quarantined → verified → approved → saved; rejected is terminal; a
/// re-verification is allowed from verified/approved and resets approval.
pub fn can_import_transition(from: ImportState, to: ImportState) -> bool {
    use ImportState::*;
    matches!(
        (from, to),
        (ImportedQuarantined, ImportVerified)
            | (ImportVerified, ImportVerified)
            | (ImportApproved, ImportVerified)
            | (ImportVerified, ImportApproved)
            | (ImportedQuarantined, ImportRejected)
            | (ImportVerified, ImportRejected)
            | (ImportApproved, ImportRejected)
            | (ImportApproved, ImportSaved)
    )
}

/// Persistence for pack manifests and lockfiles.
pub struct PackStore {
    paths: ProjectPaths,
}

impl PackStore {
    pub fn new(paths: ProjectPaths) -> Self {
        PackStore { paths }
    }

    pub fn exists(&self, pack_id: &str) -> bool {
        self.paths.pack_manifest(pack_id).exists()
    }

    pub fn write_manifest(&self, manifest: &PackManifest) -> DraftResult<()> {
        fsutil::ensure_dir(&self.paths.pack_dir(&manifest.pack_id))?;
        fsutil::write_json(&self.paths.pack_manifest(&manifest.pack_id), manifest)
    }

    pub fn read_manifest(&self, pack_id: &str) -> DraftResult<PackManifest> {
        let path = self.paths.pack_manifest(pack_id);
        if !path.exists() {
            return Err(DraftError::not_found(format!("pack {pack_id} not found")));
        }
        let manifest: PackManifest = fsutil::read_json(&path)?;
        manifest.ensure_supported()?;
        Ok(manifest)
    }

    pub fn write_lockfile(&self, lock: &PackLockfile) -> DraftResult<()> {
        fsutil::ensure_dir(&self.paths.pack_dir(&lock.pack_id))?;
        fsutil::write_json(&self.paths.pack_lock(&lock.pack_id), lock)
    }

    pub fn read_lockfile(&self, pack_id: &str) -> DraftResult<PackLockfile> {
        fsutil::read_json(&self.paths.pack_lock(pack_id))
    }

    pub fn list(&self) -> DraftResult<Vec<PackManifest>> {
        let mut out = Vec::new();
        let dir = self.paths.packs_dir();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(&dir)
            .map_err(|e| DraftError::storage(format!("read packs dir: {e}")))?
        {
            let entry = entry.map_err(|e| DraftError::storage(e.to_string()))?;
            if entry.path().is_dir() {
                if let Some(id) = entry.file_name().to_str() {
                    if let Ok(m) = self.read_manifest(id) {
                        out.push(m);
                    }
                }
            }
        }
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(out)
    }

    /// Enforce unique pack names within the workspace.
    pub fn name_taken(&self, name: &str) -> DraftResult<bool> {
        Ok(self.list()?.iter().any(|m| m.name == name))
    }

    /// Find where a pack currently lives: trusted store first, then quarantine.
    pub fn locate(&self, pack_id: &str) -> Option<PackLocation> {
        if self.paths.pack_manifest(pack_id).exists() {
            return Some(PackLocation::Store);
        }
        if self
            .paths
            .quarantine_dir()
            .join(pack_id)
            .join("manifest.json")
            .exists()
        {
            return Some(PackLocation::Quarantine);
        }
        None
    }

    /// The on-disk directory for a pack at the given location.
    pub fn dir_for(&self, loc: PackLocation, pack_id: &str) -> std::path::PathBuf {
        match loc {
            PackLocation::Store => self.paths.pack_dir(pack_id),
            PackLocation::Quarantine => self.paths.quarantine_dir().join(pack_id),
        }
    }

    pub fn read_manifest_in(&self, loc: PackLocation, pack_id: &str) -> DraftResult<PackManifest> {
        if loc == PackLocation::Store {
            return self.read_manifest(pack_id);
        }
        let path = self.dir_for(loc, pack_id).join("manifest.json");
        if !path.exists() {
            return Err(DraftError::not_found(format!("pack {pack_id} not found")));
        }
        let manifest: PackManifest = fsutil::read_json(&path)?;
        manifest.ensure_supported()?;
        Ok(manifest)
    }

    pub fn write_manifest_in(&self, loc: PackLocation, manifest: &PackManifest) -> DraftResult<()> {
        if loc == PackLocation::Store {
            return self.write_manifest(manifest);
        }
        let dir = self.dir_for(loc, &manifest.pack_id);
        fsutil::ensure_dir(&dir)?;
        fsutil::write_json(&dir.join("manifest.json"), manifest)
    }

    /// All quarantined imported packs, ordered by creation time.
    pub fn list_quarantined(&self) -> DraftResult<Vec<PackManifest>> {
        let mut out = Vec::new();
        let dir = self.paths.quarantine_dir();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(&dir)
            .map_err(|e| DraftError::storage(format!("read quarantine dir: {e}")))?
        {
            let entry = entry.map_err(|e| DraftError::storage(e.to_string()))?;
            if entry.path().is_dir() {
                if let Some(id) = entry.file_name().to_str() {
                    if let Ok(m) = self.read_manifest_in(PackLocation::Quarantine, id) {
                        out.push(m);
                    }
                }
            }
        }
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(out)
    }

    /// Move a quarantined pack (its whole evidence directory) into the trusted
    /// pack store. Fails if the destination already exists.
    pub fn promote_from_quarantine(&self, pack_id: &str) -> DraftResult<()> {
        let from = self.dir_for(PackLocation::Quarantine, pack_id);
        let to = self.dir_for(PackLocation::Store, pack_id);
        if to.exists() {
            return Err(DraftError::storage(format!(
                "pack {pack_id} already exists in the pack store"
            )));
        }
        fsutil::ensure_dir(&self.paths.packs_dir())?;
        std::fs::rename(&from, &to)
            .map_err(|e| DraftError::storage(format!("promote {pack_id} from quarantine: {e}")))
    }
}

/// Validate a local (non-imported) approval transition.
pub fn can_approve(manifest: &PackManifest) -> bool {
    manifest.import_state == ImportState::None
        && manifest.is_verified()
        && manifest.approval_state == ApprovalState::Pending
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PackManifest {
        PackManifest {
            schema_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            pack_id: "pck_test".into(),
            name: "auth".into(),
            description: "desc".into(),
            intent: PackIntent::Refactor,
            origin: "local".into(),
            actor: "act_1".into(),
            candidate: None,
            created_at: "2026-07-03T00:00:00+00:00".into(),
            base_workspace_hash: "sha256:a".into(),
            target_workspace_hash: "sha256:b".into(),
            changes_hash: "sha256:c".into(),
            risk_hash: "sha256:d".into(),
            verify_hash: String::new(),
            lsif_hash: "sha256:e".into(),
            receipt_hashes: vec![],
            import_state: ImportState::None,
            approval_state: ApprovalState::Pending,
            save_state: SaveState::Unsaved,
        }
    }

    #[test]
    fn intent_roundtrip_and_parse() {
        for s in [
            "feature",
            "bugfix",
            "refactor",
            "security",
            "migration",
            "docs",
            "test-only",
            "dependency-update",
            "cleanup",
            "generated",
        ] {
            assert_eq!(PackIntent::parse(s).unwrap().as_str(), s);
        }
        assert!(PackIntent::parse("nonsense").is_err());
    }

    #[test]
    fn lifecycle_labels() {
        let mut m = manifest();
        assert_eq!(m.lifecycle(), "created");
        m.verify_hash = "sha256:v".into();
        assert_eq!(m.lifecycle(), "verified");
        m.approval_state = ApprovalState::Approved;
        assert_eq!(m.lifecycle(), "approved");
        m.save_state = SaveState::Saved;
        assert_eq!(m.lifecycle(), "saved");
        m.save_state = SaveState::RolledBack;
        assert_eq!(m.lifecycle(), "rolled_back");

        let mut imp = manifest();
        imp.import_state = ImportState::ImportedQuarantined;
        assert_eq!(imp.lifecycle(), "imported_quarantined");
    }

    #[test]
    fn approve_requires_verified_local_pending() {
        let mut m = manifest();
        assert!(!can_approve(&m)); // not verified
        m.verify_hash = "sha256:v".into();
        assert!(can_approve(&m));
        m.import_state = ImportState::ImportedQuarantined;
        assert!(!can_approve(&m)); // imported packs use import states
    }

    #[test]
    fn unsupported_schema_rejected() {
        let mut m = manifest();
        m.schema_version = "0.3.1".into();
        assert!(m.ensure_supported().is_err());
    }

    #[test]
    fn import_state_transitions_follow_lifecycle() {
        use ImportState::*;
        // Forward path.
        assert!(can_import_transition(ImportedQuarantined, ImportVerified));
        assert!(can_import_transition(ImportVerified, ImportApproved));
        assert!(can_import_transition(ImportApproved, ImportSaved));
        // Re-verification resets approval; allowed from verified/approved.
        assert!(can_import_transition(ImportVerified, ImportVerified));
        assert!(can_import_transition(ImportApproved, ImportVerified));
        // Rejection from any non-terminal state.
        assert!(can_import_transition(ImportedQuarantined, ImportRejected));
        assert!(can_import_transition(ImportVerified, ImportRejected));
        assert!(can_import_transition(ImportApproved, ImportRejected));
        // Illegal jumps and terminal states.
        assert!(!can_import_transition(ImportedQuarantined, ImportApproved));
        assert!(!can_import_transition(ImportedQuarantined, ImportSaved));
        assert!(!can_import_transition(ImportVerified, ImportSaved));
        assert!(!can_import_transition(ImportRejected, ImportVerified));
        assert!(!can_import_transition(ImportSaved, ImportVerified));
        assert!(!can_import_transition(ImportSaved, ImportRejected));
    }

    #[test]
    fn store_locates_and_promotes_quarantined_pack() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PackStore::new(ProjectPaths::for_root(tmp.path()));
        let mut m = manifest();
        m.import_state = ImportState::ImportedQuarantined;
        assert_eq!(store.locate("pck_test"), None);

        store
            .write_manifest_in(PackLocation::Quarantine, &m)
            .unwrap();
        assert_eq!(store.locate("pck_test"), Some(PackLocation::Quarantine));
        assert_eq!(
            store
                .read_manifest_in(PackLocation::Quarantine, "pck_test")
                .unwrap(),
            m
        );
        assert_eq!(store.list_quarantined().unwrap().len(), 1);

        store.promote_from_quarantine("pck_test").unwrap();
        assert_eq!(store.locate("pck_test"), Some(PackLocation::Store));
        assert!(store.list_quarantined().unwrap().is_empty());
        assert!(!store.dir_for(PackLocation::Quarantine, "pck_test").exists());
    }

    #[test]
    fn manifest_store_roundtrip_and_unique_names() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PackStore::new(ProjectPaths::for_root(tmp.path()));
        let m = manifest();
        store.write_manifest(&m).unwrap();
        assert!(store.exists("pck_test"));
        assert_eq!(store.read_manifest("pck_test").unwrap(), m);
        assert!(store.name_taken("auth").unwrap());
        assert!(!store.name_taken("other").unwrap());
    }
}
