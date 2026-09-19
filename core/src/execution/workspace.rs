//! Durable, attributed Console edit sessions.
//!
//! Session persistence is presentation-independent: transports may browse and
//! stage edits, but only this core operation may validate and commit them.

use crate::dcg::resource::ResourceLocator;
use crate::dcg::source_view::WorkspaceRevision;
use crate::execution::lease::{LeaseStore, MutationPrecondition};
use crate::execution::operation::{RecoveryStatus, RecoveryStore};
use crate::project::layout::DraftLayout;
use crate::support::common::{now, OperationId, Timestamp, WorkspacePath};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::{ensure_dir, write_atomic, write_json};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Who a change is on behalf of.
///
/// Defined in `support` rather than here, because both the layer that owns
/// resources and the layer that stages edits need it, and `workspace` may not
/// depend on `operation`. Re-exported so its long-standing home keeps working.
pub use crate::support::common::EditAttribution;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StagedEdit {
    /// Replace a resource's content, creating it if it does not exist.
    SetContent {
        locator: ResourceLocator,
        content: String,
    },
    /// Bring a container into existence — a directory for the filesystem
    /// adapter, whatever a collection means for another.
    CreateCollection { locator: ResourceLocator },
    /// Move a resource to another locator. Identity is preserved; the target
    /// locator governs where it lands.
    Relocate {
        from: ResourceLocator,
        to: ResourceLocator,
    },
    Remove {
        locator: ResourceLocator,
        recursive: bool,
    },
}

impl StagedEdit {
    /// The locator this edit is anchored at.
    ///
    /// For a relocation that is the *source*: the edit is about the resource
    /// that is there now, and the destination is where it will go.
    fn locator(&self) -> &ResourceLocator {
        match self {
            Self::SetContent { locator, .. }
            | Self::CreateCollection { locator }
            | Self::Remove { locator, .. } => locator,
            Self::Relocate { from, .. } => from,
        }
    }

    /// The filesystem path this edit acts on, refusing any other scheme.
    ///
    /// The edit-workspace store applies changes to the working tree directly, so
    /// it handles the `file` scheme alone. Another scheme's resources are
    /// mutated through their own adapter under a Draft-authored plan.
    fn workspace_path(&self) -> DraftResult<WorkspacePath> {
        filesystem_body(self.locator())
    }
}

