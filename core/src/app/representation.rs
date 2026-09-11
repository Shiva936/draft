//! Producing the explanation of a sealed revision.
//!
//! # Why this exists at the application layer
//!
//! A representation is derived from three things that live in three different
//! places: the sealed revision, the observations that establish what the
//! workspace now holds, and whatever strategy the installed contributions
//! resolve for each touched Resource. No one of those owns the answer, so the
//! orchestration is here and the fact itself stays in `evidence`.
//!
//! # What Draft produces, and what it deliberately does not
//!
//! The neutral rendering always exists and no extension contributes it. It
//! says exactly what Core can justify: which Resource changed, between which
//! two authoritative state digests, and that the claim covers the whole
//! Resource because Draft cannot say where inside it the work landed.
//!
//! It does **not** diff content, parse a payload, or interpret a domain.
//! Doing any of that would make Core the semantic authority for every kind of
//! Resource, which is the coupling the whole contribution model exists to
//! avoid — and a `Whole` conflict claim is the honest, conservative statement
//! that two revisions touching one Resource cannot be shown separable.
//!
//! Where a contributed presentation claims a Resource, its strategy is
//! recorded and the resolution is carried in the summary labels. The payload
//! still says Draft produced it, because Draft did.
//! [`DerivationProvenance`] is a sum precisely so a contributed producer that
//! generates its own payload can record `Extension` provenance without Core
//! ever having to pretend it was the author.
//!
//! # No Activity event
//!
//! The frozen v1 vocabulary (§2.30) has `EvidenceProduced` and
//! `AssessmentProduced` and deliberately no representation event. That is not
//! an omission to fill: the vocabulary is closed, and a representation is a
//! regenerable explanation of sealed work rather than a decision anybody took.
//! It is a create-once immutable fact and a GC root, and that is all.

use std::collections::{BTreeMap, BTreeSet};

use draft_dcg_contract::observation::ObservationRef;
use draft_dcg_contract::Digest;

use crate::dcg::representation::{
    ConflictClaim, ConflictScope, RepresentationPayload, RepresentationSummary, ReviewUnit,
};
use crate::dcg::resource::ResourceId;
use crate::dcg::revision::ChangeRevision;
use crate::evidence::representation::{
    ChangeRepresentation, ChangeRepresentationBundle, RepresentationStore,
};
use crate::provenance::derived::DerivationProvenance;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// The Core semantics behind the neutral rendering.
///
/// Separate from every other revision in the tree: changing how a state
/// transition is explained must not look like a change to what the transition
/// *is*.
pub const NEUTRAL_REPRESENTATION_REVISION: u32 = 1;

/// The strategy id of the rendering Draft always provides.
pub const NEUTRAL_STRATEGY: &str = "draft.core/state-transition";

/// What the producer needs told, as opposed to what it reads.
pub struct RepresentationInputs<'a> {
    pub revision: &'a ChangeRevision,
    /// The exact observations that established the state being explained.
    pub observations: BTreeSet<ObservationRef>,
    /// The state each touched Resource holds now. Absent means removed.
    pub observed: &'a BTreeMap<ResourceId, draft_dcg_contract::ResourceStateDigest>,
    /// The state the accepted Baseline holds for each Resource. Absent means
    /// the Resource did not exist there.
    pub accepted: &'a BTreeMap<ResourceId, draft_dcg_contract::ResourceStateDigest>,
    /// The strategy resolved for each touched Resource, and who contributed
    /// it. `None` means nothing claimed it and the neutral rendering applies.
    pub strategies: &'a BTreeMap<ResourceId, ResolvedStrategy>,
    pub producer: draft_dcg_contract::producer::ProducerIdentity,
}

/// The presentation strategy a Resource resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStrategy {
    pub strategy_id: draft_extension_contract::identifier::NamespacedId,
    pub engine: String,
    /// The extension that contributed it, when one did.
    pub contributed_by: Option<String>,
}

