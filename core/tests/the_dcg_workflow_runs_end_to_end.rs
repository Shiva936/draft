//! The whole DCG workflow, driven only through the application boundary.
//!
//! Every call here is one a surface makes. Nothing reaches a domain store, and
//! nothing constructs a Promotion, a Publication or a Baseline itself — which
//! is the property under test as much as the workflow is. If a surface could
//! reach past these methods, it could hold a second opinion about what the
//! project accepts.
//!
//! ```text
//! open_change → seal → verify → assess → gate → decide → promote → publish
//!                                                   │         │        │
//!                                            authorizes   accepts   delivers
//! ```

mod support;

use draft_core::app::promotion::PromotionOutcome;
use draft_core::app::publish::PublishOutcome;
use draft_core::app::workflow::OperationState;
use draft_core::app::App;
use draft_core::evidence::EvidenceOutcome;
use draft_dcg_contract::publication::PublicationOutcomeKind;

/// A project with an accepted initial Baseline and one file to change.
struct Project {
    _directory: tempfile::TempDir,
    app: App,
    root: std::path::PathBuf,
}

impl Project {
    fn new() -> Self {
        static GLOBAL_HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        let home = GLOBAL_HOME.get_or_init(|| tempfile::tempdir().unwrap());
        std::env::set_var("DRAFT_GLOBAL_HOME", home.path());

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        std::fs::write(root.join("a.txt"), "hello").unwrap();

        let app = App::new();
        app.init_with_base(&root, "base change").unwrap();
        Self {
            _directory: directory,
            app,
            root,
        }
    }

    /// The scope every change here declares: the one file the project holds.
    fn scope(&self) -> Vec<String> {
        self.app
            .dcg_baseline(&self.root)
            .unwrap()
            .expect("initialization accepts a Baseline")
            .composition
            .keys()
            .map(ToString::to_string)
            .collect()
    }

    /// Everything up to and including a satisfied gate.
    fn authorize(&self, intent: &str) -> (String, String, String) {
        let change = self
            .app
            .dcg_open_change(&self.root, intent, &self.scope())
            .unwrap();
        // A revision proposes a change, so there has to be one. The intent is
        // the content, which keeps two differently-intended revisions from
        // colliding on one state root.
        std::fs::write(self.root.join("a.txt"), intent).unwrap();
        let revision = self.app.dcg_seal(&self.root, change.id.as_str()).unwrap();
        let evidence = self
            .app
            .dcg_verify(&self.root, revision.id.as_str())
            .unwrap();
        // The project declares no checks and no extension contributes one for
        // this resource, so there was nothing to ask in the first place.
        // `Unavailable` is not a pass — that distinction is what the gate then
        // acts on, and it is the whole reason the outcome is five-valued
        // rather than a boolean. It is also not `NotApplicable`: only this one
        // tells the reader that installing something would change the answer.
        assert_eq!(evidence.outcome, EvidenceOutcome::Unavailable);
        self.app
            .dcg_assess(&self.root, revision.id.as_str(), "low", "reviewed")
            .unwrap();
        let gate = self
            .app
            .dcg_evaluate_gate(&self.root, revision.id.as_str(), &[])
            .unwrap();
        (
            change.id.to_string(),
            revision.id.to_string(),
            gate.id.clone(),
        )
    }
}

/// A project whose one configured check always passes.
fn with_a_passing_check(project: &Project) {
    std::fs::write(
        project.root.join(".draft/verify.toml"),
        r#"schema_version = 1

[[checks]]
name = "always"
enabled = true

[checks.command]
program = "true"
args = []
"#,
    )
    .unwrap();
}

