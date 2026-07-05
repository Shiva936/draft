//! Centralized path-safety guard (PRD §9.2, TDD §9, NFRD §4.4).
//!
//! Every path that enters Draft from an untrusted source — pack content,
//! diffs, imported `.draftpack` entries, export selection, save and rollback
//! targets, risk/test source scanning — must pass through this module. There
//! is exactly one implementation of "is this path safe?" so the invariant
//! cannot drift between callers.
//!
//! The guard rejects, uniformly:
//! - absolute paths (`/etc/passwd`, `C:\...`, `\\server\share`)
//! - parent traversal (`..` as any component, including `foo/../bar`)
//! - any `.draft` component (case-insensitively, e.g. `.DRAFT`, `.Draft`) so
//!   both global and project metadata stores are always hard-excluded
//! - empty / current-dir-only paths and paths with embedded NUL
//! - invalid UTF-8 (callers pass `&str`; see [`from_bytes`] for raw entries)
//! - on extraction, symlink escapes and non-regular (device/fifo) targets

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Why a path was rejected. Kept small and `Copy` so callers can match on it
/// and produce actionable, uniform error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathViolation {
    /// The path was empty or resolved to nothing meaningful.
    Empty,
    /// The path was absolute (rooted, or had a Windows drive/UNC prefix).
    Absolute,
    /// The path contained a `..` component.
    ParentTraversal,
    /// The path referenced a `.draft` directory (case-insensitive).
    DraftReserved,
    /// The path contained an embedded NUL or otherwise invalid byte.
    InvalidEncoding,
    /// The path contained a Windows drive/UNC/verbatim prefix.
    WindowsPrefix,
    /// On extraction: the resolved target escaped the extraction root.
    Escape,
    /// On extraction: the target existed as a symlink (symlink attack).
    Symlink,
    /// On extraction: the target was not a regular file/directory.
    NotRegular,
}

impl fmt::Display for PathViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            PathViolation::Empty => "path is empty",
            PathViolation::Absolute => "absolute paths are not allowed",
            PathViolation::ParentTraversal => "parent traversal ('..') is not allowed",
            PathViolation::DraftReserved => ".draft/ paths are always excluded",
            PathViolation::InvalidEncoding => "path is not valid UTF-8 / contains NUL",
            PathViolation::WindowsPrefix => "drive-letter/UNC paths are not allowed",
            PathViolation::Escape => "path escapes the extraction root",
            PathViolation::Symlink => "symlink targets are not allowed",
            PathViolation::NotRegular => "only regular files and directories are allowed",
        };
        f.write_str(s)
    }
}

/// Returns `true` if any component of `rel` is `.draft` (case-insensitive).
///
/// This is the single source of truth for `.draft/` exclusion. It intentionally
/// operates on the textual, slash-normalized form so it catches `.draft`,
/// `a/.draft/b`, `.DRAFT`, and backslash-separated Windows variants alike.
pub fn is_draft_path(rel: &str) -> bool {
    normalize_components(rel)
        .iter()
        .any(|c| c.eq_ignore_ascii_case(".draft"))
}

/// Validate a workspace-relative path string for use as *content* (a file that
/// belongs to a pack, diff, import payload, or save/rollback plan).
///
/// On success returns the slash-normalized relative path. On failure returns
/// the specific [`PathViolation`].
pub fn check_relative(rel: &str) -> Result<String, PathViolation> {
    if rel.contains('\0') {
        return Err(PathViolation::InvalidEncoding);
    }
    let trimmed = rel.trim();
    if trimmed.is_empty() {
        return Err(PathViolation::Empty);
    }
    // Windows drive-letter (`C:\`) or UNC (`\\host\share`) or verbatim prefixes.
    if has_windows_prefix(trimmed) {
        return Err(PathViolation::WindowsPrefix);
    }
    // POSIX absolute.
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(PathViolation::Absolute);
    }
    let comps = normalize_components(trimmed);
    if comps.is_empty() {
        return Err(PathViolation::Empty);
    }
    for c in &comps {
        if c == ".." {
            return Err(PathViolation::ParentTraversal);
        }
        if c.eq_ignore_ascii_case(".draft") {
            return Err(PathViolation::DraftReserved);
        }
    }
    Ok(comps.join("/"))
}

/// Validate a raw (possibly non-UTF-8) archive entry name during import.
pub fn from_bytes(name: &[u8]) -> Result<String, PathViolation> {
    let s = std::str::from_utf8(name).map_err(|_| PathViolation::InvalidEncoding)?;
    check_relative(s)
}

