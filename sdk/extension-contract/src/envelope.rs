//! The contribution envelope: an open model for what an extension supplies.
//!
//! A closed enum of contribution kinds means a new kind of domain knowledge
//! requires editing Core and shipping a new Draft. The envelope replaces that
//! with three open values:
//!
//! ```text
//! ContributionEnvelope { capability, schema, payload }
//! ```
//!
//! * **capability** — what Draft may ask this contribution to do, as a
//!   namespaced [`CapabilityId`];
//! * **schema** — the [`ContributionSchemaId`] the payload is validated
//!   against, owned and shipped by the package;
//! * **payload** — a canonical structured value, opaque to Core.
//!
//! # `draft.*` is reserved, and a near-miss is an error
//!
//! Draft implements reserved capabilities but never mints one on a publisher's
//! behalf, and an unrecognised `draft.*` capability is **rejected** rather than
//! carried as an unknown vendor value. Otherwise a typo in a reserved name
//! would quietly become a new capability nobody implements — the package would
//! install, declare something plausible, and never be invoked, with no error
//! anywhere. A vendor capability, by contrast, is stored without being
//! understood, which is the point of an open vocabulary.
//!
//! # Identity
//!
//! [`ContributionEnvelope::contribution_digest`] is taken over the canonical
//! form of all three fields. Two envelopes agreeing on capability and schema
//! but differing in payload are different contributions, and a payload moved
//! under a different capability is a different contribution too — which is what
//! stops a package supplying one thing under the declaration of another.

use serde::{Deserialize, Serialize};

use draft_dcg_contract::digest::canonical_digest;
use draft_dcg_contract::{CapabilityId, ContributionSchemaId, Digest};

use crate::{FormatError, FormatResult};

/// The frozen domain separator for a contribution digest.
pub const CONTRIBUTION_DIGEST_DOMAIN: &str = "draft.extension.contribution/v1";

/// One thing an extension supplies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContributionEnvelope {
    /// What Draft may ask this contribution to do.
    pub capability: CapabilityId,
    /// The schema the payload is validated against.
    pub schema: ContributionSchemaId,
    /// The contribution itself, canonical and opaque to Core.
    pub payload: serde_json::Value,
}

