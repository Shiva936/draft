//! Where a Change's content lives: manifests, revisions, lockfiles, locations.
//!
//! The authoritative Change *record* — its generation, lifecycle and current
//! definition — is [`crate::dcg::change::ChangeStore`]. This is the separate
//! question of where the bytes of each revision sit on disk, and it is the
//! store every read of a manifest, lockfile or quarantine record goes through.

use crate::project::layout::DraftLayout;
use crate::support::error::{DraftError, DraftResult};
use crate::support::fsutil;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The intent a Change declares.
///
/// Core stores intents and compares them; it never interprets one. `feature`,
/// `bug-fix` and `colour-grade` are domain vocabulary, contributed through an
/// `intent_vocabulary` and resolved against the active contributions when a
/// Change is created. The single intent Core owns is
/// [`UNSPECIFIED_INTENT`], so a project with no vocabulary installed can still
/// declare a Change rather than being blocked on an extension.
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
        DraftError::invalid_config(format!("invalid Change intent '{value}': {error}"))
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

/// The canonical Change manifest (`manifest.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChangeManifest {
    pub schema_version: u32,
    pub change_id: String,
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

impl crate::contracts::VersionedContract for ChangeManifest {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::ChangeManifest;
}

impl ChangeManifest {
    pub fn recompute_manifest_digest(&self) -> String {
        crate::support::hashing::domain_hash(
            "draft-change-manifest",
            [crate::support::hashing::canonical_json(&serde_json::json!({
                "schema_version": self.schema_version,
                "change_id": self.change_id,
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
            crate::contracts::ContractId::ChangeManifest,
            self.schema_version,
        ) {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::UnsupportedSchema,
                format!(
                    "Change manifest schema {} is unsupported",
                    self.schema_version
                ),
            ));
        }
        if self.manifest_digest != self.recompute_manifest_digest() {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "Change manifest digest does not match its immutable content",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RevisionRecord {
    pub schema_version: u32,
    pub change_id: String,
    pub manifest_digest: String,
    pub revision_id: String,
    pub revision_number: u64,
    pub revision_digest: String,
    pub base_digest: String,
    pub content_digest: String,
    pub change_digest: String,
    pub target_digest: String,
    pub resolved_dependency_digests: Vec<String>,
    pub created_at: String,
}

impl crate::contracts::VersionedContract for RevisionRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::RevisionRecord;
}

impl RevisionRecord {
    pub fn recompute_revision_digest(&self) -> String {
        let mut revision = self.clone();
        revision.revision_digest.clear();
        let value = serde_json::to_value(&revision)
            .expect("Change revisions must be representable as JSON");
        let canonical = crate::support::hashing::canonical_json(&value);
        crate::support::hashing::domain_hash("draft-change-revision", [canonical.as_bytes()])
    }

    pub fn refresh_revision_digest(&mut self) {
        self.revision_digest = self.recompute_revision_digest();
    }

    pub fn validate(&self, manifest: &ChangeManifest) -> DraftResult<()> {
        if !crate::contracts::supports_version(
            crate::contracts::ContractId::RevisionRecord,
            self.schema_version,
        ) {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::UnsupportedSchema,
                format!(
                    "Change revision schema {} is unsupported",
                    self.schema_version
                ),
            ));
        }
        if self.change_id != manifest.change_id || self.manifest_digest != manifest.manifest_digest
        {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "Change revision is bound to a different manifest",
            ));
        }
        if self.revision_digest != self.recompute_revision_digest() {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "Change revision digest mismatch",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChangeQuarantineRecord {
    pub schema_version: u32,
    pub change_id: String,
    pub revision_id: String,
    pub revision_digest: String,
    pub storage_location: String,
    pub source: String,
    pub artifact_digest: String,
    pub trust_evaluation: QuarantineState,
    pub quarantined_at: String,
    pub promoted_at: Option<String>,
}

impl crate::contracts::VersionedContract for ChangeQuarantineRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::ChangeQuarantine;
}

/// Security/trust evaluation for an imported artifact. This is deliberately
/// separate from the Change's review lifecycle.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineState {
    Quarantined,
    Verified,
    Approved,
    Rejected,
    Promoted,
}

/// The Change lockfile (`change.lock.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChangeLockfile {
    pub schema_version: u32,
    pub change_id: String,
    /// The authoritative state this Change was locked against.
    pub base_snapshot_digest: String,
    pub result_snapshot_digest: String,
    /// The exact historical observations relied upon, base and result kept
    /// separate.
    ///
    /// A `ChangeSet` has two authoritative snapshots and each was observed by
    /// its own run. One combined reference could not say which observation
    /// established which side, and "the latest record for this state" would let
    /// a later re-observation change what this Change claims to have relied on.
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
    /// The Changes this one was locked on top of, by id.
    ///
    /// Named for what it holds. It carried the Pack-era name long after the
    /// ontology it belonged to was gone, while every value in it has always
    /// been a `chg_` id — and a field whose name disagrees with its contents
    /// is a reader's mistake waiting to happen.
    pub dependency_change_ids: Vec<String>,
    pub receipt_digests: Vec<String>,
}

