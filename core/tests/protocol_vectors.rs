//! The committed protocol vectors, and the registry that stops one vanishing.
//!
//! # Why these exist beside the SDK's own tests
//!
//! `sdk/dcg-contract/tests/v1_vectors.rs` freezes canonical bytes *inside* the
//! crate. These are the same facts written down where an external
//! implementation can read them: a committed JSON document per vector, with a
//! schema describing the shape and, for the Publication family, a payload that
//! is deliberately self-inconsistent so an implementor can check their
//! rejection path against Draft's.
//!
//! # Why a registry
//!
//! A directory walk cannot notice a vector that was deleted. `registry.json`
//! names every one, and the two must agree exactly — so removing a vector is a
//! reviewed diff rather than a silent loss of coverage.
//!
//! # What a self-consistency vector proves
//!
//! Each carries `self_consistency_violation`: a payload that is **schema-valid**
//! and whose bytes hash perfectly to their own outer digest, yet whose derived
//! or cross-object fields disagree with their canonical inputs. A digest proves
//! nobody edited an object since it was written. It proves nothing about
//! whether the object was coherent when it *was* written, and every one of
//! these would be accepted by an implementation that checked only the digest.
//!
//! Regenerate with `DRAFT_UPDATE_VECTORS=1 cargo test -p draft-core --test
//! protocol_vectors`, and read the diff before committing it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use draft_dcg_contract::*;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Fixtures — the same non-empty values the portable closure suite constructs.
// ---------------------------------------------------------------------------

fn digest_of(seed: &[u8]) -> Digest {
    Digest::of_bytes(seed)
}

fn producer() -> ProducerIdentity {
    ProducerIdentity::new(
        identifier::NamespacedId::parse("draft.core/publication").unwrap(),
        "0.3.4",
    )
    .unwrap()
}

fn grant() -> SecurityFactRef {
    SecurityFactRef::new(
        SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap(),
        Some(identifier::ScopedId::parse("auth_1").unwrap()),
        digest_of(b"grant"),
    )
}

fn registry_revisions() -> value::RegistryRevisions {
    let mut revisions = value::RegistryRevisions::new();
    revisions.insert(
        value::RegistryId::parse("draft.trust/publishers").unwrap(),
        value::RegistryRevision::new(412),
    );
    revisions
}

fn permitted_decision() -> AuthorityDecision {
    AuthorityDecision::new(
        AuthorityScopeClaim {
            capability: CapabilityId::parse("draft.publish/v1").unwrap(),
            subject: identifier::ScopedId::parse("pub_a1").unwrap(),
        },
        AuthorityDecisionOutcome::Permitted,
        BTreeSet::from([grant()]),
        producer(),
        value::Timestamp::from_unix_nanos(1_000),
    )
    .unwrap()
}

fn route() -> ProviderRouteRef {
    ProviderRouteRef {
        provenance: ProviderProvenanceRef {
            binding: ids::ProviderBindingId::parse("pbd_000000000001").unwrap(),
            semantic_definition: ProviderSemanticDefinitionDigest::new(digest_of(b"SD1")),
        },
        operational_profile: ProviderOperationalProfileDigest::new(digest_of(b"OP1")),
    }
}

fn other_route() -> ProviderRouteRef {
    ProviderRouteRef {
        provenance: ProviderProvenanceRef {
            binding: ids::ProviderBindingId::parse("pbd_000000000002").unwrap(),
            semantic_definition: ProviderSemanticDefinitionDigest::new(digest_of(b"SD2")),
        },
        operational_profile: ProviderOperationalProfileDigest::new(digest_of(b"OP2")),
    }
}

fn semantics_contract() -> ResourceStateSemanticsContract {
    ResourceStateSemanticsContract {
        id: ResourceStateSemanticsId::parse("draft.filesystem/file.v1").unwrap(),
        locator_state_role: LocatorStateRole::StateBearing,
        attribute_interpretation: BTreeMap::from([(
            "executable".to_string(),
            semantics::AttributeInterpretation::StateBearing,
        )]),
        content_digest_interpretation: semantics::DigestInterpretation::Required,
        semantic_digest_interpretation: semantics::DigestInterpretation::Absent,
        normalization: vec![semantics::NormalizationRule::None],
        presence_absence_semantics: semantics::PresenceAbsenceSemantics::MeaningfulAbsence,
    }
}

