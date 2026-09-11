//! Capability authorization for installed extensions.
//!
//! Trusting a source, installing a package, and authorizing what that package
//! may do are three separate decisions, and none of them implies the next. An
//! extension can be installed and enabled with no authorization at all: its
//! static contributions are active and its command-bearing ones are inert.
//! That is a normal steady state, not a failure.
//!
//! A grant is bound to one exact artifact — id, source, publisher, version and
//! content digest. Any change to the version or the digest means the grant no
//! longer matches, so **every** update needs fresh authorization, including one
//! that asks for exactly the same permissions. Superseded grants are kept, not
//! deleted, because they are the audit trail of what was once permitted.

use crate::contracts::{ContractId, VersionedContract};
use draft_extension_contract::ExtensionPermission;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The artifact a grant is bound to.
///
/// Every field participates in matching. Two installations of the same
/// extension id from different sources, at different versions, or with
/// different content are different artifacts and need their own grants.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizedArtifact {
    pub extension_id: String,
    /// The catalog source the package came from, or `None` for a package the
    /// user pointed at directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    pub publisher: String,
    pub package_version: String,
    pub content_hash: String,
}

impl AuthorizedArtifact {
    /// The artifact an installed record describes.
    pub fn of(installed: &super::InstalledExtension) -> Self {
        Self {
            extension_id: installed.id().to_string(),
            source_id: installed.update_source_id().map(ToOwned::to_owned),
            publisher: installed.manifest.publisher.clone(),
            package_version: installed.version().to_string(),
            content_hash: installed.content_hash.clone(),
        }
    }
}

/// One durable authorization decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionAuthorizationGrant {
    pub schema_version: u32,
    pub artifact: AuthorizedArtifact,
    pub permissions: Vec<ExtensionPermission>,
    pub actor_id: String,
    pub decision_id: String,
    pub operation_id: String,
    pub decided_at: String,
    /// Set when a later install or update replaced the artifact this grant was
    /// bound to. Superseded grants never authorize anything again; they are
    /// retained so the record of what was permitted survives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_at: Option<String>,
}

impl VersionedContract for ExtensionAuthorizationGrant {
    const CONTRACT: ContractId = ContractId::ExtensionAuthorizationGrant;
}

impl ExtensionAuthorizationGrant {
    /// Whether this grant authorizes `permission` for `artifact`.
    pub fn authorizes(
        &self,
        artifact: &AuthorizedArtifact,
        permission: ExtensionPermission,
    ) -> bool {
        self.superseded_at.is_none()
            && &self.artifact == artifact
            && self.permissions.contains(&permission)
    }

    /// Whether this grant is bound to `artifact`, regardless of what it permits.
    pub fn binds(&self, artifact: &AuthorizedArtifact) -> bool {
        &self.artifact == artifact
    }
}

/// Every authorization decision Draft has recorded, current and superseded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionAuthorizationRegistry {
    pub schema_version: u32,
    /// Current grants, keyed by extension id. At most one artifact per
    /// extension can be authorized at a time, because at most one is installed.
    pub grants: BTreeMap<String, ExtensionAuthorizationGrant>,
    /// Grants that no longer bind, kept for audit.
    #[serde(default)]
    pub superseded: Vec<ExtensionAuthorizationGrant>,
}

impl VersionedContract for ExtensionAuthorizationRegistry {
    const CONTRACT: ContractId = ContractId::ExtensionAuthorizationRegistry;
}

impl Default for ExtensionAuthorizationRegistry {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                ContractId::ExtensionAuthorizationRegistry,
            ),
            grants: BTreeMap::new(),
            superseded: Vec::new(),
        }
    }
}

impl ExtensionAuthorizationRegistry {
    /// The permissions currently authorized for `artifact`.
    ///
    /// A grant recorded against a different version, digest, source or
    /// publisher authorizes nothing here — that is the whole point of binding
    /// grants to artifacts.
    pub fn authorized_permissions(
        &self,
        artifact: &AuthorizedArtifact,
    ) -> Vec<ExtensionPermission> {
        self.grants
            .get(&artifact.extension_id)
            .filter(|grant| grant.superseded_at.is_none() && grant.binds(artifact))
            .map(|grant| grant.permissions.clone())
            .unwrap_or_default()
    }

    /// Whether `permission` is currently authorized for `artifact`.
    pub fn authorizes(
        &self,
        artifact: &AuthorizedArtifact,
        permission: ExtensionPermission,
    ) -> bool {
        self.grants
            .get(&artifact.extension_id)
            .is_some_and(|grant| grant.authorizes(artifact, permission))
    }

