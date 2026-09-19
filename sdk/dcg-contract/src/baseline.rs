//! The accepted historical node.
//!
//! A [`BaselineId`] is the digest of a [`BaselineManifest`], and the manifest
//! is deliberately small: three roots and a single parent. Everything that
//! affects Baseline identity is reachable from those roots, and everything that
//! is not reachable from them does not affect it.
//!
//! # What that rule actually means for timestamps
//!
//! The rule is **reachability from the manifest**, never the datatype — so
//! "timestamps are excluded from `BaselineId`" is simply false and is never
//! stated here.
//!
//! * Changing only `Observation.observed_at` leaves `ProjectStateRoot`
//!   untouched, because material state does not contain observation timing —
//!   but it changes the `ObservationDigest`, hence `StateEvidenceRoot`, hence
//!   `BaselineId`. That is correct: same state, different accepted evidence.
//! * `BaselineRecord.accepted_at`, the accepting actor, activity timing,
//!   receipt and event identifiers and publication status all live *outside*
//!   the manifest and its roots, so they never affect `BaselineId`.
//!
//! Provenance timestamps are never stripped from canonical provenance objects
//! to make a tidier sentence true.
//!
//! # Single parent
//!
//! One `parent_baseline_id`, or `None` for the initial Baseline. Lineage is a
//! chain, not a merge graph: an accepted state has exactly one predecessor it
//! was promoted from.

use serde::{Deserialize, Serialize};

use crate::digest::{canonical_digest, Digest};
use crate::ids::ProjectId;
use crate::roots::{CoverageEvidenceRoot, ProjectStateRoot, StateEvidenceRoot};
use crate::{FormatError, FormatResult, DCG_FORMAT_REVISION};

/// The frozen domain separator for a baseline manifest digest.
pub const BASELINE_MANIFEST_DIGEST_DOMAIN: &str = "draft.dcg.baseline-manifest/v1";

/// The identity of one accepted Baseline: the digest of its manifest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BaselineId(Digest);

impl BaselineId {
    pub fn new(digest: Digest) -> Self {
        Self(digest)
    }

    pub fn digest(&self) -> &Digest {
        &self.0
    }
}

impl std::fmt::Display for BaselineId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Everything that makes one accepted historical node what it is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineManifest {
    pub project: ProjectId,
    /// What material state is accepted.
    pub project_state_root: ProjectStateRoot,
    /// What exact provenance establishes it.
    pub state_evidence_root: StateEvidenceRoot,
    /// What exact coverage claims justify absence and completeness.
    pub coverage_evidence_root: CoverageEvidenceRoot,
    /// The single Baseline this was promoted from, or `None` for the initial
    /// Baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_baseline_id: Option<BaselineId>,
    /// The DCG format revision this manifest is written in.
    pub format_revision: u32,
}

impl BaselineManifest {
    /// Validate the manifest before trusting its identity.
    pub fn validate(&self) -> FormatResult<()> {
        if self.format_revision != DCG_FORMAT_REVISION {
            return Err(FormatError::Identity(format!(
                "baseline manifest declares format revision {} but this build implements {}",
                self.format_revision, DCG_FORMAT_REVISION
            )));
        }
        Ok(())
    }

    /// This manifest's identity.
    pub fn baseline_id(&self) -> FormatResult<BaselineId> {
        self.validate()?;
        Ok(BaselineId(canonical_digest(
            BASELINE_MANIFEST_DIGEST_DOMAIN,
            self,
        )?))
    }

    /// Whether this is the initial Baseline of its project.
    pub fn is_initial(&self) -> bool {
        self.parent_baseline_id.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(seed: &[u8]) -> ProjectStateRoot {
        ProjectStateRoot::new(Digest::of_bytes(seed))
    }

    fn manifest() -> BaselineManifest {
        BaselineManifest {
            project: ProjectId::parse("prj_a1b2c3").unwrap(),
            project_state_root: root(b"state"),
            state_evidence_root: StateEvidenceRoot::new(Digest::of_bytes(b"evidence")),
            coverage_evidence_root: CoverageEvidenceRoot::new(Digest::of_bytes(b"coverage")),
            parent_baseline_id: None,
            format_revision: DCG_FORMAT_REVISION,
        }
    }

    #[test]
    fn the_initial_baseline_has_no_parent() {
        assert!(manifest().is_initial());
        manifest().baseline_id().unwrap();
    }

    #[test]
    fn every_root_participates_in_identity() {
        let base = manifest().baseline_id().unwrap();

        let mut changed = manifest();
        changed.project_state_root = root(b"other-state");
        assert_ne!(base, changed.baseline_id().unwrap());

        let mut changed = manifest();
        changed.state_evidence_root = StateEvidenceRoot::new(Digest::of_bytes(b"other-evidence"));
        assert_ne!(base, changed.baseline_id().unwrap());

        let mut changed = manifest();
        changed.coverage_evidence_root =
            CoverageEvidenceRoot::new(Digest::of_bytes(b"other-coverage"));
        assert_ne!(base, changed.baseline_id().unwrap());
    }

    #[test]
    fn better_coverage_of_identical_state_is_a_different_accepted_node() {
        // Same material state, different knowledge about it. Deliberately a
        // different BaselineId, not the same one.
        let complete = manifest().baseline_id().unwrap();
        let mut partial = manifest();
        partial.coverage_evidence_root =
            CoverageEvidenceRoot::new(Digest::of_bytes(b"nothing-observed"));
        assert_eq!(manifest().project_state_root, partial.project_state_root);
        assert_ne!(complete, partial.baseline_id().unwrap());
    }

    #[test]
    fn lineage_is_part_of_identity() {
        let orphan = manifest().baseline_id().unwrap();
        let mut child = manifest();
        child.parent_baseline_id = Some(orphan.clone());
        assert!(!child.is_initial());
        assert_ne!(orphan, child.baseline_id().unwrap());
    }

    #[test]
    fn a_baseline_belongs_to_exactly_one_project() {
        let mine = manifest().baseline_id().unwrap();
        let mut theirs = manifest();
        theirs.project = ProjectId::parse("prj_999999").unwrap();
        assert_ne!(mine, theirs.baseline_id().unwrap());
    }

    #[test]
    fn a_foreign_format_revision_is_refused_rather_than_guessed_at() {
        let mut future = manifest();
        future.format_revision = 2;
        assert!(matches!(future.validate(), Err(FormatError::Identity(_))));
        assert!(future.baseline_id().is_err());
    }

    #[test]
    fn the_manifest_carries_no_acceptance_metadata_at_all() {
        // The structural reason accepted_at and the accepting actor cannot
        // affect BaselineId: there is nowhere in the manifest to put them.
        let encoded = serde_json::to_string(&manifest()).unwrap();
        for outside in ["accepted_at", "actor", "origin", "receipt", "status"] {
            assert!(!encoded.contains(outside), "{outside} in {encoded}");
        }
        assert_eq!(
            serde_json::from_str::<BaselineManifest>(&encoded).unwrap(),
            manifest()
        );
    }
}
