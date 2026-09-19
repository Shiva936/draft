//! The portable record of one authorization decision.
//!
//! `core::authority` evaluates grants, revocations, scope and expiry; this
//! module owns only the immutable record of what that evaluation concluded, so
//! an independent verifier can read a historical fact and see *which* authority
//! was cited, *what* it was cited for, *who* decided and *when*.
//!
//! # What this record does not prove
//!
//! An `AuthorityDecision` is history, not permission. It says a decision was
//! reached against a named grant at a named moment. It does not assert that the
//! grant is still live now, and a later revocation neither erases it nor makes
//! it a licence for a new external effect. Current authority is always
//! re-evaluated at the commit boundary of the *next* operation.
//!
//! # Why the security context is not repeated here
//!
//! The containing fact — a `PublicationAttempt`, `PublicationResolution` or
//! `PublicationRetryAuthorization` — already carries the exact
//! `ProjectSecurityStateDigest`, `PolicyDigest` and observed registry revisions
//! that were in force. Copying them into the decision too would create two
//! canonical answers to one question, which is precisely how canonical meaning
//! drifts. The decision names the grant; the fact names the state.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::capability::CapabilityId;
use crate::identifier::ScopedId;
use crate::producer::ProducerIdentity;
use crate::security::SecurityFactRef;
use crate::value::Timestamp;
use crate::{FormatError, FormatResult};

/// Longest a decision rationale may be.
pub const MAX_REASON_LENGTH: usize = 1024;

/// Exactly what was being authorized.
///
/// A capability plus the subject it was claimed over. Deliberately narrow: a
/// scope that could describe anything would authorize anything.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityScopeClaim {
    /// The capability being exercised.
    pub capability: CapabilityId,
    /// The exact subject the capability was claimed over — the publication,
    /// change, relation or other object the decision is about.
    ///
    /// Scoped rather than free text so it is bounded and byte-compared.
    pub subject: ScopedId,
}

/// What the evaluation concluded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AuthorityDecisionOutcome {
    /// The claim was authorized.
    Permitted,
    /// The claim was refused, with the reason recorded.
    ///
    /// A refusal is kept as history rather than discarded: "we asked and were
    /// told no" is a materially different fact from "we never asked".
    Refused { reason: String },
}

/// The immutable record of one authorization decision.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityDecision {
    /// What was being authorized.
    pub scope: AuthorityScopeClaim,
    /// What was concluded.
    pub outcome: AuthorityDecisionOutcome,
    /// The exact immutable security facts the decision rested on.
    ///
    /// A set of exact refs, so a verifier can re-resolve every one and detect
    /// substitution. Never empty for a `Permitted` outcome: a permission that
    /// cites no authority is not a permission.
    pub considered: BTreeSet<SecurityFactRef>,
    /// Who performed the evaluation.
    pub evaluator: ProducerIdentity,
    /// When the decision was reached.
    pub decided_at: Timestamp,
}

impl AuthorityDecision {
    pub fn new(
        scope: AuthorityScopeClaim,
        outcome: AuthorityDecisionOutcome,
        considered: BTreeSet<SecurityFactRef>,
        evaluator: ProducerIdentity,
        decided_at: Timestamp,
    ) -> FormatResult<Self> {
        let decision = Self {
            scope,
            outcome,
            considered,
            evaluator,
            decided_at,
        };
        decision.validate()?;
        Ok(decision)
    }

    /// Structural validation of the decision on its own terms.
    pub fn validate(&self) -> FormatResult<()> {
        if matches!(self.outcome, AuthorityDecisionOutcome::Permitted) && self.considered.is_empty()
        {
            return Err(FormatError::Consistency(
                "a permitted authority decision must cite at least one security fact".into(),
            ));
        }
        if let AuthorityDecisionOutcome::Refused { reason } = &self.outcome {
            if reason.trim().is_empty() {
                return Err(FormatError::Consistency(
                    "a refused authority decision must record why".into(),
                ));
            }
            if reason.len() > MAX_REASON_LENGTH {
                return Err(FormatError::Consistency(format!(
                    "authority refusal reason exceeds {MAX_REASON_LENGTH} bytes"
                )));
            }
        }
        if !self.scope.capability.is_acceptable() {
            return Err(FormatError::Identity(format!(
                "authority decision cites unrecognised reserved capability '{}'",
                self.scope.capability
            )));
        }
        Ok(())
    }

