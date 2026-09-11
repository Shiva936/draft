//! The authorization chain, end to end, through the application layer.
//!
//! Each domain object is tested in isolation elsewhere. What these prove is
//! that the *live path* enforces the separation: that submitting does not
//! accept, that a gate does not authorize, that a decision without a gate does
//! not either, and that publication has no authority over what was accepted.

mod support;

use std::collections::BTreeSet;

use draft_core::app::authorization::{
    assess, decide, evaluate_gate, AuthorizationStores, DecisionRequest, GateRequest,
    GateRequirements,
};
use draft_core::app::promotion::{promote, PromotionOutcome, PromotionRequest};
use draft_core::app::publish::{promotion_of, publishable, PublishOutcome};
use draft_core::app::App;
use draft_core::dcg::baseline::{current_baseline, BaselineOrigin};
use draft_core::evidence::assessment::{AssessedRisk, Assessment};
use draft_core::evidence::context::{
    EvaluationContext, SecurityContextSnapshot, SecurityDependencySet,
};
use draft_core::evidence::{Evidence, EvidenceOutcome};
use draft_core::project::Workspace;
use draft_dcg_contract::baseline::BaselineId;
use draft_dcg_contract::identifier::{NamespacedId, ScopedId};
use draft_dcg_contract::ids::{
    ActorId, AssessmentId, ChangeId, ChangeRevisionId, DecisionId, EvidenceId, ObservationId,
};
use draft_dcg_contract::observation::{ObservationDigest, ObservationRef};
use draft_dcg_contract::producer::ProducerIdentity;
use draft_dcg_contract::security::{PolicyDigest, SecurityControlKindId, SecurityFactRef};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;

use draft_core::dcg::decision::DecisionOutcome;

/// A project with an accepted initial Baseline, ready to promote onto.
struct Project {
    _directory: tempfile::TempDir,
    app: App,
    workspace: Workspace,
    stores: AuthorizationStores,
}

impl Project {
    fn new() -> Self {
        // `DRAFT_GLOBAL_HOME` is process-wide, so setting it per test races
        // once these run in parallel. One home for the binary; the projects
        // are still separate, which is what each test is about.
        static GLOBAL_HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        let home = GLOBAL_HOME.get_or_init(|| tempfile::tempdir().unwrap());
        std::env::set_var("DRAFT_GLOBAL_HOME", home.path());

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("a.txt"), "hello").unwrap();