#[test]
fn the_workflow_reaches_a_baseline_and_then_publishes_it() {
    let project = Project::new();
    with_a_passing_check(&project);
    let initial = project
        .app
        .dcg_baseline(&project.root)
        .unwrap()
        .unwrap()
        .baseline;

    let change = project
        .app
        .dcg_open_change(&project.root, "change the file", &project.scope())
        .unwrap();
    std::fs::write(project.root.join("a.txt"), "changed").unwrap();

    let revision = project
        .app
        .dcg_seal(&project.root, change.id.as_str())
        .unwrap();
    let evidence = project
        .app
        .dcg_verify(&project.root, revision.id.as_str())
        .unwrap();
    assert_eq!(
        evidence.outcome,
        EvidenceOutcome::Passed,
        "a configured check that passes over observed inputs is a pass"
    );

    project
        .app
        .dcg_assess(&project.root, revision.id.as_str(), "low", "reviewed")
        .unwrap();
    let gate = project
        .app
        .dcg_evaluate_gate(&project.root, revision.id.as_str(), &[])
        .unwrap();
    assert!(
        gate.is_satisfied(),
        "the gate should be satisfied: {gate:?}"
    );

    // A satisfied gate is not authority. The Baseline has not moved.
    assert_eq!(
        project
            .app
            .dcg_baseline(&project.root)
            .unwrap()
            .unwrap()
            .baseline,
        initial,
        "evaluating a gate does not change what the project accepts"
    );

    let decision = project
        .app
        .dcg_decide(
            &project.root,
            revision.id.as_str(),
            Some(&gate.id),
            true,
            None,
        )
        .unwrap();

    // Nor does a decision. Deciding authorizes; promotion accepts.
    assert_eq!(
        project
            .app
            .dcg_baseline(&project.root)
            .unwrap()
            .unwrap()
            .baseline,
        initial,
        "an approving decision authorizes a promotion; it does not perform one"
    );

    let outcome = project
        .app
        .dcg_promote(
            &project.root,
            change.id.as_str(),
            revision.id.as_str(),
            decision.id.as_str(),
            &gate.id,
            Some(&initial.digest().to_string()),
        )
        .unwrap();
    let promoted = match &outcome {
        PromotionOutcome::Promoted { baseline, .. } => baseline.clone(),
        other => panic!("expected a promotion, got {other:?}"),
    };
    assert_ne!(promoted, initial, "promotion advances the Baseline");

    // The promotion projects as completed, and names what it accepted.
    let view = project
        .app
        .dcg_promotion(&project.root, outcome.promotion().as_str())
        .unwrap()
        .unwrap();
    assert_eq!(view.state, OperationState::Completed);
    assert_eq!(view.baseline, Some(promoted.clone()));

    // Publishing is its own capability. Nobody has granted it, so the effect
    // is refused before anything external happens — being permitted to accept
    // work into a Baseline is not being permitted to announce it.
    let ungranted = project
        .app
        .dcg_publish(&project.root, None, "draft.publish/export", "req-0", None)
        .unwrap_err();
    assert_eq!(
        ungranted.kind,
        draft_core::support::error::DraftErrorKind::CapabilityNotAuthorized,
        "got {ungranted:?}"
    );

    project.app.dcg_grant_publish(&project.root).unwrap();

    // Publication delivers it, and changes nothing about it.
    let published = project
        .app
        .dcg_publish(&project.root, None, "draft.publish/export", "req-1", None)
        .unwrap();
    assert!(
        matches!(
            published,
            PublishOutcome::Concluded {
                outcome: PublicationOutcomeKind::Succeeded { .. },
                ..
            }
        ),
        "expected a successful delivery, got {published:?}"
    );
    assert_eq!(
        project
            .app
            .dcg_baseline(&project.root)
            .unwrap()
            .unwrap()
            .baseline,
        promoted,
        "publication has no authority over what the project accepts"
    );
}

#[test]
fn a_gate_that_is_not_satisfied_stops_the_decision_and_therefore_the_promotion() {
    // No check is configured and nothing contributes one, so there was
    // nothing to ask: `Unavailable`, which is not a pass. The gate must
    // refuse, and the decision with it.
    let project = Project::new();
    let (_, revision, gate) = project.authorize("unverifiable work");

    let evaluation = project
        .app
        .dcg_evaluate_gate(&project.root, &revision, &[])
        .unwrap();
    assert!(!evaluation.is_satisfied());
    assert_eq!(evaluation.id, gate);

    let error = project
        .app
        .dcg_decide(&project.root, &revision, Some(&gate), true, None)
        .unwrap_err();
    assert_eq!(
        error.kind,
        draft_core::support::error::DraftErrorKind::ReviewRequired
    );
}

