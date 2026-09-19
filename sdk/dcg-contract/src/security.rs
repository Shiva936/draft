//! Portable security *values*: how a canonical fact names the security material
//! that authorized it.
//!
//! This module owns syntax, canonical form and digest semantics — and nothing
//! else. It does not resolve a [`SecurityFactRef`], does not know whether a
//! grant is currently active, and cannot tell you whether a revocation has
//! since landed. Those are Core's: `authority`, `extension` and `trust` own
//! their facts and their semantics, and `app/security` composes the resolvers.
//!
//! The split matters because `core::project` must **not** redefine these types.
//! One canonical value has exactly one Rust type; two definitions with a
//! conversion between them is how canonical meaning drifts.

use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::identifier::{NamespacedId, ScopedId};
use crate::FormatResult;

/// What kind of security control a fact is.
///
/// Open and namespaced, with a reserved v1 set Draft implements.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecurityControlKindId(NamespacedId);

/// The reserved v1 security control kinds.
pub const RESERVED_SECURITY_CONTROL_KINDS: &[&str] = &[
    "draft.security/authority-grant.v1",
    "draft.security/authority-revocation.v1",
    "draft.security/extension-authorization.v1",
    "draft.security/trust-decision.v1",
];

impl SecurityControlKindId {
    pub fn parse(value: &str) -> FormatResult<Self> {
        Ok(Self(NamespacedId::parse(value)?))
    }

    pub fn as_namespaced(&self) -> &NamespacedId {
        &self.0
    }

    pub fn is_reserved(&self) -> bool {
        self.0.namespace() == "draft" || self.0.namespace().starts_with("draft.")
    }

    pub fn is_recognised_reserved(&self) -> bool {
        RESERVED_SECURITY_CONTROL_KINDS.contains(&self.0.qualified().as_str())
    }

    /// An unrecognised reserved kind is rejected rather than treated as an
    /// unknown vendor control, for the same reason as capabilities: otherwise a
    /// typo in a reserved name becomes a control nobody enforces.
    pub fn is_acceptable(&self) -> bool {
        !self.is_reserved() || self.is_recognised_reserved()
    }
}

impl std::fmt::Display for SecurityControlKindId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// An **exact** reference to one immutable security fact.
///
/// The digest is the point. A bare logical id would let the bytes beneath that
/// id be replaced — widening a grant, changing its scope, or removing its
/// expiry — while every fact that cited it still appeared to cite the same
/// thing. Carrying the digest means substitution is detectable at the moment of
/// consumption.
///
/// Resolution is: load by `logical_id` (or by kind, where the control is
/// singular), recompute the canonical digest, and require equality. A mismatch
/// is an integrity violation, never a warning and never something to repair.
///
/// A ref that merely *resolves* proves which fact was cited. It does **not**
/// prove that fact was live, in scope and unrevoked when it was relied on —
/// that is a separate, current-authority question.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityFactRef {
    /// Which kind of security control this fact is.
    pub kind: SecurityControlKindId,
    /// The fact's logical id, where the control has one.
    ///
    /// Absent for a control that is singular within its kind and addressed by
    /// kind alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_id: Option<ScopedId>,
    /// The canonical digest of the exact immutable fact.
    pub digest: Digest,
}

impl SecurityFactRef {
    pub fn new(kind: SecurityControlKindId, logical_id: Option<ScopedId>, digest: Digest) -> Self {
        Self {
            kind,
            logical_id,
            digest,
        }
    }
}

