//! `ProviderCredentialHandle` — secret-material indirection, and nothing else.
//!
//! A handle names *where the secret is*. It never names where an effect
//! happens or under whose authority.
//!
//! # The rule
//!
//! Swapping the handle may **not** change the tenant, account, repository,
//! namespace, bucket, remote project, security principal class, or any other
//! non-secret route semantics. Every one of those must be committed somewhere
//! canonical — the semantic definition, the route, the
//! [`CredentialAuthorityClass`], or an authority fact — so that changing it is
//! visible as a change of *plan* rather than a change of configuration.
//!
//! The attack this closes is mundane and easy to miss: an expired credential is
//! replaced, the replacement happens to point at a different account, and a
//! publication authorized against one destination silently delivers to another.
//! Nothing in the canonical record would show it, because the handle is not in
//! any digest.
//!
//! # Why it is in no digest
//!
//! Precisely because it is allowed to change. A rotated secret must not alter
//! the identity of a Publication, an Attempt or a Receipt — a receipt that
//! changed when a key was rotated would attest something other than what
//! happened. So the handle is mutable, noncanonical, absent from every digest
//! and from receipt identity, and never a historical authority proof.
//!
//! What makes rotation safe is [`ProviderCredentialHandle::preserves_route`]:
//! a replacement is acceptable only when it resolves to the same frozen
//! non-secret authority semantics as the handle it replaces.

use draft_dcg_contract::ids::ProviderBindingId;
use draft_dcg_contract::CredentialAuthorityClass;
use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// An operational reference to secret material.
///
/// Deliberately not `Serialize`-transparent into any canonical structure: it is
/// stored beside a binding, never inside a fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCredentialHandle {
    /// The binding this credential is used with.
    pub binding: ProviderBindingId,
    /// An opaque reference the secret store understands. Never the secret.
    pub reference: String,
    /// The non-secret authority class this credential resolves under.
    ///
    /// This is the part that may not change across a rotation, and the reason
    /// the class is canonical while the handle is not.
    pub authority_class: CredentialAuthorityClass,
}

impl ProviderCredentialHandle {
    pub fn new(
        binding: ProviderBindingId,
        reference: impl Into<String>,
        authority_class: CredentialAuthorityClass,
    ) -> DraftResult<Self> {
        let handle = Self {
            binding,
            reference: reference.into(),
            authority_class,
        };
        handle.validate()?;
        Ok(handle)
    }

    fn validate(&self) -> DraftResult<()> {
        if self.reference.trim().is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::InvalidConfig,
                "a credential handle must name where the secret is",
            ));
        }
        // A handle that looked like secret material would end up in logs and
        // diagnostics that are safe only because they never contain one.
        if self.reference.len() > 512 {
            return Err(DraftError::new(
                DraftErrorKind::InvalidConfig,
                "a credential handle is a reference, not the secret itself",
            ));
        }
        Ok(())
    }

    /// Whether `replacement` may be used in place of this handle.
    ///
    /// The one check that makes rotation safe. A replacement resolving to a
    /// different binding or a different authority class would move where the
    /// effect happens, or whose authority it happens under — neither of which a
    /// credential rotation is permitted to do.
    pub fn preserves_route(&self, replacement: &Self) -> DraftResult<()> {
        if self.binding != replacement.binding {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "the replacement credential is for binding '{}', not '{}'; a rotation may \
                     not redirect where an effect happens",
                    replacement.binding, self.binding
                ),
            ));
        }
        if self.authority_class != replacement.authority_class {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "the replacement credential resolves under '{}', not '{}'; a rotation may \
                     not change whose authority an effect happens under",
                    replacement.authority_class, self.authority_class
                ),
            )
            .with_suggestion(
                "Changing the account or tenant is a change of plan: re-plan and re-authorize.",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(name: &str) -> ProviderBindingId {
        ProviderBindingId::parse(format!("pbd_{name}")).unwrap()
    }

    fn class(name: &str) -> CredentialAuthorityClass {
        CredentialAuthorityClass::parse(name).unwrap()
    }

    fn handle(reference: &str) -> ProviderCredentialHandle {
        ProviderCredentialHandle::new(
            binding("aaa"),
            reference,
            class("acme.cloud/tenant-production"),
        )
        .unwrap()
    }

    #[test]
    fn an_expired_secret_may_be_replaced_in_place() {
        // The legitimate case rotation exists for.
        handle("vault://keys/deploy-2024")
            .preserves_route(&handle("vault://keys/deploy-2025"))
            .unwrap();
    }

    #[test]
    fn a_replacement_may_not_change_the_account_it_resolves_under() {
        // The mundane failure this closes: an expired credential is replaced,
        // the replacement points somewhere else, and a publication authorized
        // against one destination silently delivers to another.
        let staging = ProviderCredentialHandle::new(
            binding("aaa"),
            "vault://keys/deploy-2025",
            class("acme.cloud/tenant-staging"),
        )
        .unwrap();
        let error = handle("vault://keys/deploy-2024")
            .preserves_route(&staging)
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
        assert!(
            error.message.contains("whose authority"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_replacement_may_not_redirect_to_another_binding() {
        let elsewhere = ProviderCredentialHandle::new(
            binding("zzz"),
            "vault://keys/deploy-2025",
            class("acme.cloud/tenant-production"),
        )
        .unwrap();
        assert!(handle("vault://keys/deploy-2024")
            .preserves_route(&elsewhere)
            .is_err());
    }

    #[test]
    fn a_handle_must_name_where_the_secret_is() {
        assert!(ProviderCredentialHandle::new(
            binding("aaa"),
            "   ",
            class("acme.cloud/tenant-production")
        )
        .is_err());
    }

    #[test]
    fn a_handle_is_a_reference_rather_than_the_secret() {
        // Bounded, so something that looked like key material cannot be stored
        // here and end up in diagnostics that are safe only because they never
        // contain one.
        assert!(ProviderCredentialHandle::new(
            binding("aaa"),
            "x".repeat(513),
            class("acme.cloud/tenant-production")
        )
        .is_err());
    }

    #[test]
    fn the_authority_class_is_what_travels_canonically_not_the_handle() {
        // Two handles differing only in reference resolve under the same class,
        // which is exactly what lets rotation leave every digest untouched.
        let before = handle("vault://keys/deploy-2024");
        let after = handle("vault://keys/deploy-2025");
        assert_ne!(before.reference, after.reference);
        assert_eq!(before.authority_class, after.authority_class);
        before.preserves_route(&after).unwrap();
    }
}
