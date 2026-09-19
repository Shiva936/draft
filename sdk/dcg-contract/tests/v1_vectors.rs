//! Frozen v1 canonical vectors for the portable DCG contract.
//!
//! These assert exact **bytes and digests**, not behaviour. Every behavioural
//! test in this crate would still pass if a canonical encoding, a field order,
//! a domain separator or a Merkle rule changed — and every historical Baseline,
//! Observation and Publication ever written would silently stop verifying.
//! These vectors are what make that impossible to do by accident.
//!
//! A failure here is never fixed by pasting in the new value. The question is
//! whether the canonical form was meant to change at all, and for v1 the answer
//! is no.

use std::collections::{BTreeMap, BTreeSet};

use draft_dcg_contract::*;

fn digest(bytes: &[u8]) -> Digest {
    Digest::of_bytes(bytes)
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
        content_digest: Some(digest(b"hello\n")),
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
        producer: ProducerIdentity::new(
            identifier::NamespacedId::parse("draft.core/filesystem").unwrap(),
            "0.3.4",
        )
        .unwrap(),
        execution: None,
        attempted_domains: BTreeSet::from([CoverageDomainRef::parse("root").unwrap()]),
        committed_domains: BTreeSet::from([CoverageDomainRef::parse("root").unwrap()]),
        observation_context: digest(b"context"),
        started_at: value::Timestamp::from_unix_nanos(1_700_000_000_000_000_000),
        completed_at: value::Timestamp::from_unix_nanos(1_700_000_001_000_000_000),
        terminal_status: ObservationTerminalStatus::Completed,
    }
}

fn observation() -> Observation {
    Observation {
        id: ids::ObservationId::parse("obs_000000000001").unwrap(),
        resource: ids::ResourceId::parse("res_000000000001").unwrap(),
        state: resource_state().digest().unwrap(),
        provider_binding: ids::ProviderBindingId::parse("pbd_000000000001").unwrap(),
        provider_semantic_definition: ProviderSemanticDefinitionDigest::new(digest(b"SD1")),
        stability: ObservationStability::Stable,
        observation_context: digest(b"context"),
        run: observation_run().reference().unwrap(),
        execution: None,
        observed_at: value::Timestamp::from_unix_nanos(1_700_000_000_500_000_000),
    }
}

fn state_root() -> ProjectStateRoot {
    let mut builder = ProjectStateRootBuilder::new();
    builder
        .insert_resource(
            ids::ResourceId::parse("res_000000000001").unwrap(),
            resource_state().digest().unwrap(),
        )
        .unwrap();
    builder.build().unwrap()
}

fn evidence_root() -> StateEvidenceRoot {
    let mut builder = StateEvidenceRootBuilder::new();
    builder
        .insert(BaselineStateEvidenceEntry::Resource {
            resource_id: ids::ResourceId::parse("res_000000000001").unwrap(),
            state: resource_state().digest().unwrap(),
            primary: observation().reference().unwrap(),
            corroborating: BTreeSet::new(),
        })
        .unwrap();
    builder.build().unwrap()
}

fn coverage_root() -> CoverageEvidenceRoot {
    let mut builder = CoverageEvidenceRootBuilder::new();
    builder
        .insert(CoverageEvidence {
            provider_binding: ids::ProviderBindingId::parse("pbd_000000000001").unwrap(),
            provider_semantic_definition: ProviderSemanticDefinitionDigest::new(digest(b"SD1")),
            domain: CoverageDomainRef::parse("root").unwrap(),
            status: CoverageStatus::Complete,
            observation_run: Some(observation_run().reference().unwrap()),
            attempted: true,
            committed: true,
            known_gaps: BTreeSet::new(),
        })
        .unwrap();
    builder.build().unwrap()
}

