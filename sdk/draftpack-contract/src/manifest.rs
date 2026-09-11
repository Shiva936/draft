//! The DraftPack manifest, its canonical entry metadata, and verification.
//!
//! A DraftPack carries an accepted Baseline out of one Draft installation so
//! another can verify it. The manifest is what makes that possible without
//! trusting the sender: it lists every member with its exact digest, so the
//! recipient can check the bytes it actually received against what the exporter
//! said it was sending.
//!
//! # Why entry digests rather than one archive digest
//!
//! A single digest over the whole archive proves the bytes are unchanged in
//! transit, and nothing else. Per-entry digests additionally say *which*
//! member is wrong when one is, let a recipient verify members it cares about
//! without materializing the rest, and — because the entry list is part of the
//! manifest — make a member that was silently added or removed a manifest
//! mismatch rather than an unnoticed difference.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use draft_dcg_contract::digest::canonical_digest;
use draft_dcg_contract::ids::{ActorId, ProjectId};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::{BaselineId, Digest, ProducerIdentity, ReceiptEnvelope};

use crate::path::SafeEntryPath;
use crate::{limits, FormatError, FormatResult, DRAFTPACK_FORMAT_REVISION};

/// The frozen domain separator for a DraftPack manifest digest.
pub const DRAFTPACK_MANIFEST_DIGEST_DOMAIN: &str = "draft.draftpack.manifest/v1";

/// The manifest member every DraftPack carries.
pub const MANIFEST_ENTRY_PATH: &str = "draftpack.json";

/// The media type a DraftPack is served as.
pub const DRAFTPACK_MEDIA_TYPE: &str = "application/vnd.draft.draftpack";

/// What the manifest says about one archive member.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveEntryMetadata {
    pub path: SafeEntryPath,
    pub size: u64,
    pub digest: Digest,
}

impl ArchiveEntryMetadata {
    /// Describe `content` at `path`.
    pub fn describe(path: SafeEntryPath, content: &[u8]) -> FormatResult<Self> {
        let size = content.len() as u64;
        if !limits::entry_size_permitted(size) {
            return Err(FormatError::Limit(format!(
                "entry '{path}' is {size} bytes, over the {} byte limit",
                limits::MAX_ENTRY_BYTES
            )));
        }
        Ok(Self {
            path,
            size,
            digest: Digest::of_bytes(content),
        })
    }

    /// Whether `content` is exactly what this entry describes.
    pub fn matches(&self, content: &[u8]) -> bool {
        content.len() as u64 == self.size && Digest::of_bytes(content) == self.digest
    }
}

/// The canonical digest of a [`DraftpackManifest`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DraftpackManifestDigest(Digest);

impl DraftpackManifestDigest {
    pub fn new(digest: Digest) -> Self {
        Self(digest)
    }

    pub fn digest(&self) -> &Digest {
        &self.0
    }
}

impl std::fmt::Display for DraftpackManifestDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// What a DraftPack contains and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftpackManifest {
    pub format_revision: u32,
    /// The project the exported Baseline belongs to.
    pub project: ProjectId,
    /// The exact accepted Baseline this pack carries.
    pub baseline: BaselineId,
    /// Every member, canonically ordered by path.
    pub entries: BTreeSet<ArchiveEntryMetadata>,
    /// Receipts travelling with the pack.
    ///
    /// Embedded rather than referenced, so a recipient can check the
    /// attestations without contacting the exporter. They are evidence of what
    /// was attested, never a grant of local trust: whether the signing key is
    /// trusted *here* is the importer's question, not the archive's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receipts: Vec<ReceiptEnvelope>,
    pub exported_by: ActorId,
    pub exported_at: Timestamp,
    pub producer: ProducerIdentity,
}

/// What verification found, in enough detail to say what is wrong.
///
/// Deliberately not a bool: "this archive is invalid" is not an actionable
/// answer, and an importer needs to tell a missing member from a corrupted one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationReport {
    /// Members the manifest lists that the archive does not contain.
    pub missing: BTreeSet<SafeEntryPath>,
    /// Members the archive contains that the manifest does not list.
    pub unexpected: BTreeSet<SafeEntryPath>,
    /// Members present in both whose bytes do not match the manifest.
    pub corrupted: BTreeSet<SafeEntryPath>,
}

impl VerificationReport {
    /// Whether the archive is exactly what the manifest describes.
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.unexpected.is_empty() && self.corrupted.is_empty()
    }
}

impl std::fmt::Display for VerificationReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_complete() {
            return formatter.write_str("archive matches its manifest");
        }
        let mut parts = Vec::new();
        if !self.missing.is_empty() {
            parts.push(format!("{} missing", self.missing.len()));
        }
        if !self.unexpected.is_empty() {
            parts.push(format!("{} unexpected", self.unexpected.len()));
        }
        if !self.corrupted.is_empty() {
            parts.push(format!("{} corrupted", self.corrupted.len()));
        }
        formatter.write_str(&parts.join(", "))
    }
}

