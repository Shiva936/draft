//! Project `.draft/` canonical v0.3.2 layout (PRD §9.1, TDD §7).
//!
//! This is the authoritative map of the project metadata store as specified for
//! v0.3.2. It coexists with the legacy [`crate::DraftLayout`] (which still owns
//! the content-addressed object/snapshot store); both point at the same
//! `<root>/.draft` directory. New trust artifacts — the event log, receipts,
//! the transparency chain, pack manifests/lockfiles, import quarantine, exports,
//! and the LSIF index — live at the exact paths defined here.

use crate::error::DraftResult;
use crate::fsutil::ensure_dir;
use crate::hidden::{self, HiddenStatus};
use std::path::{Path, PathBuf};

/// Handle to a project `.draft/` store and its canonical layout.
#[derive(Debug, Clone)]
pub struct ProjectPaths {
    draft_dir: PathBuf,
}

impl ProjectPaths {
    /// Build the layout for a project root (the directory that contains `.draft`).
    pub fn for_root(root: &Path) -> Self {
        ProjectPaths {
            draft_dir: root.join(".draft"),
        }
    }

    /// Build the layout given the `.draft` directory directly.
    pub fn at(draft_dir: impl Into<PathBuf>) -> Self {
        ProjectPaths {
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

    // ---- Packs -----------------------------------------------------------

    pub fn packs_dir(&self) -> PathBuf {
        self.draft_dir.join("packs")
    }
    pub fn pack_dir(&self, pack_id: &str) -> PathBuf {
        self.packs_dir().join(pack_id)
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

    /// Create the full v0.3.2 project tree and mark `.draft/` hidden.
    /// Idempotent; leaves existing files untouched.
    pub fn create_all(&self) -> DraftResult<HiddenStatus> {
        for dir in [
            self.draft_dir.clone(),
            self.events_dir(),
            self.receipts_dir(),
            self.transparency_dir(),
            self.packs_dir(),
            self.checkpoints_dir(),
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
        let p = ProjectPaths::for_root(tmp.path());
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
