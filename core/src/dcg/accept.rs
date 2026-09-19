//! The accepted-state path: observe the project, then accept what was seen.
//!
//! This is the only way a Baseline comes into existence. It runs the whole
//! chain in one place so no caller can perform half of it:
//!
//! ```text
//! enumerate  →  ObservationRun + Observations  →  three roots  →  BaselineRecord
//! ```
//!
//! # Why acceptance and observation are one operation
//!
//! A Baseline's `StateEvidenceRoot` names the observations that established
//! it. If observing and accepting were separate calls, a caller could accept
//! state established by an observation run that had since been superseded —
//! recording provenance for a moment other than the one accepted.
//!
//! Doing both here means the evidence in a Baseline is always the evidence
//! from the run that produced it.
//!
//! # Why a failed enumeration still produces a Baseline
//!
//! It produces one that says so. The run is `Partial`, the domains it could
//! not finish carry `NotObserved` coverage with `attempted` set, and the
//! `CoverageEvidenceRoot` records that absence was not established.
//!
//! Refusing to accept anything would leave the project with no accepted state
//! at all, which is a worse answer than an accepted state that is honest about
//! what it could not see.

use draft_dcg_contract::baseline::{BaselineId, BaselineManifest};
use draft_dcg_contract::coverage::CoverageDomainRef;
use draft_dcg_contract::ids::{ActorId, ProjectId};
use draft_dcg_contract::observation::ObservationStability;
use draft_dcg_contract::state::ResourceStateDigest;
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;

use crate::dcg::baseline::{manifest, BaselineOrigin, BaselineRecord, BaselineStore};
use crate::dcg::observation_set::ObservationStore;
use crate::dcg::observe::{
    self, EnumerationResult, ObservationOutcome, ObservedResource, ObservingBinding,
};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// One accepted Baseline and the evidence behind it.
#[derive(Debug, Clone)]
pub struct Accepted {
    pub manifest: BaselineManifest,
    pub record: BaselineRecord,
    pub observations: ObservationOutcome,
}

impl Accepted {
    pub fn baseline_id(&self) -> &BaselineId {
        &self.record.baseline_id
    }
}

/// Translate an observation of the project into the canonical shape.
///
/// The scanner reports what it saw in its own vocabulary; this is where that
/// becomes the ontology's. A resource whose adapter could not establish a
/// deterministic state digest is refused rather than carried — an adapter
/// reports such a thing as untrackable, and inventing a digest for it would
/// put a fiction into the state root.
pub fn canonicalize(
    snapshot: &crate::dcg::state::Snapshot,
    started_at: Timestamp,
    completed_at: Timestamp,
) -> DraftResult<EnumerationResult> {
    let mut attempted = Vec::new();
    let mut committed = Vec::new();
    for coverage in &snapshot.observation_map.domains {
        let domain = domain_ref(&coverage.domain)?;
        attempted.push(domain.clone());
        if coverage.status.is_complete() {
            committed.push(domain);
        }
    }

    // Which domain each resource was found in. A resource the map does not
    // place is one nothing claimed coverage for, which is a coverage bug
    // rather than a resource Draft may quietly accept.
    let mut membership = std::collections::BTreeMap::new();
    for entry in &snapshot.observation_map.resource_membership {
        membership.insert(entry.resource_id.clone(), domain_ref(&entry.domain)?);
    }

    let mut resources = Vec::new();
    for state in &snapshot.resources {
        crate::dcg::resource::require_state_digest(state)?;
        let domain = membership.get(&state.resource_id).cloned().ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "resource '{}' was observed but belongs to no coverage domain, so nothing \
                     accounts for how it was found",
                    state.resource_id
                ),
            )
        })?;
        resources.push(ObservedResource {
            resource: state.resource_id.clone(),
            state: ResourceStateDigest::new(Digest::of_bytes(state.state_digest.as_bytes())),
            domain,
            // The filesystem adapter fences by device/inode/size/mtime, which
            // can alias inside a timestamp tick. Claiming Stable would assert
            // a reproducibility it cannot prove.
            stability: ObservationStability::Unknown,
        });
    }

    Ok(EnumerationResult {
        attempted_domains: attempted,
        committed_domains: committed,
        resources,
        started_at,
        completed_at,
    })
}

