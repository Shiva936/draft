//! Derived classification: what installed extensions say a resource *is*.
//!
//! Classification is interpretation, never observed state. Two properties
//! follow, and both are structural here rather than conventional:
//!
//! * A resource carries a **set** of classes. A Rust file is legitimately both
//!   a text document and a language source, and neither assignment makes the
//!   other ambiguous. Only two incompatible definitions of the *same* class
//!   collide, and that collision is scoped to that class alone.
//! * Nothing in this module participates in `Snapshot` or `ChangeSet` identity.
//!   Installing a classifier teaches Draft something new about unchanged state;
//!   it does not make the project different.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::contracts::{current_version, ContractId, VersionedContract};
use crate::dcg::resource::ResourceId;
use crate::dcg::state::Snapshot;
use crate::extension::{ActiveContributions, ClassCollision};
use crate::provenance::derived::{
    DerivationInputs, DerivedArtifactKind, DerivedArtifactRef, SubjectRef,
};
use crate::support::hashing::canonical_hash;
use crate::support::predicate::ResourceView;
use draft_extension_contract::NamespacedId;

/// The revision of Draft's class union and collision scoping.
///
/// Separate from every authoritative constant: a change to how classes compose
/// must never perturb the identity of the state they describe.
pub const CLASSIFICATION_AGGREGATOR_REVISION: u32 = 1;

/// One class Draft derived for one resource.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceClassAssignment {
    pub resource_id: ResourceId,
    pub class_id: NamespacedId,
    /// Every extension that assigned this class, sorted. Several agreeing
    /// publishers are recorded as several, never collapsed into one author.
    pub contributed_by: Vec<String>,
}

/// A same-class disagreement, scoped to the resource and class it affects.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedClassCollision {
    pub resource_id: ResourceId,
    pub class_id: NamespacedId,
    pub contributors: Vec<String>,
}

/// Every class assignment derived for one snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassificationBundle {
    pub schema_version: u32,
    pub inputs: DerivationInputs,
    pub aggregator_revision: u32,
    /// Sorted by `(resource_id, class_id)`.
    pub assignments: Vec<ResourceClassAssignment>,
    /// Sorted; empty when every classifier agreed.
    pub collisions: Vec<ScopedClassCollision>,
    pub classification_bundle_digest: String,
}

impl VersionedContract for ClassificationBundle {
    const CONTRACT: ContractId = ContractId::ClassificationBundle;
}

impl ClassificationBundle {
    /// Seal the canonical digest over the sorted assignments and collisions.
    pub fn seal(mut self) -> Self {
        self.assignments.sort();
        self.assignments.dedup();
        self.collisions.sort();
        self.collisions.dedup();
        self.classification_bundle_digest.clear();
        self.classification_bundle_digest = canonical_hash(&self);
        self
    }

    /// Every class assigned to one resource.
    pub fn classes_of(&self, resource_id: &ResourceId) -> std::collections::BTreeSet<NamespacedId> {
        self.assignments
            .iter()
            .filter(|assignment| &assignment.resource_id == resource_id)
            .map(|assignment| assignment.class_id.clone())
            .collect()
    }

    /// A lookup of every resource's classes, for callers that walk a whole
    /// change set.
    pub fn by_resource(&self) -> BTreeMap<ResourceId, std::collections::BTreeSet<NamespacedId>> {
        let mut grouped: BTreeMap<ResourceId, std::collections::BTreeSet<NamespacedId>> =
            BTreeMap::new();
        for assignment in &self.assignments {
            grouped
                .entry(assignment.resource_id.clone())
                .or_default()
                .insert(assignment.class_id.clone());
        }
        grouped
    }

    /// A dependency reference to this bundle, for a downstream artifact.
    pub fn reference(&self) -> DerivedArtifactRef {
        DerivedArtifactRef {
            kind: DerivedArtifactKind::ClassificationBundle,
            digest: self.classification_bundle_digest.clone(),
        }
    }
}

