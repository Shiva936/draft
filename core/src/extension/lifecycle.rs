//! The state Draft holds about an installed extension.

use crate::extension::provenance::InstalledExtensionProvenance;
use draft_extension_contract::ExtensionManifest;
use serde::{Deserialize, Serialize};

/// One installed extension artifact, exactly as Draft recorded it.
///
/// `content_hash` identifies the artifact, not the extension: reinstalling a
/// different build of the same version produces a different hash. Draft
/// re-derives it on every load, so a package edited underneath Draft stops
/// matching its own record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledExtension {
    pub manifest: ExtensionManifest,
    pub content_hash: String,
    pub enabled: bool,
    pub installed_at: String,
    pub provenance: InstalledExtensionProvenance,
}

impl InstalledExtension {
    /// The extension's canonical id.
    pub fn id(&self) -> &str {
        self.manifest.id.as_str()
    }

    /// The installed version.
    pub fn version(&self) -> &str {
        &self.manifest.version
    }

    /// The catalog source an update must resolve from, when there is one.
    pub fn update_source_id(&self) -> Option<&str> {
        self.provenance.catalog_source_id()
    }

    /// Whether this record still describes the artifact it claims to.
    ///
    /// The manifest, the provenance and the installed tree each carry the
    /// package identity; a record whose three copies disagree is not usable.
    pub fn describes_artifact(&self, observed_content_hash: &str) -> bool {
        self.content_hash == observed_content_hash
            && self.provenance.package_id == self.manifest.id.as_str()
            && self.provenance.package_version == self.manifest.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::provenance::ExtensionTrustProvenance;
    use draft_extension_contract::ExtensionId;
    use std::collections::BTreeMap;

    fn installed() -> InstalledExtension {
        InstalledExtension {
            manifest: ExtensionManifest {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::ExtensionManifest,
                ),
                id: ExtensionId::parse("draft.language.rust").unwrap(),
                name: "Rust".into(),
                version: "1.0.0".into(),
                schemas: Vec::new(),
                publisher: "draft".into(),
                draft_api: format!("^{}", crate::DRAFT_API_VERSION),
                contributions: vec![],
                permissions: vec![],
                description: None,
                keywords: vec![],
                documentation: vec![],
                licenses: vec![],
                assets: vec![],
            },
            content_hash: "sha256:artifact".into(),
            enabled: true,
            installed_at: "2026-01-01T00:00:00Z".into(),
            provenance: InstalledExtensionProvenance {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::InstalledExtensionProvenance,
                ),
                package_id: "draft.language.rust".into(),
                package_version: "1.0.0".into(),
                operation_id: "op_install".into(),
                trust: ExtensionTrustProvenance::HttpsCatalog {
                    source_id: "draft-official".into(),
                    catalog_id: "official".into(),
                    trusted_root_fingerprint: "sha256:root".into(),
                    signed_metadata_counters: BTreeMap::new(),
                    target_digest: "sha256:artifact".into(),
                    verified_at: "2026-01-01T00:00:00Z".into(),
                },
            },
        }
    }

    #[test]
    fn an_installed_record_exposes_its_identity_and_lineage() {
        let record = installed();
        assert_eq!(record.id(), "draft.language.rust");
        assert_eq!(record.version(), "1.0.0");
        assert_eq!(record.update_source_id(), Some("draft-official"));
    }

    #[test]
    fn a_record_whose_identity_copies_disagree_is_not_usable() {
        let record = installed();
        assert!(record.describes_artifact("sha256:artifact"));
        assert!(!record.describes_artifact("sha256:other"));

        let mut renamed = installed();
        renamed.provenance.package_id = "draft.language.python".into();
        assert!(!renamed.describes_artifact("sha256:artifact"));

        let mut reversioned = installed();
        reversioned.provenance.package_version = "2.0.0".into();
        assert!(!reversioned.describes_artifact("sha256:artifact"));
    }
}
