//! Capability identifiers and contribution schema identifiers.
//!
//! A capability names *what an extension may be asked to do* —
//! `draft.resource.observe/v1`, `draft.publish/v1`. It is an open namespaced
//! vocabulary rather than a closed enum, so a new capability is added by
//! minting an identifier rather than by editing Core.
//!
//! `draft.*` is reserved: Draft implements reserved capabilities but never
//! mints them on a third party's behalf, and an unrecognised `draft.*`
//! capability is **rejected at validation time**. Passing it through as an
//! unknown vendor value would turn a typo in a reserved name into a silently
//! unowned capability.

use serde::{Deserialize, Serialize};

use crate::identifier::NamespacedId;
use crate::FormatResult;

/// What an extension may be asked to do.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityId(NamespacedId);

/// The reserved v1 capability vocabulary, in full.
///
/// Frozen for v1. Draft implements these; nobody else may mint one.
pub const RESERVED_CAPABILITIES: &[&str] = &[
    "draft.resource.detect/v1",
    "draft.resource.observe/v1",
    "draft.resource.fingerprint/v1",
    "draft.change.operate/v1",
    "draft.change.represent/v1",
    "draft.validate/v1",
    "draft.assess/v1",
    "draft.workspace.materialize/v1",
    "draft.compare/v1",
    "draft.merge/v1",
    "draft.lock/v1",
    "draft.publish/v1",
    "draft.recover/v1",
];

impl CapabilityId {
    pub fn parse(value: &str) -> FormatResult<Self> {
        Ok(Self(NamespacedId::parse(value)?))
    }

    pub fn as_namespaced(&self) -> &NamespacedId {
        &self.0
    }

    /// Whether this capability is in Draft's reserved namespace.
    pub fn is_reserved(&self) -> bool {
        self.0.namespace() == "draft" || self.0.namespace().starts_with("draft.")
    }

    /// Whether Draft recognises this exact reserved capability.
    pub fn is_recognised_reserved(&self) -> bool {
        RESERVED_CAPABILITIES.contains(&self.0.qualified().as_str())
    }

    /// Whether this capability is acceptable at validation time.
    ///
    /// A vendor capability is always acceptable — Core stores it without
    /// interpreting it. A reserved one is acceptable only if Draft actually
    /// implements it.
    pub fn is_acceptable(&self) -> bool {
        !self.is_reserved() || self.is_recognised_reserved()
    }
}

impl std::fmt::Display for CapabilityId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Identifies the schema a contribution payload is validated against.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContributionSchemaId(NamespacedId);

impl ContributionSchemaId {
    pub fn parse(value: &str) -> FormatResult<Self> {
        Ok(Self(NamespacedId::parse(value)?))
    }

    pub fn as_namespaced(&self) -> &NamespacedId {
        &self.0
    }
}

impl std::fmt::Display for ContributionSchemaId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reserved_capability_parses_and_is_recognised() {
        for reserved in RESERVED_CAPABILITIES {
            let capability = CapabilityId::parse(reserved).unwrap();
            assert!(capability.is_reserved(), "{reserved}");
            assert!(capability.is_recognised_reserved(), "{reserved}");
            assert!(capability.is_acceptable(), "{reserved}");
        }
    }

    #[test]
    fn an_unrecognised_reserved_capability_is_rejected_not_passed_through() {
        // The whole point: a near-miss on a reserved name must fail loudly
        // rather than become an unowned capability nobody implements.
        let typo = CapabilityId::parse("draft.resource.observ/v1").unwrap();
        assert!(typo.is_reserved());
        assert!(!typo.is_recognised_reserved());
        assert!(!typo.is_acceptable());
    }

    #[test]
    fn a_vendor_capability_round_trips_without_being_understood() {
        let vendor = CapabilityId::parse("acme.crm/reconcile").unwrap();
        assert!(!vendor.is_reserved());
        assert!(vendor.is_acceptable());
        let encoded = serde_json::to_string(&vendor).unwrap();
        assert_eq!(encoded, "\"acme.crm/reconcile\"");
        assert_eq!(
            serde_json::from_str::<CapabilityId>(&encoded).unwrap(),
            vendor
        );
    }

    #[test]
    fn a_capability_must_name_its_owner() {
        assert!(CapabilityId::parse("observe").is_err());
    }
}
