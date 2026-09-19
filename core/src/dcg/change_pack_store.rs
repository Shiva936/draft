//! Where a ChangePack's content lives: manifests, content revisions, lockfiles.
//!
//! The authoritative ChangePack *record* — its generation, lifecycle and current
//! definition — is [`crate::dcg::change_pack::ChangePackStore`]. This is the separate
//! question of where the bytes of each revision sit on disk, and it is the
//! store every read of a manifest, lockfile or review-progress record goes through.

use crate::project::layout::DraftLayout;
use crate::support::error::{DraftError, DraftResult};
use crate::support::fsutil;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The intent a ChangePack declares.
///
/// Core stores intents and compares them; it never interprets one. `feature`,
/// `bug-fix` and `colour-grade` are domain vocabulary, contributed through an
/// `intent_vocabulary` and resolved against the active contributions when a
/// ChangePack is created. The single intent Core owns is
/// [`UNSPECIFIED_INTENT`], so a project with no vocabulary installed can still
/// declare a ChangePack rather than being blocked on an extension.
pub type IntentId = draft_extension_contract::NamespacedId;

/// The one intent Core owns: "this project has not said".
///
/// It is not a neutral synonym for any domain intent, and no rule may assume it
/// means low risk — it means the vocabulary that would have named the intent is
/// not installed.
pub const UNSPECIFIED_INTENT: &str = "draft.core/unspecified";

/// The intent every workspace can always declare, whatever is installed.
pub fn unspecified_intent() -> IntentId {
    IntentId::parse(UNSPECIFIED_INTENT).expect("the core-owned intent id is well formed")
}

/// Parse a declared intent and check that something actually declares it.
///
/// Shape alone is not enough: an id that parses but that no installed
/// vocabulary declares would silently never match a rule, so it is refused here
/// with the list of intents that *are* available.
pub fn resolve_intent(
    value: &str,
    declared: &std::collections::BTreeSet<String>,
) -> DraftResult<IntentId> {
    let intent = IntentId::parse(value).map_err(|error| {
        DraftError::invalid_config(format!("invalid ChangePack intent '{value}': {error}"))
    })?;
    if intent.qualified() == UNSPECIFIED_INTENT || declared.contains(&intent.qualified()) {
        return Ok(intent);
    }
    let mut available: Vec<&str> = declared.iter().map(String::as_str).collect();
    available.push(UNSPECIFIED_INTENT);
    available.sort_unstable();
    Err(DraftError::invalid_config(format!(
        "no installed extension declares the intent '{value}'; available: {}",
        available.join(", ")
    ))
    .with_suggestion(
        "install an extension contributing an `intent_vocabulary`, or declare \
         `draft.core/unspecified`",
    ))
}

/// The canonical ChangePack manifest (`manifest.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChangePackManifest {
    pub schema_version: u32,
    pub change_pack_id: String,
    pub manifest_digest: String,
    pub name: String,
    pub description: String,
    pub intent: IntentId,
    pub provenance: Value,
    pub author_id: String,
    pub candidate_id: Option<String>,
    pub declared_dependencies: Vec<String>,
    pub created_at: String,
}

impl crate::contracts::VersionedContract for ChangePackManifest {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::ChangePackManifest;
}