/// The workspace-relative path behind a `file`-scheme locator.
fn filesystem_body(locator: &ResourceLocator) -> DraftResult<WorkspacePath> {
    if !locator.is_file() {
        return Err(DraftError::invalid_config(format!(
            "the '{}' scheme is mutated through its own adapter, not the edit workspace store",
            locator.scheme
        )));
    }
    Ok(WorkspacePath::new(&locator.body))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceState {
    Open,
    Committed,
    Abandoned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceOperationEntry {
    pub operation_id: OperationId,
    pub action: String,
    pub recorded_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub schema_version: u32,
    /// This Workspace's own identity.
    pub id: String,
    /// The project it belongs to.
    pub project: String,
    pub base_revision: WorkspaceRevision,
    pub attribution: EditAttribution,
    pub state: WorkspaceState,
    pub staged_edits: Vec<StagedEdit>,
    pub operation_history: Vec<WorkspaceOperationEntry>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub resulting_revision: Option<WorkspaceRevision>,
}

impl crate::contracts::VersionedContract for Workspace {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::WorkspaceContract;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCommitResult {
    pub schema_version: u32,
    /// This Workspace's own identity.
    pub id: String,
    /// The project it belongs to.
    pub project: String,
    pub attribution: EditAttribution,
    pub operation_id: OperationId,
    pub previous_revision: WorkspaceRevision,
    pub resulting_revision: WorkspaceRevision,
    pub resources_changed: Vec<String>,
    pub lease_id: String,
    pub fencing_token: u64,
}

impl crate::contracts::VersionedContract for WorkspaceCommitResult {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::WorkspaceCommitResultContract;
}

pub struct WorkspaceStore {
    root: PathBuf,
    sessions: PathBuf,
    /// The project's effective protections, resolved once when the store is
    /// opened. Core contributes only its own control plane; the rest arrive
    /// from project config and installed `control_policy` contributions.
    protections: Vec<crate::project::protected::ProtectionRule>,
}

impl WorkspaceStore {
    pub fn for_workspace(
        root: &Path,
        protections: Vec<crate::project::protected::ProtectionRule>,
    ) -> Self {
        let sessions = DraftLayout::for_root(root)
            .workspaces_dir()
            .join("sessions");
        Self {
            root: root.to_path_buf(),
            sessions,
            protections,
        }
    }

    pub fn open(
        &self,
        attribution: EditAttribution,
        operation_id: OperationId,
    ) -> DraftResult<Workspace> {
        let revision = WorkspaceRevision::derive(&self.root)?;
        let at = now();
        let workspace = Workspace {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::WorkspaceContract,
            ),
            id: format!("wsp_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]),
            project: revision.workspace_id.to_string(),
            base_revision: revision,
            attribution,
            state: WorkspaceState::Open,
            staged_edits: Vec::new(),
            operation_history: vec![WorkspaceOperationEntry {
                operation_id,
                action: "workspace.opened".into(),
                recorded_at: at,
            }],
            created_at: at,
            updated_at: at,
            resulting_revision: None,
        };
        self.persist(&workspace)?;
        Ok(workspace)
    }

    pub fn load(&self, id: &str) -> DraftResult<Workspace> {
        validate_workspace_id(id)?;
        crate::contracts::read_persisted(&self.workspace_file(id))
    }

    pub fn stage_content(
        &self,
        id: &str,
        locator: &ResourceLocator,
        content: String,
        operation_id: OperationId,
    ) -> DraftResult<Workspace> {
        let mut workspace = self.load(id)?;
        self.ensure_open_and_current_workspace(&workspace)?;
        let locator = checked_locator(&self.root, &self.protections, locator)?;
        // One staged content edit per resource: a later write supersedes an
        // earlier one rather than queueing behind it.
        workspace
            .staged_edits
            .retain(|edit| edit.locator() != &locator);
        workspace
            .staged_edits
            .push(StagedEdit::SetContent { locator, content });
        workspace.operation_history.push(WorkspaceOperationEntry {
            operation_id,
            action: "content.staged".into(),
            recorded_at: now(),
        });
        workspace.updated_at = now();
        self.persist(&workspace)?;
        Ok(workspace)
    }

    pub fn stage_create_collection(
        &self,
        id: &str,
        locator: &ResourceLocator,
        operation_id: OperationId,
    ) -> DraftResult<Workspace> {
        self.stage_tree_edit(
            id,
            StagedEdit::CreateCollection {
                locator: checked_locator(&self.root, &self.protections, locator)?,
            },
            "collection.staged",
            operation_id,
        )
    }

    pub fn stage_relocate(
        &self,
        id: &str,
        from: &ResourceLocator,
        to: &ResourceLocator,
        operation_id: OperationId,
    ) -> DraftResult<Workspace> {
        let from = checked_locator(&self.root, &self.protections, from)?;
        let to = checked_locator(&self.root, &self.protections, to)?;
        self.stage_tree_edit(
            id,
            StagedEdit::Relocate { from, to },
            "relocate.staged",
            operation_id,
        )
    }

    pub fn stage_remove(
        &self,
        id: &str,
        locator: &ResourceLocator,
        recursive: bool,
        operation_id: OperationId,
    ) -> DraftResult<Workspace> {
        self.stage_tree_edit(
            id,
            StagedEdit::Remove {
                locator: checked_locator(&self.root, &self.protections, locator)?,
                recursive,
            },
            "remove.staged",
            operation_id,
        )
    }

    fn stage_tree_edit(
        &self,
        id: &str,
        edit: StagedEdit,
        action: &str,
        operation_id: OperationId,
    ) -> DraftResult<Workspace> {
        let mut workspace = self.load(id)?;
        self.ensure_open_and_current_workspace(&workspace)?;
        workspace.staged_edits.push(edit);
        workspace.operation_history.push(WorkspaceOperationEntry {
            operation_id,
            action: action.into(),
            recorded_at: now(),
        });
        workspace.updated_at = now();
        self.persist(&workspace)?;
        Ok(workspace)
    }

    pub fn commit(
        &self,
        id: &str,
        operation_id: OperationId,
    ) -> DraftResult<WorkspaceCommitResult> {
        self.commit_with_validation(id, operation_id, |_, _| Ok(()))
    }

    /// Commit with an orchestration-owned validation hook. The hook runs
    /// after deriving the new revision and before finalizing the transaction.
    pub fn commit_with_validation<F>(
        &self,
        id: &str,
        operation_id: OperationId,
        validate: F,
    ) -> DraftResult<WorkspaceCommitResult>
    where
        F: FnOnce(&Workspace, &WorkspaceRevision) -> DraftResult<()>,
    {
        let mut workspace = self.load(id)?;
        self.ensure_open_and_current_workspace(&workspace)?;
        if workspace.staged_edits.is_empty() {
            return Err(DraftError::invalid_config(
                "editor workspace has no staged edits",
            ));
        }

        let leases = LeaseStore::at(DraftLayout::for_root(&self.root).locks_dir());
        let scope = format!("workspace-{}", workspace.project);
        let lease = leases.acquire(&scope, operation_id.clone(), chrono::Duration::minutes(2))?;
        let precondition = MutationPrecondition {
            workspace_id: workspace.project.clone(),
            expected_workspace_revision: workspace.base_revision.clone(),
            operation_id: operation_id.clone(),
            lease_id: lease.lease_id.clone(),
            fencing_token: lease.fencing_token,
            policy_revision: None,
        };

        let outcome = self.commit_with_lease(
            &mut workspace,
            operation_id.clone(),
            &leases,
            &lease,
            &precondition,
            validate,
        );
        // Once canonical source and workspace state are committed, lease cleanup
        // is best-effort; the short expiry remains a safe fallback.
        let _ = leases.release(&lease);
        outcome
    }

    fn commit_with_lease<F>(
        &self,
        workspace: &mut Workspace,
        operation_id: OperationId,
        leases: &LeaseStore,
        lease: &crate::execution::lease::FencedLease,
        precondition: &MutationPrecondition,
        validate: F,
    ) -> DraftResult<WorkspaceCommitResult>
    where
        F: FnOnce(&Workspace, &WorkspaceRevision) -> DraftResult<()>,
    {
        // The complete fenced precondition is revalidated immediately before
        // the first persisted source mutation.
        leases.validate(&self.root, precondition)?;
        let transaction_dir = DraftLayout::for_root(&self.root)
            .workspaces_dir()
            .join("transactions")
            .join(operation_id.as_str());
        ensure_dir(&transaction_dir)?;
        let recovery = RecoveryStore::for_root(&self.root);
        let entry = recovery.start(
            "editor.workspace.commit",
            Some(workspace.id.clone()),
            serde_json::json!({
                "operation_id": operation_id,
                "workspace_id": workspace.project,
                "base_revision": workspace.base_revision,
                "attribution": workspace.attribution,
                "lease_id": lease.lease_id,
                "fencing_token": lease.fencing_token,
            }),
        )?;
        let entry =
            recovery.mark_in_progress(entry, Some(transaction_dir.display().to_string()))?;

        let mut applied = Vec::new();
        let apply_result = (|| -> DraftResult<Vec<String>> {
            let mut changed = Vec::new();
            for (index, edit) in workspace.staged_edits.iter().enumerate() {
                match edit {
                    StagedEdit::SetContent { locator, content } => {
                        let path = edit.workspace_path()?;
                        let destination =
                            checked_path(&self.root, &self.protections, path.as_str())
                                .and_then(|path| safe_destination(&self.root, &path))?;
                        let previous = if destination.exists() {
                            Some(fs::read(&destination).map_err(|error| {
                                DraftError::storage(format!(
                                    "failed to back up {}: {error}",
                                    locator.body
                                ))
                            })?)
                        } else {
                            None
                        };
                        applied.push(AppliedEdit::WriteFile {
                            destination: destination.clone(),
                            previous,
                        });
                        write_atomic(&destination, content.as_bytes())?;
                        changed.push(locator.body.clone());
                    }
                    StagedEdit::CreateCollection { locator } => {
                        let path = edit.workspace_path()?;
                        let destination =
                            checked_path(&self.root, &self.protections, path.as_str())
                                .and_then(|path| safe_destination(&self.root, &path))?;
                        let existed = destination.exists();
                        if existed && !destination.is_dir() {
                            return Err(DraftError::invalid_config(format!(
                                "{} already exists and is not a collection",
                                locator.body
                            )));
                        }
                        fs::create_dir_all(&destination)?;
                        applied.push(AppliedEdit::CreateDirectory {
                            destination,
                            existed,
                        });
                        changed.push(locator.body.clone());
                    }
                    StagedEdit::Relocate { from, to } => {
                        let from_path = filesystem_body(from)?;
                        let to_path = filesystem_body(to)?;
                        let source =
                            safe_existing_path(&self.root, &self.protections, from_path.as_str())?;
                        let destination =
                            checked_path(&self.root, &self.protections, to_path.as_str())
                                .and_then(|path| safe_destination(&self.root, &path))?;
                        if destination.exists() {
                            return Err(DraftError::new(
                                DraftErrorKind::ConflictDetected,
                                format!("{} already exists", to.body),
                            ));
                        }
                        if let Some(parent) = destination.parent() {
                            ensure_dir(parent)?;
                        }
                        fs::rename(&source, &destination)?;
                        applied.push(AppliedEdit::Rename {
                            source,
                            destination,
                        });
                        changed.push(from.body.clone());
                        changed.push(to.body.clone());
                    }
                    StagedEdit::Remove { locator, recursive } => {
                        let path = edit.workspace_path()?;
                        let source =
                            safe_existing_path(&self.root, &self.protections, path.as_str())?;
                        if source.is_dir() && !recursive && fs::read_dir(&source)?.next().is_some()
                        {
                            return Err(DraftError::invalid_config(
                                "removing a non-empty collection requires recursive confirmation",
                            ));
                        }
                        let backup = transaction_dir.join(format!("deleted-{index}"));
                        fs::rename(&source, &backup)?;
                        applied.push(AppliedEdit::Delete { source, backup });
                        changed.push(locator.body.clone());
                    }
                }
            }
            Ok(changed)
        })();

        let resources_changed = match apply_result {
            Ok(changed) => changed,
            Err(error) => {
                rollback_edits(&applied)?;
                let _ = recovery.update(
                    entry,
                    RecoveryStatus::RolledBack,
                    None,
                    Some(error.message.clone()),
                );
                return Err(error);
            }
        };

        let resulting_revision = match WorkspaceRevision::derive(&self.root) {
            Ok(revision) => revision,
            Err(error) => {
                rollback_edits(&applied)?;
                let _ = recovery.update(
                    entry,
                    RecoveryStatus::RolledBack,
                    None,
                    Some(error.message.clone()),
                );
                return Err(error);
            }
        };
        if let Err(error) = validate(workspace, &resulting_revision) {
            rollback_edits(&applied)?;
            let _ = recovery.update(
                entry,
                RecoveryStatus::RolledBack,
                None,
                Some(error.message.clone()),
            );
            return Err(error);
        }
        workspace.state = WorkspaceState::Committed;
        workspace.resulting_revision = Some(resulting_revision.clone());
        workspace.updated_at = now();
        workspace.operation_history.push(WorkspaceOperationEntry {
            operation_id: operation_id.clone(),
            action: "workspace.committed".into(),
            recorded_at: now(),
        });
        if let Err(error) = self.persist(workspace) {
            rollback_edits(&applied)?;
            let _ = recovery.update(
                entry,
                RecoveryStatus::RolledBack,
                None,
                Some(error.message.clone()),
            );
            return Err(error);
        }
        // Recovery records are recovery diagnostics. A failure to mark one
        // complete must not turn an already committed source transaction into
        // a retryable failure.
        let _ = recovery.complete(
            entry,
            serde_json::json!({
                "operation_id": operation_id,
                "resulting_revision": resulting_revision,
                "resources_changed": resources_changed,
            }),
        );
        Ok(WorkspaceCommitResult {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::WorkspaceCommitResultContract,
            ),
            id: workspace.id.clone(),
            project: workspace.project.clone(),
            attribution: workspace.attribution.clone(),
            operation_id,
            previous_revision: workspace.base_revision.clone(),
            resulting_revision,
            resources_changed,
            lease_id: lease.lease_id.clone(),
            fencing_token: lease.fencing_token,
        })
    }

    fn ensure_open_and_current_workspace(&self, workspace: &Workspace) -> DraftResult<()> {
        if workspace.state != WorkspaceState::Open {
            return Err(DraftError::invalid_config("editor workspace is not open"));
        }
        let revision = WorkspaceRevision::derive(&self.root)?;
        if revision.workspace_id.as_str() != workspace.project {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "editor workspace belongs to a different workspace identity",
            ));
        }
        Ok(())
    }

    fn persist(&self, workspace: &Workspace) -> DraftResult<()> {
        validate_workspace_id(&workspace.id)?;
        ensure_dir(&self.sessions)?;
        write_json(&self.workspace_file(&workspace.id), workspace)
    }

    fn workspace_file(&self, id: &str) -> PathBuf {
        self.sessions.join(id).join("workspace.json")
    }
}

