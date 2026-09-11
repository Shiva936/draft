//! Proof coverage: which resources a body of evidence actually speaks about.
//!
//! # Why this is conservative to the point of seeming unhelpful
//!
//! "Is this resource covered?" is a question people act on. A reviewer sees
//! `covered` and stops looking; a gate sees `covered` and lets a change
//! through. So a wrong `covered` is worse than a missing one — it does not
//! merely fail to help, it actively stops the checking that would have caught
//! the problem.
//!
//! Everything that looks like coverage but is not is therefore rejected:
//!
//! ```text
//! same directory                 → NOT coverage. Layout is a filing habit.
//! imported / depended upon       → NOT coverage. Using a thing does not test it.
//! adjacent in the graph          → NOT coverage. An edge is not an assertion.
//! reachable from something tested → NOT coverage. Reachability is not exercise.
//! named similarly                → NOT coverage. Names are a convention.
//! ```
//!
//! Each of those is a plausible heuristic, and each would produce a confident
//! `covered` for a resource nothing has ever verified. The distance between
//! "related to something tested" and "tested" is exactly the distance between
//! a passing review and an outage.
//!
//! # The two things that do count
//!
//! **Direct** coverage requires an explicit binding: this evidence names this
//! resource. Nothing weaker.
//!
//! **Indirect** coverage requires somebody to have *said so* — either a
//! declared coverage relationship in the graph, or an attestation from the
//! producer that generated the evidence. Both are assertions somebody is
//! accountable for, which is what separates them from the inferences above.
//!
//! Indirect is reported as its own answer rather than folded into `covered`,
//! because "something asserts this is covered" and "this evidence names this
//! resource" carry different weight, and the reader deciding whether to look
//! closer needs to know which one they have.

use std::collections::{BTreeMap, BTreeSet};

use draft_dcg_contract::identifier::NamespacedId;
use draft_dcg_contract::ids::{EvidenceId, ResourceId};

/// How a resource came to be considered covered.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(tag = "basis", rename_all = "snake_case")]
pub enum CoverageBasis {
    /// The evidence explicitly names this resource.
    DirectBinding { evidence: EvidenceId },
    /// A declared relationship in the graph asserts that evidence about one
    /// resource covers another.
    DeclaredRelationship {
        evidence: EvidenceId,
        /// The resource the evidence directly binds, which the relationship
        /// extends from.
        via: ResourceId,
        /// The relationship kind, kept so a reader can judge the claim rather
        /// than take "indirect" on trust.
        relationship: NamespacedId,
    },
    /// The producer that generated the evidence attested that it covers this
    /// resource.
    ProducerAttestation {
        evidence: EvidenceId,
        producer: NamespacedId,
    },
}

impl CoverageBasis {
    /// Whether this basis is a direct binding.
    pub fn is_direct(&self) -> bool {
        matches!(self, Self::DirectBinding { .. })
    }

    /// The evidence this basis rests on.
    pub fn evidence(&self) -> &EvidenceId {
        match self {
            Self::DirectBinding { evidence }
            | Self::DeclaredRelationship { evidence, .. }
            | Self::ProducerAttestation { evidence, .. } => evidence,
        }
    }
}

/// What is known about one resource.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "coverage", rename_all = "snake_case")]
pub enum ResourceCoverage {
    /// At least one piece of evidence names this resource.
    Direct { bases: Vec<CoverageBasis> },
    /// No evidence names it, but something asserts it is covered.
    Indirect { bases: Vec<CoverageBasis> },
    /// Nothing names it and nothing asserts it.
    ///
    /// The honest answer for a resource that merely sits near, imports, or is
    /// reachable from something that *is* covered.
    Uncovered,
}

impl ResourceCoverage {
    /// Whether a gate that requires proof may treat this as proven.
    ///
    /// Only `Direct` qualifies. Indirect coverage is a claim worth surfacing
    /// to a person, not a substitute for evidence that names the thing.
    pub fn is_proven(&self) -> bool {
        matches!(self, Self::Direct { .. })
    }

    /// The bases behind this answer, empty when uncovered.
    pub fn bases(&self) -> &[CoverageBasis] {
        match self {
            Self::Direct { bases } | Self::Indirect { bases } => bases,
            Self::Uncovered => &[],
        }
    }
}

/// The inputs, all of which are assertions somebody made.
///
/// There is deliberately no dependency graph, no directory tree and no
/// resource-name list in this type. A field that is not here cannot be used as
/// a coverage signal by accident, which is a stronger guarantee than a rule
/// saying it must not be.
#[derive(Debug, Clone, Default)]
pub struct CoverageInputs {
    /// Evidence → the resources it explicitly names.
    pub direct_bindings: BTreeMap<EvidenceId, BTreeSet<ResourceId>>,
    /// Declared relationships: evidence about `from` also covers `to`.
    pub declared_relationships: Vec<DeclaredCoverage>,
    /// Producer attestations: this producer's evidence covers these resources.
    pub producer_attestations: Vec<ProducerAttestation>,
}

