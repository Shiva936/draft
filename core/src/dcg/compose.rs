//! Baseline composition: what established accepted state, and what could act now.
//!
//! Two questions that look similar and must never be conflated:
//!
//! | | Answers | ChangePacks when |
//! |---|---|---|
//! | [`HistoricalBaselineComposition`] | what established this accepted state? | never |
//! | [`CurrentProviderRoutability`] | could that provider act right now? | the binding moves |
//!
//! Composition is derived from each Resource's **primary observation**, and it
//! yields a [`ProviderProvenanceRef`] — a binding and the semantic definition
//! in force when the observation was made. It never yields a route. Including
//! the operational profile would make re-tuning how a provider is driven look
//! like a change in accepted history, when it changed nothing about what was
//! observed.
//!
//! # Why they are separate types
//!
//! A single "provider status" would have to answer both at once, and every
//! caller would silently get whichever the implementation happened to compute.
//! Rendering an unbound provider as though it had never established anything
//! would erase real history; rendering a historical composition as though the
//! provider were still usable would invite an action that must be refused.
//!
//! So a Baseline's composition is immutable and answers only the first, while
//! routability is recomputed and answers only the second — and a caller has to
//! say which one it wanted.

use std::collections::{BTreeMap, BTreeSet};

use draft_dcg_contract::ids::{ProviderBindingId, ResourceId};
use draft_dcg_contract::{
    BaselineStateEvidenceEntry, ProviderProvenanceRef, ProviderSemanticDefinitionDigest,
    ResourceStateDigest,
};

use crate::project::provider::{ProviderBinding, ProviderBindingLifecycle};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// What established a Baseline's accepted state.
///
/// Immutable. Derived once from the evidence root's primary observations, and
/// unaffected by anything that happens to a binding afterwards.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalBaselineComposition {
    /// The provenance behind each Resource's accepted state.
    pub resource_provenance: BTreeMap<ResourceId, ProviderProvenanceRef>,
    /// The exact state each Resource was accepted at.
    ///
    /// Derived from the same evidence entries as the provenance, so the two
    /// can never disagree about what a Baseline accepted. Recorded because a
    /// Baseline's roots prove the whole state at once and say nothing about
    /// any one Resource — and "what did this revision actually change" is a
    /// per-Resource question.
    pub accepted_state: BTreeMap<ResourceId, ResourceStateDigest>,
}

impl HistoricalBaselineComposition {
    /// Derive composition from a Baseline's state evidence.
    ///
    /// Only Resource entries contribute: a Relation's evidence establishes that
    /// an edge is state-bearing, which is a different claim from which provider
    /// observed a Resource.
    pub fn derive<'a>(
        evidence: impl IntoIterator<Item = &'a BaselineStateEvidenceEntry>,
        provenance_of: impl Fn(
            &draft_dcg_contract::ObservationRef,
        ) -> DraftResult<ProviderProvenanceRef>,
    ) -> DraftResult<Self> {
        let mut resource_provenance = BTreeMap::new();
        let mut accepted_state = BTreeMap::new();
        for entry in evidence {
            let BaselineStateEvidenceEntry::Resource {
                resource_id,
                state,
                primary,
                ..
            } = entry
            else {
                continue;
            };
            accepted_state.insert(resource_id.clone(), state.clone());
            // The primary observation alone. A corroborating observation proves
            // the same state, but the accepted state was established by one
            // observation and provenance must name that one.
            let provenance = provenance_of(primary)?;
            if let Some(existing) = resource_provenance.insert(resource_id.clone(), provenance) {
                return Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "resource '{resource_id}' appears twice in state evidence, so its \
                         provenance is ambiguous (first was {:?})",
                        existing.binding
                    ),
                ));
            }
        }
        Ok(Self {
            resource_provenance,
            accepted_state,
        })
    }

    /// The state `resource` was accepted at, if the Baseline holds it.
    pub fn accepted_state_of(&self, resource: &ResourceId) -> Option<&ResourceStateDigest> {
        self.accepted_state.get(resource)
    }

    /// Every binding that contributed to this Baseline.
    pub fn contributing_bindings(&self) -> BTreeSet<ProviderBindingId> {
        self.resource_provenance
            .values()
            .map(|provenance| provenance.binding.clone())
            .collect()
    }

    /// The semantic definition a binding contributed under, if it contributed.
    pub fn definition_for(
        &self,
        binding: &ProviderBindingId,
    ) -> Option<&ProviderSemanticDefinitionDigest> {
        self.resource_provenance
            .values()
            .find(|provenance| &provenance.binding == binding)
            .map(|provenance| &provenance.semantic_definition)
    }
}