    /// Record `grant`, retiring whatever it replaces.
    pub fn record(&mut self, grant: ExtensionAuthorizationGrant) {
        let extension_id = grant.artifact.extension_id.clone();
        if let Some(previous) = self.grants.remove(&extension_id) {
            self.retire(previous, &grant.decided_at);
        }
        self.grants.insert(extension_id, grant);
    }

    /// Retire the grant for `extension_id` because its artifact changed.
    ///
    /// Called when a package is updated or reinstalled: the new artifact has a
    /// different version or digest, so the old grant can no longer match and
    /// must not be silently carried forward.
    pub fn supersede(
        &mut self,
        extension_id: &str,
        at: &str,
    ) -> Option<ExtensionAuthorizationGrant> {
        let previous = self.grants.remove(extension_id)?;
        let retired = previous.clone();
        self.retire(previous, at);
        Some(retired)
    }

    /// Drop the current grant for `extension_id` at the user's request.
    pub fn revoke(&mut self, extension_id: &str, at: &str) -> Option<ExtensionAuthorizationGrant> {
        self.supersede(extension_id, at)
    }

    /// Drop only `permission` from the current grant, retiring the grant
    /// entirely if nothing is left.
    pub fn revoke_permission(
        &mut self,
        extension_id: &str,
        permission: ExtensionPermission,
        at: &str,
    ) -> Option<ExtensionAuthorizationGrant> {
        let current = self.grants.get(extension_id)?.clone();
        if !current.permissions.contains(&permission) {
            return None;
        }
        let remaining: Vec<ExtensionPermission> = current
            .permissions
            .iter()
            .copied()
            .filter(|held| *held != permission)
            .collect();
        if remaining.is_empty() {
            return self.revoke(extension_id, at);
        }
        let narrowed = ExtensionAuthorizationGrant {
            permissions: remaining,
            decided_at: at.to_string(),
            ..current.clone()
        };
        self.retire(current.clone(), at);
        self.grants.insert(extension_id.to_string(), narrowed);
        Some(current)
    }

    fn retire(&mut self, mut grant: ExtensionAuthorizationGrant, at: &str) {
        if grant.superseded_at.is_none() {
            grant.superseded_at = Some(at.to_string());
        }
        self.superseded.push(grant);
    }
}