impl ChangePackManifest {
    pub fn recompute_manifest_digest(&self) -> String {
        crate::support::hashing::domain_hash(
            "draft-change-pack-manifest",
            [crate::support::hashing::canonical_json(&serde_json::json!({
                "schema_version": self.schema_version,
                "change_pack_id": self.change_pack_id,
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
            crate::contracts::ContractId::ChangePackManifest,
            self.schema_version,
        ) {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::UnsupportedSchema,
                format!(
                    "ChangePack manifest schema {} is unsupported",
                    self.schema_version
                ),
            ));
        }
        if self.manifest_digest != self.recompute_manifest_digest() {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "ChangePack manifest digest does not match its immutable content",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChangePackContentRevisionRecord {
    pub schema_version: u32,
    pub change_pack_id: String,
    pub manifest_digest: String,
    pub content_revision_id: String,
    pub content_revision_number: u64,
    pub content_revision_digest: String,
    pub base_digest: String,
    pub content_digest: String,
    pub change_set_digest: String,
    pub target_digest: String,
    pub resolved_dependency_digests: Vec<String>,
    pub created_at: String,
}

impl crate::contracts::VersionedContract for ChangePackContentRevisionRecord {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::ChangePackContentRevisionRecord;
}

impl ChangePackContentRevisionRecord {
    pub fn recompute_content_revision_digest(&self) -> String {
        let mut revision = self.clone();
        revision.content_revision_digest.clear();
        let value = serde_json::to_value(&revision)
            .expect("ChangePack revisions must be representable as JSON");
        let canonical = crate::support::hashing::canonical_json(&value);
        crate::support::hashing::domain_hash(
            "draft-change-pack-content-revision",
            [canonical.as_bytes()],
        )
    }

    pub fn refresh_content_revision_digest(&mut self) {
        self.content_revision_digest = self.recompute_content_revision_digest();
    }

    pub fn validate(&self, manifest: &ChangePackManifest) -> DraftResult<()> {
        if !crate::contracts::supports_version(
            crate::contracts::ContractId::ChangePackContentRevisionRecord,
            self.schema_version,
        ) {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::UnsupportedSchema,
                format!(
                    "ChangePack revision schema {} is unsupported",
                    self.schema_version
                ),
            ));
        }
        if self.change_pack_id != manifest.change_pack_id
            || self.manifest_digest != manifest.manifest_digest
        {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "ChangePack revision is bound to a different manifest",
            ));
        }
        if self.content_revision_digest != self.recompute_content_revision_digest() {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "ChangePack revision digest mismatch",
            ));
        }
        Ok(())
    }
}

/// The ChangePack lockfile (`change-pack.lock.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChangePackLockfile {
    pub schema_version: u32,
    pub change_pack_id: String,
    /// The authoritative state this ChangePack was locked against.
    pub base_snapshot_digest: String,
    pub result_snapshot_digest: String,
    /// The exact historical observations relied upon, base and result kept
    /// separate.
    ///
    /// A `ChangeSet` has two authoritative snapshots and each was observed by
    /// its own run. One combined reference could not say which observation
    /// established which side, and "the latest record for this state" would let
    /// a later re-observation change what this ChangePack claims to have relied on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_observation: Option<crate::dcg::observation::SnapshotObservationRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_observation: Option<crate::dcg::observation::SnapshotObservationRef>,
    /// The observation semantics both were taken under.
    pub observation_context_digest: String,
    /// The transition itself.
    pub change_set_digest: String,
    /// Each touched resource's authoritative result state, keyed by resource id.
    ///
    /// The adapter's own digest, not a re-read of content: for a resource whose
    /// state is not bytes at all, re-reading would have nothing to hash.
    pub resource_state_digests: BTreeMap<String, String>,
    pub policy_version: String,
    /// The Core semantics behind each derived layer, kept separate so a change
    /// to one cannot be mistaken for a change to another.
    pub change_derivation_revision: u32,
    pub classification_aggregator_revision: u32,
    pub verification_aggregator_revision: u32,
    pub risk_aggregator_revision: u32,
    pub impact_merge_revision: u32,
    pub verification_commands: Vec<LockedCommand>,
    /// The ChangePacks this one was locked on top of, by id.
    ///
    /// Named for what it holds. It carried the Pack-era name long after the
    /// ontology it belonged to was gone, while every value in it has always
    /// been a `cpk_` id — and a field whose name disagrees with its contents
    /// is a reader's mistake waiting to happen.
    pub dependency_change_pack_ids: Vec<String>,
    pub receipt_digests: Vec<String>,
}

impl crate::contracts::VersionedContract for ChangePackLockfile {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::ChangePackLock;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedCommand {
    pub command: String,
    pub command_hash: String,
}

/// Persistence for ChangePack manifests and lockfiles.
pub struct ChangePackContentStore {
    paths: DraftLayout,
}

impl ChangePackContentStore {
    pub fn new(paths: DraftLayout) -> Self {
        ChangePackContentStore { paths }
    }

