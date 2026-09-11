//! The Baseline — one exact accepted historical node.
//!
//! ```text
//! ProjectStateRoot        WHAT material state is accepted?
//! StateEvidenceRoot       WHAT provenance establishes the entries that exist?
//! CoverageEvidenceRoot    WHAT coverage claims justify what is absent?
//! BaselineId              all three, plus lineage
//! ```
//!
//! # Why three roots and not one hash of everything
//!
//! They answer different questions, and conflating them destroys the ability
//! to tell apart changes that matter differently.
//!
//! Observing the same files again with a better instrument changes no material
//! state — `ProjectStateRoot` is identical — but it does change what
//! established that state, so `StateEvidenceRoot` moves and the `BaselineId`
//! with it. That is correct and deliberate: the project accepted a different
//! *justification* for the same content, and a single hash could not express
//! the difference between "the code changed" and "we now know it better".
//!
//! Equally, absence has to be proved. A resource missing from
//! `ProjectStateRoot` is either genuinely not there or was never looked for,
//! and only `CoverageEvidenceRoot` separates those. A Baseline without it
//! would silently claim "this is everything" whenever an observation failed.
//!
//! # Why the record is separate from the manifest
//!
//! `BaselineManifest` is what the Baseline *is* — the roots and its parent —
//! and its digest is the `BaselineId`. `BaselineRecord` is the bookkeeping
//! around accepting it: who accepted it, when, and under what origin.
//!
//! The split is what keeps identity stable. If `accepted_at` were inside the
//! manifest, accepting identical state twice would produce two different
//! `BaselineId`s, and the same project state would have two names. So the
//! record carries the timing and the manifest carries the identity — the rule
//! being reachability from the manifest, never the datatype (§2.18).
//!
//! # Why acceptance is create-once
//!
//! A `BaselineId` is cited by promotions, publications and receipts. If the
//! bytes under one could be replaced, every one of those citations would
//! silently come to mean something else. Records are written once and
//! re-verified on load.

use serde::{Deserialize, Serialize};

use draft_dcg_contract::baseline::{BaselineId, BaselineManifest};
use draft_dcg_contract::ids::{ActorId, ChangeRevisionId, ProjectId, PromotionId};
use draft_dcg_contract::roots::{
    CoverageEvidenceRoot, ProjectStateRoot, ProjectStateRootBuilder, StateEvidenceRoot,
    StateEvidenceRootBuilder,
};
use draft_dcg_contract::value::Timestamp;

use crate::project::layout::DraftLayout;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::immutable_store::{ImmutableFactStore, StoreOutcome};

/// How a Baseline came to be accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum BaselineOrigin {
    /// The project's first Baseline, with no parent.
    Initial,
    /// Accepted by promoting an exact Change revision.
    ///
    /// Both ids are kept: the promotion says which transaction accepted it,
    /// the revision says exactly what work was accepted. Recording only the
    /// promotion would leave "what was in it?" answerable only by inference.
    Promotion {
        promotion: PromotionId,
        change_revision: ChangeRevisionId,
    },
}

impl BaselineOrigin {
    pub fn is_initial(&self) -> bool {
        matches!(self, Self::Initial)
    }
}

/// The bookkeeping around one acceptance.
///
/// Deliberately holds nothing that feeds `BaselineId`. Everything here is
/// outside the manifest, so none of it can change what the Baseline is called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineRecord {
    pub baseline_id: BaselineId,
    pub origin: BaselineOrigin,
    pub actor: ActorId,
    pub accepted_at: Timestamp,
}

impl BaselineRecord {
    /// Check the record against the manifest it claims to describe.
    ///
    /// The two are stored separately, so nothing but this check stops a record
    /// naming a Baseline it was not written for.
    pub fn validate_against(&self, manifest: &BaselineManifest) -> DraftResult<()> {
        let expected = manifest.baseline_id().map_err(format_error)?;
        if self.baseline_id != expected {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "baseline record names {} but its manifest computes to {expected}",
                    self.baseline_id
                ),
            ));
        }
        if self.origin.is_initial() != manifest.is_initial() {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "baseline {} disagrees with its manifest about whether it is the project's \
                     first: an Initial origin has no parent, and a promoted one always has",
                    self.baseline_id
                ),
            ));
        }
        Ok(())
    }
}

fn format_error(error: draft_dcg_contract::FormatError) -> DraftError {
    DraftError::new(DraftErrorKind::CorruptData, error.to_string())
}

