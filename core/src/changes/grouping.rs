//! Automatic grouping of a provider delta into Draft changes.
//!
//! Heuristic, path-based classification (parity with v0.1.0's change grouper),
//! but operating on the neutral [`ProviderDelta`].

use sha2::{Digest, Sha256};

use crate::common::{DraftChangeId, WorkspaceId};
use crate::vcs::types::{FileDelta, ProviderDelta};

use super::{DraftChange, FileChangeRef, GroupingSource};

/// Derive a **stable** change id from the workspace, group kind, and the sorted
/// set of paths. The same logical group keeps the same id across rescans, so
/// review decisions recorded in one scan still match at finalization time.
fn deterministic_id(ws: &WorkspaceId, kind_title: &str, files: &[FileChangeRef]) -> DraftChangeId {
    let mut paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    let mut h = Sha256::new();
    h.update(ws.as_str().as_bytes());
    h.update([0]);
    h.update(kind_title.as_bytes());
    for p in paths {
        h.update([0]);
        h.update(p.as_bytes());
    }
    let digest = format!("{:x}", h.finalize());
    DraftChangeId::new(format!("chg_{}", &digest[..12]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeKind {
    Test,
    Config,
    Dependency,
    Migration,
    Docs,
    Generated,
    Binary,
    Source,
}

impl ChangeKind {
    fn title(&self) -> &'static str {
        match self {
            ChangeKind::Test => "Tests",
            ChangeKind::Config => "Configuration",
            ChangeKind::Dependency => "Dependencies",
            ChangeKind::Migration => "Migrations",
            ChangeKind::Docs => "Documentation",
            ChangeKind::Generated => "Generated files",
            ChangeKind::Binary => "Binary assets",
            ChangeKind::Source => "Source changes",
        }
    }
}

fn classify(f: &FileDelta) -> ChangeKind {
    if f.binary {
        return ChangeKind::Binary;
    }
    let p = f.path.as_str().to_ascii_lowercase();
    let base = p.rsplit('/').next().unwrap_or(&p);
    if p.contains("migration") || p.contains("/migrations/") {
        ChangeKind::Migration
    } else if p.contains("test")
        || p.contains("/tests/")
        || p.contains("__tests__")
        || base.contains("_test.")
    {
        ChangeKind::Test
    } else if matches!(
        base,
        "cargo.toml"
            | "cargo.lock"
            | "package.json"
            | "package-lock.json"
            | "go.mod"
            | "go.sum"
            | "requirements.txt"
            | "pom.xml"
    ) || base.ends_with(".lock")
    {
        ChangeKind::Dependency
    } else if p.contains("/docs/") || base.ends_with(".md") || base == "readme" {
        ChangeKind::Docs
    } else if p.contains("generated") || p.contains(".gen.") || base.ends_with(".pb.go") {
        ChangeKind::Generated
    } else if base.ends_with(".toml")
        || base.ends_with(".yaml")
        || base.ends_with(".yml")
        || base.ends_with(".ini")
        || base.ends_with(".cfg")
        || base.starts_with('.')
    {
        ChangeKind::Config
    } else {
        ChangeKind::Source
    }
}

fn file_ref(f: &FileDelta) -> FileChangeRef {
    FileChangeRef {
        path: f.path.clone(),
        old_path: f.old_path.clone(),
        status: f.status,
        additions: f.additions,
        deletions: f.deletions,
        binary: f.binary,
    }
}

/// Group a delta's files into automatic Draft changes (one per kind present),
/// preserving a stable order.
pub fn group_delta(workspace_id: &WorkspaceId, delta: &ProviderDelta) -> Vec<DraftChange> {
    use std::collections::BTreeMap;
    let mut buckets: BTreeMap<&'static str, (ChangeKind, Vec<FileChangeRef>)> = BTreeMap::new();
    for f in &delta.files {
        let kind = classify(f);
        buckets
            .entry(kind.title())
            .or_insert_with(|| (kind, Vec::new()))
            .1
            .push(file_ref(f));
    }
    buckets
        .into_values()
        .map(|(kind, files)| {
            let id = deterministic_id(workspace_id, kind.title(), &files);
            let mut change = DraftChange::new(
                workspace_id.clone(),
                Some(kind.title().to_string()),
                files,
                GroupingSource::Automatic,
            );
            change.id = id;
            change
        })
        .collect()
}
