//! Atomic, user-scoped Draft project registry.
//!
//! Project identity is the immutable `workspace_id`; a canonical filesystem
//! path is mutable location metadata. The registry preserves conflicting and
//! missing records for diagnosis instead of silently merging or dropping them.

use crate::project::home::DraftGlobalStore;
use crate::support::common::{now, Timestamp};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::{ensure_dir, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocationHistoryEntry {
    pub canonical_path: String,
    pub location_revision: u64,
    pub replaced_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRegistryEntry {
    pub schema_version: u32,
    pub workspace_id: String,
    pub name: String,
    pub project_path: String,
    pub storage_path: String,
    pub created_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub last_accepted_baseline: Option<String>,
    pub draft_version: String,
    pub health: String,
    pub location_revision: u64,
    pub path_history: Vec<LocationHistoryEntry>,
    pub last_workspace_revision: Option<String>,
}

impl crate::contracts::VersionedContract for ProjectRegistryEntry {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::ProjectRegistryEntry;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryIssue {
    pub workspace_id: String,
    pub kind: String,
    pub path: String,
    pub fixable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRegistryEnvelope {
    pub schema_version: u32,
    pub revision: u64,
    pub updated_at: Timestamp,
    pub entries: Vec<ProjectRegistryEntry>,
}

impl crate::contracts::VersionedContract for ProjectRegistryEnvelope {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::ProjectRegistry;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdoptionReceipt {
    pub schema_version: u32,
    pub adoption_receipt: String,
    pub workspace_id: String,
    pub origin_workspace_id: String,
    pub origin_state_digest: String,
    pub adopted_at: Timestamp,
    pub adopted_by: serde_json::Value,
    pub project_path: String,
    pub backup_path: String,
}

impl crate::contracts::VersionedContract for AdoptionReceipt {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::AdoptionReceipt;
}

impl Default for ProjectRegistryEnvelope {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ProjectRegistry,
            ),
            revision: 0,
            updated_at: now(),
            entries: Vec::new(),
        }
    }
}

pub struct ProjectRegistry {
    path: PathBuf,
}

impl ProjectRegistry {
    pub fn global() -> DraftResult<Self> {
        let store = DraftGlobalStore::locate()?;
        Ok(Self::at(store.registry_dir()))
    }

    pub fn at(directory: impl Into<PathBuf>) -> Self {
        let directory = directory.into();
        Self {
            path: directory.join("projects.json"),
        }
    }

    pub fn envelope(&self) -> DraftResult<ProjectRegistryEnvelope> {
        if self.path.exists() {
            let bytes = fs::read(&self.path)?;
            let envelope = crate::contracts::decode_persisted(&bytes)?;
            return Ok(envelope);
        }
        let retired = self
            .path
            .parent()
            .map(|directory| directory.join("projects.jsonl"));
        if retired.as_ref().is_some_and(|path| path.exists()) {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "a retired registry format exists but canonical registry state is absent",
            )
            .with_suggestion("restore or recreate canonical v1 state explicitly; Draft will not migrate or rewrite the existing bytes"));
        }
        Ok(ProjectRegistryEnvelope::default())
    }

    pub fn list(&self) -> DraftResult<Vec<ProjectRegistryEntry>> {
        let mut entries = self.envelope()?.entries;
        entries.sort_by(|a, b| a.workspace_id.cmp(&b.workspace_id));
        Ok(entries)
    }

    pub fn find(&self, workspace_id: &str) -> DraftResult<Option<ProjectRegistryEntry>> {
        Ok(self
            .list()?
            .into_iter()
            .find(|entry| entry.workspace_id == workspace_id))
    }

    pub fn resolve(&self, id_or_path: &str) -> DraftResult<ProjectRegistryEntry> {
        let canonical = Path::new(id_or_path).canonicalize().ok();
        self.list()?
            .into_iter()
            .find(|entry| {
                entry.workspace_id == id_or_path
                    || canonical
                        .as_ref()
                        .is_some_and(|path| Path::new(&entry.project_path) == path)
            })
            .ok_or_else(|| {
                DraftError::not_found(format!("project '{id_or_path}' is not registered"))
            })
    }

    /// Record a project in the registry.
    ///
    /// `workspace_revision` is supplied rather than derived. Deriving it would
    /// mean the registry — which owns *where a project is* — reaching up into
    /// the graph that describes what is in it, and the registry must stay
    /// readable by anything that can read a path. A caller that has the graph
    /// passes the digest; one that does not passes `None`.
    pub fn upsert(
        &self,
        workspace_id: &str,
        root: &Path,
        stable: Option<String>,
        workspace_revision: Option<String>,
    ) -> DraftResult<ProjectRegistryEntry> {
        let canonical = canonical_project_path(root)?;
        verify_workspace_identity(&canonical, workspace_id)?;
        let canonical_text = canonical.to_string_lossy().into_owned();
        let mut envelope = self.envelope()?;

        if let Some(path_owner) = envelope.entries.iter().find(|entry| {
            entry.project_path == canonical_text && entry.workspace_id != workspace_id
        }) {
            return Err(identity_conflict(format!(
                "path '{}' is already registered to workspace '{}'",
                canonical.display(),
                path_owner.workspace_id
            )));
        }

        if let Some(existing) = envelope
            .entries
            .iter_mut()
            .find(|entry| entry.workspace_id == workspace_id)
        {
            if existing.project_path != canonical_text {
                let old_path = existing.project_path.clone();
                if Path::new(&old_path).exists() {
                    existing.health = "conflicting".into();
                    let message = format!(
                        "workspace '{}' exists at both '{}' and '{}'",
                        workspace_id,
                        old_path,
                        canonical.display()
                    );
                    self.persist(&mut envelope)?;
                    return Err(identity_conflict(message));
                }
                return Err(DraftError::new(
                    DraftErrorKind::ConflictDetected,
                    format!("workspace '{}' moved; use project relocate", workspace_id),
                ));
            }
            existing.last_seen_at = now();
            existing.last_accepted_baseline = stable;
            existing.health = "healthy".into();
            let result = existing.clone();
            self.persist(&mut envelope)?;
            return Ok(result);
        }

        let at = now();
        let entry = ProjectRegistryEntry {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ProjectRegistryEntry,
            ),
            workspace_id: workspace_id.into(),
            name: canonical
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("project")
                .into(),
            project_path: canonical_text,
            storage_path: canonical.join(".draft").to_string_lossy().into_owned(),
            created_at: at,
            last_seen_at: at,
            last_accepted_baseline: stable,
            draft_version: crate::DRAFT_VERSION.into(),
            health: "healthy".into(),
            location_revision: 1,
            path_history: Vec::new(),
            last_workspace_revision: workspace_revision,
        };
        envelope.entries.push(entry.clone());
        self.persist(&mut envelope)?;
        Ok(entry)
    }

    pub fn relocate(
        &self,
        workspace_id: &str,
        destination: &Path,
    ) -> DraftResult<ProjectRegistryEntry> {
        let destination = canonical_project_path(destination)?;
        verify_workspace_identity(&destination, workspace_id)?;
        let destination_text = destination.to_string_lossy().into_owned();
        let mut envelope = self.envelope()?;
        if envelope.entries.iter().any(|entry| {
            entry.project_path == destination_text && entry.workspace_id != workspace_id
        }) {
            return Err(identity_conflict(
                "destination path belongs to another workspace",
            ));
        }
        let entry = envelope
            .entries
            .iter_mut()
            .find(|entry| entry.workspace_id == workspace_id)
            .ok_or_else(|| {
                DraftError::not_found(format!("workspace '{workspace_id}' is not registered"))
            })?;
        if entry.project_path != destination_text {
            entry.path_history.push(LocationHistoryEntry {
                canonical_path: entry.project_path.clone(),
                location_revision: entry.location_revision,
                replaced_at: now(),
            });
            entry.location_revision = entry.location_revision.saturating_add(1);
            entry.project_path = destination_text;
            entry.storage_path = destination.join(".draft").to_string_lossy().into_owned();
        }
        entry.last_seen_at = now();
        entry.health = "healthy".into();
        let result = entry.clone();
        self.persist(&mut envelope)?;
        Ok(result)
    }

    pub fn remove(&self, workspace_id: &str) -> DraftResult<bool> {
        let mut envelope = self.envelope()?;
        let before = envelope.entries.len();
        envelope
            .entries
            .retain(|entry| entry.workspace_id != workspace_id);
        if envelope.entries.len() != before {
            self.persist(&mut envelope)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn inspect(&self) -> DraftResult<Vec<RegistryIssue>> {
        let entries = self.list()?;
        let mut issues = Vec::new();
        let mut paths = BTreeMap::<String, String>::new();
        let mut live_ids = BTreeMap::<String, String>::new();
        for entry in entries {
            let root = Path::new(&entry.project_path);
            if !root.exists() {
                issues.push(issue(&entry, "path_missing", &entry.project_path, true));
                continue;
            }
            if !Path::new(&entry.storage_path).is_dir() {
                issues.push(issue(&entry, "draft_missing", &entry.storage_path, false));
            } else {
                match read_workspace_id(root) {
                    Ok(actual) if actual != entry.workspace_id => issues.push(issue(
                        &entry,
                        "path_reused_by_different_workspace",
                        &entry.project_path,
                        false,
                    )),
                    Err(_) => issues.push(issue(
                        &entry,
                        "workspace_identity_corrupt",
                        &entry.project_path,
                        false,
                    )),
                    Ok(actual) => {
                        if let Some(other) = live_ids.insert(actual, entry.project_path.clone()) {
                            issues.push(issue(
                                &entry,
                                &format!("identity_conflict_with:{other}"),
                                &entry.project_path,
                                false,
                            ));
                        }
                    }
                }
            }
            if let Some(other) =
                paths.insert(entry.project_path.clone(), entry.workspace_id.clone())
            {
                issues.push(issue(
                    &entry,
                    &format!("duplicate_path_with:{other}"),
                    &entry.project_path,
                    false,
                ));
            }
        }
        Ok(issues)
    }

    /// Removes only records whose paths are absent. Identity corruption and
    /// conflicts always require an explicit user decision.
    pub fn fix_stale(&self) -> DraftResult<Vec<RegistryIssue>> {
        let issues = self.inspect()?;
        let stale: std::collections::BTreeSet<_> = issues
            .iter()
            .filter(|issue| issue.fixable && issue.kind == "path_missing")
            .map(|issue| issue.workspace_id.clone())
            .collect();
        let mut envelope = self.envelope()?;
        envelope
            .entries
            .retain(|entry| !stale.contains(&entry.workspace_id));
        self.persist(&mut envelope)?;
        Ok(issues)
    }

    fn persist(&self, envelope: &mut ProjectRegistryEnvelope) -> DraftResult<()> {
        if let Some(parent) = self.path.parent() {
            ensure_dir(parent)?;
        }
        envelope.schema_version =
            crate::contracts::current_version(crate::contracts::ContractId::ProjectRegistry);
        envelope.revision = envelope.revision.saturating_add(1);
        envelope.updated_at = now();
        envelope
            .entries
            .sort_by(|a, b| a.workspace_id.cmp(&b.workspace_id));
        write_atomic(
            &self.path,
            &serde_json::to_vec_pretty(envelope)
                .map_err(|error| DraftError::storage(error.to_string()))?,
        )
    }
}

fn canonical_project_path(root: &Path) -> DraftResult<PathBuf> {
    root.canonicalize().map_err(|error| {
        DraftError::storage(format!("cannot register {}: {error}", root.display()))
    })
}

fn read_workspace_id(root: &Path) -> DraftResult<String> {
    let metadata: crate::project::WorkspaceMetadata =
        crate::contracts::read_persisted(&root.join(".draft/project.json"))?;
    Ok(metadata.workspace_id.to_string())
}

fn verify_workspace_identity(root: &Path, expected: &str) -> DraftResult<()> {
    let actual = read_workspace_id(root)?;
    if actual == expected {
        Ok(())
    } else {
        Err(identity_conflict(format!(
            "workspace identity mismatch: expected '{expected}', found '{actual}'"
        )))
    }
}

fn identity_conflict(message: impl Into<String>) -> DraftError {
    DraftError::new(DraftErrorKind::ConflictDetected, message).with_suggestion(
        "choose the canonical location or explicitly adopt the copy as a new project",
    )
}

fn issue(entry: &ProjectRegistryEntry, kind: &str, path: &str, fixable: bool) -> RegistryIssue {
    RegistryIssue {
        workspace_id: entry.workspace_id.clone(),
        kind: kind.into(),
        path: path.into(),
        fixable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_registration_is_idempotent_and_relocation_is_explicit() {
        let global = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let _scope = crate::project::home::ScopedGlobalHome::set(global.path().join("home"));
        let workspace_id = crate::project::mint_project_id().to_string();
        let layout = crate::project::layout::DraftLayout::for_root(project.path());
        layout.create_all().unwrap();
        crate::support::fsutil::write_json(
            &layout.project_json(),
            &crate::project::WorkspaceMetadata {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::ProjectRegistryEntry,
                ),
                workspace_id: draft_dcg_contract::ids::ProjectId::parse(workspace_id.clone())
                    .unwrap(),
                draft_version: crate::DRAFT_VERSION.into(),
                created_at: now(),
            },
        )
        .unwrap();
        let registry = ProjectRegistry::at(global.path().join("registry"));
        let first = registry
            .upsert(&workspace_id, project.path(), None, None)
            .unwrap();
        let second = registry
            .upsert(&workspace_id, project.path(), None, None)
            .unwrap();
        assert_eq!(first.location_revision, second.location_revision);
        assert_eq!(registry.list().unwrap().len(), 1);
    }

    #[test]
    fn retired_registry_is_rejected_without_modification() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = ProjectRegistry::at(tmp.path());
        let path = tmp.path().join("projects.jsonl");
        let original = b"retired bytes must remain untouched\n";
        fs::write(&path, original).unwrap();
        assert_eq!(
            registry.envelope().unwrap_err().kind,
            DraftErrorKind::CorruptData
        );
        assert_eq!(fs::read(path).unwrap(), original);
        assert!(!tmp.path().join("projects.json").exists());
    }
}