#[test]
fn action_availability_tracks_the_workflow_stage_by_stage() {
    let project = Project::new();
    with_a_projects_check(&project);
    let initial = project
        .app
        .dcg_baseline(&project.root)
        .unwrap()
        .unwrap()
        .baseline;

    // Nothing has been promoted, so the accepted Baseline is the project's
    // initial one and there is nothing authorized to deliver.
    let overview = project.app.dcg_project(&project.root).unwrap();
    assert_eq!(
        availability(&overview.actions, "publish"),
        Some(false),
        "the initial Baseline was never promoted, so publishing it is not offered"
    );

    let change = project
        .app
        .dcg_open_change(&project.root, "staged work", &project.scope())
        .unwrap();
    std::fs::write(project.root.join("a.txt"), "staged").unwrap();
    let revision = project
        .app
        .dcg_seal(&project.root, change.id.as_str())
        .unwrap();

    // Before any assessment: gating is not offered, and neither is deciding.
    let view = project
        .app
        .dcg_authorization(&project.root, change.id.as_str(), revision.id.as_str())
        .unwrap();
    assert_eq!(availability(&view.actions, "assess"), Some(true));
    assert_eq!(availability(&view.actions, "evaluate_gate"), Some(false));
    assert_eq!(availability(&view.actions, "decide"), Some(false));
    assert_eq!(availability(&view.actions, "promote"), Some(false));

    project
        .app
        .dcg_verify(&project.root, revision.id.as_str())
        .unwrap();
    project
        .app
        .dcg_assess(&project.root, revision.id.as_str(), "low", "reviewed")
        .unwrap();

    // With an assessment, gating becomes available; deciding still does not.
    let view = project
        .app
        .dcg_authorization(&project.root, change.id.as_str(), revision.id.as_str())
        .unwrap();
    assert_eq!(availability(&view.actions, "evaluate_gate"), Some(true));
    assert_eq!(availability(&view.actions, "decide"), Some(false));

    let gate = project
        .app
        .dcg_evaluate_gate(&project.root, revision.id.as_str(), &[])
        .unwrap();
    assert!(gate.is_satisfied());

    // With a satisfied gate, deciding becomes available; promoting does not,
    // because no decision exists yet.
    let view = project
        .app
        .dcg_authorization(&project.root, change.id.as_str(), revision.id.as_str())
        .unwrap();
    assert_eq!(availability(&view.actions, "decide"), Some(true));
    assert_eq!(availability(&view.actions, "promote"), Some(false));

    project
        .app
        .dcg_decide(
            &project.root,
            revision.id.as_str(),
            Some(&gate.id),
            true,
            None,
        )
        .unwrap();

    let view = project
        .app
        .dcg_authorization(&project.root, change.id.as_str(), revision.id.as_str())
        .unwrap();
    assert_eq!(
        availability(&view.actions, "promote"),
        Some(true),
        "an approving decision over a satisfied gate makes promotion available"
    );

    let decision = view
        .approving_decision()
        .expect("the decision is on record");
    project
        .app
        .dcg_promote(
            &project.root,
            change.id.as_str(),
            revision.id.as_str(),
            decision.id.as_str(),
            &gate.id,
            Some(&initial.digest().to_string()),
        )
        .unwrap();

    // After promotion the Change is completed, so the same work is not
    // offered for promotion again — and publishing becomes available.
    let view = project
        .app
        .dcg_authorization(&project.root, change.id.as_str(), revision.id.as_str())
        .unwrap();
    assert_eq!(availability(&view.actions, "promote"), Some(false));

    let overview = project.app.dcg_project(&project.root).unwrap();
    assert_eq!(availability(&overview.actions, "publish"), Some(true));
}

fn with_a_projects_check(project: &Project) {
    with_a_passing_check(project);
}

fn availability(
    actions: &[draft_core::app::workflow::ActionAvailability],
    name: &str,
) -> Option<bool> {
    actions
        .iter()
        .find(|action| action.action == name)
        .map(|action| action.available)
}

