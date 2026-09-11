//! Composing the security resolvers, and committing what they agree on.
//!
//! # Who resolves what
//!
//! §2.27 splits this deliberately across four layers:
//!
//! | Layer | Owns |
//! |---|---|
//! | `draft-dcg-contract` | `SecurityFactRef` syntax, canonical form, digests |
//! | `core::project` | the `ProjectSecurityState` object and its storage |
//! | `authority` / `extension` / `trust` | retrieving facts, and their semantics |
//! | here | composing the resolvers, re-verifying digests, then the CAS |
//!
//! The split exists because each layer knows something the others must not
//! have to. `project` stores a set of references without being able to reach
//! the stores that own them; the owning stores know their own semantics
//! without knowing what a project's security state is; and this module — which
//! sits above all of them — is the only place that can ask every owner and
//! compare the answers.
//!
//! # Why every digest is recomputed here
//!
//! A `SecurityFactRef` carries a digest so that resolving it proves you got
//! the fact it names. Trusting the id alone would let a substituted grant
//! inherit every authorization the original earned, and the substitution would
//! be invisible: the reference still resolves, the store still returns a
//! grant, and only the bytes differ.
//!
//! So resolution re-derives the digest of what came back and compares. A
//! mismatch is corruption, refused — never repaired, because repairing it
//! would mean choosing which of two conflicting histories to believe.
//!
//! # Why the CAS comes last
//!
//! Verification is only true for the instant it was computed. Committing the
//! result under the control lock, against the generation the verification
//! observed, is what makes "these facts were valid" and "this is now the
//! project's state" the same event rather than two that can disagree.

use draft_dcg_contract::security::{SecurityControlKindId, SecurityFactRef};
use std::collections::BTreeSet;

use crate::authority::grant::{AuthorityGrantStore, AUTHORITY_GRANT_KIND};
use crate::authority::revocation::{AuthorityRevocationStore, AUTHORITY_REVOCATION_KIND};
use crate::project::security::ProjectSecurityState;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// What a resolver concluded about one reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactResolution {
    /// The fact was found and its canonical digest matches the reference.
    Verified,
    /// Nothing is stored under that logical id.
    Missing,
    /// A fact is stored, but its content digest is not the one referenced.
    ///
    /// Kept distinct from `Missing` because they mean opposite things about
    /// the project: one is an incomplete store, the other is a store holding
    /// something it should not.
    Substituted,
}

/// The stores that own the reserved fact kinds.
///
/// Passed in rather than located, so a caller cannot accidentally verify
/// against a different set of stores than the ones it will commit against.
pub struct SecurityResolvers<'a> {
    pub grants: &'a AuthorityGrantStore,
    pub revocations: &'a AuthorityRevocationStore,
}

impl SecurityResolvers<'_> {
    /// Resolve one reference through the store that owns its kind.
    ///
    /// An unrecognised reserved kind is refused rather than skipped: Draft
    /// reserves `draft.security.*`, so a reference in that namespace that
    /// nothing here implements is a fact this build cannot evaluate, and
    /// treating it as absent would silently drop a security control.
    pub fn resolve(&self, reference: &SecurityFactRef) -> DraftResult<FactResolution> {
        let kind = reference.kind.as_namespaced().to_string();
        let logical_id = reference.logical_id.as_ref().ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::Validation,
                format!("a {kind} reference must name the fact it resolves"),
            )
        })?;

        match kind.as_str() {
            AUTHORITY_GRANT_KIND => {
                let id = draft_dcg_contract::ids::AuthorityGrantId::parse(logical_id.as_str())
                    .map_err(|error| {
                        DraftError::new(DraftErrorKind::Validation, error.to_string())
                    })?;
                match self.grants.get(&id)? {
                    None => Ok(FactResolution::Missing),
                    Some(grant) => Ok(verify(grant.digest()?, reference)),
                }
            }
            AUTHORITY_REVOCATION_KIND => {
                let id = draft_dcg_contract::ids::AuthorityGrantId::parse(logical_id.as_str())
                    .map_err(|error| {
                        DraftError::new(DraftErrorKind::Validation, error.to_string())
                    })?;
                match self.revocations.get(&id)? {
                    None => Ok(FactResolution::Missing),
                    Some(revocation) => Ok(verify(revocation.digest()?, reference)),
                }
            }
            other if reference.kind.is_reserved() => Err(DraftError::new(
                DraftErrorKind::UnsupportedSchema,
                format!(
                    "'{other}' is in Draft's reserved security namespace but this build resolves \
                     no such fact; refusing rather than treating a security control as absent"
                ),
            )),
            // A vendor kind Draft does not own. Nothing here can speak to its
            // semantics, and inventing an answer would be worse than saying so.
            other => Err(DraftError::new(
                DraftErrorKind::UnsupportedSchema,
                format!("no resolver owns security control kind '{other}'"),
            )),
        }
    }
}

