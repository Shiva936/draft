//! Safe local maintenance for canonical Draft state.

use crate::activity::ActivityLog;
use crate::dcg::change_store::ChangeContentStore;
use crate::dcg::revision::RevisionState;
use crate::project::layout::DraftLayout;
use crate::project::WorkspaceMetadata;
use crate::support::error::DraftResult;
use crate::support::fsutil;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct GcReport {
    pub removed_entries: usize,
    pub accepted_baseline_valid: bool,
    pub activity_chain_valid: bool,
    pub active_changes_preserved: usize,
    pub disposed_changes_pruned: usize,
    pub orphaned_change_dirs_pruned: usize,
    pub affected_path_index_changes: usize,
    /// What the reachability walk concluded about the project's own artifacts.
    ///
    /// Reported even though this sweep deletes only rebuildable material,
    /// because the interesting number is not what was removed — it is what was
    /// retained, and why. An operator watching `unknown` climb is watching
    /// something GC can no longer answer questions about.
    pub reachability: ReachabilitySummary,
}

/// The reachability walk's verdict, in aggregate.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ReachabilitySummary {
    pub retained: usize,
    /// Retained only because a transaction is unfinished. Self-clearing: when
    /// the journal finalizes this falls to zero on its own.
    pub retained_by_recovery: usize,
    pub collectible: usize,
    /// Artifacts GC could not resolve. Never collected, and never folded into
    /// either other number — "we could not tell" and "it is garbage" must not
    /// be the same value.
    pub unknown: usize,
}

pub fn run(paths: &DraftLayout) -> DraftResult<GcReport> {
    // The accepted Baseline is a GC root: collection may not run against a
    // project whose accepted state cannot be resolved.
    let accepted = crate::dcg::baseline::accepted_state_root(paths)?;
    let workspace: WorkspaceMetadata = crate::contracts::read_persisted(&paths.project_json())?;
    ActivityLog::new(paths.events_dir(), workspace.workspace_id.to_string()).verify_chain()?;
    let accepted_baseline_valid = accepted.is_some();
    let activity_chain_valid = true;
    let active_changes_preserved = active_change_count(paths)?;
    let mut removed_entries = 0;
    // Immutable Change manifests, revisions, evidence, and history are
    // authoritative and are never garbage-collected.
    let disposed_changes_pruned = 0;
    let orphaned_change_dirs_pruned = 0;

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

    // The reachability walk, over the project's real root graph. Nothing here
    // deletes a canonical artifact — this sweep removes only rebuildable
    // material — but the classification runs on every sweep, because a root
    // graph nobody walks is a root graph that silently stops covering the
    // artifacts a new domain adds.
    let reachability = summarize(&crate::app::gc::classify(&crate::app::roots::build(paths)?));

    // Rebuild the affected-path index from the Changes that survived pruning
    // Collection rebuilds the performance indexes as it goes, so a pruned
    // store never leaves a stale index behind.
    let affected_path_index_changes = crate::read_model::index::AffectedPathIndex::rebuild(paths)?;

    fsutil::write_json(
        &paths.change_graph_index(),
        &serde_json::json!({
            "schema_version": crate::contracts::current_version(crate::contracts::ContractId::ChangeGraphIndex),
            "accepted_baseline_valid": accepted_baseline_valid,
            "activity_chain_valid": activity_chain_valid,
            "active_changes": active_changes_preserved,
            "disposed_changes_pruned": disposed_changes_pruned,
            "orphaned_change_dirs_pruned": orphaned_change_dirs_pruned,
            "affected_path_index_changes": affected_path_index_changes,
        }),
    )?;

    Ok(GcReport {
        removed_entries,
        accepted_baseline_valid,
        activity_chain_valid,
        active_changes_preserved,
        disposed_changes_pruned,
        orphaned_change_dirs_pruned,
        affected_path_index_changes,
        reachability,
    })
}

fn summarize(
    answers: &std::collections::BTreeMap<crate::app::gc::ArtifactKey, crate::app::gc::Reachability>,
) -> ReachabilitySummary {
    use crate::app::gc::{Reachability, RetentionReason};
    let mut summary = ReachabilitySummary::default();
    for answer in answers.values() {
        match answer {
            Reachability::Retained(reason) => {
                summary.retained += 1;
                if matches!(
                    reason,
                    RetentionReason::ActiveJournal(_) | RetentionReason::UndrainedOutbox(_)
                ) {
                    summary.retained_by_recovery += 1;
                }
            }
            Reachability::Collectible => summary.collectible += 1,
            Reachability::Unknown { .. } => summary.unknown += 1,
        }
    }
    summary
}

fn active_change_count(paths: &DraftLayout) -> DraftResult<usize> {
    let store = ChangeContentStore::new(paths.clone());
    let mut count = 0;
    for manifest in store.list()? {
        if store
            .read_lifecycle_in(
                crate::dcg::change_store::ChangeLocation::Store,
                &manifest.change_id,
            )?
            .lifecycle
            != RevisionState::Submitted
        {
            count += 1;
        }
    }
    Ok(count)
}