/// A declared assertion that covering one resource covers another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredCoverage {
    pub from: ResourceId,
    pub to: ResourceId,
    pub relationship: NamespacedId,
}

/// A producer's own claim about what its evidence covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducerAttestation {
    pub evidence: EvidenceId,
    pub producer: NamespacedId,
    pub covers: BTreeSet<ResourceId>,
}

/// Resolve coverage for `resources`.
///
/// # Why declared relationships are followed exactly one step
///
/// A chain of relationships would let coverage travel arbitrarily far from the
/// evidence that started it, and each hop weakens the claim while the answer
/// stays the same word. One hop keeps every `Indirect` answer traceable to a
/// single assertion a person can read and disagree with.
///
/// Extending this to transitive closure would be the same mistake as inferring
/// from reachability, just spelled with more ceremony.
pub fn resolve(
    inputs: &CoverageInputs,
    resources: &BTreeSet<ResourceId>,
) -> BTreeMap<ResourceId, ResourceCoverage> {
    let mut answers = BTreeMap::new();

    for resource in resources {
        let mut direct = Vec::new();
        let mut indirect = Vec::new();

        for (evidence, bound) in &inputs.direct_bindings {
            if bound.contains(resource) {
                direct.push(CoverageBasis::DirectBinding {
                    evidence: evidence.clone(),
                });
            }
        }

        // One hop, from a resource the evidence directly binds.
        for declared in &inputs.declared_relationships {
            if &declared.to != resource {
                continue;
            }
            for (evidence, bound) in &inputs.direct_bindings {
                if bound.contains(&declared.from) {
                    indirect.push(CoverageBasis::DeclaredRelationship {
                        evidence: evidence.clone(),
                        via: declared.from.clone(),
                        relationship: declared.relationship.clone(),
                    });
                }
            }
        }

        for attestation in &inputs.producer_attestations {
            if attestation.covers.contains(resource) {
                indirect.push(CoverageBasis::ProducerAttestation {
                    evidence: attestation.evidence.clone(),
                    producer: attestation.producer.clone(),
                });
            }
        }

        // Direct wins outright rather than merging: a resource the evidence
        // names is proven, and listing the weaker claims alongside would
        // invite a reader to treat the pile as stronger than its best member.
        let answer = if !direct.is_empty() {
            direct.sort();
            direct.dedup();
            ResourceCoverage::Direct { bases: direct }
        } else if !indirect.is_empty() {
            indirect.sort();
            indirect.dedup();
            ResourceCoverage::Indirect { bases: indirect }
        } else {
            ResourceCoverage::Uncovered
        };
        answers.insert(resource.clone(), answer);
    }

    answers
}

