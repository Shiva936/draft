//! Who produced a canonical fact.
//!
//! Provenance, not authority. Recording that a package produced an Observation
//! says nothing about whether that package was trusted or authorized to do so —
//! those are separate facts (`SecurityFactRef`, `AuthorityDecision`) evaluated
//! by Core. Keeping them apart is what stops "we know who did it" from being
//! read as "they were allowed to".

use serde::{Deserialize, Serialize};

use crate::identifier::NamespacedId;
use crate::{FormatError, FormatResult};

/// Longest a producer version string may be.
pub const MAX_VERSION_LENGTH: usize = 64;

/// The identity of whatever produced a canonical fact.
///
/// `draft-extension-contract` adapts its `PackageIdentity` into this form, so
/// an extension-produced fact and a Core-produced one are described the same
/// way and a verifier needs only this crate to read either.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProducerIdentity {
    /// The namespaced identity of the producer.
    pub producer: NamespacedId,
    /// The producer's version, exactly as it declared it.
    ///
    /// Opaque to Core: it is compared byte-for-byte and never parsed as
    /// semver, because ordering vendor versions is not Core's judgement to
    /// make and a fact records what was declared.
    pub version: String,
}

impl ProducerIdentity {
    pub fn new(producer: NamespacedId, version: impl Into<String>) -> FormatResult<Self> {
        let version = version.into();
        if version.is_empty() {
            return Err(FormatError::Identity(
                "producer version must not be empty".into(),
            ));
        }
        if version.len() > MAX_VERSION_LENGTH {
            return Err(FormatError::Identity(format!(
                "producer version '{version}' exceeds {MAX_VERSION_LENGTH} bytes"
            )));
        }
        if let Some(character) = version.chars().find(|c| !c.is_ascii_graphic()) {
            return Err(FormatError::Identity(format!(
                "producer version '{version}' contains '{character}'; it must be printable ASCII"
            )));
        }
        Ok(Self { producer, version })
    }
}

impl std::fmt::Display for ProducerIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}@{}", self.producer, self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn producer() -> NamespacedId {
        NamespacedId::parse("acme.tools/observer").unwrap()
    }

    #[test]
    fn a_producer_names_itself_and_its_version() {
        let identity = ProducerIdentity::new(producer(), "1.4.2").unwrap();
        assert_eq!(identity.to_string(), "acme.tools/observer@1.4.2");
    }

    #[test]
    fn a_version_is_bounded_printable_and_never_empty() {
        assert!(ProducerIdentity::new(producer(), "").is_err());
        assert!(ProducerIdentity::new(producer(), "1.0 beta").is_err());
        assert!(ProducerIdentity::new(producer(), "a".repeat(MAX_VERSION_LENGTH + 1)).is_err());
    }

    #[test]
    fn versions_are_compared_byte_for_byte_not_as_semver() {
        // Two versions Core will not order for you. `1.10.0` is not "greater"
        // than `1.9.0` here; it is simply different.
        let older = ProducerIdentity::new(producer(), "1.9.0").unwrap();
        let newer = ProducerIdentity::new(producer(), "1.10.0").unwrap();
        assert_ne!(older, newer);
        assert!(newer < older, "byte ordering, deliberately not semver");
    }

    #[test]
    fn the_wire_form_round_trips() {
        let identity = ProducerIdentity::new(producer(), "1.4.2").unwrap();
        let encoded = serde_json::to_string(&identity).unwrap();
        assert_eq!(
            serde_json::from_str::<ProducerIdentity>(&encoded).unwrap(),
            identity
        );
    }
}