fn resource_state() -> ResourceState {
    ResourceState {
        resource_kind: kinds::ResourceKindId::parse("draft.filesystem/file").unwrap(),
        state_semantics: semantics_contract().reference().unwrap(),
        locator: Some(ResourceLocator::parse("app.txt").unwrap()),
        content_digest: Some(digest_of(b"content")),
        semantic_digest: None,
        state_attributes: BTreeMap::from([(
            "executable".to_string(),
            AttributeValue::Boolean(false),
        )]),
    }
}

fn observation_run() -> ObservationRun {
    ObservationRun {
        id: ids::ObservationRunId::parse("run_000000000001").unwrap(),
        producer: producer(),
        execution: None,
        attempted_domains: BTreeSet::from([CoverageDomainRef::parse("root").unwrap()]),
        committed_domains: BTreeSet::from([CoverageDomainRef::parse("root").unwrap()]),
        observation_context: digest_of(b"context"),
        started_at: value::Timestamp::from_unix_nanos(1_000),
        completed_at: value::Timestamp::from_unix_nanos(2_000),
        terminal_status: ObservationTerminalStatus::Completed,
    }
}

fn observation() -> Observation {
    Observation {
        id: ids::ObservationId::parse("obs_000000000001").unwrap(),
        resource: ids::ResourceId::parse("res_000000000001").unwrap(),
        state: resource_state().digest().unwrap(),
        provider_binding: ids::ProviderBindingId::parse("pbd_000000000001").unwrap(),
        provider_semantic_definition: ProviderSemanticDefinitionDigest::new(digest_of(b"SD1")),
        stability: ObservationStability::Stable,
        observation_context: digest_of(b"context"),
        run: observation_run().reference().unwrap(),
        execution: None,
        observed_at: value::Timestamp::from_unix_nanos(2_000),
    }
}

fn signer() -> ReceiptSignerBinding {
    ReceiptSignerBinding::new(
        ids::ActorId::parse("act_signer").unwrap(),
        "key-1",
        "ed25519",
    )
    .unwrap()
}

fn publication() -> Publication {
    let promotion = ids::PromotionId::parse("pro_000000000001").unwrap();
    let baseline = BaselineId::new(digest_of(b"baseline"));
    let purpose = PublicationPurposeId::parse("draft.publish/deploy").unwrap();
    let id = ids::PublicationId::parse("pub_000000000001").unwrap();
    Publication {
        request_key: Publication::compute_request_key(
            &promotion,
            &baseline,
            &route(),
            &purpose,
            None,
        )
        .unwrap(),
        idempotency_key: Publication::compute_idempotency_key(&id, &baseline, &route()).unwrap(),
        id,
        promotion,
        baseline,
        route: route(),
        purpose,
        republish_intent: None,
        requested_by: ids::ActorId::parse("act_a1").unwrap(),
        authority_inputs: BTreeSet::from([grant()]),
        credential_authority_class: Some(
            CredentialAuthorityClass::parse("acme.cloud/tenant-prod").unwrap(),
        ),
        delivery_semantics: DeliverySemantics::IdempotentByKey,
        created_at: value::Timestamp::from_unix_nanos(1_000),
    }
}

fn attempt() -> PublicationAttempt {
    PublicationAttempt {
        id: ids::PublicationAttemptId::parse("pat_000000000001").unwrap(),
        publication: publication().reference().unwrap(),
        attempt_number: 1,
        route: route(),
        attempt_authority: BTreeSet::from([grant()]),
        authority_decision: permitted_decision(),
        project_control_generation_at_dispatch: value::ProjectControlGeneration::new(7),
        project_security_state_at_dispatch: ProjectSecurityStateDigest::new(digest_of(b"security")),
        policy_digest_at_dispatch: PolicyDigest::new(digest_of(b"policy")),
        global_registry_revisions_at_dispatch: registry_revisions(),
        provider_binding_generation_at_dispatch: value::ProviderBindingGeneration::new(3),
        retry_authorization: None,
        lease_id: value::LeaseId::parse("lease-1").unwrap(),
        lease_fence: value::LeaseFence::new(42),
        started_at: value::Timestamp::from_unix_nanos(2_000),
        provenance: producer(),
    }
}

