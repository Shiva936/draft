//! Scenario B: evidence and assessments never carry to a later revision.
//!
//! The shortcut this forbids is the tempting one. An agent seals `rev_1`,
//! evidence is gathered, a reviewer approves. The agent then edits one file and
//! seals `rev_2`. Letting the earlier evidence satisfy the new revision would
//! be convenient and would look right most of the time — the change was small,
//! the tests passed, nothing obviously moved.
//!
//! But nobody ran anything against `rev_2`. An approval resting on evidence
//! gathered before the edit approves work that was never examined, and the
//! record would show a satisfied gate either way. So the binding is exact, and
//! there is deliberately no API for asking whether evidence "still" applies.

mod support;

use draft_core::evidence::assessment::{AssessedRisk, Assessment, AssessmentStore};
use draft_core::evidence::context::{
    EvaluationContext, SecurityContextSnapshot, SecurityDependencySet,
};
use draft_core::evidence::{Evidence, EvidenceOutcome, EvidenceStore};
use draft_dcg_contract::identifier::NamespacedId;
use draft_dcg_contract::ids::{AssessmentId, ChangeRevisionId, EvidenceId, ObservationId};
use draft_dcg_contract::observation::{ObservationDigest, ObservationRef};
use draft_dcg_contract::producer::ProducerIdentity;
use draft_dcg_contract::security::PolicyDigest;
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;

fn revision(id: &str) -> ChangeRevisionId {
    ChangeRevisionId::parse(id).unwrap()
}

fn producer() -> ProducerIdentity {
    ProducerIdentity::new(NamespacedId::parse("draft.core/verification").unwrap(), "1").unwrap()
}

fn context() -> EvaluationContext {
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

fn observation() -> ObservationRef {
    ObservationRef {
        id: ObservationId::parse("obs_000000000001").unwrap(),
        digest: ObservationDigest::new(Digest::of_bytes(b"observed-state")),
    }
}

fn evidence_for(id: &str, rev: &str) -> Evidence {
    Evidence {
        id: EvidenceId::parse(id).unwrap(),
        revision: revision(rev),
        inputs: [observation()].into_iter().collect(),
        producer: producer(),
        configuration: Digest::of_bytes(b"verify.toml"),
        outcome: EvidenceOutcome::Passed,
        context: context(),
    }
}

#[test]
fn evidence_for_one_revision_does_not_cover_the_next() {
    let evidence = evidence_for("evd_000000000001", "rev_000000000001");

    assert!(evidence.covers(&revision("rev_000000000001")));
    assert!(
        !evidence.covers(&revision("rev_000000000002")),
        "a later revision was never examined by this evidence"
    );
}

#[test]
fn an_assessment_binds_the_revision_it_judged() {
    let assessment = Assessment {
        id: AssessmentId::parse("asm_000000000001").unwrap(),
        revision: revision("rev_000000000001"),
        inputs: [EvidenceId::parse("evd_000000000001").unwrap()]
            .into_iter()
            .collect(),
        risk: AssessedRisk::Low,
        rationale: "no protected resources touched".into(),
        producer: producer(),
        configuration: Digest::of_bytes(b"risk.toml"),
        context: context(),
    };

    assert!(assessment.covers(&revision("rev_000000000001")));
    assert!(!assessment.covers(&revision("rev_000000000002")));
}

#[test]
fn evidence_cannot_pass_over_nothing() {
    // "Unavailable" and "passed with no inputs" are different claims, and only
    // one of them is honest about having examined nothing.
    let mut hollow = evidence_for("evd_000000000002", "rev_000000000001");
    hollow.inputs.clear();
    assert!(hollow.validate().is_err());

    hollow.outcome = EvidenceOutcome::Unavailable;
    hollow
        .validate()
        .expect("reporting that nothing could be checked is always allowed");
}

#[test]
fn an_unassessed_risk_is_not_low_risk() {
    // Defaulting to Low would make "nobody looked" and "somebody looked and
    // found nothing" the same fact.
    assert!(!AssessedRisk::Unassessed.is_assessed());
    assert!(AssessedRisk::Low.is_assessed());
    assert!(!EvidenceOutcome::Unavailable.is_satisfying());
    assert!(EvidenceOutcome::Passed.is_satisfying());
}

#[test]
fn evidence_is_immutable_behind_its_id() {
    // §2.45: the same id must never resolve to different bytes. Otherwise a
    // failing result could be replaced with a passing one and every decision
    // citing that evidence id would silently inherit the new answer.
    let directory = tempfile::tempdir().unwrap();
    let store = EvidenceStore::new(directory.path());

    let passed = evidence_for("evd_000000000001", "rev_000000000001");
    store.put(&passed).unwrap();
    store
        .put(&passed)
        .expect("re-recording identical evidence is idempotent");

    let mut rewritten = passed.clone();
    rewritten.outcome = EvidenceOutcome::Failed;
    assert!(
        store.put(&rewritten).is_err(),
        "a verdict cannot be rewritten behind the id decisions already cite"
    );

    assert_eq!(
        store
            .get(&EvidenceId::parse("evd_000000000001").unwrap())
            .unwrap(),
        Some(passed)
    );
}

#[test]
fn an_assessment_is_immutable_behind_its_id() {
    let directory = tempfile::tempdir().unwrap();
    let store = AssessmentStore::new(directory.path());
    let critical = Assessment {
        id: AssessmentId::parse("asm_000000000001").unwrap(),
        revision: revision("rev_000000000001"),
        inputs: [EvidenceId::parse("evd_000000000001").unwrap()]
            .into_iter()
            .collect(),
        risk: AssessedRisk::Critical,
        rationale: "touches credential handling".into(),
        producer: producer(),
        configuration: Digest::of_bytes(b"risk.toml"),
        context: context(),
    };
    store.put(&critical).unwrap();

    let downgraded = Assessment {
        risk: AssessedRisk::Low,
        ..critical.clone()
    };
    assert!(
        store.put(&downgraded).is_err(),
        "a risk level cannot be quietly downgraded behind its id"
    );
}
