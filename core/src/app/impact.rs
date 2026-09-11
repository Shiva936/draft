//! What a revision reaches, and what has actually been proved about it.
//!
//! Two questions that look alike and are answered by different machinery:
//!
//! ```text
//! impact     which elements this revision touched, and what relates to them
//! coverage   which Resources the evidence about it actually speaks for
//! ```
//!
//! # Impact is contributed, and Core privileges no vocabulary
//!
//! An element has a stable id, an optional namespaced kind, a name and typed
//! attributes; a relation has a namespaced kind and two endpoints. Core stores,
//! links and counts them and never learns what any of them means. There is no
//! notion of a "public API" or a "reference" built in, and the relation index
//! is direction-agnostic because which way a domain points its relations is the
//! domain's business.
//!
//! Nothing is inferred. An element exists because an authorized extractor said
//! so; a relation exists because one said so. Directory layout, dependency
//! edges and name similarity produce no elements at all.
//!
//! # Coverage is deliberately hard to satisfy
//!
//! [`crate::read_model::coverage`] holds the rule; this module supplies its
//! inputs and nothing else. A resource is directly covered when evidence read
//! an observation *of that resource* — nothing weaker. Indirect coverage needs
//! somebody to have said so, and v1 has no store for a declared coverage
//! relationship or a producer attestation, so the answer reports that those
//! sources are unavailable rather than reporting their absence as "nothing is
//! indirectly covered". Those are different facts and only one of them is
//! fixed by installing something.

use std::collections::{BTreeMap, BTreeSet};

use draft_dcg_contract::ids::{ChangeRevisionId, ResourceId};
use draft_extension_contract::{EngineId, Executor, NamespacedId};
use serde_json::Value;

use crate::app::Workspace;
use crate::dcg::impact::{
    merge_extractor_results, ElementRelation, ExtractorResult, ImpactIndex, ResourceElement,
};
use crate::extension::ActiveContributions;
use crate::read_model::coverage::{self, CoverageInputs, ResourceCoverage};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// What extraction established for one revision, and what it could not ask.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImpactReport {
    pub revision: ChangeRevisionId,
    /// The Resources the revision touched.
    pub touched: BTreeSet<ResourceId>,
    /// Every element an authorized extractor found inside them.
    pub elements: Vec<ResourceElement>,
    /// Resources reachable from those elements through a contributed relation.
    ///
    /// Reachability through a *declared* relation, never through the graph, a
    /// directory or a dependency: an edge somebody contributed is an
    /// assertion, and proximity is not.
    pub related_resources: Vec<ResourceId>,
    /// Elements two extractors claimed with different content. Scoped: every
    /// other element from both extractors is kept.
    pub collisions: Vec<crate::dcg::impact::ElementCollision>,
    /// Resources nothing installed can extract elements from.
    ///
    /// A real answer, not a failure. Without it "no elements" would mean both
    /// "nothing is in there" and "nothing knows how to look".
    pub unextractable: Vec<ResourceId>,
    pub extractors: Vec<String>,
}

/// One extractor's declared operation, and the decision that permits it.
struct AuthorizedExtractor<'a> {
    extractor_id: NamespacedId,
    contributed_by: &'a str,
    operation: &'a draft_extension_contract::MechanismOperation,
    applies_to: &'a draft_extension_contract::ResourcePredicate,
    decision: Option<String>,
}

/// Derive coverage for the Resources a revision touched.
///
/// The inputs are read from authoritative facts, never inferred, and the
/// answer keeps the three states apart: proved directly, asserted indirectly,
/// and nothing at all.
pub fn coverage_of(
    workspace: &Workspace,
    revision: &ChangeRevisionId,
) -> DraftResult<CoverageReport> {
    let revisions = crate::dcg::revision::RevisionStore::new(workspace.layout.revisions_dir());
    let sealed = revisions.get(revision)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("revision '{revision}' has not been sealed"),
        )
    })?;

    let stores = crate::app::authorization::AuthorizationStores::for_layout(&workspace.layout);
    let observations =
        crate::dcg::observation_set::ObservationStore::new(workspace.layout.observations_dir());

    let mut inputs = CoverageInputs::default();
    for evidence in stores.evidence.list()? {
        if !evidence.covers(revision) {
            continue;
        }
        let mut bound = BTreeSet::new();
        for reference in &evidence.inputs {
            // Resolved through the exact reference, so a substituted
            // observation cannot silently extend what the evidence covers.
            if let Some(observation) = observations.resolve(reference)? {
                bound.insert(observation.resource.clone());
            }
        }
        if !bound.is_empty() {
            inputs.direct_bindings.insert(evidence.id.clone(), bound);
        }
    }

    let resolved = coverage::resolve(&inputs, &sealed.touched);
    let unproven = coverage::unproven(&inputs, &sealed.touched);
    Ok(CoverageReport {
        revision: revision.clone(),
        coverage: resolved,
        unproven,
        indirect_sources: IndirectSources::default(),
    })
}