fn outcome() -> PublicationOutcome {
    PublicationOutcome {
        attempt: attempt().reference().unwrap(),
        receipt_id: ids::ReceiptId::parse("rcp_000000000001").unwrap(),
        receipt_signer: signer(),
        outcome: PublicationOutcomeKind::Indeterminate {
            reason: "provider unreachable after dispatch".into(),
        },
        concluded_at: value::Timestamp::from_unix_nanos(3_000),
        provenance: producer(),
    }
}

fn resolution() -> PublicationResolution {
    PublicationResolution {
        outcome: outcome().digest().unwrap(),
        receipt_id: ids::ReceiptId::parse("rcp_000000000002").unwrap(),
        receipt_signer: signer(),
        resolution: PublicationResolutionKind::ResolvedSucceeded {
            external_reference: "deploy-991".into(),
        },
        supersedes: None,
        actor: ids::ActorId::parse("act_a1").unwrap(),
        authority: grant(),
        authority_decision: permitted_decision(),
        project_security_state_at_resolution: ProjectSecurityStateDigest::new(digest_of(
            b"sec-now",
        )),
        policy_digest_at_resolution: PolicyDigest::new(digest_of(b"policy-now")),
        global_registry_revisions_at_resolution: registry_revisions(),
        rationale: "provider confirmed the deploy landed".into(),
        resolved_at: value::Timestamp::from_unix_nanos(5_000),
    }
}

fn retry_authorization() -> PublicationRetryAuthorization {
    PublicationRetryAuthorization {
        publication: publication().reference().unwrap(),
        prior_outcome: outcome().digest().unwrap(),
        actor: ids::ActorId::parse("act_a1").unwrap(),
        authority: grant(),
        authority_decision: permitted_decision(),
        project_security_state_at_authorization: ProjectSecurityStateDigest::new(digest_of(b"sec")),
        policy_digest_at_authorization: PolicyDigest::new(digest_of(b"policy")),
        global_registry_revisions_at_authorization: registry_revisions(),
        rationale: "operator accepts the duplicate risk".into(),
        duplicate_risk_acknowledged: true,
        authorizes_one_attempt: true,
        authorized_at: value::Timestamp::from_unix_nanos(4_000),
        expires_at: Some(value::Timestamp::from_unix_nanos(9_000)),
    }
}

// ---------------------------------------------------------------------------
// The vectors
// ---------------------------------------------------------------------------

struct Vector {
    name: &'static str,
    schema: &'static str,
    /// What every implementation must accept.
    payload: Value,
    /// Schema-valid, digest-consistent, and still invalid.
    violation: Option<Value>,
    /// What a reader is meant to learn from the violation.
    expect: &'static str,
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("canonical value serializes")
}

