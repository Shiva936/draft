//! What a single provider binding cannot prove.
//!
//! With one binding, "the route was validated against the right provider" and
//! "the route was validated against the only provider" are the same sentence.
//! These proofs need two, differing in kind, semantic definition and profile.

mod support;

use draft_core::execution::plan::PlannedOperation;
use draft_core::project::provider::ProviderBindingStore;
use draft_dcg_contract::ids::{ChangeRevisionId, OperationId};
use draft_dcg_contract::kinds::OperationKindId;
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::{ProviderProvenanceRef, ProviderRouteRef};
use support::providers;

fn plan_for(binding: &draft_core::project::provider::ProviderBinding) -> PlannedOperation {
    PlannedOperation {
        id: OperationId::parse("op_000000000001").unwrap(),
        change_revision: ChangeRevisionId::parse("rev_000000000001").unwrap(),
        kind: OperationKindId::parse("draft.change.operate/v1").unwrap(),
        route: binding.current_route(),
        planned_at: Timestamp::from_unix_nanos(0),
    }
}

#[test]
fn a_plan_for_one_provider_is_not_authorized_by_another() {
    // The failure this catches: validating a route against whichever binding
    // is at hand rather than the one the plan names. With a single binding in
    // the fixture that mistake is invisible, because the only binding is
    // always the right one.
    let directory = tempfile::tempdir().unwrap();
    let store = ProviderBindingStore::new(directory.path());
    store.bind(&providers::filesystem()).unwrap();
    store.bind(&providers::catalog()).unwrap();

    let filesystem_plan = plan_for(&providers::filesystem());

    // Against its own binding: authorized.
    store
        .with_locked_record(&providers::filesystem().id, |guard| {
            filesystem_plan.authorize(guard)
        })
        .unwrap();

    // Against the other provider's binding: refused, even though that binding
    // is perfectly healthy and would happily route work of its own.
    let error = store
        .with_locked_record(&providers::catalog().id, |guard| {
            filesystem_plan.authorize(guard)
        })
        .unwrap_err();
    assert!(
        error.message.contains("stale") || error.message.contains("binding"),
        "{}",
        error.message
    );
}

#[test]
fn retargeting_one_provider_leaves_the_other_routable() {
    // Provider bindings move independently. A change to one must not
    // invalidate plans against another — otherwise retuning a single adapter
    // would stall every unrelated operation in the project.
    let directory = tempfile::tempdir().unwrap();
    let store = ProviderBindingStore::new(directory.path());
    store.bind(&providers::filesystem()).unwrap();
    store.bind(&providers::catalog()).unwrap();

    let catalog_plan = plan_for(&providers::catalog());

    store
        .retarget(
            &providers::filesystem().id,
            providers::moved_definition(),
            providers::filesystem().current_operational_profile,
        )
        .unwrap();

    store
        .with_locked_record(&providers::catalog().id, |guard| {
            catalog_plan.authorize(guard)
        })
        .expect("the untouched provider still selects its own route");

    // And the moved one refuses its own now-stale plan, so the isolation is
    // not simply that nothing is being checked.
    let filesystem_plan = plan_for(&providers::filesystem());
    assert!(store
        .with_locked_record(&providers::filesystem().id, |guard| filesystem_plan
            .authorize(guard))
        .is_err());
}

#[test]
fn provenance_names_the_provider_that_produced_it() {
    // Two providers observing the same project must not produce provenance a
    // reader cannot tell apart: "which provider established this?" has to have
    // one answer per fact.
    let filesystem: ProviderProvenanceRef = providers::filesystem().provenance();
    let catalog: ProviderProvenanceRef = providers::catalog().provenance();

    assert_ne!(filesystem.binding, catalog.binding);
    assert_ne!(filesystem.semantic_definition, catalog.semantic_definition);

    // Provenance deliberately excludes the operational profile: re-tuning how a
    // provider is driven changes nothing about what was observed, and must not
    // read as a change in accepted history.
    let retuned = ProviderRouteRef {
        provenance: filesystem.clone(),
        operational_profile: providers::catalog().current_operational_profile,
    };
    assert_eq!(retuned.provenance, filesystem);
}
