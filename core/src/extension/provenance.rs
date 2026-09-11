//! Where an installed extension came from.
//!
//! Provenance is Draft-operational, not portable: it records what *this*
//! installation decided to trust and when. It is durable and append-only in
//! spirit — removing a catalog source must never erase the record of how an
//! already-installed package was acquired, because that record is the audit
//! trail and the lineage a later update has to follow.

use crate::contracts::{ContractId, VersionedContract};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The trust decision that authorized one installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source_kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtensionTrustProvenance {
    /// A verified HTTPS catalog chain.
    HttpsCatalog {
        source_id: String,
        catalog_id: String,
        trusted_root_fingerprint: String,
        signed_metadata_counters: BTreeMap<String, u64>,
        target_digest: String,
        verified_at: String,
    },
    /// A local-directory catalog the user explicitly trusted.
    TrustedLocalCatalog {
        source_id: String,
        catalog_id: String,
        local_source: String,
        trust_root_id: String,
        trust_decision_id: String,
        signed_metadata_counters: BTreeMap<String, u64>,
        target_digest: String,
        verified_at: String,
    },
    /// A directory the user pointed at directly, with no catalog in between.
    DirectLocal {
        decision_id: String,
        actor_id: String,
        decided_at: String,
        source_description: String,
        artifact_digest: String,
    },
}

/// What Draft durably records about the acquisition of one installed package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledExtensionProvenance {
    pub schema_version: u32,
    pub package_id: String,
    pub package_version: String,
    pub operation_id: String,
    pub trust: ExtensionTrustProvenance,
}

impl VersionedContract for InstalledExtensionProvenance {
    const CONTRACT: ContractId = ContractId::InstalledExtensionProvenance;
}

/// The historical trust fact recorded when one artifact was accepted.
///
/// This is provenance, not policy. Later trust decisions never rewrite it: a
/// rotated signing key does not make evidence produced under the old key
/// unverifiable, uninstalling the package does not destroy the record, and an
/// artifact installed without publisher verification records that weaker
/// provenance permanently rather than being upgraded later.
///
/// Deliberately absent: any grant, permission or authorization state. *What this
/// artifact is* and *what it was allowed to do* are separate historical
/// questions, and answering them with one record would make a revocation look
/// like it retroactively changed what was trusted. See
/// [`super::authorization::AuthorizationDecision`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactAttestation {
    pub schema_version: u32,
    pub extension_id: String,
    pub extension_version: String,
    pub manifest_digest: String,
    pub package_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing_key_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_signature_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_identity_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_root_fingerprint: Option<String>,
    pub verification_result: ArtifactVerification,
    pub verified_at: String,
    pub attestation_digest: String,
}

impl VersionedContract for ArtifactAttestation {
    const CONTRACT: ContractId = ContractId::ArtifactAttestation;
}

/// How thoroughly an artifact's origin was established.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactVerification {
    /// A signature chain was verified against a trusted root.
    Verified { role_thresholds_met: Vec<String> },
    /// Installed from a location the user pointed at, with no publisher
    /// verification. Permanent: this is never upgraded to `Verified` later.
    LocallyInstalledUnverified { reason: String },
}

impl ArtifactVerification {
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified { .. })
    }
}

impl ArtifactAttestation {
    /// Seal an attestation, deriving its content-addressed identity.
    pub fn seal(mut self) -> Self {
        self.schema_version = crate::contracts::current_version(ContractId::ArtifactAttestation);
        self.attestation_digest = String::new();
        let digest = crate::support::hashing::canonical_hash(&self);
        self.attestation_digest = digest;
        self
    }

    /// Whether this record still describes the bytes it claims to.
    pub fn matches(&self, package_digest: &str) -> bool {
        self.package_digest == package_digest
    }
}

/// A reference to the artifact that produced a derived result.
///
/// Defined in [`crate::contracts`], below every domain that records one, and
/// re-exported here because this is where an extension's own provenance is
/// assembled. One canonical value, one Rust type.
pub use crate::contracts::ProducerRef;

#[cfg(test)]
mod attestation_tests {
    use super::*;

    fn attestation(verification: ArtifactVerification) -> ArtifactAttestation {
        ArtifactAttestation {
            schema_version: 0,
            extension_id: "example.pub".into(),
            extension_version: "1.0.0".into(),
            manifest_digest: "sha256:manifest".into(),
            package_digest: "sha256:package".into(),
            publisher_identity: Some("example".into()),
            signing_key_fingerprint: Some("fp".into()),
            package_signature_digest: Some("sha256:sig".into()),
            source_id: Some("official".into()),
            catalog_identity_digest: Some("sha256:catalog".into()),
            trust_root_fingerprint: Some("root-fp".into()),
            verification_result: verification,
            verified_at: "2026-01-01T00:00:00Z".into(),
            attestation_digest: String::new(),
        }
        .seal()
    }

