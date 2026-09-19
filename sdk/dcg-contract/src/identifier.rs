//! Canonical rules for every identifier that participates in hashing or lookup.
//!
//! Two distinct families live here, and they are deliberately not interchangeable:
//!
//! * **Contributed identifiers** are owned by an extension publisher. They are
//!   namespaced by the owning extension identity, so two publishers can neither
//!   collide nor squat one another's names.
//! * **Runtime-scoped identifiers** are Core-owned or adapter-local. They are
//!   meaningful only inside the scope that issued them — an adapter's local
//!   coverage-domain id means nothing outside its binding — and are never forced
//!   into a package namespace.
//!
//! Both families share one canonical-form rule set, because both end up inside a
//! canonical hash or a lookup key: a bounded length, a restricted character set,
//! no control characters, explicit case sensitivity (identifiers are compared
//! byte-for-byte, never case-folded), no Unicode-normalisation ambiguity, and a
//! deterministic lexical ordering. Neither family gives `/` or `.` any
//! path-traversal meaning; a specific contract may define its own structure, and
//! only that contract's parser interprets it.

use crate::{FormatError, FormatResult};
use serde::{Deserialize, Serialize};

/// Longest any canonical identifier segment may be.
pub const MAX_SEGMENT_LENGTH: usize = 64;
/// Longest a fully qualified namespaced identifier may be, including the
/// separator.
pub const MAX_QUALIFIED_LENGTH: usize = 160;

/// Which character set an identifier is held to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierClass {
    /// Authority-sensitive: lowercase ASCII, digits, `-`, `.`. Used wherever an
    /// identifier participates in a trust, authorization or namespace decision.
    Restricted,
    /// Runtime-scoped values an adapter or Core mints. Printable ASCII without
    /// whitespace, still bounded and still byte-compared.
    Scoped,
}

impl IdentifierClass {
    fn admits(self, character: char) -> bool {
        match self {
            Self::Restricted => {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '-' | '.')
            }
            Self::Scoped => {
                character.is_ascii_graphic() && !matches!(character, '\\' | '"' | '\'' | '`')
            }
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Restricted => "lowercase ASCII, digits, '-' or '.'",
            Self::Scoped => "printable ASCII without whitespace or quoting characters",
        }
    }
}

/// Validate one identifier segment against the canonical-form rules.
pub fn validate_segment(value: &str, class: IdentifierClass, what: &str) -> FormatResult<()> {
    if value.is_empty() {
        return Err(FormatError::Identity(format!("{what} must not be empty")));
    }
    if value.len() > MAX_SEGMENT_LENGTH {
        return Err(FormatError::Identity(format!(
            "{what} '{value}' exceeds {MAX_SEGMENT_LENGTH} bytes"
        )));
    }
    // ASCII-only by construction, so there is no Unicode normalisation form in
    // which two distinct byte strings could compare equal.
    if !value.is_ascii() {
        return Err(FormatError::Identity(format!(
            "{what} '{value}' must be ASCII so its canonical form is unambiguous"
        )));
    }
    if let Some(character) = value.chars().find(|c| !class.admits(*c)) {
        return Err(FormatError::Identity(format!(
            "{what} '{value}' contains '{character}'; it must be {}",
            class.describe()
        )));
    }
    Ok(())
}

/// An identifier owned by a contributing extension, qualified by the namespace
/// that owns it.
///
/// The wire form is `<namespace>/<id>`. `namespace` is an extension id, so a
/// publisher can only mint identifiers under a name it already owns; validation
/// against the declaring manifest is what enforces that at install time.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NamespacedId {
    namespace: String,
    id: String,
}

impl NamespacedId {
    pub fn new(namespace: impl Into<String>, id: impl Into<String>) -> FormatResult<Self> {
        let namespace = namespace.into();
        let id = id.into();
        validate_segment(
            &namespace,
            IdentifierClass::Restricted,
            "identifier namespace",
        )?;
        validate_segment(&id, IdentifierClass::Restricted, "identifier")?;
        if namespace.len() + id.len() + 1 > MAX_QUALIFIED_LENGTH {
            return Err(FormatError::Identity(format!(
                "qualified identifier '{namespace}/{id}' exceeds {MAX_QUALIFIED_LENGTH} bytes"
            )));
        }
        Ok(Self { namespace, id })
    }

    /// Parse the `<namespace>/<id>` wire form.
    ///
    /// Exactly one separator is permitted. A value with none is not "in the
    /// default namespace" — it is unowned, and refused, because an unowned
    /// identifier is exactly what lets two publishers collide.
    pub fn parse(value: &str) -> FormatResult<Self> {
        let mut parts = value.splitn(2, '/');
        let namespace = parts.next().unwrap_or_default();
        let Some(id) = parts.next() else {
            return Err(FormatError::Identity(format!(
                "contributed identifier '{value}' must be namespaced as '<owner>/<id>'"
            )));
        };
        if id.contains('/') {
            return Err(FormatError::Identity(format!(
                "contributed identifier '{value}' must contain exactly one '/'"
            )));
        }
        Self::new(namespace, id)
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Whether this identifier is owned by `extension`.
    pub fn is_owned_by(&self, extension: &str) -> bool {
        self.namespace == extension
    }

    pub fn qualified(&self) -> String {
        format!("{}/{}", self.namespace, self.id)
    }
}

impl std::fmt::Display for NamespacedId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.namespace, self.id)
    }
}

