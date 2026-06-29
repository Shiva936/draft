//! Provider-neutral workspace model: detection, init, metadata, layout.

pub mod config;
pub mod detection;
pub mod init;
pub mod layout;
pub mod metadata;

use std::path::{Path, PathBuf};

pub use config::{
    FinalizationConfig, ProviderBinding, RiskConfig, VerificationCommandConfig, VerificationConfig,
    WorkspaceConfig,
};
pub use detection::{detect_provider, find_workspace_root};
pub use init::initialize;
pub use layout::{DraftLayout, DRAFT_DIR};
pub use metadata::WorkspaceMetadata;

use crate::common::WorkspaceId;
use crate::error::{DraftError, DraftErrorKind, DraftResult};
use crate::fsutil::{read_json, read_toml, write_json, write_toml};
use crate::vcs::types::ProviderId;

/// A runtime handle to an initialized Draft workspace.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub root: PathBuf,
    pub draft_dir: PathBuf,
    pub provider_id: ProviderId,
    pub provider_root: PathBuf,
    pub config: WorkspaceConfig,
}

impl Workspace {
    pub fn layout(&self) -> DraftLayout {
        DraftLayout::new(self.draft_dir.clone())
    }

    /// Open an existing workspace by walking up from `start` to find `.draft/`.
    pub fn open(start: &Path) -> DraftResult<Workspace> {
        let root = find_workspace_root(start).ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::WorkspaceNotFound,
                "No Draft workspace found here or in any parent directory.",
            )
            .with_suggestion("Run `draft workspace init` to create one.")
        })?;
        Workspace::open_at(&root)
    }

    /// Open the workspace whose root is exactly `root`.
    pub fn open_at(root: &Path) -> DraftResult<Workspace> {
        let layout = DraftLayout::for_root(root);
        if !layout.exists() {
            return Err(DraftError::new(
                DraftErrorKind::WorkspaceNotFound,
                format!("No .draft/ directory at {}", root.display()),
            ));
        }
        let meta: WorkspaceMetadata = read_json(&layout.workspace_json())?;
        let config: WorkspaceConfig = read_toml(&layout.config_toml())?;
        let provider_root = normalize_provider_root(root, &meta.provider_root_rel);
        Ok(Workspace {
            id: meta.id,
            root: root.to_path_buf(),
            draft_dir: layout.draft_dir.clone(),
            provider_id: meta.provider_id,
            provider_root,
            config,
        })
    }

    /// Persist config + metadata for this workspace.
    pub fn save(&self) -> DraftResult<()> {
        let layout = self.layout();
        write_toml(&layout.config_toml(), &self.config)?;
        let provider_root_rel = self
            .provider_root
            .strip_prefix(&self.root)
            .map(|p| {
                if p.as_os_str().is_empty() {
                    ".".to_string()
                } else {
                    p.to_string_lossy().replace('\\', "/")
                }
            })
            .unwrap_or_else(|_| ".".to_string());
        let meta = WorkspaceMetadata {
            id: self.id.clone(),
            draft_version: crate::DRAFT_VERSION.to_string(),
            provider_id: self.provider_id.clone(),
            provider_root_rel,
            created_at: crate::common::now(),
            migrated_from: None,
        };
        write_json(&layout.workspace_json(), &meta)?;
        Ok(())
    }
}

fn normalize_provider_root(root: &Path, rel: &str) -> PathBuf {
    if rel == "." || rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    }
}