    #[test]
    fn an_attestation_carries_no_grant_or_authorization_state() {
        let encoded = serde_json::to_value(attestation(ArtifactVerification::Verified {
            role_thresholds_met: vec!["targets".into()],
        }))
        .unwrap();
        let fields: Vec<&str> = encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for forbidden in [
            "grant",
            "grant_reference",
            "authorization",
            "authorization_reference",
            "permissions",
            "required_permissions",
        ] {
            assert!(
                !fields.contains(&forbidden),
                "trust attestation must not carry {forbidden}: authorization is a separate fact"
            );
        }
    }

    #[test]
    fn attestation_identity_is_content_addressed_and_stable() {
        let first = attestation(ArtifactVerification::Verified {
            role_thresholds_met: vec!["targets".into()],
        });
        let again = attestation(ArtifactVerification::Verified {
            role_thresholds_met: vec!["targets".into()],
        });
        assert_eq!(first.attestation_digest, again.attestation_digest);
        assert!(!first.attestation_digest.is_empty());

        let unverified = attestation(ArtifactVerification::LocallyInstalledUnverified {
            reason: "installed from a local directory".into(),
        });
        // Weaker provenance is a different historical fact, not the same one.
        assert_ne!(first.attestation_digest, unverified.attestation_digest);
        assert!(!unverified.verification_result.is_verified());
    }

    #[test]
    fn a_producer_reference_points_at_its_attestation() {
        let attested = attestation(ArtifactVerification::Verified {
            role_thresholds_met: vec!["targets".into()],
        });
        let producer = ProducerRef {
            extension_id: attested.extension_id.clone(),
            extension_version: attested.extension_version.clone(),
            package_digest: attested.package_digest.clone(),
            attestation_digest: attested.attestation_digest.clone(),
        };
        assert!(attested.matches(&producer.package_digest));
        assert!(!attested.matches("sha256:something-else"));
    }
}

impl InstalledExtensionProvenance {
    /// The digest of the artifact this installation authorized.
    pub fn artifact_digest(&self) -> &str {
        match &self.trust {
            ExtensionTrustProvenance::HttpsCatalog { target_digest, .. }
            | ExtensionTrustProvenance::TrustedLocalCatalog { target_digest, .. } => target_digest,
            ExtensionTrustProvenance::DirectLocal {
                artifact_digest, ..
            } => artifact_digest,
        }
    }

    /// The catalog source this package was acquired from, when there was one.
    ///
    /// This is the update lineage: an update resolves from the source recorded
    /// here, never opportunistically from another source publishing the same
    /// extension id.
    pub fn catalog_source_id(&self) -> Option<&str> {
        match &self.trust {
            ExtensionTrustProvenance::HttpsCatalog { source_id, .. }
            | ExtensionTrustProvenance::TrustedLocalCatalog { source_id, .. } => Some(source_id),
            ExtensionTrustProvenance::DirectLocal { .. } => None,
        }
    }

    /// The catalog identity this package was acquired from, when there was one.
    pub fn catalog_id(&self) -> Option<&str> {
        match &self.trust {
            ExtensionTrustProvenance::HttpsCatalog { catalog_id, .. }
            | ExtensionTrustProvenance::TrustedLocalCatalog { catalog_id, .. } => Some(catalog_id),
            ExtensionTrustProvenance::DirectLocal { .. } => None,
        }
    }