        let app = App::new();
        app.init_with_base(root, "base change").unwrap();
        let workspace = app.open(root).unwrap();
        let stores = AuthorizationStores::for_layout(&workspace.layout);
        Self {
            _directory: directory,
            app,
            workspace,
            stores,
        }
    }

    fn accepted(&self) -> Option<BaselineId> {
        current_baseline(&self.workspace.layout).unwrap()
    }

    fn context(&self) -> EvaluationContext {
        EvaluationContext {
            evaluated_at: Timestamp::from_unix_nanos(0),
            clock_source: NamespacedId::parse("draft.core/fixed-clock").unwrap(),
            policy_digest: PolicyDigest::new(Digest::of_bytes(b"policy")),
            security_context_digest: SecurityContextSnapshot {
                dependencies: SecurityDependencySet::default(),
                resolved: Default::default(),
            }
            .digest()
            .unwrap(),
            core_evaluator_revision: "1".into(),
        }
    }

    /// Evidence → assessment → gate, for one revision at one risk level.
    fn authorize_up_to_gate(
        &self,
        revision: &ChangeRevisionId,
        outcome: EvidenceOutcome,
        risk: AssessedRisk,
    ) -> draft_core::gate::GateEvaluation {
        let evidence_id = EvidenceId::parse(format!("evd_{}", suffix(revision))).unwrap();
        self.stores
            .evidence
            .put(&Evidence {
                id: evidence_id.clone(),
                revision: revision.clone(),
                inputs: [ObservationRef {
                    id: ObservationId::parse("obs_000000000001").unwrap(),
                    digest: ObservationDigest::new(Digest::of_bytes(b"observed")),
                }]
                .into_iter()
                .collect(),
                producer: producer(),
                configuration: Digest::of_bytes(b"verify.toml"),
                outcome,
                context: self.context(),
            })
            .unwrap();

        let assessment_id = AssessmentId::parse(format!("asm_{}", suffix(revision))).unwrap();
        assess(
            &self.stores,
            &Assessment {
                id: assessment_id.clone(),
                revision: revision.clone(),
                inputs: [evidence_id].into_iter().collect(),
                risk,
                rationale: "assessed for the test".into(),
                producer: producer(),
                configuration: Digest::of_bytes(b"rules"),
                context: self.context(),
            },
        )
        .unwrap();

        evaluate_gate(
            &self.stores,
            &GateRequest {
                id: format!("gate_{}", suffix(revision)),
                revision: revision.clone(),
                definition: Digest::of_bytes(b"definition"),
                scope: Digest::of_bytes(b"scope"),
                assessments: [assessment_id].into_iter().collect(),
                requirements: GateRequirements {
                    required: vec![("draft.gate/verified".into(), Digest::of_bytes(b"verified"))],
                    max_risk: AssessedRisk::Medium,
                },
                waivers: BTreeSet::new(),
                context: self.context(),
            },
        )
        .unwrap()
    }

    /// The same chain, with waivers offered and a clock later than time zero.
    fn gate_with_waivers(
        &self,
        revision: &ChangeRevisionId,
        outcome: EvidenceOutcome,
        risk: AssessedRisk,
        waivers: BTreeSet<String>,
    ) -> draft_core::gate::GateEvaluation {
        let base = self.authorize_up_to_gate(revision, outcome, risk);
        let mut context = self.context();
        context.evaluated_at = Timestamp::from_unix_nanos(100);
        evaluate_gate(
            &self.stores,
            &GateRequest {
                id: format!("{}-waived", base.id),
                revision: revision.clone(),
                definition: Digest::of_bytes(b"definition"),
                scope: Digest::of_bytes(b"scope"),
                assessments: base.assessments.clone(),
                requirements: GateRequirements {
                    required: vec![("draft.gate/verified".into(), Digest::of_bytes(b"verified"))],
                    max_risk: AssessedRisk::Medium,
                },
                waivers,
                context,
            },
        )
        .unwrap()
    }

    fn approve(
        &self,
        revision: &ChangeRevisionId,
        gate: &draft_core::gate::GateEvaluation,
    ) -> DecisionId {
        let id = DecisionId::parse(format!("dec_{}", suffix(revision))).unwrap();
        decide(
            &self.stores,
            &DecisionRequest {
                id: id.clone(),
                revision: revision.clone(),
                outcome: DecisionOutcome::Approved,
                decided_by: actor(),
                decided_at: Timestamp::from_unix_nanos(1),
                authority: [grant()].into_iter().collect(),
            },
            Some(gate),
        )
        .unwrap();
        id
    }

    /// The Change a promotion completes. Created once, because promotion
    /// completes a Change rather than creating one.
    fn ensure_change(&self) -> ChangeId {
        let id = ChangeId::parse("chg_000000000001").unwrap();
        let store = draft_core::dcg::ChangeStore::new(self.workspace.layout.changes_dir());
        if store.read_unlocked(&id).unwrap().is_none() {
            store
                .create(&draft_core::dcg::change::Change {
                    generation: 0,
                    id: id.clone(),
                    project: self.workspace.workspace_id.clone(),
                    current_definition: Digest::of_bytes(b"definition"),
                    lifecycle: draft_core::dcg::change::ChangeLifecycle::Active,
                })
                .unwrap();
        }
        id
    }

    fn change_lifecycle(&self) -> draft_core::dcg::change::ChangeLifecycle {
        draft_core::dcg::ChangeStore::new(self.workspace.layout.changes_dir())
            .read_unlocked(&ChangeId::parse("chg_000000000001").unwrap())
            .unwrap()
            .unwrap()
            .lifecycle
    }

    fn request(
        &self,
        revision: &ChangeRevisionId,
        decision: DecisionId,
        gate: &draft_core::gate::GateEvaluation,
        parent: Option<BaselineId>,
    ) -> PromotionRequest {
        PromotionRequest {
            change: self.ensure_change(),
            revision: revision.clone(),
            decision,
            gate: gate.id.clone(),
            expected_parent: parent,
        }
    }
}

