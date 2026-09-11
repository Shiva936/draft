//! The open kind vocabularies.
//!
//! Draft Core defines no closed enum of resource kinds, relation types,
//! operations, evidence, assessments, representations or providers. Each is a
//! [`NamespacedId`] minted by whoever owns the namespace, so a new domain is
//! added by contributing identifiers rather than by editing Core.
//!
//! That is what keeps the platform domain-neutral: nothing here names a file
//! extension, a programming language, a toolchain or a diff. Core stores and
//! compares these values without interpreting them.
//!
//! `draft.*` is reserved. Draft implements reserved identifiers but a third
//! party may never mint one, and an unrecognised `draft.*` identifier is
//! rejected at validation time rather than passed through as an unknown vendor
//! value — otherwise a typo in a reserved name would silently become a new,
//! unowned kind.

use serde::{Deserialize, Serialize};

use crate::identifier::NamespacedId;
use crate::FormatResult;

/// The namespace Draft reserves for identifiers it owns.
pub const RESERVED_NAMESPACE_PREFIX: &str = "draft.";

/// Declares one open, namespaced kind vocabulary.
macro_rules! kind_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(NamespacedId);

        impl $name {
            pub fn parse(value: &str) -> FormatResult<Self> {
                Ok(Self(NamespacedId::parse(value)?))
            }

            pub fn from_namespaced(value: NamespacedId) -> Self {
                Self(value)
            }

            pub fn as_namespaced(&self) -> &NamespacedId {
                &self.0
            }

            /// Whether this identifier is in Draft's reserved namespace.
            ///
            /// Reserved identifiers must be recognised by name; an unknown one
            /// is an error, never an opaque vendor value.
            pub fn is_reserved(&self) -> bool {
                self.0.namespace() == "draft"
                    || self.0.namespace().starts_with(RESERVED_NAMESPACE_PREFIX)
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

kind_id!(
    /// What kind of thing a Resource is. Never inferred from a locator.
    ResourceKindId);
kind_id!(
    /// What kind of relationship one Resource bears to another.
    RelationTypeId);
kind_id!(
    /// What kind of mutation an Operation performs.
    OperationKindId);
kind_id!(
    /// What kind of Evidence was produced about a ChangeRevision.
    EvidenceKindId);
kind_id!(
    /// What kind of Assessment was produced about a ChangeRevision.
    AssessmentKindId);
kind_id!(
    /// What kind of rendering a ChangeRepresentation is.
    ///
    /// A line diff is one representation among many, never the universal one.
    RepresentationKindId);
kind_id!(
    /// What kind of provider a binding attaches to.
    ProviderKindId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_must_name_the_namespace_that_owns_it() {
        assert!(ResourceKindId::parse("draft.text.document/document").is_ok());
        // An unowned kind is refused: it is exactly what lets two vendors
        // collide on one name.
        assert!(ResourceKindId::parse("document").is_err());
    }

    #[test]
    fn the_reserved_namespace_is_recognisable() {
        assert!(ResourceKindId::parse("draft.text.document/document")
            .unwrap()
            .is_reserved());
        assert!(!ResourceKindId::parse("acme.crm/account")
            .unwrap()
            .is_reserved());
        // A namespace that merely starts with the same letters is not
        // reserved; ownership is by namespace, not by prefix similarity.
        assert!(!ResourceKindId::parse("drafty.tool/thing")
            .unwrap()
            .is_reserved());
    }

    #[test]
    fn kinds_are_distinct_types_even_with_equal_wire_forms() {
        let resource = ResourceKindId::parse("acme.crm/account").unwrap();
        let relation = RelationTypeId::parse("acme.crm/account").unwrap();
        // They serialize identically, and that is fine — what matters is that
        // the type system will not let one be passed where the other is meant.
        assert_eq!(
            serde_json::to_string(&resource).unwrap(),
            serde_json::to_string(&relation).unwrap()
        );
    }
}
