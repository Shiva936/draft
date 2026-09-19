//! What a change actually did, explained for a reviewer.
//!
//! A representation is the explanation of one revision: what changed, where,
//! and what a reader has to weigh. It sits in `evidence` rather than in the
//! graph because it carries **production provenance** — which producer made
//! this result, under which schema and artifact — and provenance names a
//! producer that only exists because something is installed. The graph itself
//! must stay readable with nothing installed at all.
//!
//! # It binds an exact revision
//!
//! Like the Evidence and Assessments beside it, a representation names one
//! `RevisionPackId` and never carries to another. Two revisions can share a
//! change set — a reseal that touched nothing material — while differing in
//! everything a reviewer weighed, so "explains the same bytes" is not
//! "explains the same revision".

use std::collections::{BTreeMap, BTreeSet};

use draft_dcg_contract::ids::RevisionPackId;
use draft_dcg_contract::observation::ObservationRef;
use draft_dcg_contract::producer::ProducerIdentity;
use draft_dcg_contract::Digest;
use draft_extension_contract::identifier::NamespacedId;
use draft_extension_contract::SchemaRef;
use serde::{Deserialize, Serialize};

use crate::dcg::representation::{
    reconcile, ClaimRelation, ConflictClaim, RepresentationPayload, RepresentationSummary,
    ReviewUnit, MAX_INLINE_PAYLOAD_BYTES,
};
use crate::dcg::resource::ResourceId;
use crate::dcg::revision_pack::RevisionPack;
use crate::provenance::derived::DerivationProvenance;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing;

/// One derived explanation of how one resource changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionPackRepresentation {
    pub representation_id: String,
    pub resource_id: ResourceId,
    /// Contributed and namespaced.
    pub strategy_id: NamespacedId,
    /// Who produced this result, so it stays verifiable after that producer is
    /// updated or removed.
    ///
    /// A sum, because the two cases are genuinely different: a contributed
    /// strategy names the exact package and schema behind it, and the neutral
    /// rendering — which always exists and no extension contributes — names
    /// Draft's own derivation revision. Forcing the second into an extension
    /// shape would invent an author for it.
    pub provenance: DerivationProvenance,
    pub result_contract: SchemaRef,
    pub payload: RepresentationPayload,
    #[serde(default)]
    pub summary: RepresentationSummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_claims: Vec<ConflictClaim>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_units: Vec<ReviewUnit>,
}

/// Every representation derived for one exact sealed revision.
///
/// # Why the revision, and not the transition
///
/// Two revisions can carry the same material transition — a reseal that
/// touched nothing — while differing in everything a reviewer weighed. An
/// explanation bound to the transition would silently transfer between them,
/// which is exactly what Evidence and Assessments bind an exact revision to
/// prevent. All three now answer "about which revision?" the same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionPackRepresentationBundle {
    pub schema_version: u32,
    /// The exact revision this explains.
    pub revision_pack: RevisionPackId,
    /// The exact observations the explanation read, id and digest.
    ///
    /// Re-observing the same state later produces a different historical
    /// observation, and an explanation naming only ids would silently re-point
    /// at it.
    pub inputs: BTreeSet<ObservationRef>,
    /// Who derived the bundle as a whole.
    pub producer: ProducerIdentity,
    /// The digest of the strategy selection this ran under.
    ///
    /// A representation produced while one extension claimed a resource is a
    /// different explanation from one produced after another took over, and a
    /// reader comparing two bundles needs to know whether the rules moved.
    pub configuration: Digest,
    pub representations: Vec<RevisionPackRepresentation>,
    pub representation_bundle_digest: String,
}

impl crate::contracts::VersionedContract for RevisionPackRepresentationBundle {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::RevisionPackRepresentationBundle;
}

impl RevisionPackRepresentationBundle {
    /// Seal a bundle, deriving its own identity.
    ///
    /// Note what this does *not* touch: the revision it names. A bundle is an
    /// explanation of sealed work, and explanations do not get to redefine the
    /// thing explained.
    pub fn seal(mut self) -> Self {
        self.schema_version = crate::contracts::current_version(
            crate::contracts::ContractId::RevisionPackRepresentationBundle,
        );
        self.representations.sort_by(|left, right| {
            (&left.resource_id, &left.strategy_id).cmp(&(&right.resource_id, &right.strategy_id))
        });
        self.representation_bundle_digest = String::new();
        let digest = hashing::canonical_hash(&self);
        self.representation_bundle_digest = digest;
        self
    }

    /// The representation for one resource, if any was derived.
    pub fn for_resource(&self, resource_id: &ResourceId) -> Option<&RevisionPackRepresentation> {
        self.representations
            .iter()
            .find(|representation| &representation.resource_id == resource_id)
    }

