//! §2.46 — the checks a digest cannot make.
//!
//! A digest proves nobody edited the bytes since they were written. It proves
//! nothing about whether they were coherent *when* written, and nothing about
//! whether the object was filed under the right key. An object whose derived
//! fields disagree with its own canonical inputs has a perfectly valid digest
//! over its self-consistently corrupted bytes.
//!
//! # The split, and why it is where it is
//!
//! The SDK validates what one object can check about itself: that a
//! `Publication`'s request and idempotency keys recompute from its own inputs,
//! that an attempt's route matches its Publication's, that an outcome carries
//! the receipt and signer it was filed against.
//!
//! What the SDK cannot check is anything requiring a second durable record.
//! Whether an attempt's `attempt_number` is the number the control record
//! actually reserved for it, and whether its dispatch-time snapshots are the
//! ones the journal durably recorded, are questions only Core can ask —
//! Core is what holds those records.
//!
//! That is the whole of this module: the cross-record half.
//!
//! # Why "the attempt says so" is not enough
//!
//! Every field checked here is one an attempt asserts about itself. An attempt
//! claiming an attempt number nobody reserved, or a security snapshot
//! different from the one the dispatch was durably authorized under, is an
//! external effect attributed to authority that never permitted it. The
//! attempt's own digest would verify perfectly.

use draft_dcg_contract::publication::{Publication, PublicationAttempt, PublicationAttemptRef};

use crate::publication::journal::AttemptJournalState;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// Verify an attempt against everything outside itself.
///
/// `publication` is the exact Publication the attempt claims, and `journal` is
/// the attempt's authoritative journal state, read under its guard.
///
/// Runs the SDK's intra-object checks first, then the cross-record ones the
/// SDK has no access to. Order matters only for the error a caller sees: a
/// structurally invalid attempt is not worth comparing against anything.
pub fn verify_dispatched_attempt(
    attempt: &PublicationAttempt,
    reference: &PublicationAttemptRef,
    publication: &Publication,
    journal: &AttemptJournalState,
) -> DraftResult<()> {
    let format = |error: draft_dcg_contract::FormatError| {
        DraftError::new(DraftErrorKind::CorruptData, error.to_string())
    };
    // The exact-reference boundary: the attempt's bytes must still hash to
    // the digest the reference names.
    attempt.verify_reference(reference).map_err(|error| {
        crate::support::telemetry::Counter::PublicationAttemptDigestMismatches.increment();
        format(error)
    })?;
    // §2.46's cross-field checks. A digest proves the bytes are unchanged; it
    // proves nothing about whether the derived fields inside them agree with
    // their own canonical inputs.
    attempt.validate_against(publication).map_err(|error| {
        crate::support::telemetry::Counter::PublicationSelfConsistencyRejections.increment();
        format(error)
    })?;

    // The number must be the one the allocation actually reserved. An attempt
    // asserting a number nobody reserved would claim a slot in the sequence
    // that was never allocated to it.
    let reserved = journal.attempt_number().ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!(
                "attempt '{}' has no journal state carrying the number it was allocated",
                attempt.id
            ),
        )
    })?;
    if attempt.attempt_number != reserved {
        return Err(DraftError::new(
            DraftErrorKind::CorruptData,
            format!(
                "attempt '{}' claims number {} but {reserved} was reserved for it",
                attempt.id, attempt.attempt_number
            ),
        ));
    }

    // The journal must agree that this exact attempt reached the durable
    // dispatch boundary. A `Dispatching` state naming different bytes would
    // mean the attempt was replaced after it was authorized.
    match journal {
        AttemptJournalState::Dispatching {
            attempt: dispatched,
            ..
        }
        | AttemptJournalState::OutcomePrepared {
            attempt: dispatched,
            ..
        }
        | AttemptJournalState::OutcomeRecorded {
            attempt: dispatched,
            ..
        } => {
            if dispatched != reference {
                return Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "attempt '{}' does not match the exact reference its journal recorded at \
                         the dispatch boundary",
                        attempt.id
                    ),
                ));
            }
        }
        other => {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "attempt '{}' has a dispatched attempt object but its journal is {}, which no \
                     dispatch reaches",
                    attempt.id,
                    other.name()
                ),
            ))
        }
    }

    Ok(())
}