/// What still stands between an installed extension and acting on all of its
/// contributions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAuthorization {
    pub extension_id: String,
    pub package_version: String,
    /// Permissions the package declares that are not currently authorized for
    /// the installed artifact.
    pub missing_permissions: Vec<ExtensionPermission>,
    /// Whether a grant exists for a *different* artifact of the same extension
    /// — the update case, where re-authorization is required.
    pub superseded_by_update: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact() -> AuthorizedArtifact {
        AuthorizedArtifact {
            extension_id: "draft.language.rust".into(),
            source_id: Some("draft-official".into()),
            publisher: "draft".into(),
            package_version: "1.0.0".into(),
            content_hash: "sha256:a".into(),
        }
    }

    fn grant(artifact: AuthorizedArtifact) -> ExtensionAuthorizationGrant {
        ExtensionAuthorizationGrant {
            schema_version: crate::contracts::current_version(
                ContractId::ExtensionAuthorizationGrant,
            ),
            artifact,
            permissions: vec![ExtensionPermission::ProcessExecute],
            actor_id: "act_1".into(),
            decision_id: "dec_1".into(),
            operation_id: "op_1".into(),
            decided_at: "2026-01-01T00:00:00Z".into(),
            superseded_at: None,
        }
    }

    #[test]
    fn a_grant_authorizes_only_the_artifact_it_was_bound_to() {
        let mut registry = ExtensionAuthorizationRegistry::default();
        registry.record(grant(artifact()));
        assert!(registry.authorizes(&artifact(), ExtensionPermission::ProcessExecute));

        for different in [
            AuthorizedArtifact {
                package_version: "1.0.1".into(),
                ..artifact()
            },
            AuthorizedArtifact {
                content_hash: "sha256:b".into(),
                ..artifact()
            },
            AuthorizedArtifact {
                source_id: Some("acme".into()),
                ..artifact()
            },
            AuthorizedArtifact {
                publisher: "someone-else".into(),
                ..artifact()
            },
        ] {
            assert!(
                !registry.authorizes(&different, ExtensionPermission::ProcessExecute),
                "a grant must not carry over to {different:?}"
            );
            assert!(registry.authorized_permissions(&different).is_empty());
        }
    }

    #[test]
    fn an_equal_permission_update_still_needs_reauthorization() {
        let mut registry = ExtensionAuthorizationRegistry::default();
        registry.record(grant(artifact()));

        // The update asks for exactly the same permission, but it is a
        // different artifact, so the old grant cannot cover it.
        let updated = AuthorizedArtifact {
            package_version: "2.0.0".into(),
            content_hash: "sha256:b".into(),
            ..artifact()
        };
        registry.supersede("draft.language.rust", "2026-02-01T00:00:00Z");
        assert!(!registry.authorizes(&updated, ExtensionPermission::ProcessExecute));

        registry.record(grant(updated.clone()));
        assert!(registry.authorizes(&updated, ExtensionPermission::ProcessExecute));
        // And the original artifact is no longer authorized either.
        assert!(!registry.authorizes(&artifact(), ExtensionPermission::ProcessExecute));
    }

    #[test]
    fn superseded_grants_are_retained_for_audit() {
        let mut registry = ExtensionAuthorizationRegistry::default();
        registry.record(grant(artifact()));
        registry.supersede("draft.language.rust", "2026-02-01T00:00:00Z");

        assert!(registry.grants.is_empty());
        assert_eq!(registry.superseded.len(), 1);
        assert_eq!(
            registry.superseded[0].superseded_at.as_deref(),
            Some("2026-02-01T00:00:00Z")
        );
        assert!(
            !registry.superseded[0].authorizes(&artifact(), ExtensionPermission::ProcessExecute)
        );
    }

    #[test]
    fn revoking_the_only_permission_retires_the_grant() {
        let mut registry = ExtensionAuthorizationRegistry::default();
        registry.record(grant(artifact()));
        registry.revoke_permission(
            "draft.language.rust",
            ExtensionPermission::ProcessExecute,
            "2026-02-01T00:00:00Z",
        );
        assert!(registry.grants.is_empty());
        assert_eq!(registry.superseded.len(), 1);
        assert!(!registry.authorizes(&artifact(), ExtensionPermission::ProcessExecute));
    }

    #[test]
    fn revoking_an_unheld_permission_changes_nothing() {
        let mut registry = ExtensionAuthorizationRegistry::default();
        registry.record(ExtensionAuthorizationGrant {
            permissions: vec![],
            ..grant(artifact())
        });
        assert!(registry
            .revoke_permission(
                "draft.language.rust",
                ExtensionPermission::ProcessExecute,
                "2026-02-01T00:00:00Z"
            )
            .is_none());
        assert_eq!(registry.grants.len(), 1);
        assert!(registry.superseded.is_empty());
    }

    #[test]
    fn recording_a_replacement_retires_what_it_replaces() {
        let mut registry = ExtensionAuthorizationRegistry::default();
        registry.record(grant(artifact()));
        let replacement = grant(AuthorizedArtifact {
            content_hash: "sha256:b".into(),
            ..artifact()
        });
        registry.record(ExtensionAuthorizationGrant {
            decided_at: "2026-03-01T00:00:00Z".into(),
            ..replacement
        });
        assert_eq!(registry.grants.len(), 1);
        assert_eq!(registry.superseded.len(), 1);
        assert!(!registry.authorizes(&artifact(), ExtensionPermission::ProcessExecute));
    }
}

/// The Draft semantics that decide an authorization outcome.
///
/// Authorization is more than byte equality: it spans artifact digest and
/// version binding, publisher identity, permission widening and revocation. A
/// historical decision therefore records the evaluator it was produced by, so a
/// later change to those rules cannot silently reinterpret what was permitted at
/// the time. Independent of the acceptance, change-derivation and aggregator
/// revisions: authorization semantics move on their own schedule.
pub const AUTHORIZATION_EVALUATOR_REVISION: u32 = 1;

/// Whether an artifact was permitted to perform one capability, at one moment.
///
/// This is the other half of the pair whose first half is
/// [`super::provenance::ArtifactAttestation`]. Trust answers *what this artifact
/// is*; this answers *what it was allowed to do*. Keeping them apart is what
/// makes a later revocation affect future runs without rewriting the record that
/// an operation was authorized when it ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationDecision {
    pub schema_version: u32,
    /// The evaluator semantics this outcome was produced under.
    pub evaluator_revision: u32,
    /// The trust fact this decision was made about.
    pub artifact_attestation_digest: String,
    pub package_digest: String,
    pub subject: AuthorizationSubject,
    pub required_permissions: Vec<ExtensionPermission>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_decision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_binding_digest: Option<String>,
    pub evaluated_at: String,
    pub result: AuthorizationResult,
    pub decision_digest: String,
}

