//! Safe local maintenance for canonical Draft state.

use crate::pack::lifecycle::PackLifecycle;
use crate::pack::PackStore;
use crate::support::error::DraftResult;
use crate::support::fsutil;
use crate::trust::event::EventLog;
use crate::workspace::layout::DraftLayout;
use crate::workspace::stable::StableHeadStore;
use crate::workspace::WorkspaceMetadata;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct GcReport {
    pub removed_entries: usize,
    pub stable_head_valid: bool,
    pub event_chain_valid: bool,
    pub active_packs_preserved: usize,
    pub disposed_packs_pruned: usize,
    pub orphaned_pack_dirs_pruned: usize,
    pub affected_path_index_packs: usize,
}

pub fn run(paths: &DraftLayout) -> DraftResult<GcReport> {
    StableHeadStore::new(paths.clone()).read()?;
    let workspace: WorkspaceMetadata = crate::contracts::read_persisted(&paths.workspace_json())?;
    EventLog::workspace(paths.clone(), workspace.workspace_id.to_string()).verify_chain()?;
    let stable_head_valid = true;
    let event_chain_valid = true;
    let active_packs_preserved = active_pack_count(paths)?;
    let mut removed_entries = 0;
    // Immutable pack manifests, revisions, evidence, and history are
    // authoritative and are never garbage-collected.
    let disposed_packs_pruned = 0;
    let orphaned_pack_dirs_pruned = 0;

    for dir in [paths.tmp_dir(), paths.cache_dir().join("verify")] {
        if dir.exists() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.is_file() {
                    std::fs::remove_file(&path)?;
                    removed_entries += 1;
                } else if path.is_dir() && path.starts_with(paths.tmp_dir()) {
                    std::fs::remove_dir_all(&path)?;
                    removed_entries += 1;
                }
            }
        }
    }

    // Rebuild the affected-path index from the packs that survived pruning
    // (SRS-FR-143: gc rebuilds performance indexes).
    let affected_path_index_packs = crate::review::index::AffectedPathIndex::rebuild(paths)?;

    fsutil::write_json(
        &paths.stable_graph_index(),
        &serde_json::json!({
            "schema_version": crate::contracts::current_version(crate::contracts::ContractId::StableGraphIndex),
            "stable_head_valid": stable_head_valid,
            "event_chain_valid": event_chain_valid,
            "active_packs": active_packs_preserved,
            "disposed_packs_pruned": disposed_packs_pruned,
            "orphaned_pack_dirs_pruned": orphaned_pack_dirs_pruned,
            "affected_path_index_packs": affected_path_index_packs,
        }),
    )?;

    Ok(GcReport {
        removed_entries,
        stable_head_valid,
        event_chain_valid,
        active_packs_preserved,
        disposed_packs_pruned,
        orphaned_pack_dirs_pruned,
        affected_path_index_packs,
    })
}

fn active_pack_count(paths: &DraftLayout) -> DraftResult<usize> {
    let store = PackStore::new(paths.clone());
    let mut count = 0;
    for manifest in store.list()? {
        if store
            .read_lifecycle_in(crate::pack::PackLocation::Store, &manifest.pack_id)?
            .lifecycle
            != PackLifecycle::Submitted
        {
            count += 1;
        }
    }
    Ok(count)
}