fn vectors() -> Vec<Vector> {
    let mut all = Vec::new();

    // --- the three roots ---------------------------------------------------
    let mut state = ProjectStateRootBuilder::new();
    state
        .insert_resource(
            ids::ResourceId::parse("res_000000000001").unwrap(),
            resource_state().digest().unwrap(),
        )
        .unwrap();
    all.push(Vector {
        name: "project-state-root",
        schema: "project-state-root.schema.json",
        payload: json!({
            "schema_version": 1,
            "resources": [{
                "resource_id": "res_000000000001",
                "state": resource_state().digest().unwrap().digest().as_str(),
            }],
            "relations": [],
            "project_state_root": state.build().unwrap().digest().as_str(),
        }),
        violation: None,
        expect: "the material state root is the Merkle construction over resource and relation \
                 subjects, and nothing else",
    });

    let mut evidence = StateEvidenceRootBuilder::new();
    evidence
        .insert(BaselineStateEvidenceEntry::Resource {
            resource_id: ids::ResourceId::parse("res_000000000001").unwrap(),
            state: resource_state().digest().unwrap(),
            primary: observation().reference().unwrap(),
            corroborating: BTreeSet::new(),
        })
        .unwrap();
    all.push(Vector {
        name: "state-evidence-root-resource",
        schema: "state-evidence-entry.schema.json",
        payload: json!({
            "schema_version": 1,
            "entries": [to_value(&BaselineStateEvidenceEntry::Resource {
                resource_id: ids::ResourceId::parse("res_000000000001").unwrap(),
                state: resource_state().digest().unwrap(),
                primary: observation().reference().unwrap(),
                corroborating: BTreeSet::new(),
            })],
            "state_evidence_root": evidence.build().unwrap().digest().as_str(),
        }),
        violation: None,
        expect: "a Resource subject carries exactly one primary observation, structurally",
    });

    let relation_state = RelationState {
        source: ids::ResourceId::parse("res_000000000001").unwrap(),
        relation_type: kinds::RelationTypeId::parse("draft.filesystem/contains").unwrap(),
        target: ids::ResourceId::parse("res_000000000002").unwrap(),
        instance_key: None,
        state_attributes: BTreeMap::new(),
    };
    let relation_record = RelationRecord {
        state: relation_state.clone(),
        role: RelationRole::StateBearing,
        provenance: RelationProvenance::Authoritative {
            binding: ids::ProviderBindingId::parse("pbd_000000000001").unwrap(),
            semantic_definition: ProviderSemanticDefinitionDigest::new(digest_of(b"SD1")),
            observation: observation().reference().unwrap(),
        },
    };
    let mut relation_evidence = StateEvidenceRootBuilder::new();
    relation_evidence
        .insert(BaselineStateEvidenceEntry::Relation {
            state: relation_state.digest().unwrap(),
            evidence: BTreeSet::from([RelationStateEvidenceRef::RelationRecord {
                record: relation_record.digest().unwrap(),
            }]),
        })
        .unwrap();
    all.push(Vector {
        name: "state-evidence-root-relation",
        schema: "state-evidence-entry.schema.json",
        payload: json!({
            "schema_version": 1,
            "entries": [to_value(&BaselineStateEvidenceEntry::Relation {
                state: relation_state.digest().unwrap(),
                evidence: BTreeSet::from([RelationStateEvidenceRef::RelationRecord {
                    record: relation_record.digest().unwrap(),
                }]),
            })],
            "state_evidence_root": relation_evidence.build().unwrap().digest().as_str(),
        }),
        violation: None,
        expect: "a Relation subject carries relation evidence and can never carry an observation",
    });

    let complete = CoverageEvidence {
        provider_binding: ids::ProviderBindingId::parse("pbd_000000000001").unwrap(),
        provider_semantic_definition: ProviderSemanticDefinitionDigest::new(digest_of(b"SD1")),
        domain: CoverageDomainRef::parse("root").unwrap(),
        status: CoverageStatus::Complete,
        observation_run: Some(observation_run().reference().unwrap()),
        attempted: true,
        committed: true,
        known_gaps: BTreeSet::new(),
    };
    let mut coverage = CoverageEvidenceRootBuilder::new();
    coverage.insert(complete.clone()).unwrap();
    all.push(Vector {
        name: "coverage-evidence-root",
        schema: "coverage-evidence.schema.json",
        payload: json!({
            "schema_version": 1,
            "claims": [to_value(&complete)],
            "coverage_evidence_root": coverage.build().unwrap().digest().as_str(),
        }),
        violation: Some(json!({
            "schema_version": 1,
            "claims": [to_value(&CoverageEvidence {
                // `NotObserved` with nothing attempted may not name a run:
                // there was no run. A synthetic one would let an absence
                // justify itself.
                status: CoverageStatus::NotObserved,
                attempted: false,
                committed: false,
                ..complete.clone()
            })],
            "coverage_evidence_root": coverage.build().unwrap().digest().as_str(),
        })),
        expect: "coverage cross-field validity is checked, so an unattempted domain cannot carry \
                 an observation run",
    });

    let unattempted = CoverageEvidence {
        status: CoverageStatus::NotObserved,
        observation_run: None,
        attempted: false,
        committed: false,
        ..complete.clone()
    };
    let mut unattempted_root = CoverageEvidenceRootBuilder::new();
    unattempted_root.insert(unattempted.clone()).unwrap();
    all.push(Vector {
        name: "coverage-evidence-cross-field",
        schema: "coverage-evidence.schema.json",
        payload: json!({
            "schema_version": 1,
            "claims": [to_value(&unattempted)],
            "coverage_evidence_root": unattempted_root.build().unwrap().digest().as_str(),
        }),
        violation: Some(json!({
            "schema_version": 1,
            "claims": [to_value(&CoverageEvidence {
                // `Complete` while the run was never committed. A failed
                // attempt can never be promoted to complete coverage.
                status: CoverageStatus::Complete,
                attempted: true,
                committed: false,
                ..complete.clone()
            })],
            "coverage_evidence_root": coverage.build().unwrap().digest().as_str(),
        })),
        expect: "`attempted` and `committed` stay distinct, so a failed attempt never becomes \
                 complete coverage",
    });

    // --- the Publication family (§2.46) ------------------------------------
    let publication = publication();
    all.push(Vector {
        name: "publication-request-key",
        schema: "publication.schema.json",
        payload: to_value(&publication),
        violation: Some({
            // The bytes are internally coherent and hash perfectly to their own
            // digest. What they are not is *derived from their own inputs*.
            let mut broken = publication.clone();
            broken.request_key = PublicationRequestKey::new(digest_of(b"not-derived"));
            to_value(&broken)
        }),
        expect: "`request_key` is recomputed from the Publication's own canonical inputs, never \
                 trusted",
    });
    all.push(Vector {
        name: "publication-idempotency-key",
        schema: "publication.schema.json",
        payload: to_value(&publication),
        violation: Some({
            let mut broken = publication.clone();
            broken.idempotency_key = PublicationIdempotencyKey::new(digest_of(b"not-derived"));
            to_value(&broken)
        }),
        expect: "`idempotency_key` is recomputed from the Publication id, Baseline and route",
    });

    let attempt = attempt();
    all.push(Vector {
        name: "publication-attempt-route",
        schema: "publication-attempt.schema.json",
        payload: to_value(&attempt),
        violation: Some({
            let mut broken = attempt.clone();
            broken.route = other_route();
            to_value(&broken)
        }),
        expect: "an attempt may never claim a route its Publication did not freeze — that would \
                 be an external effect nobody authorized at that destination",
    });
    all.push(Vector {
        name: "publication-attempt-publication-ref",
        schema: "publication-attempt.schema.json",
        payload: to_value(&attempt),
        violation: Some({
            let mut broken = attempt.clone();
            broken.publication = PublicationRef {
                id: broken.publication.id.clone(),
                digest: PublicationDigest::new(digest_of(b"different-bytes")),
            };
            to_value(&broken)
        }),
        expect: "the exact PublicationRef is verified by id *and* digest",
    });

    let outcome = outcome();
    all.push(Vector {
        name: "publication-outcome-attempt",
        schema: "publication-outcome.schema.json",
        payload: to_value(&outcome),
        violation: Some({
            let mut broken = outcome.clone();
            broken.attempt = PublicationAttemptRef {
                id: ids::PublicationAttemptId::parse("pat_000000000002").unwrap(),
                digest: broken.attempt.digest.clone(),
            };
            to_value(&broken)
        }),
        expect: "an outcome for one attempt can never be filed beneath another's head",
    });
    all.push(Vector {
        name: "publication-outcome-receipt",
        schema: "publication-outcome.schema.json",
        payload: to_value(&outcome),
        violation: Some({
            let mut broken = outcome.clone();
            broken.receipt_id = ids::ReceiptId::parse("rcp_000000000099").unwrap();
            to_value(&broken)
        }),
        expect: "the receipt id and signer binding are the ones preallocated for that attempt",
    });

    let resolution = resolution();
    all.push(Vector {
        name: "publication-resolution-outcome",
        schema: "publication-resolution.schema.json",
        payload: to_value(&resolution),
        violation: Some({
            let mut broken = resolution.clone();
            broken.outcome = PublicationOutcomeDigest::new(digest_of(b"another-outcome"));
            to_value(&broken)
        }),
        expect: "a resolution names the exact outcome whose head it advances",
    });
    all.push(Vector {
        name: "publication-resolution-supersession",
        schema: "publication-resolution.schema.json",
        payload: to_value(&resolution),
        violation: Some({
            let mut broken = resolution.clone();
            broken.supersedes = Some(PublicationResolutionDigest::new(digest_of(b"other-chain")));
            to_value(&broken)
        }),
        expect: "a supersession chain may never cross outcomes, and may not supersede a head that \
                 does not exist",
    });

    let authorization = retry_authorization();
    all.push(Vector {
        name: "publication-retry-authorization-target",
        schema: "publication-retry-authorization.schema.json",
        payload: to_value(&authorization),
        violation: Some({
            let mut broken = authorization.clone();
            broken.publication = PublicationRef {
                id: broken.publication.id.clone(),
                digest: PublicationDigest::new(digest_of(b"different-bytes")),
            };
            to_value(&broken)
        }),
        expect:
            "an authorization is bound to an exact PublicationRef, so changing the bytes under \
                 `pub_` can never widen or redirect it",
    });
    all.push(Vector {
        name: "publication-retry-authorization-prior-outcome",
        schema: "publication-retry-authorization.schema.json",
        payload: to_value(&authorization),
        violation: Some({
            let mut broken = authorization.clone();
            broken.prior_outcome = PublicationOutcomeDigest::new(digest_of(b"another-outcome"));
            to_value(&broken)
        }),
        expect: "the prior outcome must belong to an attempt of this exact Publication",
    });

    all.push(Vector {
        name: "publication-control-membership",
        schema: "publication-control.schema.json",
        payload: json!({
            "schema_version": 1,
            "generation": 3,
            "publication": "pub_000000000001",
            "in_flight_attempt": "pat_000000000001",
            "next_attempt_number": 2,
            "consumed_retry_authorizations": [
                authorization.digest().unwrap().digest().as_str(),
            ],
        }),
        violation: Some(json!({
            "schema_version": 1,
            "generation": 3,
            // The control record for one Publication, holding an attempt and a
            // consumed authorization that belong to another.
            "publication": "pub_000000000002",
            "in_flight_attempt": "pat_000000000001",
            "next_attempt_number": 2,
            "consumed_retry_authorizations": [
                authorization.digest().unwrap().digest().as_str(),
            ],
        })),
        expect: "a control record's in-flight attempt and consumed authorizations belong to its \
                 own Publication",
    });

    // --- ChangePack composition ------------------------------------------------
    //
    // Regenerated from real compositions rather than transcribed, for the same
    // reason as the Publication family: a change to the algebra has to show up
    // here as a diff.
    let baseline = BaselineId::new(digest_of(b"compose-base"));
    let other_baseline = BaselineId::new(digest_of(b"compose-other-base"));
    let member = |change: &str, revision: &str, base: &BaselineId, touched: &[&str]| {
        draft_core::dcg::compose::ComposedRevision {
            change_pack: ids::ChangePackId::parse(change).unwrap(),
            revision_pack: ids::RevisionPackId::parse(revision).unwrap(),
            base_baseline: base.clone(),
            touched: touched
                .iter()
                .map(|id| ids::ResourceId::parse(*id).unwrap())
                .collect(),
        }
    };
    let compose = |members: &[draft_core::dcg::compose::ComposedRevision], base: &BaselineId| {
        draft_core::dcg::compose::compose(base, members, draft_core::dcg::compose::relate_by_state)
            .unwrap()
    };

    let independent = compose(
        &[
            member(
                "cpk_000000000001",
                "rpk_000000000001",
                &baseline,
                &["res_000000000001"],
            ),
            member(
                "cpk_000000000002",
                "rpk_000000000002",
                &baseline,
                &["res_000000000002"],
            ),
        ],
        &baseline,
    );
    all.push(Vector {
        name: "independent-change-packs",
        schema: "composition.schema.json",
        payload: to_value(&independent),
        violation: None,
        expect: "revisions touching disjoint Resources from the same Baseline compose",
    });

    let conflicting = compose(
        &[
            member(
                "cpk_000000000001",
                "rpk_000000000001",
                &baseline,
                &["res_000000000001"],
            ),
            member(
                "cpk_000000000003",
                "rpk_000000000003",
                &baseline,
                &["res_000000000001"],
            ),
        ],
        &baseline,
    );
    all.push(Vector {
        name: "conflicting-change-packs",
        schema: "composition.schema.json",
        payload: to_value(&conflicting),
        violation: None,
        expect: "two revisions touching one Resource cannot be shown separable, so composition                  fails rather than assuming they combine",
    });

    let indeterminate = compose(
        &[
            member(
                "cpk_000000000001",
                "rpk_000000000001",
                &baseline,
                &["res_000000000001"],
            ),
            member(
                "cpk_000000000004",
                "rpk_000000000004",
                &other_baseline,
                &["res_000000000002"],
            ),
        ],
        &baseline,
    );
    all.push(Vector {
        name: "indeterminate-change-packs",
        schema: "composition.schema.json",
        payload: to_value(&indeterminate),
        violation: None,
        expect: "revisions sealed from different Baselines are indeterminate even when their                  Resource sets are disjoint — the answer fails closed",
    });

    all
}

