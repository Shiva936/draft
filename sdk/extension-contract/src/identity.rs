//! Who published a package, and how that becomes provenance.
//!
//! A [`PackageIdentity`] names an installed artifact: an extension id and the
//! version it declared. When that package produces a canonical fact — an
//! Observation, a derived Relation — the fact records a
//! [`ProducerIdentity`] instead, which is the portable form an external
//! verifier can read without knowing anything about extension packaging.
//!
//! The two are deliberately different types rather than one shared struct.
//! `PackageIdentity` is about *installation*: it answers "which artifact is on
//! this machine". `ProducerIdentity` is about *history*: it answers "what
//! produced this fact", and it travels inside canonical bytes that outlive the
//! installation entirely. Adapting one into the other is a real conversion, and
//! making it explicit is what stops installation concerns leaking into a
//! digest.
//!
//! Neither is authority. Recording that a package produced something says
//! nothing about whether it was trusted or authorized to — those are separate
//! facts Core evaluates.

use serde::{Deserialize, Serialize};

use draft_dcg_contract::identifier::NamespacedId;
use draft_dcg_contract::ProducerIdentity;

use crate::manifest::ExtensionId;
use crate::{FormatError, FormatResult};

/// An installed package: its extension id and declared version.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIdentity {
    pub extension: ExtensionId,
    /// The version exactly as the package declared it.
    ///
    /// Opaque: compared byte-for-byte and never parsed as semver, because
    /// ordering vendor versions is not Draft's judgement to make and a fact
    /// records what was declared rather than what it implies.
    pub version: String,
}

impl PackageIdentity {
    pub fn new(extension: ExtensionId, version: impl Into<String>) -> Self {
        Self {
            extension,
            version: version.into(),
        }
    }

    /// The portable provenance form recorded inside a canonical fact.
    ///
    /// Fallible because the portable form is stricter: it bounds the version
    /// and requires printable ASCII, so a package that could be installed but
    /// whose identity could not be canonically recorded fails here rather than
    /// producing a fact nobody can verify.
    pub fn to_producer_identity(&self) -> FormatResult<ProducerIdentity> {
        let producer = NamespacedId::parse(&format!("{}/package", self.extension.as_str()))
            .map_err(|error| {
                FormatError::Identity(format!(
                    "extension '{}' does not form a producer identity: {error}",
                    self.extension.as_str()
                ))
            })?;
        ProducerIdentity::new(producer, self.version.clone()).map_err(FormatError::from)
    }
}

impl std::fmt::Display for PackageIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}@{}", self.extension.as_str(), self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(extension: &str, version: &str) -> PackageIdentity {
        PackageIdentity::new(ExtensionId::parse(extension).unwrap(), version)
    }

    #[test]
    fn a_package_adapts_into_portable_provenance() {
        let producer = identity("acme.tools", "1.4.2")
            .to_producer_identity()
            .unwrap();
        assert_eq!(producer.version, "1.4.2");
        assert_eq!(producer.producer.namespace(), "acme.tools");
    }

    #[test]
    fn two_versions_of_one_package_are_different_producers() {
        // A fact records which build produced it, so an upgrade is visible in
        // provenance rather than silently reattributed.
        assert_ne!(
            identity("acme.tools", "1.4.2")
                .to_producer_identity()
                .unwrap(),
            identity("acme.tools", "1.5.0")
                .to_producer_identity()
                .unwrap()
        );
    }

    #[test]
    fn an_identity_that_cannot_be_recorded_fails_at_the_conversion() {
        // The portable form is stricter than installation. Failing here is
        // better than installing something whose facts nobody could verify.
        assert!(identity("acme.tools", "").to_producer_identity().is_err());
        assert!(identity("acme.tools", "1.0 beta")
            .to_producer_identity()
            .is_err());
        assert!(identity("acme.tools", &"9".repeat(200))
            .to_producer_identity()
            .is_err());
    }

    #[test]
    fn versions_are_compared_byte_for_byte_not_as_semver() {
        // Draft will not order vendor versions for you: 1.10.0 is not
        // "greater" than 1.9.0 here, it is simply different.
        let older = identity("acme.tools", "1.9.0");
        let newer = identity("acme.tools", "1.10.0");
        assert_ne!(older, newer);
        assert!(newer < older, "byte ordering, deliberately not semver");
    }

    #[test]
    fn the_wire_form_round_trips() {
        let identity = identity("acme.tools", "1.4.2");
        let encoded = serde_json::to_string(&identity).unwrap();
        assert_eq!(
            serde_json::from_str::<PackageIdentity>(&encoded).unwrap(),
            identity
        );
        assert_eq!(identity.to_string(), "acme.tools@1.4.2");
    }
}