#[test]
fn a_rejected_decision_authorizes_nothing() {
    let project = Project::new();
    with_a_passing_check(&project);
    let initial = project
        .app
        .dcg_baseline(&project.root)
        .unwrap()
        .unwrap()
        .baseline;

    let change = project
        .app
        .dcg_open_change(&project.root, "work to refuse", &project.scope())
        .unwrap();
    std::fs::write(project.root.join("a.txt"), "refused").unwrap();
    let revision = project
        .app
        .dcg_seal(&project.root, change.id.as_str())
        .unwrap();
    project
        .app
        .dcg_verify(&project.root, revision.id.as_str())
        .unwrap();
    project
        .app
        .dcg_assess(&project.root, revision.id.as_str(), "low", "reviewed")
        .unwrap();
    let gate = project
        .app
        .dcg_evaluate_gate(&project.root, revision.id.as_str(), &[])
        .unwrap();
    assert!(gate.is_satisfied());

    // Rejecting needs no gate: refusing work is legitimate whatever the checks
    // say. What it must not do is authorize anything.
    let decision = project
        .app
        .dcg_decide(
            &project.root,
            revision.id.as_str(),
            None,
            false,
            Some("not this"),
        )
        .unwrap();

    let view = project
        .app
        .dcg_authorization(&project.root, change.id.as_str(), revision.id.as_str())
        .unwrap();
    assert!(
        view.approving_decision().is_none(),
        "a rejection is not an approval"
    );
    assert_eq!(availability(&view.actions, "promote"), Some(false));

    let error = project
        .app
        .dcg_promote(
            &project.root,
            change.id.as_str(),
            revision.id.as_str(),
            decision.id.as_str(),
            &gate.id,
            Some(&initial.digest().to_string()),
        )
        .unwrap_err();
    assert_eq!(
        error.kind,
        draft_core::support::error::DraftErrorKind::ReviewRequired
    );
    assert_eq!(
        project
            .app
            .dcg_baseline(&project.root)
            .unwrap()
            .unwrap()
            .baseline,
        initial
    );
}

#[test]
fn a_promotion_against_a_baseline_that_has_moved_is_refused_not_rebased() {
    // The stale-view case a surface produces: somebody read the project, went
    // away, and acted on what they saw. Rebasing would accept work onto a
    // parent nobody judged it against.
    let project = Project::new();
    with_a_passing_check(&project);
    let stale = project
        .app
        .dcg_baseline(&project.root)
        .unwrap()
        .unwrap()
        .baseline;

    // A first promotion moves the Baseline.
    let first = promote_one(&project, "first change", "one");
    assert_ne!(first, stale);

    // A second, decided against the Baseline as it was before.
    let change = project
        .app
        .dcg_open_change(&project.root, "second change", &project.scope())
        .unwrap();
    std::fs::write(project.root.join("a.txt"), "two").unwrap();
    let revision = project
        .app
        .dcg_seal(&project.root, change.id.as_str())
        .unwrap();
    project
        .app
        .dcg_verify(&project.root, revision.id.as_str())
        .unwrap();
    project
        .app
        .dcg_assess(&project.root, revision.id.as_str(), "low", "reviewed")
        .unwrap();
    let gate = project
        .app
        .dcg_evaluate_gate(&project.root, revision.id.as_str(), &[])
        .unwrap();
    let decision = project
        .app
        .dcg_decide(
            &project.root,
            revision.id.as_str(),
            Some(&gate.id),
            true,
            None,
        )
        .unwrap();

    let error = project
        .app
        .dcg_promote(
            &project.root,
            change.id.as_str(),
            revision.id.as_str(),
            decision.id.as_str(),
            &gate.id,
            Some(&stale.digest().to_string()),
        )
        .unwrap_err();
    assert_eq!(
        error.kind,
        draft_core::support::error::DraftErrorKind::StaleBaseline
    );
    assert_eq!(
        project
            .app
            .dcg_baseline(&project.root)
            .unwrap()
            .unwrap()
            .baseline,
        first,
        "the refused promotion left the accepted Baseline exactly where it was"
    );
}

