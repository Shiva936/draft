//! How a Resource is located and described, without saying what it is.
//!
//! A [`ResourceLocator`] says *where* something was found; a [`ResourceForm`]
//! says what intrinsic shape an observer saw; [`AttributeValue`] carries typed
//! intrinsic facts. None of them is a classification: deciding that a resource
//! is "source code" or "a customer record" is a domain judgement contributed by
//! an extension, never something Core reads out of a path.
//!
//! This matters for identity. A Resource keeps its `res_` across move and
//! rename precisely because the locator is not the identity — it is one
//! observed attribute of it.

use serde::{Deserialize, Serialize};

use crate::{FormatError, FormatResult};

/// Where an observer found a Resource, inside the scope that observed it.
///
/// Deliberately an opaque string with no path semantics in Core: `/` and `.`
/// carry no traversal meaning here. A provider's semantic definition says how
/// to interpret its own locators, and only that definition's parser does so.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ResourceLocator(String);

/// Longest a canonical locator may be.
pub const MAX_LOCATOR_LENGTH: usize = 4096;

impl ResourceLocator {
    pub fn parse(value: impl Into<String>) -> FormatResult<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(FormatError::Identity(
                "resource locator must not be empty".into(),
            ));
        }
        if value.len() > MAX_LOCATOR_LENGTH {
            return Err(FormatError::Identity(format!(
                "resource locator exceeds {MAX_LOCATOR_LENGTH} bytes"
            )));
        }
        // Control characters would make a locator ambiguous in every log,
        // error message and canonical rendering it appears in.
        if let Some(character) = value.chars().find(|c| c.is_control()) {
            return Err(FormatError::Identity(format!(
                "resource locator contains the control character U+{:04X}",
                character as u32
            )));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ResourceLocator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for ResourceLocator {
    type Error = FormatError;

    fn try_from(value: String) -> FormatResult<Self> {
        Self::parse(value)
    }
}

impl From<ResourceLocator> for String {
    fn from(value: ResourceLocator) -> String {
        value.0
    }
}

/// The intrinsic shape an observer saw. Never an extension classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceForm {
    /// Carries a byte stream.
    Bytes,
    /// Contains other resources.
    Collection,
    /// Points at something else.
    Reference,
    /// Exists only as declared state, with no byte stream at all.
    Logical,
}

/// A typed intrinsic attribute value.
///
/// The three cases are deliberately few. An attribute is a fact an observer
/// established about a Resource, not a place to smuggle structured domain data
/// past the contribution schemas that would otherwise validate it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttributeValue {
    Text(String),
    Integer(i64),
    Boolean(bool),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locator_carries_no_traversal_meaning() {
        // Accepted verbatim: interpreting `..` is the provider's business, and
        // Core refusing it here would be Core claiming path semantics it does
        // not have.
        let locator = ResourceLocator::parse("a/../b").unwrap();
        assert_eq!(locator.as_str(), "a/../b");
    }

    #[test]
    fn a_locator_is_bounded_and_free_of_control_characters() {
        assert!(ResourceLocator::parse("").is_err());
        assert!(ResourceLocator::parse("a\nb").is_err());
        assert!(ResourceLocator::parse("a\u{0}b").is_err());
        assert!(ResourceLocator::parse("a".repeat(MAX_LOCATOR_LENGTH + 1)).is_err());
    }

    #[test]
    fn a_locator_admits_non_ascii_because_the_world_has_names() {
        // Unlike an identifier, a locator is not a namespace or authority
        // decision, so it is not restricted to ASCII.
        let locator = ResourceLocator::parse("docs/引き継ぎ.md").unwrap();
        assert_eq!(
            serde_json::from_str::<ResourceLocator>(&serde_json::to_string(&locator).unwrap())
                .unwrap(),
            locator
        );
    }

    #[test]
    fn attribute_values_keep_their_type_across_the_wire() {
        for value in [
            AttributeValue::Text("hello".into()),
            AttributeValue::Integer(-7),
            AttributeValue::Boolean(true),
        ] {
            let encoded = serde_json::to_string(&value).unwrap();
            assert_eq!(
                serde_json::from_str::<AttributeValue>(&encoded).unwrap(),
                value
            );
        }
        // Untagged, so the JSON is the bare value rather than a wrapper.
        assert_eq!(
            serde_json::to_string(&AttributeValue::Integer(7)).unwrap(),
            "7"
        );
    }

    #[test]
    fn resource_form_uses_a_stable_snake_case_wire_form() {
        assert_eq!(
            serde_json::to_string(&ResourceForm::Collection).unwrap(),
            "\"collection\""
        );
    }
}