fn checked_path(
    root: &Path,
    protections: &[crate::project::protected::ProtectionRule],
    path: &str,
) -> DraftResult<WorkspacePath> {
    let normalized = crate::support::pathguard::check_relative(path).map_err(|error| {
        DraftError::new(
            DraftErrorKind::ProtectedResourceAccess,
            format!("unsafe editor path '{path}': {error}"),
        )
    })?;
    let path = WorkspacePath::new(normalized);
    crate::project::protected::ensure_allowed(protections, &path)?;
    safe_destination(root, &path)?;
    Ok(path)
}

/// A locator checked the same way, refusing any scheme this store cannot apply.
fn checked_locator(
    root: &Path,
    protections: &[crate::project::protected::ProtectionRule],
    locator: &ResourceLocator,
) -> DraftResult<ResourceLocator> {
    let path = filesystem_body(locator)?;
    let checked = checked_path(root, protections, path.as_str())?;
    Ok(ResourceLocator::file(checked.as_str()))
}

fn safe_destination(root: &Path, path: &WorkspacePath) -> DraftResult<PathBuf> {
    crate::support::pathguard::safe_join(root, path.as_str()).map_err(|error| {
        DraftError::new(
            DraftErrorKind::ProtectedResourceAccess,
            format!("unsafe editor destination '{}': {error}", path.as_str()),
        )
    })
}

