//! Provider-neutral checkpoint engine (Phase 7).
//!
//! Core owns checkpoint *metadata* and delegates the provider-specific snapshot
//! mechanics to the provider via the `VcsRepository` trait.

use serde::{Deserialize, Serialize};

use crate::common::{now, CheckpointId, OperationId, Timestamp, WorkspaceId};
use crate::error::DraftResult;
use crate::fsutil::{list_with_extension, read_json, write_json};
use crate::identity::ActorRef;
use crate::operations::{NewOperation, ObjectKind, ObjectRef, OperationKind, OperationLog};
use crate::vcs::traits::VcsRepository;
use crate::vcs::types::{
    CheckpointInput, ProviderCheckpoint, ProviderCheckpointRef, ProviderId, ProviderRestoreResult,
};
use crate::workspace::layout::DraftLayout;
use crate::workspace::Workspace;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftCheckpoint {
    pub id: CheckpointId,
    pub workspace_id: WorkspaceId,
    pub provider_id: ProviderId,
    pub provider_checkpoint: Option<ProviderCheckpoint>,
    pub operation_id: Option<OperationId>,
    pub created_at: Timestamp,
    pub description: Option<String>,
}

fn path(layout: &DraftLayout, id: &CheckpointId) -> std::path::PathBuf {
    layout
        .checkpoints_dir()
        .join(format!("checkpoint_{id}.json"))
}

/// Create a checkpoint: delegate to the provider, persist metadata, and append
/// a `CheckpointCreated` operation.
pub fn create(
    ws: &Workspace,
    repo: &dyn VcsRepository,
    log: &OperationLog,
    actor: ActorRef,
    description: Option<String>,
) -> DraftResult<DraftCheckpoint> {
    let provider_cp = repo.create_checkpoint(CheckpointInput {
        description: description.clone(),
    })?;
    let id = CheckpointId::generate();
    let op = log.append(
        NewOperation::new(
            OperationKind::CheckpointCreated,
            actor,
            ws.provider_id.clone(),
        )
        .output(ObjectRef::new(ObjectKind::Checkpoint, id.to_string()))
        .message(description.clone().unwrap_or_else(|| "checkpoint".into())),
    )?;
    let cp = DraftCheckpoint {
        id: id.clone(),
        workspace_id: ws.id.clone(),
        provider_id: ws.provider_id.clone(),
        provider_checkpoint: Some(provider_cp),
        operation_id: Some(op.id),
        created_at: now(),
        description,
    };
    write_json(&path(&ws.layout(), &id), &cp)?;
    Ok(cp)
}

/// Restore a checkpoint, delegating safety to the provider, then append
/// `CheckpointRestored`.
pub fn restore(
    ws: &Workspace,
    repo: &dyn VcsRepository,
    log: &OperationLog,
    actor: ActorRef,
    id: &CheckpointId,
) -> DraftResult<ProviderRestoreResult> {
    let cp = load(&ws.layout(), id)?;
    let provider_cp = cp.provider_checkpoint.ok_or_else(|| {
        crate::error::DraftError::not_found("checkpoint has no provider snapshot")
    })?;
    let result = repo.restore_checkpoint(ProviderCheckpointRef::from(&provider_cp))?;
    log.append(
        NewOperation::new(
            OperationKind::CheckpointRestored,
            actor,
            ws.provider_id.clone(),
        )
        .input(ObjectRef::new(ObjectKind::Checkpoint, id.to_string()))
        .message(result.message.clone()),
    )?;
    Ok(result)
}

pub fn load(layout: &DraftLayout, id: &CheckpointId) -> DraftResult<DraftCheckpoint> {
    read_json(&path(layout, id))
}

pub fn latest(layout: &DraftLayout) -> DraftResult<Option<DraftCheckpoint>> {
    let mut latest: Option<DraftCheckpoint> = None;
    for p in list_with_extension(&layout.checkpoints_dir(), "json")? {
        if let Ok(c) = read_json::<DraftCheckpoint>(&p) {
            match &latest {
                None => latest = Some(c),
                Some(cur) if c.created_at > cur.created_at => latest = Some(c),
                _ => {}
            }
        }
    }
    Ok(latest)
}
