//! Scenario ER: the portable DCG contract really is Core-free.
//!
//! This suite constructs every canonical type the Publication family and the
//! Baseline roots depend on, **with non-empty nested values** — security,
//! trust, lease, generation and authority fields all populated — then
//! serializes, reparses and recomputes each digest.
//!
//! The point is not that these types work. It is that none of them needs
//! anything outside this crate to exist. `scripts/check-portable-contract-
//! closure.sh` runs the same construction from a scratch crate outside the
//! workspace and inspects the dependency graph, which proves the package
//! boundary mechanically rather than by documentation.

use std::collections::{BTreeMap, BTreeSet};

use draft_dcg_contract::*;

fn digest(seed: &[u8]) -> Digest {
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
        digest(b"grant"),
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

fn permitted_decision(subject: &str) -> AuthorityDecision {
    AuthorityDecision::new(
        AuthorityScopeClaim {
            capability: CapabilityId::parse("draft.publish/v1").unwrap(),
            subject: identifier::ScopedId::parse(subject).unwrap(),
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
            binding: ids::ProviderBindingId::parse("pbd_a1").unwrap(),
            semantic_definition: ProviderSemanticDefinitionDigest::new(digest(b"SD1")),
        },
        operational_profile: ProviderOperationalProfileDigest::new(digest(b"OP1")),
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

fn signer() -> ReceiptSignerBinding {
    ReceiptSignerBinding::new(
        ids::ActorId::parse("act_signer").unwrap(),
        "key-1",
        "ed25519",
    )
    .unwrap()
}

/// Round-trip a canonical value through its wire form.
fn round_trip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let encoded = serde_json::to_string(value).expect("canonical value serializes");
    let decoded: T = serde_json::from_str(&encoded).expect("canonical value reparses");
    assert_eq!(&decoded, value, "wire form must round-trip exactly");
    decoded
}

#[test]
fn the_state_substrate_is_constructible_and_verifiable() {
    let contract = semantics_contract();
    let reference = contract.reference().unwrap();
    reference.verify(&contract).unwrap();
    round_trip(&contract);

    let state = ResourceState {
        resource_kind: kinds::ResourceKindId::parse("draft.filesystem/file").unwrap(),
        state_semantics: reference,
        locator: Some(ResourceLocator::parse("app.txt").unwrap()),
        content_digest: Some(digest(b"hello\n")),
        semantic_digest: None,
        state_attributes: BTreeMap::from([(
            "executable".to_string(),
            AttributeValue::Boolean(false),
        )]),
    };
    state.validate_against(&contract).unwrap();
    let state_digest = state.digest().unwrap();
    round_trip(&state);

    let run = ObservationRun {
        id: ids::ObservationRunId::parse("run_a1").unwrap(),
        producer: producer(),
        execution: Some(ids::ExecutionId::parse("exe_a1").unwrap()),
        attempted_domains: BTreeSet::from([CoverageDomainRef::parse("root").unwrap()]),
        committed_domains: BTreeSet::from([CoverageDomainRef::parse("root").unwrap()]),
        observation_context: digest(b"context"),
        started_at: value::Timestamp::from_unix_nanos(1_000),
        completed_at: value::Timestamp::from_unix_nanos(2_000),
        terminal_status: ObservationTerminalStatus::Completed,
    };
    run.validate().unwrap();
    let run_ref = run.reference().unwrap();
    run_ref.verify(&run).unwrap();

    let observation = Observation {
        id: ids::ObservationId::parse("obs_a1").unwrap(),
        resource: ids::ResourceId::parse("res_a1").unwrap(),
        state: state_digest.clone(),
        provider_binding: ids::ProviderBindingId::parse("pbd_a1").unwrap(),
        provider_semantic_definition: ProviderSemanticDefinitionDigest::new(digest(b"SD1")),
        stability: ObservationStability::Stable,
        observation_context: digest(b"context"),
        run: run_ref.clone(),
        execution: None,
        observed_at: value::Timestamp::from_unix_nanos(1_500),
    };
    let observation_ref = observation.reference().unwrap();
    observation_ref.verify(&observation).unwrap();
    round_trip(&observation);

    // The three roots, with a complete bijection.
    let mut state_root = ProjectStateRootBuilder::new();
    state_root
        .insert_resource(
            ids::ResourceId::parse("res_a1").unwrap(),
            state_digest.clone(),
        )
        .unwrap();

    let mut evidence_root = StateEvidenceRootBuilder::new();
    evidence_root
        .insert(BaselineStateEvidenceEntry::Resource {
            resource_id: ids::ResourceId::parse("res_a1").unwrap(),
            state: state_digest,
            primary: observation_ref,
            corroborating: BTreeSet::new(),
        })
        .unwrap();
    evidence_root.verify_bijection(&state_root).unwrap();

    let mut coverage_root = CoverageEvidenceRootBuilder::new();
    coverage_root
        .insert(CoverageEvidence {
            provider_binding: ids::ProviderBindingId::parse("pbd_a1").unwrap(),
            provider_semantic_definition: ProviderSemanticDefinitionDigest::new(digest(b"SD1")),
            domain: CoverageDomainRef::parse("root").unwrap(),
            status: CoverageStatus::Complete,
            observation_run: Some(run_ref),
            attempted: true,
            committed: true,
            known_gaps: BTreeSet::new(),
        })
        .unwrap();

    let manifest = BaselineManifest {
        project: ids::ProjectId::parse("prj_a1").unwrap(),
        project_state_root: state_root.build().unwrap(),
        state_evidence_root: evidence_root.build().unwrap(),
        coverage_evidence_root: coverage_root.build().unwrap(),
        parent_baseline_id: None,
        format_revision: DCG_FORMAT_REVISION,
    };
    manifest.baseline_id().unwrap();
    round_trip(&manifest);
}

#[test]
fn relations_and_their_authorized_promotion_are_constructible() {
    let state = RelationState {
        source: ids::ResourceId::parse("res_source").unwrap(),
        relation_type: kinds::RelationTypeId::parse("acme.crm/owns").unwrap(),
        target: ids::ResourceId::parse("res_target").unwrap(),
        instance_key: Some(RelationInstanceKey::parse("primary").unwrap()),
        state_attributes: BTreeMap::from([("weight".to_string(), AttributeValue::Integer(3))]),
    };

    let record = RelationRecord {
        state: state.clone(),
        role: RelationRole::Derived,
        provenance: RelationProvenance::Derived {
            producer: producer(),
            inputs: vec![ObservationRef {
                id: ids::ObservationId::parse("obs_a1").unwrap(),
                digest: ObservationDigest::new(digest(b"observation")),
            }],
            derivation: digest(b"rule"),
        },
    };
    record.validate().unwrap();

    let declaration = StateBearingDeclaration {
        relation_state: state.digest().unwrap(),
        source_relation_record: record.digest().unwrap(),
        producer: producer(),
        policy_digest: PolicyDigest::new(digest(b"policy")),
        authorizing_grant: grant(),
        actor: ids::ActorId::parse("act_a1").unwrap(),
        declared_at: value::Timestamp::from_unix_nanos(1_000),
    };
    declaration.verify_promotes(&record).unwrap();
    declaration.digest().unwrap();
    round_trip(&declaration);
}

#[test]
fn the_whole_publication_family_is_constructible_with_non_empty_nested_values() {
    let promotion = ids::PromotionId::parse("pro_a1").unwrap();
    let baseline = BaselineId::new(digest(b"baseline"));
    let purpose = PublicationPurposeId::parse("draft.publish/deploy").unwrap();
    let publication_id = ids::PublicationId::parse("pub_a1").unwrap();

    let publication = Publication {
        request_key: Publication::compute_request_key(
            &promotion,
            &baseline,
            &route(),
            &purpose,
            None,
        )
        .unwrap(),
        idempotency_key: Publication::compute_idempotency_key(&publication_id, &baseline, &route())
            .unwrap(),
        id: publication_id,
        promotion,
        baseline,
        route: route(),
        purpose,
        republish_intent: Some(RepublishIntentId::parse("rerun-1").unwrap()),
        requested_by: ids::ActorId::parse("act_a1").unwrap(),
        authority_inputs: BTreeSet::from([grant()]),
        credential_authority_class: Some(
            CredentialAuthorityClass::parse("acme.cloud/tenant-prod").unwrap(),
        ),
        delivery_semantics: DeliverySemantics::IdempotentByKey,
        created_at: value::Timestamp::from_unix_nanos(1_000),
    };
    // The republish intent changes the request key, so recompute and rebuild.
    let publication = Publication {
        request_key: Publication::compute_request_key(
            &publication.promotion,
            &publication.baseline,
            &publication.route,
            &publication.purpose,
            publication.republish_intent.as_ref(),
        )
        .unwrap(),
        ..publication
    };
    publication.validate().unwrap();
    let publication_ref = publication.reference().unwrap();
    publication.verify_reference(&publication_ref).unwrap();
    round_trip(&publication);

    // Every nested security, trust, lease and generation value is populated.
    let attempt = PublicationAttempt {
        id: ids::PublicationAttemptId::parse("pat_a1").unwrap(),
        publication: publication_ref.clone(),
        attempt_number: 1,
        route: route(),
        attempt_authority: BTreeSet::from([grant()]),
        authority_decision: permitted_decision("pub_a1"),
        project_control_generation_at_dispatch: value::ProjectControlGeneration::new(7),
        project_security_state_at_dispatch: ProjectSecurityStateDigest::new(digest(b"security")),
        policy_digest_at_dispatch: PolicyDigest::new(digest(b"policy")),
        global_registry_revisions_at_dispatch: registry_revisions(),
        provider_binding_generation_at_dispatch: value::ProviderBindingGeneration::new(3),
        retry_authorization: None,
        lease_id: value::LeaseId::parse("lease-1").unwrap(),
        lease_fence: value::LeaseFence::new(42),
        started_at: value::Timestamp::from_unix_nanos(2_000),
        provenance: producer(),
    };
    attempt.validate_against(&publication).unwrap();
    let attempt_ref = attempt.reference().unwrap();
    attempt.verify_reference(&attempt_ref).unwrap();
    round_trip(&attempt);

    let outcome = PublicationOutcome {
        attempt: attempt_ref.clone(),
        receipt_id: ids::ReceiptId::parse("rcp_a1").unwrap(),
        receipt_signer: signer(),
        outcome: PublicationOutcomeKind::Indeterminate {
            reason: "provider unreachable after dispatch".into(),
        },
        concluded_at: value::Timestamp::from_unix_nanos(3_000),
        provenance: producer(),
    };
    outcome
        .validate_under(
            &attempt_ref,
            &ids::ReceiptId::parse("rcp_a1").unwrap(),
            &signer(),
        )
        .unwrap();
    let outcome_digest = outcome.digest().unwrap();
    round_trip(&outcome);

    let resolution = PublicationResolution {
        outcome: outcome_digest.clone(),
        receipt_id: ids::ReceiptId::parse("rcp_r1").unwrap(),
        receipt_signer: signer(),
        resolution: PublicationResolutionKind::ResolvedSucceeded {
            external_reference: "deploy-991".into(),
        },
        supersedes: None,
        actor: ids::ActorId::parse("act_a1").unwrap(),
        authority: grant(),
        authority_decision: permitted_decision("pub_a1"),
        project_security_state_at_resolution: ProjectSecurityStateDigest::new(digest(b"sec-now")),
        policy_digest_at_resolution: PolicyDigest::new(digest(b"policy-now")),
        global_registry_revisions_at_resolution: registry_revisions(),
        rationale: "provider confirmed the deploy landed".into(),
        resolved_at: value::Timestamp::from_unix_nanos(5_000),
    };
    resolution
        .validate_advancing(&outcome_digest, None)
        .unwrap();
    resolution.digest().unwrap();
    round_trip(&resolution);

    let authorization = PublicationRetryAuthorization {
        publication: publication_ref,
        prior_outcome: outcome_digest,
        actor: ids::ActorId::parse("act_a1").unwrap(),
        authority: grant(),
        authority_decision: permitted_decision("pub_a1"),
        project_security_state_at_authorization: ProjectSecurityStateDigest::new(digest(b"sec")),
        policy_digest_at_authorization: PolicyDigest::new(digest(b"policy")),
        global_registry_revisions_at_authorization: registry_revisions(),
        rationale: "operator accepts the duplicate risk".into(),
        duplicate_risk_acknowledged: true,
        authorizes_one_attempt: true,
        authorized_at: value::Timestamp::from_unix_nanos(4_000),
        expires_at: Some(value::Timestamp::from_unix_nanos(9_000)),
    };
    authorization
        .validate_for(&publication, &outcome, &attempt)
        .unwrap();
    authorization.digest().unwrap();
    round_trip(&authorization);
}

#[test]
fn a_receipt_signs_its_payload_and_signer_binding_together() {
    let payload = ReceiptPayload {
        receipt_id: ids::ReceiptId::parse("rcp_a1").unwrap(),
        subject: ReceiptKind::Promotion {
            promotion: ids::PromotionId::parse("pro_a1").unwrap(),
            baseline: BaselineId::new(digest(b"baseline")),
        },
        issued_by: ids::ActorId::parse("act_issuer").unwrap(),
        issued_at: value::Timestamp::from_unix_nanos(1_000),
    };
    let message = ReceiptSigningMessage {
        payload: payload.clone(),
        signer: signer(),
    };
    // The signing bytes exist and are derived from payload *and* signer, with
    // no signing capability in this crate.
    let bytes = message.signing_bytes().unwrap();
    assert!(!bytes.is_empty());

    let mut other = message.clone();
    other.signer = ReceiptSignerBinding::new(
        ids::ActorId::parse("act_other").unwrap(),
        "key-9",
        "ed25519",
    )
    .unwrap();
    assert_ne!(
        bytes,
        other.signing_bytes().unwrap(),
        "the signer binding must be inside the signed bytes"
    );

    round_trip(&message);
    let envelope = ReceiptEnvelope {
        payload,
        signer: signer(),
        signature: "AA==".into(),
    };
    // Structure is checkable here; trust is deliberately not.
    assert!(envelope.verify_structure().is_ok());
}