/// Why a binding cannot be used for new work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotRoutable {
    /// Withdrawn by an operator.
    Unbound,
    /// The binding no longer exists.
    Absent,
}

impl std::fmt::Display for NotRoutable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unbound => "unbound",
            Self::Absent => "no longer configured",
        })
    }
}

/// Whether a provider could act right now.
///
/// Recomputed from current binding state, and deliberately carries no
/// historical claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentProviderRoutability {
    pub binding: ProviderBindingId,
    /// `None` when the binding is usable; otherwise why it is not.
    pub refusal: Option<NotRoutable>,
    /// The definition it currently selects, when it is still configured.
    pub current_semantic_definition: Option<ProviderSemanticDefinitionDigest>,
}

impl CurrentProviderRoutability {
    /// Assess a binding as it stands now.
    pub fn of(binding: &ProviderBinding) -> Self {
        Self {
            binding: binding.id.clone(),
            refusal: match binding.lifecycle {
                ProviderBindingLifecycle::Active => None,
                ProviderBindingLifecycle::Unbound => Some(NotRoutable::Unbound),
            },
            current_semantic_definition: Some(binding.current_semantic_definition.clone()),
        }
    }

    /// A binding that no longer exists.
    pub fn absent(binding: ProviderBindingId) -> Self {
        Self {
            binding,
            refusal: Some(NotRoutable::Absent),
            current_semantic_definition: None,
        }
    }

    pub fn is_routable(&self) -> bool {
        self.refusal.is_none()
    }