    /// Whether this decision permitted the claim.
    pub fn is_permitted(&self) -> bool {
        matches!(self.outcome, AuthorityDecisionOutcome::Permitted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::Digest;
    use crate::identifier::NamespacedId;
    use crate::security::SecurityControlKindId;

    fn grant() -> SecurityFactRef {
        SecurityFactRef::new(
            SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap(),
            Some(ScopedId::parse("auth_1").unwrap()),
            Digest::of_bytes(b"grant"),
        )
    }

    fn scope() -> AuthorityScopeClaim {
        AuthorityScopeClaim {
            capability: CapabilityId::parse("draft.publish/v1").unwrap(),
            subject: ScopedId::parse("pub_a1b2c3").unwrap(),
        }
    }

    fn evaluator() -> ProducerIdentity {
        ProducerIdentity::new(
            NamespacedId::parse("draft.core/authority").unwrap(),
            "0.3.4",
        )
        .unwrap()
    }

    fn permitted() -> AuthorityDecision {
        AuthorityDecision::new(
            scope(),
            AuthorityDecisionOutcome::Permitted,
            BTreeSet::from([grant()]),
            evaluator(),
            Timestamp::from_unix_nanos(1_700_000_000_000_000_000),
        )
        .unwrap()
    }

    #[test]
    fn a_permission_must_cite_the_authority_it_rests_on() {
        assert!(permitted().is_permitted());
        let uncited = AuthorityDecision::new(
            scope(),
            AuthorityDecisionOutcome::Permitted,
            BTreeSet::new(),
            evaluator(),
            Timestamp::from_unix_nanos(1),
        );
        assert!(matches!(uncited, Err(FormatError::Consistency(_))));
    }

    #[test]
    fn a_refusal_is_kept_as_history_and_must_say_why() {
        let refused = AuthorityDecision::new(
            scope(),
            AuthorityDecisionOutcome::Refused {
                reason: "grant does not cover this publication".into(),
            },
            BTreeSet::new(),
            evaluator(),
            Timestamp::from_unix_nanos(1),
        )
        .unwrap();
        assert!(!refused.is_permitted());

        // A refusal with no reason records nothing useful.
        assert!(AuthorityDecision::new(
            scope(),
            AuthorityDecisionOutcome::Refused {
                reason: "   ".into()
            },
            BTreeSet::new(),
            evaluator(),
            Timestamp::from_unix_nanos(1),
        )
        .is_err());
    }

    #[test]
    fn an_unrecognised_reserved_capability_cannot_be_authorized() {
        let mut claim = scope();
        claim.capability = CapabilityId::parse("draft.publish/v2").unwrap();
        assert!(AuthorityDecision::new(
            claim,
            AuthorityDecisionOutcome::Permitted,
            BTreeSet::from([grant()]),
            evaluator(),
            Timestamp::from_unix_nanos(1),
        )
        .is_err());
    }

    #[test]
    fn substituting_a_cited_grant_changes_the_decision() {
        let decision = permitted();
        let mut tampered = decision.clone();
        tampered.considered = BTreeSet::from([SecurityFactRef::new(
            SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap(),
            Some(ScopedId::parse("auth_1").unwrap()),
            Digest::of_bytes(b"grant-widened"),
        )]);
        assert_ne!(decision, tampered);
    }

    #[test]
    fn the_wire_form_round_trips_and_rejects_unknown_fields() {
        let decision = permitted();
        let encoded = serde_json::to_string(&decision).unwrap();
        assert_eq!(
            serde_json::from_str::<AuthorityDecision>(&encoded).unwrap(),
            decision
        );
        let widened = encoded.replace("{\"scope\"", "{\"escalate\":true,\"scope\"");
        assert!(serde_json::from_str::<AuthorityDecision>(&widened).is_err());
    }
}