fn baseline_manifest() -> BaselineManifest {
    BaselineManifest {
        project: ids::ProjectId::parse("prj_000000000001").unwrap(),
        project_state_root: state_root(),
        state_evidence_root: evidence_root(),
        coverage_evidence_root: coverage_root(),
        parent_baseline_id: None,
        format_revision: DCG_FORMAT_REVISION,
    }
}

fn route() -> ProviderRouteRef {
    ProviderRouteRef {
        provenance: ProviderProvenanceRef {
            binding: ids::ProviderBindingId::parse("pbd_000000000001").unwrap(),
            semantic_definition: ProviderSemanticDefinitionDigest::new(digest(b"SD1")),
        },
        operational_profile: ProviderOperationalProfileDigest::new(digest(b"OP1")),
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

#[test]
fn the_format_revision_is_frozen() {
    assert_eq!(DCG_FORMAT_REVISION, 1);
}

#[test]
fn every_domain_separator_is_frozen() {
    // Changing one of these re-identifies every historical fact that used it.
    assert_eq!(
        semantics::SEMANTICS_CONTRACT_DIGEST_DOMAIN,
        "draft.dcg.resource-state-semantics-contract/v1"
    );
    assert_eq!(
        state::RESOURCE_STATE_DIGEST_DOMAIN,
        "draft.dcg.resource-state/v1"
    );
    assert_eq!(
        observation::OBSERVATION_DIGEST_DOMAIN,
        "draft.dcg.observation/v1"
    );
    assert_eq!(
        observation::OBSERVATION_RUN_DIGEST_DOMAIN,
        "draft.dcg.observation-run/v1"
    );
    assert_eq!(
        relation::RELATION_STATE_DIGEST_DOMAIN,
        "draft.dcg.relation-state/v1"
    );
    assert_eq!(
        relation::RELATION_RECORD_DIGEST_DOMAIN,
        "draft.dcg.relation-record/v1"
    );
    assert_eq!(
        relation::STATE_BEARING_DECLARATION_DIGEST_DOMAIN,
        "draft.dcg.state-bearing-declaration/v1"
    );
    assert_eq!(
        roots::PROJECT_STATE_ROOT_DOMAIN,
        "draft.dcg.project-state-root/v1"
    );
    assert_eq!(
        roots::STATE_EVIDENCE_ROOT_DOMAIN,
        "draft.dcg.state-evidence-root/v1"
    );
    assert_eq!(
        roots::COVERAGE_EVIDENCE_ROOT_DOMAIN,
        "draft.dcg.coverage-evidence-root/v1"
    );
    assert_eq!(
        baseline::BASELINE_MANIFEST_DIGEST_DOMAIN,
        "draft.dcg.baseline-manifest/v1"
    );
    assert_eq!(
        publication::PUBLICATION_REQUEST_KEY_DOMAIN,
        "draft.dcg.publication-request-key/v1"
    );
    assert_eq!(
        publication::PUBLICATION_IDEMPOTENCY_KEY_DOMAIN,
        "draft.dcg.publication-idempotency-key/v1"
    );
    assert_eq!(
        publication::PUBLICATION_DIGEST_DOMAIN,
        "draft.dcg.publication/v1"
    );
    assert_eq!(
        publication::PUBLICATION_ATTEMPT_DIGEST_DOMAIN,
        "draft.dcg.publication-attempt/v1"
    );
    assert_eq!(
        publication::PUBLICATION_OUTCOME_DIGEST_DOMAIN,
        "draft.dcg.publication-outcome/v1"
    );
    assert_eq!(
        publication::PUBLICATION_RESOLUTION_DIGEST_DOMAIN,
        "draft.dcg.publication-resolution/v1"
    );
    assert_eq!(
        publication::PUBLICATION_RETRY_AUTHORIZATION_DIGEST_DOMAIN,
        "draft.dcg.publication-retry-authorization/v1"
    );
    assert_eq!(RECEIPT_SIGNATURE_DOMAIN, "draft.dcg.receipt-signature/v1");
}

#[test]
fn the_merkle_chunk_size_is_frozen() {
    // Part of the root construction: changing it changes every root.
    assert_eq!(merkle::MERKLE_CHUNK_SIZE, 256);
}

#[test]
fn every_identifier_prefix_is_frozen() {
    assert_eq!(ids::ProjectId::PREFIX, "prj_");
    assert_eq!(ids::TaskId::PREFIX, "tsk_");
    assert_eq!(ids::ResourceId::PREFIX, "res_");
    assert_eq!(ids::ObservationId::PREFIX, "obs_");
    assert_eq!(ids::ObservationRunId::PREFIX, "run_");
    assert_eq!(ids::ChangePackId::PREFIX, "cpk_");
    assert_eq!(ids::RevisionPackId::PREFIX, "rpk_");
    assert_eq!(ids::OperationId::PREFIX, "op_");
    assert_eq!(ids::WorkspaceId::PREFIX, "wsp_");
    assert_eq!(ids::CheckpointId::PREFIX, "ckp_");
    assert_eq!(ids::EvidenceId::PREFIX, "evd_");
    assert_eq!(ids::AssessmentId::PREFIX, "asm_");
    assert_eq!(ids::ReviewId::PREFIX, "rvw_");
    assert_eq!(ids::DecisionId::PREFIX, "dec_");
    assert_eq!(ids::BaselineIdentifier::PREFIX, "bas_");
    assert_eq!(ids::AuthorityGrantId::PREFIX, "auth_");
    assert_eq!(ids::ReceiptId::PREFIX, "rcp_");
    assert_eq!(ids::ActivityEventId::PREFIX, "evt_");
    assert_eq!(ids::ActorId::PREFIX, "act_");
    assert_eq!(ids::ExecutionId::PREFIX, "exe_");
    assert_eq!(ids::RecoveryPlanId::PREFIX, "rcv_");
    assert_eq!(ids::ProviderBindingId::PREFIX, "pbd_");
    assert_eq!(ids::PromotionId::PREFIX, "pro_");
    assert_eq!(ids::PublicationId::PREFIX, "pub_");
    assert_eq!(ids::PublicationAttemptId::PREFIX, "pat_");
}

// ---------------------------------------------------------------------------
// Canonical encodings
// ---------------------------------------------------------------------------

#[test]
fn the_semantics_contract_canonical_form_is_frozen() {
    assert_eq!(
        canonical_bytes(&semantics_contract())
            .map(String::from_utf8)
            .unwrap()
            .unwrap(),
        concat!(
            r#"{"attribute_interpretation":{"executable":"state_bearing"},"#,
            r#""content_digest_interpretation":"required","id":"draft.filesystem/file.v1","#,
            r#""locator_state_role":"state_bearing","normalization":["none"],"#,
            r#""presence_absence_semantics":"meaningful_absence","#,
            r#""semantic_digest_interpretation":"absent"}"#
        )
    );
    assert_eq!(
        semantics_contract().digest().unwrap().to_string(),
        "sha256:835f8564f201ca18f988ea21e921c9c1c8bc4bf3381ae02e42618fd83dc497c5"
    );
}

#[test]
fn the_resource_state_canonical_form_and_digest_are_frozen() {
    assert_eq!(
        serde_json::to_string(&resource_state()).unwrap(),
        concat!(
            r#"{"resource_kind":"draft.filesystem/file","state_semantics":"#,
            r#"{"id":"draft.filesystem/file.v1","contract_digest":"#,
            r#""sha256:835f8564f201ca18f988ea21e921c9c1c8bc4bf3381ae02e42618fd83dc497c5"},"#,
            r#""locator":"app.txt","content_digest":"#,
            r#""sha256:5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03","#,
            r#""state_attributes":{"executable":false}}"#
        )
    );
    assert_eq!(
        resource_state().digest().unwrap().to_string(),
        "sha256:e20664b5ab18cbb6ce16af834e3a8c0c3a8b3d402a319e6f54f923bad193cd14"
    );
}

#[test]
fn the_observation_and_run_digests_are_frozen() {
    assert_eq!(
        observation_run().digest().unwrap().to_string(),
        "sha256:c2f1724e16d0bbc44ca63507e8258b6142f637a7b7bcf3dcd877b32b00e9d2fd"
    );
    assert_eq!(
        observation().digest().unwrap().to_string(),
        "sha256:565f287a70174da3e6d5f1dc251fbd423628b6530cc52f94dce304018985bb26"
    );
}

#[test]
fn the_three_roots_are_frozen() {
    assert_eq!(
        state_root().to_string(),
        "sha256:0254c214d3064e480ee0e89046a264f29fc2347430f2d404cfba83b70b6daa4c"
    );
    assert_eq!(
        evidence_root().to_string(),
        "sha256:aeedd3809f499d0a976ef79ae4b4dc1750cd109c9786af220ee4eb26237e197e"
    );
    assert_eq!(
        coverage_root().to_string(),
        "sha256:39bdcb4f85bbf101a47ab12864be5204dfc0658062b16c25d0a3b85d0167d58e"
    );
}

#[test]
fn the_empty_state_root_constant_is_frozen() {
    // A project with nothing accepted still has a defined, non-zero root.
    assert_eq!(
        ProjectStateRootBuilder::new().build().unwrap().to_string(),
        "sha256:f1381094e8840681e85a7ce283f4f5c618613c0f78ec8f5cb5d213a9586d04df"
    );
}

#[test]
fn the_baseline_manifest_and_id_are_frozen() {
    assert_eq!(
        serde_json::to_string(&baseline_manifest()).unwrap(),
        concat!(
            r#"{"project":"prj_000000000001","project_state_root":"#,
            r#""sha256:0254c214d3064e480ee0e89046a264f29fc2347430f2d404cfba83b70b6daa4c","#,
            r#""state_evidence_root":"#,
            r#""sha256:aeedd3809f499d0a976ef79ae4b4dc1750cd109c9786af220ee4eb26237e197e","#,
            r#""coverage_evidence_root":"#,
            r#""sha256:39bdcb4f85bbf101a47ab12864be5204dfc0658062b16c25d0a3b85d0167d58e","#,
            r#""format_revision":1}"#
        )
    );
    assert_eq!(
        baseline_manifest().baseline_id().unwrap().to_string(),
        "sha256:12042926772003b52f140395bb5625a85846f64192899d2bc84b9466b7c52bed"
    );
}

#[test]
fn the_publication_keys_are_frozen() {
    let baseline = baseline_manifest().baseline_id().unwrap();
    assert_eq!(
        Publication::compute_request_key(
            &ids::PromotionId::parse("pro_000000000001").unwrap(),
            &baseline,
            &route(),
            &PublicationPurposeId::parse("draft.publish/deploy").unwrap(),
            None,
        )
        .unwrap()
        .to_string(),
        "sha256:43099bc4fe5bf96c4be9f8d38183855f987df7a17edaca46f3f41fec446c0708"
    );
    assert_eq!(
        Publication::compute_idempotency_key(
            &ids::PublicationId::parse("pub_000000000001").unwrap(),
            &baseline,
            &route(),
        )
        .unwrap()
        .to_string(),
        "sha256:2cf76f15e3dca302a1caaf097f4e3e4c3716a812382474aa1543c3bdbd027750"
    );
}

#[test]
fn absent_optional_fields_are_omitted_rather_than_null() {
    // Two spellings of "absent" would give one logical value two digests.
    let encoded = serde_json::to_string(&resource_state()).unwrap();
    assert!(!encoded.contains("semantic_digest"), "{encoded}");
    let manifest = serde_json::to_string(&baseline_manifest()).unwrap();
    assert!(!manifest.contains("parent_baseline_id"), "{manifest}");
    assert!(!manifest.contains("null"), "{manifest}");
}
