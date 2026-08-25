//! Workspace scanning and immutable snapshot persistence.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::support::actor::ActorRef;
use crate::support::common::{now, SnapshotId, WorkspacePath};
use crate::support::error::{DraftError, DraftResult};
use crate::support::fsutil::write_json;
use crate::support::hashing::{sha256_hex, try_canonical_hash};
use crate::support::pathguard;
use crate::workspace::object_store::ObjectStore;
use crate::workspace::state::{
    FileChange, FileChangeKind, FileKind, FileManifestEntry, Snapshot, WorkspaceStatus,
};
use crate::workspace::Workspace;

pub(crate) struct Scanner<'a> {
    workspace: &'a Workspace,
    ignore: IgnoreMatcher,
}

impl<'a> Scanner<'a> {
    pub(crate) fn new(workspace: &'a Workspace) -> DraftResult<Self> {
        Ok(Self {
            workspace,
            ignore: IgnoreMatcher::load(&workspace.layout.ignore_file())?,
        })
    }

    pub(crate) fn status(&self) -> DraftResult<WorkspaceStatus> {
        let previous = latest_snapshot(self.workspace)?;
        let current = self.current_manifest()?;
        let previous_map: BTreeMap<_, _> = previous
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .files
                    .iter()
                    .map(|file| (file.path.clone(), file.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut changes = diff_manifests(&previous_map, &current);
        detect_renames(&mut changes);
        Ok(WorkspaceStatus {
            workspace_id: self.workspace.workspace_id.clone(),
            root_path: self.workspace.root.display().to_string(),
            scanned_at: now(),
            ignored_count: self.ignore.ignored_count,
            has_draft_dir_violation: false,
            changes,
        })
    }

    pub(crate) fn current_manifest(
        &self,
    ) -> DraftResult<BTreeMap<WorkspacePath, FileManifestEntry>> {
        let mut manifest = BTreeMap::new();
        let store = ObjectStore::new(self.workspace.layout.clone());
        walk_dir(&self.workspace.root, &mut |path| {
            let relative = relative_path(&self.workspace.root, path)?;
            if self.ignore.is_ignored(relative.as_str()) || path.is_dir() {
                return Ok(());
            }
            let metadata = fs::symlink_metadata(path)?;
            let kind = file_kind(path, &metadata)?;
            let (content_hash, size_bytes) = if matches!(kind, FileKind::Directory) {
                (None, 0)
            } else if matches!(kind, FileKind::Symlink) {
                let target = fs::read_link(path)
                    .map_err(|error| {
                        DraftError::storage(format!(
                            "cannot read symlink target {}: {error}",
                            path.display()
                        ))
                    })?
                    .to_string_lossy()
                    .into_owned();
                (
                    Some(store.put_bytes(target.as_bytes())?),
                    target.len() as u64,
                )
            } else {
                let data = fs::read(path)?;
                (Some(store.put_bytes(&data)?), data.len() as u64)
            };
            manifest.insert(
                relative.clone(),
                FileManifestEntry {
                    path: relative,
                    file_kind: kind,
                    content_hash,
                    size_bytes,
                    modified_time: metadata.modified().ok().map(DateTime::<Utc>::from),
                    executable: executable(&metadata),
                },
            );
            Ok(())
        })?;
        Ok(manifest)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct IgnoreMatcher {
    patterns: Vec<String>,
    ignored_count: usize,
}

impl IgnoreMatcher {
    pub(crate) fn load(path: &Path) -> DraftResult<Self> {
        Ok(Self {
            patterns: read_ignore_lines(path)?,
            ignored_count: 0,
        })
    }

    pub(crate) fn is_ignored(&self, path: &str) -> bool {
        if pathguard::is_draft_path(path) {
            return true;
        }
        let mut ignored = false;
        for pattern in &self.patterns {
            let negated = pattern.starts_with('!');
            if pattern_match(pattern.trim_start_matches('!'), path) {
                ignored = !negated;
            }
        }
        ignored
    }
}

pub(crate) struct Snapshotter<'a> {
    workspace: &'a Workspace,
}

impl<'a> Snapshotter<'a> {
    pub(crate) fn new(workspace: &'a Workspace) -> DraftResult<Self> {
        Ok(Self { workspace })
    }

    pub(crate) fn create_snapshot(&self, actor: ActorRef) -> DraftResult<Snapshot> {
        let scanner = Scanner::new(self.workspace)?;
        let mut files: Vec<_> = scanner.current_manifest()?.into_values().collect();
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let content_object_refs = files
            .iter()
            .filter_map(|file| file.content_hash.clone())
            .collect();
        let ignored_patterns_hash = sha256_hex(
            read_ignore_lines(&self.workspace.layout.ignore_file())?
                .join("\n")
                .as_bytes(),
        );
        let mut snapshot = Snapshot {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::WorkspaceSnapshot,
            ),
            id: SnapshotId::generate(),
            workspace_id: self.workspace.workspace_id.clone(),
            manifest_hash: String::new(),
            files,
            content_object_refs,
            ignored_patterns_hash,
            created_at: now(),
            created_by: actor,
        };
        snapshot.manifest_hash = try_canonical_hash(&snapshot)?;
        write_json(
            &self
                .workspace
                .layout
                .snapshots_dir()
                .join(format!("{}.json", snapshot.id)),
            &snapshot,
        )?;
        Ok(snapshot)
    }
}

pub(crate) fn read_ignore_lines(path: &Path) -> DraftResult<Vec<String>> {
    let content = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    Ok(content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToString::to_string)
        .collect())
}

pub(crate) fn pattern_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    if pattern == ".draft/" {
        return pathguard::is_draft_path(path);
    }
    if let Some(directory) = pattern.strip_suffix("/**") {
        return path == directory || path.starts_with(&format!("{directory}/"));
    }
    if let Some(directory) = pattern.strip_suffix('/') {
        return path == directory || path.starts_with(&format!("{directory}/"));
    }
    if let Some(extension) = pattern.strip_prefix("*.") {
        return path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .ends_with(&format!(".{extension}"));
    }
    if pattern.contains('*') {
        let mut remainder = path;
        for part in pattern.split('*').filter(|part| !part.is_empty()) {
            let Some(index) = remainder.find(part) else {
                return false;
            };
            remainder = &remainder[index + part.len()..];
        }
        return true;
    }
    path == pattern || path.starts_with(&format!("{pattern}/"))
}

