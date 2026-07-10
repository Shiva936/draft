//! Safe local maintenance for Draft v0.3.3.

use crate::error::DraftResult;
use crate::event::EventLog;
use crate::fsutil;
use crate::layout::ProjectPaths;
use crate::pack::{ImportState, PackManifest, SaveState};
use crate::stable::StableHeadStore;
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

pub fn run(paths: &ProjectPaths) -> DraftResult<GcReport> {
    let stable_head_valid = StableHeadStore::new(paths.clone()).read().is_ok();
    let event_chain_valid = EventLog::new(paths.clone()).verify_chain().is_ok();
    let active_packs_preserved = active_pack_count(paths)?;
    let mut removed_entries = 0;
    let (disposed_packs_pruned, orphaned_pack_dirs_pruned) = prune_inactive_pack_dirs(paths)?;
    removed_entries += disposed_packs_pruned + orphaned_pack_dirs_pruned;

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
    let affected_path_index_packs = crate::index::AffectedPathIndex::rebuild(paths)?;

    fsutil::write_json(
        &paths.stable_graph_index(),
        &serde_json::json!({
            "schema_version": crate::DRAFT_SCHEMA_VERSION,
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

fn active_pack_count(paths: &ProjectPaths) -> DraftResult<usize> {
    if !paths.packs_dir().exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in std::fs::read_dir(paths.packs_dir())? {
        let manifest_path = entry?.path().join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        let Ok(manifest) = fsutil::read_json::<PackManifest>(&manifest_path) else {
            count += 1;
            continue;
        };
        if manifest.save_state != SaveState::Saved
            && manifest.import_state != ImportState::ImportSaved
        {
            count += 1;
        }
    }
    Ok(count)
}

fn prune_inactive_pack_dirs(paths: &ProjectPaths) -> DraftResult<(usize, usize)> {
    if !paths.packs_dir().exists() {
        return Ok((0, 0));
    }
    let mut disposed = 0;
    let mut orphaned = 0;
    for entry in std::fs::read_dir(paths.packs_dir())? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("manifest.json");
        if !manifest_path.exists() {
            std::fs::remove_dir_all(&path)?;
            orphaned += 1;
            continue;
        }
        let Ok(manifest) = fsutil::read_json::<PackManifest>(&manifest_path) else {
            continue;
        };
        let finalized = manifest.save_state == SaveState::Saved
            || manifest.import_state == ImportState::ImportSaved;
        if finalized {
            std::fs::remove_dir_all(&path)?;
            disposed += 1;
        }
    }
    Ok((disposed, orphaned))
}
