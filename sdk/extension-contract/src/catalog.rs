//! The signed extension catalog format.
//!
//! A catalog is a compact TUF-style chain: an explicitly bootstrapped root
//! authorizes timestamp, snapshot and targets roles, and snapshot metadata may
//! authenticate one level of namespace-restricted delegated targets. This
//! module owns the *documents* and the signature arithmetic over them —
//! everything a publisher needs to produce a catalog and everything a consumer
//! needs to check one role's signatures.
//!
//! It deliberately owns none of Draft's trust *policy*: version floors,
//! rollback and replay rejection, expiry handling, delegation escape rules,
//! caching and durable trust state stay in Draft, which is the party that has
//! to remember what it previously believed.

use crate::{FormatError, FormatResult};

/// The Ed25519 verification primitive, re-exported from the portable DCG
/// contract so a catalog signature and a receipt signature are checked by the
/// same code rather than by two implementations that could disagree.
pub use draft_dcg_contract::verify_signature;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The only signature algorithm the format admits.
pub const SIGNATURE_ALGORITHM: &str = "ed25519";

/// A detached signature by one named key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureRecord {
    pub key_id: String,
    pub signature: String,
}

/// Role metadata together with the signatures over its canonical form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedEnvelope<T> {
    pub signed: T,
    pub signatures: Vec<SignatureRecord>,
}

impl<T: Serialize> SignedEnvelope<T> {
    /// The exact bytes a signature over this role must be computed on.
    pub fn signable_bytes(&self) -> FormatResult<Vec<u8>> {
        signable_bytes(&self.signed)
    }
}

/// The exact bytes a signature over `document` must be computed on.
pub fn signable_bytes<T: Serialize>(document: &T) -> FormatResult<Vec<u8>> {
    Ok(crate::canonical::canonical_bytes(document)?)
}

/// A public key admitted by a catalog root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogKey {
    pub algorithm: String,
    pub public_key: String,
}

/// Which keys may sign a role, and how many of them must.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSpec {
    pub key_ids: Vec<String>,
    pub threshold: u32,
}

impl RoleSpec {
    /// A role spec is usable only if its key ids are unique and its threshold
    /// is satisfiable by them.
    pub fn validate(&self) -> FormatResult<()> {
        let unique: BTreeSet<_> = self.key_ids.iter().collect();
        if self.threshold == 0
            || self.threshold as usize > unique.len()
            || unique.len() != self.key_ids.len()
        {
            return Err(FormatError::Signature(
                "role key ids/threshold are invalid".into(),
            ));
        }
        Ok(())
    }
}

/// The catalog trust anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootMetadata {
    pub schema_version: u32,
    pub catalog_id: String,
    pub role: String,
    pub version: u64,
    pub expires_at: String,
    pub keys: BTreeMap<String, CatalogKey>,
    pub roles: BTreeMap<String, RoleSpec>,
    pub revoked_key_ids: Vec<String>,
    pub revoked_packages: Vec<String>,
}

/// The roles a root must always authorize.
pub const REQUIRED_ROLES: &[&str] = &["root", "timestamp", "snapshot", "targets"];

impl RootMetadata {
    /// Structural validity of a root document, independent of whether Draft
    /// currently trusts it.
    pub fn validate_shape(&self) -> FormatResult<()> {
        if self.schema_version != crate::FORMAT_REVISION {
            return Err(FormatError::Identity(format!(
                "catalog root schema_version {} is not the supported format revision {}",
                self.schema_version,
                crate::FORMAT_REVISION
            )));
        }
        if self.role != "root" {
            return Err(FormatError::Identity(format!(
                "catalog root declares role '{}'",
                self.role
            )));
        }
        if self.catalog_id.trim().is_empty() {
            return Err(FormatError::Identity(
                "catalog root must name its catalog".into(),
            ));
        }
        if self.version == 0 || self.keys.is_empty() {
            return Err(FormatError::Identity(
                "catalog root has an invalid version or empty key set".into(),
            ));
        }
        for role in REQUIRED_ROLES {
            let spec = self.roles.get(*role).ok_or_else(|| {
                FormatError::Identity(format!("catalog root is missing role '{role}'"))
            })?;
            spec.validate()?;
            if spec.key_ids.iter().any(|id| !self.keys.contains_key(id)) {
                return Err(FormatError::Identity(format!(
                    "role '{role}' references an unknown key"
                )));
            }
        }
        for package_id in &self.revoked_packages {
            crate::ExtensionId::parse(package_id.as_str())?;
        }
        Ok(())
    }