impl TryFrom<String> for NamespacedId {
    type Error = FormatError;

    fn try_from(value: String) -> FormatResult<Self> {
        Self::parse(&value)
    }
}

impl From<NamespacedId> for String {
    fn from(value: NamespacedId) -> String {
        value.qualified()
    }
}

/// A runtime-scoped identifier: Core-owned or adapter-local, meaningful only
/// within the scope that issued it, and never package-namespaced.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ScopedId(String);

impl ScopedId {
    pub fn parse(value: impl Into<String>) -> FormatResult<Self> {
        let value = value.into();
        validate_segment(&value, IdentifierClass::Scoped, "scoped identifier")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ScopedId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for ScopedId {
    type Error = FormatError;

    fn try_from(value: String) -> FormatResult<Self> {
        Self::parse(value)
    }
}

impl From<ScopedId> for String {
    fn from(value: ScopedId) -> String {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contributed_identifier_must_name_its_owner() {
        let owned = NamespacedId::parse("draft.text.document/document").unwrap();
        assert_eq!(owned.namespace(), "draft.text.document");
        assert_eq!(owned.id(), "document");
        assert!(owned.is_owned_by("draft.text.document"));
        // A prefix is not ownership: `draft.text` does not own what
        // `draft.text.document` mints, however similar the names look.
        assert!(!owned.is_owned_by("draft.text"));
        assert!(!owned.is_owned_by("draft.language.rust"));

        // An unowned identifier is refused rather than silently placed in a
        // shared namespace, which is what would let two publishers collide.
        assert!(matches!(
            NamespacedId::parse("document"),
            Err(FormatError::Identity(_))
        ));
        assert!(matches!(
            NamespacedId::parse("a/b/c"),
            Err(FormatError::Identity(_))
        ));
    }

    #[test]
    fn identifiers_are_byte_compared_and_never_case_folded() {
        // Uppercase is refused outright for authority-sensitive identifiers, so
        // there is no pair of values that differ only by case.
        assert!(NamespacedId::parse("draft.text.document/Document").is_err());
        assert!(validate_segment("Rust", IdentifierClass::Restricted, "x").is_err());
    }

    #[test]
    fn non_ascii_is_refused_so_normalisation_cannot_alias() {
        // 'ﬁ' normalises to "fi" under NFKC; admitting either would make two
        // distinct byte strings compare equal after normalisation.
        assert!(validate_segment("\u{fb01}le", IdentifierClass::Restricted, "x").is_err());
        assert!(validate_segment("\u{fb01}le", IdentifierClass::Scoped, "x").is_err());
    }

    #[test]
    fn bounds_and_control_characters_are_enforced() {
        let long = "a".repeat(MAX_SEGMENT_LENGTH + 1);
        assert!(validate_segment(&long, IdentifierClass::Restricted, "x").is_err());
        assert!(validate_segment("a\u{0}b", IdentifierClass::Scoped, "x").is_err());
        assert!(validate_segment("a b", IdentifierClass::Scoped, "x").is_err());
        assert!(validate_segment("", IdentifierClass::Scoped, "x").is_err());
    }

    #[test]
    fn a_separator_carries_no_traversal_meaning() {
        // '.' is admitted inside a segment and means nothing structural: it is
        // part of the name, never a parent reference.
        let dotted = NamespacedId::parse("draft.language.rust/source").unwrap();
        assert_eq!(dotted.namespace(), "draft.language.rust");
        // A runtime-scoped id may look path-like; nothing interprets it.
        let scoped = ScopedId::parse("root/collection-1").unwrap();
        assert_eq!(scoped.as_str(), "root/collection-1");
    }

    #[test]
    fn ordering_is_deterministic_for_canonical_sorting() {
        let mut ids = [
            NamespacedId::parse("draft.text.document/z").unwrap(),
            NamespacedId::parse("draft.text.document/a").unwrap(),
            NamespacedId::parse("draft.a/z").unwrap(),
        ];
        ids.sort();
        assert_eq!(
            ids.iter().map(NamespacedId::qualified).collect::<Vec<_>>(),
            [
                "draft.a/z",
                "draft.text.document/a",
                "draft.text.document/z"
            ]
        );
    }

    #[test]
    fn the_wire_form_round_trips() {
        let id = NamespacedId::parse("draft.software.project/bug-fix").unwrap();
        let encoded = serde_json::to_string(&id).unwrap();
        assert_eq!(encoded, "\"draft.software.project/bug-fix\"");
        assert_eq!(serde_json::from_str::<NamespacedId>(&encoded).unwrap(), id);
        assert!(serde_json::from_str::<NamespacedId>("\"unowned\"").is_err());
    }
}
