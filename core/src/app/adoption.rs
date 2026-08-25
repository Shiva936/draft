//! Explicit copied-workspace adoption orchestration.

use crate::support::common::now;
use crate::support::error::{DraftError, DraftResult};
use crate::support::fsutil::{ensure_dir, write_json};
use crate::workspace::home::DraftGlobalStore;
use crate::workspace::registry::{AdoptionReceipt, ProjectRegistry};
use crate::workspace::WorkspaceMetadata;
use std::fs;
use std::path::{Path, PathBuf};

/// Convert a manually copied Draft project into a distinct workspace after
/// first making and verifying a byte-preserving backup of the copied state.
pub fn adopt_copy(root: &Path) -> DraftResult<AdoptionReceipt> {
    let root = canonical_project_path(root)?;
    let source = root.join(".draft");
    if !source.is_dir() {
        return Err(DraftError::not_found(
            "the project copy has no .draft state",
        ));
    }
    let metadata: WorkspaceMetadata =
        crate::contracts::read_persisted(&source.join("workspace.json"))?;
    let origin_workspace_id = metadata.workspace_id.to_string();
    let origin_state_digest = directory_digest(&source)?;
    let staging = tempfile::tempdir()
        .map_err(|error| DraftError::storage(format!("cannot stage adoption backup: {error}")))?;
    let staged_state = staging.path().join("source-state");
    copy_tree(&source, &staged_state)?;
    if directory_digest(&staged_state)? != origin_state_digest {
        return Err(DraftError::storage("adoption backup verification failed"));
    }

    fs::remove_dir_all(&source)?;
    let init = match super::App::new().init(&root) {
        Ok(init) => init,
        Err(error) => {
            let _ = copy_tree(&staged_state, &source);
            return Err(error);
        }
    };
    let receipt_id = format!("adopt_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
    let receipt_dir = root.join(".draft/backups/adoptions").join(&receipt_id);
    let backup = receipt_dir.join("source-state");
    copy_tree(&staged_state, &backup)?;
    let home = DraftGlobalStore::locate()?;
    let adopted_by = crate::trust::identity::global::ensure_actor(&home)
        .map(|actor| serde_json::json!({ "kind": "actor", "id": actor.actor_id }))
        .unwrap_or_else(|_| serde_json::json!({ "kind": "actor", "id": "act_unknown" }));
    let receipt = AdoptionReceipt {
        schema_version: crate::contracts::current_version(
            crate::contracts::ContractId::AdoptionReceipt,
        ),
        adoption_receipt: receipt_id,
        workspace_id: init.workspace_id.clone(),
        origin_workspace_id,
        origin_state_digest,
        adopted_at: now(),
        adopted_by,
        project_path: root.display().to_string(),
        backup_path: backup.display().to_string(),
    };
    write_json(&receipt_dir.join("adoption-receipt.json"), &receipt)?;
    ProjectRegistry::global()?.upsert(&init.workspace_id, &root, None)?;
    Ok(receipt)
}

fn canonical_project_path(root: &Path) -> DraftResult<PathBuf> {
    root.canonicalize()
        .map_err(|error| DraftError::storage(format!("cannot adopt {}: {error}", root.display())))
}

fn copy_tree(source: &Path, destination: &Path) -> DraftResult<()> {
    ensure_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_symlink() {
            return Err(DraftError::invalid_config(
                "adoption refuses symlinks inside copied Draft metadata",
            ));
        }
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(DraftError::invalid_config(
                "adoption refuses non-regular Draft metadata",
            ));
        }
    }
    Ok(())
}

fn directory_digest(root: &Path) -> DraftResult<String> {
    let mut entries = Vec::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| DraftError::storage(error.to_string()))?;
        if entry.file_type().is_dir() {
            continue;
        }
        if !entry.file_type().is_file() {
            return Err(DraftError::invalid_config(
                "Draft metadata contains a non-regular entry",
            ));
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|error| DraftError::storage(error.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        entries.push((
            relative,
            crate::support::hashing::sha256_hex(&fs::read(entry.path())?),
        ));
    }
    entries.sort();
    Ok(crate::support::hashing::canonical_hash(&entries))
}
