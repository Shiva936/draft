//! Portable provider identity and provenance — describing a route, never
//! executing one.
//!
//! These values are deliberately SDK-owned, because canonical historical facts
//! contain them: a Baseline records which provider established its state, and a
//! `PublicationAttempt` records exactly which route it was authorized against.
//! A verifier holding only this crate must be able to read both.
//!
//! What is forbidden here is the *runtime*: provider clients, HTTP/Git/cloud
//! SDKs, credential secrets, `CredentialHandleRef`, sessions, mutable binding
//! state and dispatch code all live in Core.
//!
//! # Provenance is not a route
//!
//! The distinction is the point of this module.
//!
//! * A [`ProviderProvenanceRef`] says *what established this state*: a binding
//!   and the semantic definition in force. Baseline composition exposes this,
//!   and it never changes when a binding is later reprofiled or unbound.
//! * A [`ProviderRouteRef`] adds the operational profile, and exists only to
//!   plan an executable action.
//!
//! A planned route is frozen at planning time and is **never** re-resolved
//! against current pointers at execution. If the binding has since moved to a
//! different semantic definition or operational profile, the plan is stale and
//! is refused, re-planned or re-authorized. That a historical definition still
//! exists and would still work is never sufficient authority to perform a new
//! external side effect.

use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::ids::ProviderBindingId;

/// Declares a canonical digest naming an immutable provider fact.
macro_rules! provider_digest {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Digest);

        impl $name {
            pub fn new(digest: Digest) -> Self {
                Self(digest)
            }

            pub fn digest(&self) -> &Digest {
                &self.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

provider_digest!(
    /// The digest of an immutable `ProviderSemanticDefinition`: what the
    /// provider's namespace, roots and endpoints *mean*, and which resource
    /// state semantics it produces.
    ///
    /// Opaque to Core. It participates in material-state provenance, so
    /// changing it changes what an observation is claiming.
    ProviderSemanticDefinitionDigest
);
provider_digest!(
    /// The digest of an immutable `ProviderOperationalProfile`: capabilities,
    /// merge capability, concurrency policy, delivery semantics and limits.
    ///
    /// Deliberately **not** part of accepted state provenance — how a provider
    /// is operated does not change what it observed. It is part of a route,
    /// because it does change how an action would be performed.
    ProviderOperationalProfileDigest
);

/// What established a piece of accepted state.
///
/// This is what Baseline composition exposes. It is immutable history: later
/// unbinding, reprofiling or redefining the binding leaves it untouched.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProvenanceRef {
    /// The binding that produced the observation.
    pub binding: ProviderBindingId,
    /// The semantic definition in force when it did.
    pub semantic_definition: ProviderSemanticDefinitionDigest,
}

/// An exact executable route, frozen at planning time.
///
/// Constructed only when an executable action is planned. Before the first
/// provider side effect, execution takes the binding's correctness lock and
/// requires **exact equality** with the binding's current pointers — not mere
/// availability of the definition and profile named here.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRouteRef {
    /// Which binding and semantic definition this route runs against.
    pub provenance: ProviderProvenanceRef,
    /// The operational profile the action was planned under.
    pub operational_profile: ProviderOperationalProfileDigest,
}

impl ProviderRouteRef {
    /// The binding this route names.
    pub fn binding(&self) -> &ProviderBindingId {
        &self.provenance.binding
    }

    /// Whether a binding's current pointers still select exactly this route.
    ///
    /// The whole check, in one place, so no caller can accidentally accept a
    /// weaker condition. `lifecycle_active` is supplied by Core, which owns the
    /// binding's lifecycle; everything else is exact equality.
    ///
    /// A `false` result means the plan is **stale**, not that the route is
    /// unusable in principle: the correct response is to refuse, re-plan and
    /// re-authorize, never to proceed on the historical route and never to
    /// silently adopt the binding's new one.
    pub fn is_selected_by(
        &self,
        lifecycle_active: bool,
        binding: &ProviderBindingId,
        current_semantic_definition: &ProviderSemanticDefinitionDigest,
        current_operational_profile: &ProviderOperationalProfileDigest,
    ) -> bool {
        lifecycle_active
            && &self.provenance.binding == binding
            && &self.provenance.semantic_definition == current_semantic_definition
            && &self.operational_profile == current_operational_profile
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> ProviderBindingId {
        ProviderBindingId::parse("pbd_a1b2c3").unwrap()
    }

    fn definition(seed: &[u8]) -> ProviderSemanticDefinitionDigest {
        ProviderSemanticDefinitionDigest::new(Digest::of_bytes(seed))
    }

    fn profile(seed: &[u8]) -> ProviderOperationalProfileDigest {
        ProviderOperationalProfileDigest::new(Digest::of_bytes(seed))
    }

    fn route() -> ProviderRouteRef {
        ProviderRouteRef {
            provenance: ProviderProvenanceRef {
                binding: binding(),
                semantic_definition: definition(b"SD1"),
            },
            operational_profile: profile(b"OP1"),
        }
    }

    #[test]
    fn a_route_is_selected_only_by_exact_current_equality() {
        let route = route();
        assert!(route.is_selected_by(true, &binding(), &definition(b"SD1"), &profile(b"OP1")));
    }

    #[test]
    fn a_moved_semantic_definition_makes_the_plan_stale() {
        let route = route();
        assert!(!route.is_selected_by(true, &binding(), &definition(b"SD2"), &profile(b"OP1")));
    }

    #[test]
    fn a_moved_operational_profile_makes_the_plan_stale() {
        let route = route();
        assert!(!route.is_selected_by(true, &binding(), &definition(b"SD1"), &profile(b"OP2")));
    }

    #[test]
    fn an_inactive_binding_never_selects_a_route() {
        // Availability is not authority: the definition and profile still
        // match exactly, and the route is still refused.
        let route = route();
        assert!(!route.is_selected_by(false, &binding(), &definition(b"SD1"), &profile(b"OP1")));
    }

    #[test]
    fn another_binding_never_selects_this_route() {
        let route = route();
        let other = ProviderBindingId::parse("pbd_999999").unwrap();
        assert!(!route.is_selected_by(true, &other, &definition(b"SD1"), &profile(b"OP1")));
    }

    #[test]
    fn changing_the_operational_profile_leaves_provenance_untouched() {
        // The distinction the whole module exists for: reprofiling a binding
        // changes routes but not what was accepted.
        let accepted = route().provenance;
        let reprofiled = ProviderRouteRef {
            provenance: accepted.clone(),
            operational_profile: profile(b"OP2"),
        };
        assert_eq!(accepted, reprofiled.provenance);
        assert_ne!(route(), reprofiled);
    }

    #[test]
    fn the_wire_forms_round_trip_and_reject_unknown_fields() {
        let route = route();
        let encoded = serde_json::to_string(&route).unwrap();
        assert_eq!(
            serde_json::from_str::<ProviderRouteRef>(&encoded).unwrap(),
            route
        );
        assert!(serde_json::from_str::<ProviderProvenanceRef>(
            r#"{"binding":"pbd_a1","semantic_definition":"sha256:00","credential":"secret"}"#
        )
        .is_err());
    }
}
