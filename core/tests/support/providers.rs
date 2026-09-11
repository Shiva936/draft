//! Two provider bindings, so multi-provider behaviour can be tested at all.
//!
//! A single binding cannot distinguish "this rule holds" from "this rule
//! happens to hold when there is only one of everything". Several of the
//! guarantees in §2.19–§2.20 are precisely about *which* provider a fact came
//! from, and they are trivially satisfied when there is nothing to confuse.
//!
//! So there are two, deliberately different in kind, semantic definition and
//! operational profile. Tests that need to prove a fact is attributed to the
//! right provider, or that moving one binding leaves the other alone, use both.

use draft_core::project::provider::{ProviderBinding, ProviderBindingLifecycle};
use draft_dcg_contract::ids::{ProjectId, ProviderBindingId};
use draft_dcg_contract::{
    Digest, ProviderKindId, ProviderOperationalProfileDigest, ProviderSemanticDefinitionDigest,
};

pub fn project() -> ProjectId {
    ProjectId::parse("prj_000000000001").unwrap()
}

fn definition(seed: &[u8]) -> ProviderSemanticDefinitionDigest {
    ProviderSemanticDefinitionDigest::new(Digest::of_bytes(seed))
}

fn profile(seed: &[u8]) -> ProviderOperationalProfileDigest {
    ProviderOperationalProfileDigest::new(Digest::of_bytes(seed))
}

/// The first double: a local filesystem provider.
pub fn filesystem() -> ProviderBinding {
    ProviderBinding {
        generation: 0,
        id: ProviderBindingId::parse("pbd_000000000001").unwrap(),
        project: project(),
        kind: ProviderKindId::parse("draft.filesystem/local").unwrap(),
        current_semantic_definition: definition(b"filesystem-semantics"),
        current_operational_profile: profile(b"filesystem-profile"),
        lifecycle: ProviderBindingLifecycle::Active,
    }
}

/// The second double: a different kind entirely.
///
/// Not a second filesystem binding. A double that differs only by id would let
/// a test pass while the code confused *kinds*, which is the confusion that
/// matters — a catalog provider and a filesystem provider observe different
/// things and must never be substituted for one another.
pub fn catalog() -> ProviderBinding {
    ProviderBinding {
        generation: 0,
        id: ProviderBindingId::parse("pbd_000000000002").unwrap(),
        project: project(),
        kind: ProviderKindId::parse("draft.catalog/records").unwrap(),
        current_semantic_definition: definition(b"catalog-semantics"),
        current_operational_profile: profile(b"catalog-profile"),
        lifecycle: ProviderBindingLifecycle::Active,
    }
}

/// A semantic definition digest distinct from either double's current one.
pub fn moved_definition() -> ProviderSemanticDefinitionDigest {
    definition(b"a-definition-neither-double-points-at")
}
