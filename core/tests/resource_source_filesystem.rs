//! Draft's own observer is an adapter like any other.
//!
//! The filesystem implementation is Core, not an extension: it needs no
//! package, no attestation and no grant, and its observation provenance says
//! so. What it does *not* get is a private path into Draft. It is reached
//! through [`ResourceSource`] exactly as a contributed adapter is, and it sits
//! in the same registry keyed by the same scheme rule.
//!
//! That is not tidiness. A built-in observer with a shortcut is how a port ends
//! up shaped around one implementation: whatever the filesystem needs quietly
//! becomes reachable without the contract, and the first real adapter discovers
//! the contract was never sufficient. Driving Draft's own observer through the
//! same trait is what keeps the contract honest — a capability the filesystem
//! enjoys is one the port offers everybody.

use draft_core::app::App;
use draft_core::dcg::filesystem_source::FilesystemSource;
use draft_core::dcg::resource::{ContentAccess, ObservedRef, ResourceLocator};
use draft_core::dcg::source::{
    MutationPrecondition, MutationStep, ResourceMutationPlan, ResourceSource,
    ResourceSourceRegistry, ViewRules,
};
use draft_core::project::Workspace;
use draft_core::support::error::DraftErrorKind;
use draft_extension_contract::ObservationConsistency;

mod support;
use support::lifecycle::global_home;

fn project() -> (tempfile::TempDir, Workspace) {
    global_home("resource-source-filesystem");
    let directory = tempfile::tempdir().unwrap();
    App::new().init(directory.path()).unwrap();
    std::fs::write(directory.path().join("app.txt"), "hello\n").unwrap();
    std::fs::create_dir_all(directory.path().join("cache")).unwrap();
    std::fs::write(directory.path().join("cache/blob.bin"), "cached\n").unwrap();
    let layout = draft_core::project::layout::DraftLayout::for_root(directory.path());
    let workspace = Workspace {
        workspace_id: draft_dcg_contract::ids::ProjectId::parse("prj_port-test").unwrap(),
        root: directory.path().to_path_buf(),
        layout,
    };
    (directory, workspace)
}

/// Observe one locator through the port and hand back a live reference.
fn observe(source: &FilesystemSource, body: &str) -> ObservedRef {
    let locator = ResourceLocator::file(body);
    let observed = source.describe(&locator).unwrap();
    ObservedRef {
        resource_id: observed.state.resource_id.clone(),
        locator,
        expected_state_digest: observed.state.state_digest.clone(),
        observation_token: observed.observation_token.clone(),
    }
}

#[test]
fn the_built_in_observer_enumerates_through_the_port() {
    let (_project, workspace) = project();
    let source = FilesystemSource::new(&workspace);

    assert_eq!(source.scheme(), "file");
    let outcome = source
        .enumerate(&ViewRules {
            exclusions: Vec::new(),
        })
        .unwrap();

    let bodies: Vec<&str> = outcome
        .resources
        .iter()
        .map(|observed| observed.state.locator.body.as_str())
        .collect();
    assert!(bodies.contains(&"app.txt"));
    assert!(bodies.contains(&"cache/blob.bin"));

    // Every resource carries a mandatory state digest and belongs to exactly
    // one of this adapter's own coverage domains, scoped to its binding.
    for observed in &outcome.resources {
        assert!(!observed.state.state_digest.is_empty());
        assert_eq!(
            observed.coverage_domain.adapter_binding_id,
            source.binding_id()
        );
    }
    assert!(outcome
        .coverage
        .iter()
        .all(|coverage| coverage.domain.adapter_binding_id == source.binding_id()));
    assert!(
        outcome.gaps.is_empty(),
        "a readable project has nothing it could not establish"
    );

    // Draft's control plane is not project state, and no view rule is consulted
    // about that.
    assert!(
        !bodies.iter().any(|body| body.starts_with(".draft/")),
        "the control plane never enters the observed universe"
    );
}