impl DraftpackManifest {
    /// Validate the manifest on its own terms.
    pub fn validate(&self) -> FormatResult<()> {
        if self.format_revision != DRAFTPACK_FORMAT_REVISION {
            return Err(FormatError::Identity(format!(
                "DraftPack declares format revision {} but this build implements {}",
                self.format_revision, DRAFTPACK_FORMAT_REVISION
            )));
        }
        if self.entries.len() > limits::MAX_ENTRIES {
            return Err(FormatError::Limit(format!(
                "DraftPack lists {} entries, over the {} entry limit",
                self.entries.len(),
                limits::MAX_ENTRIES
            )));
        }

        let mut total: u64 = 0;
        let mut seen: BTreeSet<&SafeEntryPath> = BTreeSet::new();
        for entry in &self.entries {
            if !limits::entry_size_permitted(entry.size) {
                return Err(FormatError::Limit(format!(
                    "entry '{}' is {} bytes, over the {} byte limit",
                    entry.path,
                    entry.size,
                    limits::MAX_ENTRY_BYTES
                )));
            }
            if !limits::total_size_permitted(total, entry.size) {
                return Err(FormatError::Limit(format!(
                    "DraftPack exceeds the {} byte archive limit",
                    limits::MAX_TOTAL_BYTES
                )));
            }
            total = total.saturating_add(entry.size);
            // The set is ordered by (path, size, digest), so two entries can
            // share a path while differing elsewhere. That would make the
            // archive's content ambiguous, so it is refused.
            if !seen.insert(&entry.path) {
                return Err(FormatError::Identity(format!(
                    "DraftPack lists entry '{}' more than once",
                    entry.path
                )));
            }
        }
        Ok(())
    }

    /// This manifest's canonical digest.
    pub fn digest(&self) -> FormatResult<DraftpackManifestDigest> {
        self.validate()?;
        Ok(DraftpackManifestDigest(canonical_digest(
            DRAFTPACK_MANIFEST_DIGEST_DOMAIN,
            self,
        )?))
    }