fn verify(actual: draft_dcg_contract::Digest, reference: &SecurityFactRef) -> FactResolution {
    if actual == reference.digest {
        FactResolution::Verified
    } else {
        // The bytes beneath a security fact's logical id have moved. Counted
        // at the one boundary that can tell — recomputing the digest and
        // comparing it against the reference that cited it.
        crate::support::telemetry::Counter::SecurityFactDigestMismatches.increment();
        FactResolution::Substituted
    }
}

/// Every reference in a security state that did not resolve cleanly.
///
/// Returns the failures rather than the successes: a caller needs to know what
/// is wrong, and an empty result is the only shape that means "commit this".
pub fn unresolved(
    resolvers: &SecurityResolvers<'_>,
    state: &ProjectSecurityState,
) -> DraftResult<Vec<(SecurityFactRef, FactResolution)>> {
    let mut failures = Vec::new();
    for reference in references_of(state) {
        match resolvers.resolve(&reference)? {
            FactResolution::Verified => {}
            other => failures.push((reference, other)),
        }
    }
    Ok(failures)
}

/// Whether a security state is internally consistent before it is committed.
///
/// A revocation whose grant is not itself in the state would withdraw
/// something the project never recorded granting, which is a state no sequence
/// of legitimate operations produces.
pub fn structurally_valid(state: &ProjectSecurityState) -> DraftResult<()> {
    state.validate()?;
    let granted: BTreeSet<&str> = state
        .active_authority_grants
        .iter()
        .filter_map(|reference| reference.logical_id.as_ref().map(|id| id.as_str()))
        .collect();
    for revocation in &state.authority_revocations {
        let Some(id) = revocation.logical_id.as_ref() else {
            continue;
        };
        if !granted.contains(id.as_str()) {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "the state revokes '{}' but records no grant of it; a revocation of \
                     something never granted describes a history that did not happen",
                    id.as_str()
                ),
            ));
        }
    }
    Ok(())
}

fn references_of(state: &ProjectSecurityState) -> Vec<SecurityFactRef> {
    state
        .active_authority_grants
        .iter()
        .chain(state.authority_revocations.iter())
        .chain(state.controls.iter())
        .cloned()
        .collect()
}

