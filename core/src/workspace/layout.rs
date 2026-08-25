//! The sole authoritative map of a project-local `.draft/` store.

use crate::support::error::DraftResult;
use crate::support::fsutil::ensure_dir;
use crate::support::hidden::{self, HiddenStatus};
use std::path::{Path, PathBuf};

/// Handle to a project `.draft/` store and its canonical layout.
#[derive(Debug, Clone)]
pub struct DraftLayout {
    pub draft_dir: PathBuf,
}

impl DraftLayout {
    /// Build the layout for a project root (the directory that contains `.draft`).
    pub fn for_root(root: &Path) -> Self {
        DraftLayout {
            draft_dir: root.join(".draft"),
        }
    }

    /// Build the layout given the `.draft` directory directly.
    pub fn at(draft_dir: impl Into<PathBuf>) -> Self {
        DraftLayout {
            draft_dir: draft_dir.into(),
        }
    }

    pub fn draft_dir(&self) -> &Path {
        &self.draft_dir
    }

    pub fn root(&self) -> PathBuf {
        self.draft_dir
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    }

    // ---- Top-level files -------------------------------------------------

    pub fn workspace_json(&self) -> PathBuf {
        self.draft_dir.join("workspace.json")
    }
    pub fn config_toml(&self) -> PathBuf {
        self.draft_dir.join("config.toml")
    }
    pub fn ignore_file(&self) -> PathBuf {
        self.draft_dir.join(".ignore")
    }
    pub fn verify_toml(&self) -> PathBuf {
        self.draft_dir.join("verify.toml")
    }
    pub fn risk_toml(&self) -> PathBuf {
        self.draft_dir.join("risk.toml")
    }
    pub fn selected_pack_file(&self) -> PathBuf {
        self.draft_dir.join("selected-pack")
    }
    pub fn policy_toml(&self) -> PathBuf {
        self.draft_dir.join("policy.toml")
    }

    // ---- Events ----------------------------------------------------------

    pub fn events_dir(&self) -> PathBuf {
        self.draft_dir.join("events")
    }
    pub fn event_log(&self) -> PathBuf {
        self.events_dir().join("event.log")
    }
    pub fn event_index(&self) -> PathBuf {
        self.events_dir().join("event.index")
    }

    // ---- Receipts & transparency ----------------------------------------

    pub fn receipts_dir(&self) -> PathBuf {
        self.draft_dir.join("receipts")
    }
    pub fn receipt_file(&self, receipt_id: &str) -> PathBuf {
        self.receipts_dir().join(format!("{receipt_id}.json"))
    }
    pub fn transparency_dir(&self) -> PathBuf {
        self.draft_dir.join("transparency")
    }
    pub fn transparency_chain(&self) -> PathBuf {
        self.transparency_dir().join("chain.log")
    }

    // ---- Stable head / indexes / operation state ------------------------

    pub fn stable_head_dir(&self) -> PathBuf {
        self.draft_dir.join("stable_head")
    }
    pub fn stable_head_file(&self) -> PathBuf {
        self.stable_head_dir().join("head.json")
    }
    pub fn stable_head_receipt_file(&self) -> PathBuf {
        self.stable_head_dir().join("receipt.json")
    }
    pub fn index_dir(&self) -> PathBuf {
        self.draft_dir.join("index")
    }
    pub fn stable_graph_index(&self) -> PathBuf {
        self.index_dir().join("stable-graph.json")
    }
    pub fn affected_path_index(&self) -> PathBuf {
        self.index_dir().join("affected-paths.json")
    }
    pub fn verification_cache_manifest(&self) -> PathBuf {
        self.index_dir().join("verification-cache.json")
    }
    pub fn workspace_hash_cache(&self) -> PathBuf {
        self.cache_sub("hashes").join("workspace-hash.json")
    }
    pub fn tmp_dir(&self) -> PathBuf {
        self.draft_dir.join("tmp")
    }
    pub fn op_tmp_dir(&self, op_id: &str) -> PathBuf {
        self.tmp_dir().join(op_id)
    }
    pub fn locks_dir(&self) -> PathBuf {
        self.draft_dir.join("locks")
    }
    pub fn objects_dir(&self) -> PathBuf {
        self.draft_dir.join("objects/blake3")
    }
    pub fn object_packs_dir(&self) -> PathBuf {
        self.draft_dir.join("objects/packs")
    }
    pub fn snapshots_dir(&self) -> PathBuf {
        self.draft_dir.join("snapshots")
    }
    /// Mutable workspaces used while deriving immutable pack revisions.
    pub fn pack_workspaces_dir(&self) -> PathBuf {
        self.draft_dir.join("pack-workspaces")
    }
    pub fn pack_workspace_dir(&self, pack_id: impl ToString) -> PathBuf {
        self.pack_workspaces_dir().join(pack_id.to_string())
    }
    pub fn index_file(&self) -> PathBuf {
        self.indexes_dir().join("draft.sqlite")
    }