pub(crate) fn relative_path(root: &Path, path: &Path) -> DraftResult<WorkspacePath> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| DraftError::storage("path escaped workspace root"))?;
    Ok(WorkspacePath::from_relative(relative))
}

pub(crate) fn walk_dir<F: FnMut(&Path) -> DraftResult<()>>(
    root: &Path,
    callback: &mut F,
) -> DraftResult<()> {
    walk_dir_inner(root, root, callback)
}

fn walk_dir_inner<F: FnMut(&Path) -> DraftResult<()>>(
    root: &Path,
    directory: &Path,
    callback: &mut F,
) -> DraftResult<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let relative = relative_path(root, &path)?;
        if pathguard::is_draft_path(relative.as_str()) {
            continue;
        }
        callback(&path)?;
        if path.is_dir() {
            walk_dir_inner(root, &path, callback)?;
        }
    }
    Ok(())
}

fn file_kind(path: &Path, metadata: &fs::Metadata) -> DraftResult<FileKind> {
    if metadata.file_type().is_symlink() {
        return Ok(FileKind::Symlink);
    }
    if metadata.is_dir() {
        return Ok(FileKind::Directory);
    }
    let mut buffer = [0u8; 1024];
    let length = fs::File::open(path)
        .and_then(|mut file| file.read(&mut buffer))
        .unwrap_or(0);
    if buffer[..length].contains(&0) {
        Ok(FileKind::Binary)
    } else {
        Ok(FileKind::Text)
    }
}

#[cfg(unix)]
fn executable(metadata: &fs::Metadata) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(_metadata: &fs::Metadata) -> Option<bool> {
    None
}

pub(crate) fn diff_manifests(
    old: &BTreeMap<WorkspacePath, FileManifestEntry>,
    new: &BTreeMap<WorkspacePath, FileManifestEntry>,
) -> Vec<FileChange> {
    let mut changes = Vec::new();
    for (path, new_entry) in new {
        match old.get(path) {
            None => changes.push(FileChange {
                path: path.clone(),
                change_kind: FileChangeKind::Added,
                file_kind: new_entry.file_kind.clone(),
                old_hash: None,
                new_hash: new_entry.content_hash.clone(),
                size_bytes: Some(new_entry.size_bytes),
                executable: new_entry.executable,
            }),
            Some(old_entry)
                if old_entry.content_hash != new_entry.content_hash
                    || old_entry.file_kind != new_entry.file_kind
                    || old_entry.executable != new_entry.executable =>
            {
                changes.push(FileChange {
                    path: path.clone(),
                    change_kind: if old_entry.file_kind != new_entry.file_kind {
                        FileChangeKind::TypeChanged
                    } else if old_entry.executable != new_entry.executable {
                        FileChangeKind::PermissionChanged
                    } else {
                        FileChangeKind::Modified
                    },
                    file_kind: new_entry.file_kind.clone(),
                    old_hash: old_entry.content_hash.clone(),
                    new_hash: new_entry.content_hash.clone(),
                    size_bytes: Some(new_entry.size_bytes),
                    executable: new_entry.executable,
                });
            }
            _ => {}
        }
    }
    for (path, old_entry) in old {
        if !new.contains_key(path) {
            changes.push(FileChange {
                path: path.clone(),
                change_kind: FileChangeKind::Deleted,
                file_kind: old_entry.file_kind.clone(),
                old_hash: old_entry.content_hash.clone(),
                new_hash: None,
                size_bytes: Some(old_entry.size_bytes),
                executable: old_entry.executable,
            });
        }
    }
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    changes
}

fn detect_renames(changes: &mut [FileChange]) {
    let deleted: Vec<_> = changes
        .iter()
        .filter(|change| matches!(change.change_kind, FileChangeKind::Deleted))
        .map(|change| (change.old_hash.clone(), change.path.clone()))
        .collect();
    for change in changes
        .iter_mut()
        .filter(|change| matches!(change.change_kind, FileChangeKind::Added))
    {
        if let Some((_, from)) = deleted
            .iter()
            .find(|(hash, _)| hash.is_some() && hash == &change.new_hash)
        {
            change.change_kind = FileChangeKind::Renamed { from: from.clone() };
        }
    }
}

pub(crate) fn latest_snapshot(workspace: &Workspace) -> DraftResult<Option<Snapshot>> {
    let mut snapshots = Vec::new();
    for path in
        crate::support::fsutil::list_with_extension(&workspace.layout.snapshots_dir(), "json")?
    {
        snapshots.push(crate::contracts::read_persisted(&path)?);
    }
    snapshots.sort_by_key(|snapshot: &Snapshot| snapshot.created_at);
    Ok(snapshots.pop())
}