    /// Whether the binding has moved since it contributed to a Baseline.
    ///
    /// Informational. A moved binding does not invalidate history — it means a
    /// new plan is needed, which is a different statement entirely.
    pub fn has_moved_since(&self, historical: &ProviderProvenanceRef) -> bool {
        self.current_semantic_definition
            .as_ref()
            .is_some_and(|current| current != &historical.semantic_definition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::ids::{ObservationId, ProjectId};
    use draft_dcg_contract::{
        Digest, ObservationDigest, ObservationRef, ProviderKindId,
        ProviderOperationalProfileDigest, ResourceStateDigest,
    };

    fn resource(name: &str) -> ResourceId {
        ResourceId::parse(format!("res_{name}")).unwrap()
    }

    fn observation(name: &str) -> ObservationRef {
        ObservationRef {
            id: ObservationId::parse(format!("obs_{name}")).unwrap(),
            digest: ObservationDigest::new(Digest::of_bytes(name.as_bytes())),
        }
    }

    fn binding_id(name: &str) -> ProviderBindingId {
        ProviderBindingId::parse(format!("pbd_{name}")).unwrap()
    }

    fn definition(seed: &[u8]) -> ProviderSemanticDefinitionDigest {
        ProviderSemanticDefinitionDigest::new(Digest::of_bytes(seed))
    }

    fn entry(name: &str, observation_name: &str) -> BaselineStateEvidenceEntry {
        BaselineStateEvidenceEntry::Resource {
            resource_id: resource(name),
            state: ResourceStateDigest::new(Digest::of_bytes(name.as_bytes())),
            primary: observation(observation_name),
            corroborating: BTreeSet::new(),
        }
    }

    fn provenance_from(binding: &str, seed: &[u8]) -> ProviderProvenanceRef {
        ProviderProvenanceRef {
            binding: binding_id(binding),
            semantic_definition: definition(seed),
        }
    }

    fn binding(lifecycle: ProviderBindingLifecycle, seed: &[u8]) -> ProviderBinding {
        ProviderBinding {
            generation: 0,
            id: binding_id("aaa"),
            project: ProjectId::parse("prj_000000000001").unwrap(),
            kind: ProviderKindId::parse("draft.filesystem/local").unwrap(),
            current_semantic_definition: definition(seed),
            current_operational_profile: ProviderOperationalProfileDigest::new(Digest::of_bytes(
                b"OP1",
            )),
            lifecycle,
        }
    }

    #[test]
    fn composition_names_the_primary_observations_provenance() {
        let entries = vec![entry("aaa", "one"), entry("bbb", "two")];
        let composition =
            HistoricalBaselineComposition::derive(&entries, |_| Ok(provenance_from("aaa", b"SD1")))
                .unwrap();

        assert_eq!(composition.resource_provenance.len(), 2);
        assert_eq!(
            composition.contributing_bindings(),
            BTreeSet::from([binding_id("aaa")])
        );
    }

    #[test]
    fn relation_evidence_does_not_contribute_provenance() {
        // A Relation's evidence establishes that an edge is state-bearing,
        // which is a different claim from which provider observed a Resource.
        let entries = vec![
            entry("aaa", "one"),
            BaselineStateEvidenceEntry::Relation {
                state: draft_dcg_contract::RelationStateDigest::new(Digest::of_bytes(b"edge")),
                evidence: BTreeSet::from([
                    draft_dcg_contract::RelationStateEvidenceRef::RelationRecord {
                        record: draft_dcg_contract::RelationRecordDigest::new(Digest::of_bytes(
                            b"record",
                        )),
                    },
                ]),
            },
        ];
        let composition =
            HistoricalBaselineComposition::derive(&entries, |_| Ok(provenance_from("aaa", b"SD1")))
                .unwrap();
        assert_eq!(composition.resource_provenance.len(), 1);
    }

    #[test]
    fn a_duplicated_resource_makes_provenance_ambiguous_and_is_refused() {
        let entries = vec![entry("aaa", "one"), entry("aaa", "two")];
        let error =
            HistoricalBaselineComposition::derive(&entries, |_| Ok(provenance_from("aaa", b"SD1")))
                .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn composition_carries_no_operational_profile() {
        // Structural: there is nowhere in a ProviderProvenanceRef to put one,
        // which is why re-tuning a provider cannot change accepted history.
        let entries = vec![entry("aaa", "one")];
        let composition =
            HistoricalBaselineComposition::derive(&entries, |_| Ok(provenance_from("aaa", b"SD1")))
                .unwrap();
        let provenance = &composition.resource_provenance[&resource("aaa")];
        assert_eq!(provenance.semantic_definition, definition(b"SD1"));
        assert_eq!(provenance.binding, binding_id("aaa"));
    }

    #[test]
    fn unbinding_changes_routability_and_leaves_composition_untouched() {
        // The distinction the two types exist for. Rendering an unbound
        // provider as though it had never established anything would erase real
        // history.
        let entries = vec![entry("aaa", "one")];
        let composition =
            HistoricalBaselineComposition::derive(&entries, |_| Ok(provenance_from("aaa", b"SD1")))
                .unwrap();

        let active =
            CurrentProviderRoutability::of(&binding(ProviderBindingLifecycle::Active, b"SD1"));
        assert!(active.is_routable());

        let withdrawn =
            CurrentProviderRoutability::of(&binding(ProviderBindingLifecycle::Unbound, b"SD1"));
        assert!(!withdrawn.is_routable());
        assert_eq!(withdrawn.refusal, Some(NotRoutable::Unbound));

        // Composition is identical either way.
        let recomputed =
            HistoricalBaselineComposition::derive(&entries, |_| Ok(provenance_from("aaa", b"SD1")))
                .unwrap();
        assert_eq!(composition, recomputed);
    }

    #[test]
    fn a_moved_binding_is_reported_without_invalidating_history() {
        // A moved binding means a new plan is needed, which is a different
        // statement from history being wrong.
        let historical = provenance_from("aaa", b"SD1");
        let moved =
            CurrentProviderRoutability::of(&binding(ProviderBindingLifecycle::Active, b"SD2"));
        assert!(moved.is_routable(), "still usable, just for something else");
        assert!(moved.has_moved_since(&historical));

        let unchanged =
            CurrentProviderRoutability::of(&binding(ProviderBindingLifecycle::Active, b"SD1"));
        assert!(!unchanged.has_moved_since(&historical));
    }

    #[test]
    fn a_binding_that_no_longer_exists_is_still_describable() {
        let absent = CurrentProviderRoutability::absent(binding_id("aaa"));
        assert!(!absent.is_routable());
        assert_eq!(absent.refusal, Some(NotRoutable::Absent));
        assert!(absent.current_semantic_definition.is_none());
        // And it makes no claim about history.
        assert!(!absent.has_moved_since(&provenance_from("aaa", b"SD1")));
    }

    #[test]
    fn composition_reports_which_definition_a_binding_contributed_under() {
        let entries = vec![entry("aaa", "one")];
        let composition =
            HistoricalBaselineComposition::derive(&entries, |_| Ok(provenance_from("aaa", b"SD1")))
                .unwrap();
        assert_eq!(
            composition.definition_for(&binding_id("aaa")),
            Some(&definition(b"SD1"))
        );
        assert!(composition.definition_for(&binding_id("zzz")).is_none());
    }
}

// ---- ChangePack composition ----

use draft_dcg_contract::ids::{ChangePackId, RevisionPackId};
use draft_dcg_contract::BaselineId;
use serde::{Deserialize, Serialize};

use crate::support::hashing;

/// How two sealed revisions stand to each other.
///
/// Three answers rather than two, and the third is not a hedge. `Conflicting`
/// is a claim Draft can defend; `Indeterminate` is the honest answer when it
/// cannot establish separability at all — two revisions sealed from different
/// Baselines, or claim shapes Core has no way to relate. Both refuse
/// composition, and only one of them says the work overlaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relationship {
    Independent,
    Conflicting,
    Indeterminate,
}

impl Relationship {
    /// Whether this pair may be composed. Only `Independent` may.
    pub fn is_composable(self) -> bool {
        matches!(self, Self::Independent)
    }
}

/// Whether a composition holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositionStatus {
    Verified,
    Failed,
}

/// One member of a composition: the exact revision, not the ChangePack.
///
/// A ChangePack is an intention that can be resealed; a revision is what was
/// actually sealed. Composing ChangePacks rather than revisions would let a reseal
/// change what a composition claimed without the composition moving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposedRevision {
    pub change_pack: ChangePackId,
    pub revision_pack: RevisionPackId,
    /// The Baseline this revision was sealed from.
    pub base_baseline: BaselineId,
    pub touched: BTreeSet<ResourceId>,
}