    pub fn exists(&self, change_pack_id: &str) -> bool {
        self.paths.change_pack_manifest(change_pack_id).exists()
    }

    /// Write an immutable manifest; an existing different one is a conflict.
    pub fn write_manifest(&self, manifest: &ChangePackManifest) -> DraftResult<()> {
        let dir = self.dir_for(&manifest.change_pack_id);
        fsutil::ensure_dir(&dir)?;
        let mut manifest = manifest.clone();
        manifest.refresh_manifest_digest();
        let path = dir.join("manifest.json");
        if path.exists() {
            let existing: ChangePackManifest = crate::contracts::read_persisted(&path)?;
            existing.ensure_supported()?;
            if existing != manifest {
                return Err(DraftError::new(
                    crate::support::error::DraftErrorKind::ConflictDetected,
                    "immutable ChangePack manifest already exists with different content",
                ));
            }
            return Ok(());
        }
        fsutil::write_json(&path, &manifest)
    }

    pub fn read_manifest(&self, change_pack_id: &str) -> DraftResult<ChangePackManifest> {
        let path = self.paths.change_pack_manifest(change_pack_id);
        if !path.exists() {
            return Err(DraftError::not_found(format!(
                "ChangePack {change_pack_id} not found"
            )));
        }
        let manifest: ChangePackManifest = crate::contracts::read_persisted(&path)?;
        manifest.ensure_supported()?;
        Ok(manifest)
    }

    pub fn write_lockfile(&self, lock: &ChangePackLockfile) -> DraftResult<()> {
        fsutil::ensure_dir(&self.paths.change_pack_content_dir(&lock.change_pack_id))?;
        fsutil::write_json(&self.paths.change_pack_lock(&lock.change_pack_id), lock)
    }

    pub fn read_lockfile(&self, change_pack_id: &str) -> DraftResult<ChangePackLockfile> {
        crate::contracts::read_persisted(&self.paths.change_pack_lock(change_pack_id))
    }

    pub fn list(&self) -> DraftResult<Vec<ChangePackManifest>> {
        let mut out = Vec::new();
        let dir = self.paths.change_packs_content_dir();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(&dir)
            .map_err(|e| DraftError::storage(format!("read changes dir: {e}")))?
        {
            let entry = entry.map_err(|e| DraftError::storage(e.to_string()))?;
            if entry.path().is_dir() {
                if let Some(id) = entry.file_name().to_str() {
                    out.push(self.read_manifest(id)?);
                } else {
                    return Err(DraftError::new(
                        crate::support::error::DraftErrorKind::CorruptData,
                        "change directory name is not valid UTF-8",
                    ));
                }
            }
        }
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(out)
    }

    /// Enforce unique ChangePack names within the project.
    pub fn name_taken(&self, name: &str) -> DraftResult<bool> {
        Ok(self.list()?.iter().any(|m| m.name == name))
    }

    /// The on-disk directory holding one ChangePack's content.
    fn dir_for(&self, change_pack_id: &str) -> std::path::PathBuf {
        self.paths.change_pack_content_dir(change_pack_id)
    }

    pub fn write_revision(&self, revision: &ChangePackContentRevisionRecord) -> DraftResult<()> {
        let manifest = self.read_manifest(&revision.change_pack_id)?;
        revision.validate(&manifest)?;
        let path = self
            .dir_for(&revision.change_pack_id)
            .join("content-revisions")
            .join(format!("{}.json", revision.content_revision_id));
        if path.exists() {
            let existing: ChangePackContentRevisionRecord =
                crate::contracts::read_persisted(&path)?;
            if existing != *revision {
                return Err(DraftError::new(
                    crate::support::error::DraftErrorKind::ConflictDetected,
                    "immutable ChangePack revision already exists with different content",
                ));
            }
            return Ok(());
        }
        fsutil::write_json(&path, revision)
    }

