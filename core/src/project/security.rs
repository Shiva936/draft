//! `ProjectSecurityState` — the security facts a project has in force.
//!
//! Three sets of **exact** references, and nothing else:
//!
//! ```text
//! ProjectSecurityState { active_authority_grants, authority_revocations, controls }
//! ```
//!
//! Each entry is a [`SecurityFactRef`] carrying the digest of the immutable
//! fact it names. That is what makes the state tamper-evident rather than
//! merely descriptive: a grant cannot be widened, re-scoped or stripped of its
//! expiry beneath an identifier the project already trusts, because the
//! reference would stop resolving.
//!
//! # This module owns composition, not semantics
//!
//! What a grant *means*, whether it is currently in scope, and whether a
//! revocation applies are questions for `authority`, `extension` and `trust` —
//! the modules that own those facts. `project` owns only the set: which facts
//! are in force, and what digest that set has.
//!
//! The split matters because `project` sits below those modules and must not
//! import them. A `ProjectSecurityState` that could evaluate its own contents
//! would have to, and the platform's base would depend on the security
//! subsystems that are supposed to sit above it.
//!
//! # A revoked grant is removed *and* recorded
//!
//! Revocations are kept alongside the active set rather than simply deleting
//! the grant. "This was never granted" and "this was granted and then withdrawn"
//! are different histories, and only the second explains why an operation that
//! used to succeed now fails.

use std::collections::BTreeSet;

use draft_dcg_contract::{ProjectSecurityStateDigest, SecurityControlKindId, SecurityFactRef};
use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;

/// The security facts in force for one project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSecurityState {
    /// Grants currently conferring authority.
    pub active_authority_grants: BTreeSet<SecurityFactRef>,
    /// Revocations, retained so a withdrawal stays explicable.
    pub authority_revocations: BTreeSet<SecurityFactRef>,
    /// Every other security control in force — extension authorizations, trust
    /// decisions, and whatever later kinds are added.
    pub controls: BTreeSet<SecurityFactRef>,
}

impl ProjectSecurityState {
    /// This state's canonical digest.
    pub fn digest(&self) -> DraftResult<ProjectSecurityStateDigest> {
        self.validate()?;
        let digest = try_canonical_hash(self)?;
        Ok(ProjectSecurityStateDigest::new(
            draft_dcg_contract::Digest::parse(digest)
                .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
        ))
    }