/// Compose a manifest from roots that have already been built.
///
/// Takes the three roots rather than the raw state, so the caller that
/// established them is the one accountable for their bijection — this function
/// cannot silently accept an evidence root that does not match its state root,
/// because it never sees the entries.
pub fn manifest(
    project: ProjectId,
    project_state_root: ProjectStateRoot,
    state_evidence_root: StateEvidenceRoot,
    coverage_evidence_root: CoverageEvidenceRoot,
    parent_baseline_id: Option<BaselineId>,
) -> DraftResult<BaselineManifest> {
    let manifest = BaselineManifest {
        project,
        project_state_root,
        state_evidence_root,
        coverage_evidence_root,
        parent_baseline_id,
        format_revision: 1,
    };
    manifest.validate().map_err(format_error)?;
    Ok(manifest)
}

/// Build the state and evidence roots together, enforcing their bijection.
///
/// # Why both are built by one call
///
/// Every material entry in the state root must have exactly one evidence
/// entry, and the evidence root must contain no subject the state root does
/// not. Building them separately makes that a rule somebody remembers to
/// check; building them together makes it the only way to get either.
pub fn build_state_and_evidence(
    state: &ProjectStateRootBuilder,
    evidence: &StateEvidenceRootBuilder,
) -> DraftResult<(ProjectStateRoot, StateEvidenceRoot)> {
    evidence.verify_bijection(state).map_err(format_error)?;
    Ok((
        state.build().map_err(format_error)?,
        evidence.build().map_err(format_error)?,
    ))
}

/// The Baseline a project currently accepts, if it has one.
pub fn current_baseline(layout: &DraftLayout) -> DraftResult<Option<BaselineId>> {
    let control = crate::project::control::ProjectControlStore::new(layout.project_control_dir());
    Ok(control
        .read_unlocked()?
        .map(|state| state.accepted_baseline))
}

/// The material state the project's accepted Baseline holds.
pub fn accepted_state_root(
    layout: &DraftLayout,
) -> DraftResult<Option<draft_dcg_contract::roots::ProjectStateRoot>> {
    let Some(baseline) = current_baseline(layout)? else {
        return Ok(None);
    };
    let stores = BaselineStore::new(layout.baselines_dir());
    Ok(stores
        .manifest(&baseline)?
        .map(|manifest| manifest.project_state_root))
}

/// Accepted Baselines, written once.
#[derive(Debug, Clone)]
pub struct BaselineStore {
    manifests: ImmutableFactStore<BaselineManifest>,
    records: ImmutableFactStore<BaselineRecord>,
    compositions: ImmutableFactStore<crate::dcg::compose::HistoricalBaselineComposition>,
}