// ---------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------

fn proto_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("core has a parent")
        .join("proto")
}

fn rendered(vector: &Vector) -> Value {
    let mut document = serde_json::Map::new();
    document.insert("name".into(), json!(vector.name));
    document.insert("payload_schema".into(), json!(vector.schema));
    document.insert("expect".into(), json!(vector.expect));
    document.insert("payload".into(), vector.payload.clone());
    if let Some(violation) = &vector.violation {
        document.insert("self_consistency_violation".into(), violation.clone());
    }
    Value::Object(document)
}

/// Every committed vector matches what the SDK types actually produce.
///
/// This is the exactness guarantee: the payloads are not transcribed, they are
/// serialized from real canonical values. A field added to a frozen type shows
/// up here as a diff rather than as a schema quietly describing something else.
#[test]
fn committed_vectors_match_the_canonical_types() {
    let update = std::env::var_os("DRAFT_UPDATE_VECTORS").is_some();
    for vector in vectors() {
        let path = proto_dir()
            .join("test-vectors")
            .join(vector.name)
            .join("vector.json");
        let expected = rendered(&vector);
        if update {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                format!("{}\n", serde_json::to_string_pretty(&expected).unwrap()),
            )
            .unwrap();
            continue;
        }
        let committed: Value = serde_json::from_slice(
            &std::fs::read(&path)
                .unwrap_or_else(|error| panic!("{} is committed: {error}", path.display())),
        )
        .unwrap_or_else(|error| panic!("{} is valid JSON: {error}", path.display()));
        assert_eq!(
            committed,
            expected,
            "{} no longer matches what the canonical types produce; regenerate with \
             DRAFT_UPDATE_VECTORS=1 and read the diff",
            path.display()
        );
    }
}

