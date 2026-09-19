//! Performance-ready indexes.
//!
//! See `docs/release-compliance.md` for the performance budgets these serve.
//!
//! Two small JSON artifacts under `.draft/index/`:
//! - the **affected-path index** maps each active ChangePack to the workspace paths
//!   it touches, so conflict detection can filter by path overlap instead of
//!   loading every ChangePack payload;
//! - the **verification cache manifest** associates deterministic
//!   [`crate::evidence::verification::VerificationKey`]s with verification results, so
//!   a result can be recognized as already-proven for an identical workspace,
//!   config, toolchain, command set, and environment.
//!
//! Both are maintained as evidence is recorded and promotions commit, and are
//! rebuilt or pruned by `draft maintenance gc`. Both are derived: losing either
//! loses no history.

use crate::project::layout::DraftLayout;
use crate::support::error::DraftResult;
use crate::support::fsutil;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Maximum number of verification cache entries retained (most recent first).
const VERIFICATION_CACHE_CAP: usize = 256;

/// ChangePack → affected project paths for every active (non-disposed) ChangePack.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AffectedPathIndex {
    pub schema_version: u32,
    pub changes: BTreeMap<String, Vec<String>>,
}

impl crate::contracts::VersionedContract for AffectedPathIndex {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::AffectedPathIndex;
}

impl Default for AffectedPathIndex {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::AffectedPathIndex,
            ),
            changes: BTreeMap::new(),
        }
    }
}

impl AffectedPathIndex {
    pub fn load(paths: &DraftLayout) -> DraftResult<Self> {
        let path = paths.affected_path_index();
        if path.exists() {
            crate::contracts::read_persisted(&path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, paths: &DraftLayout) -> DraftResult<()> {
        fsutil::write_json(&paths.affected_path_index(), self)
    }

    /// Record (or refresh) a ChangePack's affected paths.
    pub fn upsert(
        paths: &DraftLayout,
        change_pack_id: &str,
        affected: Vec<String>,
    ) -> DraftResult<()> {
        let mut index = Self::load(paths)?;
        index.changes.insert(change_pack_id.to_string(), affected);
        index.save(paths)
    }

    /// Drop a ChangePack from the index (on disposal).
    pub fn remove(paths: &DraftLayout, change_pack_id: &str) -> DraftResult<()> {
        let mut index = Self::load(paths)?;
        if index.changes.remove(change_pack_id).is_some() {
            index.save(paths)?;
        }
        Ok(())
    }

    /// ChangePacks whose affected paths overlap `candidate` paths.
    pub fn change_packs_touching(&self, candidate: &[String]) -> Vec<String> {
        let set: std::collections::BTreeSet<&str> = candidate.iter().map(String::as_str).collect();
        self.changes
            .iter()
            .filter(|(_, paths)| paths.iter().any(|p| set.contains(p.as_str())))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Rebuild the index from the lockfiles of every active ChangePack directory.
    /// Returns the number of changes indexed.
    pub fn rebuild(paths: &DraftLayout) -> DraftResult<usize> {
        let mut index = Self::default();
        if paths.change_packs_content_dir().exists() {
            for entry in std::fs::read_dir(paths.change_packs_content_dir())? {
                let dir = entry?.path();
                if !dir.is_dir() {
                    continue;
                }
                let lock_path = dir.join("change-pack.lock.json");
                let lock: crate::dcg::change_pack_store::ChangePackLockfile =
                    crate::contracts::read_persisted(&lock_path)?;
                index.changes.insert(
                    lock.change_pack_id.clone(),
                    lock.resource_state_digests.keys().cloned().collect(),
                );
            }
        }
        let count = index.changes.len();
        index.save(paths)?;
        Ok(count)
    }
}

/// One recorded verification result keyed by its deterministic key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationCacheEntry {
    pub verification_key: String,
    pub change_pack_id: String,
    pub result_hash: String,
    pub passed: bool,
    pub recorded_at: String,
}

/// Most-recent-first list of verification results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationCacheManifest {
    pub schema_version: u32,
    pub entries: Vec<VerificationCacheEntry>,
}

impl crate::contracts::VersionedContract for VerificationCacheManifest {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::VerificationCache;
}

impl Default for VerificationCacheManifest {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::VerificationCache,
            ),
            entries: Vec::new(),
        }
    }
}

impl VerificationCacheManifest {
    pub fn load(paths: &DraftLayout) -> DraftResult<Self> {
        let path = paths.verification_cache_manifest();
        if path.exists() {
            crate::contracts::read_persisted(&path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, paths: &DraftLayout) -> DraftResult<()> {
        fsutil::write_json(&paths.verification_cache_manifest(), self)
    }

    /// Record a verification result; the newest entry for a key wins and the
    /// manifest is capped at [`VERIFICATION_CACHE_CAP`] entries.
    pub fn record(paths: &DraftLayout, entry: VerificationCacheEntry) -> DraftResult<()> {
        let mut manifest = Self::load(paths)?;
        manifest
            .entries
            .retain(|e| e.verification_key != entry.verification_key);
        manifest.entries.insert(0, entry);
        manifest.entries.truncate(VERIFICATION_CACHE_CAP);
        manifest.save(paths)
    }

    /// Look up a previously recorded result for `key`.
    pub fn lookup(&self, key: &str) -> Option<&VerificationCacheEntry> {
        self.entries.iter().find(|e| e.verification_key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, DraftLayout) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = DraftLayout::for_root(tmp.path());
        paths.create_all().unwrap();
        (tmp, paths)
    }

    #[test]
    fn affected_path_index_upsert_query_remove() {
        let (_tmp, paths) = project();
        AffectedPathIndex::upsert(&paths, "cpk_a", vec!["src/lib.rs".into()]).unwrap();
        AffectedPathIndex::upsert(&paths, "cpk_b", vec!["src/main.rs".into()]).unwrap();
        let index = AffectedPathIndex::load(&paths).unwrap();
        assert_eq!(
            index.change_packs_touching(&["src/lib.rs".to_string()]),
            vec!["cpk_a".to_string()]
        );
        AffectedPathIndex::remove(&paths, "cpk_a").unwrap();
        let index = AffectedPathIndex::load(&paths).unwrap();
        assert!(index
            .change_packs_touching(&["src/lib.rs".to_string()])
            .is_empty());
        assert_eq!(index.changes.len(), 1);
    }

    #[test]
    fn verification_cache_records_and_replaces_by_key() {
        let (_tmp, paths) = project();
        let entry = |key: &str, passed: bool| VerificationCacheEntry {
            verification_key: key.to_string(),
            change_pack_id: "cpk_x".to_string(),
            result_hash: "sha256:r".to_string(),
            passed,
            recorded_at: "2026-01-01T00:00:00Z".to_string(),
        };
        VerificationCacheManifest::record(&paths, entry("k1", false)).unwrap();
        VerificationCacheManifest::record(&paths, entry("k1", true)).unwrap();
        VerificationCacheManifest::record(&paths, entry("k2", true)).unwrap();
        let manifest = VerificationCacheManifest::load(&paths).unwrap();
        assert_eq!(manifest.entries.len(), 2);
        assert!(manifest.lookup("k1").unwrap().passed);
        assert_eq!(manifest.entries[0].verification_key, "k2");
    }
}
