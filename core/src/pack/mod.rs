//! Canonical pack model: manifest, revisions, lifecycle, and archives.
//! machine (PRD §9.5/9.6/9.11/9.12, TDD §17–21).
//!
//! A pack lives at `.draft/packs/pck_<id>/` and is described by an immutable
//! `manifest.json` (identity, intent, provenance, and immutable authorship)
//! plus a `pack.lock.json` (per-file hashes and the tool/policy versions used to
//! verify it). Lifecycle, quarantine, evidence, and rollback remain distinct.

pub mod archive;
pub mod composition;
pub mod lifecycle;
pub mod staging;

use crate::support::error::{DraftError, DraftResult};
use crate::support::fsutil;
use crate::workspace::layout::DraftLayout;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

crate::id_newtype!(PatchSetId, "patch_");

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

/// The canonical pack manifest (`manifest.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackManifest {
    pub schema_version: u32,
    pub pack_id: String,
    pub manifest_digest: String,
    pub name: String,
    pub description: String,
    pub intent: PackIntent,
    pub provenance: Value,
    pub author_id: String,
    pub candidate_id: Option<String>,
    pub declared_dependencies: Vec<String>,
    pub created_at: String,
}

impl crate::contracts::VersionedContract for PackManifest {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::PackManifest;
}

impl PackManifest {
    pub fn recompute_manifest_digest(&self) -> String {
        crate::support::hashing::domain_hash(
            "draft-pack-manifest",
            [crate::support::hashing::canonical_json(&serde_json::json!({
                "schema_version": self.schema_version,
                "pack_id": self.pack_id,
                "name": self.name,
                "description": self.description,
                "intent": self.intent,
                "provenance": self.provenance,
                "author_id": self.author_id,
                "candidate_id": self.candidate_id,
                "declared_dependencies": self.declared_dependencies,
                "created_at": self.created_at,
            }))
            .as_bytes()],
        )
    }

    pub fn refresh_manifest_digest(&mut self) {
        self.manifest_digest = self.recompute_manifest_digest();
    }

    /// Validate that this manifest's schema is supported (fail closed on drift).
    pub fn ensure_supported(&self) -> DraftResult<()> {
        if !crate::contracts::supports_version(
            crate::contracts::ContractId::PackManifest,
            self.schema_version,
        ) {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::UnsupportedSchema,
                format!(
                    "pack manifest schema {} is unsupported",
                    self.schema_version
                ),
            ));
        }
        if self.manifest_digest != self.recompute_manifest_digest() {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "pack manifest digest does not match its immutable content",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackRevision {
    pub schema_version: u32,
    pub pack_id: String,
    pub manifest_digest: String,
    pub revision_id: String,
    pub revision_number: u64,
    pub revision_digest: String,
    pub base_digest: String,
    pub content_digest: String,
    pub diff_digest: String,
    pub target_digest: String,
    pub resolved_dependency_digests: Vec<String>,
    pub created_at: String,
}

impl crate::contracts::VersionedContract for PackRevision {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::PackRevision;
}

impl PackRevision {
    pub fn recompute_revision_digest(&self) -> String {
        let mut revision = self.clone();
        revision.revision_digest.clear();
        let value =
            serde_json::to_value(&revision).expect("pack revisions must be representable as JSON");
        let canonical = crate::support::hashing::canonical_json(&value);
        crate::support::hashing::domain_hash("draft-pack-revision", [canonical.as_bytes()])
    }

    pub fn refresh_revision_digest(&mut self) {
        self.revision_digest = self.recompute_revision_digest();
    }

    pub fn validate(&self, manifest: &PackManifest) -> DraftResult<()> {
        if !crate::contracts::supports_version(
            crate::contracts::ContractId::PackRevision,
            self.schema_version,
        ) {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::UnsupportedSchema,
                format!(
                    "pack revision schema {} is unsupported",
                    self.schema_version
                ),
            ));
        }
        if self.pack_id != manifest.pack_id || self.manifest_digest != manifest.manifest_digest {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "pack revision is bound to a different manifest",
            ));
        }
        if self.revision_digest != self.recompute_revision_digest() {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "pack revision digest mismatch",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackQuarantineRecord {
    pub schema_version: u32,
    pub pack_id: String,
    pub revision_id: String,
    pub revision_digest: String,
    pub storage_location: String,
    pub source: String,
    pub artifact_digest: String,
    pub trust_evaluation: QuarantineState,
    pub quarantined_at: String,
    pub promoted_at: Option<String>,
}

impl crate::contracts::VersionedContract for PackQuarantineRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::PackQuarantine;
}

