//! Extension-owned result schemas, and the rules that keep them safe.
//!
//! An extension that produces an opaque payload — a change representation, an
//! extracted element set, a tool result, an adapter response — declares the
//! schema that payload is validated against, and ships it inside the package.
//! Because `schemas/` is part of the package content hash, the schema bytes are
//! covered by the package signature, which is what lets a *historical* artifact
//! be validated years later against the exact schema that produced it rather
//! than whichever version happens to be installed now.
//!
//! Three properties are enforced here, in the portable crate, so a publisher can
//! check them without Draft:
//!
//! 1. **No network.** A schema may reference only local pointers. Anything that
//!    would make validation depend on fetching bytes at use time is refused.
//! 2. **Bounded.** Size, depth, node count and reference count are all capped,
//!    so a schema cannot become a denial-of-service vector against the validator.
//! 3. **Owned.** `(schema_id, revision)` is immutable within its owning
//!    namespace, so a publisher cannot redefine what a revision means, and two
//!    publishers cannot collide.

use crate::identifier::NamespacedId;
use crate::package;
use crate::{FormatError, FormatResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The schema reference, re-exported from the portable DCG contract.
///
/// The reference is a DCG value — canonical payloads carry one — while
/// everything below about *packaging* a schema document is a fact about a
/// package and stays here.
pub use draft_dcg_contract::SchemaRef;

/// One schema a package ships.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSchema {
    pub schema_id: NamespacedId,
    pub revision: u32,
    pub path: String,
}

impl PackageSchema {
    /// The reference that resolves to this schema.
    pub fn reference(&self) -> SchemaRef {
        SchemaRef::new(self.schema_id.clone(), self.revision)
    }
}

/// Validate one schema document's shape against the portable safety rules.
///
/// This is deliberately about the document's *structure*, not about whether it
/// is a semantically useful schema: the point is that compiling it, and
/// validating payloads against it, can never fetch anything or run unbounded.
pub fn validate_schema_document(document: &Value, what: &str) -> FormatResult<()> {
    let mut nodes = 0u32;
    let mut refs = 0u32;
    walk(document, 0, &mut nodes, &mut refs, what)?;
    Ok(())
}

fn walk(
    value: &Value,
    depth: u32,
    nodes: &mut u32,
    refs: &mut u32,
    what: &str,
) -> FormatResult<()> {
    *nodes += 1;
    if *nodes > package::MAX_SCHEMA_NODES {
        return Err(FormatError::Limit(format!(
            "{what} exceeds {} nodes",
            package::MAX_SCHEMA_NODES
        )));
    }
    if depth > package::MAX_SCHEMA_DEPTH {
        return Err(FormatError::Limit(format!(
            "{what} nests deeper than {}",
            package::MAX_SCHEMA_DEPTH
        )));
    }
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                // Dynamic resolution would make validation depend on state
                // outside the document, which is exactly what must not happen
                // when a historical payload is re-validated.
                if key == "$dynamicRef" || key == "$recursiveRef" || key == "$dynamicAnchor" {
                    return Err(FormatError::Identity(format!(
                        "{what} uses '{key}', which is not permitted in an extension schema"
                    )));
                }
                if key == "$ref" {
                    *refs += 1;
                    if *refs > package::MAX_SCHEMA_REFS {
                        return Err(FormatError::Limit(format!(
                            "{what} exceeds {} references",
                            package::MAX_SCHEMA_REFS
                        )));
                    }
                    let Some(target) = child.as_str() else {
                        return Err(FormatError::Identity(format!(
                            "{what} has a non-string '$ref'"
                        )));
                    };
                    validate_local_ref(target, what)?;
                }
                walk(child, depth + 1, nodes, refs, what)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for item in items {
                walk(item, depth + 1, nodes, refs, what)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Only a local pointer into this same document is admissible.
fn validate_local_ref(target: &str, what: &str) -> FormatResult<()> {
    if target.starts_with('#') {
        return Ok(());
    }
    let reason = if target.contains("://") {
        "names a network location"
    } else if target.starts_with("file:")
        || target.starts_with("http:")
        || target.starts_with("https:")
    {
        "names an external scheme"
    } else {
        "is not a local '#' pointer"
    };
    Err(FormatError::Identity(format!(
        "{what} has a '$ref' that {reason}: '{target}'"
    )))
}

/// Validate a schema document's raw bytes: size first, then structure.
pub fn validate_schema_bytes(bytes: &[u8], what: &str) -> FormatResult<Value> {
    if bytes.len() as u64 > package::MAX_SCHEMA_BYTES {
        return Err(FormatError::Limit(format!(
            "{what} exceeds the {} byte limit",
            package::MAX_SCHEMA_BYTES
        )));
    }
    let document: Value = serde_json::from_slice(bytes)
        .map_err(|error| FormatError::Encoding(format!("{what} is not valid JSON: {error}")))?;
    validate_schema_document(&document, what)?;
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_local_pointer_is_the_only_admissible_reference() {
        validate_schema_document(&json!({"$ref": "#/$defs/thing"}), "s").unwrap();
        for hostile in [
            "https://example.test/schema.json",
            "http://example.test/s",
            "file:///etc/passwd",
            "other.json",
        ] {
            assert!(
                validate_schema_document(&json!({ "$ref": hostile }), "s").is_err(),
                "{hostile} must be refused"
            );
        }
    }

    #[test]
    fn dynamic_resolution_is_refused() {
        for key in ["$dynamicRef", "$recursiveRef", "$dynamicAnchor"] {
            assert!(validate_schema_document(&json!({ key: "#x" }), "s").is_err());
        }
    }

    #[test]
    fn depth_nodes_and_refs_are_bounded() {
        let mut deep = json!({});
        for _ in 0..(package::MAX_SCHEMA_DEPTH + 4) {
            deep = json!({ "items": deep });
        }
        assert!(matches!(
            validate_schema_document(&deep, "s"),
            Err(FormatError::Limit(_))
        ));

        let many: Vec<Value> = (0..(package::MAX_SCHEMA_REFS + 2))
            .map(|_| json!({"$ref": "#/x"}))
            .collect();
        assert!(matches!(
            validate_schema_document(&json!({ "anyOf": many }), "s"),
            Err(FormatError::Limit(_))
        ));
    }

    #[test]
    fn oversized_bytes_are_refused_before_parsing() {
        let huge = vec![b'{'; (package::MAX_SCHEMA_BYTES + 1) as usize];
        assert!(matches!(
            validate_schema_bytes(&huge, "s"),
            Err(FormatError::Limit(_))
        ));
    }

    #[test]
    fn a_schema_reference_names_its_owner_and_revision() {
        let reference = SchemaRef::new(
            NamespacedId::parse("draft.text.document/line-changes").unwrap(),
            1,
        );
        assert_eq!(reference.to_string(), "draft.text.document/line-changes@1");
        assert!(!reference.is_core());
        assert!(
            SchemaRef::new(NamespacedId::parse("draft.core/whole-resource").unwrap(), 1).is_core()
        );
    }
}