impl VersionedContract for AuthorizationDecision {
    const CONTRACT: ContractId = ContractId::AuthorizationDecision;
}

/// What an authorization decision was about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationSubject {
    /// The contribution whose operation was being run.
    pub contribution_id: String,
    /// The capability being exercised, in Draft's neutral vocabulary.
    pub capability: super::capability::ExtensionCapabilityKind,
    /// The operation this decision authorized, when one had been opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthorizationResult {
    Authorized,
    Refused { reason: String },
}

impl AuthorizationResult {
    pub fn is_authorized(&self) -> bool {
        matches!(self, Self::Authorized)
    }
}

impl AuthorizationDecision {
    /// Seal a decision, deriving its content-addressed identity.
    pub fn seal(mut self) -> Self {
        self.schema_version = crate::contracts::current_version(ContractId::AuthorizationDecision);
        self.evaluator_revision = AUTHORIZATION_EVALUATOR_REVISION;
        self.decision_digest = String::new();
        let digest = crate::support::hashing::canonical_hash(&self);
        self.decision_digest = digest;
        self
    }

    pub fn is_authorized(&self) -> bool {
        self.result.is_authorized()
    }
}

#[cfg(test)]
mod decision_tests {
    use super::*;
    use crate::extension::capability::ExtensionCapabilityKind;

    fn decision(
        capability: ExtensionCapabilityKind,
        result: AuthorizationResult,
    ) -> AuthorizationDecision {
        AuthorizationDecision {
            schema_version: 0,
            evaluator_revision: 0,
            artifact_attestation_digest: "sha256:attestation".into(),
            package_digest: "sha256:package".into(),
            subject: AuthorizationSubject {
                contribution_id: "verification".into(),
                capability,
                operation_id: Some("op_1".into()),
            },
            required_permissions: vec![ExtensionPermission::ProcessExecute],
            grant_decision_id: Some("dec_1".into()),
            grant_binding_digest: Some("sha256:binding".into()),
            evaluated_at: "2026-01-01T00:00:00Z".into(),
            result,
            decision_digest: String::new(),
        }
        .seal()
    }

    #[test]
    fn a_decision_commits_to_the_evaluator_that_produced_it() {
        let sealed = decision(
            ExtensionCapabilityKind::Verification,
            AuthorizationResult::Authorized,
        );
        assert_eq!(sealed.evaluator_revision, AUTHORIZATION_EVALUATOR_REVISION);
        assert!(!sealed.decision_digest.is_empty());

        // The revision participates in identity, so a future change to matching
        // or revocation semantics cannot masquerade as the same decision.
        let mut other = sealed.clone();
        other.decision_digest = String::new();
        other.evaluator_revision = AUTHORIZATION_EVALUATOR_REVISION + 1;
        let digest = crate::support::hashing::canonical_hash(&other);
        assert_ne!(digest, sealed.decision_digest);
    }

    #[test]
    fn authorization_is_scoped_to_one_capability() {
        // The same trusted artifact may be permitted one capability and refused
        // another: trust is not a blanket permission.
        let allowed = decision(
            ExtensionCapabilityKind::Verification,
            AuthorizationResult::Authorized,
        );
        let refused = decision(
            ExtensionCapabilityKind::ToolAction,
            AuthorizationResult::Refused {
                reason: "no grant covers tool actions for this artifact".into(),
            },
        );
        assert_eq!(
            allowed.artifact_attestation_digest,
            refused.artifact_attestation_digest
        );
        assert!(allowed.is_authorized());
        assert!(!refused.is_authorized());
        assert_ne!(allowed.decision_digest, refused.decision_digest);
    }

    #[test]
    fn a_decision_records_the_trust_fact_it_was_made_against() {
        let sealed = decision(
            ExtensionCapabilityKind::Comparison,
            AuthorizationResult::Authorized,
        );
        // Historical provenance can chain: attestation -> decision -> result,
        // without either record standing in for the other.
        assert_eq!(sealed.artifact_attestation_digest, "sha256:attestation");
        let encoded = serde_json::to_value(&sealed).unwrap();
        assert!(encoded.get("verification_result").is_none());
        assert!(encoded.get("publisher_identity").is_none());
    }
}
