//! Draft Core is a generic platform on its own.
//!
//! With no extensions installed — the state of a fresh Draft build, and of any
//! build made without `/extensions/` present at all — Core still initializes a
//! workspace, still creates and verifies packs, still records evidence, and
//! still returns the same exit-worthy outcomes. What it does not do is guess at
//! domain knowledge it was never given: it reports the artifact kinds it could
//! not interpret and moves on.

use draft_core::app::App;
use draft_core::evidence::EvidenceOutcome;
use draft_core::extension::{
    ActiveContributions, CapabilityGap, ExtensionCapabilityKind, ExtensionContributionSource,
    NoExtensions,
};

/// One global store for the whole binary.
///
/// `DRAFT_GLOBAL_HOME` is process-global, so pointing it at a fresh directory
/// per test lets concurrently running tests read one another's store. The
/// store holds identity and the registry, which these tests share happily;
/// what they must not share is a project, and each still gets its own.
fn global_home() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let root =
            std::env::temp_dir().join(format!("draft-no-extensions-global-{}", std::process::id()));
        std::env::set_var("DRAFT_GLOBAL_HOME", root);
    });
}

/// A project whose accepted Baseline holds exactly `files`.
///
/// The initial Baseline is accepted over whatever the workspace holds at
/// `init`, and a ChangePack may only declare Resources the Baseline already has —
/// so a fixture that wants to change a file has to put it there first.
fn workspace_of(files: &[(&str, &str)]) -> tempfile::TempDir {
    global_home();
    let directory = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let file = directory.path().join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(file, contents).unwrap();
    }
    App::new().init(directory.path()).unwrap();
    directory
}

fn workspace() -> tempfile::TempDir {
    workspace_of(&[])
}

/// Every Resource the accepted Baseline holds, as a ChangePack scope declaration.
///
/// A ChangePack may narrow the Baseline but never exceed it, so declaring all of
/// it is the widest scope any of these tests could legally ask for.
fn whole_baseline(app: &App, root: &std::path::Path) -> Vec<String> {
    app.dcg_baseline(root)
        .unwrap()
        .expect("initialization accepts a Baseline")
        .composition
        .keys()
        .map(ToString::to_string)
        .collect()
}

/// The Resource id Draft derives for a workspace file.
fn resource_of(path: &str) -> draft_dcg_contract::ids::ResourceId {
    draft_core::dcg::resource::resource_id_for_locator(&format!("file:{path}"))
}

#[test]
fn an_app_with_no_extensions_contributes_nothing() {
    assert!(App::new().active_contributions().is_empty());
    assert!(NoExtensions.active_contributions().is_empty());
    assert_eq!(
        App::new().active_contributions(),
        ActiveContributions::default()
    );
}

#[test]
fn verification_completes_and_says_plainly_that_nothing_checked_anything() {
    let directory = workspace_of(&[("src/auth.rs", "fn old() {}\n")]);
    let root = directory.path();
    let app = App::new();

    app.checkpoint(root, "base").unwrap();
    std::fs::write(root.join("src/auth.rs"), "pub fn validate_token() {}\n").unwrap();

    let change = app
        .dcg_open_change_pack(root, "generic-change", &whole_baseline(&app, root))
        .unwrap();
    let revision = app.dcg_seal(root, change.id.as_str()).unwrap();
    let evidence = app
        .dcg_verify(root, &revision.id.to_string())
        .expect("verification completes with no extensions installed");

    // Verification really ran and produced evidence: an immutable fact about
    // this exact revision, over the observations it was sealed from.
    assert!(evidence.covers(&revision.id));
    assert!(!evidence.inputs.is_empty());

    // Nothing checked anything — and that is emphatically not `passed`. This is
    // the distinction the five states exist for: a gate reading this as a pass
    // would approve a change nobody verified.
    //
    // The state is `unavailable` rather than `not_applicable` because there was
    // no capability to ask in the first place. Both are refused by the same
    // gate, but only this one tells the reader that installing something would
    // change the answer.
    assert_ne!(evidence.outcome, EvidenceOutcome::Passed);
    assert_eq!(evidence.outcome, EvidenceOutcome::Unavailable);
    assert!(
        !evidence.outcome.is_satisfying(),
        "the absence of a capability to check something is not evidence that \
         it is fine"
    );
}