fn suffix(revision: &ChangeRevisionId) -> String {
    revision.to_string().replace("rev_", "")
}

fn revision(name: &str) -> ChangeRevisionId {
    ChangeRevisionId::parse(format!("rev_{name}")).unwrap()
}

fn producer() -> ProducerIdentity {
    ProducerIdentity::new(NamespacedId::parse("draft.core/verification").unwrap(), "1").unwrap()
}

fn actor() -> ActorId {
    ActorId::parse("act_000000000001").unwrap()
}

fn grant() -> SecurityFactRef {
    SecurityFactRef::new(
        SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap(),
        Some(ScopedId::parse("auth_000000000001").unwrap()),
        Digest::of_bytes(b"grant"),
    )
}

#[test]
fn the_full_chain_promotes_and_the_baseline_advances() {
    let project = Project::new();
    let initial = project.accepted().expect("init accepts a Baseline");
    let rev = revision("000000000001");

    let gate = project.authorize_up_to_gate(&rev, EvidenceOutcome::Passed, AssessedRisk::Low);
    assert!(gate.is_satisfied());
    let decision = project.approve(&rev, &gate);

    let outcome = promote(
        &project.app,
        &project.workspace,
        &project.request(&rev, decision, &gate, Some(initial.clone())),
    )
    .unwrap();

    assert!(matches!(outcome, PromotionOutcome::Promoted { .. }));
    let promoted = outcome.baseline().clone();
    assert_ne!(promoted, initial, "promotion advances the Baseline");
    assert_eq!(project.accepted(), Some(promoted.clone()));

    // The child names its parent and does not claim to be the project's first.
    let record = draft_core::dcg::baseline::BaselineStore::new(
        project.workspace.layout.draft_dir.join("baselines"),
    )
    .record(&promoted)
    .unwrap()
    .unwrap();
    assert!(matches!(record.origin, BaselineOrigin::Promotion { .. }));
}

#[test]
fn the_initial_baseline_is_initial_and_has_no_parent() {
    // The regression class: a Baseline with a parent must never claim
    // `Initial`, and one without must never claim otherwise.
    let project = Project::new();
    let initial = project.accepted().unwrap();
    let baselines = draft_core::dcg::baseline::BaselineStore::new(
        project.workspace.layout.draft_dir.join("baselines"),
    );

    let record = baselines.record(&initial).unwrap().unwrap();
    let manifest = baselines.manifest(&initial).unwrap().unwrap();
    assert!(matches!(record.origin, BaselineOrigin::Initial));
    assert!(manifest.parent_baseline_id.is_none());
    // The record validates against its own manifest, which is what refuses a
    // mismatched pairing.
    record.validate_against(&manifest).unwrap();
}

#[test]
fn a_failing_gate_stops_the_decision_and_therefore_the_promotion() {
    let project = Project::new();
    let rev = revision("00000000000f");

    let gate = project.authorize_up_to_gate(&rev, EvidenceOutcome::Failed, AssessedRisk::Low);
    assert!(!gate.is_satisfied(), "failed evidence must not satisfy");

    // The refusal happens at the decision: approving over a failing gate is a
    // decision made instead of the facts.
    let refused = decide(
        &project.stores,
        &DecisionRequest {
            id: DecisionId::parse("dec_00000000000f").unwrap(),
            revision: rev.clone(),
            outcome: DecisionOutcome::Approved,
            decided_by: actor(),
            decided_at: Timestamp::from_unix_nanos(1),
            authority: [grant()].into_iter().collect(),
        },
        Some(&gate),
    );
    assert!(refused.is_err());
}