impl ContributionEnvelope {
    pub fn new(
        capability: CapabilityId,
        schema: ContributionSchemaId,
        payload: serde_json::Value,
    ) -> FormatResult<Self> {
        let envelope = Self {
            capability,
            schema,
            payload,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    /// Check the envelope before anything depends on it.
    pub fn validate(&self) -> FormatResult<()> {
        if !self.capability.is_acceptable() {
            return Err(FormatError::Identity(format!(
                "'{}' is in Draft's reserved namespace but is not a capability this build \
                 implements; a reserved name is never carried as an unknown vendor value",
                self.capability
            )));
        }
        // A payload must be a structured value. A bare scalar carries no field
        // for a schema to validate, so it could never be checked against the
        // schema the envelope names.
        if !self.payload.is_object() && !self.payload.is_array() {
            return Err(FormatError::Encoding(format!(
                "contribution '{}' has a scalar payload; a contribution is a structured value \
                 its schema can validate",
                self.capability
            )));
        }
        Ok(())
    }

    /// This contribution's stable identity.
    pub fn contribution_digest(&self) -> FormatResult<Digest> {
        self.validate()?;
        canonical_digest(CONTRIBUTION_DIGEST_DOMAIN, self).map_err(FormatError::from)
    }

    /// Whether this contribution exercises a capability Draft itself defines.
    pub fn is_reserved(&self) -> bool {
        self.capability.is_reserved()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn capability(name: &str) -> CapabilityId {
        CapabilityId::parse(name).unwrap()
    }

    fn schema(name: &str) -> ContributionSchemaId {
        ContributionSchemaId::parse(name).unwrap()
    }

    fn envelope(capability_name: &str) -> ContributionEnvelope {
        ContributionEnvelope::new(
            capability(capability_name),
            schema("acme.tools/observer.v1"),
            json!({"root": "/srv", "recursive": true}),
        )
        .unwrap()
    }

    #[test]
    fn a_reserved_capability_is_accepted() {
        for reserved in draft_dcg_contract::capability::RESERVED_CAPABILITIES {
            ContributionEnvelope::new(
                capability(reserved),
                schema("acme.tools/thing.v1"),
                json!({}),
            )
            .unwrap_or_else(|error| panic!("{reserved}: {error}"));
        }
    }

    #[test]
    fn a_near_miss_on_a_reserved_name_is_rejected() {
        // The failure this prevents: the package installs, declares something
        // plausible, is never invoked, and nothing anywhere reports why.
        let error = ContributionEnvelope::new(
            capability("draft.resource.observ/v1"),
            schema("acme.tools/observer.v1"),
            json!({}),
        )
        .unwrap_err();
        assert!(matches!(error, FormatError::Identity(_)), "{error}");
    }

    #[test]
    fn a_vendor_capability_is_stored_without_being_understood() {
        // The point of an open vocabulary: Core does not know what this means
        // and does not need to.
        let vendor = envelope("acme.crm/reconcile");
        assert!(!vendor.is_reserved());
        vendor.contribution_digest().unwrap();
    }

    #[test]
    fn a_scalar_payload_is_refused() {
        // Nothing a schema could validate.
        for scalar in [json!("text"), json!(7), json!(true), json!(null)] {
            assert!(ContributionEnvelope::new(
                capability("draft.validate/v1"),
                schema("acme.tools/check.v1"),
                scalar,
            )
            .is_err());
        }
    }

    #[test]
    fn the_digest_covers_all_three_fields() {
        // A payload moved under a different capability, or validated against a
        // different schema, is a different contribution — which is what stops a
        // package supplying one thing under the declaration of another.
        let base = envelope("draft.resource.observe/v1")
            .contribution_digest()
            .unwrap();

        let moved = ContributionEnvelope::new(
            capability("draft.resource.detect/v1"),
            schema("acme.tools/observer.v1"),
            json!({"root": "/srv", "recursive": true}),
        )
        .unwrap()
        .contribution_digest()
        .unwrap();
        assert_ne!(base, moved);

        let reschemad = ContributionEnvelope::new(
            capability("draft.resource.observe/v1"),
            schema("acme.tools/other.v1"),
            json!({"root": "/srv", "recursive": true}),
        )
        .unwrap()
        .contribution_digest()
        .unwrap();
        assert_ne!(base, reschemad);

        let repayloaded = ContributionEnvelope::new(
            capability("draft.resource.observe/v1"),
            schema("acme.tools/observer.v1"),
            json!({"root": "/elsewhere", "recursive": true}),
        )
        .unwrap()
        .contribution_digest()
        .unwrap();
        assert_ne!(base, repayloaded);
    }

    #[test]
    fn the_digest_ignores_payload_key_order() {
        // Canonical, so two serializers produce one identity.
        let forward = ContributionEnvelope::new(
            capability("draft.validate/v1"),
            schema("acme.tools/check.v1"),
            json!({"a": 1, "b": 2}),
        )
        .unwrap();
        let reversed = ContributionEnvelope::new(
            capability("draft.validate/v1"),
            schema("acme.tools/check.v1"),
            json!({"b": 2, "a": 1}),
        )
        .unwrap();
        assert_eq!(
            forward.contribution_digest().unwrap(),
            reversed.contribution_digest().unwrap()
        );
    }

    #[test]
    fn an_unowned_capability_or_schema_is_refused() {
        assert!(CapabilityId::parse("observe").is_err());
        assert!(ContributionSchemaId::parse("check").is_err());
    }

    #[test]
    fn the_wire_form_round_trips_and_rejects_unknown_fields() {
        let envelope = envelope("draft.validate/v1");
        let encoded = serde_json::to_string(&envelope).unwrap();
        assert_eq!(
            serde_json::from_str::<ContributionEnvelope>(&encoded).unwrap(),
            envelope
        );
        let widened = encoded.replace("{\"capability\"", "{\"execute\":true,\"capability\"");
        assert!(serde_json::from_str::<ContributionEnvelope>(&widened).is_err());
    }
}