/// What is proved about the Resources a revision touched.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CoverageReport {
    pub revision: ChangeRevisionId,
    pub coverage: BTreeMap<ResourceId, ResourceCoverage>,
    /// Everything no evidence names. Indirect coverage does not remove a
    /// Resource from this list: a reader asking "what is unproven?" must see
    /// everything nothing has named, or the list answers a different question.
    pub unproven: BTreeSet<ResourceId>,
    pub indirect_sources: IndirectSources,
}

/// Where indirect coverage could have come from, and whether it was available.
///
/// Reported rather than silently empty. "No declared relationship asserts
/// this" and "Draft has nowhere to record such an assertion" are different
/// facts, and only the first is a statement about the project.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndirectSources {
    pub declared_relationships: SourceAvailability,
    pub producer_attestations: SourceAvailability,
}

impl Default for IndirectSources {
    fn default() -> Self {
        Self {
            declared_relationships: SourceAvailability::Unavailable {
                reason: "v1 records no declared coverage relationship, so none can be consulted",
            },
            producer_attestations: SourceAvailability::Unavailable {
                reason: "v1 records no producer coverage attestation, so none can be consulted",
            },
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum SourceAvailability {
    Available { asserted: usize },
    Unavailable { reason: &'static str },
}

/// Every extractor an installed, authorized extension contributes.
fn authorized_extractors<'a>(
    app: &crate::app::App,
    contributions: &'a ActiveContributions,
) -> Vec<AuthorizedExtractor<'a>> {
    contributions
        .element_extractors
        .iter()
        .map(|contributed| {
            let (_, decision) = app.authorized_command(
                contributions,
                &contributed.extension_id,
                &contributed.value.operation,
            );
            AuthorizedExtractor {
                extractor_id: contributed.value.extractor_id.clone(),
                contributed_by: contributed.extension_id.as_str(),
                operation: &contributed.value.operation,
                applies_to: &contributed.value.applies_to,
                decision,
            }
        })
        .collect()
}

/// Run one extractor over one Resource.
///
/// Engine-backed extraction runs Draft's own projection over the Resource's
/// declared attributes and reads no content at all — which is what makes it
/// usable for a Resource whose bytes Draft cannot or should not fetch.
/// Command-backed extraction crosses the one process boundary, carrying the
/// authorization decision that permits it.
fn extract(
    extractor: &AuthorizedExtractor<'_>,
    state: &crate::dcg::resource::RawResourceState,
    producer: crate::extension::ProducerRef,
    workspace_id: &str,
) -> DraftResult<Option<ExtractorResult>> {
    match &extractor.operation.executor {
        Executor::Engine {
            engine: EngineId::AttributeProjection,
            engine_revision,
            config,
        } => {
            crate::execution::mechanism::engine::check_revision(
                EngineId::AttributeProjection,
                *engine_revision,
            )?;
            let config =
                crate::execution::mechanism::engine::attribute::ProjectionConfig::parse(config)?;
            let projection = crate::execution::mechanism::engine::attribute::project(
                &config,
                &state.resource_id,
                &state.attributes,
                &producer,
            );
            Ok(Some(ExtractorResult {
                extractor_id: extractor.extractor_id.clone(),
                resource_id: state.resource_id.clone(),
                elements: projection.elements,
                relations: projection.relations,
                producer,
            }))
        }
        Executor::Engine { engine, .. } => Err(DraftError::invalid_config(format!(
            "extractor '{}' names engine '{}', which extracts nothing",
            extractor.extractor_id.qualified(),
            engine.as_str()
        ))),
        Executor::Command { .. } => {
            let Some(decision) = extractor.decision.clone() else {
                // Installed and trusted is not authorized, and running anyway
                // would make the grant decorative.
                return Ok(None);
            };
            let request = serde_json::json!({
                "extractor_id": extractor.extractor_id.qualified(),
                "resource_id": state.resource_id.as_str(),
                "locator": state.locator,
                "media_type": state.media_type,
                "state_digest": state.state_digest,
                "attributes": state.attributes,
            });
            let response = crate::execution::mechanism::invoke_command(
                extractor.operation,
                &request,
                &crate::execution::mechanism::MechanismInputs::default(),
                &crate::execution::mechanism::MechanismContext {
                    workspace_id: workspace_id.to_string(),
                    operation_id: crate::support::common::OperationId::generate().to_string(),
                    producer: producer.clone(),
                    authorization_decision: Some(decision),
                },
            )?;
            Ok(Some(parse_extraction(
                extractor,
                &state.resource_id,
                producer,
                &response.payload,
            )?))
        }
    }
}

/// Read one extractor's response, stamping the provenance Draft owns.
///
/// The extension supplies elements and relations; the extractor id, the
/// Resource and the producer are Draft's, because a contribution that could
/// mint its own provenance could attribute its results to somebody else.
fn parse_extraction(
    extractor: &AuthorizedExtractor<'_>,
    resource_id: &ResourceId,
    producer: crate::extension::ProducerRef,
    payload: &Value,
) -> DraftResult<ExtractorResult> {
    #[derive(serde::Deserialize)]
    struct RawElement {
        element_id: String,
        #[serde(default)]
        kind: Option<NamespacedId>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        attributes: BTreeMap<String, draft_extension_contract::AttributeValue>,
    }
    #[derive(serde::Deserialize)]
    struct RawRelation {
        relation_kind: NamespacedId,
        from: String,
        to: String,
    }
    #[derive(serde::Deserialize)]
    struct RawResult {
        #[serde(default)]
        elements: Vec<RawElement>,
        #[serde(default)]
        relations: Vec<RawRelation>,
    }

    let raw: RawResult = serde_json::from_value(payload.clone()).map_err(|error| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!(
                "extractor '{}' returned a response Draft cannot read: {error}",
                extractor.extractor_id.qualified()
            ),
        )
    })?;
    Ok(ExtractorResult {
        extractor_id: extractor.extractor_id.clone(),
        resource_id: resource_id.clone(),
        elements: raw
            .elements
            .into_iter()
            .map(|element| ResourceElement {
                element_id: element.element_id,
                resource_id: resource_id.clone(),
                kind: element.kind,
                name: element.name,
                attributes: element.attributes,
                producer: producer.clone(),
            })
            .collect(),
        relations: raw
            .relations
            .into_iter()
            .map(|relation| ElementRelation {
                relation_kind: relation.relation_kind,
                from: relation.from,
                to: relation.to,
                producer: producer.clone(),
            })
            .collect(),
        producer,
    })
}