    /// Compare the archive actually received against what this manifest says.
    ///
    /// The manifest member itself is excluded from `contents`, because it
    /// cannot describe its own digest.
    pub fn verify(&self, contents: &BTreeMap<SafeEntryPath, Vec<u8>>) -> VerificationReport {
        let mut missing = BTreeSet::new();
        let mut corrupted = BTreeSet::new();
        let mut listed = BTreeSet::new();

        for entry in &self.entries {
            listed.insert(entry.path.clone());
            match contents.get(&entry.path) {
                None => {
                    missing.insert(entry.path.clone());
                }
                Some(content) if !entry.matches(content) => {
                    corrupted.insert(entry.path.clone());
                }
                Some(_) => {}
            }
        }

        let unexpected = contents
            .keys()
            .filter(|path| !listed.contains(*path) && path.as_str() != MANIFEST_ENTRY_PATH)
            .cloned()
            .collect();

        VerificationReport {
            missing,
            unexpected,
            corrupted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::identifier::NamespacedId;

    fn path(value: &str) -> SafeEntryPath {
        SafeEntryPath::parse(value).unwrap()
    }

    fn manifest_with(entries: BTreeSet<ArchiveEntryMetadata>) -> DraftpackManifest {
        DraftpackManifest {
            format_revision: DRAFTPACK_FORMAT_REVISION,
            project: ProjectId::parse("prj_a1").unwrap(),
            baseline: BaselineId::new(Digest::of_bytes(b"baseline")),
            entries,
            receipts: Vec::new(),
            exported_by: ActorId::parse("act_a1").unwrap(),
            exported_at: Timestamp::from_unix_nanos(1_000),
            producer: ProducerIdentity::new(
                NamespacedId::parse("draft.core/draftpack").unwrap(),
                "0.3.4",
            )
            .unwrap(),
        }
    }

    fn manifest() -> DraftpackManifest {
        manifest_with(BTreeSet::from([
            ArchiveEntryMetadata::describe(path("baseline/manifest.json"), b"{}").unwrap(),
            ArchiveEntryMetadata::describe(path("objects/blake3/aa"), b"payload").unwrap(),
        ]))
    }

    fn contents() -> BTreeMap<SafeEntryPath, Vec<u8>> {
        BTreeMap::from([
            (path("baseline/manifest.json"), b"{}".to_vec()),
            (path("objects/blake3/aa"), b"payload".to_vec()),
        ])
    }

    #[test]
    fn a_faithful_archive_verifies() {
        let report = manifest().verify(&contents());
        assert!(report.is_complete(), "{report}");
        assert_eq!(report.to_string(), "archive matches its manifest");
    }

    #[test]
    fn a_corrupted_member_is_named_rather_than_summarised() {
        let mut tampered = contents();
        tampered.insert(path("objects/blake3/aa"), b"tampered".to_vec());
        let report = manifest().verify(&tampered);
        assert!(!report.is_complete());
        assert_eq!(
            report.corrupted,
            BTreeSet::from([path("objects/blake3/aa")])
        );
        assert!(report.missing.is_empty() && report.unexpected.is_empty());
    }

    #[test]
    fn a_removed_member_is_missing_not_merely_invalid() {
        let mut truncated = contents();
        truncated.remove(&path("objects/blake3/aa"));
        let report = manifest().verify(&truncated);
        assert_eq!(report.missing, BTreeSet::from([path("objects/blake3/aa")]));
        assert!(report.corrupted.is_empty());
    }

    #[test]
    fn a_smuggled_member_is_reported_as_unexpected() {
        // The attack this catches: an archive carrying a file the manifest
        // never mentioned, which a manifest-only check would let through.
        let mut smuggled = contents();
        smuggled.insert(path("evil/payload.sh"), b"rm -rf".to_vec());
        let report = manifest().verify(&smuggled);
        assert_eq!(report.unexpected, BTreeSet::from([path("evil/payload.sh")]));
        assert!(!report.is_complete());
    }

    #[test]
    fn the_manifest_member_is_not_itself_unexpected() {
        let mut with_manifest = contents();
        with_manifest.insert(path(MANIFEST_ENTRY_PATH), b"{}".to_vec());
        assert!(manifest().verify(&with_manifest).is_complete());
    }

    #[test]
    fn a_member_of_the_right_length_but_wrong_bytes_is_caught() {
        // Same size, different content: only the digest separates them.
        let mut swapped = contents();
        swapped.insert(path("objects/blake3/aa"), b"paylOad".to_vec());
        assert_eq!(
            manifest().verify(&swapped).corrupted,
            BTreeSet::from([path("objects/blake3/aa")])
        );
    }

    #[test]
    fn a_foreign_format_revision_is_refused() {
        let mut future = manifest();
        future.format_revision = 2;
        assert!(matches!(future.validate(), Err(FormatError::Identity(_))));
        assert!(future.digest().is_err());
    }

    #[test]
    fn an_oversized_entry_is_refused() {
        let oversized = ArchiveEntryMetadata {
            path: path("huge"),
            size: limits::MAX_ENTRY_BYTES + 1,
            digest: Digest::of_bytes(b"x"),
        };
        let manifest = manifest_with(BTreeSet::from([oversized]));
        assert!(matches!(manifest.validate(), Err(FormatError::Limit(_))));
    }

    #[test]
    fn an_archive_over_the_total_budget_is_refused() {
        let entries = (0..8)
            .map(|index| ArchiveEntryMetadata {
                path: path(&format!("part-{index}")),
                size: limits::MAX_ENTRY_BYTES,
                digest: Digest::of_bytes(&[index as u8]),
            })
            .collect();
        assert!(matches!(
            manifest_with(entries).validate(),
            Err(FormatError::Limit(_))
        ));
    }

    #[test]
    fn one_path_may_not_be_listed_twice() {
        // Two entries differing only in digest sort as distinct set members,
        // so the set alone does not prevent this. The archive's content would
        // otherwise be ambiguous.
        let entries = BTreeSet::from([
            ArchiveEntryMetadata::describe(path("dup"), b"one").unwrap(),
            ArchiveEntryMetadata::describe(path("dup"), b"two").unwrap(),
        ]);
        assert_eq!(entries.len(), 2);
        assert!(matches!(
            manifest_with(entries).validate(),
            Err(FormatError::Identity(_))
        ));
    }

    #[test]
    fn entry_order_does_not_affect_manifest_identity() {
        let forward = manifest().digest().unwrap();
        let mut reversed_entries: Vec<_> = manifest().entries.into_iter().collect();
        reversed_entries.reverse();
        let reversed = manifest_with(reversed_entries.into_iter().collect())
            .digest()
            .unwrap();
        assert_eq!(forward, reversed);
    }

    #[test]
    fn changing_any_member_changes_manifest_identity() {
        let base = manifest().digest().unwrap();
        let altered = manifest_with(BTreeSet::from([
            ArchiveEntryMetadata::describe(path("baseline/manifest.json"), b"{}").unwrap(),
            ArchiveEntryMetadata::describe(path("objects/blake3/aa"), b"different").unwrap(),
        ]))
        .digest()
        .unwrap();
        assert_ne!(base, altered);
    }

    #[test]
    fn the_wire_form_round_trips_and_validates_paths() {
        let manifest = manifest();
        let encoded = serde_json::to_string(&manifest).unwrap();
        assert_eq!(
            serde_json::from_str::<DraftpackManifest>(&encoded).unwrap(),
            manifest
        );
        // An unsafe path cannot enter through deserialization either.
        let attacked = encoded.replace("objects/blake3/aa", "../../etc/passwd");
        assert!(serde_json::from_str::<DraftpackManifest>(&attacked).is_err());
    }
}