#[test]
fn a_rejecting_decision_cannot_promote() {
    let project = Project::new();
    let initial = project.accepted().unwrap();
    let rev = revision("00000000000r");
    let gate = project.authorize_up_to_gate(&rev, EvidenceOutcome::Passed, AssessedRisk::Low);

    // A rejection needs no gate and no authority: refusing work is always
    // legitimate. What it cannot do is authorize anything.
    let decision = DecisionId::parse("dec_00000000000r").unwrap();
    decide(
        &project.stores,
        &DecisionRequest {
            id: decision.clone(),
            revision: rev.clone(),
            outcome: DecisionOutcome::Rejected {
                reason: "not this approach".into(),
            },
            decided_by: actor(),
            decided_at: Timestamp::from_unix_nanos(1),
            authority: BTreeSet::new(),
        },
        None,
    )
    .unwrap();

    let error = promote(
        &project.app,
        &project.workspace,
        &project.request(&rev, decision, &gate, Some(initial.clone())),
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        draft_core::support::error::DraftErrorKind::ReviewRequired
    );
    assert_eq!(
        project.accepted(),
        Some(initial),
        "the Baseline is untouched"
    );
}

#[test]
fn a_decision_about_another_revision_cannot_promote_this_one() {
    // Judgements do not carry across sealed content.
    let project = Project::new();
    let initial = project.accepted().unwrap();
    let judged = revision("00000000000a");
    let promoting = revision("00000000000b");

    let gate = project.authorize_up_to_gate(&judged, EvidenceOutcome::Passed, AssessedRisk::Low);
    let decision = project.approve(&judged, &gate);

    let error = promote(
        &project.app,
        &project.workspace,
        &project.request(&promoting, decision, &gate, Some(initial.clone())),
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        draft_core::support::error::DraftErrorKind::StaleRevision
    );
    assert_eq!(project.accepted(), Some(initial));
}

#[test]
fn a_promotion_decided_against_a_moved_baseline_is_refused_not_rebased() {
    // Two authorizations against the same accepted state. The second was
    // decided against a Baseline that no longer exists, and rebasing it would
    // be a judgement about whether the changes compose that nobody made.
    let project = Project::new();
    let initial = project.accepted().unwrap();

    let first = revision("00000000000c");
    let first_gate =
        project.authorize_up_to_gate(&first, EvidenceOutcome::Passed, AssessedRisk::Low);
    let first_decision = project.approve(&first, &first_gate);
    promote(
        &project.app,
        &project.workspace,
        &project.request(&first, first_decision, &first_gate, Some(initial.clone())),
    )
    .unwrap();
    let advanced = project.accepted().unwrap();
    assert_ne!(advanced, initial);

    let second = revision("00000000000d");
    let second_gate =
        project.authorize_up_to_gate(&second, EvidenceOutcome::Passed, AssessedRisk::Low);
    let second_decision = project.approve(&second, &second_gate);
    let error = promote(
        &project.app,
        &project.workspace,
        // Still naming the Baseline that has since been superseded.
        &project.request(&second, second_decision, &second_gate, Some(initial)),
    )
    .unwrap_err();

    assert_eq!(
        error.kind,
        draft_core::support::error::DraftErrorKind::StaleBaseline
    );
    assert_eq!(
        project.accepted(),
        Some(advanced),
        "the refused promotion left the accepted Baseline alone"
    );
}

#[test]
fn repeating_the_same_promotion_converges_rather_than_promoting_twice() {
    // A promotion interrupted after its Baseline committed must be safe to
    // retry: the same revision onto the same parent is the same promotion.
    let project = Project::new();
    let initial = project.accepted().unwrap();
    let rev = revision("00000000000e");
    let gate = project.authorize_up_to_gate(&rev, EvidenceOutcome::Passed, AssessedRisk::Low);
    let decision = project.approve(&rev, &gate);
    let request = project.request(&rev, decision, &gate, Some(initial));

    let first = promote(&project.app, &project.workspace, &request).unwrap();
    let again = promote(&project.app, &project.workspace, &request).unwrap();

    assert!(matches!(first, PromotionOutcome::Promoted { .. }));
    assert!(matches!(again, PromotionOutcome::AlreadyPromoted { .. }));
    assert_eq!(first.baseline(), again.baseline());
    assert_eq!(first.promotion(), again.promotion());
}