/// Extract, merge and index the elements a revision touched.
pub fn index_revision(
    app: &crate::app::App,
    workspace: &Workspace,
    revision: &ChangeRevisionId,
) -> DraftResult<ImpactReport> {
    let revisions = crate::dcg::revision::RevisionStore::new(workspace.layout.revisions_dir());
    let sealed = revisions.get(revision)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("revision '{revision}' has not been sealed"),
        )
    })?;

    let contributions = app.active_contributions();
    let extractors = authorized_extractors(app, &contributions);
    let snapshot = app.observe(workspace)?;
    let classification =
        crate::evidence::classification::classify_snapshot(&snapshot, &contributions);
    let classes = classification.by_resource();

    let mut results = Vec::new();
    let mut unextractable = Vec::new();
    let empty = BTreeSet::new();
    for state in &snapshot.resources {
        if !sealed.touched.contains(&state.resource_id) {
            continue;
        }
        let view = crate::extension::ResourceView {
            locator_scheme: state.locator.scheme.as_str(),
            locator_body: state.locator.body.as_str(),
            media_type: state.media_type.as_deref(),
            form: state.form,
            attributes: &state.attributes,
            content_size: state.content_size,
        };
        let classes = classes.get(&state.resource_id).unwrap_or(&empty);
        let mut asked = false;
        for extractor in &extractors {
            if !crate::extension::capability::matches(extractor.applies_to, &view, classes) {
                continue;
            }
            asked = true;
            let producer = crate::app::producer_ref_for(&contributions, extractor.contributed_by);
            if let Some(result) =
                extract(extractor, state, producer, workspace.workspace_id.as_str())?
            {
                results.push(result);
            }
        }
        if !asked {
            unextractable.push(state.resource_id.clone());
        }
    }

    let merged = merge_extractor_results(&results);
    let index = ImpactIndex::open(&workspace.layout)?;
    index.index_revision(revision.as_str(), &merged)?;
    let touched_elements = index.elements_touched_by(revision.as_str())?;
    let related_resources = index.resources_related_by_relations(&touched_elements)?;

    let mut extractor_ids: Vec<String> = results
        .iter()
        .map(|result| result.extractor_id.qualified())
        .collect();
    extractor_ids.sort();
    extractor_ids.dedup();

    Ok(ImpactReport {
        revision: revision.clone(),
        touched: sealed.touched.clone(),
        elements: merged.elements,
        related_resources,
        collisions: merged.collisions,
        unextractable,
        extractors: extractor_ids,
    })
}
