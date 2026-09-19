//! The generic schema-reference primitive.
//!
//! A [`SchemaRef`] names *which* schema a structured payload is validated
//! against, as an owned identifier plus an immutable revision. It says nothing
//! about where the schema document lives — packaging, package-relative paths
//! and the safety rules for a shipped schema document belong to
//! `draft-extension-contract`, because they are facts about a package rather
//! than about the DCG.
//!
//! The reference lives here because contributions, representations and other
//! canonical structures carry one, and an independent verifier must be able to
//! read it without knowing anything about extension packaging.
//!
//! `(schema_id, revision)` is immutable within its owning namespace. A
//! publisher cannot redefine what a revision means, and two publishers cannot
//! collide, because the identifier is namespaced to the owner.

use serde::{Deserialize, Serialize};

use crate::identifier::NamespacedId;

/// A reference to a schema by owned identifier and immutable revision.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaRef {
    pub schema_id: NamespacedId,
    pub revision: u32,
}

impl SchemaRef {
    pub fn new(schema_id: NamespacedId, revision: u32) -> Self {
        Self {
            schema_id,
            revision,
        }
    }

    /// Draft's own built-in namespace — the only one that resolves without an
    /// installed package.
    pub const CORE_NAMESPACE: &'static str = "draft.core";

    /// Whether this reference resolves in Draft's built-in namespace.
    pub fn is_core(&self) -> bool {
        self.schema_id.namespace() == Self::CORE_NAMESPACE
    }
}

impl std::fmt::Display for SchemaRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}@{}", self.schema_id, self.revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(namespace: &str, revision: u32) -> SchemaRef {
        SchemaRef::new(
            NamespacedId::parse(&format!("{namespace}/result")).unwrap(),
            revision,
        )
    }

    #[test]
    fn a_revision_is_part_of_the_reference() {
        assert_ne!(reference("acme.tools", 1), reference("acme.tools", 2));
    }

    #[test]
    fn ownership_is_by_namespace_so_publishers_cannot_collide() {
        assert_ne!(reference("acme.tools", 1), reference("other.tools", 1));
        assert!(reference("draft.core", 1).is_core());
        assert!(!reference("acme.tools", 1).is_core());
        // A namespace that merely starts the same is not Draft's.
        assert!(!reference("draft.core.extra", 1).is_core());
    }

    #[test]
    fn the_wire_form_round_trips_and_rejects_unknown_fields() {
        let reference = reference("acme.tools", 3);
        let encoded = serde_json::to_string(&reference).unwrap();
        assert_eq!(
            serde_json::from_str::<SchemaRef>(&encoded).unwrap(),
            reference
        );
        assert!(serde_json::from_str::<SchemaRef>(
            r#"{"schema_id":"acme.tools/result","revision":3,"url":"http://x"}"#
        )
        .is_err());
    }
}
