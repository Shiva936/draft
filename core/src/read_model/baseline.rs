//! Joining accepted provenance to current configuration.
//!
//! The two views themselves live in [`crate::dcg::compose`]:
//! `HistoricalBaselineComposition` answers "what established this accepted
//! state?" and never changes; `CurrentProviderRoutability` answers "could that
//! binding act right now?" and moves whenever the binding does. They are not
//! restated here — one canonical value has exactly one Rust type, and a second
//! pair with the same names would drift from the first.
//!
//! What belongs here is the one operation that needs both: constructing the
//! route a *new* Operation would use.
//!
//! # Why the join is explicit
//!
//! A `ProviderRouteRef` is `{binding, semantic_definition, operational_profile}`
//! — everything needed to send something. A Baseline's provenance is only
//! `{binding, semantic_definition}`: what produced the observation it accepted.
//!
//! The operational profile is missing on purpose. It describes how to reach a
//! target *today*, and a Baseline is a fact about the past. If provenance
//! carried a profile, reprofiling a binding would silently rewrite what every
//! historical Baseline claims to have been made from — history changing
//! because configuration changed.
//!
//! So a Baseline never derives a route (Scenario CR). This function is the
//! visible moment where past and present are joined, and it reads the profile
//! from the binding rather than from its caller precisely because that is the
//! half that must come from now.
//!
//! # Why unroutable is not an error
//!
//! A Baseline composed from a binding that has since been unbound is still
//! valid and verifiable. It simply cannot receive new work. Reporting that as
//! corruption would make a routine configuration change look like data loss,
//! and push people toward "fixing" history to clear the error.

use draft_dcg_contract::provider::{ProviderProvenanceRef, ProviderRouteRef};

use crate::dcg::compose::CurrentProviderRoutability;
use crate::project::provider::ProviderBinding;

/// Why accepted provenance cannot be routed to now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteRefusal {
    /// The binding is gone, or is no longer active.
    NotRoutable,
    /// The binding is active but now selects different semantics.
    ///
    /// Refused rather than silently re-pointed: the accepted provenance says
    /// what the state was observed under, and sending new work under different
    /// semantics would attribute it to an agreement that has since changed.
    SemanticsRedefined,
}

/// Construct the route a new Operation against `provenance` would use.
///
/// `binding` is `None` when nothing is bound under that id. Returns the
/// refusal instead of a route when current configuration cannot serve it.
pub fn route_for(
    provenance: &ProviderProvenanceRef,
    binding: Option<&ProviderBinding>,
) -> Result<ProviderRouteRef, RouteRefusal> {
    let Some(binding) = binding else {
        return Err(RouteRefusal::NotRoutable);
    };
    if !CurrentProviderRoutability::of(binding).is_routable() {
        return Err(RouteRefusal::NotRoutable);
    }
    if binding.current_semantic_definition != provenance.semantic_definition {
        return Err(RouteRefusal::SemanticsRedefined);
    }
    Ok(ProviderRouteRef {
        provenance: provenance.clone(),
        // From the binding, never from the caller: this is the half that must
        // come from the present.
        operational_profile: binding.current_operational_profile.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::provider::ProviderBindingLifecycle;
    use draft_dcg_contract::ids::{ProjectId, ProviderBindingId};
    use draft_dcg_contract::provider::ProviderOperationalProfileDigest;
    use draft_dcg_contract::{Digest, ProviderKindId, ProviderSemanticDefinitionDigest};

    fn binding_id() -> ProviderBindingId {
        ProviderBindingId::parse("pbd_000000000001").unwrap()
    }

    fn semantics(seed: &[u8]) -> ProviderSemanticDefinitionDigest {
        ProviderSemanticDefinitionDigest::new(Digest::of_bytes(seed))
    }

    fn profile(seed: &[u8]) -> ProviderOperationalProfileDigest {
        ProviderOperationalProfileDigest::new(Digest::of_bytes(seed))
    }

    fn provenance() -> ProviderProvenanceRef {
        ProviderProvenanceRef {
            binding: binding_id(),
            semantic_definition: semantics(b"SD1"),
        }
    }

    fn bound(
        semantic_definition: ProviderSemanticDefinitionDigest,
        operational_profile: ProviderOperationalProfileDigest,
        lifecycle: ProviderBindingLifecycle,
    ) -> ProviderBinding {
        ProviderBinding {
            generation: 1,
            id: binding_id(),
            project: ProjectId::parse("prj_000000000001").unwrap(),
            kind: ProviderKindId::parse("draft.filesystem/local").unwrap(),
            current_semantic_definition: semantic_definition,
            current_operational_profile: operational_profile,
            lifecycle,
        }
    }

    fn active(profile_seed: &[u8]) -> ProviderBinding {
        bound(
            semantics(b"SD1"),
            profile(profile_seed),
            ProviderBindingLifecycle::Active,
        )
    }

    #[test]
    fn reprofiling_changes_the_route_and_not_the_provenance() {
        // Scenario CR. OP1 -> OP2 gives a different route for new work and the
        // same accepted provenance, because history did not move.
        let before = route_for(&provenance(), Some(&active(b"OP1"))).unwrap();
        let after = route_for(&provenance(), Some(&active(b"OP2"))).unwrap();

        assert_ne!(before, after);
        assert_eq!(
            before.provenance, after.provenance,
            "both routes carry the same accepted provenance"
        );
        assert_eq!(after.operational_profile, profile(b"OP2"));
    }

    #[test]
    fn an_absent_or_unbound_binding_refuses_without_claiming_corruption() {
        assert_eq!(
            route_for(&provenance(), None),
            Err(RouteRefusal::NotRoutable)
        );
        let withdrawn = bound(
            semantics(b"SD1"),
            profile(b"OP1"),
            ProviderBindingLifecycle::Unbound,
        );
        assert_eq!(
            route_for(&provenance(), Some(&withdrawn)),
            Err(RouteRefusal::NotRoutable)
        );
    }

    #[test]
    fn redefined_semantics_refuse_rather_than_silently_re_pointing() {
        // Sending new work under semantics the state was never observed under
        // would attribute it to an agreement that has since changed.
        let redefined = bound(
            semantics(b"SD2"),
            profile(b"OP1"),
            ProviderBindingLifecycle::Active,
        );
        assert_eq!(
            route_for(&provenance(), Some(&redefined)),
            Err(RouteRefusal::SemanticsRedefined)
        );
    }

    #[test]
    fn the_profile_comes_from_the_binding_not_from_the_caller() {
        // There is no parameter through which a caller could supply a stale
        // profile, which is what keeps the "from now" half honest.
        let route = route_for(&provenance(), Some(&active(b"OP-current"))).unwrap();
        assert_eq!(route.operational_profile, profile(b"OP-current"));
    }
}