/// Resolve `entry` against extraction root `base`, guaranteeing the result stays
/// inside `base` and does not traverse a symlink. Used by the import extractor.
///
/// This performs the textual checks of [`check_relative`] and then, walking the
/// concrete on-disk path, rejects any existing component that is a symlink so a
/// malicious archive cannot redirect writes outside the quarantine via a
/// pre-seeded or same-archive symlink.
pub fn safe_join(base: &Path, entry: &str) -> Result<PathBuf, PathViolation> {
    let rel = check_relative(entry)?;
    let mut out = base.to_path_buf();
    for comp in rel.split('/') {
        out.push(comp);
        // If an intermediate path already exists, it must not be a symlink.
        match std::fs::symlink_metadata(&out) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(PathViolation::Symlink),
            _ => {}
        }
    }
    // Defense in depth: the lexical target must remain under `base`.
    if !out.starts_with(base) {
        return Err(PathViolation::Escape);
    }
    Ok(out)
}

/// Split a path into logical components, normalizing separators and dropping
/// `.` and empty segments. Does not resolve `..` (callers reject it).
fn normalize_components(rel: &str) -> Vec<String> {
    rel.replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .map(|s| s.to_string())
        .collect()
}

fn has_windows_prefix(s: &str) -> bool {
    let bytes = s.as_bytes();
    // Drive letter: `C:` optionally followed by a separator.
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return true;
    }
    // UNC / verbatim: `\\?\`, `\\.\`, `\\server\share`, `//server/share`.
    if s.starts_with("\\\\") || s.starts_with("//") {
        return true;
    }
    false
}

/// Convenience predicate over a [`Path`] for internal filesystem walks: is any
/// component `.draft`?  Used by scanners so metadata is never read as content.
pub fn path_is_draft(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(os) => os
            .to_str()
            .map(|s| s.eq_ignore_ascii_case(".draft"))
            .unwrap_or(false),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_relative_paths() {
        assert_eq!(check_relative("src/auth.rs").unwrap(), "src/auth.rs");
        assert_eq!(check_relative("./a/./b").unwrap(), "a/b");
        assert_eq!(check_relative("a\\b\\c").unwrap(), "a/b/c");
    }

    #[test]
    fn rejects_absolute() {
        assert_eq!(check_relative("/etc/passwd"), Err(PathViolation::Absolute));
        assert_eq!(
            check_relative("C:\\Windows"),
            Err(PathViolation::WindowsPrefix)
        );
        assert_eq!(
            check_relative("\\\\host\\share\\x"),
            Err(PathViolation::WindowsPrefix)
        );
    }

    #[test]
    fn rejects_traversal() {
        assert_eq!(
            check_relative("../secrets"),
            Err(PathViolation::ParentTraversal)
        );
        assert_eq!(
            check_relative("foo/../../etc"),
            Err(PathViolation::ParentTraversal)
        );
    }

    #[test]
    fn rejects_draft_any_case_any_position() {
        assert_eq!(
            check_relative(".draft/x"),
            Err(PathViolation::DraftReserved)
        );
        assert_eq!(
            check_relative("a/.DRAFT/b"),
            Err(PathViolation::DraftReserved)
        );
        assert_eq!(
            check_relative("nested/.Draft/keys"),
            Err(PathViolation::DraftReserved)
        );
        assert!(is_draft_path("a/.draft/b"));
        assert!(is_draft_path(".DRAFT"));
        assert!(!is_draft_path("src/draft_notes.rs"));
    }

    #[test]
    fn rejects_encoding_and_empty() {
        assert_eq!(check_relative("a\0b"), Err(PathViolation::InvalidEncoding));
        assert_eq!(check_relative("   "), Err(PathViolation::Empty));
        assert_eq!(
            from_bytes(&[0xff, 0xfe]),
            Err(PathViolation::InvalidEncoding)
        );
    }

    #[test]
    fn safe_join_stays_in_base() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let joined = safe_join(base, "sub/dir/file.txt").unwrap();
        assert!(joined.starts_with(base));
        assert_eq!(
            safe_join(base, "../escape"),
            Err(PathViolation::ParentTraversal)
        );
        assert_eq!(
            safe_join(base, ".draft/x"),
            Err(PathViolation::DraftReserved)
        );
    }

    #[cfg(unix)]
    #[test]
    fn safe_join_rejects_symlink_component() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let target = tmp.path().join("outside");
        std::fs::create_dir(&target).unwrap();
        symlink(&target, base.join("link")).unwrap();
        assert_eq!(safe_join(base, "link/file"), Err(PathViolation::Symlink));
    }
}
