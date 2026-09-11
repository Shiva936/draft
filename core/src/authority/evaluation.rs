//! Deciding whether a claim may proceed, and recording why.
//!
//! Evaluation reads grants and revocations and produces an
//! [`AuthorityDecision`] — a record of what was concluded against which exact
//! facts. It never mutates anything and never caches: the answer is only true
//! for the instant it was computed, under the locks the caller held.

use draft_dcg_contract::authority::{
    AuthorityDecision, AuthorityDecisionOutcome, AuthorityScopeClaim,
};
use draft_dcg_contract::capability::CapabilityId;
use draft_dcg_contract::identifier::ScopedId;
use draft_dcg_contract::ids::ActorId;
use draft_dcg_contract::producer::ProducerIdentity;
use draft_dcg_contract::value::Timestamp;
use std::collections::BTreeSet;

use crate::authority::grant::AuthorityGrant;
use crate::authority::revocation::AuthorityRevocation;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

fn contract_error(error: draft_dcg_contract::FormatError) -> DraftError {
    DraftError::new(DraftErrorKind::Validation, error.to_string())
}

/// What is being claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityClaim {
    pub actor: ActorId,
    pub capability: CapabilityId,
    pub subject: ScopedId,
}

impl AuthorityClaim {
    /// The claim to publish a Baseline to an external target.
    ///
    /// # Why publication is its own capability class
    ///
    /// Publishing is the one capability whose exercise is visible outside
    /// Draft and cannot be undone by Draft. Promotion moves accepted state
    /// inside a project and is recoverable; publication tells the world.
    ///
    /// So no other grant satisfies it — not a broader-sounding one, and
    /// specifically not the promotion authority that necessarily precedes it.
    /// Someone permitted to accept work into a Baseline has not thereby been
    /// permitted to announce it, and collapsing the two would make every
    /// approver an unwitting publisher.
    pub fn publish(actor: ActorId, subject: ScopedId) -> DraftResult<Self> {
        Ok(Self {
            actor,
            capability: CapabilityId::parse("draft.publish/v1").map_err(contract_error)?,
            subject,
        })
    }

    /// Whether this claim is the publication class.
    pub fn is_publication(&self) -> bool {
        self.capability.as_namespaced().qualified() == "draft.publish/v1"
    }
}

/// The facts the decision is made against.
///
/// Passed in rather than fetched, so evaluation cannot silently read a
/// different set than the caller validated under its locks.
#[derive(Debug, Clone, Default)]
pub struct AuthorityInputs {
    pub grants: Vec<AuthorityGrant>,
    pub revocations: Vec<AuthorityRevocation>,
}