fn safe_existing_path(
    root: &Path,
    protections: &[crate::project::protected::ProtectionRule],
    path: &str,
) -> DraftResult<PathBuf> {
    let checked = checked_path(root, protections, path)?;
    let destination = safe_destination(root, &checked)?;
    if !destination.exists() {
        return Err(DraftError::not_found(format!(
            "editor path '{path}' was not found"
        )));
    }
    Ok(destination)
}

enum AppliedEdit {
    WriteFile {
        destination: PathBuf,
        previous: Option<Vec<u8>>,
    },
    CreateDirectory {
        destination: PathBuf,
        existed: bool,
    },
    Rename {
        source: PathBuf,
        destination: PathBuf,
    },
    Delete {
        source: PathBuf,
        backup: PathBuf,
    },
}

fn rollback_edits(applied: &[AppliedEdit]) -> DraftResult<()> {
    for edit in applied.iter().rev() {
        match edit {
            AppliedEdit::WriteFile {
                destination,
                previous: Some(bytes),
            } => write_atomic(destination, bytes)?,
            AppliedEdit::WriteFile {
                destination,
                previous: None,
            } if destination.exists() => fs::remove_file(destination).map_err(|error| {
                DraftError::storage(format!(
                    "failed to roll back {}: {error}",
                    destination.display()
                ))
            })?,
            AppliedEdit::CreateDirectory {
                destination,
                existed: false,
            } if destination.exists() => fs::remove_dir(destination).map_err(|error| {
                DraftError::storage(format!(
                    "failed to roll back directory {}: {error}",
                    destination.display()
                ))
            })?,
            AppliedEdit::Rename {
                source,
                destination,
            } if destination.exists() => fs::rename(destination, source)?,
            AppliedEdit::Delete { source, backup } if backup.exists() => {
                if let Some(parent) = source.parent() {
                    ensure_dir(parent)?;
                }
                fs::rename(backup, source)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_workspace_id(id: &str) -> DraftResult<()> {
    if id.starts_with("wsp_")
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        Ok(())
    } else {
        Err(DraftError::invalid_config("invalid workspace id"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{DraftLayout, WorkspaceMetadata};

    fn workspace() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let layout = DraftLayout::for_root(temp.path());
        layout.create_all().unwrap();
        DraftLayout::for_root(temp.path()).create_all().unwrap();
        write_json(
            &layout.project_json(),
            &WorkspaceMetadata {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::WorkspaceMetadata,
                ),
                workspace_id: crate::project::mint_project_id(),
                draft_version: crate::DRAFT_VERSION.into(),
                created_at: now(),
            },
        )
        .unwrap();
        temp
    }

    #[test]
    fn save_persists_a_session_and_commit_is_explicit_and_fenced() {
        let temp = workspace();
        fs::write(temp.path().join("note.txt"), "before\n").unwrap();
        let store =
            WorkspaceStore::for_workspace(temp.path(), crate::project::protected::core_rules());
        let workspace = store
            .open(
                EditAttribution::Task {
                    id: "task-a".into(),
                },
                OperationId::new("op_open"),
            )
            .unwrap();
        store
            .stage_content(
                &workspace.id,
                &ResourceLocator::file("note.txt"),
                "after\n".into(),
                OperationId::new("op_stage"),
            )
            .unwrap();
        assert_eq!(
            fs::read_to_string(temp.path().join("note.txt")).unwrap(),
            "before\n"
        );
        let result = store
            .commit(&workspace.id, OperationId::new("op_commit"))
            .unwrap();
        assert_eq!(
            fs::read_to_string(temp.path().join("note.txt")).unwrap(),
            "after\n"
        );
        assert_ne!(result.previous_revision, result.resulting_revision);
        assert!(result.fencing_token > 0);
    }

    #[test]
    fn attributed_session_commits_create_rename_and_recursive_delete_together() {
        let temp = workspace();
        fs::create_dir_all(temp.path().join("old/tree")).unwrap();
        fs::write(temp.path().join("old/tree/note.txt"), "before\n").unwrap();
        let store =
            WorkspaceStore::for_workspace(temp.path(), crate::project::protected::core_rules());
        let workspace = store
            .open(
                EditAttribution::Task {
                    id: "task-tree".into(),
                },
                OperationId::new("op_tree_open"),
            )
            .unwrap();
        store
            .stage_create_collection(
                &workspace.id,
                &ResourceLocator::file("created"),
                OperationId::new("op_tree_dir"),
            )
            .unwrap();
        store
            .stage_relocate(
                &workspace.id,
                &ResourceLocator::file("old/tree/note.txt"),
                &ResourceLocator::file("created/renamed.txt"),
                OperationId::new("op_tree_rename"),
            )
            .unwrap();
        store
            .stage_remove(
                &workspace.id,
                &ResourceLocator::file("old"),
                true,
                OperationId::new("op_tree_delete"),
            )
            .unwrap();
        let result = store
            .commit(&workspace.id, OperationId::new("op_tree_commit"))
            .unwrap();
        assert_eq!(
            fs::read_to_string(temp.path().join("created/renamed.txt")).unwrap(),
            "before\n"
        );
        assert!(!temp.path().join("old").exists());
        assert_eq!(result.resources_changed.len(), 4);
    }

    #[test]
    fn stale_revision_and_draft_control_plane_fail_closed() {
        let temp = workspace();
        fs::write(temp.path().join("note.txt"), "before\n").unwrap();
        let store =
            WorkspaceStore::for_workspace(temp.path(), crate::project::protected::core_rules());
        let workspace = store
            .open(
                EditAttribution::ChangePack { id: "chg-a".into() },
                OperationId::new("op_open"),
            )
            .unwrap();
        // Draft's own control plane is unreachable through a workspace, whatever
        // is installed. Another tool's control directory is *not* Core's
        // business: excluding `.git` is a view rule the software extension
        // contributes, so with nothing installed it is an ordinary resource.
        assert!(store
            .stage_content(
                &workspace.id,
                &ResourceLocator::file(".draft/config.toml"),
                "bad".into(),
                OperationId::new("op_bad"),
            )
            .is_err());
        store
            .stage_content(
                &workspace.id,
                &ResourceLocator::file("note.txt"),
                "after\n".into(),
                OperationId::new("op_stage"),
            )
            .unwrap();
        fs::write(temp.path().join("other.txt"), "concurrent\n").unwrap();
        let error = store
            .commit(&workspace.id, OperationId::new("op_commit"))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
        assert_eq!(
            fs::read_to_string(temp.path().join("note.txt")).unwrap(),
            "before\n"
        );
    }
}

// ---- ChangePack workspace staging ----

use crate::support::common::{EvidenceId, ExecutionId, SnapshotId, TaskId};
use draft_dcg_contract::ids::ChangePackId;

// Mutable staging state used while deriving immutable ChangePack revisions.
//
// Lifecycle, verification, review, and rollback state are not
// stored here; each is owned by its canonical domain record.

use crate::dcg::resource::ResourceId;

use chrono::{DateTime, Utc};
use draft_dcg_contract::ids::ProjectId;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangePackWorkspace {
    pub schema_version: u32,
    pub id: ChangePackId,
    pub name: Option<String>,
    pub task_id: Option<TaskId>,
    pub execution_id: Option<ExecutionId>,
    pub workspace_id: ProjectId,
    pub base_snapshot_id: SnapshotId,
    pub result_snapshot_id: SnapshotId,
    pub change_set_refs: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub verification_refs: Vec<String>,
    pub review_refs: Vec<String>,
    pub decision_refs: Vec<String>,
    pub receipt_refs: Vec<String>,
    pub source_change_pack_ids: Vec<String>,
    /// The declared purpose of the change, from the contributed intent
    /// vocabulary. `None` when no vocabulary is installed to name one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<draft_extension_contract::NamespacedId>,
    /// The candidate that produced this ChangePack, when an agent did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub manifest_hash: String,
}

/// A fresh ChangePack id for a workspace that is not yet bound to a derived
/// DCG ChangePack. Same family and grammar as every other `cpk_` id.
fn mint_workspace_change_pack_id() -> ChangePackId {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    ChangePackId::parse(format!("{}{}", ChangePackId::PREFIX, &raw[..12]))
        .expect("a cpk_ prefix and twelve hex digits is a valid ChangePack id")
}

impl crate::contracts::VersionedContract for ChangePackWorkspace {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::ChangePackWorkspace;
}

impl ChangePackWorkspace {
    pub(crate) fn new(
        workspace_id: ProjectId,
        task_id: Option<TaskId>,
        execution_id: Option<ExecutionId>,
        base_snapshot_id: SnapshotId,
        result_snapshot_id: SnapshotId,
        name: Option<String>,
    ) -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ChangePackWorkspace,
            ),
            id: mint_workspace_change_pack_id(),
            name,
            task_id,
            execution_id,
            workspace_id,
            base_snapshot_id,
            result_snapshot_id,
            change_set_refs: vec![],
            evidence_refs: vec![],
            verification_refs: vec![],
            review_refs: vec![],
            decision_refs: vec![],
            receipt_refs: vec![],
            source_change_pack_ids: vec![],
            intent: None,
            candidate_id: None,
            created_at: now(),
            updated_at: now(),
            manifest_hash: String::new(),
        }
    }

    pub(crate) fn validate(&self) -> DraftResult<()> {
        let mut canonical = self.clone();
        canonical.manifest_hash.clear();
        if self.manifest_hash != crate::support::hashing::try_canonical_hash(&canonical)? {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("ChangePack workspace {} staging digest mismatch", self.id),
            ));
        }
        Ok(())
    }
}