/// Security/trust evaluation for an imported artifact. This is deliberately
/// separate from the pack's review lifecycle.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineState {
    Quarantined,
    Verified,
    Approved,
    Rejected,
    Promoted,
}

/// The pack lockfile (`pack.lock.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackLockfile {
    pub schema_version: u32,
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
    pub receipt_digests: Vec<String>,
}

impl crate::contracts::VersionedContract for PackLockfile {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::PackLock;
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

/// Validate an import trust transition. Review state remains in the canonical
/// lifecycle record and is never encoded here.
pub fn can_quarantine_transition(from: QuarantineState, to: QuarantineState) -> bool {
    use QuarantineState::*;
    matches!(
        (from, to),
        (Quarantined, Verified)
            | (Verified, Verified)
            | (Approved, Verified)
            | (Verified, Approved)
            | (Quarantined, Rejected)
            | (Verified, Rejected)
            | (Approved, Rejected)
            | (Approved, Promoted)
    )
}

/// Persistence for pack manifests and lockfiles.
pub struct PackStore {
    paths: DraftLayout,
}

impl PackStore {
    pub fn new(paths: DraftLayout) -> Self {
        PackStore { paths }
    }

    pub fn exists(&self, pack_id: &str) -> bool {
        self.paths.pack_manifest(pack_id).exists()
    }

    pub fn write_manifest(&self, manifest: &PackManifest) -> DraftResult<()> {
        self.write_manifest_in(PackLocation::Store, manifest)
    }

    pub fn read_manifest(&self, pack_id: &str) -> DraftResult<PackManifest> {
        let path = self.paths.pack_manifest(pack_id);
        if !path.exists() {
            return Err(DraftError::not_found(format!("pack {pack_id} not found")));
        }
        let manifest: PackManifest = crate::contracts::read_persisted(&path)?;
        manifest.ensure_supported()?;
        Ok(manifest)
    }

    pub fn write_lockfile(&self, lock: &PackLockfile) -> DraftResult<()> {
        fsutil::ensure_dir(&self.paths.pack_dir(&lock.pack_id))?;
        fsutil::write_json(&self.paths.pack_lock(&lock.pack_id), lock)
    }

    pub fn read_lockfile(&self, pack_id: &str) -> DraftResult<PackLockfile> {
        crate::contracts::read_persisted(&self.paths.pack_lock(pack_id))
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
                    out.push(self.read_manifest(id)?);
                } else {
                    return Err(DraftError::new(
                        crate::support::error::DraftErrorKind::CorruptData,
                        "pack directory name is not valid UTF-8",
                    ));
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
        let manifest: PackManifest = crate::contracts::read_persisted(&path)?;
        manifest.ensure_supported()?;
        Ok(manifest)
    }

    pub fn write_manifest_in(&self, loc: PackLocation, manifest: &PackManifest) -> DraftResult<()> {
        let dir = self.dir_for(loc, &manifest.pack_id);
        fsutil::ensure_dir(&dir)?;
        let mut manifest = manifest.clone();
        manifest.refresh_manifest_digest();
        let path = dir.join("manifest.json");
        if path.exists() {
            let existing: PackManifest = crate::contracts::read_persisted(&path)?;
            existing.ensure_supported()?;
            if existing != manifest {
                return Err(DraftError::new(
                    crate::support::error::DraftErrorKind::ConflictDetected,
                    "immutable pack manifest already exists with different content",
                ));
            }
            return Ok(());
        }
        fsutil::write_json(&path, &manifest)
    }

