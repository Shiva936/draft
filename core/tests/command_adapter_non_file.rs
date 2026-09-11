//! The generic command adapter, driven against a non-`file` scheme.
//!
//! These are the port's own guarantees, checked one at a time: an authorized
//! adapter observes, an unauthorized one cannot observe at all, content comes
//! back only through the declared operation and only within its bound, a stale
//! generation is refused rather than served, and a mutation is authored by
//! Draft and merely carried out by the adapter. Each is a promise Core makes to
//! every adapter, so each is checked against one that Core was never built for.

#![cfg(unix)]

use draft_core::dcg::resource::{ObservedRef, ResourceLocator};
use draft_core::dcg::source::{MutationStep, ResourceMutationPlan, ResourceSource, ViewRules};

mod support;
use support::catalog::{fixture, source};

#[test]
fn a_non_file_domain_is_observed_without_core_knowing_anything_about_it() {
    let fixture = fixture();
    let adapter = source(&fixture, true);

    let outcome = adapter.enumerate(&ViewRules::default()).unwrap();
    assert_eq!(outcome.resources.len(), 4);

    // Coverage domains are the adapter's own names, scoped to its binding.
    // Nothing here is `root`, and nothing is derived from a path.
    let domains: Vec<String> = outcome
        .coverage
        .iter()
        .map(|coverage| coverage.domain.local_id.0.clone())
        .collect();
    assert_eq!(domains, vec!["north".to_string(), "south".to_string()]);
    for coverage in &outcome.coverage {
        assert_eq!(coverage.domain.adapter_binding_id.0, "ext.catalog");
    }

    // Every resource carries a mandatory state digest that Core computed. The
    // adapter had nowhere to assert one.
    for observed in &outcome.resources {
        assert_eq!(observed.state.locator.scheme, "catalog");
        assert!(!observed.state.state_digest.is_empty());
        assert!(
            !observed.state.locator.body.contains('/'),
            "these locators are SKUs, not paths: {}",
            observed.state.locator.body
        );
    }

    // Two resources with different declared state have different identity.
    let a100 = outcome
        .resources
        .iter()
        .find(|observed| observed.state.locator.body == "A-100")
        .unwrap();
    let b100 = outcome
        .resources
        .iter()
        .find(|observed| observed.state.locator.body == "B-100")
        .unwrap();
    assert_ne!(a100.state.state_digest, b100.state.state_digest);
}

#[test]
fn an_unauthorized_adapter_cannot_observe_at_all() {
    let fixture = fixture();
    // Installed and trusted, but with no grant to execute. Trust and authority
    // are separate facts, and this is the one that gates running a process.
    let adapter = source(&fixture, false);
    let error = adapter.enumerate(&ViewRules::default()).unwrap_err();
    assert_eq!(
        error.kind,
        draft_core::support::error::DraftErrorKind::CapabilityNotAuthorized
    );
}

#[test]
fn bounded_content_comes_back_through_the_declared_operation() {
    let fixture = fixture();
    let adapter = source(&fixture, true);
    let observed = adapter
        .describe(&ResourceLocator::new("catalog", "A-100"))
        .unwrap();

    let bytes = adapter
        .read_range(&observed.observed_ref(), 0, 1024)
        .unwrap();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded["name"], "widget");

    // A read past the bound is refused before anything is allocated for it.
    let error = adapter
        .read_range(&observed.observed_ref(), 0, u64::MAX)
        .unwrap_err();
    assert_eq!(
        error.kind,
        draft_core::support::error::DraftErrorKind::Validation
    );
}

#[test]
fn a_stale_generation_is_refused_rather_than_served() {
    let fixture = fixture();
    let adapter = source(&fixture, true);
    let observed = adapter
        .describe(&ResourceLocator::new("catalog", "A-100"))
        .unwrap();
    let stale: ObservedRef = observed.observed_ref();

    // Somebody else changes the catalogue between observation and access.
    let mut data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.store_path).unwrap()).unwrap();
    data["north"]["A-100"]["price"] = serde_json::json!(999);
    std::fs::write(&fixture.store_path, serde_json::to_string(&data).unwrap()).unwrap();

    // Content from the new generation must not be served under the old state's
    // identity, whatever the adapter would have been willing to return.
    let error = adapter.read_range(&stale, 0, 1024).unwrap_err();
    assert_eq!(
        error.kind,
        draft_core::support::error::DraftErrorKind::ConflictDetected
    );
}

#[test]
fn draft_authors_the_mutation_and_the_adapter_carries_it_out() {
    let fixture = fixture();
    let adapter = source(&fixture, true);

    // The plan is Draft's: its operation id, its attribution, its
    // preconditions. The adapter is told what to do, never asked what it may do.
    let plan = ResourceMutationPlan {
        operation_id: draft_core::support::common::OperationId::generate(),
        attribution: draft_core::execution::workspace::EditAttribution::Task { id: "tsk_1".into() },
        preconditions: Vec::new(),
        steps: vec![MutationStep::Remove {
            locator: ResourceLocator::new("catalog", "A-200"),
            recursive: false,
        }],
    };
    let outcome = adapter.mutate(&plan).unwrap();
    assert_eq!(outcome.resources_changed.len(), 1);
    assert_eq!(outcome.resources_changed[0].body, "A-200");

    // And the domain actually changed.
    let after = adapter.enumerate(&ViewRules::default()).unwrap();
    assert_eq!(after.resources.len(), 3);
    assert!(!after
        .resources
        .iter()
        .any(|observed| observed.state.locator.body == "A-200"));
}