/// How one pair of members stands, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairwiseRelation {
    pub left: RevisionPackId,
    pub right: RevisionPackId,
    pub relation: Relationship,
    /// Empty for `Independent`. Otherwise names what stands in the way, so a
    /// reader can judge the claim rather than take a verdict on trust.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// The Resources the pair both touched, when any.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub shared_resources: BTreeSet<ResourceId>,
}

/// Several sealed revisions, and whether they hold together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Composition {
    pub schema_version: u32,
    pub id: String,
    /// The Baseline every member must have been sealed from.
    pub base_baseline: BaselineId,
    pub members: Vec<ComposedRevision>,
    pub status: CompositionStatus,
    pub relations: Vec<PairwiseRelation>,
    pub affected_resources: BTreeSet<ResourceId>,
    pub composition_digest: String,
}

impl crate::contracts::VersionedContract for Composition {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::Composition;
}

/// Compose sealed revisions against one Baseline.
///
/// `relate` decides each pair. It is supplied rather than computed here because
/// the finest answer needs the representations that explain each revision, and
/// those are derived facts that live above the graph — the graph must stay
/// readable with nothing installed. A caller with no representations passes a
/// whole-Resource relation and gets the conservative answer.
///
/// A member sealed from a different Baseline makes the whole composition
/// `Failed`: two revisions worked from different starting points describe
/// different projects, and composing them would be a guess about what the
/// result means.
pub fn compose(
    base_baseline: &BaselineId,
    members: &[ComposedRevision],
    relate: impl Fn(&ComposedRevision, &ComposedRevision) -> PairwiseRelation,
) -> DraftResult<Composition> {
    if members.len() < 2 {
        return Err(DraftError::new(
            DraftErrorKind::Validation,
            "a composition needs at least two revisions; one revision composes with nothing",
        ));
    }
    let mut seen = BTreeSet::new();
    for member in members {
        if !seen.insert(member.revision_pack.clone()) {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "RevisionPack '{}' appears twice; a composition names each member once",
                    member.revision_pack
                ),
            ));
        }
    }

    let mut relations = Vec::new();
    let mut affected_resources = BTreeSet::new();
    let mut holds = true;
    for (index, member) in members.iter().enumerate() {
        affected_resources.extend(member.touched.iter().cloned());
        if &member.base_baseline != base_baseline {
            holds = false;
        }
        for other in &members[index + 1..] {
            let relation = relate(member, other);
            if !relation.relation.is_composable() {
                holds = false;
            }
            relations.push(relation);
        }
    }

    let mut composition = Composition {
        schema_version: crate::contracts::current_version(
            crate::contracts::ContractId::Composition,
        ),
        id: String::new(),
        base_baseline: base_baseline.clone(),
        members: members.to_vec(),
        status: if holds {
            CompositionStatus::Verified
        } else {
            CompositionStatus::Failed
        },
        relations,
        affected_resources,
        composition_digest: String::new(),
    };
    // Derived from the content, so composing the same revisions twice is the
    // same composition rather than a second one to reason about.
    composition.composition_digest = hashing::canonical_hash(&composition);
    // The hex body, not the `sha256:` wire form: an id is an identifier and
    // must stay within the identifier grammar.
    let body: String = composition
        .composition_digest
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .take(12)
        .collect();
    composition.id = format!("cmp_{body}");
    Ok(composition)
}