/// Declares a canonical digest value whose runtime object lives in Core.
macro_rules! digest_value {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Digest);

        impl $name {
            pub fn new(digest: Digest) -> Self {
                Self(digest)
            }

            pub fn digest(&self) -> &Digest {
                &self.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

digest_value!(
    /// The digest of a `ProjectSecurityState` — the set of security facts a
    /// project had in force at one moment.
    ///
    /// The value is portable; the object, its composition and its pointer
    /// lifecycle live in `core::project`. Never conflated with a
    /// `SecurityContextDigest`, which covers a gate evaluation's resolved
    /// dependencies rather than the project's state.
    ProjectSecurityStateDigest
);
digest_value!(
    /// The digest of the `PolicySnapshot` in force. The snapshot itself and
    /// policy state live in `core::project`.
    PolicyDigest
);
digest_value!(
    /// The digest of a `SecurityContextSnapshot` — the dependencies an
    /// evaluation rested on **together with the facts they resolved to**.
    ///
    /// Never conflated with [`ProjectSecurityStateDigest`]. That one covers the
    /// *set of references* a project holds; this one covers the resolved
    /// content behind them. They move independently, and the difference is
    /// exactly the case that matters: substituting a fact behind a stable
    /// reference leaves the project's state digest unchanged while changing
    /// what an evaluation actually rested on.
    ///
    /// The snapshot object lives in `core::evidence`.
    SecurityContextDigest
);

/// The non-secret authority class a credential resolves under.
///
/// Deliberately not a credential, a handle, or anything that could identify
/// secret material: it is the *class of authority* an external effect would be
/// performed under, which is policy-relevant and therefore belongs in a
/// canonical fact. Secret indirection lives in `core::project` as an
/// operational `CredentialHandleRef`, appears in no digest, and can never
/// change the account, tenant or route a Publication was frozen against.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialAuthorityClass(NamespacedId);

impl CredentialAuthorityClass {
    pub fn parse(value: &str) -> FormatResult<Self> {
        Ok(Self(NamespacedId::parse(value)?))
    }

    pub fn as_namespaced(&self) -> &NamespacedId {
        &self.0
    }
}

impl std::fmt::Display for CredentialAuthorityClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(seed: &[u8]) -> Digest {
        Digest::of_bytes(seed)
    }

    fn grant_kind() -> SecurityControlKindId {
        SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap()
    }

    #[test]
    fn every_reserved_control_kind_parses_and_is_recognised() {
        for reserved in RESERVED_SECURITY_CONTROL_KINDS {
            let kind = SecurityControlKindId::parse(reserved).unwrap();
            assert!(kind.is_reserved() && kind.is_recognised_reserved() && kind.is_acceptable());
        }
    }

    #[test]
    fn an_unrecognised_reserved_control_kind_is_rejected() {
        let typo = SecurityControlKindId::parse("draft.security/authority-grants.v1").unwrap();
        assert!(typo.is_reserved());
        assert!(!typo.is_acceptable());
    }

    #[test]
    fn a_ref_distinguishes_facts_that_share_a_logical_id() {
        // This is the substitution the digest exists to catch: same kind, same
        // logical id, different bytes.
        let id = ScopedId::parse("auth_9f8e7d").unwrap();
        let original = SecurityFactRef::new(grant_kind(), Some(id.clone()), digest(b"grant-v1"));
        let substituted = SecurityFactRef::new(grant_kind(), Some(id), digest(b"grant-widened"));
        assert_ne!(original, substituted);
    }

    #[test]
    fn a_singular_control_may_be_addressed_by_kind_alone() {
        let reference = SecurityFactRef::new(grant_kind(), None, digest(b"x"));
        let encoded = serde_json::to_string(&reference).unwrap();
        // The absent id is omitted rather than written as null, so the
        // canonical form has one shape.
        assert!(!encoded.contains("logical_id"), "{encoded}");
        assert_eq!(
            serde_json::from_str::<SecurityFactRef>(&encoded).unwrap(),
            reference
        );
    }

    #[test]
    fn refs_sort_deterministically_for_canonical_sets() {
        let mut refs = [
            SecurityFactRef::new(
                grant_kind(),
                Some(ScopedId::parse("b").unwrap()),
                digest(b"1"),
            ),
            SecurityFactRef::new(
                grant_kind(),
                Some(ScopedId::parse("a").unwrap()),
                digest(b"1"),
            ),
        ];
        refs.sort();
        assert_eq!(refs[0].logical_id.as_ref().unwrap().as_str(), "a");
    }

    #[test]
    fn a_credential_authority_class_is_namespaced_and_carries_no_secret() {
        let class = CredentialAuthorityClass::parse("acme.cloud/tenant-production").unwrap();
        assert_eq!(class.to_string(), "acme.cloud/tenant-production");
        assert!(CredentialAuthorityClass::parse("tenant-production").is_err());
    }

    #[test]
    fn digest_values_are_distinct_types_over_the_same_wire_form() {
        let security = ProjectSecurityStateDigest::new(digest(b"s"));
        let policy = PolicyDigest::new(digest(b"s"));
        assert_eq!(
            serde_json::to_string(&security).unwrap(),
            serde_json::to_string(&policy).unwrap()
        );
    }
}