    /// Structural checks this layer can make without interpreting a fact.
    ///
    /// Deliberately shallow. Whether a grant is in scope is not knowable here,
    /// and pretending otherwise would drag the security subsystems below the
    /// layer they sit above.
    pub fn validate(&self) -> DraftResult<()> {
        // The same fact cannot be simultaneously active and revoked. That is
        // not a scope judgement — it is the set contradicting itself.
        if let Some(contradiction) = self
            .active_authority_grants
            .intersection(&self.authority_revocations)
            .next()
        {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "security fact {} is listed as both active and revoked",
                    contradiction.digest
                ),
            ));
        }
        for kind in self
            .active_authority_grants
            .iter()
            .chain(&self.authority_revocations)
            .chain(&self.controls)
            .map(|reference| &reference.kind)
        {
            if !kind.is_acceptable() {
                return Err(DraftError::new(
                    DraftErrorKind::Validation,
                    format!(
                        "'{kind}' is in Draft's reserved namespace but is not a control kind this \
                         build implements"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Whether this exact fact is in force.
    ///
    /// Exact: the digest must match too, so a fact whose bytes have changed is
    /// not the fact the project accepted.
    pub fn is_active(&self, reference: &SecurityFactRef) -> bool {
        self.active_authority_grants.contains(reference) || self.controls.contains(reference)
    }

    /// Whether this exact fact has been revoked.
    pub fn is_revoked(&self, reference: &SecurityFactRef) -> bool {
        self.authority_revocations.contains(reference)
    }

    /// Every fact of one control kind.
    pub fn controls_of_kind(&self, kind: &SecurityControlKindId) -> Vec<&SecurityFactRef> {
        self.controls
            .iter()
            .chain(&self.active_authority_grants)
            .filter(|reference| &reference.kind == kind)
            .collect()
    }

    /// Grant `reference`, removing any prior revocation of the same fact.
    pub fn grant(&self, reference: SecurityFactRef) -> Self {
        let mut next = self.clone();
        next.authority_revocations.remove(&reference);
        next.active_authority_grants.insert(reference);
        next
    }

    /// Revoke `reference`, keeping the revocation on the record.
    pub fn revoke(&self, reference: SecurityFactRef) -> Self {
        let mut next = self.clone();
        next.active_authority_grants.remove(&reference);
        next.authority_revocations.insert(reference);
        next
    }
}

/// Create-once storage for project security states, keyed by their digest.
///
/// The control record names the state in force by digest; this is where the
/// bytes behind that digest live. Storing them by digest makes the create-once
/// binding trivially true — a different set of facts is a different state —
/// and means a reader can resolve what the project's authority actually *was*
/// at any historical moment rather than only that it hashed to something.
///
/// # Why the state is stored at all
///
/// Without it, `ProjectControlState.project_security_state` is a digest of
/// something nobody can read back. Every grant would be unresolvable, so every
/// authority evaluation would refuse — and a system that can only ever refuse
/// is not enforcing a rule, it is failing to implement one.
pub struct ProjectSecurityStateStore {
    facts: crate::support::immutable_store::ImmutableFactStore<ProjectSecurityState>,
}

impl ProjectSecurityStateStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            facts: crate::support::immutable_store::ImmutableFactStore::new(directory),
        }
    }

    pub fn for_layout(layout: &crate::project::layout::DraftLayout) -> Self {
        Self::new(layout.security_states_dir())
    }

    /// Store a state and return the digest it is filed under.
    pub fn put(&self, state: &ProjectSecurityState) -> DraftResult<ProjectSecurityStateDigest> {
        let digest = state.digest()?;
        self.facts.put(&digest.digest().to_string(), state)?;
        Ok(digest)
    }

    /// The state behind a digest, verified against the digest it is filed under.
    ///
    /// A stored state that no longer recomputes to its own key is corruption,
    /// not something to return: it would mean the facts a decision rested on
    /// had changed while the decision still cited them.
    pub fn get(
        &self,
        digest: &ProjectSecurityStateDigest,
    ) -> DraftResult<Option<ProjectSecurityState>> {
        let Some(state) = self.facts.get(&digest.digest().to_string())? else {
            return Ok(None);
        };
        let recomputed = state.digest()?;
        if &recomputed != digest {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("security state {digest} holds facts computing to {recomputed}"),
            ));
        }
        Ok(Some(state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::identifier::ScopedId;
    use draft_dcg_contract::Digest;

    fn grant_kind() -> SecurityControlKindId {
        SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap()
    }

    fn fact(id: &str, seed: &[u8]) -> SecurityFactRef {
        SecurityFactRef::new(
            grant_kind(),
            Some(ScopedId::parse(id).unwrap()),
            Digest::of_bytes(seed),
        )
    }

    #[test]
    fn an_empty_state_has_a_defined_digest() {
        ProjectSecurityState::default().digest().unwrap();
    }

    #[test]
    fn granting_and_revoking_move_a_fact_between_sets() {
        let granted = ProjectSecurityState::default().grant(fact("auth_1", b"grant"));
        assert!(granted.is_active(&fact("auth_1", b"grant")));
        assert!(!granted.is_revoked(&fact("auth_1", b"grant")));

        let revoked = granted.revoke(fact("auth_1", b"grant"));
        assert!(!revoked.is_active(&fact("auth_1", b"grant")));
        assert!(revoked.is_revoked(&fact("auth_1", b"grant")));
    }

    #[test]
    fn a_revocation_is_retained_so_a_withdrawal_stays_explicable() {
        // "Never granted" and "granted then withdrawn" are different histories,
        // and only the second explains why something that used to work stopped.
        let revoked = ProjectSecurityState::default()
            .grant(fact("auth_1", b"grant"))
            .revoke(fact("auth_1", b"grant"));
        assert_eq!(revoked.authority_revocations.len(), 1);

        let never_granted = ProjectSecurityState::default();
        assert!(never_granted.authority_revocations.is_empty());
        assert_ne!(
            revoked.digest().unwrap(),
            never_granted.digest().unwrap(),
            "the two histories must not have the same security state"
        );
    }

    #[test]
    fn re_granting_a_revoked_fact_clears_the_revocation() {
        let restored = ProjectSecurityState::default()
            .grant(fact("auth_1", b"grant"))
            .revoke(fact("auth_1", b"grant"))
            .grant(fact("auth_1", b"grant"));
        assert!(restored.is_active(&fact("auth_1", b"grant")));
        assert!(!restored.is_revoked(&fact("auth_1", b"grant")));
        restored.validate().unwrap();
    }

    #[test]
    fn a_fact_cannot_be_both_active_and_revoked() {
        // Not a scope judgement — the set contradicting itself.
        let mut contradictory = ProjectSecurityState::default();
        contradictory
            .active_authority_grants
            .insert(fact("auth_1", b"grant"));
        contradictory
            .authority_revocations
            .insert(fact("auth_1", b"grant"));
        assert_eq!(
            contradictory.validate().unwrap_err().kind,
            DraftErrorKind::CorruptData
        );
    }

    #[test]
    fn a_substituted_grant_is_no_longer_the_fact_the_project_accepted() {
        // The whole reason the reference carries a digest: a grant cannot be
        // widened beneath an identifier the project already trusts.
        let state = ProjectSecurityState::default().grant(fact("auth_1", b"grant"));
        assert!(state.is_active(&fact("auth_1", b"grant")));
        assert!(!state.is_active(&fact("auth_1", b"grant-widened")));
    }

    #[test]
    fn an_unrecognised_reserved_control_kind_is_refused() {
        let mut state = ProjectSecurityState::default();
        state.controls.insert(SecurityFactRef::new(
            SecurityControlKindId::parse("draft.security/authority-grants.v1").unwrap(),
            None,
            Digest::of_bytes(b"x"),
        ));
        assert_eq!(
            state.validate().unwrap_err().kind,
            DraftErrorKind::Validation
        );
    }

    #[test]
    fn a_vendor_control_kind_is_carried_without_being_understood() {
        let mut state = ProjectSecurityState::default();
        state.controls.insert(SecurityFactRef::new(
            SecurityControlKindId::parse("acme.security/approval.v1").unwrap(),
            None,
            Digest::of_bytes(b"x"),
        ));
        state.validate().unwrap();
        state.digest().unwrap();
    }

    #[test]
    fn the_digest_is_order_independent_and_content_sensitive() {
        let forward = ProjectSecurityState::default()
            .grant(fact("auth_1", b"a"))
            .grant(fact("auth_2", b"b"));
        let reversed = ProjectSecurityState::default()
            .grant(fact("auth_2", b"b"))
            .grant(fact("auth_1", b"a"));
        assert_eq!(forward.digest().unwrap(), reversed.digest().unwrap());

        let different = ProjectSecurityState::default()
            .grant(fact("auth_1", b"a"))
            .grant(fact("auth_2", b"changed"));
        assert_ne!(forward.digest().unwrap(), different.digest().unwrap());
    }

    #[test]
    fn controls_are_findable_by_kind() {
        let state = ProjectSecurityState::default().grant(fact("auth_1", b"grant"));
        assert_eq!(state.controls_of_kind(&grant_kind()).len(), 1);
        assert!(state
            .controls_of_kind(
                &SecurityControlKindId::parse("draft.security/trust-decision.v1").unwrap()
            )
            .is_empty());
    }
}