    pub fn write_revision(&self, revision: &PackRevision) -> DraftResult<()> {
        self.write_revision_in(PackLocation::Store, revision)
    }

    pub fn write_revision_in(&self, loc: PackLocation, revision: &PackRevision) -> DraftResult<()> {
        let manifest = self.read_manifest_in(loc, &revision.pack_id)?;
        revision.validate(&manifest)?;
        let path = self
            .dir_for(loc, &revision.pack_id)
            .join("revisions")
            .join(format!("{}.json", revision.revision_id));
        if path.exists() {
            let existing: PackRevision = crate::contracts::read_persisted(&path)?;
            if existing != *revision {
                return Err(DraftError::new(
                    crate::support::error::DraftErrorKind::ConflictDetected,
                    "immutable pack revision already exists with different content",
                ));
            }
            return Ok(());
        }
        fsutil::write_json(&path, revision)
    }

    pub fn revisions(&self, pack_id: &str) -> DraftResult<Vec<PackRevision>> {
        self.revisions_in(PackLocation::Store, pack_id)
    }

    pub fn revisions_in(&self, loc: PackLocation, pack_id: &str) -> DraftResult<Vec<PackRevision>> {
        let manifest = self.read_manifest_in(loc, pack_id)?;
        let directory = self.dir_for(loc, pack_id).join("revisions");
        let mut revisions = Vec::new();
        for path in fsutil::list_with_extension(&directory, "json")? {
            let revision: PackRevision = crate::contracts::read_persisted(&path)?;
            revision.validate(&manifest)?;
            revisions.push(revision);
        }
        revisions.sort_by(|left, right| {
            left.revision_number
                .cmp(&right.revision_number)
                .then_with(|| left.revision_id.cmp(&right.revision_id))
        });
        Ok(revisions)
    }