/// Classify every resource in a snapshot against what is currently contributed.
///
/// With nothing installed this returns an empty bundle rather than an error:
/// "no extension recognizes these resources" is a true and useful answer, and
/// the capability gap that explains it is reported separately by the caller.
pub fn classify_snapshot(
    snapshot: &Snapshot,
    contributions: &ActiveContributions,
) -> ClassificationBundle {
    let mut assignments = Vec::new();
    let mut collisions = Vec::new();

    for resource in &snapshot.resources {
        let view = ResourceView {
            locator_scheme: resource.locator.scheme.as_str(),
            locator_body: resource.locator.body.as_str(),
            media_type: resource.media_type.as_deref(),
            form: resource.form,
            attributes: &resource.attributes,
            content_size: resource.content_size,
        };
        let outcome = contributions.classes_for(&view);
        for class_id in outcome.assigned {
            assignments.push(ResourceClassAssignment {
                resource_id: resource.resource_id.clone(),
                contributed_by: contributors_of(contributions, &view, &class_id),
                class_id,
            });
        }
        for ClassCollision {
            class_id,
            contributors,
        } in outcome.collisions
        {
            collisions.push(ScopedClassCollision {
                resource_id: resource.resource_id.clone(),
                class_id,
                contributors,
            });
        }
    }

    ClassificationBundle {
        schema_version: current_version(ContractId::ClassificationBundle),
        inputs: DerivationInputs::new(
            SubjectRef::Snapshot {
                snapshot_digest: snapshot.snapshot_digest.clone(),
            },
            [],
        ),
        aggregator_revision: CLASSIFICATION_AGGREGATOR_REVISION,
        assignments,
        collisions,
        classification_bundle_digest: String::new(),
    }
    .seal()
}

/// Classify the resources one transition touches.
///
/// Works from the change set's own state summaries rather than from a snapshot,
/// which is what lets an imported Change be classified in a project that never
/// observed its states: a change set carries both sides, and that is all
/// classification needs.
pub fn classify_change_set(
    change_set: &crate::dcg::change_set::ChangeSet,
    contributions: &ActiveContributions,
) -> ClassificationBundle {
    let mut assignments = Vec::new();
    let mut collisions = Vec::new();

    for change in &change_set.resources {
        // The state it ended in, or the state it was removed from: a removal is
        // still classifiable by what it was.
        let Some(side) = change.after.as_ref().or(change.before.as_ref()) else {
            continue;
        };
        let view = ResourceView {
            locator_scheme: side.locator.scheme.as_str(),
            locator_body: side.locator.body.as_str(),
            media_type: side.media_type.as_deref(),
            form: side.form,
            attributes: &side.attributes,
            content_size: side.content_size,
        };
        let outcome = contributions.classes_for(&view);
        for class_id in outcome.assigned {
            assignments.push(ResourceClassAssignment {
                resource_id: change.resource_id.clone(),
                contributed_by: contributors_of(contributions, &view, &class_id),
                class_id,
            });
        }
        for ClassCollision {
            class_id,
            contributors,
        } in outcome.collisions
        {
            collisions.push(ScopedClassCollision {
                resource_id: change.resource_id.clone(),
                class_id,
                contributors,
            });
        }
    }

    ClassificationBundle {
        schema_version: current_version(ContractId::ClassificationBundle),
        inputs: DerivationInputs::new(
            SubjectRef::ChangeSet {
                change_set_digest: change_set.change_set_digest.clone(),
            },
            [],
        ),
        aggregator_revision: CLASSIFICATION_AGGREGATOR_REVISION,
        assignments,
        collisions,
        classification_bundle_digest: String::new(),
    }
    .seal()
}