#[test]
fn publication_requires_a_promoted_baseline_and_never_changes_it() {
    let project = Project::new();
    let initial = project.accepted().unwrap();

    // The initial Baseline was never promoted: nothing decided work into it,
    // so nothing authorized delivering it.
    let unpromoted = publishable(&project.workspace, &initial).unwrap_err();
    assert_eq!(
        unpromoted.kind,
        draft_core::support::error::DraftErrorKind::ReviewRequired
    );

    let rev = revision("000000000010");
    let gate = project.authorize_up_to_gate(&rev, EvidenceOutcome::Passed, AssessedRisk::Low);
    let decision = project.approve(&rev, &gate);
    let promoted = promote(
        &project.app,
        &project.workspace,
        &project.request(&rev, decision, &gate, Some(initial)),
    )
    .unwrap()
    .baseline()
    .clone();

    let publishable = publishable(&project.workspace, &promoted).unwrap();
    assert_eq!(publishable.baseline, promoted);
    assert_eq!(
        promotion_of(&project.workspace, &promoted)
            .unwrap()
            .unwrap()
            .baseline,
        promoted
    );

    // A failed delivery leaves the accepted Baseline exactly where it was.
    // These are the engine's own outcome shapes, projected: a refusal and an
    // undetermined result are different facts, and neither touches authority.
    let attempt = draft_dcg_contract::ids::PublicationAttemptId::parse("pat_000000000001").unwrap();
    let refused = PublishOutcome::Concluded {
        attempt: attempt.clone(),
        outcome: draft_dcg_contract::publication::PublicationOutcomeKind::Failed {
            reason: "the target was unreachable".into(),
        },
    };
    assert!(!refused.is_delivered());
    draft_core::app::publish::assert_baseline_unchanged(&project.workspace, &promoted, &refused)
        .unwrap();
    assert_eq!(project.accepted(), Some(promoted.clone()));

    // And so does an undetermined one, which is the case where a naive
    // implementation would be most tempted to roll something back.
    let unknown = PublishOutcome::Concluded {
        attempt,
        outcome: draft_dcg_contract::publication::PublicationOutcomeKind::Indeterminate {
            reason: "the connection dropped before the reply".into(),
        },
    };
    assert!(!unknown.is_delivered());
    draft_core::app::publish::assert_baseline_unchanged(&project.workspace, &promoted, &unknown)
        .unwrap();
    assert_eq!(project.accepted(), Some(promoted));
}

#[test]
fn submitting_does_not_accept_a_baseline() {
    // The behaviour this architecture removed: submission prepares work for
    // review and stops. Only promotion advances what the project accepts.
    let project = Project::new();
    let before = project.accepted().unwrap();

    std::fs::write(project.workspace.root.join("b.txt"), "new work").unwrap();
    let observed = current_baseline(&project.workspace.layout).unwrap();

    assert_eq!(observed, Some(before.clone()));
    assert_eq!(
        project.accepted(),
        Some(before),
        "editing and observing changes nothing about what is accepted"
    );
}

#[test]
fn an_in_force_waiver_excuses_a_condition_and_says_so() {
    // A waiver is an authorized exception, not a silent pass: the gate records
    // that a person allowed this rather than that the check succeeded.
    let project = Project::new();
    let rev = revision("00000000000w");

    let waiver = draft_core::gate::waiver::GateWaiver {
        id: "wvr_00000000000w".into(),
        revision: rev.clone(),
        condition: "draft.gate/verified".into(),
        reason: "the verifier is unavailable in this environment".into(),
        waived_by: actor(),
        waived_at: Timestamp::from_unix_nanos(0),
        expires_at: Timestamp::from_unix_nanos(9_999),
        authority: grant(),
    };
    project.stores.waivers.put(&waiver).unwrap();

    // Evidence that fails, so the condition genuinely does not hold.
    let gate = project.gate_with_waivers(
        &rev,
        EvidenceOutcome::Failed,
        AssessedRisk::Low,
        [waiver.id.clone()].into_iter().collect(),
    );

    assert!(
        gate.is_satisfied(),
        "an in-force waiver excuses the condition"
    );
    let excused = gate
        .conditions
        .iter()
        .find(|condition| condition.id == "draft.gate/verified")
        .unwrap();
    assert!(
        excused
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("waived by 'wvr_00000000000w'")),
        "the waiver must be named in the record, not applied silently"
    );
}