/// The gap Core reports names what could not be interpreted, and nothing else.
///
/// Draft does not advertise on any publisher's behalf, so a gap is never a
/// place to name the extension, source or catalog that might fill it.
#[test]
fn a_capability_gap_never_names_a_publisher() {
    let gap = CapabilityGap::new(
        ExtensionCapabilityKind::Verification,
        ["src/auth.rs".to_string()],
        "no installed extension contributes a check for these resources",
    );
    assert_eq!(gap.resource_classes, vec!["src/auth.rs".to_string()]);
    let encoded = serde_json::to_value(&gap).unwrap();
    for forbidden in ["extension_id", "source_id", "package", "catalog_id"] {
        assert!(
            encoded.get(forbidden).is_none(),
            "a capability gap must not name {forbidden}"
        );
    }
}

#[test]
fn risk_is_unassessed_rather_than_low() {
    let directory = workspace_of(&[("thing.dat", "before\n")]);
    let root = directory.path();
    let app = App::new();

    app.checkpoint(root, "base").unwrap();
    std::fs::write(root.join("thing.dat"), "after\n").unwrap();
    let change = app
        .dcg_open_change_pack(root, "risky", &whole_baseline(&app, root))
        .unwrap();
    let revision = app.dcg_seal(root, change.id.as_str()).unwrap();
    app.dcg_verify(root, &revision.id.to_string()).unwrap();

    // With no contributed rules Draft has no notion of what makes a change
    // risky in an arbitrary domain. Nothing assessed it, so no Assessment
    // exists — Draft does not manufacture a `low` and let a threshold gate
    // wave the change through.
    let view = app
        .dcg_authorization(root, change.id.as_str(), &revision.id.to_string())
        .unwrap();
    assert!(
        view.assessments.is_empty(),
        "an unassessed revision carries no risk judgement: {:?}",
        view.assessments
    );

    // And the gate reads that absence as a refusal rather than a pass — by
    // name, so a reader can tell "nobody judged this" from "a check failed".
    let gate = app
        .dcg_evaluate_gate(root, &revision.id.to_string(), &[])
        .unwrap();
    assert!(
        !gate.is_satisfied(),
        "a gate over a revision nobody assessed must not be satisfied"
    );
    let assessed = gate
        .conditions
        .iter()
        .find(|condition| condition.id == "draft.gate/assessed")
        .expect("the absence of any assessment is its own named condition");
    assert!(!assessed.satisfied);
    assert!(
        !gate
            .conditions
            .iter()
            .any(|condition| condition.id == "draft.gate/risk" && condition.satisfied),
        "an unassessed revision must not satisfy the risk condition: {:?}",
        gate.conditions
    );
}

#[test]
fn resources_are_listed_and_editable_without_being_classified() {
    let directory = workspace();
    let root = directory.path();
    let app = App::new();

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/auth.rs"), "fn validate() {}\n").unwrap();
    std::fs::write(root.join("README"), "notes\n").unwrap();

    // Every resource is still there. Draft manages what it cannot interpret;
    // classification is a contributed capability, not a prerequisite.
    let tree = app.resource_tree(root).unwrap();
    let listed: Vec<&str> = tree
        .iter()
        .map(|entry| entry.locator.body.as_str())
        .collect();
    assert!(listed.contains(&"src/auth.rs"));
    assert!(listed.contains(&"README"));

    // Nothing classifies anything, and nothing is guessed from a name.
    for entry in &tree {
        assert!(
            entry.classes.is_empty(),
            "{} was classified with no extension installed",
            entry.locator.body
        );
        assert!(entry.class_collisions.is_empty());
    }

    // The report distinguishes "nothing matched" from "nothing can match".
    let report = app.classification_report(root).unwrap();
    assert!(report.assigned.is_empty());
    assert!(report.collisions.is_empty());
    assert_eq!(
        report
            .gaps
            .iter()
            .map(|gap| gap.capability)
            .collect::<Vec<_>>(),
        vec![ExtensionCapabilityKind::Classification]
    );
}