    pub fn revisions(
        &self,
        change_pack_id: &str,
    ) -> DraftResult<Vec<ChangePackContentRevisionRecord>> {
        let manifest = self.read_manifest(change_pack_id)?;
        let directory = self.dir_for(change_pack_id).join("content-revisions");
        let mut revisions = Vec::new();
        for path in fsutil::list_with_extension(&directory, "json")? {
            let revision: ChangePackContentRevisionRecord =
                crate::contracts::read_persisted(&path)?;
            revision.validate(&manifest)?;
            revisions.push(revision);
        }
        revisions.sort_by(|left, right| {
            left.content_revision_number
                .cmp(&right.content_revision_number)
                .then_with(|| left.content_revision_id.cmp(&right.content_revision_id))
        });
        Ok(revisions)
    }

    pub fn write_review_progress(
        &self,
        record: &crate::dcg::revision_pack::ReviewProgressRecord,
    ) -> DraftResult<()> {
        let revision = self
            .revisions(&record.change_pack_id)?
            .into_iter()
            .find(|revision| revision.content_revision_id == record.content_revision_id)
            .ok_or_else(|| {
                DraftError::new(
                    crate::support::error::DraftErrorKind::CorruptData,
                    "review progress references a missing content revision",
                )
            })?;
        if revision.content_revision_digest != record.content_revision_digest {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "review progress digest does not match the immutable content revision",
            ));
        }
        fsutil::write_json(
            &self
                .dir_for(&record.change_pack_id)
                .join("review-progress.json"),
            record,
        )
    }

    pub fn read_review_progress(
        &self,
        change_pack_id: &str,
    ) -> DraftResult<crate::dcg::revision_pack::ReviewProgressRecord> {
        let record: crate::dcg::revision_pack::ReviewProgressRecord =
            crate::contracts::read_persisted(
                &self.dir_for(change_pack_id).join("review-progress.json"),
            )?;
        if record.change_pack_id != change_pack_id {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "review progress identity does not match its containing ChangePack",
            ));
        }
        let revision = self
            .revisions(change_pack_id)?
            .into_iter()
            .find(|revision| revision.content_revision_id == record.content_revision_id)
            .ok_or_else(|| {
                DraftError::new(
                    crate::support::error::DraftErrorKind::CorruptData,
                    "review progress references a missing content revision",
                )
            })?;
        if revision.content_revision_digest != record.content_revision_digest {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "review progress content revision digest mismatch",
            ));
        }
        Ok(record)
    }

    pub fn current_content_revision(
        &self,
        change_pack_id: &str,
    ) -> DraftResult<ChangePackContentRevisionRecord> {
        let progress = self.read_review_progress(change_pack_id)?;
        self.revisions(change_pack_id)?
            .into_iter()
            .find(|revision| revision.content_revision_id == progress.content_revision_id)
            .ok_or_else(|| {
                DraftError::new(
                    crate::support::error::DraftErrorKind::CorruptData,
                    "current ChangePack content revision is missing",
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ChangePackManifest {
        ChangePackManifest {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ChangePackManifest,
            ),
            change_pack_id: "cpk_test".into(),
            manifest_digest: String::new(),
            name: "auth".into(),
            description: "desc".into(),
            intent: IntentId::parse("draft.software.project/refactor").unwrap(),
            provenance: serde_json::json!({"origin": "test"}),
            author_id: "act_1".into(),
            candidate_id: None,
            declared_dependencies: Vec::new(),
            created_at: "2026-07-03T00:00:00+00:00".into(),
        }
    }

    fn revision(manifest: &ChangePackManifest) -> ChangePackContentRevisionRecord {
        let mut revision = ChangePackContentRevisionRecord {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ChangePackContentRevisionRecord,
            ),
            change_pack_id: manifest.change_pack_id.clone(),
            manifest_digest: manifest.manifest_digest.clone(),
            content_revision_id: "content_initial".into(),
            content_revision_number: 1,
            content_revision_digest: String::new(),
            base_digest: "sha256:a".into(),
            content_digest: "sha256:b".into(),
            change_set_digest: "sha256:c".into(),
            target_digest: "sha256:d".into(),
            resolved_dependency_digests: Vec::new(),
            created_at: "2026-07-03T00:00:00+00:00".into(),
        };
        revision.refresh_content_revision_digest();
        revision
    }

    #[test]
    fn an_intent_must_be_declared_by_something_installed() {
        let mut declared = std::collections::BTreeSet::new();
        declared.insert("draft.software.project/security".to_string());

        // A contributed intent resolves.
        assert_eq!(
            resolve_intent("draft.software.project/security", &declared)
                .unwrap()
                .qualified(),
            "draft.software.project/security"
        );
        // The core-owned intent always resolves, even against an empty
        // vocabulary: a bare workspace can still declare a ChangePack.
        assert_eq!(
            resolve_intent(UNSPECIFIED_INTENT, &Default::default())
                .unwrap()
                .qualified(),
            UNSPECIFIED_INTENT
        );
        // A well-formed id nothing declares is refused rather than silently
        // never matching a rule, and the error names what is available.
        let error = resolve_intent("draft.software.project/migration", &declared).unwrap_err();
        assert!(
            error.message.contains("draft.software.project/security")
                && error.message.contains(UNSPECIFIED_INTENT),
            "{}",
            error.message
        );
        // Unowned ids are refused: that is exactly how two publishers collide.
        assert!(resolve_intent("security", &declared).is_err());
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
    fn manifest_store_roundtrip_and_unique_names() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ChangePackContentStore::new(DraftLayout::for_root(tmp.path()));
        let m = manifest();
        store.write_manifest(&m).unwrap();
        let mut expected = m.clone();
        expected.refresh_manifest_digest();
        assert!(store.exists("cpk_test"));
        assert_eq!(store.read_manifest("cpk_test").unwrap(), expected);
        assert!(store.name_taken("auth").unwrap());
        assert!(!store.name_taken("other").unwrap());

        let mut changed = m;
        changed.description = "mutated".into();
        assert_eq!(
            store.write_manifest(&changed).unwrap_err().kind,
            crate::support::error::DraftErrorKind::ConflictDetected
        );
    }

    #[test]
    fn a_content_revision_serializes_exactly_the_frozen_field_set() {
        let mut m = manifest();
        m.refresh_manifest_digest();
        let value = serde_json::to_value(revision(&m)).unwrap();
        let fields: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut expected = vec![
            "schema_version",
            "change_pack_id",
            "manifest_digest",
            "content_revision_id",
            "content_revision_number",
            "content_revision_digest",
            "base_digest",
            "content_digest",
            "change_set_digest",
            "target_digest",
            "resolved_dependency_digests",
            "created_at",
        ];
        let mut fields = fields;
        fields.sort_unstable();
        expected.sort_unstable();
        assert_eq!(fields, expected);
    }

    #[test]
    fn the_content_revision_digest_uses_its_own_domain() {
        let mut m = manifest();
        m.refresh_manifest_digest();
        let record = revision(&m);
        let mut unsigned = record.clone();
        unsigned.content_revision_digest.clear();
        let canonical =
            crate::support::hashing::canonical_json(&serde_json::to_value(&unsigned).unwrap());
        assert_eq!(
            record.content_revision_digest,
            crate::support::hashing::domain_hash(
                "draft-change-pack-content-revision",
                [canonical.as_bytes()]
            )
        );
        record.validate(&m).unwrap();
    }

    #[test]
    fn the_retired_content_revision_spellings_fail_closed() {
        let mut m = manifest();
        m.refresh_manifest_digest();
        let current = serde_json::to_value(revision(&m)).unwrap();
        // retired-architecture-ok: the retired spellings are the subject of the test.
        for (new, old) in [
            ("change_pack_id", "change_id"),
            ("content_revision_id", "revision_id"),
            ("content_revision_number", "revision_number"),
            ("content_revision_digest", "revision_digest"),
            ("change_set_digest", "change_digest"),
        ] {
            let mut legacy = current.as_object().unwrap().clone();
            let value = legacy.remove(new).unwrap();
            legacy.insert(old.to_string(), value);
            assert!(
                serde_json::from_value::<ChangePackContentRevisionRecord>(legacy.into()).is_err(),
                "{old} must not parse"
            );
        }
    }
}