    /// The immutable trust fact this installation established.
    ///
    /// Derived from the install record rather than stored beside it, so the two
    /// cannot drift. Note what it does not contain: no grant, no permission, no
    /// authorization state. What this artifact *is* and what it was *allowed to
    /// do* are separate historical questions, and a revocation must never look
    /// like it retroactively changed what was trusted.
    pub fn attestation(&self, manifest_digest: &str) -> ArtifactAttestation {
        ArtifactAttestation {
            schema_version: crate::contracts::current_version(ContractId::ArtifactAttestation),
            extension_id: self.package_id.clone(),
            extension_version: self.package_version.clone(),
            manifest_digest: manifest_digest.to_string(),
            package_digest: self.artifact_digest().to_string(),
            publisher_identity: None,
            signing_key_fingerprint: self.trust_root_fingerprint().map(ToString::to_string),
            package_signature_digest: None,
            source_id: self.catalog_source_id().map(ToString::to_string),
            catalog_identity_digest: self.catalog_id().map(ToString::to_string),
            trust_root_fingerprint: self.trust_root_fingerprint().map(ToString::to_string),
            verification_result: match &self.trust {
                ExtensionTrustProvenance::HttpsCatalog { .. }
                | ExtensionTrustProvenance::TrustedLocalCatalog { .. } => {
                    ArtifactVerification::Verified {
                        role_thresholds_met: vec!["targets".to_string()],
                    }
                }
                // Installed from a location the user pointed at, with no
                // publisher verification. Recorded permanently as the weaker
                // provenance it is, and never upgraded later.
                ExtensionTrustProvenance::DirectLocal {
                    source_description, ..
                } => ArtifactVerification::LocallyInstalledUnverified {
                    reason: format!("installed directly from {source_description}"),
                },
            },
            verified_at: self.verified_at().to_string(),
            attestation_digest: String::new(),
        }
        .seal()
    }

    /// The trust root this package's signature chained to, when there was one.
    pub fn trust_root_fingerprint(&self) -> Option<&str> {
        match &self.trust {
            ExtensionTrustProvenance::HttpsCatalog {
                trusted_root_fingerprint,
                ..
            } => Some(trusted_root_fingerprint),
            ExtensionTrustProvenance::TrustedLocalCatalog { trust_root_id, .. } => {
                Some(trust_root_id)
            }
            ExtensionTrustProvenance::DirectLocal { .. } => None,
        }
    }

    /// When this artifact's origin was established.
    pub fn verified_at(&self) -> &str {
        match &self.trust {
            ExtensionTrustProvenance::HttpsCatalog { verified_at, .. }
            | ExtensionTrustProvenance::TrustedLocalCatalog { verified_at, .. } => verified_at,
            ExtensionTrustProvenance::DirectLocal { decided_at, .. } => decided_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance(trust: ExtensionTrustProvenance) -> InstalledExtensionProvenance {
        InstalledExtensionProvenance {
            schema_version: crate::contracts::current_version(
                ContractId::InstalledExtensionProvenance,
            ),
            package_id: "draft.language.rust".into(),
            package_version: "1.0.0".into(),
            operation_id: "op_install".into(),
            trust,
        }
    }

    #[test]
    fn catalog_installs_carry_their_update_lineage() {
        let record = provenance(ExtensionTrustProvenance::HttpsCatalog {
            source_id: "draft-official".into(),
            catalog_id: "official".into(),
            trusted_root_fingerprint: "sha256:root".into(),
            signed_metadata_counters: BTreeMap::new(),
            target_digest: "sha256:target".into(),
            verified_at: "2026-01-01T00:00:00Z".into(),
        });
        assert_eq!(record.catalog_source_id(), Some("draft-official"));
        assert_eq!(record.catalog_id(), Some("official"));
        assert_eq!(record.artifact_digest(), "sha256:target");
    }

    #[test]
    fn direct_local_installs_have_no_lineage_to_update_from() {
        let record = provenance(ExtensionTrustProvenance::DirectLocal {
            decision_id: "dec_1".into(),
            actor_id: "act_1".into(),
            decided_at: "2026-01-01T00:00:00Z".into(),
            source_description: "/tmp/package".into(),
            artifact_digest: "sha256:artifact".into(),
        });
        assert_eq!(record.catalog_source_id(), None);
        assert_eq!(record.catalog_id(), None);
        assert_eq!(record.artifact_digest(), "sha256:artifact");
    }

    #[test]
    fn the_trust_discriminant_is_stable_on_the_wire() {
        let record = provenance(ExtensionTrustProvenance::TrustedLocalCatalog {
            source_id: "devcatalog".into(),
            catalog_id: "dev".into(),
            local_source: "/srv/catalog".into(),
            trust_root_id: "root_1".into(),
            trust_decision_id: "dec_1".into(),
            signed_metadata_counters: BTreeMap::from([("targets".into(), 3)]),
            target_digest: "sha256:target".into(),
            verified_at: "2026-01-01T00:00:00Z".into(),
        });
        let encoded = serde_json::to_value(&record).unwrap();
        assert_eq!(encoded["trust"]["source_kind"], "trusted_local_catalog");
        let decoded: InstalledExtensionProvenance = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, record);
    }
}