/// Whether one member of a composition can be advanced on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispersedRevision {
    pub change_pack: ChangePackId,
    pub revision_pack: RevisionPackId,
    /// True when nothing in the composition stands in this member's way.
    pub independent: bool,
    /// The members it cannot be separated from, and why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held_by: Vec<PairwiseRelation>,
}

/// Disperse a composition back into revisions that can move separately.
///
/// The inverse of [`compose`], and deliberately not a mutation: dispersing
/// reports which members already stand alone. A member held by another is
/// named along with the relation holding it, so the answer says what to
/// resolve rather than merely refusing.
pub fn disperse(composition: &Composition) -> Vec<DispersedRevision> {
    composition
        .members
        .iter()
        .map(|member| {
            let held_by: Vec<PairwiseRelation> = composition
                .relations
                .iter()
                .filter(|relation| {
                    !relation.relation.is_composable()
                        && (relation.left == member.revision_pack
                            || relation.right == member.revision_pack)
                })
                .cloned()
                .collect();
            DispersedRevision {
                change_pack: member.change_pack.clone(),
                revision_pack: member.revision_pack.clone(),
                independent: held_by.is_empty()
                    && member.base_baseline == composition.base_baseline,
                held_by,
            }
        })
        .collect()
}

/// The conservative relation between two revisions, from state alone.
///
/// What a caller with no representations gets: two revisions that touch one
/// Resource cannot be shown separable at this level, and are reported as
/// conflicting rather than assumed composable. Sealed from different
/// Baselines, even disjoint sets prove nothing, so the answer is
/// indeterminate — a distinction that fails closed.
pub fn relate_by_state(left: &ComposedRevision, right: &ComposedRevision) -> PairwiseRelation {
    let shared: BTreeSet<ResourceId> = left.touched.intersection(&right.touched).cloned().collect();
    let (relation, detail) = if !shared.is_empty() {
        (
            Relationship::Conflicting,
            "both revisions touch these Resources, and nothing explains where inside them"
                .to_string(),
        )
    } else if left.base_baseline == right.base_baseline {
        (Relationship::Independent, String::new())
    } else {
        (
            Relationship::Indeterminate,
            "sealed from different Baselines, so disjoint Resource sets prove nothing".to_string(),
        )
    };
    PairwiseRelation {
        left: left.revision_pack.clone(),
        right: right.revision_pack.clone(),
        relation,
        detail,
        shared_resources: shared,
    }
}