    pub fn tasks_dir(&self) -> PathBuf {
        self.draft_dir.join("tasks")
    }
    pub fn task_file(&self, id: &str) -> PathBuf {
        self.tasks_dir().join(format!("{id}.json"))
    }
    pub fn executions_dir(&self) -> PathBuf {
        self.draft_dir.join("executions")
    }
    pub fn execution_file(&self, id: &str) -> PathBuf {
        self.executions_dir().join(format!("{id}.json"))
    }
    /// Per-execution runtime state: task contract, logs, isolated workspace.
    pub fn runtime_dir(&self) -> PathBuf {
        self.draft_dir.join("runtime")
    }
    pub fn execution_runtime_dir(&self, execution_id: &str) -> PathBuf {
        self.runtime_dir().join(execution_id)
    }
    pub fn execution_contract_file(&self, execution_id: &str) -> PathBuf {
        self.execution_runtime_dir(execution_id)
            .join("task_contract.json")
    }
    pub fn execution_work_dir(&self, execution_id: &str) -> PathBuf {
        self.execution_runtime_dir(execution_id).join("work")
    }
    /// Canonical derived indexes tree (`.draft/indexes/`).
    pub fn indexes_dir(&self) -> PathBuf {
        self.draft_dir.join("indexes")
    }
    pub fn task_indexes_dir(&self) -> PathBuf {
        self.indexes_dir().join("tasks")
    }
    pub fn task_name_index(&self) -> PathBuf {
        self.task_indexes_dir().join("by-name.json")
    }
    pub fn evidence_dir(&self) -> PathBuf {
        self.draft_dir.join("evidence")
    }
    pub fn decisions_dir(&self) -> PathBuf {
        self.draft_dir.join("decisions")
    }
    pub fn waivers_dir(&self) -> PathBuf {
        self.draft_dir.join("waivers")
    }
    pub fn editor_dir(&self) -> PathBuf {
        self.draft_dir.join("editor")
    }
    pub fn recovery_dir(&self) -> PathBuf {
        self.draft_dir.join("recovery")
    }
    pub fn backups_dir(&self) -> PathBuf {
        self.draft_dir.join("backups")
    }
    pub fn lock_file(&self, name: &str) -> PathBuf {
        self.locks_dir().join(format!("{name}.lock"))
    }

    // ---- Packs -----------------------------------------------------------

    pub fn packs_dir(&self) -> PathBuf {
        self.draft_dir.join("packs")
    }
    pub fn pack_dir(&self, pack_id: impl ToString) -> PathBuf {
        self.packs_dir().join(pack_id.to_string())
    }
    pub fn pack_manifest(&self, pack_id: &str) -> PathBuf {
        self.pack_dir(pack_id).join("manifest.json")
    }
    pub fn pack_lock(&self, pack_id: &str) -> PathBuf {
        self.pack_dir(pack_id).join("pack.lock.json")
    }
    pub fn pack_changes(&self, pack_id: &str) -> PathBuf {
        self.pack_dir(pack_id).join("changes.patch")
    }
    pub fn pack_risk(&self, pack_id: &str) -> PathBuf {
        self.pack_dir(pack_id).join("risk.json")
    }
    pub fn pack_verify(&self, pack_id: &str) -> PathBuf {
        self.pack_dir(pack_id).join("verify.json")
    }
    pub fn pack_lsif(&self, pack_id: &str) -> PathBuf {
        self.pack_dir(pack_id).join("lsif.json")
    }
    pub fn pack_receipts(&self, pack_id: &str) -> PathBuf {
        self.pack_dir(pack_id).join("receipts.json")
    }

