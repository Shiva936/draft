//! The context an evaluation was made in, and the security it rested on.
//!
//! # Why a historical fact records its context
//!
//! An evaluation result is only meaningful alongside what it was evaluated
//! against. "The gate was satisfied" is unanswerable a month later unless the
//! fact also says which policy, which security state, and which evaluator
//! reached that conclusion — otherwise re-running it and getting a different
//! answer is indistinguishable from the original being wrong.
//!
//! # Why the two digests are never conflated
//!
//! `ProjectSecurityStateDigest` is the *set of references* a project holds.
//! [`SecurityContextDigest`] is those references **plus the resolved facts they
//! name**. They move independently: revoking a grant changes the first, and so
//! does adding one — but the second also changes when a referenced fact's
//! content is substituted while the reference set stands still.
//!
//! Treating them as one value would make exactly that substitution invisible,
//! which is the attack the exact-reference discipline exists to catch.

use draft_dcg_contract::identifier::NamespacedId;
use draft_dcg_contract::producer::ProducerIdentity;
use draft_dcg_contract::security::{PolicyDigest, SecurityContextDigest, SecurityFactRef};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;

/// Everything security-relevant an evaluation depended on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityDependencySet {
    pub grants: BTreeSet<SecurityFactRef>,
    pub authorizations: BTreeSet<SecurityFactRef>,
    pub trust_decisions: BTreeSet<SecurityFactRef>,
    /// The packages that produced the inputs.
    pub producer_packages: BTreeSet<ProducerIdentity>,
    /// The packages that performed the evaluation.
    ///
    /// Separate from producers because they are different trust questions: one
    /// is who made the thing being judged, the other is who judged it.
    pub evaluator_packages: BTreeSet<ProducerIdentity>,
    /// The global trust registry revisions observed.
    pub global_registry_revisions: BTreeSet<String>,
    pub credential_authority_classes: BTreeSet<String>,
}

/// The dependency set together with the facts it resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityContextSnapshot {
    pub dependencies: SecurityDependencySet,
    /// The canonical digest of each resolved fact, keyed by its reference.
    ///
    /// Carrying the resolved content — not just the references — is what makes
    /// this digest move when a fact is substituted behind a stable reference.
    pub resolved: BTreeSet<(SecurityFactRef, Digest)>,
}

impl SecurityContextSnapshot {
    pub fn digest(&self) -> DraftResult<SecurityContextDigest> {
        let digest = Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;
        Ok(SecurityContextDigest::new(digest))
    }
}

/// When and under what an evaluation was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationContext {
    pub evaluated_at: Timestamp,
    /// Which clock produced `evaluated_at`.
    ///
    /// A conclusion reached against a fixed test clock is a different claim
    /// from one reached against real time, and a verifier that could not tell
    /// them apart would read a fixture as evidence about the world.
    pub clock_source: NamespacedId,
    pub policy_digest: PolicyDigest,
    pub security_context_digest: SecurityContextDigest,
    /// The Core evaluator that reached the conclusion.
    pub core_evaluator_revision: String,
}

impl EvaluationContext {
    /// Whether this context still describes the current policy and security.
    ///
    /// What a commit boundary asks before acting on an earlier evaluation. A
    /// difference in either digest means the conclusion was reached under rules
    /// or authority that have since moved, so it must be re-evaluated rather
    /// than trusted.
    pub fn still_current(&self, policy: &PolicyDigest, security: &SecurityContextDigest) -> bool {
        &self.policy_digest == policy && &self.security_context_digest == security
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(seed: &[u8]) -> SecurityFactRef {
        SecurityFactRef::new(
            draft_dcg_contract::security::SecurityControlKindId::parse(
                "draft.security/authority-grant.v1",
            )
            .unwrap(),
            Some(draft_dcg_contract::identifier::ScopedId::parse("auth_000000000001").unwrap()),
            Digest::of_bytes(seed),
        )
    }

    fn snapshot(resolved: &[u8]) -> SecurityContextSnapshot {
        SecurityContextSnapshot {
            dependencies: SecurityDependencySet {
                grants: [reference(b"grant")].into_iter().collect(),
                ..Default::default()
            },
            resolved: [(reference(b"grant"), Digest::of_bytes(resolved))]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn substituting_a_resolved_fact_moves_the_context_digest() {
        // The whole reason the context carries resolved content: the reference
        // set is identical in both, so a digest over references alone could not
        // tell these apart — and the substitution would be invisible.
        let original = snapshot(b"the-grant-as-issued");
        let substituted = snapshot(b"a-different-grant-under-the-same-id");

        assert_eq!(
            original.dependencies, substituted.dependencies,
            "the dependency sets are identical"
        );
        assert_ne!(
            original.digest().unwrap(),
            substituted.digest().unwrap(),
            "but the resolved content differs, so the context digest must move"
        );
    }

    #[test]
    fn a_context_is_stale_once_policy_or_security_moves() {
        let context = EvaluationContext {
            evaluated_at: Timestamp::from_unix_nanos(0),
            clock_source: NamespacedId::parse("draft.core/fixed-clock").unwrap(),
            policy_digest: PolicyDigest::new(Digest::of_bytes(b"policy")),
            security_context_digest: snapshot(b"grant").digest().unwrap(),
            core_evaluator_revision: "1".into(),
        };

        assert!(context.still_current(
            &PolicyDigest::new(Digest::of_bytes(b"policy")),
            &snapshot(b"grant").digest().unwrap()
        ));

        // Either half moving is enough to make the conclusion stale.
        assert!(!context.still_current(
            &PolicyDigest::new(Digest::of_bytes(b"relaxed-policy")),
            &snapshot(b"grant").digest().unwrap()
        ));
        assert!(!context.still_current(
            &PolicyDigest::new(Digest::of_bytes(b"policy")),
            &snapshot(b"revoked").digest().unwrap()
        ));
    }
}
