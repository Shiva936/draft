//! Canonical project source view and workspace revision contracts.
//!
//! All consumers that need to describe source state use this module. The view
//! is deterministic, hard-excludes `.draft/`, normalizes separators, records
//! file type and symlink targets, and applies the project ignore file. Provider
//! control directories are represented by generic policy rather than provider
//! semantics in core.

use crate::support::common::WorkspaceId;
use crate::support::error::{DraftError, DraftResult};
use crate::support::hashing::{canonical_hash, canonical_json, domain_hash};
use crate::support::pathguard;
use crate::workspace::{DraftLayout, WorkspaceMetadata};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalFileType {
    Text,
    Binary,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalSourceEntry {
    pub path: String,
    pub kind: CanonicalFileType,
    pub digest: String,
    pub bytes: u64,
    pub executable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalSourcePolicy {
    pub schema_version: u32,
    /// Generic provider/control directories excluded by source policy.
    pub excluded_control_directories: Vec<String>,
    /// If false, a source symlink is rejected instead of represented in-view.
    pub allow_symlinks: bool,
}

impl crate::contracts::VersionedContract for CanonicalSourcePolicy {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::CanonicalSourcePolicy;
}

impl Default for CanonicalSourcePolicy {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::CanonicalSourcePolicy,
            ),
            excluded_control_directories: vec![".git".into()],
            allow_symlinks: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalSourceView {
    pub schema_version: u32,
    pub entries: Vec<CanonicalSourceEntry>,
    pub content_digest: String,
}

impl crate::contracts::VersionedContract for CanonicalSourceView {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::CanonicalSourceView;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRevision {
    pub schema_version: u32,
    pub workspace_id: WorkspaceId,
    pub content_digest: String,
}

impl crate::contracts::VersionedContract for WorkspaceRevision {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::WorkspaceRevision;
}

impl WorkspaceRevision {
    pub fn derive(root: &Path) -> DraftResult<Self> {
        let layout = DraftLayout::for_root(root);
        let metadata: WorkspaceMetadata =
            crate::contracts::read_persisted(&layout.workspace_json())?;
        Self::derive_for(
            root,
            metadata.workspace_id,
            &CanonicalSourcePolicy::default(),
        )
    }

    pub fn derive_for(
        root: &Path,
        workspace_id: WorkspaceId,
        policy: &CanonicalSourcePolicy,
    ) -> DraftResult<Self> {
        let view = CanonicalSourceView::build(root, policy)?;
        Ok(Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::WorkspaceRevision,
            ),
            workspace_id,
            content_digest: view.content_digest,
        })
    }

    pub fn cache_key(&self) -> String {
        canonical_hash(self)
    }
}

impl CanonicalSourceView {
    pub fn build(root: &Path, policy: &CanonicalSourcePolicy) -> DraftResult<Self> {
        let root = root.canonicalize().map_err(|error| {
            DraftError::storage(format!(
                "cannot open source root {}: {error}",
                root.display()
            ))
        })?;
        let mut builder = ignore::WalkBuilder::new(&root);
        builder
            .hidden(false)
            .git_ignore(false)
            .git_exclude(false)
            .parents(false);
        let ignore_file = root.join(".draft/.ignore");
        if ignore_file.is_file() {
            builder.add_ignore(ignore_file);
        }

        let mut entries = Vec::new();
        for item in builder.build() {
            let item = item.map_err(|error| DraftError::storage(error.to_string()))?;
            let path = item.path();
            if path == root || pathguard::path_is_draft(path) {
                continue;
            }
            let relative = path
                .strip_prefix(&root)
                .map_err(|error| DraftError::storage(error.to_string()))?
                .to_str()
                .ok_or_else(|| DraftError::invalid_config("source path is not valid UTF-8"))?;
            let relative = pathguard::check_relative(relative).map_err(|error| {
                DraftError::invalid_config(format!("source path is not canonical: {error}"))
            })?;
            let first = relative.split('/').next().unwrap_or_default();
            if policy
                .excluded_control_directories
                .iter()
                .any(|directory| first.eq_ignore_ascii_case(directory))
            {
                continue;
            }
            let metadata = fs::symlink_metadata(path)?;
            if metadata.is_dir() {
                continue;
            }
            if metadata.file_type().is_symlink() {
                if !policy.allow_symlinks {
                    return Err(DraftError::invalid_config(format!(
                        "source policy rejects symlink '{relative}'"
                    )));
                }
                let target = fs::read_link(path)?;
                let target = target.to_str().ok_or_else(|| {
                    DraftError::invalid_config(format!(
                        "symlink target for '{relative}' is not valid UTF-8"
                    ))
                })?;
                let target = pathguard::check_relative(target).map_err(|error| {
                    DraftError::invalid_config(format!(
                        "symlink target for '{relative}' is not canonical: {error}"
                    ))
                })?;
                entries.push(CanonicalSourceEntry {
                    path: relative,
                    kind: CanonicalFileType::Symlink,
                    digest: source_entry_digest("symlink", false, target.as_bytes()),
                    bytes: target.len() as u64,
                    executable: false,
                    symlink_target: Some(target),
                });
                continue;
            }
            if !metadata.is_file() {
                return Err(DraftError::invalid_config(format!(
                    "source view rejects non-regular entry '{relative}'"
                )));
            }
            let bytes = fs::read(path)?;
            let kind = if bytes.iter().take(8192).any(|byte| *byte == 0) {
                CanonicalFileType::Binary
            } else {
                CanonicalFileType::Text
            };
            let executable = tracked_executable(&metadata);
            entries.push(CanonicalSourceEntry {
                path: relative,
                kind: kind.clone(),
                digest: source_entry_digest(kind_label(&kind), executable, &bytes),
                bytes: bytes.len() as u64,
                executable,
                symlink_target: None,
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let encoded = entries
            .iter()
            .map(|entry| canonical_json(&serde_json::to_value(entry).expect("source entry")))
            .collect::<Vec<_>>();
        let content_digest = domain_hash(
            "draft-canonical-source-view",
            encoded.iter().map(|entry| entry.as_bytes()),
        );
        Ok(Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::CanonicalSourceView,
            ),
            entries,
            content_digest,
        })
    }
}

/// Compute the canonical source digest for a workspace root.
pub fn workspace_hash(root: &Path) -> DraftResult<String> {
    Ok(CanonicalSourceView::build(root, &CanonicalSourcePolicy::default())?.content_digest)
}

/// Persist a rebuildable digest cache while preserving bit-for-bit equivalence
/// with full canonical recomputation.
pub fn workspace_hash_cached(root: &Path, cache_file: &Path) -> DraftResult<String> {
    let digest = workspace_hash(root)?;
    let _ = crate::support::fsutil::write_json(
        cache_file,
        &serde_json::json!({
            "schema_version": crate::contracts::current_version(crate::contracts::ContractId::CanonicalSourceView),
            "content_digest": digest,
        }),
    );
    Ok(digest)
}

fn kind_label(kind: &CanonicalFileType) -> &'static str {
    match kind {
        CanonicalFileType::Text => "text",
        CanonicalFileType::Binary => "binary",
        CanonicalFileType::Symlink => "symlink",
    }
}

fn source_entry_digest(kind: &str, executable: bool, bytes: &[u8]) -> String {
    let executable = [u8::from(executable)];
    domain_hash(
        "draft-canonical-source-entry",
        [kind.as_bytes(), executable.as_slice(), bytes],
    )
}

#[cfg(unix)]
fn tracked_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn tracked_executable(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_is_deterministic_and_hard_excludes_draft_and_control_state() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".draft")).unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join("b.txt"), "b").unwrap();
        fs::write(tmp.path().join("a.txt"), "a").unwrap();
        fs::write(tmp.path().join(".draft/secret"), "secret").unwrap();
        fs::write(tmp.path().join(".git/config"), "control").unwrap();
        let view = CanonicalSourceView::build(tmp.path(), &Default::default()).unwrap();
        assert_eq!(
            view.entries
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.txt", "b.txt"]
        );
        assert_eq!(
            view,
            CanonicalSourceView::build(tmp.path(), &Default::default()).unwrap()
        );
    }

    #[test]
    fn content_digest_is_independent_of_workspace_identity_and_location() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        fs::write(left.path().join("same.txt"), b"same bytes").unwrap();
        fs::write(right.path().join("same.txt"), b"same bytes").unwrap();
        let left_view = CanonicalSourceView::build(left.path(), &Default::default()).unwrap();
        let right_view = CanonicalSourceView::build(right.path(), &Default::default()).unwrap();
        assert_eq!(left_view.content_digest, right_view.content_digest);

        let left_revision = WorkspaceRevision::derive_for(
            left.path(),
            WorkspaceId::new("ws_left"),
            &Default::default(),
        )
        .unwrap();
        let right_revision = WorkspaceRevision::derive_for(
            right.path(),
            WorkspaceId::new("ws_right"),
            &Default::default(),
        )
        .unwrap();
        assert_ne!(left_revision.workspace_id, right_revision.workspace_id);
        assert_eq!(left_revision.content_digest, right_revision.content_digest);
    }
}