#[test]
fn observation_is_complete_and_the_control_plane_never_enters_project_state() {
    let directory = workspace_of(&[
        ("src/a.txt", "alpha\n"),
        ("b.txt", "beta\n"),
        ("a.txt", "one\n"),
    ]);
    let root = directory.path();
    let app = App::new();

    app.checkpoint(root, "base").unwrap();

    let scope = whole_baseline(&app, root);
    let change = app.dcg_open_change_pack(root, "obs", &scope).unwrap();

    // A revision proposes a change, and this workspace holds exactly what the
    // Baseline accepts. There is nothing to seal, and Draft says so rather
    // than minting an empty thing to verify, gate, decide and promote.
    let nothing = app.dcg_seal(root, change.id.as_str()).unwrap_err();
    assert_eq!(
        nothing.kind,
        draft_core::support::error::DraftErrorKind::Validation
    );

    // Sealing observes the workspace rather than asserting it, so the state
    // root before the edit and the state root after it are different facts.
    std::fs::write(root.join("a.txt"), "two\n").unwrap();
    let before = app.dcg_seal(root, change.id.as_str()).unwrap();
    std::fs::write(root.join("a.txt"), "three\n").unwrap();
    let after = app.dcg_seal(root, change.id.as_str()).unwrap();
    assert_ne!(before.project_state_root, after.project_state_root);
    assert_ne!(before.id, after.id);

    // Exactly the edited resource was touched. `.draft/**` is written on every
    // checkpoint and seal, so anything counting it would show up here — the
    // control plane is not project state and never enters a revision.
    assert_eq!(
        after.touched,
        [resource_of("a.txt")].into_iter().collect(),
        "only the edited resource is touched"
    );
    assert_eq!(before.touched, after.touched);

    // Nor is it in the Baseline the revision resolved its scope against.
    assert!(!scope.contains(&resource_of(".draft/HEAD").to_string()));
    assert!(scope.contains(&resource_of("a.txt").to_string()));

    // A revision names both authoritative sides — the Baseline it was worked
    // from and the state it proposes. One digest cannot describe a transition,
    // and neither side is inferred from the other.
    let accepted = app
        .dcg_baseline(root)
        .unwrap()
        .expect("initialization accepts a Baseline")
        .baseline;
    assert_eq!(after.base_baseline, accepted);
    assert_eq!(before.base_baseline, accepted);

    // Looking again is a new historical event, not a correction of an old one:
    // the first revision reads back exactly as it was sealed.
    let store = draft_core::dcg::revision_pack::RevisionPackStore::new(
        draft_core::project::layout::DraftLayout::for_root(root).revision_packs_dir(),
    );
    assert_eq!(store.get(&before.id).unwrap().as_ref(), Some(&before));
    assert_eq!(store.get(&after.id).unwrap().as_ref(), Some(&after));
}