/// Derive the bundle explaining one sealed revision.
pub fn derive(inputs: RepresentationInputs<'_>) -> DraftResult<ChangeRepresentationBundle> {
    let revision = inputs.revision;
    let mut representations = Vec::new();
    // The rules the explanation ran under: which strategy each Resource
    // resolved to. Hashing only the neutral revision would let installing an
    // extension change what a bundle says while the digest claimed the rules
    // had not moved, which is the one thing this field exists to report.
    let mut rules: Vec<String> = vec![format!(
        "{NEUTRAL_STRATEGY}@{NEUTRAL_REPRESENTATION_REVISION}"
    )];

    for resource in &revision.touched {
        let before = inputs.accepted.get(resource);
        let after = inputs.observed.get(resource);
        let aspect = match (before, after) {
            (None, Some(_)) => "added",
            (Some(_), None) => "removed",
            _ => "modified",
        };
        let resolved = inputs.strategies.get(resource);
        let strategy_id = match resolved {
            Some(strategy) => strategy.strategy_id.qualified(),
            None => NEUTRAL_STRATEGY.to_string(),
        };
        rules.push(format!("{resource} {strategy_id}"));

        let representation_id = format!("{}/{resource}", revision.id);
        let mut labels = BTreeMap::from([("aspect".to_string(), aspect.to_string())]);
        if let Some(strategy) = resolved {
            labels.insert("presentation.engine".into(), strategy.engine.clone());
            if let Some(contributor) = &strategy.contributed_by {
                labels.insert("presentation.contributed_by".into(), contributor.clone());
            }
        }

        representations.push(ChangeRepresentation {
            representation_id: representation_id.clone(),
            resource_id: resource.clone(),
            strategy_id: parse_strategy(&strategy_id)?,
            // Draft built these bytes, so Draft is what the record names.
            provenance: DerivationProvenance::core(
                "draft.core/representation",
                NEUTRAL_REPRESENTATION_REVISION,
            ),
            result_contract: draft_extension_contract::SchemaRef {
                schema_id: parse_strategy(NEUTRAL_STRATEGY)?,
                revision: NEUTRAL_REPRESENTATION_REVISION,
            },
            payload: RepresentationPayload::Inline {
                document: serde_json::json!({
                    "resource": resource.to_string(),
                    "aspect": aspect,
                    "before_state": before.map(ToString::to_string),
                    "after_state": after.map(ToString::to_string),
                }),
            },
            summary: RepresentationSummary {
                metrics: BTreeMap::new(),
                labels,
            },
            // Whole-resource, and deliberately nothing narrower. Core cannot
            // say where inside a Resource the work landed, and a claim that
            // pretended otherwise would let two revisions compose on a guess.
            conflict_claims: vec![ConflictClaim {
                id: format!("{representation_id}#whole"),
                scope: ConflictScope::Whole,
            }],
            review_units: vec![ReviewUnit {
                unit_id: representation_id,
                label: Some(resource.to_string()),
                summary: BTreeMap::new(),
                conflict_claim_ids: Vec::new(),
            }],
        });
    }

    rules.sort();
    let bundle = ChangeRepresentationBundle {
        schema_version: 0,
        revision: revision.id.clone(),
        inputs: inputs.observations,
        producer: inputs.producer,
        configuration: Digest::of_bytes(rules.join("\n").as_bytes()),
        representations,
        representation_bundle_digest: String::new(),
    }
    .seal();
    bundle.validate_against(revision)?;
    Ok(bundle)
}

/// Record a bundle, converging when the same explanation is derived again.
pub fn record(store: &RepresentationStore, bundle: &ChangeRepresentationBundle) -> DraftResult<()> {
    // One revision has one explanation, and the first one stands.
    //
    // This matters because sealing the same state twice converges on the same
    // revision but not on the same observation *run*: a run's identity includes
    // when it happened, so the second seal reads the same material state
    // through different observations. Both readings are true, and the bundle
    // already recorded is the one a reviewer may have read — replacing it would
    // swap an explanation somebody acted on for one saying the same thing about
    // different provenance.
    //
    // Deliberately not last-writer-wins. The store still refuses a *different*
    // explanation written directly beneath the same revision; what this decides
    // is only that a re-seal does not re-explain.
    if store.get(&bundle.revision)?.is_some() {
        return Ok(());
    }
    store.put(bundle)
}

fn parse_strategy(value: &str) -> DraftResult<draft_extension_contract::identifier::NamespacedId> {
    draft_extension_contract::identifier::NamespacedId::parse(value)
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}
