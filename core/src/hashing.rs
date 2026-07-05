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
        let bytes = std::fs::read(path).unwrap_or_default();
        entries.push(FileEntry {
            rel,
            content_hash: sha256_hex(&bytes),
        });
    }
    entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    let mut hasher = Sha256::new();
    for e in &entries {
        hasher.update(e.rel.as_bytes());
        hasher.update([0u8]);
        hasher.update(e.content_hash.as_bytes());
        hasher.update([b'\n']);
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