/// The resources in `resources` that no evidence names.
///
/// Indirect coverage does not exclude a resource from this list. A gate asking
/// "what is unproven?" must see everything nothing has named, or the list is
/// answering a different question than the one asked.
pub fn unproven(inputs: &CoverageInputs, resources: &BTreeSet<ResourceId>) -> BTreeSet<ResourceId> {
    resolve(inputs, resources)
        .into_iter()
        .filter(|(_, coverage)| !coverage.is_proven())
        .map(|(resource, _)| resource)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(suffix: &str) -> ResourceId {
        ResourceId::parse(format!("res_{suffix}")).unwrap()
    }

    fn evidence(suffix: &str) -> EvidenceId {
        EvidenceId::parse(format!("evd_{suffix}")).unwrap()
    }

    fn kind(name: &str) -> NamespacedId {
        NamespacedId::parse(name).unwrap()
    }

    fn asked(resources: &[ResourceId]) -> BTreeSet<ResourceId> {
        resources.iter().cloned().collect()
    }

    fn bound(evidence_id: EvidenceId, resources: &[ResourceId]) -> CoverageInputs {
        CoverageInputs {
            direct_bindings: [(evidence_id, resources.iter().cloned().collect())]
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn an_explicit_binding_is_the_only_thing_that_proves_coverage() {
        let inputs = bound(evidence("00000000000a"), &[resource("00000000000a")]);
        let answers = resolve(&inputs, &asked(&[resource("00000000000a")]));
        assert!(answers[&resource("00000000000a")].is_proven());
    }

    #[test]
    fn a_resource_that_merely_relates_to_a_covered_one_is_uncovered() {
        // The whole point of the module. Nothing here declares coverage, so
        // no arrangement of the inputs may produce anything but Uncovered —
        // proximity, dependency, adjacency and reachability all live in data
        // this function never receives.
        let inputs = bound(evidence("00000000000a"), &[resource("00000000000a")]);
        let answers = resolve(&inputs, &asked(&[resource("00000000000b")]));
        assert_eq!(
            answers[&resource("00000000000b")],
            ResourceCoverage::Uncovered
        );
    }

    #[test]
    fn a_declared_relationship_yields_indirect_never_direct() {
        // Somebody asserted it, so it is reportable — and it is still not
        // proof, because no evidence names this resource.
        let inputs = CoverageInputs {
            declared_relationships: vec![DeclaredCoverage {
                from: resource("00000000000a"),
                to: resource("00000000000b"),
                relationship: kind("draft.core/covers"),
            }],
            ..bound(evidence("00000000000a"), &[resource("00000000000a")])
        };
        let answers = resolve(&inputs, &asked(&[resource("00000000000b")]));
        match &answers[&resource("00000000000b")] {
            ResourceCoverage::Indirect { bases } => {
                assert_eq!(bases.len(), 1);
                assert!(!bases[0].is_direct());
            }
            other => panic!("expected Indirect, got {other:?}"),
        }
        assert!(!answers[&resource("00000000000b")].is_proven());
    }

    #[test]
    fn a_relationship_whose_source_is_itself_uncovered_asserts_nothing() {
        // The relationship exists but nothing sits behind it. An
        // implementation that trusted the edge alone would report coverage
        // derived from no evidence at all.
        let inputs = CoverageInputs {
            declared_relationships: vec![DeclaredCoverage {
                from: resource("00000000000a"),
                to: resource("00000000000b"),
                relationship: kind("draft.core/covers"),
            }],
            ..Default::default()
        };
        let answers = resolve(&inputs, &asked(&[resource("00000000000b")]));
        assert_eq!(
            answers[&resource("00000000000b")],
            ResourceCoverage::Uncovered
        );
    }

    #[test]
    fn declared_relationships_do_not_chain() {
        // a → b → c, with evidence naming only a. `c` stays uncovered:
        // following the chain would let coverage travel arbitrarily far while
        // the answer kept the same word.
        let inputs = CoverageInputs {
            declared_relationships: vec![
                DeclaredCoverage {
                    from: resource("00000000000a"),
                    to: resource("00000000000b"),
                    relationship: kind("draft.core/covers"),
                },
                DeclaredCoverage {
                    from: resource("00000000000b"),
                    to: resource("00000000000c"),
                    relationship: kind("draft.core/covers"),
                },
            ],
            ..bound(evidence("00000000000a"), &[resource("00000000000a")])
        };
        let answers = resolve(
            &inputs,
            &asked(&[resource("00000000000b"), resource("00000000000c")]),
        );
        assert!(matches!(
            answers[&resource("00000000000b")],
            ResourceCoverage::Indirect { .. }
        ));
        assert_eq!(
            answers[&resource("00000000000c")],
            ResourceCoverage::Uncovered
        );
    }

    #[test]
    fn a_producer_attestation_yields_indirect() {
        let inputs = CoverageInputs {
            producer_attestations: vec![ProducerAttestation {
                evidence: evidence("00000000000a"),
                producer: kind("draft.core/verification"),
                covers: [resource("00000000000b")].into_iter().collect(),
            }],
            ..Default::default()
        };
        let answers = resolve(&inputs, &asked(&[resource("00000000000b")]));
        assert!(matches!(
            answers[&resource("00000000000b")],
            ResourceCoverage::Indirect { .. }
        ));
    }

    #[test]
    fn direct_coverage_is_not_diluted_by_weaker_claims_alongside_it() {
        // A pile of assertions is not stronger than its best member, and
        // listing them together would invite a reader to think otherwise.
        let inputs = CoverageInputs {
            producer_attestations: vec![ProducerAttestation {
                evidence: evidence("00000000000b"),
                producer: kind("draft.core/verification"),
                covers: [resource("00000000000a")].into_iter().collect(),
            }],
            ..bound(evidence("00000000000a"), &[resource("00000000000a")])
        };
        match &answers_for(&inputs, resource("00000000000a")) {
            ResourceCoverage::Direct { bases } => {
                assert!(bases.iter().all(CoverageBasis::is_direct));
            }
            other => panic!("expected Direct, got {other:?}"),
        }
    }

    #[test]
    fn indirect_coverage_never_removes_a_resource_from_the_unproven_list() {
        // A gate asking "what is unproven?" must see everything no evidence
        // names, or it is being answered a different question.
        let inputs = CoverageInputs {
            producer_attestations: vec![ProducerAttestation {
                evidence: evidence("00000000000a"),
                producer: kind("draft.core/verification"),
                covers: [resource("00000000000b")].into_iter().collect(),
            }],
            ..bound(evidence("00000000000a"), &[resource("00000000000a")])
        };
        assert_eq!(
            unproven(
                &inputs,
                &asked(&[resource("00000000000a"), resource("00000000000b")])
            ),
            asked(&[resource("00000000000b")])
        );
    }

    #[test]
    fn asking_about_nothing_answers_nothing() {
        let inputs = bound(evidence("00000000000a"), &[resource("00000000000a")]);
        assert!(resolve(&inputs, &BTreeSet::new()).is_empty());
        assert!(unproven(&inputs, &BTreeSet::new()).is_empty());
    }

    fn answers_for(inputs: &CoverageInputs, resource: ResourceId) -> ResourceCoverage {
        resolve(inputs, &asked(std::slice::from_ref(&resource)))
            .remove(&resource)
            .unwrap()
    }
}