    /// Whether the root has revoked every version of `package_id`.
    pub fn revokes_package(&self, package_id: &str) -> bool {
        self.revoked_packages
            .iter()
            .any(|revoked| revoked == package_id)
    }
}

/// A hash-pinned pointer from one role to the next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataDescriptor {
    pub version: u64,
    pub length: u64,
    pub sha256: String,
}

impl MetadataDescriptor {
    /// Whether `bytes` are exactly the document this descriptor pins.
    pub fn matches(&self, bytes: &[u8]) -> bool {
        self.length == bytes.len() as u64 && self.sha256 == crate::package::digest(bytes)
    }
}

/// The short-lived role that pins the current snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimestampMetadata {
    pub schema_version: u32,
    pub catalog_id: String,
    pub role: String,
    pub version: u64,
    pub expires_at: String,
    pub snapshot: MetadataDescriptor,
}

/// The role that pins every targets document in one consistent set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotMetadata {
    pub schema_version: u32,
    pub catalog_id: String,
    pub role: String,
    pub version: u64,
    pub expires_at: String,
    pub roles: BTreeMap<String, MetadataDescriptor>,
}

/// A delegated targets role, restricted to a package-id namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delegation {
    pub role: String,
    pub keys: BTreeMap<String, CatalogKey>,
    pub key_ids: Vec<String>,
    pub threshold: u32,
    pub path_prefixes: Vec<String>,
}

impl Delegation {
    /// The role spec this delegation grants.
    pub fn role_spec(&self) -> RoleSpec {
        RoleSpec {
            key_ids: self.key_ids.clone(),
            threshold: self.threshold,
        }
    }

    /// Whether this delegation is permitted to publish `package_id`.
    pub fn covers(&self, package_id: &str) -> bool {
        self.path_prefixes
            .iter()
            .any(|prefix| package_id.starts_with(prefix))
    }
}

/// One published package version, as the catalog authenticates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogTarget {
    pub id: String,
    pub version: String,
    pub publisher: String,
    pub draft_api: String,
    pub artifact_path: String,
    pub length: u64,
    pub sha256: String,
    /// Display name, as the package's own manifest states it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Capabilities the package contributes, so a search can ask for what it
    /// needs rather than guessing from a name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

impl CatalogTarget {
    /// Every field a search matches against, lowercased.
    ///
    /// This metadata rides inside the signed targets role, so it is
    /// authenticated exactly as the artifact digest is: a search result cannot
    /// be steered by an unsigned description.
    pub fn searchable_text(&self) -> Vec<String> {
        std::iter::once(self.id.clone())
            .chain(self.name.clone())
            .chain(self.description.clone())
            .chain(std::iter::once(self.publisher.clone()))
            .chain(self.keywords.iter().cloned())
            .chain(self.capabilities.iter().cloned())
            .map(|value| value.to_ascii_lowercase())
            .collect()
    }

    /// Whether this package declares `capability`.
    pub fn provides(&self, capability: &str) -> bool {
        self.capabilities
            .iter()
            .any(|declared| declared.eq_ignore_ascii_case(capability))
    }
}

/// The role that publishes packages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetsMetadata {
    pub schema_version: u32,
    pub catalog_id: String,
    pub role: String,
    pub version: u64,
    pub expires_at: String,
    pub packages: Vec<CatalogTarget>,
    pub delegations: Vec<Delegation>,
}

