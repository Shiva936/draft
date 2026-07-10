//! Deterministic hashing primitives (TDD §28).
//!
//! Two things live here:
//! - [`canonical_json`] / [`sha256_hex`] — the canonical serialization used to
//!   hash events, receipts, and transparency entries so a hash is stable across
//!   machines and serde versions (object keys sorted, no insignificant
//!   whitespace).
//! - [`workspace_hash`] — a deterministic digest of the project's content that
//!   **excludes** `.draft/` (via the central path guard), normalizes path
//!   separators, and sorts entries. It underpins pack manifests/lockfiles,
//!   verification cache keys, receipts, and the strict save gate.

use crate::error::DraftResult;
use crate::pathguard;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Hex-encode a SHA-256 digest of `bytes`, prefixed `sha256:`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(7 + 64);
    s.push_str("sha256:");
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Serialize a JSON value canonically: object keys sorted recursively, arrays
/// preserved in order, compact separators. Two structurally equal values always
/// produce byte-identical output.
pub fn canonical_json(value: &serde_json::Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

/// Convenience: canonicalize `value` then hash it.
pub fn canonical_hash<T: serde::Serialize>(value: &T) -> String {
    let v = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
    sha256_hex(canonical_json(&v).as_bytes())
}

fn write_canonical(value: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_json_string(s, out),
        Value::Array(arr) => {
            out.push('[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(v, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_string(k, out);
                out.push(':');
                write_canonical(&map[*k], out);
            }
            out.push('}');
        }
    }
}

fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Canonical hash of the project's Draft configuration (SRS-FR-096/097).
/// `config.toml` and `policy.toml` are parsed to values before hashing so
/// formatting and comments never affect the digest; a missing file hashes as
/// null.
pub fn config_hash(paths: &crate::layout::ProjectPaths) -> String {
    let read = |p: &Path| -> serde_json::Value {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
            .and_then(|v| serde_json::to_value(v).ok())
            .unwrap_or(serde_json::Value::Null)
    };
    canonical_hash(&serde_json::json!({
        "config": read(&paths.config_toml()),
        "policy": read(&paths.policy_toml()),
    }))
}

/// A single file's contribution to the workspace hash.
struct FileEntry {
    rel: String,
    content_hash: String,
}

/// Compute a deterministic hash over the project's content rooted at `root`,
/// excluding `.draft/` and honoring the same ignore rules the scanner uses.
/// The digest is over `sha256:<hex>` lines of `path\0content_hash`, sorted by
/// path, so it is stable regardless of filesystem traversal order.
pub fn workspace_hash(root: &Path) -> DraftResult<String> {
    workspace_hash_inner(root, None)
}

/// Per-file content-hash cache entry keyed by size and mtime (NFR-PF-001).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CachedFileHash {
    size: u64,
    mtime_ns: u128,
    hash: String,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct WorkspaceHashCache {
    #[serde(default)]
    files: std::collections::BTreeMap<String, CachedFileHash>,
}

/// [`workspace_hash`] with a changed-file cache: produces the identical digest
/// but reuses per-file content hashes whose (size, mtime) are unchanged, so
/// repeated hashing of a large, mostly-unchanged tree avoids full re-reads.
/// The cache lives under `.draft/cache/` (excluded from the walk) and cache
/// read/write failures degrade to full hashing.
pub fn workspace_hash_cached(root: &Path, cache_file: &Path) -> DraftResult<String> {
    workspace_hash_inner(root, Some(cache_file))
}