    /// Every contributed metric, summed across representations.
    ///
    /// Keys stay contributed: this only makes them reachable by a rule or a
    /// budget that names one.
    pub fn metrics(&self) -> BTreeMap<String, i64> {
        let mut totals: BTreeMap<String, i64> = BTreeMap::new();
        for representation in &self.representations {
            for (key, value) in &representation.summary.metrics {
                *totals.entry(key.clone()).or_default() += value;
            }
        }
        totals
    }

    /// Reject a bundle that does not explain the revision it names.
    pub fn validate_against(&self, revision: &RevisionPack) -> DraftResult<()> {
        if self.revision_pack != revision.id {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "representation bundle explains a different revision",
            ));
        }
        for representation in &self.representations {
            if !revision.touched.contains(&representation.resource_id) {
                return Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "representation explains resource {} which revision {} did not touch",
                        representation.resource_id, revision.id
                    ),
                ));
            }
            if let RepresentationPayload::Inline { document } = &representation.payload {
                let encoded = hashing::canonical_json(document);
                if encoded.len() > MAX_INLINE_PAYLOAD_BYTES {
                    return Err(DraftError::new(
                        DraftErrorKind::Validation,
                        format!(
                            "inline representation payload for {} exceeds {MAX_INLINE_PAYLOAD_BYTES} bytes",
                            representation.resource_id
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// How two change sets relate over the resources they both touch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceInterference {
    pub resource_id: ResourceId,
    pub relation: InterferenceRelation,
    pub detail: String,
}

/// The persisted form of [`ClaimRelation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterferenceRelation {
    Independent,
    Conflicting,
    Indeterminate,
}

/// Decide how two revisions interfere, resource by resource.
///
/// Only the resources both sides touched are considered — silence is the
/// answer for the rest, and listing them would bury the ones that actually
/// interfere.
///
/// Where both sides derived a representation, the claim algebra decides. Where
/// either did not, Draft falls back to whole-resource state: two revisions
/// that both touch a resource without explaining where cannot be shown
/// separable, so they are reported as interfering rather than assumed
/// composable.
pub fn interference(
    left_touched: &BTreeSet<ResourceId>,
    left_bundle: Option<&RevisionPackRepresentationBundle>,
    right_touched: &BTreeSet<ResourceId>,
    right_bundle: Option<&RevisionPackRepresentationBundle>,
) -> Vec<ResourceInterference> {
    let mut findings = Vec::new();
    for resource in left_touched.intersection(right_touched) {
        let left_claims = left_bundle
            .and_then(|bundle| bundle.for_resource(resource))
            .map(|representation| representation.conflict_claims.as_slice())
            .unwrap_or_default();
        let right_claims = right_bundle
            .and_then(|bundle| bundle.for_resource(resource))
            .map(|representation| representation.conflict_claims.as_slice())
            .unwrap_or_default();

        let relation = reconcile(left_claims, right_claims);
        let (kind, detail) = match &relation {
            ClaimRelation::Independent => (InterferenceRelation::Independent, String::new()),
            ClaimRelation::Conflicting { reason } => {
                (InterferenceRelation::Conflicting, reason.clone())
            }
            ClaimRelation::Indeterminate { reason } => {
                (InterferenceRelation::Indeterminate, reason.clone())
            }
        };
        if matches!(kind, InterferenceRelation::Independent) {
            continue;
        }
        findings.push(ResourceInterference {
            resource_id: resource.clone(),
            relation: kind,
            detail,
        });
    }
    findings.sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
    findings
}

/// Create-once storage for revision-bound representation bundles.
pub struct RepresentationStore {
    facts: crate::support::immutable_store::ImmutableFactStore<RevisionPackRepresentationBundle>,
}

impl RepresentationStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            facts: crate::support::immutable_store::ImmutableFactStore::new(directory),
        }
    }

    /// Record the representation of a revision.
    ///
    /// Keyed by revision: one revision has one explanation. A second,
    /// different explanation of the same revision is refused rather than
    /// layered, because a reviewer who read one and a reader who later sees
    /// the other would disagree about what the change was, with nothing to say
    /// which they saw.
    pub fn put(&self, bundle: &RevisionPackRepresentationBundle) -> DraftResult<()> {
        self.facts.put(bundle.revision_pack.as_str(), bundle)?;
        Ok(())
    }

    pub fn get(
        &self,
        revision: &RevisionPackId,
    ) -> DraftResult<Option<RevisionPackRepresentationBundle>> {
        self.facts.get(revision.as_str())
    }

    /// Every revision this project has an explanation for.
    pub fn list(&self) -> DraftResult<Vec<RevisionPackRepresentationBundle>> {
        let mut bundles = Vec::new();
        for id in self.facts.list_ids()? {
            if let Some(bundle) = self.facts.get(&id)? {
                bundles.push(bundle);
            }
        }
        bundles.sort_by(|left, right| {
            left.revision_pack
                .as_str()
                .cmp(right.revision_pack.as_str())
        });
        Ok(bundles)
    }
}