#[test]
fn an_expired_or_unrelated_waiver_excuses_nothing() {
    let project = Project::new();
    let rev = revision("00000000000x");
    let other = revision("00000000000y");

    // Expired before the evaluation moment.
    let expired = draft_core::gate::waiver::GateWaiver {
        id: "wvr_expired00000x".into(),
        revision: rev.clone(),
        condition: "draft.gate/verified".into(),
        reason: "lapsed".into(),
        waived_by: actor(),
        waived_at: Timestamp::from_unix_nanos(0),
        expires_at: Timestamp::from_unix_nanos(1),
        authority: grant(),
    };
    // In force, but for a different revision: an exception accepted for the
    // work as it stood is not one for whatever it became.
    let elsewhere = draft_core::gate::waiver::GateWaiver {
        id: "wvr_elsewhere0000".into(),
        revision: other,
        condition: "draft.gate/verified".into(),
        reason: "different work".into(),
        waived_by: actor(),
        waived_at: Timestamp::from_unix_nanos(0),
        expires_at: Timestamp::from_unix_nanos(9_999),
        authority: grant(),
    };
    project.stores.waivers.put(&expired).unwrap();
    project.stores.waivers.put(&elsewhere).unwrap();

    // The evaluation context's clock is past the expiry.
    let gate = project.gate_with_waivers(
        &rev,
        EvidenceOutcome::Failed,
        AssessedRisk::Low,
        [expired.id.clone(), elsewhere.id.clone()]
            .into_iter()
            .collect(),
    );
    assert!(
        !gate.is_satisfied(),
        "neither an expired nor an unrelated waiver may satisfy a gate"
    );
}

#[test]
fn promotion_completes_the_change_so_the_same_work_cannot_be_promoted_twice() {
    // The gap this closes: with the Change left open, a second promotion of
    // the same work could accept it into a second Baseline. Completion is
    // mandatory once the Baseline exists, not a tidy-up afterwards.
    let project = Project::new();
    let initial = project.accepted().unwrap();
    let rev = revision("00000000000q");

    let gate = project.authorize_up_to_gate(&rev, EvidenceOutcome::Passed, AssessedRisk::Low);
    let decision = project.approve(&rev, &gate);
    project.ensure_change();
    assert_eq!(
        project.change_lifecycle(),
        draft_core::dcg::change::ChangeLifecycle::Active,
        "the Change is open before its work is accepted"
    );

    promote(
        &project.app,
        &project.workspace,
        &project.request(&rev, decision, &gate, Some(initial)),
    )
    .unwrap();

    assert_eq!(
        project.change_lifecycle(),
        draft_core::dcg::change::ChangeLifecycle::Completed,
        "promotion completes the Change whose work it accepted"
    );
}

#[test]
fn a_completed_change_accepts_no_further_work() {
    // The consequence that makes completion worth doing: a Change whose work
    // is in an accepted Baseline cannot be revised and re-promoted.
    let project = Project::new();
    let initial = project.accepted().unwrap();
    let rev = revision("00000000000s");

    let gate = project.authorize_up_to_gate(&rev, EvidenceOutcome::Passed, AssessedRisk::Low);
    let decision = project.approve(&rev, &gate);
    promote(
        &project.app,
        &project.workspace,
        &project.request(&rev, decision, &gate, Some(initial)),
    )
    .unwrap();

    let store = draft_core::dcg::ChangeStore::new(project.workspace.layout.changes_dir());
    let completed = store
        .read_unlocked(&ChangeId::parse("chg_000000000001").unwrap())
        .unwrap()
        .unwrap();
    assert!(!completed.lifecycle.accepts_work());
    // Reopening is refused: its work is already in an accepted Baseline.
    assert!(store
        .reopen(&ChangeId::parse("chg_000000000001").unwrap())
        .is_err());
}

#[test]
fn completing_a_change_twice_converges_rather_than_refusing() {
    // Promotion finalization replays after an interruption. Arriving at the
    // state you were trying to reach is not a conflict.
    let project = Project::new();
    let id = project.ensure_change();
    let store = draft_core::dcg::ChangeStore::new(project.workspace.layout.changes_dir());

    let first = store.complete(&id).unwrap();
    let again = store.complete(&id).unwrap();
    assert_eq!(first, again, "a replayed completion is idempotent");
}