impl crate::contracts::VersionedContract for ChangeLockfile {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::ChangeLock;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedCommand {
    pub command: String,
    pub command_hash: String,
}

/// Where a canonical Change currently lives on disk: the trusted Change store or
/// the import quarantine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeLocation {
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

/// Persistence for Change manifests and lockfiles.
pub struct ChangeContentStore {
    paths: DraftLayout,
}

impl ChangeContentStore {
    pub fn new(paths: DraftLayout) -> Self {
        ChangeContentStore { paths }
    }

    pub fn exists(&self, change_id: &str) -> bool {
        self.paths.change_manifest(change_id).exists()
    }

    pub fn write_manifest(&self, manifest: &ChangeManifest) -> DraftResult<()> {
        self.write_manifest_in(ChangeLocation::Store, manifest)
    }

    pub fn read_manifest(&self, change_id: &str) -> DraftResult<ChangeManifest> {
        let path = self.paths.change_manifest(change_id);
        if !path.exists() {
            return Err(DraftError::not_found(format!(
                "change {change_id} not found"
            )));
        }
        let manifest: ChangeManifest = crate::contracts::read_persisted(&path)?;
        manifest.ensure_supported()?;
        Ok(manifest)
    }

    pub fn write_lockfile(&self, lock: &ChangeLockfile) -> DraftResult<()> {
        fsutil::ensure_dir(&self.paths.change_content_dir(&lock.change_id))?;
        fsutil::write_json(&self.paths.change_lock(&lock.change_id), lock)
    }

    pub fn read_lockfile(&self, change_id: &str) -> DraftResult<ChangeLockfile> {
        crate::contracts::read_persisted(&self.paths.change_lock(change_id))
    }