#[test]
fn a_view_rule_narrows_the_universe_rather_than_deleting_from_it() {
    let (_project, workspace) = project();
    let source = FilesystemSource::new(&workspace);

    let unrestricted = source
        .enumerate(&ViewRules {
            exclusions: Vec::new(),
        })
        .unwrap();
    let excluded = source
        .enumerate(&ViewRules {
            exclusions: vec![draft_extension_contract::ResourceRule {
                predicate: draft_extension_contract::RawResourcePredicate::PathGlob {
                    glob: "cache/**".into(),
                },
                reason: "not authored project state".into(),
            }],
        })
        .unwrap();

    let bodies: Vec<&str> = excluded
        .resources
        .iter()
        .map(|observed| observed.state.locator.body.as_str())
        .collect();
    assert!(bodies.contains(&"app.txt"));
    assert!(!bodies.contains(&"cache/blob.bin"));

    // A view rule removes the resource from the universe rather than marking
    // it skipped, and the three outcomes stay distinct. It is not a gap —
    // nothing failed — and it is not counted as ignored, which is what
    // `.draftignore` produces. "Draft never looked here, by an adopted rule"
    // is a different fact from "Draft looked and could not see", and only the
    // second one may bound what can be proved absent.
    assert!(
        excluded.gaps.is_empty(),
        "an exclusion is a decision, not a failure to observe"
    );
    assert!(excluded.untrackable.is_empty());
    assert!(
        excluded.resources.len() < unrestricted.resources.len(),
        "the universe is genuinely narrower"
    );

    // And the decision is recorded where it belongs: in the semantics, not in
    // a per-scan counter. Re-running under the same rules gives the same
    // universe, which is what makes the context digest meaningful.
    let again = source
        .enumerate(&ViewRules {
            exclusions: vec![draft_extension_contract::ResourceRule {
                predicate: draft_extension_contract::RawResourcePredicate::PathGlob {
                    glob: "cache/**".into(),
                },
                reason: "not authored project state".into(),
            }],
        })
        .unwrap();
    assert_eq!(again.resources.len(), excluded.resources.len());
}

#[test]
fn content_is_fenced_to_the_generation_that_was_observed() {
    let (project, workspace) = project();
    let source = FilesystemSource::new(&workspace);

    // This adapter declares the weaker fencing guarantee, which is why Draft
    // revalidates the digest rather than trusting the token.
    assert_eq!(
        source.capabilities().observation_consistency,
        ObservationConsistency::BestEffortGeneration
    );

    let observed = observe(&source, "app.txt");
    match source.content_access(&observed).unwrap() {
        ContentAccess::Ranged { length, .. } => assert_eq!(length, 6),
        other => panic!("a byte resource must offer ranged access, got {other:?}"),
    }
    assert_eq!(source.read_range(&observed, 0, 5).unwrap(), b"hello");

    // Move the resource on. The old reference now names a generation that is
    // gone, and serving its bytes would attribute them to a state they did not
    // come from.
    std::fs::write(project.path().join("app.txt"), "goodbye\n").unwrap();
    let error = source.read_range(&observed, 0, 5).unwrap_err();
    assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    assert!(error.message.contains("changed since it was observed"));
    assert!(source.content_access(&observed).is_err());
}

#[test]
fn a_mutation_is_refused_when_its_precondition_no_longer_holds() {
    let (project, workspace) = project();
    let source = FilesystemSource::new(&workspace);
    let observed = observe(&source, "app.txt");

    // Draft authors the plan — the operation id, the attribution, the
    // preconditions. The adapter only carries it out.
    let plan = |precondition| ResourceMutationPlan {
        operation_id: draft_core::support::common::OperationId::new("op_test"),
        attribution: draft_core::support::common::EditAttribution::Task {
            id: "tsk_test".into(),
        },
        preconditions: vec![precondition],
        steps: vec![MutationStep::SetContent {
            locator: ResourceLocator::file("app.txt"),
            content: b"written\n".to_vec(),
        }],
    };

    // Somebody else changed it first.
    std::fs::write(project.path().join("app.txt"), "raced\n").unwrap();
    let error = source
        .mutate(&plan(MutationPrecondition::StateEquals(observed.clone())))
        .unwrap_err();
    assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    assert_eq!(
        std::fs::read_to_string(project.path().join("app.txt")).unwrap(),
        "raced\n",
        "a refused mutation must not have applied any of its steps"
    );

    // Re-observe, and the same plan applies.
    let fresh = observe(&source, "app.txt");
    let outcome = source
        .mutate(&plan(MutationPrecondition::StateEquals(fresh)))
        .unwrap();
    assert_eq!(
        outcome.resources_changed,
        vec![ResourceLocator::file("app.txt")]
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join("app.txt")).unwrap(),
        "written\n"
    );
}

#[test]
fn the_registry_owns_one_adapter_per_scheme() {
    let (_project, workspace) = project();

    let mut registry = ResourceSourceRegistry::default();
    registry
        .register(Box::new(FilesystemSource::new(&workspace)))
        .unwrap();
    assert_eq!(registry.schemes(), vec!["file".to_string()]);
    assert_eq!(registry.for_scheme("file").unwrap().scheme(), "file");

    // A second claim on the same scheme is a conflict Draft reports, never one
    // it settles by registration order.
    let error = registry
        .register(Box::new(FilesystemSource::new(&workspace)))
        .unwrap_err();
    assert_eq!(error.kind, DraftErrorKind::ConflictDetected);

    // And a scheme nobody owns is an unavailable capability, not an empty
    // result that would read as "there is nothing there".
    assert!(registry.for_scheme("catalog").is_none());
    let Err(error) = registry.resolve(&ResourceLocator::new("catalog", "A-100")) else {
        panic!("an unowned scheme must not resolve to an adapter");
    };
    assert_eq!(error.kind, DraftErrorKind::CapabilityUnavailable);
}