/// The reserved kinds this build can resolve.
pub fn resolvable_kinds() -> Vec<SecurityControlKindId> {
    [AUTHORITY_GRANT_KIND, AUTHORITY_REVOCATION_KIND]
        .into_iter()
        .filter_map(|kind| SecurityControlKindId::parse(kind).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::grant::AuthorityGrant;
    use crate::authority::revocation::AuthorityRevocation;
    use draft_dcg_contract::capability::CapabilityId;
    use draft_dcg_contract::identifier::ScopedId;
    use draft_dcg_contract::ids::{ActorId, AuthorityGrantId};
    use draft_dcg_contract::value::Timestamp;

    struct Fixture {
        _directory: tempfile::TempDir,
        grants: AuthorityGrantStore,
        revocations: AuthorityRevocationStore,
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        Fixture {
            grants: AuthorityGrantStore::new(directory.path().join("grants")),
            revocations: AuthorityRevocationStore::new(directory.path().join("revocations")),
            _directory: directory,
        }
    }

    fn grant(id: &str, subject: &str) -> AuthorityGrant {
        AuthorityGrant {
            id: AuthorityGrantId::parse(id).unwrap(),
            grantee: ActorId::parse("act_000000000001").unwrap(),
            capability: CapabilityId::parse("draft.publish/v1").unwrap(),
            subject: ScopedId::parse(subject).unwrap(),
            granted_by: ActorId::parse("act_000000000002").unwrap(),
            granted_at: Timestamp::from_unix_nanos(0),
            expires_at: None,
        }
    }

    #[test]
    fn a_reference_resolves_only_to_the_exact_fact_it_names() {
        let fixture = fixture();
        let stored = grant("auth_000000000001", "bas_000000000001");
        fixture.grants.put(&stored).unwrap();
        let resolvers = SecurityResolvers {
            grants: &fixture.grants,
            revocations: &fixture.revocations,
        };

        assert_eq!(
            resolvers.resolve(&stored.reference().unwrap()).unwrap(),
            FactResolution::Verified
        );
    }

    #[test]
    fn a_substituted_grant_is_detected_rather_than_silently_accepted() {
        // The attack this closes: a grant over a narrow subject is referenced
        // by a decision, then replaced with one over everything. The id still
        // resolves and the store still returns a grant — only the bytes moved.
        let fixture = fixture();
        let narrow = grant("auth_000000000001", "bas_000000000001");
        let reference = narrow.reference().unwrap();

        let wide = grant("auth_000000000001", "bas_999999999999");
        fixture.grants.put(&wide).unwrap();

        let resolvers = SecurityResolvers {
            grants: &fixture.grants,
            revocations: &fixture.revocations,
        };
        assert_eq!(
            resolvers.resolve(&reference).unwrap(),
            FactResolution::Substituted,
            "the reference names content the store no longer holds"
        );
    }

    #[test]
    fn a_missing_fact_is_distinct_from_a_substituted_one() {
        // Different problems needing different responses: an incomplete store
        // versus a store holding something it should not.
        let fixture = fixture();
        let absent = grant("auth_000000000009", "bas_000000000001");
        let resolvers = SecurityResolvers {
            grants: &fixture.grants,
            revocations: &fixture.revocations,
        };
        assert_eq!(
            resolvers.resolve(&absent.reference().unwrap()).unwrap(),
            FactResolution::Missing
        );
    }

    #[test]
    fn an_unimplemented_reserved_kind_is_refused_rather_than_ignored() {
        // Treating a reserved control this build cannot evaluate as absent
        // would silently drop a security control that a newer Draft enforces.
        let fixture = fixture();
        let resolvers = SecurityResolvers {
            grants: &fixture.grants,
            revocations: &fixture.revocations,
        };
        let unknown = SecurityFactRef::new(
            SecurityControlKindId::parse("draft.security/trust-decision.v1").unwrap(),
            Some(ScopedId::parse("auth_000000000001").unwrap()),
            draft_dcg_contract::Digest::of_bytes(b"whatever"),
        );
        let error = resolvers.resolve(&unknown).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::UnsupportedSchema);
    }

    #[test]
    fn revoking_something_never_granted_is_not_a_valid_state() {
        let fixture = fixture();
        let stored = grant("auth_000000000001", "bas_000000000001");
        fixture.grants.put(&stored).unwrap();
        let revocation = AuthorityRevocation {
            grant: stored.reference().unwrap(),
            grant_id: stored.id.clone(),
            revoked_by: ActorId::parse("act_000000000002").unwrap(),
            revoked_at: Timestamp::from_unix_nanos(1),
            reason: "left the project".into(),
        };
        fixture.revocations.put(&revocation).unwrap();

        let orphaned = ProjectSecurityState {
            active_authority_grants: Default::default(),
            authority_revocations: [revocation.reference().unwrap()].into_iter().collect(),
            controls: Default::default(),
        };
        assert!(
            structurally_valid(&orphaned).is_err(),
            "a revocation with no matching grant describes a history that did not happen"
        );

        let coherent = ProjectSecurityState {
            active_authority_grants: [stored.reference().unwrap()].into_iter().collect(),
            authority_revocations: [revocation.reference().unwrap()].into_iter().collect(),
            controls: Default::default(),
        };
        structurally_valid(&coherent).unwrap();
    }
}
