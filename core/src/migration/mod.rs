//! Migration from v0.1.0 (Git-coupled) `.draft/` metadata to v0.2.0
//! provider-neutral metadata (Phase 17, FR-WS-004).
//!
//! v0.1.0 layout markers: `.draft/config.toml` (with `repo_id`), `.draft/repo.toml`,
//! and `.draft/receipts/<commit_hash>.json`. The absence of `workspace.json`
//! distinguishes a v0.1.0 workspace from a v0.2.0 one.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::common::{now, WorkspaceId};
use crate::error::DraftResult;
use crate::fsutil::{list_with_extension, read_json, write_json};
use crate::identity::ActorRef;
use crate::operations::{NewOperation, OperationKind, OperationLog};
use crate::receipts::{self, DraftReceipt};
use crate::vcs::types::{ProviderId, ProviderObjectId, ProviderObjectRef};
use crate::workspace::config::WorkspaceConfig;
use crate::workspace::layout::DraftLayout;
use crate::workspace::metadata::WorkspaceMetadata;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationReport {
    pub workspace_id: String,
    pub provider_id: String,
    pub receipts_migrated: usize,
}

/// True if `root` holds a v0.1.0 Draft workspace that needs migration.
pub fn needs_migration(root: &Path) -> bool {
    let layout = DraftLayout::for_root(root);
    layout.exists() && !layout.workspace_json().exists() && layout.config_toml().exists()
}

/// The subset of the v0.1.0 `CommitReceipt` we can carry forward.
#[derive(Debug, Clone, Deserialize)]
struct OldReceipt {
    commit_hash: String,
    #[serde(default)]
    commit_message: String,
}

/// Run migration for `root`, binding the workspace to Git (v0.1.0 was Git-only).
pub fn migrate(root: &Path) -> DraftResult<MigrationReport> {
    let layout = DraftLayout::for_root(root);
    layout.create_all()?;
    let provider_id = ProviderId::new("git");

    // Back up old top-level metadata files.
    for name in ["config.toml", "repo.toml"] {
        let src = layout.draft_dir.join(name);
        if src.exists() {
            let dst = layout.backup_dir().join(name);
            let _ = std::fs::copy(&src, &dst);
        }
    }

    // Write fresh provider-neutral config + metadata.
    let id = WorkspaceId::generate();
    let config = WorkspaceConfig::new(provider_id.clone());
    write_json(
        &layout.workspace_json(),
        &WorkspaceMetadata {
            id: id.clone(),
            draft_version: crate::DRAFT_VERSION.to_string(),
            provider_id: provider_id.clone(),
            provider_root_rel: ".".to_string(),
            created_at: now(),
            migrated_from: Some("0.1.0".to_string()),
        },
    )?;
    crate::fsutil::write_toml(&layout.config_toml(), &config)?;

    // Convert old receipts (best-effort) into provider-neutral receipts.
    let mut migrated = 0;
    for path in list_with_extension(&layout.receipts_dir(), "json")? {
        // Skip already-converted receipts and the index.
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with("receipt_") || name == "index.json" {
            continue;
        }
        if let Ok(old) = read_json::<OldReceipt>(&path) {
            let mut receipt =
                DraftReceipt::builder(id.clone(), provider_id.clone(), ActorRef::unknown());
            receipt.provider_objects = vec![ProviderObjectRef {
                provider_id: provider_id.clone(),
                object_id: ProviderObjectId::new(old.commit_hash.clone()),
                kind: "commit".to_string(),
                label: Some(old.commit_hash.chars().take(8).collect()),
            }];
            receipt.finalization_summary = Some(crate::finalization::FinalizationSummary {
                object: receipt.provider_objects.first().cloned(),
                change_count: 0,
                message_title: old.commit_message.lines().next().unwrap_or("").to_string(),
            });
            receipts::create(&layout, &receipt)?;
            // Move the old receipt into backup to avoid double counting.
            let _ = std::fs::rename(&path, layout.backup_dir().join(name));
            migrated += 1;
        }
    }

    // Record the migration in the operation log.
    let log = OperationLog::new(layout.clone(), id.clone());
    log.append(
        NewOperation::new(
            OperationKind::WorkspaceMigrated,
            ActorRef::unknown(),
            provider_id.clone(),
        )
        .message(format!(
            "migrated v0.1.0 workspace; {migrated} receipt(s) converted"
        )),
    )?;

    Ok(MigrationReport {
        workspace_id: id.to_string(),
        provider_id: provider_id.to_string(),
        receipts_migrated: migrated,
    })
}

/// Migrate if needed; returns `Ok(Some(report))` when a migration happened.
pub fn migrate_if_needed(root: &Path) -> DraftResult<Option<MigrationReport>> {
    if needs_migration(root) {
        Ok(Some(migrate(root)?))
    } else {
        Ok(None)
    }
}