/// Verify that a retry authorization belongs to the exact Publication it is
/// being spent against.
///
/// A `PublicationRetryAuthorization` binds a `PublicationRef` — id **and**
/// digest — rather than a bare id. That is what stops the bytes under `pub_A`
/// changing beneath an existing authorization and quietly widening it to a
/// different route, baseline or purpose.
pub fn retry_authorization_targets(
    authorization_publication: &draft_dcg_contract::publication::PublicationRef,
    publication: &Publication,
) -> DraftResult<()> {
    publication
        .verify_reference(authorization_publication)
        .map_err(|error| {
            // Either the authorization names a different Publication, or the
            // bytes beneath this one have moved. Both are the same failure at
            // this boundary: the exact reference no longer resolves.
            crate::support::telemetry::Counter::PublicationDigestMismatches.increment();
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "the retry authorization is bound to a Publication that is not this one: \
                     {error}"
                ),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication::control::PublicationControl;
    use crate::publication::journal::{ControlTransition, NonCommitEvidence};
    use draft_dcg_contract::authority::{
        AuthorityDecision, AuthorityDecisionOutcome, AuthorityScopeClaim,
    };
    use draft_dcg_contract::baseline::BaselineId;
    use draft_dcg_contract::capability::CapabilityId;
    use draft_dcg_contract::identifier::NamespacedId;
    use draft_dcg_contract::identifier::ScopedId;
    use draft_dcg_contract::ids::{
        ActivityEventId, ActorId, PromotionId, PublicationAttemptId, PublicationId,
    };
    use draft_dcg_contract::producer::ProducerIdentity;
    use draft_dcg_contract::provider::{
        ProviderOperationalProfileDigest, ProviderProvenanceRef, ProviderRouteRef,
    };
    use draft_dcg_contract::publication::{
        DeliverySemantics, PublicationAttemptDigest, PublicationPurposeId,
    };
    use draft_dcg_contract::security::{PolicyDigest, ProjectSecurityStateDigest};
    use draft_dcg_contract::security::{SecurityControlKindId, SecurityFactRef};
    use draft_dcg_contract::value::{
        LeaseFence, LeaseId, ProjectControlGeneration, ProviderBindingGeneration,
        RegistryRevisions, Timestamp,
    };
    use draft_dcg_contract::Digest;

    fn route() -> ProviderRouteRef {
        ProviderRouteRef {
            provenance: ProviderProvenanceRef {
                binding: draft_dcg_contract::ids::ProviderBindingId::parse("pbd_000000000001")
                    .unwrap(),
                semantic_definition: draft_dcg_contract::ProviderSemanticDefinitionDigest::new(
                    Digest::of_bytes(b"semantics"),
                ),
            },
            operational_profile: ProviderOperationalProfileDigest::new(Digest::of_bytes(
                b"profile",
            )),
        }
    }

    fn publication() -> Publication {
        let id = PublicationId::parse("pub_000000000001").unwrap();
        let promotion = PromotionId::parse("pro_000000000001").unwrap();
        let baseline = BaselineId::new(Digest::of_bytes(b"baseline"));
        let purpose = PublicationPurposeId::parse("draft.core/deploy").unwrap();
        Publication {
            request_key: Publication::compute_request_key(
                &promotion,
                &baseline,
                &route(),
                &purpose,
                None,
            )
            .unwrap(),
            idempotency_key: Publication::compute_idempotency_key(&id, &baseline, &route())
                .unwrap(),
            id,
            promotion,
            baseline,
            route: route(),
            purpose,
            republish_intent: None,
            requested_by: ActorId::parse("act_000000000001").unwrap(),
            authority_inputs: Default::default(),
            credential_authority_class: None,
            delivery_semantics: DeliverySemantics::IdempotentByKey,
            created_at: Timestamp::from_unix_nanos(0),
        }
    }

    fn attempt(number: u32) -> PublicationAttempt {
        PublicationAttempt {
            id: PublicationAttemptId::parse("pat_000000000001").unwrap(),
            publication: publication().reference().unwrap(),
            attempt_number: number,
            route: route(),
            attempt_authority: Default::default(),
            authority_decision: permitted_decision(),
            project_control_generation_at_dispatch: ProjectControlGeneration::new(1),
            project_security_state_at_dispatch: ProjectSecurityStateDigest::new(Digest::of_bytes(
                b"security",
            )),
            policy_digest_at_dispatch: PolicyDigest::new(Digest::of_bytes(b"policy")),
            global_registry_revisions_at_dispatch: RegistryRevisions::new(),
            provider_binding_generation_at_dispatch: ProviderBindingGeneration::new(1),
            retry_authorization: None,
            lease_id: LeaseId::parse("lse_000000000001").unwrap(),
            lease_fence: LeaseFence::new(1),
            started_at: Timestamp::from_unix_nanos(0),
            provenance: ProducerIdentity::new(
                NamespacedId::parse("draft.core/publication").unwrap(),
                "1",
            )
            .unwrap(),
        }
    }

    fn permitted_decision() -> AuthorityDecision {
        AuthorityDecision::new(
            AuthorityScopeClaim {
                capability: CapabilityId::parse("draft.publish/v1").unwrap(),
                subject: ScopedId::parse("pub_000000000001").unwrap(),
            },
            AuthorityDecisionOutcome::Permitted,
            [SecurityFactRef::new(
                SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap(),
                Some(ScopedId::parse("auth_000000000001").unwrap()),
                Digest::of_bytes(b"grant"),
            )]
            .into_iter()
            .collect(),
            ProducerIdentity::new(NamespacedId::parse("draft.core/publication").unwrap(), "1")
                .unwrap(),
            Timestamp::from_unix_nanos(0),
        )
        .unwrap()
    }

    fn control(generation: u64) -> PublicationControl {
        let mut value =
            PublicationControl::initial(PublicationId::parse("pub_000000000001").unwrap());
        value.generation = generation;
        value
    }

    fn dispatching(reference: &PublicationAttemptRef) -> AttemptJournalState {
        AttemptJournalState::Dispatching {
            attempt: reference.clone(),
            attempt_number: 1,
            dispatch_event: ActivityEventId::parse("evt_000000000001").unwrap(),
        }
    }

    #[test]
    fn a_dispatched_attempt_that_agrees_with_every_record_verifies() {
        let attempt = attempt(1);
        let reference = attempt.reference().unwrap();
        verify_dispatched_attempt(
            &attempt,
            &reference,
            &publication(),
            &dispatching(&reference),
        )
        .unwrap();
    }

    #[test]
    fn an_attempt_claiming_a_number_nobody_reserved_is_refused() {
        // Its own digest verifies. Only the second record catches this.
        let attempt = attempt(7);
        let reference = attempt.reference().unwrap();
        let journal = AttemptJournalState::AttemptPrepared {
            candidate_attempt_number: 1,
            allocation: ControlTransition {
                expected: control(0),
                planned: control(1),
            },
            retry_authorization: None,
        };
        let error =
            verify_dispatched_attempt(&attempt, &reference, &publication(), &journal).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
        assert!(error.message.contains("but 1 was reserved"));
    }

    #[test]
    fn an_attempt_replaced_after_authorization_does_not_match_its_journal() {
        // Both objects are internally valid and both digests verify. What is
        // wrong is that the journal recorded a different one at the dispatch
        // boundary.
        let dispatched = attempt(1);
        let reference = dispatched.reference().unwrap();
        let substituted = PublicationAttemptRef {
            id: dispatched.id.clone(),
            digest: PublicationAttemptDigest::new(Digest::of_bytes(b"different-bytes")),
        };
        let error = verify_dispatched_attempt(
            &dispatched,
            &reference,
            &publication(),
            &dispatching(&substituted),
        )
        .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn a_dispatched_attempt_object_under_a_never_dispatched_journal_is_refused() {
        let attempt = attempt(1);
        let reference = attempt.reference().unwrap();
        let journal = AttemptJournalState::Abandoned {
            evidence: NonCommitEvidence {
                candidate_attempt_number: 1,
                observed_control: control(0),
                classified_at: Timestamp::from_unix_nanos(0),
            },
        };
        let error =
            verify_dispatched_attempt(&attempt, &reference, &publication(), &journal).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn an_attempt_whose_reference_names_other_bytes_is_refused_before_anything_else() {
        let attempt = attempt(1);
        let wrong = PublicationAttemptRef {
            id: attempt.id.clone(),
            digest: PublicationAttemptDigest::new(Digest::of_bytes(b"not-this-attempt")),
        };
        let error =
            verify_dispatched_attempt(&attempt, &wrong, &publication(), &dispatching(&wrong))
                .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn a_retry_authorization_bound_to_other_bytes_under_the_same_id_is_refused() {
        // The reason the binding is a PublicationRef and not a bare id:
        // changing the bytes under pub_A must not widen an authorization.
        let publication = publication();
        let exact = publication.reference().unwrap();
        retry_authorization_targets(&exact, &publication).unwrap();

        let widened = draft_dcg_contract::publication::PublicationRef {
            id: publication.id.clone(),
            digest: draft_dcg_contract::publication::PublicationDigest::new(Digest::of_bytes(
                b"a-different-publication",
            )),
        };
        let error = retry_authorization_targets(&widened, &publication).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }
}