#[test]
fn a_repeated_promotion_request_converges_rather_than_promoting_twice() {
    // What a surface retry after a lost reply looks like. The promotion id is
    // derived from the revision and the parent, so the second call finds the
    // promotion it already made.
    let project = Project::new();
    with_a_passing_check(&project);
    let initial = project
        .app
        .dcg_baseline(&project.root)
        .unwrap()
        .unwrap()
        .baseline;

    let change = project
        .app
        .dcg_open_change(&project.root, "work promoted twice", &project.scope())
        .unwrap();
    std::fs::write(project.root.join("a.txt"), "once").unwrap();
    let revision = project
        .app
        .dcg_seal(&project.root, change.id.as_str())
        .unwrap();
    project
        .app
        .dcg_verify(&project.root, revision.id.as_str())
        .unwrap();
    project
        .app
        .dcg_assess(&project.root, revision.id.as_str(), "low", "reviewed")
        .unwrap();
    let gate = project
        .app
        .dcg_evaluate_gate(&project.root, revision.id.as_str(), &[])
        .unwrap();
    let decision = project
        .app
        .dcg_decide(
            &project.root,
            revision.id.as_str(),
            Some(&gate.id),
            true,
            None,
        )
        .unwrap();

    let run = || {
        project.app.dcg_promote(
            &project.root,
            change.id.as_str(),
            revision.id.as_str(),
            decision.id.as_str(),
            &gate.id,
            Some(&initial.digest().to_string()),
        )
    };
    let first = run().unwrap();
    let again = run().unwrap();

    assert!(matches!(first, PromotionOutcome::Promoted { .. }));
    assert!(
        matches!(again, PromotionOutcome::AlreadyPromoted { .. }),
        "a retry converges: {again:?}"
    );
    assert_eq!(first.baseline(), again.baseline());
    assert_eq!(first.promotion(), again.promotion());
    assert_eq!(
        project
            .app
            .dcg_baseline(&project.root)
            .unwrap()
            .unwrap()
            .lineage
            .len(),
        2,
        "one promotion produced one new Baseline, however many times it was asked for"
    );
}

#[test]
fn publishing_before_promoting_is_refused() {
    // The project's initial Baseline exists but nothing decided work into it,
    // so nothing authorized delivering it anywhere.
    let project = Project::new();
    let error = project
        .app
        .dcg_publish(&project.root, None, "draft.publish/export", "req-1", None)
        .unwrap_err();
    assert_eq!(
        error.kind,
        draft_core::support::error::DraftErrorKind::ReviewRequired
    );
}

#[test]
fn a_publication_retry_under_the_same_request_converges_on_one_delivery() {
    // The §9 property, through the application boundary: an HTTP retry, an SSE
    // reconnect or a re-run command must not become a second external effect.
    let project = Project::new();
    with_a_passing_check(&project);
    let promoted = promote_one(&project, "publishable work", "one");
    project.app.dcg_grant_publish(&project.root).unwrap();

    let first = project
        .app
        .dcg_publish(
            &project.root,
            None,
            "draft.publish/export",
            "attempt-1",
            None,
        )
        .unwrap();
    assert!(first.is_delivered());

    let retry = project
        .app
        .dcg_publish(
            &project.root,
            None,
            "draft.publish/export",
            "attempt-1",
            None,
        )
        .unwrap();
    assert!(
        matches!(retry, PublishOutcome::AlreadyConcluded { .. }),
        "the same request id converges: {retry:?}"
    );
    assert_eq!(first.attempt(), retry.attempt());

    // One Publication, one delivery, and the Baseline untouched throughout.
    let publications = project.app.dcg_publications(&project.root).unwrap();
    assert_eq!(publications.len(), 1);
    assert_eq!(publications[0].state, OperationState::Completed);
    assert_eq!(publications[0].baseline, promoted);
    assert_eq!(
        project
            .app
            .dcg_baseline(&project.root)
            .unwrap()
            .unwrap()
            .baseline,
        promoted
    );
}

#[test]
fn a_failed_delivery_leaves_the_baseline_exactly_as_accepted() {
    // The invariant a Console must be able to render honestly: publication
    // failed, the Baseline did not.
    let project = Project::new();
    with_a_passing_check(&project);
    let promoted = promote_one(&project, "work that fails to deliver", "one");
    project.app.dcg_grant_publish(&project.root).unwrap();

    let workspace = project.app.open(&project.root).unwrap();
    let request = draft_core::app::publish::PublishRequest {
        baseline: promoted.clone(),
        purpose: draft_dcg_contract::publication::PublicationPurposeId::parse(
            "draft.publish/export",
        )
        .unwrap(),
        semantics: draft_core::app::publish::filesystem_delivery_semantics(),
        retry_authorization: None,
        republish_intent: None,
        request_id: "doomed".into(),
    };
    let outcome = draft_core::app::publish::publish(&workspace, &request, || {
        draft_core::publication::dispatch::DeliveryResult::Failed {
            reason: "the target refused it".into(),
        }
    })
    .unwrap();

    assert!(!outcome.is_delivered());
    assert_eq!(
        project
            .app
            .dcg_baseline(&project.root)
            .unwrap()
            .unwrap()
            .baseline,
        promoted,
        "delivery has no authority over what the project accepts"
    );
    let publications = project.app.dcg_publications(&project.root).unwrap();
    assert_eq!(publications[0].state, OperationState::Failed);
    assert_eq!(
        publications[0].baseline, promoted,
        "the failed publication still names the Baseline it was delivering, which is still accepted"
    );
}