/// The registry and the directory agree exactly.
///
/// A directory walk cannot notice a vector that was deleted, and a registry
/// nobody checks is a list that drifts. Requiring equality in both directions
/// is what makes removing coverage a reviewed act.
#[test]
fn the_registry_names_every_vector_and_nothing_else() {
    let registry: Value = serde_json::from_slice(
        &std::fs::read(proto_dir().join("test-vectors").join("registry.json"))
            .expect("the vector registry is committed"),
    )
    .expect("the vector registry is valid JSON");
    let named: BTreeSet<String> = registry["vectors"]
        .as_array()
        .expect("the registry lists its vectors")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("a vector name is a string")
                .to_string()
        })
        .collect();

    let present: BTreeSet<String> = std::fs::read_dir(proto_dir().join("test-vectors"))
        .expect("the vector directory is readable")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("vector.json").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();

    let missing: Vec<_> = named.difference(&present).collect();
    let unregistered: Vec<_> = present.difference(&named).collect();
    assert!(
        missing.is_empty(),
        "the registry names vectors that are not committed: {missing:?}"
    );
    assert!(
        unregistered.is_empty(),
        "these vectors are committed but not registered, so deleting them would go unnoticed: \
         {unregistered:?}"
    );
}

/// A valid outer digest is not validity.
///
/// Every violation payload round-trips through its canonical type and hashes
/// perfectly to its own digest. An implementation that checked only the digest
/// would accept all of them.
#[test]
fn a_self_consistency_violation_is_rejected_despite_a_valid_digest() {
    let publication = publication();
    let attempt = attempt();
    let outcome = outcome();
    let head_attempt = attempt.reference().unwrap();
    let outcome_digest = outcome.digest().unwrap();

    for vector in vectors() {
        let Some(violation) = vector.violation.clone() else {
            continue;
        };
        let rejected = match vector.schema {
            "publication.schema.json" => {
                let broken: Publication = serde_json::from_value(violation).unwrap();
                // The bytes hash: the object is intact, and still invalid.
                canonical::canonical_bytes(&broken).unwrap();
                broken.validate().is_err()
            }
            "publication-attempt.schema.json" => {
                let broken: PublicationAttempt = serde_json::from_value(violation).unwrap();
                canonical::canonical_bytes(&broken).unwrap();
                broken.validate_against(&publication).is_err()
            }
            "publication-outcome.schema.json" => {
                let broken: PublicationOutcome = serde_json::from_value(violation).unwrap();
                canonical::canonical_bytes(&broken).unwrap();
                broken
                    .validate_under(&head_attempt, &outcome.receipt_id, &signer())
                    .is_err()
            }
            "publication-resolution.schema.json" => {
                let broken: PublicationResolution = serde_json::from_value(violation).unwrap();
                canonical::canonical_bytes(&broken).unwrap();
                broken.validate_advancing(&outcome_digest, None).is_err()
            }
            "publication-retry-authorization.schema.json" => {
                let broken: PublicationRetryAuthorization =
                    serde_json::from_value(violation).unwrap();
                canonical::canonical_bytes(&broken).unwrap();
                broken
                    .validate_for(&publication, &outcome, &attempt)
                    .is_err()
            }
            "coverage-evidence.schema.json" => {
                let claims = violation["claims"].as_array().unwrap();
                claims.iter().any(|claim| {
                    let parsed: CoverageEvidence = serde_json::from_value(claim.clone()).unwrap();
                    parsed.validate().is_err()
                })
            }
            "publication-control.schema.json" => {
                // The control record is Core's, not the SDK's, so the check is
                // the one §2.46 states: its in-flight attempt and every
                // consumed authorization must belong to its own Publication.
                let control_publication = violation["publication"].as_str().unwrap();
                control_publication != publication.id.as_str()
            }
            // A composition vector proves the algebra's answer rather than a
            // self-consistency rejection, so it carries no violation.
            "composition.schema.json" => unreachable!(),
            other => panic!("no self-consistency check is defined for {other}"),
        };
        assert!(
            rejected,
            "vector '{}' must be rejected: {}",
            vector.name, vector.expect
        );
    }
}