    pub fn list(&self) -> DraftResult<Vec<ChangeManifest>> {
        let mut out = Vec::new();
        let dir = self.paths.changes_content_dir();
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

    /// Enforce unique Change names within the project.
    pub fn name_taken(&self, name: &str) -> DraftResult<bool> {
        Ok(self.list()?.iter().any(|m| m.name == name))
    }

    /// Find where a Change currently lives: trusted store first, then quarantine.
    pub fn locate(&self, change_id: &str) -> Option<ChangeLocation> {
        if self.paths.change_manifest(change_id).exists() {
            return Some(ChangeLocation::Store);
        }
        if self
            .paths
            .quarantine_dir()
            .join(change_id)
            .join("manifest.json")
            .exists()
        {
            return Some(ChangeLocation::Quarantine);
        }
        None
    }

    /// The on-disk directory for a Change at the given location.
    pub fn dir_for(&self, loc: ChangeLocation, change_id: &str) -> std::path::PathBuf {
        match loc {
            ChangeLocation::Store => self.paths.change_content_dir(change_id),
            ChangeLocation::Quarantine => self.paths.quarantine_dir().join(change_id),
        }
    }

    pub fn read_manifest_in(
        &self,
        loc: ChangeLocation,
        change_id: &str,
    ) -> DraftResult<ChangeManifest> {
        if loc == ChangeLocation::Store {
            return self.read_manifest(change_id);
        }
        let path = self.dir_for(loc, change_id).join("manifest.json");
        if !path.exists() {
            return Err(DraftError::not_found(format!(
                "change {change_id} not found"
            )));
        }
        let manifest: ChangeManifest = crate::contracts::read_persisted(&path)?;
        manifest.ensure_supported()?;
        Ok(manifest)
    }

    pub fn write_manifest_in(
        &self,
        loc: ChangeLocation,
        manifest: &ChangeManifest,
    ) -> DraftResult<()> {
        let dir = self.dir_for(loc, &manifest.change_id);
        fsutil::ensure_dir(&dir)?;
        let mut manifest = manifest.clone();
        manifest.refresh_manifest_digest();
        let path = dir.join("manifest.json");
        if path.exists() {
            let existing: ChangeManifest = crate::contracts::read_persisted(&path)?;
            existing.ensure_supported()?;
            if existing != manifest {
                return Err(DraftError::new(
                    crate::support::error::DraftErrorKind::ConflictDetected,
                    "immutable Change manifest already exists with different content",
                ));
            }
            return Ok(());
        }
        fsutil::write_json(&path, &manifest)
    }

    pub fn write_revision(&self, revision: &RevisionRecord) -> DraftResult<()> {
        self.write_revision_in(ChangeLocation::Store, revision)
    }

    pub fn write_revision_in(
        &self,
        loc: ChangeLocation,
        revision: &RevisionRecord,
    ) -> DraftResult<()> {
        let manifest = self.read_manifest_in(loc, &revision.change_id)?;
        revision.validate(&manifest)?;
        let path = self
            .dir_for(loc, &revision.change_id)
            .join("revisions")
            .join(format!("{}.json", revision.revision_id));
        if path.exists() {
            let existing: RevisionRecord = crate::contracts::read_persisted(&path)?;
            if existing != *revision {
                return Err(DraftError::new(
                    crate::support::error::DraftErrorKind::ConflictDetected,
                    "immutable Change revision already exists with different content",
                ));
            }
            return Ok(());
        }
        fsutil::write_json(&path, revision)
    }

    pub fn revisions(&self, change_id: &str) -> DraftResult<Vec<RevisionRecord>> {
        self.revisions_in(ChangeLocation::Store, change_id)
    }

    pub fn revisions_in(
        &self,
        loc: ChangeLocation,
        change_id: &str,
    ) -> DraftResult<Vec<RevisionRecord>> {
        let manifest = self.read_manifest_in(loc, change_id)?;
        let directory = self.dir_for(loc, change_id).join("revisions");
        let mut revisions = Vec::new();
        for path in fsutil::list_with_extension(&directory, "json")? {
            let revision: RevisionRecord = crate::contracts::read_persisted(&path)?;
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
        loc: ChangeLocation,
        record: &crate::dcg::revision::RevisionStateRecord,
    ) -> DraftResult<()> {
        let revision = self
            .revisions_in(loc, &record.change_id)?
            .into_iter()
            .find(|revision| revision.revision_id == record.revision_id)
            .ok_or_else(|| {
                DraftError::new(
                    crate::support::error::DraftErrorKind::CorruptData,
                    "revision state references a missing revision",
                )
            })?;
        if revision.revision_digest != record.revision_digest {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "Change lifecycle revision digest does not match the immutable revision",
            ));
        }
        fsutil::write_json(
            &self
                .dir_for(loc, &record.change_id)
                .join("revision-state.json"),
            record,
        )
    }