/// Which extensions assigned one agreed class to one resource.
fn contributors_of(
    contributions: &ActiveContributions,
    view: &ResourceView<'_>,
    class_id: &NamespacedId,
) -> Vec<String> {
    let mut sources: Vec<String> = contributions
        .classifications
        .iter()
        .filter(|rule| {
            &rule.value.class_id == class_id
                && crate::support::predicate::matches_raw(&rule.value.applies_to, view)
        })
        .map(|rule| rule.extension_id.clone())
        .collect();
    sources.sort();
    sources.dedup();
    sources
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::Contributed;
    use draft_extension_contract::{
        RawResourcePredicate, ResourceClassificationRule, ResourceForm,
    };

    fn id(value: &str) -> NamespacedId {
        NamespacedId::parse(value).unwrap()
    }

    fn rule(extension: &str, class: &str, suffix: &str) -> Contributed<ResourceClassificationRule> {
        Contributed::new(
            extension,
            ResourceClassificationRule {
                class_id: id(class),
                display_name: class.to_string(),
                applies_to: RawResourcePredicate::PathSuffix {
                    suffix: suffix.to_string(),
                },
                attributes: Vec::new(),
            },
        )
    }

    fn snapshot_with(bodies: &[&str]) -> Snapshot {
        crate::dcg::state::tests_support::sealed("prj_classify", bodies)
    }

    #[test]
    fn a_resource_carries_every_class_that_recognizes_it() {
        let contributions = ActiveContributions {
            classifications: vec![
                rule("draft.text.document", "draft.text.document/document", ".rs"),
                rule("draft.language.rust", "draft.language.rust/source", ".rs"),
            ],
            ..ActiveContributions::default()
        };
        let bundle = classify_snapshot(&snapshot_with(&["src/main.rs"]), &contributions);
        let classes = bundle.classes_of(&crate::dcg::resource::resource_id_for_locator(
            "file:src/main.rs",
        ));
        assert_eq!(classes.len(), 2, "both classifiers apply: {classes:?}");
        assert!(classes.contains(&id("draft.text.document/document")));
        assert!(classes.contains(&id("draft.language.rust/source")));
        assert!(bundle.collisions.is_empty());
    }

    #[test]
    fn only_the_disputed_class_collides() {
        let mut disagreeing = rule("other.publisher", "draft.text.document/document", ".rs");
        disagreeing.value.attributes = vec![draft_extension_contract::ClassAttribute {
            name: "encoding".into(),
            value: draft_extension_contract::AttributeValue::Text("utf-16".into()),
        }];
        let contributions = ActiveContributions {
            classifications: vec![
                rule("draft.text.document", "draft.text.document/document", ".rs"),
                disagreeing,
                rule("draft.language.rust", "draft.language.rust/source", ".rs"),
            ],
            ..ActiveContributions::default()
        };
        let bundle = classify_snapshot(&snapshot_with(&["src/main.rs"]), &contributions);
        // The disputed class is withheld; the unrelated one survives.
        assert_eq!(
            bundle.classes_of(&crate::dcg::resource::resource_id_for_locator(
                "file:src/main.rs"
            )),
            [id("draft.language.rust/source")].into_iter().collect()
        );
        assert_eq!(bundle.collisions.len(), 1);
        assert_eq!(
            bundle.collisions[0].class_id,
            id("draft.text.document/document")
        );
        assert_eq!(
            bundle.collisions[0].contributors,
            ["draft.text.document", "other.publisher"]
        );
    }

    #[test]
    fn classification_is_independent_of_installation_order() {
        let forward = ActiveContributions {
            classifications: vec![
                rule("a.publisher", "a.publisher/document", ".rs"),
                rule("b.publisher", "b.publisher/source", ".rs"),
            ],
            ..ActiveContributions::default()
        };
        let reversed = ActiveContributions {
            classifications: forward.classifications.iter().rev().cloned().collect(),
            ..ActiveContributions::default()
        };
        let snapshot = snapshot_with(&["src/main.rs", "README.md"]);
        assert_eq!(
            classify_snapshot(&snapshot, &forward).classification_bundle_digest,
            classify_snapshot(&snapshot, &reversed).classification_bundle_digest
        );
    }

    #[test]
    fn nothing_installed_classifies_nothing_and_is_not_an_error() {
        let bundle = classify_snapshot(
            &snapshot_with(&["src/main.rs"]),
            &ActiveContributions::default(),
        );
        assert!(bundle.assignments.is_empty());
        assert!(bundle.collisions.is_empty());
        assert!(!bundle.classification_bundle_digest.is_empty());
    }

    #[test]
    fn a_form_predicate_does_not_match_a_body_that_looks_like_a_path() {
        let contributions = ActiveContributions {
            classifications: vec![Contributed::new(
                "draft.filesystem",
                ResourceClassificationRule {
                    class_id: id("draft.filesystem/collection"),
                    display_name: "Collection".into(),
                    applies_to: RawResourcePredicate::Form {
                        equals: ResourceForm::Collection,
                    },
                    attributes: Vec::new(),
                },
            )],
            ..ActiveContributions::default()
        };
        let bundle = classify_snapshot(&snapshot_with(&["src/main.rs"]), &contributions);
        assert!(
            bundle.assignments.is_empty(),
            "a byte resource is not a collection"
        );
    }
}
