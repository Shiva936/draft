//! Performance-ready indexes (SRS-FR-143, NFR-PF-002/003/004).
//!
//! Two small JSON artifacts under `.draft/index/`:
//! - the **affected-path index** maps each active pack to the workspace paths
//!   it touches, so conflict detection can filter by path overlap instead of
//!   loading every pack payload;
//! - the **verification cache manifest** associates deterministic
//!   [`crate::verification::VerificationKey`]s with verification results, so
//!   a result can be recognized as already-proven for an identical workspace,
//!   config, toolchain, command set, and environment.
//!
//! Both are maintained on save/verify and rebuilt or pruned by `draft gc`.

use crate::error::DraftResult;
use crate::fsutil;
use crate::layout::ProjectPaths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Maximum number of verification cache entries retained (most recent first).
const VERIFICATION_CACHE_CAP: usize = 256;

/// Pack → affected workspace paths for every active (non-disposed) pack.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AffectedPathIndex {
    #[serde(default)]
    pub packs: BTreeMap<String, Vec<String>>,
}

impl AffectedPathIndex {
    pub fn load(paths: &ProjectPaths) -> Self {
        fsutil::read_json(&paths.affected_path_index()).unwrap_or_default()
    }

    pub fn save(&self, paths: &ProjectPaths) -> DraftResult<()> {
        fsutil::write_json(&paths.affected_path_index(), self)
    }

    /// Record (or refresh) a pack's affected paths.
    pub fn upsert(paths: &ProjectPaths, pack_id: &str, affected: Vec<String>) -> DraftResult<()> {
        let mut index = Self::load(paths);
        index.packs.insert(pack_id.to_string(), affected);
        index.save(paths)
    }

    /// Drop a pack from the index (on disposal).
    pub fn remove(paths: &ProjectPaths, pack_id: &str) -> DraftResult<()> {
        let mut index = Self::load(paths);
        if index.packs.remove(pack_id).is_some() {
            index.save(paths)?;
        }
        Ok(())
    }

    /// Packs whose affected paths overlap `candidate` paths.
    pub fn packs_touching(&self, candidate: &[String]) -> Vec<String> {
        let set: std::collections::BTreeSet<&str> = candidate.iter().map(String::as_str).collect();
        self.packs
            .iter()
            .filter(|(_, paths)| paths.iter().any(|p| set.contains(p.as_str())))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Rebuild the index from the lockfiles of every active pack directory.
    /// Returns the number of packs indexed.
    pub fn rebuild(paths: &ProjectPaths) -> DraftResult<usize> {
        let mut index = Self::default();
        if paths.packs_dir().exists() {
            for entry in std::fs::read_dir(paths.packs_dir())? {
                let dir = entry?.path();
                if !dir.is_dir() {
                    continue;
                }
                let lock_path = dir.join("pack.lock.json");
                let Ok(lock) = fsutil::read_json::<crate::pack::PackLockfile>(&lock_path) else {
                    continue;
                };
                index.packs.insert(
                    lock.pack_id.clone(),
                    lock.file_hashes.keys().cloned().collect(),
                );
            }
        }
        let count = index.packs.len();
        index.save(paths)?;
        Ok(count)
    }
}

/// One recorded verification result keyed by its deterministic key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationCacheEntry {
    pub verification_key: String,
    pub pack_id: String,
    pub result_hash: String,
    pub passed: bool,
    pub recorded_at: String,
}

/// Most-recent-first list of verification results (SRS-FR-144).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VerificationCacheManifest {
    #[serde(default)]
    pub entries: Vec<VerificationCacheEntry>,
}

impl VerificationCacheManifest {
    pub fn load(paths: &ProjectPaths) -> Self {
        fsutil::read_json(&paths.verification_cache_manifest()).unwrap_or_default()
    }

    pub fn save(&self, paths: &ProjectPaths) -> DraftResult<()> {
        fsutil::write_json(&paths.verification_cache_manifest(), self)
    }

    /// Record a verification result; the newest entry for a key wins and the
    /// manifest is capped at [`VERIFICATION_CACHE_CAP`] entries.
    pub fn record(paths: &ProjectPaths, entry: VerificationCacheEntry) -> DraftResult<()> {
        let mut manifest = Self::load(paths);
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

    fn project() -> (tempfile::TempDir, ProjectPaths) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::for_root(tmp.path());
        paths.create_all().unwrap();
        (tmp, paths)
    }

    #[test]
    fn affected_path_index_upsert_query_remove() {
        let (_tmp, paths) = project();
        AffectedPathIndex::upsert(&paths, "pck_a", vec!["src/lib.rs".into()]).unwrap();
        AffectedPathIndex::upsert(&paths, "pck_b", vec!["src/main.rs".into()]).unwrap();
        let index = AffectedPathIndex::load(&paths);
        assert_eq!(
            index.packs_touching(&["src/lib.rs".to_string()]),
            vec!["pck_a".to_string()]
        );
        AffectedPathIndex::remove(&paths, "pck_a").unwrap();
        let index = AffectedPathIndex::load(&paths);
        assert!(index.packs_touching(&["src/lib.rs".to_string()]).is_empty());
        assert_eq!(index.packs.len(), 1);
    }

    #[test]
    fn verification_cache_records_and_replaces_by_key() {
        let (_tmp, paths) = project();
        let entry = |key: &str, passed: bool| VerificationCacheEntry {
            verification_key: key.to_string(),
            pack_id: "pck_x".to_string(),
            result_hash: "sha256:r".to_string(),
            passed,
            recorded_at: "2026-01-01T00:00:00Z".to_string(),
        };
        VerificationCacheManifest::record(&paths, entry("k1", false)).unwrap();
        VerificationCacheManifest::record(&paths, entry("k1", true)).unwrap();
        VerificationCacheManifest::record(&paths, entry("k2", true)).unwrap();
        let manifest = VerificationCacheManifest::load(&paths);
        assert_eq!(manifest.entries.len(), 2);
        assert!(manifest.lookup("k1").unwrap().passed);
        assert_eq!(manifest.entries[0].verification_key, "k2");
    }
}