    pub fn write_lifecycle_in(
        &self,
        loc: PackLocation,
        record: &lifecycle::PackLifecycleRecord,
    ) -> DraftResult<()> {
        let revision = self
            .revisions_in(loc, &record.pack_id)?
            .into_iter()
            .find(|revision| revision.revision_id == record.revision_id)
            .ok_or_else(|| {
                DraftError::new(
                    crate::support::error::DraftErrorKind::CorruptData,
                    "pack lifecycle references a missing revision",
                )
            })?;
        if revision.revision_digest != record.revision_digest {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "pack lifecycle revision digest does not match the immutable revision",
            ));
        }
        fsutil::write_json(
            &self.dir_for(loc, &record.pack_id).join("lifecycle.json"),
            record,
        )
    }

    pub fn read_lifecycle_in(
        &self,
        loc: PackLocation,
        pack_id: &str,
    ) -> DraftResult<lifecycle::PackLifecycleRecord> {
        let record: lifecycle::PackLifecycleRecord =
            crate::contracts::read_persisted(&self.dir_for(loc, pack_id).join("lifecycle.json"))?;
        if record.pack_id != pack_id {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "pack lifecycle identity does not match its containing pack",
            ));
        }
        let revision = self
            .revisions_in(loc, pack_id)?
            .into_iter()
            .find(|revision| revision.revision_id == record.revision_id)
            .ok_or_else(|| {
                DraftError::new(
                    crate::support::error::DraftErrorKind::CorruptData,
                    "pack lifecycle references a missing revision",
                )
            })?;
        if revision.revision_digest != record.revision_digest {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "pack lifecycle revision digest mismatch",
            ));
        }
        Ok(record)
    }

    pub fn current_revision_in(
        &self,
        loc: PackLocation,
        pack_id: &str,
    ) -> DraftResult<PackRevision> {
        let lifecycle = self.read_lifecycle_in(loc, pack_id)?;
        self.revisions_in(loc, pack_id)?
            .into_iter()
            .find(|revision| revision.revision_id == lifecycle.revision_id)
            .ok_or_else(|| {
                DraftError::new(
                    crate::support::error::DraftErrorKind::CorruptData,
                    "current pack revision is missing",
                )
            })
    }

    pub fn read_quarantine(&self, pack_id: &str) -> DraftResult<PackQuarantineRecord> {
        let loc = self
            .locate(pack_id)
            .ok_or_else(|| DraftError::not_found(format!("pack {pack_id} not found")))?;
        let record: PackQuarantineRecord =
            crate::contracts::read_persisted(&self.dir_for(loc, pack_id).join("quarantine.json"))?;
        let revision = self.current_revision_in(loc, pack_id)?;
        if record.pack_id != pack_id
            || record.revision_id != revision.revision_id
            || record.revision_digest != revision.revision_digest
        {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "quarantine record is not bound to the current immutable revision",
            ));
        }
        Ok(record)
    }

    pub fn write_quarantine(&self, record: &PackQuarantineRecord) -> DraftResult<()> {
        let loc = self
            .locate(&record.pack_id)
            .ok_or_else(|| DraftError::not_found(format!("pack {} not found", record.pack_id)))?;
        let revision = self.current_revision_in(loc, &record.pack_id)?;
        if record.revision_id != revision.revision_id
            || record.revision_digest != revision.revision_digest
        {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "quarantine record revision binding is invalid",
            ));
        }
        fsutil::write_json(
            &self.dir_for(loc, &record.pack_id).join("quarantine.json"),
            record,
        )
    }

    pub fn is_quarantined(&self, pack_id: &str) -> bool {
        self.paths
            .quarantine_dir()
            .join(pack_id)
            .join("quarantine.json")
            .exists()
    }

    pub fn quarantine_record(&self, pack_id: &str) -> DraftResult<Option<PackQuarantineRecord>> {
        let Some(loc) = self.locate(pack_id) else {
            return Ok(None);
        };
        let path = self.dir_for(loc, pack_id).join("quarantine.json");
        if !path.exists() {
            return Ok(None);
        }
        self.read_quarantine(pack_id).map(Some)
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
                    out.push(self.read_manifest_in(PackLocation::Quarantine, id)?);
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
        let mut quarantine = self.read_quarantine(pack_id)?;
        if quarantine.trust_evaluation != QuarantineState::Approved {
            return Err(DraftError::invalid_config(
                "only an approved quarantined pack may be promoted",
            ));
        }
        quarantine.trust_evaluation = QuarantineState::Promoted;
        quarantine.storage_location = "pack_store".into();
        quarantine.promoted_at = Some(crate::support::common::now().to_rfc3339());
        self.write_quarantine(&quarantine)?;
        fsutil::ensure_dir(&self.paths.packs_dir())?;
        std::fs::rename(&from, &to)
            .map_err(|e| DraftError::storage(format!("promote {pack_id} from quarantine: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PackManifest {
        PackManifest {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::PackManifest,
            ),
            pack_id: "pck_test".into(),
            manifest_digest: String::new(),
            name: "auth".into(),
            description: "desc".into(),
            intent: PackIntent::Refactor,
            provenance: serde_json::json!({"origin": "test"}),
            author_id: "act_1".into(),
            candidate_id: None,
            declared_dependencies: Vec::new(),
            created_at: "2026-07-03T00:00:00+00:00".into(),
        }
    }

    fn revision(manifest: &PackManifest) -> PackRevision {
        let mut revision = PackRevision {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::PackRevision,
            ),
            pack_id: manifest.pack_id.clone(),
            manifest_digest: manifest.manifest_digest.clone(),
            revision_id: "rev_test".into(),
            revision_number: 1,
            revision_digest: String::new(),
            base_digest: "sha256:a".into(),
            content_digest: "sha256:b".into(),
            diff_digest: "sha256:c".into(),
            target_digest: "sha256:d".into(),
            resolved_dependency_digests: Vec::new(),
            created_at: "2026-07-03T00:00:00+00:00".into(),
        };
        revision.refresh_revision_digest();
        revision
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
    fn unsupported_schema_rejected() {
        let mut m = manifest();
        m.schema_version = 2;
        assert_eq!(
            m.ensure_supported().unwrap_err().kind,
            crate::support::error::DraftErrorKind::UnsupportedSchema
        );
    }

    #[test]
    fn quarantine_transitions_are_separate_from_review_lifecycle() {
        use QuarantineState::*;
        assert!(can_quarantine_transition(Quarantined, Verified));
        assert!(can_quarantine_transition(Verified, Approved));
        assert!(can_quarantine_transition(Approved, Promoted));
        assert!(can_quarantine_transition(Approved, Verified));
        assert!(can_quarantine_transition(Verified, Rejected));
        assert!(!can_quarantine_transition(Quarantined, Approved));
        assert!(!can_quarantine_transition(Rejected, Verified));
        assert!(!can_quarantine_transition(Promoted, Verified));
    }

    #[test]
    fn store_locates_and_promotes_quarantined_pack() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PackStore::new(DraftLayout::for_root(tmp.path()));
        let m = manifest();
        assert_eq!(store.locate("pck_test"), None);

        store
            .write_manifest_in(PackLocation::Quarantine, &m)
            .unwrap();
        let mut expected = m.clone();
        expected.refresh_manifest_digest();
        let revision = revision(&expected);
        store
            .write_revision_in(PackLocation::Quarantine, &revision)
            .unwrap();
        store
            .write_lifecycle_in(
                PackLocation::Quarantine,
                &lifecycle::PackLifecycleRecord {
                    schema_version: crate::contracts::current_version(
                        crate::contracts::ContractId::PackManifest,
                    ),
                    pack_id: expected.pack_id.clone(),
                    revision_id: revision.revision_id.clone(),
                    revision_digest: revision.revision_digest.clone(),
                    lifecycle: lifecycle::PackLifecycle::Approved,
                    updated_at: crate::support::common::now(),
                    last_operation_id: crate::support::common::OperationId::new("op_test"),
                },
            )
            .unwrap();
        store
            .write_quarantine(&PackQuarantineRecord {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::PackRevision,
                ),
                pack_id: expected.pack_id.clone(),
                revision_id: revision.revision_id.clone(),
                revision_digest: revision.revision_digest.clone(),
                storage_location: "quarantine".into(),
                source: "fixture".into(),
                artifact_digest: "sha256:artifact".into(),
                trust_evaluation: QuarantineState::Approved,
                quarantined_at: "2026-07-03T00:00:00+00:00".into(),
                promoted_at: None,
            })
            .unwrap();
        assert_eq!(store.locate("pck_test"), Some(PackLocation::Quarantine));
        assert_eq!(
            store
                .read_manifest_in(PackLocation::Quarantine, "pck_test")
                .unwrap(),
            expected
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
        let store = PackStore::new(DraftLayout::for_root(tmp.path()));
        let m = manifest();
        store.write_manifest(&m).unwrap();
        let mut expected = m.clone();
        expected.refresh_manifest_digest();
        assert!(store.exists("pck_test"));
        assert_eq!(store.read_manifest("pck_test").unwrap(), expected);
        assert!(store.name_taken("auth").unwrap());
        assert!(!store.name_taken("other").unwrap());

        let mut changed = m;
        changed.description = "mutated".into();
        assert_eq!(
            store.write_manifest(&changed).unwrap_err().kind,
            crate::support::error::DraftErrorKind::ConflictDetected
        );
    }
}