/// A ChangePack may introduce a Resource the Baseline has never held.
///
/// Scope resolves against the accepted Baseline *and* the project as it stands,
/// so adding a file is as ordinary a change as editing one. Resolving against
/// the Baseline alone would drop the new file out of the scope silently — and a
/// reviewer would approve a revision whose whole point was invisible in it.
#[test]
fn a_change_can_introduce_a_resource_the_baseline_never_held() {
    let directory = workspace_of(&[("kept.txt", "unchanged\n")]);
    let root = directory.path();
    let app = App::new();

    // The file does not exist yet, so it has no id to look up. A locator is
    // how it gets named — and the same id comes back either way.
    let change = app
        .dcg_open_change_pack(root, "add a module", &["src/new.rs".to_string()])
        .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/new.rs"), "pub fn added() {}\n").unwrap();

    let revision = app.dcg_seal(root, change.id.as_str()).unwrap();
    assert_eq!(
        revision.touched,
        [resource_of("src/new.rs")].into_iter().collect(),
        "the added Resource is what the revision changed"
    );

    // The Baseline is untouched by any of this: a revision proposes, and only
    // a promotion accepts.
    let composition = whole_baseline(&app, root);
    assert!(!composition.contains(&resource_of("src/new.rs").to_string()));
}

/// Naming a Resource by path, by locator or by id is the same declaration.
///
/// Each spelling is declared under its own intent, because a ChangePack's identity
/// comes from its intent and its Baseline — three changes with one intent would
/// converge whatever the scope resolved to, and prove nothing about the
/// spelling. What must agree is the scope each one actually resolved.
#[test]
fn a_scope_entry_names_the_same_resource_by_path_or_by_id() {
    let directory = workspace_of(&[("a.txt", "one\n"), ("other.txt", "kept\n")]);
    let root = directory.path();
    let app = App::new();

    // Only `a.txt` is edited, so a spelling that failed to name it would
    // resolve to an empty scope and refuse to seal at all.
    std::fs::write(root.join("a.txt"), "two\n").unwrap();

    let mut sealed = Vec::new();
    for (intent, spelling) in [
        ("by path", "a.txt".to_string()),
        ("by locator", "file:a.txt".to_string()),
        ("by id", resource_of("a.txt").to_string()),
    ] {
        let change = app.dcg_open_change_pack(root, intent, &[spelling]).unwrap();
        let revision = app.dcg_seal(root, change.id.as_str()).unwrap();
        assert_eq!(
            revision.touched,
            [resource_of("a.txt")].into_iter().collect(),
            "'{intent}' resolved to a different scope"
        );
        sealed.push(revision);
    }

    // Three separate ChangePacks over one state: same proposal, different reasons.
    assert_eq!(sealed[0].project_state_root, sealed[1].project_state_root);
    assert_eq!(sealed[0].project_state_root, sealed[2].project_state_root);
    assert_ne!(sealed[0].change_pack, sealed[1].change_pack);
}

#[test]
fn rollback_is_complete_only_after_the_restored_state_is_re_observed() {
    let directory = workspace();
    let root = directory.path();
    let app = App::new();

    std::fs::write(root.join("kept.txt"), "original\n").unwrap();
    let checkpoint = app.checkpoint(root, "target").unwrap();

    // Change one resource and add another the target never had.
    std::fs::write(root.join("kept.txt"), "modified\n").unwrap();
    std::fs::write(root.join("extra.txt"), "new\n").unwrap();

    let plan = app.rollback_plan(root, &checkpoint.snapshot_id).unwrap();
    // The plan says, before anything runs, what it will put back and what it
    // will remove.
    assert_eq!(plan.restored_resources.len(), 1);
    assert_eq!(plan.removed_resources.len(), 1);
    assert!(plan.recovery_status.is_fully_anchored());

    let record = app.rollback(root, &checkpoint.snapshot_id, true).unwrap();
    // Complete, because the re-observed state equals the target — presence,
    // absence and every digest.
    assert_eq!(record.status, "complete", "{:?}", record.outcome);
    assert!(record.outcome.is_complete());
    assert_eq!(
        std::fs::read_to_string(root.join("kept.txt")).unwrap(),
        "original\n"
    );
    assert!(
        !root.join("extra.txt").exists(),
        "rollback restores target absence, not only target presence"
    );
}

