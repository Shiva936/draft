//! Scenario ED: immutable facts cannot be substituted behind their logical id.
//!
//! Every immutable Store holds the same two properties, so they are asserted
//! through one reusable harness rather than restated per Store. A duplicated
//! assertion is one that eventually gets duplicated wrongly, and the point of
//! the invariant is that it holds *everywhere* — proving it for Observations
//! and Publications while quietly omitting Decisions would leave exactly the
//! gap an attacker wants.
//!
//! Every family's real Store is now exercised directly — Revisions,
//! Definitions, Operations, Evidence, Decisions and Gate evaluations — so no
//! stand-in shapes remain. The one fixture below is not standing in for a
//! family: it is a payload for asserting the *shared mechanism's* behaviour on
//! a substituted file, which is a property of `ImmutableFactStore` itself
//! rather than of any fact's schema.

use draft_core::support::immutable_store::ImmutableFactStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredFact {
    id: String,
    revision: String,
    outcome: String,
}

#[test]
fn a_sealed_change_revision_cannot_be_altered_behind_its_id() {
    use draft_core::dcg::revision_pack::{RevisionPack, RevisionPackStore};
    use draft_dcg_contract::ids::{ActorId, ChangePackId, RevisionPackId};
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::{BaselineId, Digest, ProjectStateRoot};

    fn revision(state_root: &[u8]) -> RevisionPack {
        RevisionPack {
            id: RevisionPackId::parse("rpk_000000000001").unwrap(),
            change_pack: ChangePackId::parse("cpk_000000000001").unwrap(),
            definition: Digest::of_bytes(b"definition"),
            scope: Digest::of_bytes(b"scope"),
            base_baseline: BaselineId::new(Digest::of_bytes(b"base")),
            project_state_root: ProjectStateRoot::new(Digest::of_bytes(state_root)),
            touched: Default::default(),
            sealed_by: ActorId::parse("act_000000000001").unwrap(),
            sealed_at: Timestamp::from_unix_nanos(0),
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let store = RevisionPackStore::new(directory.path());
    let sealed = revision(b"accepted-state");
    store.put(&sealed).unwrap();
    // Re-sealing identical content is idempotent.
    store.put(&sealed).unwrap();
    assert_eq!(
        store
            .get(&RevisionPackId::parse("rpk_000000000001").unwrap())
            .unwrap(),
        Some(sealed)
    );

    // The substitution that matters: the same revision id now claiming a
    // different accepted state. Every judgement bound to `rpk_…` would
    // silently transfer to work nobody reviewed.
    let error = store.put(&revision(b"substituted-state")).unwrap_err();
    assert!(
        matches!(
            error.kind,
            draft_core::support::error::DraftErrorKind::ConflictDetected
                | draft_core::support::error::DraftErrorKind::CorruptData
        ),
        "substituting a sealed revision must be refused, got {:?}",
        error.kind
    );
}

#[test]
fn a_change_definition_and_its_scope_resolution_cannot_be_altered_behind_their_ids() {
    use draft_core::dcg::definition::{ChangePackDefinition, DefinitionStore, ScopeResolution};
    use draft_dcg_contract::ids::{ActorId, ChangePackId, ResourceId};
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::{BaselineId, Digest};

    let directory = tempfile::tempdir().unwrap();
    let store = DefinitionStore::new(directory.path().join("def"), directory.path().join("scope"));

    let definition = ChangePackDefinition {
        change_pack: ChangePackId::parse("cpk_000000000001").unwrap(),
        intent: "tighten the submit gate".into(),
        scope_declaration: [ResourceId::parse("res_000000000001").unwrap()]
            .into_iter()
            .collect(),
        created_by: ActorId::parse("act_000000000001").unwrap(),
        created_at: Timestamp::from_unix_nanos(0),
    };
    let digest = store.put_definition(&definition).unwrap();
    store.put_definition(&definition).unwrap();
    assert_eq!(store.definition(&digest).unwrap(), Some(definition.clone()));

    // A definition stored under its own digest cannot be substituted: widening
    // the declared scope is a different definition, so it gets a different id
    // rather than quietly replacing what was reviewed.
    let widened = ChangePackDefinition {
        scope_declaration: [
            ResourceId::parse("res_000000000001").unwrap(),
            ResourceId::parse("res_000000000002").unwrap(),
        ]
        .into_iter()
        .collect(),
        ..definition.clone()
    };
    let widened_digest = store.put_definition(&widened).unwrap();
    assert_ne!(
        widened_digest, digest,
        "a widened scope is a different fact"
    );
    assert_eq!(
        store.definition(&digest).unwrap(),
        Some(definition),
        "the original definition is untouched by the wider one"
    );

    let resolution = ScopeResolution {
        change_pack: ChangePackId::parse("cpk_000000000001").unwrap(),
        definition: digest.clone(),
        base_baseline: BaselineId::new(Digest::of_bytes(b"base")),
        resources: [ResourceId::parse("res_000000000001").unwrap()]
            .into_iter()
            .collect(),
        resolved_at: Timestamp::from_unix_nanos(0),
    };
    let resolution_digest = store.put_resolution(&resolution).unwrap();
    assert_eq!(
        store.resolution(&resolution_digest).unwrap(),
        Some(resolution)
    );
}

#[test]
fn a_completed_operation_outcome_cannot_be_altered_behind_its_id() {
    use draft_core::execution::operation::{OperationStore, SealedOperation};
    use draft_core::support::common::OperationId;

    let directory = tempfile::tempdir().unwrap();
    let store = OperationStore::at(directory.path());
    let operation = OperationId::new("op_seal");
    let begun = store
        .begin(operation.clone(), "submit.run", "req_hash", None)
        .unwrap();
    let record = match begun {
        draft_core::execution::operation::BeginOperation::New(record) => record,
        other => panic!("expected a new operation, got {other:?}"),
    };

    store
        .complete(record.clone(), serde_json::json!({ "receipt": "rcp_a" }))
        .unwrap();
    let sealed = store.sealed_outcome(&operation).unwrap();
    assert_eq!(
        sealed,
        Some(SealedOperation {
            operation_id: operation.clone(),
            method: "submit.run".into(),
            request_hash: "req_hash".into(),
            result: Some(serde_json::json!({ "receipt": "rcp_a" })),
        })
    );

    // Replaying the identical completion is idempotent — a retry after a crash
    // between sealing and saving must not be refused.
    store
        .complete(record.clone(), serde_json::json!({ "receipt": "rcp_a" }))
        .unwrap();

    // Completing the same operation with a *different* outcome is refused:
    // anything that later cites "operation op_seal produced rcp_a" would
    // otherwise be citing something that had been swapped.
    let error = store
        .complete(record, serde_json::json!({ "receipt": "rcp_b" }))
        .unwrap_err();
    assert!(
        matches!(
            error.kind,
            draft_core::support::error::DraftErrorKind::ConflictDetected
                | draft_core::support::error::DraftErrorKind::CorruptData
        ),
        "substituting a completed outcome must be refused, got {:?}",
        error.kind
    );
}

#[test]
fn evidence_cannot_be_altered_behind_its_id() {
    use draft_core::evidence::context::{
        EvaluationContext, SecurityContextSnapshot, SecurityDependencySet,
    };
    use draft_core::evidence::{Evidence, EvidenceOutcome, EvidenceStore};
    use draft_dcg_contract::identifier::NamespacedId;
    use draft_dcg_contract::ids::{EvidenceId, ObservationId, RevisionPackId};
    use draft_dcg_contract::observation::{ObservationDigest, ObservationRef};
    use draft_dcg_contract::producer::ProducerIdentity;
    use draft_dcg_contract::security::PolicyDigest;
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::Digest;

    fn evidence(outcome: EvidenceOutcome) -> Evidence {
        Evidence {
            id: EvidenceId::parse("evd_000000000001").unwrap(),
            revision_pack: RevisionPackId::parse("rpk_000000000001").unwrap(),
            inputs: [ObservationRef {
                id: ObservationId::parse("obs_000000000001").unwrap(),
                digest: ObservationDigest::new(Digest::of_bytes(b"observed")),
            }]
            .into_iter()
            .collect(),
            producer: ProducerIdentity::new(
                NamespacedId::parse("draft.core/verification").unwrap(),
                "1",
            )
            .unwrap(),
            configuration: Digest::of_bytes(b"verify.toml"),
            outcome,
            context: EvaluationContext {
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
            },
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let store = EvidenceStore::new(directory.path());
    let failed = evidence(EvidenceOutcome::Failed);
    store.put(&failed).unwrap();
    store.put(&failed).unwrap();

    // Turning a failure into a pass is exactly what the binding prevents.
    assert!(store.put(&evidence(EvidenceOutcome::Passed)).is_err());
    assert_eq!(
        store
            .get(&EvidenceId::parse("evd_000000000001").unwrap())
            .unwrap(),
        Some(failed)
    );
}

#[test]
fn a_recorded_decision_cannot_be_altered_behind_its_id() {
    use draft_core::dcg::decision::{Decision, DecisionOutcome, DecisionStore};
    use draft_dcg_contract::identifier::ScopedId;
    use draft_dcg_contract::ids::{ActorId, DecisionId, RevisionPackId};
    use draft_dcg_contract::security::{SecurityControlKindId, SecurityFactRef};
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::Digest;

    let directory = tempfile::tempdir().unwrap();
    let store = DecisionStore::new(directory.path());
    let rejected = Decision {
        id: DecisionId::parse("dec_000000000001").unwrap(),
        revision_pack: RevisionPackId::parse("rpk_000000000001").unwrap(),
        outcome: DecisionOutcome::Rejected {
            reason: "not this approach".into(),
        },
        decided_by: ActorId::parse("act_000000000001").unwrap(),
        decided_at: Timestamp::from_unix_nanos(0),
        authority: Default::default(),
    };
    store.put(&rejected).unwrap();
    store.put(&rejected).unwrap();

    let flipped = Decision {
        outcome: DecisionOutcome::Approved,
        authority: [SecurityFactRef::new(
            SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap(),
            Some(ScopedId::parse("auth_000000000001").unwrap()),
            Digest::of_bytes(b"grant"),
        )]
        .into_iter()
        .collect(),
        ..rejected.clone()
    };
    assert!(store.put(&flipped).is_err());
    assert_eq!(
        store
            .get(&DecisionId::parse("dec_000000000001").unwrap())
            .unwrap(),
        Some(rejected)
    );
}

#[test]
fn a_gate_evaluation_cannot_be_altered_behind_its_id() {
    use draft_core::evidence::context::{
        EvaluationContext, SecurityContextSnapshot, SecurityDependencySet,
    };
    use draft_core::gate::{GateCondition, GateEvaluation, GateEvaluationStore};
    use draft_dcg_contract::identifier::NamespacedId;
    use draft_dcg_contract::ids::RevisionPackId;
    use draft_dcg_contract::security::PolicyDigest;
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::Digest;

    fn gate(satisfied: bool) -> GateEvaluation {
        GateEvaluation {
            id: "gate_000000000001".into(),
            revision_pack: RevisionPackId::parse("rpk_000000000001").unwrap(),
            definition: Digest::of_bytes(b"definition"),
            scope: Digest::of_bytes(b"scope"),
            evidence: Default::default(),
            assessments: Default::default(),
            conditions: vec![GateCondition {
                id: "tests".into(),
                definition: Digest::of_bytes(b"tests"),
                satisfied,
                detail: (!satisfied).then(|| "the suite failed".to_string()),
            }],
            context: EvaluationContext {
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
            },
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let store = GateEvaluationStore::new(directory.path());
    store.put(&gate(false)).unwrap();
    // Flipping an unsatisfied gate to satisfied is what the binding prevents:
    // a promotion citing this id would otherwise inherit the new answer.
    assert!(store.put(&gate(true)).is_err());
}

#[test]
fn a_substituted_fact_stays_unreadable_rather_than_silently_repaired() {
    // The load path must not "fix" a mismatch by rebinding: history is not
    // repaired in place, and a Store that quietly re-bound would turn a
    // detected tamper into an accepted one.
    let directory = tempfile::tempdir().unwrap();
    let store: ImmutableFactStore<StoredFact> = ImmutableFactStore::new(directory.path());
    let original = StoredFact {
        id: "dec_000000000001".into(),
        revision: "rpk_000000000001".into(),
        outcome: "rejected".into(),
    };
    store.put("dec_000000000001", &original).unwrap();
    let bound = store.bound_digest("dec_000000000001").unwrap();

    std::fs::write(
        store.payload_path("dec_000000000001"),
        serde_json::to_vec_pretty(&StoredFact {
            outcome: "approved".into(),
            ..original
        })
        .unwrap(),
    )
    .unwrap();

    // Repeated reads keep failing, and the binding is untouched.
    for _ in 0..3 {
        assert!(store.get("dec_000000000001").is_err());
    }
    assert_eq!(store.bound_digest("dec_000000000001").unwrap(), bound);
}