/// Verify that an envelope carries enough unique, authorized, unrevoked
/// signatures over its canonical form to satisfy `role`.
///
/// Duplicate signatures by one key count once; unauthorized, revoked and
/// non-Ed25519 keys do not count at all.
pub fn verify_envelope<T: Serialize>(
    envelope: &SignedEnvelope<T>,
    role: &RoleSpec,
    keys: &BTreeMap<String, CatalogKey>,
    revoked_key_ids: &[String],
) -> FormatResult<()> {
    role.validate()?;
    let authorized: BTreeSet<&String> = role.key_ids.iter().collect();
    let revoked: BTreeSet<&String> = revoked_key_ids.iter().collect();
    let message = envelope.signable_bytes()?;

    let mut satisfied: BTreeSet<&String> = BTreeSet::new();
    for signature in &envelope.signatures {
        if satisfied.contains(&signature.key_id)
            || !authorized.contains(&signature.key_id)
            || revoked.contains(&signature.key_id)
        {
            continue;
        }
        let Some(key) = keys.get(&signature.key_id) else {
            continue;
        };
        if key.algorithm != SIGNATURE_ALGORITHM {
            continue;
        }
        if verify_signature(&key.public_key, &message, &signature.signature)? {
            satisfied.insert(&signature.key_id);
        }
    }

    if satisfied.len() < role.threshold as usize {
        return Err(FormatError::Signature(format!(
            "signed metadata has {} unique authorized signatures but threshold is {}",
            satisfied.len(),
            role.threshold
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    fn keypair(seed: u8) -> (SigningKey, CatalogKey) {
        let signing = SigningKey::from_bytes(&[seed; 32]);
        let public = CatalogKey {
            algorithm: SIGNATURE_ALGORITHM.into(),
            public_key: BASE64.encode(signing.verifying_key().to_bytes()),
        };
        (signing, public)
    }

    fn envelope(signers: &[(&str, &SigningKey)]) -> SignedEnvelope<serde_json::Value> {
        let signed = serde_json::json!({"catalog_id": "official", "version": 1});
        let message = signable_bytes(&signed).unwrap();
        SignedEnvelope {
            signatures: signers
                .iter()
                .map(|(key_id, signing)| SignatureRecord {
                    key_id: (*key_id).to_string(),
                    signature: BASE64.encode(signing.sign(&message).to_bytes()),
                })
                .collect(),
            signed,
        }
    }

    #[test]
    fn a_threshold_of_unique_authorized_signatures_is_required() {
        let (first_signing, first_key) = keypair(1);
        let (second_signing, second_key) = keypair(2);
        let keys = BTreeMap::from([
            ("first".to_string(), first_key),
            ("second".to_string(), second_key),
        ]);
        let role = RoleSpec {
            key_ids: vec!["first".into(), "second".into()],
            threshold: 2,
        };

        let both = envelope(&[("first", &first_signing), ("second", &second_signing)]);
        verify_envelope(&both, &role, &keys, &[]).unwrap();

        let one = envelope(&[("first", &first_signing)]);
        assert!(matches!(
            verify_envelope(&one, &role, &keys, &[]),
            Err(FormatError::Signature(_))
        ));
    }

    #[test]
    fn duplicate_signatures_by_one_key_count_once() {
        let (signing, key) = keypair(1);
        let keys = BTreeMap::from([("first".to_string(), key)]);
        let role = RoleSpec {
            key_ids: vec!["first".into()],
            threshold: 1,
        };
        let mut duplicated = envelope(&[("first", &signing)]);
        let repeat = duplicated.signatures[0].clone();
        duplicated.signatures.push(repeat);

        // Still one unique key, so a threshold of one passes and a threshold of
        // two cannot be forged by repetition.
        verify_envelope(&duplicated, &role, &keys, &[]).unwrap();
        let stricter = RoleSpec {
            key_ids: vec!["first".into()],
            threshold: 2,
        };
        assert!(verify_envelope(&duplicated, &stricter, &keys, &[]).is_err());
    }

    #[test]
    fn revoked_and_unauthorized_keys_do_not_count() {
        let (signing, key) = keypair(1);
        let keys = BTreeMap::from([("first".to_string(), key)]);
        let role = RoleSpec {
            key_ids: vec!["first".into()],
            threshold: 1,
        };
        let signed = envelope(&[("first", &signing)]);

        assert!(verify_envelope(&signed, &role, &keys, &["first".to_string()]).is_err());

        let unrelated = RoleSpec {
            key_ids: vec!["other".into()],
            threshold: 1,
        };
        assert!(verify_envelope(&signed, &unrelated, &keys, &[]).is_err());
    }

    #[test]
    fn tampering_with_the_signed_document_invalidates_it() {
        let (signing, key) = keypair(1);
        let keys = BTreeMap::from([("first".to_string(), key)]);
        let role = RoleSpec {
            key_ids: vec!["first".into()],
            threshold: 1,
        };
        let mut tampered = envelope(&[("first", &signing)]);
        tampered.signed = serde_json::json!({"catalog_id": "official", "version": 2});
        assert!(verify_envelope(&tampered, &role, &keys, &[]).is_err());
    }

    #[test]
    fn a_non_ed25519_key_never_satisfies_a_role() {
        let (signing, mut key) = keypair(1);
        key.algorithm = "rsa".into();
        let keys = BTreeMap::from([("first".to_string(), key)]);
        let role = RoleSpec {
            key_ids: vec!["first".into()],
            threshold: 1,
        };
        assert!(verify_envelope(&envelope(&[("first", &signing)]), &role, &keys, &[]).is_err());
    }

    #[test]
    fn root_shape_requires_every_role_and_known_keys() {
        let (_, key) = keypair(1);
        let spec = RoleSpec {
            key_ids: vec!["first".into()],
            threshold: 1,
        };
        let mut root = RootMetadata {
            schema_version: crate::FORMAT_REVISION,
            catalog_id: "official".into(),
            role: "root".into(),
            version: 1,
            expires_at: "2030-01-01T00:00:00Z".into(),
            keys: BTreeMap::from([("first".to_string(), key)]),
            roles: REQUIRED_ROLES
                .iter()
                .map(|role| ((*role).to_string(), spec.clone()))
                .collect(),
            revoked_key_ids: vec![],
            revoked_packages: vec![],
        };
        root.validate_shape().unwrap();

        let mut missing = root.clone();
        missing.roles.remove("targets");
        assert!(missing.validate_shape().is_err());

        let mut unknown_key = root.clone();
        unknown_key.roles.insert(
            "targets".into(),
            RoleSpec {
                key_ids: vec!["absent".into()],
                threshold: 1,
            },
        );
        assert!(unknown_key.validate_shape().is_err());

        root.revoked_packages = vec!["Not An Id".into()];
        assert!(root.validate_shape().is_err());
    }

    #[test]
    fn descriptors_pin_length_and_digest() {
        let bytes = b"catalog document";
        let descriptor = MetadataDescriptor {
            version: 1,
            length: bytes.len() as u64,
            sha256: crate::package::digest(bytes),
        };
        assert!(descriptor.matches(bytes));
        assert!(!descriptor.matches(b"catalog documenT"));
        assert!(!descriptor.matches(b"short"));
    }

    #[test]
    fn delegations_only_cover_their_namespace() {
        let delegation = Delegation {
            role: "community".into(),
            keys: BTreeMap::new(),
            key_ids: vec!["k".into()],
            threshold: 1,
            path_prefixes: vec!["community.".into()],
        };
        assert!(delegation.covers("community.tool"));
        assert!(!delegation.covers("draft.language.rust"));
    }
}