/// Decide a claim.
///
/// A grant permits the claim only if all of these hold: it is held by this
/// actor, covers exactly this capability and subject, has not expired, and has
/// not been revoked. Each is checked separately so the refusal says which one
/// failed — "not authorized" without a reason is not something anyone can act
/// on.
///
/// `considered` carries every grant the evaluation actually rested on. For a
/// permitted outcome that set is never empty, because a permission citing no
/// authority is not a permission; for a refusal it carries the grants that were
/// examined and rejected, which is what makes the refusal reviewable.
pub fn evaluate(
    claim: &AuthorityClaim,
    inputs: &AuthorityInputs,
    now: Timestamp,
    evaluator: ProducerIdentity,
) -> DraftResult<AuthorityDecision> {
    let revoked: BTreeSet<&str> = inputs
        .revocations
        .iter()
        .map(|revocation| revocation.grant_id.as_str())
        .collect();

    let mut considered = BTreeSet::new();
    let mut refusal: Option<String> = None;

    for grant in &inputs.grants {
        if grant.grantee != claim.actor || !grant.covers(&claim.capability, &claim.subject) {
            // Not about this claim at all. Not a refusal reason, and not
            // something the decision should cite as having been weighed.
            continue;
        }
        considered.insert(grant.reference()?);

        if revoked.contains(grant.id.as_str()) {
            refusal.get_or_insert_with(|| format!("grant {} was revoked", grant.id.as_str()));
            continue;
        }
        if grant.is_expired_at(now) {
            refusal.get_or_insert_with(|| format!("grant {} has expired", grant.id.as_str()));
            continue;
        }

        return AuthorityDecision::new(
            AuthorityScopeClaim {
                capability: claim.capability.clone(),
                subject: claim.subject.clone(),
            },
            AuthorityDecisionOutcome::Permitted,
            considered,
            evaluator,
            now,
        )
        .map_err(contract_error);
    }

    let reason = refusal.unwrap_or_else(|| {
        format!(
            "no grant gives {} '{}' over {}",
            claim.actor.as_str(),
            claim.capability.as_namespaced(),
            claim.subject.as_str()
        )
    });

    AuthorityDecision::new(
        AuthorityScopeClaim {
            capability: claim.capability.clone(),
            subject: claim.subject.clone(),
        },
        AuthorityDecisionOutcome::Refused { reason },
        considered,
        evaluator,
        now,
    )
    .map_err(contract_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::grant::AuthorityGrant;
    use crate::authority::revocation::AuthorityRevocation;
    use draft_dcg_contract::ids::AuthorityGrantId;

    fn actor(id: &str) -> ActorId {
        ActorId::parse(id).unwrap()
    }

    fn capability() -> CapabilityId {
        CapabilityId::parse("draft.publish/v1").unwrap()
    }

    fn subject() -> ScopedId {
        ScopedId::parse("bas_000000000001").unwrap()
    }

    fn at(nanos: i64) -> Timestamp {
        Timestamp::from_unix_nanos(nanos)
    }

    fn evaluator() -> ProducerIdentity {
        ProducerIdentity::new(
            draft_dcg_contract::identifier::NamespacedId::parse("draft.core/authority").unwrap(),
            "1",
        )
        .unwrap()
    }

    fn grant(id: &str, expires_at: Option<Timestamp>) -> AuthorityGrant {
        AuthorityGrant {
            id: AuthorityGrantId::parse(id).unwrap(),
            grantee: actor("act_000000000001"),
            capability: capability(),
            subject: subject(),
            granted_by: actor("act_000000000002"),
            granted_at: at(0),
            expires_at,
        }
    }

    fn claim() -> AuthorityClaim {
        AuthorityClaim {
            actor: actor("act_000000000001"),
            capability: capability(),
            subject: subject(),
        }
    }

    #[test]
    fn a_live_grant_permits_and_the_decision_cites_it() {
        let inputs = AuthorityInputs {
            grants: vec![grant("auth_000000000001", None)],
            revocations: vec![],
        };
        let decision = evaluate(&claim(), &inputs, at(10), evaluator()).unwrap();

        assert_eq!(decision.outcome, AuthorityDecisionOutcome::Permitted);
        // A permission that cites no authority is not a permission.
        assert_eq!(decision.considered.len(), 1);
    }

    #[test]
    fn a_revoked_grant_refuses_and_says_so() {
        let revoked = grant("auth_000000000001", None);
        let inputs = AuthorityInputs {
            revocations: vec![AuthorityRevocation {
                grant: revoked.reference().unwrap(),
                grant_id: revoked.id.clone(),
                revoked_by: actor("act_000000000002"),
                revoked_at: at(5),
                reason: "left the project".into(),
            }],
            grants: vec![revoked],
        };
        let decision = evaluate(&claim(), &inputs, at(10), evaluator()).unwrap();

        match &decision.outcome {
            AuthorityDecisionOutcome::Refused { reason } => {
                assert!(reason.contains("revoked"), "{reason}")
            }
            other => panic!("a revoked grant must not permit: {other:?}"),
        }
        // The refusal still cites what it weighed, so it can be reviewed.
        assert_eq!(decision.considered.len(), 1);
    }

    #[test]
    fn an_expired_grant_refuses_even_though_it_was_never_revoked() {
        // Expiry and revocation both stop a grant authorizing, and the
        // difference matters: only one of them is a decision somebody made.
        let inputs = AuthorityInputs {
            grants: vec![grant("auth_000000000001", Some(at(5)))],
            revocations: vec![],
        };
        let decision = evaluate(&claim(), &inputs, at(10), evaluator()).unwrap();

        match &decision.outcome {
            AuthorityDecisionOutcome::Refused { reason } => {
                assert!(reason.contains("expired"), "{reason}")
            }
            other => panic!("an expired grant must not permit: {other:?}"),
        }
    }

    #[test]
    fn a_grant_over_a_different_subject_is_not_a_narrower_grant() {
        // The tempting reading is that a grant over *something* is a partial
        // permission. It is not: it is a permission over something else, and
        // it must not be cited as though it were weighed for this claim.
        let mut elsewhere = grant("auth_000000000001", None);
        elsewhere.subject = ScopedId::parse("bas_999999999999").unwrap();
        let inputs = AuthorityInputs {
            grants: vec![elsewhere],
            revocations: vec![],
        };
        let decision = evaluate(&claim(), &inputs, at(10), evaluator()).unwrap();

        assert!(matches!(
            decision.outcome,
            AuthorityDecisionOutcome::Refused { .. }
        ));
        assert!(
            decision.considered.is_empty(),
            "a grant over another subject was never weighed for this claim"
        );
    }

    #[test]
    fn no_other_capability_authorizes_publication() {
        // Publication is the one capability whose exercise leaves Draft and
        // cannot be taken back. A grant of anything else — however broad it
        // sounds — must not satisfy it.
        let publish = AuthorityClaim::publish(actor("act_000000000001"), subject()).unwrap();
        assert!(publish.is_publication());

        for other in [
            "draft.change.operate/v1",
            "draft.merge/v1",
            "draft.lock/v1",
            "draft.recover/v1",
        ] {
            let mut held = grant("auth_000000000001", None);
            held.capability = CapabilityId::parse(other).unwrap();
            let inputs = AuthorityInputs {
                grants: vec![held],
                revocations: vec![],
            };
            let decision = evaluate(&publish, &inputs, at(10), evaluator()).unwrap();
            assert!(
                matches!(decision.outcome, AuthorityDecisionOutcome::Refused { .. }),
                "'{other}' must not authorize publication"
            );
        }
    }

    #[test]
    fn authority_to_promote_is_not_authority_to_publish() {
        // The mistake worth naming: promotion necessarily precedes publication,
        // so it is tempting to treat the approver as already permitted. That
        // would make everyone who accepts work into a Baseline an unwitting
        // publisher of it.
        let mut promote = grant("auth_000000000001", None);
        promote.capability = CapabilityId::parse("draft.change.operate/v1").unwrap();
        let inputs = AuthorityInputs {
            grants: vec![promote],
            revocations: vec![],
        };

        let publish = AuthorityClaim::publish(actor("act_000000000001"), subject()).unwrap();
        let decision = evaluate(&publish, &inputs, at(10), evaluator()).unwrap();
        assert!(matches!(
            decision.outcome,
            AuthorityDecisionOutcome::Refused { .. }
        ));
        assert!(
            decision.considered.is_empty(),
            "a promotion grant was never weighed for a publication claim"
        );
    }

    #[test]
    fn a_publication_grant_authorizes_only_its_own_target() {
        // Publishing to one target is not permission to publish to another:
        // the subject is part of what was granted.
        let inputs = AuthorityInputs {
            grants: vec![grant("auth_000000000001", None)],
            revocations: vec![],
        };
        let elsewhere = AuthorityClaim::publish(
            actor("act_000000000001"),
            ScopedId::parse("bas_999999999999").unwrap(),
        )
        .unwrap();
        let decision = evaluate(&elsewhere, &inputs, at(10), evaluator()).unwrap();
        assert!(matches!(
            decision.outcome,
            AuthorityDecisionOutcome::Refused { .. }
        ));
    }

    #[test]
    fn a_live_grant_still_permits_when_a_different_grant_was_revoked() {
        // A revocation withdraws the grant it names and nothing else.
        let revoked = grant("auth_000000000001", None);
        let mut live = grant("auth_000000000002", None);
        live.granted_at = at(1);
        let inputs = AuthorityInputs {
            revocations: vec![AuthorityRevocation {
                grant: revoked.reference().unwrap(),
                grant_id: revoked.id.clone(),
                revoked_by: actor("act_000000000002"),
                revoked_at: at(5),
                reason: "superseded".into(),
            }],
            grants: vec![revoked, live],
        };
        let decision = evaluate(&claim(), &inputs, at(10), evaluator()).unwrap();
        assert_eq!(decision.outcome, AuthorityDecisionOutcome::Permitted);
    }
}