/// The canonical domain reference for an adapter's domain.
fn domain_ref(
    domain: &crate::dcg::observation::CoverageDomainRef,
) -> DraftResult<CoverageDomainRef> {
    // Scoped by the adapter binding already, so the flat form keeps the same
    // distinctness: two adapters that both call a domain `root` stay separate.
    CoverageDomainRef::parse(domain.to_string().replace(':', "."))
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

/// Observe and accept, producing the project's next Baseline.
///
/// `parent` is `None` only for a project's first Baseline; every later one
/// names what it succeeded, which is what makes lineage walkable.
#[allow(clippy::too_many_arguments)]
pub fn accept(
    project: ProjectId,
    observer: &ObservingBinding,
    enumeration: &EnumerationResult,
    observations: &ObservationStore,
    baselines: &BaselineStore,
    origin: BaselineOrigin,
    actor: ActorId,
    accepted_at: Timestamp,
    parent: Option<BaselineId>,
) -> DraftResult<Accepted> {
    let outcome = observe::record(observer, enumeration)?;

    // The run and its observations become durable before anything cites them,
    // so a Baseline can never name evidence that does not exist.
    observations.put_run(&outcome.run)?;
    for observation in &outcome.observations {
        observations.put(observation)?;
    }

    let authoritative = outcome.authoritative()?;
    let (state_root, evidence_root) = authoritative.build_roots()?;
    let coverage_root = crate::dcg::observation_set::build_coverage_root(outcome.coverage.clone())?;

    // What established this state, derived from the same entries the evidence
    // root was built from. The observations are already durable above, so the
    // provenance lookup reads facts rather than the in-memory outcome.
    let composition = crate::dcg::compose::HistoricalBaselineComposition::derive(
        authoritative.state_evidence_entries()?.iter(),
        |reference| {
            let observation = observations.get(&reference.id)?.ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "observation '{}' was cited as evidence but is not stored",
                        reference.id
                    ),
                )
            })?;
            Ok(draft_dcg_contract::ProviderProvenanceRef {
                binding: observation.provider_binding.clone(),
                semantic_definition: observation.provider_semantic_definition.clone(),
            })
        },
    )?;

    let manifest = manifest(project, state_root, evidence_root, coverage_root, parent)?;
    let record = BaselineRecord {
        baseline_id: manifest
            .baseline_id()
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
        origin,
        actor,
        accepted_at,
    };
    baselines.accept(&manifest, &record, &composition)?;

    Ok(Accepted {
        manifest,
        record,
        observations: outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::identifier::NamespacedId;
    use draft_dcg_contract::ids::{PromotionId, ResourceId, RevisionPackId};
    use draft_dcg_contract::producer::ProducerIdentity;
    use draft_dcg_contract::ProviderSemanticDefinitionDigest;

    fn project() -> ProjectId {
        ProjectId::parse("prj_000000000001").unwrap()
    }

    fn actor() -> ActorId {
        ActorId::parse("act_000000000001").unwrap()
    }

    fn observer() -> ObservingBinding {
        ObservingBinding {
            binding: crate::dcg::filesystem_provider::filesystem_binding_id(),
            semantic_definition: ProviderSemanticDefinitionDigest::new(Digest::of_bytes(b"SD1")),
            producer: ProducerIdentity::new(
                NamespacedId::parse("draft.core/filesystem-observer").unwrap(),
                "1",
            )
            .unwrap(),
            observation_context: Digest::of_bytes(b"context"),
        }
    }

    fn enumeration(states: &[(&str, &[u8])], committed: bool) -> EnumerationResult {
        let domain = CoverageDomainRef::parse("draft.filesystem.root").unwrap();
        EnumerationResult {
            attempted_domains: vec![domain.clone()],
            committed_domains: if committed {
                vec![domain.clone()]
            } else {
                vec![]
            },
            resources: states
                .iter()
                .map(|(id, state)| ObservedResource {
                    resource: ResourceId::parse(format!("res_00000000000{id}")).unwrap(),
                    state: ResourceStateDigest::new(Digest::of_bytes(state)),
                    domain: domain.clone(),
                    stability: ObservationStability::Unknown,
                })
                .collect(),
            started_at: Timestamp::from_unix_nanos(0),
            completed_at: Timestamp::from_unix_nanos(1),
        }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        observations: ObservationStore,
        baselines: BaselineStore,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            Self {
                observations: ObservationStore::new(directory.path().join("observations")),
                baselines: BaselineStore::new(directory.path().join("baselines")),
                _directory: directory,
            }
        }

        fn accept(
            &self,
            enumeration: &EnumerationResult,
            origin: BaselineOrigin,
            parent: Option<BaselineId>,
        ) -> DraftResult<Accepted> {
            accept(
                project(),
                &observer(),
                enumeration,
                &self.observations,
                &self.baselines,
                origin,
                actor(),
                Timestamp::from_unix_nanos(5),
                parent,
            )
        }
    }

    #[test]
    fn accepting_an_observed_project_produces_a_baseline_with_its_evidence() {
        let fixture = Fixture::new();
        let accepted = fixture
            .accept(
                &enumeration(&[("1", b"state-1")], true),
                BaselineOrigin::Initial,
                None,
            )
            .unwrap();

        // The Baseline is durable, and so is every observation it names.
        assert_eq!(
            fixture.baselines.manifest(accepted.baseline_id()).unwrap(),
            Some(accepted.manifest.clone())
        );
        for observation in &accepted.observations.observations {
            assert!(fixture.observations.get(&observation.id).unwrap().is_some());
        }
    }

    #[test]
    fn a_second_acceptance_names_its_parent_and_the_lineage_walks_back() {
        let fixture = Fixture::new();
        let first = fixture
            .accept(
                &enumeration(&[("1", b"state-1")], true),
                BaselineOrigin::Initial,
                None,
            )
            .unwrap();
        let second = fixture
            .accept(
                &enumeration(&[("1", b"state-2")], true),
                BaselineOrigin::Promotion {
                    promotion: PromotionId::parse("pro_000000000001").unwrap(),
                    change_revision: RevisionPackId::parse("rpk_000000000001").unwrap(),
                },
                Some(first.baseline_id().clone()),
            )
            .unwrap();

        assert_eq!(
            fixture.baselines.lineage(second.baseline_id()).unwrap(),
            vec![second.baseline_id().clone(), first.baseline_id().clone()]
        );
    }

    #[test]
    fn a_failed_enumeration_is_accepted_as_a_baseline_that_says_so() {
        // Refusing to accept anything would leave the project with no accepted
        // state, which is worse than an accepted state honest about its gaps.
        let fixture = Fixture::new();
        let accepted = fixture
            .accept(&enumeration(&[], false), BaselineOrigin::Initial, None)
            .unwrap();

        assert_eq!(
            accepted.observations.run.terminal_status,
            draft_dcg_contract::observation::ObservationTerminalStatus::Failed
        );
        let claim = &accepted.observations.coverage[0];
        assert!(claim.attempted && !claim.committed);
    }

    #[test]
    fn accepting_identical_state_twice_converges_on_one_baseline() {
        // Same observations, same roots, same identity — and the store's
        // create-once binding accepts the repeat rather than refusing it.
        let fixture = Fixture::new();
        let first = fixture
            .accept(
                &enumeration(&[("1", b"state-1")], true),
                BaselineOrigin::Initial,
                None,
            )
            .unwrap();
        let again = fixture
            .accept(
                &enumeration(&[("1", b"state-1")], true),
                BaselineOrigin::Initial,
                None,
            )
            .unwrap();

        assert_eq!(first.baseline_id(), again.baseline_id());
    }

    #[test]
    fn changing_material_state_changes_the_baseline() {
        let fixture = Fixture::new();
        let before = fixture
            .accept(
                &enumeration(&[("1", b"state-1")], true),
                BaselineOrigin::Initial,
                None,
            )
            .unwrap();
        let after = fixture
            .accept(
                &enumeration(&[("1", b"state-2")], true),
                BaselineOrigin::Initial,
                None,
            )
            .unwrap();

        assert_ne!(before.baseline_id(), after.baseline_id());
    }
}