#[test]
fn core_protects_only_its_own_control_plane() {
    let directory = workspace_of(&[(".env", "TOKEN=abc\n"), ("notes.md", "plain\n")]);
    let root = directory.path();
    let app = App::new();

    // Draft's own control plane, unconditionally.
    let protections = App::new().protections(root).unwrap();
    assert_eq!(
        protections.len(),
        1,
        "Draft owns exactly one protection: {protections:?}"
    );

    // A credential-shaped file is ordinary project state here. Whether `.env`
    // holds secrets is a judgement about a particular kind of project, and with
    // nothing installed nobody has made it. Draft tracks the resource rather
    // than pretending to knowledge it does not have.
    let tree = app.resource_tree(root).unwrap();
    let env = tree
        .iter()
        .find(|entry| entry.locator.body == ".env")
        .expect("a credential-shaped resource is still observed");
    assert!(
        !env.protected,
        "with no policy installed nothing declares .env protected"
    );

    // And the control plane is not in project state at all, whatever anyone
    // configures.
    assert!(
        !tree
            .iter()
            .any(|entry| entry.locator.body.starts_with(".draft")),
        "the control plane is never project state"
    );

    // A revision over it therefore seals, and the change is tracked normally.
    app.checkpoint(root, "base").unwrap();
    let change = app
        .dcg_open_change_pack(root, "rotate", &whole_baseline(&app, root))
        .unwrap();
    std::fs::write(root.join(".env"), "TOKEN=rotated\n").unwrap();
    let revision = app.dcg_seal(root, change.id.as_str()).unwrap();
    assert!(
        revision.touched.contains(&resource_of(".env")),
        "the change is tracked like any other: {:?}",
        revision.touched
    );
}

#[test]
fn sealing_a_revision_records_the_neutral_explanation_of_it() {
    let directory = workspace_of(&[("src/auth.rs", "fn old() {}\n")]);
    let root = directory.path();
    let app = App::new();

    app.checkpoint(root, "base").unwrap();
    std::fs::write(root.join("src/auth.rs"), "pub fn validate_token() {}\n").unwrap();
    let change = app
        .dcg_open_change_pack(root, "explain-me", &whole_baseline(&app, root))
        .unwrap();
    let revision = app.dcg_seal(root, change.id.as_str()).unwrap();

    // Sealing is the producer. Nothing else has to be installed, run or asked
    // for the explanation to exist — a representation nobody can reach is not
    // a representation.
    let bundle = app
        .dcg_representation(root, &revision.id.to_string())
        .unwrap()
        .expect("sealing a revision records its explanation");

    // It binds this exact revision, and validates against it.
    assert_eq!(bundle.revision_pack, revision.id);
    bundle
        .validate_against(&revision)
        .expect("the bundle explains the revision it names");

    // It names the exact observations it read, not merely their ids.
    assert!(
        !bundle.inputs.is_empty(),
        "an explanation derived from nothing explains nothing"
    );

    // Every touched Resource is explained, by the neutral rendering, which is
    // Core's own work and does not pretend an extension authored it.
    let explained: std::collections::BTreeSet<_> = bundle
        .representations
        .iter()
        .map(|representation| representation.resource_id.clone())
        .collect();
    assert_eq!(explained, revision.touched);
    for representation in &bundle.representations {
        assert_eq!(
            representation.strategy_id.to_string(),
            draft_core::app::representation::NEUTRAL_STRATEGY
        );
        assert!(
            representation.provenance.producer().is_none(),
            "with nothing installed, no extension produced this: {:?}",
            representation.provenance
        );
    }

    // Create-once: the first explanation of a revision stands. Resealing the
    // same change produces a *different* revision, with its own bundle, rather
    // than overwriting this one.
    let resealed = app.dcg_seal(root, change.id.as_str()).unwrap();
    assert_eq!(
        app.dcg_representation(root, &revision.id.to_string())
            .unwrap(),
        Some(bundle),
        "the recorded explanation of a sealed revision never moves"
    );
    assert!(app
        .dcg_representation(root, &resealed.id.to_string())
        .unwrap()
        .is_some());
}