impl BaselineStore {
    /// Open the store over `baselines/`.
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        let directory = directory.into();
        Self {
            manifests: ImmutableFactStore::new(directory.join("manifests")),
            records: ImmutableFactStore::new(directory.join("records")),
            compositions: ImmutableFactStore::new(directory.join("compositions")),
        }
    }

    /// Accept a Baseline.
    ///
    /// The composition is written first, then the manifest, then the record.
    /// Every step before the record leaves an orphan a retry reuses; the record
    /// is last because it is the thing that says the Baseline exists.
    ///
    /// The composition is stored rather than recomputed because the evidence
    /// root is a digest: it proves the entries did not change and cannot say
    /// what they were. Without this, nothing could answer which binding
    /// established an accepted state — and therefore nothing could route a
    /// Publication of it without inventing the provenance.
    pub fn accept(
        &self,
        manifest: &BaselineManifest,
        record: &BaselineRecord,
        composition: &crate::dcg::compose::HistoricalBaselineComposition,
    ) -> DraftResult<StoreOutcome> {
        record.validate_against(manifest)?;
        let key = record.baseline_id.digest().to_string();
        self.compositions.put(&key, composition)?;
        self.manifests.put(&key, manifest)?;
        self.records.put(&key, record)
    }

    /// What established this Baseline's accepted state.
    pub fn composition(
        &self,
        baseline: &BaselineId,
    ) -> DraftResult<Option<crate::dcg::compose::HistoricalBaselineComposition>> {
        self.compositions.get(&baseline.digest().to_string())
    }

    /// The manifest for a Baseline, verified against the id it is filed under.
    pub fn manifest(&self, baseline: &BaselineId) -> DraftResult<Option<BaselineManifest>> {
        let Some(manifest) = self.manifests.get(&baseline.digest().to_string())? else {
            return Ok(None);
        };
        let recomputed = manifest.baseline_id().map_err(format_error)?;
        if &recomputed != baseline {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("baseline {baseline} holds a manifest computing to {recomputed}"),
            ));
        }
        Ok(Some(manifest))
    }

    /// The acceptance record for a Baseline.
    pub fn record(&self, baseline: &BaselineId) -> DraftResult<Option<BaselineRecord>> {
        self.records.get(&baseline.digest().to_string())
    }

    /// Walk a Baseline's lineage back to the project's first.
    ///
    /// Stops at an unknown parent rather than guessing, and refuses a cycle:
    /// lineage that loops is not lineage, and following it would not terminate.
    pub fn lineage(&self, from: &BaselineId) -> DraftResult<Vec<BaselineId>> {
        let mut chain = vec![from.clone()];
        let mut seen = std::collections::BTreeSet::new();
        seen.insert(from.clone());
        let mut current = from.clone();

        while let Some(manifest) = self.manifest(&current)? {
            let Some(parent) = manifest.parent_baseline_id.clone() else {
                break;
            };
            if !seen.insert(parent.clone()) {
                return Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!("baseline lineage from {from} revisits {parent}"),
                ));
            }
            chain.push(parent.clone());
            current = parent;
        }
        Ok(chain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::ids::{ObservationId, ResourceId};
    use draft_dcg_contract::observation::{ObservationDigest, ObservationRef};
    use draft_dcg_contract::roots::{BaselineStateEvidenceEntry, CoverageEvidenceRootBuilder};
    use draft_dcg_contract::state::ResourceStateDigest;
    use draft_dcg_contract::Digest;

    fn composition() -> crate::dcg::compose::HistoricalBaselineComposition {
        crate::dcg::compose::HistoricalBaselineComposition {
            resource_provenance: std::collections::BTreeMap::new(),
            accepted_state: std::collections::BTreeMap::new(),
        }
    }

    fn project() -> ProjectId {
        ProjectId::parse("prj_000000000001").unwrap()
    }

    fn actor() -> ActorId {
        ActorId::parse("act_000000000001").unwrap()
    }

    fn resource(seed: &str) -> ResourceId {
        ResourceId::parse(format!("res_00000000000{seed}")).unwrap()
    }

    fn state(seed: &[u8]) -> ResourceStateDigest {
        ResourceStateDigest::new(Digest::of_bytes(seed))
    }

    fn observation(seed: &str) -> ObservationRef {
        ObservationRef {
            id: ObservationId::parse(format!("obs_00000000000{seed}")).unwrap(),
            digest: ObservationDigest::new(Digest::of_bytes(seed.as_bytes())),
        }
    }

    /// One resource, its state and the observation that established it.
    fn roots(resource_state: &[u8]) -> (ProjectStateRoot, StateEvidenceRoot, CoverageEvidenceRoot) {
        let mut state_builder = ProjectStateRootBuilder::new();
        state_builder
            .insert_resource(resource("1"), state(resource_state))
            .unwrap();
        let mut evidence_builder = StateEvidenceRootBuilder::new();
        evidence_builder
            .insert(BaselineStateEvidenceEntry::Resource {
                resource_id: resource("1"),
                state: state(resource_state),
                primary: observation("1"),
                corroborating: Default::default(),
            })
            .unwrap();
        let (state_root, evidence_root) =
            build_state_and_evidence(&state_builder, &evidence_builder).unwrap();
        (
            state_root,
            evidence_root,
            CoverageEvidenceRootBuilder::new().build().unwrap(),
        )
    }

    fn initial() -> BaselineManifest {
        let (state_root, evidence_root, coverage_root) = roots(b"state-1");
        manifest(project(), state_root, evidence_root, coverage_root, None).unwrap()
    }

    fn record_for(manifest: &BaselineManifest, origin: BaselineOrigin) -> BaselineRecord {
        BaselineRecord {
            baseline_id: manifest.baseline_id().unwrap(),
            origin,
            actor: actor(),
            accepted_at: Timestamp::from_unix_nanos(0),
        }
    }

    #[test]
    fn accepting_the_same_state_twice_yields_the_same_identity() {
        // The reason acceptance timing lives in the record and not the
        // manifest: identical state accepted at two moments is one Baseline,
        // not two with the same content under different names.
        let early = record_for(&initial(), BaselineOrigin::Initial);
        let mut later = early.clone();
        later.accepted_at = Timestamp::from_unix_nanos(9_999);

        assert_eq!(early.baseline_id, later.baseline_id);
        assert_ne!(early, later);
    }

    #[test]
    fn better_evidence_for_identical_state_is_a_different_baseline() {
        // Same material state, a different observation establishing it. The
        // state root is identical and the identity is not — the project
        // accepted a different justification for the same content.
        let (state_root, _, coverage_root) = roots(b"state-1");

        let mut other_evidence = StateEvidenceRootBuilder::new();
        other_evidence
            .insert(BaselineStateEvidenceEntry::Resource {
                resource_id: resource("1"),
                state: state(b"state-1"),
                primary: observation("2"),
                corroborating: Default::default(),
            })
            .unwrap();
        let mut same_state = ProjectStateRootBuilder::new();
        same_state
            .insert_resource(resource("1"), state(b"state-1"))
            .unwrap();
        let (second_state_root, second_evidence_root) =
            build_state_and_evidence(&same_state, &other_evidence).unwrap();

        assert_eq!(
            state_root, second_state_root,
            "the material state is unchanged"
        );
        let first = initial();
        let second = manifest(
            project(),
            second_state_root,
            second_evidence_root,
            coverage_root,
            None,
        )
        .unwrap();
        assert_ne!(
            first.baseline_id().unwrap(),
            second.baseline_id().unwrap(),
            "a different justification is a different accepted node"
        );
    }

    #[test]
    fn evidence_for_a_state_the_root_does_not_hold_is_refused() {
        // The bijection. Building the two roots together is what makes this
        // unreachable rather than a rule somebody remembers.
        let mut state_builder = ProjectStateRootBuilder::new();
        state_builder
            .insert_resource(resource("1"), state(b"state-1"))
            .unwrap();
        let mut evidence_builder = StateEvidenceRootBuilder::new();
        evidence_builder
            .insert(BaselineStateEvidenceEntry::Resource {
                resource_id: resource("2"),
                state: state(b"state-2"),
                primary: observation("1"),
                corroborating: Default::default(),
            })
            .unwrap();

        let error = build_state_and_evidence(&state_builder, &evidence_builder).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn a_record_naming_a_different_baseline_is_refused() {
        // The manifest and the record are stored apart, so nothing else stops
        // a record being filed against a Baseline it was not written for.
        let manifest = initial();
        let mut wrong = record_for(&manifest, BaselineOrigin::Initial);
        wrong.baseline_id = BaselineId::new(Digest::of_bytes(b"somebody-elses-baseline"));

        assert!(wrong.validate_against(&manifest).is_err());
    }

    #[test]
    fn an_origin_that_contradicts_the_lineage_is_refused() {
        // `Initial` means no parent. A record claiming to be the project's
        // first while its manifest names a parent is describing a lineage that
        // does not exist.
        let (state_root, evidence_root, coverage_root) = roots(b"state-2");
        let child = manifest(
            project(),
            state_root,
            evidence_root,
            coverage_root,
            Some(initial().baseline_id().unwrap()),
        )
        .unwrap();

        assert!(record_for(&child, BaselineOrigin::Initial)
            .validate_against(&child)
            .is_err());
        record_for(
            &child,
            BaselineOrigin::Promotion {
                promotion: PromotionId::parse("pro_000000000001").unwrap(),
                change_revision: ChangeRevisionId::parse("rev_000000000001").unwrap(),
            },
        )
        .validate_against(&child)
        .unwrap();
    }

    #[test]
    fn an_accepted_baseline_cannot_be_altered_behind_its_id() {
        // Promotions, publications and receipts cite a BaselineId. If the
        // bytes under one could move, every citation would quietly change
        // meaning.
        let directory = tempfile::tempdir().unwrap();
        let store = BaselineStore::new(directory.path());
        let manifest = initial();
        let record = record_for(&manifest, BaselineOrigin::Initial);

        store.accept(&manifest, &record, &composition()).unwrap();
        store.accept(&manifest, &record, &composition()).unwrap();

        let mut rewritten = record.clone();
        rewritten.actor = ActorId::parse("act_000000000002").unwrap();
        assert!(store.accept(&manifest, &rewritten, &composition()).is_err());
        assert_eq!(
            store.record(&record.baseline_id).unwrap(),
            Some(record.clone())
        );
        assert_eq!(store.manifest(&record.baseline_id).unwrap(), Some(manifest));
    }

    #[test]
    fn lineage_walks_back_to_the_projects_first_baseline() {
        let directory = tempfile::tempdir().unwrap();
        let store = BaselineStore::new(directory.path());

        let first = initial();
        store
            .accept(
                &first,
                &record_for(&first, BaselineOrigin::Initial),
                &composition(),
            )
            .unwrap();

        let (state_root, evidence_root, coverage_root) = roots(b"state-2");
        let second = manifest(
            project(),
            state_root,
            evidence_root,
            coverage_root,
            Some(first.baseline_id().unwrap()),
        )
        .unwrap();
        store
            .accept(
                &second,
                &record_for(
                    &second,
                    BaselineOrigin::Promotion {
                        promotion: PromotionId::parse("pro_000000000001").unwrap(),
                        change_revision: ChangeRevisionId::parse("rev_000000000001").unwrap(),
                    },
                ),
                &composition(),
            )
            .unwrap();

        assert_eq!(
            store.lineage(&second.baseline_id().unwrap()).unwrap(),
            vec![second.baseline_id().unwrap(), first.baseline_id().unwrap()]
        );
    }

    #[test]
    fn an_unknown_baseline_has_no_manifest_rather_than_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let store = BaselineStore::new(directory.path());
        let unknown = BaselineId::new(Digest::of_bytes(b"never-accepted"));
        assert_eq!(store.manifest(&unknown).unwrap(), None);
        assert_eq!(store.record(&unknown).unwrap(), None);
    }
}