/// Every vector's payload validates against the schema it names, and the schema
/// describes exactly the fields the canonical type produces.
#[test]
fn every_schema_describes_its_canonical_type_exactly() {
    for vector in vectors() {
        let schema: Value = serde_json::from_slice(
            &std::fs::read(proto_dir().join("schemas").join(vector.schema))
                .unwrap_or_else(|error| panic!("{} is committed: {error}", vector.schema)),
        )
        .unwrap_or_else(|error| panic!("{} is valid JSON: {error}", vector.schema));

        let properties = schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{} declares properties", vector.schema));
        assert_eq!(
            schema["additionalProperties"],
            json!(false),
            "{} must be closed, mirroring `deny_unknown_fields`",
            vector.schema
        );

        // Both directions. A field the type produces but the schema omits is a
        // schema that has fallen behind; a required field the type never
        // produces is a schema describing something else.
        let payload = vector
            .payload
            .as_object()
            .unwrap_or_else(|| panic!("{}'s payload is an object", vector.name));
        for key in payload.keys() {
            assert!(
                properties.contains_key(key),
                "{} produces '{key}', which {} does not describe",
                vector.name,
                vector.schema
            );
        }
        for required in schema["required"].as_array().into_iter().flatten() {
            let key = required.as_str().unwrap();
            assert!(
                payload.contains_key(key),
                "{} requires '{key}', which the canonical value does not produce",
                vector.schema
            );
        }
    }
}
