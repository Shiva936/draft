//! Workspace initialization (FR-WS-002).

use std::path::Path;

use crate::common::{now, WorkspaceId};
use crate::error::DraftResult;
use crate::fsutil::{write_json, write_toml};
use crate::vcs::types::ProviderId;

use super::config::WorkspaceConfig;
use super::layout::DraftLayout;
use super::metadata::WorkspaceMetadata;
use super::Workspace;

/// Create `.draft/` and initial config/metadata, binding the workspace to
/// `provider_id`. Does not append an operation; the caller (orchestration layer)
/// records `WorkspaceInitialized` so the operation log stays the single writer.
pub fn initialize(
    root: &Path,
    provider_root: &Path,
    provider_id: ProviderId,
    experimental_ack: bool,
) -> DraftResult<Workspace> {
    let layout = DraftLayout::for_root(root);
    layout.create_all()?;

    let mut config = WorkspaceConfig::new(provider_id.clone());
    config.provider.experimental_ack = experimental_ack;

    let id = WorkspaceId::generate();
    let provider_root_rel = provider_root
        .strip_prefix(root)
        .map(|p| {
            if p.as_os_str().is_empty() {
                ".".to_string()
            } else {
                p.to_string_lossy().replace('\\', "/")
            }
        })
        .unwrap_or_else(|_| ".".to_string());

    let meta = WorkspaceMetadata {
        id: id.clone(),
        draft_version: crate::DRAFT_VERSION.to_string(),
        provider_id: provider_id.clone(),
        provider_root_rel,
        created_at: now(),
        migrated_from: None,
    };

    write_toml(&layout.config_toml(), &config)?;
    write_json(&layout.workspace_json(), &meta)?;

    Ok(Workspace {
        id,
        root: root.to_path_buf(),
        draft_dir: layout.draft_dir.clone(),
        provider_id,
        provider_root: provider_root.to_path_buf(),
        config,
    })
}
