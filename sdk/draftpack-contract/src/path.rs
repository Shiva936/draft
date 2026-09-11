//! The safe relative-path grammar for archive entries.
//!
//! An archive is **untrusted input**. Every name inside one is a potential path
//! traversal, an absolute write, an attempt to overwrite Draft's own state, or
//! a name that means one thing to the validator and another to the filesystem.
//!
//! The grammar is enforced here, in the portable crate, for two reasons: a
//! publisher can check an archive without Draft, and — more importantly — the
//! *same* rule is applied on the write path as on the read path. An exporter
//! that could emit a name the importer would refuse is a bug that only shows up
//! at the recipient.
//!
//! What is refused, and why:
//!
//! | Refused | Because |
//! |---|---|
//! | absolute paths, Windows prefixes | they escape the extraction root outright |
//! | any `..` component | traversal, including via a legitimate-looking prefix |
//! | a leading `.draft/` component | an archive must never write Draft's own state |
//! | `\` as a separator | it is a path separator on Windows and a filename character elsewhere, so one name would mean two things |
//! | control characters, including NUL | they make a name display differently from how it is used |
//! | empty or `.`/`..` components | they normalize away, so two distinct names could collide |
//! | over-long paths | an unbounded name is a resource-exhaustion vector |

use serde::{Deserialize, Serialize};

use crate::{FormatError, FormatResult};

/// Longest an entry path may be, in bytes.
pub const MAX_ENTRY_PATH_LENGTH: usize = 1024;

/// The directory an archive may never write into.
pub const RESERVED_DIRECTORY: &str = ".draft";

/// A validated, forward-slash, relative archive entry path.
///
/// The only way to construct one is [`SafeEntryPath::parse`], so possessing a
/// value of this type *is* the proof that the grammar was applied.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SafeEntryPath(String);

impl SafeEntryPath {
    pub fn parse(value: impl Into<String>) -> FormatResult<Self> {
        let value = value.into();

        if value.is_empty() {
            return Err(FormatError::UnsafePath("entry path is empty".into()));
        }
        if value.len() > MAX_ENTRY_PATH_LENGTH {
            return Err(FormatError::UnsafePath(format!(
                "entry path exceeds {MAX_ENTRY_PATH_LENGTH} bytes"
            )));
        }
        if let Some(character) = value.chars().find(|c| c.is_control()) {
            return Err(FormatError::UnsafePath(format!(
                "entry path contains the control character U+{:04X}",
                character as u32
            )));
        }
        if value.contains('\\') {
            return Err(FormatError::UnsafePath(format!(
                "entry path '{value}' contains a backslash, which is a separator on one platform \
                 and a filename character on another"
            )));
        }
        if value.starts_with('/') {
            return Err(FormatError::UnsafePath(format!(
                "entry path '{value}' is absolute"
            )));
        }
        // `C:` and friends. Checked explicitly because an importer on Windows
        // would otherwise treat the name as rooted.
        let bytes = value.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return Err(FormatError::UnsafePath(format!(
                "entry path '{value}' carries a drive prefix"
            )));
        }

        let components: Vec<&str> = value.split('/').collect();
        for component in &components {
            match *component {
                "" => {
                    return Err(FormatError::UnsafePath(format!(
                        "entry path '{value}' has an empty component; two spellings of one path \
                         would collide"
                    )))
                }
                "." | ".." => {
                    return Err(FormatError::UnsafePath(format!(
                        "entry path '{value}' contains a '{component}' component"
                    )))
                }
                _ => {}
            }
        }
        if components[0] == RESERVED_DIRECTORY {
            return Err(FormatError::UnsafePath(format!(
                "entry path '{value}' writes into {RESERVED_DIRECTORY}/"
            )));
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SafeEntryPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for SafeEntryPath {
    type Error = FormatError;

    fn try_from(value: String) -> FormatResult<Self> {
        Self::parse(value)
    }
}

impl From<SafeEntryPath> for String {
    fn from(value: SafeEntryPath) -> String {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_relative_paths_are_accepted() {
        for good in [
            "draftpack.json",
            "objects/blake3/ab/cdef",
            "a/b/c/d.txt",
            "名前.txt",
            ".hidden/file",
        ] {
            SafeEntryPath::parse(good).unwrap_or_else(|error| panic!("{good}: {error}"));
        }
    }

    #[test]
    fn traversal_is_refused_however_it_is_spelled() {
        for attack in [
            "../etc/passwd",
            "a/../../etc/passwd",
            "a/./b",
            "..",
            ".",
            "a/..",
        ] {
            assert!(
                matches!(
                    SafeEntryPath::parse(attack),
                    Err(FormatError::UnsafePath(_))
                ),
                "{attack} was accepted"
            );
        }
    }

    #[test]
    fn a_name_that_merely_starts_with_dots_is_not_traversal() {
        // `..foo` is an ordinary filename; refusing it would be wrong.
        SafeEntryPath::parse("..foo/bar").unwrap();
        SafeEntryPath::parse("a/.config").unwrap();
    }

    #[test]
    fn absolute_and_drive_rooted_paths_are_refused() {
        for attack in ["/etc/passwd", "C:/Windows/system32", "c:file"] {
            assert!(
                SafeEntryPath::parse(attack).is_err(),
                "{attack} was accepted"
            );
        }
    }

    #[test]
    fn an_archive_may_never_write_draft_state() {
        assert!(SafeEntryPath::parse(".draft/control.json").is_err());
        // A directory that merely starts with the same letters is fine.
        SafeEntryPath::parse(".draftpack-notes/readme").unwrap();
        // Nested is fine too: only the first component is reserved.
        SafeEntryPath::parse("exports/.draft/file").unwrap();
    }

    #[test]
    fn a_backslash_is_refused_because_it_means_two_things() {
        assert!(SafeEntryPath::parse("a\\b").is_err());
        assert!(SafeEntryPath::parse("..\\..\\etc").is_err());
    }

    #[test]
    fn control_characters_and_empty_components_are_refused() {
        assert!(SafeEntryPath::parse("a\u{0}b").is_err());
        assert!(SafeEntryPath::parse("a\nb").is_err());
        assert!(SafeEntryPath::parse("a//b").is_err());
        assert!(SafeEntryPath::parse("").is_err());
        assert!(SafeEntryPath::parse("a/").is_err());
    }

    #[test]
    fn paths_are_bounded() {
        assert!(SafeEntryPath::parse("a".repeat(MAX_ENTRY_PATH_LENGTH)).is_ok());
        assert!(SafeEntryPath::parse("a".repeat(MAX_ENTRY_PATH_LENGTH + 1)).is_err());
    }

    #[test]
    fn the_wire_form_validates_on_the_way_in() {
        let path = SafeEntryPath::parse("objects/a").unwrap();
        let encoded = serde_json::to_string(&path).unwrap();
        assert_eq!(
            serde_json::from_str::<SafeEntryPath>(&encoded).unwrap(),
            path
        );
        // Deserializing is a construction, so it is validated too — otherwise
        // the type's guarantee would have a hole exactly where untrusted input
        // enters.
        assert!(serde_json::from_str::<SafeEntryPath>("\"../escape\"").is_err());
    }
}