fn workspace_hash_inner(root: &Path, cache_file: Option<&Path>) -> DraftResult<String> {
    let mut cache = cache_file
        .map(|p| crate::fsutil::read_json::<WorkspaceHashCache>(p).unwrap_or_default())
        .unwrap_or_default();
    let mut next_cache = WorkspaceHashCache::default();
    let mut entries: Vec<FileEntry> = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .git_exclude(false)
        .parents(false)
        .build();
    for dent in walker.flatten() {
        let path = dent.path();
        if path.is_dir() {
            continue;
        }
        // Hard-exclude `.draft/` (both stores) and anything under it.
        if pathguard::path_is_draft(path) {
            continue;
        }
        let rel = match path.strip_prefix(root) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if rel.is_empty() {
            continue;
        }
        let stat = std::fs::metadata(path).ok().and_then(|m| {
            let mtime_ns = m
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos();
            Some((m.len(), mtime_ns))
        });
        let content_hash = match (cache_file.is_some(), stat, cache.files.remove(&rel)) {
            (true, Some((size, mtime_ns)), Some(hit))
                if hit.size == size && hit.mtime_ns == mtime_ns =>
            {
                hit.hash
            }
            _ => {
                let bytes = std::fs::read(path).unwrap_or_default();
                sha256_hex(&bytes)
            }
        };
        if let (true, Some((size, mtime_ns))) = (cache_file.is_some(), stat) {
            next_cache.files.insert(
                rel.clone(),
                CachedFileHash {
                    size,
                    mtime_ns,
                    hash: content_hash.clone(),
                },
            );
        }
        entries.push(FileEntry { rel, content_hash });
    }
    if let Some(cache_path) = cache_file {
        // Best effort: a failed cache write never fails the hash.
        let _ = crate::fsutil::write_json(cache_path, &next_cache);
    }
    entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    let mut hasher = Sha256::new();
    for e in &entries {
        hasher.update(e.rel.as_bytes());
        hasher.update([0u8]);
        hasher.update(e.content_hash.as_bytes());
        hasher.update(*b"\n");
    }
    let digest = hasher.finalize();
    let mut s = String::from("sha256:");
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_keys() {
        let a = json!({"b": 1, "a": 2, "nested": {"z": 1, "y": 2}});
        assert_eq!(
            canonical_json(&a),
            r#"{"a":2,"b":1,"nested":{"y":2,"z":1}}"#
        );
    }

    #[test]
    fn canonical_json_is_order_independent() {
        let a = json!({"x": 1, "y": [1, 2, 3]});
        let b = json!({"y": [1, 2, 3], "x": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
    }

    #[test]
    fn cached_workspace_hash_matches_full_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("a.txt"), "hello").unwrap();
        std::fs::write(root.join("b.txt"), "there").unwrap();
        std::fs::create_dir_all(root.join(".draft/cache/hashes")).unwrap();
        let cache = root.join(".draft/cache/hashes/workspace-hash.json");
        let full = workspace_hash(root).unwrap();
        // Cold cache, warm cache, and full hash must all agree.
        assert_eq!(workspace_hash_cached(root, &cache).unwrap(), full);
        assert!(cache.exists());
        assert_eq!(workspace_hash_cached(root, &cache).unwrap(), full);
        // A content change (different size) is picked up through the cache.
        std::fs::write(root.join("a.txt"), "hello world").unwrap();
        assert_eq!(
            workspace_hash_cached(root, &cache).unwrap(),
            workspace_hash(root).unwrap()
        );
    }

    #[test]
    fn config_hash_ignores_formatting_and_detects_change() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::layout::ProjectPaths::for_root(tmp.path());
        std::fs::create_dir_all(paths.draft_dir()).unwrap();
        std::fs::write(
            paths.config_toml(),
            "[save]\npack_disposal = \"merge_and_dispose\"\n",
        )
        .unwrap();
        let h1 = config_hash(&paths);
        // Reformatting (comments/whitespace) must not change the hash.
        std::fs::write(
            paths.config_toml(),
            "# comment\n[save]\n\npack_disposal   =   \"merge_and_dispose\"\n",
        )
        .unwrap();
        assert_eq!(h1, config_hash(&paths));
        // A value change must.
        std::fs::write(
            paths.config_toml(),
            "[save]\npack_disposal = \"dispose_only\"\n",
        )
        .unwrap();
        assert_ne!(h1, config_hash(&paths));
    }

    #[test]
    fn workspace_hash_excludes_draft_and_is_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("a.txt"), "hello").unwrap();
        std::fs::create_dir_all(root.join(".draft/packs")).unwrap();
        std::fs::write(root.join(".draft/packs/secret"), "should be ignored").unwrap();
        let h1 = workspace_hash(root).unwrap();
        // Mutating .draft/ must not change the hash.
        std::fs::write(root.join(".draft/packs/secret"), "changed").unwrap();
        let h2 = workspace_hash(root).unwrap();
        assert_eq!(h1, h2);
        // Mutating content must change it.
        std::fs::write(root.join("a.txt"), "world").unwrap();
        assert_ne!(h1, workspace_hash(root).unwrap());
    }
}
