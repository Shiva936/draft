//! Portable provenance values that canonical facts record, but do not own.
//!
//! Every type here is a *value*: the immutable thing a historical fact wrote
//! down. None of them carries the behaviour that produced it. A
//! [`LeaseFence`] is the number a lease had when an attempt was dispatched —
//! lease acquisition, expiry and enforcement live in `core::execution` and
//! `services/locks`. A [`ProjectControlGeneration`] is the generation a
//! compare-exchange observed — the counter itself lives in `core::project`.
//!
//! They are here for one reason: an independent verifier reading a canonical
//! `PublicationAttempt` must be able to parse and compare these fields without
//! linking Draft. Moving the value down does not move the subsystem down.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::identifier::NamespacedId;
use crate::{FormatError, FormatResult};

/// An instant, as nanoseconds since the Unix epoch in UTC.
///
/// Deliberately an integer rather than a formatted string. Timestamps appear
/// inside canonical objects whose digests are historical identity, and two
/// renderings of the same instant — `...T00:00:00Z` and `...T00:00:00.000Z` —
/// would produce different bytes and therefore different digests. An integer
/// has exactly one canonical form.
///
/// Whether a timestamp affects a digest is a question about *reachability from
/// the manifest roots*, never about the datatype: provenance timestamps inside
/// canonical provenance objects legitimately change `StateEvidenceRoot` and so
/// `BaselineId`, while `BaselineRecord.accepted_at` does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const fn from_unix_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    pub const fn as_unix_nanos(self) -> i64 {
        self.0
    }
}

impl std::fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Declares a monotonic `u64` provenance counter value.
macro_rules! counter_value {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                write!(formatter, "{}", self.0)
            }
        }
    };
}

counter_value!(
    /// The generation `ProjectControlState` had at the moment a fact was
    /// committed. The counter and its compare-exchange live in `core::project`.
    ProjectControlGeneration
);
counter_value!(
    /// The generation a `ProviderBinding` record had when a dispatch validated
    /// its route. The record and its store live in `core::project`.
    ProviderBindingGeneration
);
counter_value!(
    /// The revision a trust registry was observed at. The registry and its
    /// revision advancement live in `core::trust`.
    RegistryRevision
);
counter_value!(
    /// The monotonic fencing token a lease held. Allocation and enforcement
    /// live in `core::execution` and `services/locks`.
    ///
    /// Recording the fence is what lets a verifier see which lease generation
    /// authorized an external effect; it grants no authority by itself.
    LeaseFence
);

/// Identifies a trust registry whose revisions a fact observed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RegistryId(NamespacedId);

impl RegistryId {
    pub fn parse(value: &str) -> FormatResult<Self> {
        Ok(Self(NamespacedId::parse(value)?))
    }

    pub fn from_namespaced(value: NamespacedId) -> Self {
        Self(value)
    }

    pub fn as_namespaced(&self) -> &NamespacedId {
        &self.0
    }
}

impl std::fmt::Display for RegistryId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Every registry revision a fact observed, as one canonical map.
///
/// A `BTreeMap` rather than a list so the canonical form has exactly one
/// ordering and a registry can appear at most once. A verifier reads this to
/// answer "which trust state was this decision taken against?".
pub type RegistryRevisions = BTreeMap<RegistryId, RegistryRevision>;

/// Identifies a lease. The lease's lifetime, scope and owner are runtime state
/// in `core::execution`; only the identity travels in a canonical fact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LeaseId(String);

impl LeaseId {
    pub fn parse(value: impl Into<String>) -> FormatResult<Self> {
        let value = value.into();
        crate::identifier::validate_segment(
            &value,
            crate::identifier::IdentifierClass::Scoped,
            "lease id",
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LeaseId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for LeaseId {
    type Error = FormatError;

    fn try_from(value: String) -> FormatResult<Self> {
        Self::parse(value)
    }
}

impl From<LeaseId> for String {
    fn from(value: LeaseId) -> String {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_has_exactly_one_canonical_form() {
        let instant = Timestamp::from_unix_nanos(1_700_000_000_123_456_789);
        assert_eq!(
            serde_json::to_string(&instant).unwrap(),
            "1700000000123456789"
        );
        assert_eq!(
            serde_json::from_str::<Timestamp>("1700000000123456789").unwrap(),
            instant
        );
    }

    #[test]
    fn counters_are_ordered_so_a_verifier_can_compare_generations() {
        assert!(ProjectControlGeneration::new(4) > ProjectControlGeneration::new(3));
        assert!(LeaseFence::new(9) > LeaseFence::new(8));
    }

    #[test]
    fn registry_revisions_have_one_canonical_ordering() {
        let mut revisions = RegistryRevisions::new();
        revisions.insert(
            RegistryId::parse("draft.trust/publishers").unwrap(),
            RegistryRevision::new(412),
        );
        revisions.insert(
            RegistryId::parse("draft.trust/authorities").unwrap(),
            RegistryRevision::new(7),
        );
        // Sorted by registry id regardless of insertion order.
        assert_eq!(
            serde_json::to_string(&revisions).unwrap(),
            r#"{"draft.trust/authorities":7,"draft.trust/publishers":412}"#
        );
    }

    #[test]
    fn an_unowned_registry_id_is_refused() {
        assert!(RegistryId::parse("publishers").is_err());
    }

    #[test]
    fn a_lease_id_round_trips_and_rejects_whitespace() {
        let lease = LeaseId::parse("lease-7").unwrap();
        let encoded = serde_json::to_string(&lease).unwrap();
        assert_eq!(serde_json::from_str::<LeaseId>(&encoded).unwrap(), lease);
        assert!(LeaseId::parse("lease 7").is_err());
    }
}
