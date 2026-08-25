//! Durable, attributed Console edit sessions.
//!
//! Session persistence is presentation-independent: transports may browse and
//! stage edits, but only this core operation may validate and commit them.

use crate::operation::{LeaseStore, MutationPrecondition};
use crate::operation::{RecoveryStatus, RecoveryStore};
use crate::support::common::{now, OperationId, Timestamp, WorkspacePath};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::{ensure_dir, write_atomic, write_json};
use crate::workspace::layout::DraftLayout;
use crate::workspace::source_view::{CanonicalSourcePolicy, WorkspaceRevision};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditAttribution {
    Task { id: String },
    Pack { id: String },
    CandidateExecution { id: String },
    Review { id: String },
}

impl EditAttribution {
    pub fn id(&self) -> &str {
        match self {
            Self::Task { id }
            | Self::Pack { id }
            | Self::CandidateExecution { id }
            | Self::Review { id } => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StagedEdit {
    WriteFile {
        path: WorkspacePath,
        content: String,
    },
    CreateDirectory {
        path: WorkspacePath,
    },
    Rename {
        from: WorkspacePath,
        to: WorkspacePath,
    },
    Delete {
        path: WorkspacePath,
        recursive: bool,
    },
}

impl StagedEdit {
    fn path(&self) -> &WorkspacePath {
        match self {
            Self::WriteFile { path, .. }
            | Self::CreateDirectory { path }
            | Self::Delete { path, .. } => path,
            Self::Rename { from, .. } => from,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditSessionState {
    Open,
    Committed,
    Abandoned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditSessionOperationEntry {
    pub operation_id: OperationId,
    pub action: String,
    pub recorded_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditSession {
    pub schema_version: u32,
    pub session_id: String,
    pub workspace_id: String,
    pub base_revision: WorkspaceRevision,
    pub attribution: EditAttribution,
    pub state: EditSessionState,
    pub staged_edits: Vec<StagedEdit>,
    pub operation_history: Vec<EditSessionOperationEntry>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub resulting_revision: Option<WorkspaceRevision>,
}

impl crate::contracts::VersionedContract for EditSession {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::EditorSession;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditCommitResult {
    pub schema_version: u32,
    pub session_id: String,
    pub workspace_id: String,
    pub attribution: EditAttribution,
    pub operation_id: OperationId,
    pub previous_revision: WorkspaceRevision,
    pub resulting_revision: WorkspaceRevision,
    pub files_changed: Vec<String>,
    pub lease_id: String,
    pub fencing_token: u64,
}

impl crate::contracts::VersionedContract for EditCommitResult {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::EditorCommitResult;
}

pub struct EditSessionStore {
    root: PathBuf,
    sessions: PathBuf,
}

impl EditSessionStore {
    pub fn for_workspace(root: &Path) -> Self {
        let sessions = DraftLayout::for_root(root).editor_dir().join("sessions");
        Self {
            root: root.to_path_buf(),
            sessions,
        }
    }

    pub fn open(
        &self,
        attribution: EditAttribution,
        operation_id: OperationId,
    ) -> DraftResult<EditSession> {
        let revision = WorkspaceRevision::derive(&self.root)?;
        let at = now();
        let session = EditSession {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::EditorSession,
            ),
            session_id: format!("edit_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]),
            workspace_id: revision.workspace_id.to_string(),
            base_revision: revision,
            attribution,
            state: EditSessionState::Open,
            staged_edits: Vec::new(),
            operation_history: vec![EditSessionOperationEntry {
                operation_id,
                action: "session.opened".into(),
                recorded_at: at,
            }],
            created_at: at,
            updated_at: at,
            resulting_revision: None,
        };
        self.persist(&session)?;
        Ok(session)
    }

    pub fn load(&self, session_id: &str) -> DraftResult<EditSession> {
        validate_session_id(session_id)?;
        crate::contracts::read_persisted(&self.session_file(session_id))
    }

    pub fn stage_write(
        &self,
        session_id: &str,
        path: &str,
        content: String,
        operation_id: OperationId,
    ) -> DraftResult<EditSession> {
        let mut session = self.load(session_id)?;
        self.ensure_open_and_current_workspace(&session)?;
        let path = checked_path(&self.root, path)?;
        session.staged_edits.retain(|edit| edit.path() != &path);
        session
            .staged_edits
            .push(StagedEdit::WriteFile { path, content });
        session.operation_history.push(EditSessionOperationEntry {
            operation_id,
            action: "file.staged".into(),
            recorded_at: now(),
        });
        session.updated_at = now();
        self.persist(&session)?;
        Ok(session)
    }

    pub fn stage_create_directory(
        &self,
        session_id: &str,
        path: &str,
        operation_id: OperationId,
    ) -> DraftResult<EditSession> {
        self.stage_tree_edit(
            session_id,
            StagedEdit::CreateDirectory {
                path: checked_path(&self.root, path)?,
            },
            "directory.staged",
            operation_id,
        )
    }

    pub fn stage_rename(
        &self,
        session_id: &str,
        from: &str,
        to: &str,
        operation_id: OperationId,
    ) -> DraftResult<EditSession> {
        let from = checked_path(&self.root, from)?;
        let to = checked_path(&self.root, to)?;
        self.stage_tree_edit(
            session_id,
            StagedEdit::Rename { from, to },
            "rename.staged",
            operation_id,
        )
    }

    pub fn stage_delete(
        &self,
        session_id: &str,
        path: &str,
        recursive: bool,
        operation_id: OperationId,
    ) -> DraftResult<EditSession> {
        self.stage_tree_edit(
            session_id,
            StagedEdit::Delete {
                path: checked_path(&self.root, path)?,
                recursive,
            },
            "delete.staged",
            operation_id,
        )
    }

    fn stage_tree_edit(
        &self,
        session_id: &str,
        edit: StagedEdit,
        action: &str,
        operation_id: OperationId,
    ) -> DraftResult<EditSession> {
        let mut session = self.load(session_id)?;
        self.ensure_open_and_current_workspace(&session)?;
        session.staged_edits.push(edit);
        session.operation_history.push(EditSessionOperationEntry {
            operation_id,
            action: action.into(),
            recorded_at: now(),
        });
        session.updated_at = now();
        self.persist(&session)?;
        Ok(session)
    }

    pub fn commit(
        &self,
        session_id: &str,
        operation_id: OperationId,
    ) -> DraftResult<EditCommitResult> {
        self.commit_with_validation(session_id, operation_id, |_, _| Ok(()))
    }

    /// Commit with an orchestration-owned validation hook. The hook runs
    /// after deriving the new revision and before finalizing the transaction.
    pub fn commit_with_validation<F>(
        &self,
        session_id: &str,
        operation_id: OperationId,
        validate: F,
    ) -> DraftResult<EditCommitResult>
    where
        F: FnOnce(&EditSession, &WorkspaceRevision) -> DraftResult<()>,
    {
        let mut session = self.load(session_id)?;
        self.ensure_open_and_current_workspace(&session)?;
        if session.staged_edits.is_empty() {
            return Err(DraftError::invalid_config(
                "editor session has no staged edits",
            ));
        }

        let leases = LeaseStore::at(DraftLayout::for_root(&self.root).locks_dir());
        let scope = format!("workspace-{}", session.workspace_id);
        let lease = leases.acquire(&scope, operation_id.clone(), chrono::Duration::minutes(2))?;
        let precondition = MutationPrecondition {
            workspace_id: session.workspace_id.clone(),
            expected_workspace_revision: session.base_revision.clone(),
            operation_id: operation_id.clone(),
            lease_id: lease.lease_id.clone(),
            fencing_token: lease.fencing_token,
            policy_revision: None,
        };

        let outcome = self.commit_with_lease(
            &mut session,
            operation_id.clone(),
            &leases,
            &lease,
            &precondition,
            validate,
        );
        // Once canonical source and session state are committed, lease cleanup
        // is best-effort; the short expiry remains a safe fallback.
        let _ = leases.release(&lease);
        outcome
    }

    fn commit_with_lease<F>(
        &self,
        session: &mut EditSession,
        operation_id: OperationId,
        leases: &LeaseStore,
        lease: &crate::operation::FencedLease,
        precondition: &MutationPrecondition,
        validate: F,
    ) -> DraftResult<EditCommitResult>
    where
        F: FnOnce(&EditSession, &WorkspaceRevision) -> DraftResult<()>,
    {
        // The complete fenced precondition is revalidated immediately before
        // the first persisted source mutation.
        leases.validate(&self.root, precondition)?;
        let transaction_dir = DraftLayout::for_root(&self.root)
            .editor_dir()
            .join("transactions")
            .join(operation_id.as_str());
        ensure_dir(&transaction_dir)?;
        let recovery = RecoveryStore::for_root(&self.root);
        let entry = recovery.start(
            "editor.session.commit",
            Some(session.session_id.clone()),
            serde_json::json!({
                "operation_id": operation_id,
                "workspace_id": session.workspace_id,
                "base_revision": session.base_revision,
                "attribution": session.attribution,
                "lease_id": lease.lease_id,
                "fencing_token": lease.fencing_token,
            }),
        )?;
        let entry =
            recovery.mark_in_progress(entry, Some(transaction_dir.display().to_string()))?;

        let mut applied = Vec::new();
        let apply_result = (|| -> DraftResult<Vec<String>> {
            let mut changed = Vec::new();
            for (index, edit) in session.staged_edits.iter().enumerate() {
                match edit {
                    StagedEdit::WriteFile { path, content } => {
                        let destination = checked_path(&self.root, path.as_str())
                            .and_then(|path| safe_destination(&self.root, &path))?;
                        let previous = if destination.exists() {
                            Some(fs::read(&destination).map_err(|error| {
                                DraftError::storage(format!(
                                    "failed to back up {}: {error}",
                                    path.as_str()
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
                        changed.push(path.to_string());
                    }
                    StagedEdit::CreateDirectory { path } => {
                        let destination = checked_path(&self.root, path.as_str())
                            .and_then(|path| safe_destination(&self.root, &path))?;
                        let existed = destination.exists();
                        if existed && !destination.is_dir() {
                            return Err(DraftError::invalid_config(format!(
                                "{} already exists and is not a directory",
                                path.as_str()
                            )));
                        }
                        fs::create_dir_all(&destination)?;
                        applied.push(AppliedEdit::CreateDirectory {
                            destination,
                            existed,
                        });
                        changed.push(path.to_string());
                    }
                    StagedEdit::Rename { from, to } => {
                        let source = safe_existing_path(&self.root, from.as_str())?;
                        let destination = checked_path(&self.root, to.as_str())
                            .and_then(|path| safe_destination(&self.root, &path))?;
                        if destination.exists() {
                            return Err(DraftError::new(
                                DraftErrorKind::ConflictDetected,
                                format!("{} already exists", to.as_str()),
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
                        changed.push(from.to_string());
                        changed.push(to.to_string());
                    }
                    StagedEdit::Delete { path, recursive } => {
                        let source = safe_existing_path(&self.root, path.as_str())?;
                        if source.is_dir() && !recursive && fs::read_dir(&source)?.next().is_some()
                        {
                            return Err(DraftError::invalid_config(
                                "non-empty directory deletion requires recursive confirmation",
                            ));
                        }
                        let backup = transaction_dir.join(format!("deleted-{index}"));
                        fs::rename(&source, &backup)?;
                        applied.push(AppliedEdit::Delete { source, backup });
                        changed.push(path.to_string());
                    }
                }
            }
            Ok(changed)
        })();

        let files_changed = match apply_result {
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
        if let Err(error) = validate(session, &resulting_revision) {
            rollback_edits(&applied)?;
            let _ = recovery.update(
                entry,
                RecoveryStatus::RolledBack,
                None,
                Some(error.message.clone()),
            );
            return Err(error);
        }
        session.state = EditSessionState::Committed;
        session.resulting_revision = Some(resulting_revision.clone());
        session.updated_at = now();
        session.operation_history.push(EditSessionOperationEntry {
            operation_id: operation_id.clone(),
            action: "session.committed".into(),
            recorded_at: now(),
        });
        if let Err(error) = self.persist(session) {
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
                "files_changed": files_changed,
            }),
        );
        Ok(EditCommitResult {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::EditorCommitResult,
            ),
            session_id: session.session_id.clone(),
            workspace_id: session.workspace_id.clone(),
            attribution: session.attribution.clone(),
            operation_id,
            previous_revision: session.base_revision.clone(),
            resulting_revision,
            files_changed,
            lease_id: lease.lease_id.clone(),
            fencing_token: lease.fencing_token,
        })
    }

    fn ensure_open_and_current_workspace(&self, session: &EditSession) -> DraftResult<()> {
        if session.state != EditSessionState::Open {
            return Err(DraftError::invalid_config("editor session is not open"));
        }
        let revision = WorkspaceRevision::derive(&self.root)?;
        if revision.workspace_id.as_str() != session.workspace_id {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "editor session belongs to a different workspace identity",
            ));
        }
        Ok(())
    }

    fn persist(&self, session: &EditSession) -> DraftResult<()> {
        validate_session_id(&session.session_id)?;
        ensure_dir(&self.sessions)?;
        write_json(&self.session_file(&session.session_id), session)
    }

    fn session_file(&self, session_id: &str) -> PathBuf {
        self.sessions.join(session_id).join("session.json")
    }
}

fn checked_path(root: &Path, path: &str) -> DraftResult<WorkspacePath> {
    let normalized = crate::support::pathguard::check_relative(path).map_err(|error| {
        DraftError::new(
            DraftErrorKind::ProtectedFileAccess,
            format!("unsafe editor path '{path}': {error}"),
        )
    })?;
    let first = normalized.split('/').next().unwrap_or_default();
    if CanonicalSourcePolicy::default()
        .excluded_control_directories
        .iter()
        .any(|directory| first.eq_ignore_ascii_case(directory))
    {
        return Err(DraftError::new(
            DraftErrorKind::ProtectedFileAccess,
            format!("provider control directory is protected: {first}"),
        ));
    }
    let path = WorkspacePath::new(normalized);
    crate::workspace::protected::ensure_allowed(root, &path)?;
    safe_destination(root, &path)?;
    Ok(path)
}

fn safe_destination(root: &Path, path: &WorkspacePath) -> DraftResult<PathBuf> {
    crate::support::pathguard::safe_join(root, path.as_str()).map_err(|error| {
        DraftError::new(
            DraftErrorKind::ProtectedFileAccess,
            format!("unsafe editor destination '{}': {error}", path.as_str()),
        )
    })
}

fn safe_existing_path(root: &Path, path: &str) -> DraftResult<PathBuf> {
    let checked = checked_path(root, path)?;
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

fn validate_session_id(session_id: &str) -> DraftResult<()> {
    if session_id.starts_with("edit_")
        && session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        Ok(())
    } else {
        Err(DraftError::invalid_config("invalid editor session id"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::common::WorkspaceId;
    use crate::workspace::{DraftLayout, WorkspaceMetadata};

    fn workspace() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let layout = DraftLayout::for_root(temp.path());
        layout.create_all().unwrap();
        DraftLayout::for_root(temp.path()).create_all().unwrap();
        write_json(
            &layout.workspace_json(),
            &WorkspaceMetadata {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::WorkspaceMetadata,
                ),
                workspace_id: WorkspaceId::generate(),
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
        let store = EditSessionStore::for_workspace(temp.path());
        let session = store
            .open(
                EditAttribution::Task {
                    id: "task-a".into(),
                },
                OperationId::new("op_open"),
            )
            .unwrap();
        store
            .stage_write(
                &session.session_id,
                "note.txt",
                "after\n".into(),
                OperationId::new("op_stage"),
            )
            .unwrap();
        assert_eq!(
            fs::read_to_string(temp.path().join("note.txt")).unwrap(),
            "before\n"
        );
        let result = store
            .commit(&session.session_id, OperationId::new("op_commit"))
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
        let store = EditSessionStore::for_workspace(temp.path());
        let session = store
            .open(
                EditAttribution::Task {
                    id: "task-tree".into(),
                },
                OperationId::new("op_tree_open"),
            )
            .unwrap();
        store
            .stage_create_directory(
                &session.session_id,
                "created",
                OperationId::new("op_tree_dir"),
            )
            .unwrap();
        store
            .stage_rename(
                &session.session_id,
                "old/tree/note.txt",
                "created/renamed.txt",
                OperationId::new("op_tree_rename"),
            )
            .unwrap();
        store
            .stage_delete(
                &session.session_id,
                "old",
                true,
                OperationId::new("op_tree_delete"),
            )
            .unwrap();
        let result = store
            .commit(&session.session_id, OperationId::new("op_tree_commit"))
            .unwrap();
        assert_eq!(
            fs::read_to_string(temp.path().join("created/renamed.txt")).unwrap(),
            "before\n"
        );
        assert!(!temp.path().join("old").exists());
        assert_eq!(result.files_changed.len(), 4);
    }

    #[test]
    fn stale_revision_and_protected_control_paths_fail_closed() {
        let temp = workspace();
        fs::write(temp.path().join("note.txt"), "before\n").unwrap();
        let store = EditSessionStore::for_workspace(temp.path());
        let session = store
            .open(
                EditAttribution::Pack { id: "pck-a".into() },
                OperationId::new("op_open"),
            )
            .unwrap();
        assert!(store
            .stage_write(
                &session.session_id,
                ".git/config",
                "bad".into(),
                OperationId::new("op_bad"),
            )
            .is_err());
        store
            .stage_write(
                &session.session_id,
                "note.txt",
                "after\n".into(),
                OperationId::new("op_stage"),
            )
            .unwrap();
        fs::write(temp.path().join("other.txt"), "concurrent\n").unwrap();
        let error = store
            .commit(&session.session_id, OperationId::new("op_commit"))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
        assert_eq!(
            fs::read_to_string(temp.path().join("note.txt")).unwrap(),
            "before\n"
        );
    }
}