    // ---- Checkpoints / import / export ----------------------------------

    pub fn checkpoints_dir(&self) -> PathBuf {
        self.draft_dir.join("checkpoints")
    }
    pub fn imports_dir(&self) -> PathBuf {
        self.draft_dir.join("imports")
    }
    pub fn quarantine_dir(&self) -> PathBuf {
        self.imports_dir().join("quarantine")
    }
    pub fn exports_dir(&self) -> PathBuf {
        self.draft_dir.join("exports")
    }

    // ---- LSIF ------------------------------------------------------------

    pub fn lsif_dir(&self) -> PathBuf {
        self.draft_dir.join("lsif")
    }
    pub fn lsif_index_db(&self) -> PathBuf {
        self.lsif_dir().join("index.db")
    }
    pub fn lsif_symbols_db(&self) -> PathBuf {
        self.lsif_dir().join("symbols.db")
    }

    // ---- Cache & adapters ------------------------------------------------

    pub fn cache_dir(&self) -> PathBuf {
        self.draft_dir.join("cache")
    }
    pub fn cache_sub(&self, name: &str) -> PathBuf {
        self.cache_dir().join(name)
    }
    pub fn adapters_dir(&self) -> PathBuf {
        self.draft_dir.join("adapters")
    }
    pub fn adapter_overrides_dir(&self) -> PathBuf {
        self.adapters_dir().join("project-overrides")
    }

    /// Create the full v0.3.4 project tree and mark `.draft/` hidden.
    /// Idempotent; leaves existing files untouched.
    pub fn create_all(&self) -> DraftResult<HiddenStatus> {
        for dir in [
            self.draft_dir.clone(),
            self.objects_dir(),
            self.object_packs_dir(),
            self.events_dir(),
            self.receipts_dir(),
            self.transparency_dir(),
            self.stable_head_dir(),
            self.packs_dir(),
            self.snapshots_dir(),
            self.checkpoints_dir(),
            self.index_dir(),
            self.tmp_dir(),
            self.locks_dir(),
            self.tasks_dir(),
            self.executions_dir(),
            self.runtime_dir(),
            self.indexes_dir(),
            self.task_indexes_dir(),
            self.evidence_dir(),
            self.decisions_dir(),
            self.waivers_dir(),
            self.editor_dir(),
            self.recovery_dir(),
            self.backups_dir(),
            self.imports_dir(),
            self.quarantine_dir(),
            self.exports_dir(),
            self.lsif_dir(),
            self.cache_dir(),
            self.cache_sub("hashes"),
            self.cache_sub("risk"),
            self.cache_sub("verify"),
            self.cache_sub("test-selection"),
            self.cache_sub("fuzz-selection"),
            self.adapters_dir(),
            self.adapter_overrides_dir(),
        ] {
            ensure_dir(&dir)?;
        }
        Ok(hidden::ensure_hidden(&self.draft_dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_all_builds_canonical_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DraftLayout::for_root(tmp.path());
        assert!(p.create_all().unwrap().is_ok());
        assert!(p.events_dir().is_dir());
        assert!(p.transparency_dir().is_dir());
        assert!(p.quarantine_dir().is_dir());
        assert!(p.lsif_dir().is_dir());
        assert_eq!(
            p.pack_manifest("pck_abc"),
            p.draft_dir().join("packs/pck_abc/manifest.json")
        );
        assert_eq!(p.event_log(), p.draft_dir().join("events/event.log"));
    }
}