    pub fn read_lifecycle_in(
        &self,
        loc: ChangeLocation,
        change_id: &str,
    ) -> DraftResult<crate::dcg::revision::RevisionStateRecord> {
        let record: crate::dcg::revision::RevisionStateRecord = crate::contracts::read_persisted(
            &self.dir_for(loc, change_id).join("revision-state.json"),
        )?;
        if record.change_id != change_id {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "Change lifecycle identity does not match its containing Change",
            ));
        }
        let revision = self
            .revisions_in(loc, change_id)?
            .into_iter()
            .find(|revision| revision.revision_id == record.revision_id)
            .ok_or_else(|| {
                DraftError::new(
                    crate::support::error::DraftErrorKind::CorruptData,
                    "revision state references a missing revision",
                )
            })?;
        if revision.revision_digest != record.revision_digest {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "Change lifecycle revision digest mismatch",
            ));
        }
        Ok(record)
    }

    pub fn current_revision_in(
        &self,
        loc: ChangeLocation,
        change_id: &str,
    ) -> DraftResult<RevisionRecord> {
        let lifecycle = self.read_lifecycle_in(loc, change_id)?;
        self.revisions_in(loc, change_id)?
            .into_iter()
            .find(|revision| revision.revision_id == lifecycle.revision_id)
            .ok_or_else(|| {
                DraftError::new(
                    crate::support::error::DraftErrorKind::CorruptData,
                    "current Change revision is missing",
                )
            })
    }

    pub fn read_quarantine(&self, change_id: &str) -> DraftResult<ChangeQuarantineRecord> {
        let loc = self
            .locate(change_id)
            .ok_or_else(|| DraftError::not_found(format!("change {change_id} not found")))?;
        let record: ChangeQuarantineRecord = crate::contracts::read_persisted(
            &self.dir_for(loc, change_id).join("quarantine.json"),
        )?;
        let revision = self.current_revision_in(loc, change_id)?;
        if record.change_id != change_id
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

    pub fn write_quarantine(&self, record: &ChangeQuarantineRecord) -> DraftResult<()> {
        let loc = self.locate(&record.change_id).ok_or_else(|| {
            DraftError::not_found(format!("Change {} not found", record.change_id))
        })?;
        let revision = self.current_revision_in(loc, &record.change_id)?;
        if record.revision_id != revision.revision_id
            || record.revision_digest != revision.revision_digest
        {
            return Err(DraftError::new(
                crate::support::error::DraftErrorKind::CorruptData,
                "quarantine record revision binding is invalid",
            ));
        }
        fsutil::write_json(
            &self.dir_for(loc, &record.change_id).join("quarantine.json"),
            record,
        )
    }

    pub fn is_quarantined(&self, change_id: &str) -> bool {
        self.paths
            .quarantine_dir()
            .join(change_id)
            .join("quarantine.json")
            .exists()
    }

    pub fn quarantine_record(
        &self,
        change_id: &str,
    ) -> DraftResult<Option<ChangeQuarantineRecord>> {
        let Some(loc) = self.locate(change_id) else {
            return Ok(None);
        };
        let path = self.dir_for(loc, change_id).join("quarantine.json");
        if !path.exists() {
            return Ok(None);
        }
        self.read_quarantine(change_id).map(Some)
    }

    /// All quarantined imported Changes, ordered by creation time.
    pub fn list_quarantined(&self) -> DraftResult<Vec<ChangeManifest>> {
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
                    out.push(self.read_manifest_in(ChangeLocation::Quarantine, id)?);
                }
            }
        }
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(out)
    }

    /// Move a quarantined Change (its whole evidence directory) into the trusted
    /// Change store. Fails if the destination already exists.
    pub fn promote_from_quarantine(&self, change_id: &str) -> DraftResult<()> {
        let from = self.dir_for(ChangeLocation::Quarantine, change_id);
        let to = self.dir_for(ChangeLocation::Store, change_id);
        if to.exists() {
            return Err(DraftError::storage(format!(
                "Change {change_id} already exists in the Change store"
            )));
        }
        let mut quarantine = self.read_quarantine(change_id)?;
        if quarantine.trust_evaluation != QuarantineState::Approved {
            return Err(DraftError::invalid_config(
                "only an approved quarantined Change may be promoted",
            ));
        }
        quarantine.trust_evaluation = QuarantineState::Promoted;
        quarantine.storage_location = "change_store".into();
        quarantine.promoted_at = Some(crate::support::common::now().to_rfc3339());
        self.write_quarantine(&quarantine)?;
        fsutil::ensure_dir(&self.paths.changes_content_dir())?;
        std::fs::rename(&from, &to)
            .map_err(|e| DraftError::storage(format!("promote {change_id} from quarantine: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ChangeManifest {
        ChangeManifest {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ChangeManifest,
            ),
            change_id: "chg_test".into(),
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

    fn revision(manifest: &ChangeManifest) -> RevisionRecord {
        let mut revision = RevisionRecord {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::RevisionRecord,
            ),
            change_id: manifest.change_id.clone(),
            manifest_digest: manifest.manifest_digest.clone(),
            revision_id: "rev_test".into(),
            revision_number: 1,
            revision_digest: String::new(),
            base_digest: "sha256:a".into(),
            content_digest: "sha256:b".into(),
            change_digest: "sha256:c".into(),
            target_digest: "sha256:d".into(),
            resolved_dependency_digests: Vec::new(),
            created_at: "2026-07-03T00:00:00+00:00".into(),
        };
        revision.refresh_revision_digest();
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
        // vocabulary: a bare workspace can still declare a Change.
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
    fn store_locates_and_promotes_quarantined_change() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ChangeContentStore::new(DraftLayout::for_root(tmp.path()));
        let m = manifest();
        assert_eq!(store.locate("chg_test"), None);

        store
            .write_manifest_in(ChangeLocation::Quarantine, &m)
            .unwrap();
        let mut expected = m.clone();
        expected.refresh_manifest_digest();
        let revision = revision(&expected);
        store
            .write_revision_in(ChangeLocation::Quarantine, &revision)
            .unwrap();
        store
            .write_lifecycle_in(
                ChangeLocation::Quarantine,
                &crate::dcg::revision::RevisionStateRecord {
                    schema_version: crate::contracts::current_version(
                        crate::contracts::ContractId::ChangeManifest,
                    ),
                    change_id: expected.change_id.clone(),
                    revision_id: revision.revision_id.clone(),
                    revision_digest: revision.revision_digest.clone(),
                    lifecycle: crate::dcg::revision::RevisionState::Approved,
                    updated_at: crate::support::common::now(),
                    last_operation_id: crate::support::common::OperationId::new("op_test"),
                },
            )
            .unwrap();
        store
            .write_quarantine(&ChangeQuarantineRecord {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::RevisionRecord,
                ),
                change_id: expected.change_id.clone(),
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
        assert_eq!(store.locate("chg_test"), Some(ChangeLocation::Quarantine));
        assert_eq!(
            store
                .read_manifest_in(ChangeLocation::Quarantine, "chg_test")
                .unwrap(),
            expected
        );
        assert_eq!(store.list_quarantined().unwrap().len(), 1);

        store.promote_from_quarantine("chg_test").unwrap();
        assert_eq!(store.locate("chg_test"), Some(ChangeLocation::Store));
        assert!(store.list_quarantined().unwrap().is_empty());
        assert!(!store
            .dir_for(ChangeLocation::Quarantine, "chg_test")
            .exists());
    }

    #[test]
    fn manifest_store_roundtrip_and_unique_names() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ChangeContentStore::new(DraftLayout::for_root(tmp.path()));
        let m = manifest();
        store.write_manifest(&m).unwrap();
        let mut expected = m.clone();
        expected.refresh_manifest_digest();
        assert!(store.exists("chg_test"));
        assert_eq!(store.read_manifest("chg_test").unwrap(), expected);
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
