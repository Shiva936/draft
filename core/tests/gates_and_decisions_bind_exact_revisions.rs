//! Scenarios O, U, V: what a gate, a decision and a waiver actually commit to.

use draft_core::dcg::decision::{Decision, DecisionOutcome, DecisionStore};
use draft_core::evidence::context::{
    EvaluationContext, SecurityContextSnapshot, SecurityDependencySet,
};
use draft_core::gate::waiver::{GateWaiver, GateWaiverStore};
use draft_core::gate::{GateCondition, GateEvaluation, GateEvaluationStore};
use draft_dcg_contract::identifier::{NamespacedId, ScopedId};
use draft_dcg_contract::ids::{ActorId, DecisionId, RevisionPackId};
use draft_dcg_contract::security::{PolicyDigest, SecurityControlKindId, SecurityFactRef};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;

fn revision(id: &str) -> RevisionPackId {
    RevisionPackId::parse(id).unwrap()
}

fn at(nanos: i64) -> Timestamp {
    Timestamp::from_unix_nanos(nanos)
}

fn grant() -> SecurityFactRef {
    SecurityFactRef::new(
        SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap(),
        Some(ScopedId::parse("auth_000000000001").unwrap()),
        Digest::of_bytes(b"the-grant"),
    )
}

fn context() -> EvaluationContext {
    EvaluationContext {
        evaluated_at: at(0),
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

fn condition(id: &str, satisfied: bool) -> GateCondition {
    GateCondition {
        id: id.into(),
        definition: Digest::of_bytes(id.as_bytes()),
        satisfied,
        detail: (!satisfied).then(|| "the check did not pass".to_string()),
    }
}

fn evaluation(id: &str, rev: &str, conditions: Vec<GateCondition>) -> GateEvaluation {
    GateEvaluation {
        id: id.into(),
        revision_pack: revision(rev),
        definition: Digest::of_bytes(b"definition"),
        scope: Digest::of_bytes(b"scope"),
        evidence: Default::default(),
        assessments: Default::default(),
        conditions,
        context: context(),
    }
}

#[test]
fn a_gate_that_checked_nothing_does_not_pass() {
    // Without this an empty condition set satisfies `all()` vacuously, and a
    // misconfiguration that selected no conditions would read as approval.
    let empty = evaluation("gate_1", "rpk_000000000001", vec![]);
    assert!(empty.validate().is_err());
}

#[test]
fn a_failed_condition_must_say_why() {
    let mut silent = condition("tests", false);
    silent.detail = None;
    let gate = evaluation("gate_1", "rpk_000000000001", vec![silent]);
    assert!(gate.validate().is_err());
}

#[test]
fn a_gate_binds_the_revision_it_evaluated() {
    let gate = evaluation("gate_1", "rpk_000000000001", vec![condition("tests", true)]);
    assert!(gate.is_satisfied());
    assert!(gate.covers(&revision("rpk_000000000001")));
    assert!(!gate.covers(&revision("rpk_000000000002")));
}

#[test]
fn failures_are_recorded_rather_than_dropped() {
    // A reader needs to know *what* failed, not merely that the gate did not
    // pass: "risk unassessed" and "tests failed" call for different work.
    let gate = evaluation(
        "gate_1",
        "rpk_000000000001",
        vec![condition("tests", true), condition("risk", false)],
    );
    assert!(!gate.is_satisfied());
    let unsatisfied = gate.unsatisfied();
    assert_eq!(unsatisfied.len(), 1);
    assert_eq!(unsatisfied[0].id, "risk");
}

#[test]
fn a_gate_evaluation_cannot_be_rewritten_behind_its_id() {
    let directory = tempfile::tempdir().unwrap();
    let store = GateEvaluationStore::new(directory.path());
    let failed = evaluation(
        "gate_1",
        "rpk_000000000001",
        vec![condition("tests", false)],
    );
    store.put(&failed).unwrap();

    let passed = evaluation("gate_1", "rpk_000000000001", vec![condition("tests", true)]);
    assert!(
        store.put(&passed).is_err(),
        "a gate result cannot be flipped behind the id a promotion cites"
    );
}

#[test]
fn an_approval_must_cite_its_authority_but_a_refusal_need_not() {
    let approval_without_authority = Decision {
        id: DecisionId::parse("dec_000000000001").unwrap(),
        revision_pack: revision("rpk_000000000001"),
        outcome: DecisionOutcome::Approved,
        decided_by: ActorId::parse("act_000000000001").unwrap(),
        decided_at: at(1),
        authority: Default::default(),
    };
    assert!(
        approval_without_authority.validate().is_err(),
        "approving is what lets work proceed; it cannot rest on nothing"
    );

    let refusal = Decision {
        outcome: DecisionOutcome::Rejected {
            reason: "touches credential handling".into(),
        },
        ..approval_without_authority.clone()
    };
    refusal
        .validate()
        .expect("refusing needs no authority beyond being asked to review");

    let approval = Decision {
        authority: [grant()].into_iter().collect(),
        ..approval_without_authority
    };
    approval.validate().unwrap();
}

#[test]
fn requesting_changes_is_not_a_rejection() {
    // "Not yet" and "not this" leave the work in different places. Collapsing
    // them either closes work meant to continue, or leaves open work meant to
    // stop.
    let base = Decision {
        id: DecisionId::parse("dec_000000000001").unwrap(),
        revision_pack: revision("rpk_000000000001"),
        outcome: DecisionOutcome::Approved,
        decided_by: ActorId::parse("act_000000000001").unwrap(),
        decided_at: at(1),
        authority: [grant()].into_iter().collect(),
    };

    let changes = Decision {
        outcome: DecisionOutcome::ChangesRequested {
            reason: "add a test for the empty case".into(),
        },
        ..base.clone()
    };
    let rejected = Decision {
        outcome: DecisionOutcome::Rejected {
            reason: "the approach is wrong".into(),
        },
        ..base.clone()
    };

    assert_ne!(changes.outcome, rejected.outcome);
    assert!(changes.leaves_work_open() && rejected.leaves_work_open());
    assert!(base.is_approval() && !changes.is_approval());
}

#[test]
fn a_decision_cannot_be_rewritten_behind_its_id() {
    let directory = tempfile::tempdir().unwrap();
    let store = DecisionStore::new(directory.path());
    let rejected = Decision {
        id: DecisionId::parse("dec_000000000001").unwrap(),
        revision_pack: revision("rpk_000000000001"),
        outcome: DecisionOutcome::Rejected {
            reason: "not this approach".into(),
        },
        decided_by: ActorId::parse("act_000000000001").unwrap(),
        decided_at: at(1),
        authority: Default::default(),
    };
    store.put(&rejected).unwrap();

    let flipped = Decision {
        outcome: DecisionOutcome::Approved,
        authority: [grant()].into_iter().collect(),
        ..rejected.clone()
    };
    assert!(
        store.put(&flipped).is_err(),
        "a rejection cannot become an approval behind the id"
    );
}

#[test]
fn a_waiver_covers_one_condition_on_one_revision_until_it_expires() {
    let waiver = GateWaiver {
        id: "wvr_1".into(),
        revision_pack: revision("rpk_000000000001"),
        condition: "risk".into(),
        reason: "no risk rules are contributed for this project".into(),
        waived_by: ActorId::parse("act_000000000001").unwrap(),
        waived_at: at(0),
        expires_at: at(100),
        authority: grant(),
    };
    waiver.validate().unwrap();

    assert!(waiver.is_in_force(&revision("rpk_000000000001"), "risk", at(50)));
    // Not for another condition, another revision, or after it lapses.
    assert!(!waiver.is_in_force(&revision("rpk_000000000001"), "tests", at(50)));
    assert!(!waiver.is_in_force(&revision("rpk_000000000002"), "risk", at(50)));
    assert!(!waiver.is_in_force(&revision("rpk_000000000001"), "risk", at(100)));
}

#[test]
fn a_waiver_without_an_end_is_refused() {
    // An exception with no expiry is a policy change nobody decided to make.
    let waiver = GateWaiver {
        id: "wvr_1".into(),
        revision_pack: revision("rpk_000000000001"),
        condition: "risk".into(),
        reason: "accepted".into(),
        waived_by: ActorId::parse("act_000000000001").unwrap(),
        waived_at: at(10),
        expires_at: at(10),
        authority: grant(),
    };
    assert!(waiver.validate().is_err());

    let directory = tempfile::tempdir().unwrap();
    assert!(GateWaiverStore::new(directory.path()).put(&waiver).is_err());
}