#[test]
fn a_publication_interrupted_mid_delivery_is_resumed_rather_than_repeated() {
    // The crash window, reached through the application boundary: the attempt
    // is durable and no outcome exists. A later call must not start a second
    // external effect.
    let project = Project::new();
    with_a_passing_check(&project);
    promote_one(&project, "work interrupted mid-delivery", "one");
    project.app.dcg_grant_publish(&project.root).unwrap();

    let workspace = project.app.open(&project.root).unwrap();
    let baseline = draft_core::dcg::baseline::current_baseline(&workspace.layout)
        .unwrap()
        .unwrap();
    let request = |request_id: &str| draft_core::app::publish::PublishRequest {
        baseline: baseline.clone(),
        purpose: draft_dcg_contract::publication::PublicationPurposeId::parse(
            "draft.publish/export",
        )
        .unwrap(),
        semantics: draft_core::app::publish::filesystem_delivery_semantics(),
        retry_authorization: None,
        republish_intent: None,
        request_id: request_id.into(),
    };

    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        draft_core::app::publish::publish(&workspace, &request("interrupted"), || {
            panic!("the process died mid-delivery")
        })
    }));
    assert!(crashed.is_err());

    // A different request id — a genuinely new send — is still refused until
    // the interrupted attempt is resolved.
    let blocked = draft_core::app::publish::publish(&workspace, &request("a-new-send"), || {
        panic!("a second external effect must never be attempted")
    })
    .unwrap();
    assert!(
        matches!(blocked, PublishOutcome::ResumeRequired { .. }),
        "expected the barrier to send the caller back: {blocked:?}"
    );

    let publications = project.app.dcg_publications(&project.root).unwrap();
    assert_eq!(publications[0].state, OperationState::Recovering);
}

#[test]
fn a_repository_from_an_unsupported_schema_is_refused_rather_than_read() {
    // The cutover's promise: an old repository is not migrated, guessed at, or
    // partially read. It is refused, and the fix is a fresh project.
    let project = Project::new();
    let marker = project.root.join(".draft/project.json");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
    stored["schema_version"] = serde_json::json!(0);
    std::fs::write(&marker, serde_json::to_vec(&stored).unwrap()).unwrap();

    let error = project.app.dcg_project(&project.root).unwrap_err();
    assert_eq!(
        error.kind,
        draft_core::support::error::DraftErrorKind::UnsupportedSchema,
        "got {error:?}"
    );
}

/// Take one change all the way to an accepted Baseline, and return it.
fn promote_one(project: &Project, intent: &str, content: &str) -> draft_dcg_contract::BaselineId {
    let parent = project
        .app
        .dcg_baseline(&project.root)
        .unwrap()
        .unwrap()
        .baseline;
    let change = project
        .app
        .dcg_open_change(&project.root, intent, &project.scope())
        .unwrap();
    std::fs::write(project.root.join("a.txt"), content).unwrap();
    let revision = project
        .app
        .dcg_seal(&project.root, change.id.as_str())
        .unwrap();
    project
        .app
        .dcg_verify(&project.root, revision.id.as_str())
        .unwrap();
    project
        .app
        .dcg_assess(&project.root, revision.id.as_str(), "low", "reviewed")
        .unwrap();
    let gate = project
        .app
        .dcg_evaluate_gate(&project.root, revision.id.as_str(), &[])
        .unwrap();
    let decision = project
        .app
        .dcg_decide(
            &project.root,
            revision.id.as_str(),
            Some(&gate.id),
            true,
            None,
        )
        .unwrap();
    project
        .app
        .dcg_promote(
            &project.root,
            change.id.as_str(),
            revision.id.as_str(),
            decision.id.as_str(),
            &gate.id,
            Some(&parent.digest().to_string()),
        )
        .unwrap()
        .baseline()
        .clone()
}