/// The evidence gathered while producing a ChangePack.
///
/// Nothing here names a tool category. A check result carries a contributed
/// `category` — a software extension may use "test" or "lint", an audio
/// extension "loudness" — and Draft stores it without interpreting it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub schema_version: u32,
    pub id: EvidenceId,
    pub change_pack_id: ChangePackId,
    pub command_logs: Vec<String>,
    pub resources_touched: Vec<ResourceId>,
    /// The derived explanation of the change, when a comparison capability
    /// produced one. Absent is normal: a change set is complete without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation_bundle_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub check_results: Vec<CheckResultSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_summary_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_plan_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_transcript_ref: Option<String>,
    pub warnings: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// One recorded check outcome, in the contributor's own vocabulary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckResultSummary {
    pub check_id: String,
    /// Contributed. Draft groups and displays by it; it never branches on it.
    pub category: String,
    pub detail: String,
    /// What this check established, as one of the five states.
    ///
    /// Recorded rather than derived from `detail`, so a stored summary can be
    /// aggregated by exactly the same lattice a fresh run uses — and so
    /// "unavailable" can never be read back as "passed".
    pub state: draft_extension_contract::VerificationStateName,
    /// Whether this check's outcome can block acceptance.
    ///
    /// Without it an optional check could mask a required gap on reload, which
    /// is precisely the confusion the five states exist to prevent.
    pub requirement: draft_extension_contract::CheckRequirement,
}

impl crate::contracts::VersionedContract for Evidence {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::RevisionPackEvidence;
}
