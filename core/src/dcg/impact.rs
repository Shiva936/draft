//! A neutral index of the elements inside resources, and how they relate.
//!
//! Draft persists the model `resource -> element -> relation -> change set` and
//! answers the impact questions review needs: which elements a change touches,
//! which resources relate to them, and whether two changes reach the same
//! element.
//!
//! Nothing here interprets what an element *is*. An element has a stable id, an
//! optional namespaced kind, a name and typed attributes; a relation has a
//! namespaced kind and two endpoints. A software extension may contribute
//! `visibility=public`; an audio extension may contribute `role=stem`; a design
//! extension may contribute `layer=…`. Core stores, links and counts them, and
//! never privileges one vocabulary — in particular there is no notion of a
//! "public API" or a "reference" built in.
//!
//! Extraction itself is contributed: elements arrive from an authorized
//! extraction mechanism, keyed by the extractor that produced them, so
//! complementary extractors compose rather than competing.

use crate::contracts::ProducerRef;
use crate::dcg::resource::ResourceId;
use crate::project::layout::DraftLayout;
use crate::support::error::{DraftError, DraftResult};
use draft_extension_contract::{AttributeValue, ElementPredicate, NamespacedId, RelationDirection};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The backend identifier recorded in evidence so consumers know the fidelity.
pub const IMPACT_INDEX_BACKEND: &str = "basic";

/// The Core semantics that merge extractor results into one index.
///
/// Recorded with the index because the merge — how collisions are scoped, how
/// relations are keyed — is Draft's own logic, not the extractor's, and a change
/// to it must not silently reinterpret a stored index.
pub const IMPACT_MERGE_REVISION: u32 = 1;

/// One element inside a resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceElement {
    /// Stable within its resource, minted by the contributing extractor.
    pub element_id: String,
    pub resource_id: ResourceId,
    /// Namespaced and opaque. Core compares it; it never means anything here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<NamespacedId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Contributed facts. A software extractor might set `visibility`; nothing
    /// in Core reads any particular key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, AttributeValue>,
    pub producer: ProducerRef,
}

/// A directed relation between two elements.
///
/// `relation_kind` is namespaced and opaque: "calls", "contains", "renders",
/// "depends-on" and "is-mixed-into" are all just strings to Core, and none is
/// privileged as *the* reference relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementRelation {
    pub relation_kind: NamespacedId,
    pub from: String,
    pub to: String,
    pub producer: ProducerRef,
}

/// What one extractor produced for one resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractorResult {
    pub extractor_id: NamespacedId,
    pub resource_id: ResourceId,
    pub elements: Vec<ResourceElement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<ElementRelation>,
    pub producer: ProducerRef,
}

/// Two extractors claimed the same stable element id with different content.
///
/// Scoped: only this element is unusable, and every other element from both
/// extractors is kept. A whole-resource failure here would mean one publisher's
/// mistake could blind Draft to another's correct results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementCollision {
    pub resource_id: ResourceId,
    pub element_id: String,
    pub extractors: Vec<NamespacedId>,
}

/// The merged outcome for one resource.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergedElements {
    pub elements: Vec<ResourceElement>,
    pub relations: Vec<ElementRelation>,
    pub collisions: Vec<ElementCollision>,
}

/// Merge complementary extractor results.
///
/// Extractors compose by design: one may find structural elements while another
/// finds cross-references over the same resource. Results union by
/// `(resource_id, element_id)`; only a genuine disagreement about one id
/// collides, and it collides alone.
pub fn merge_extractor_results(results: &[ExtractorResult]) -> MergedElements {
    let mut by_element: BTreeMap<(ResourceId, String), Vec<(&NamespacedId, &ResourceElement)>> =
        BTreeMap::new();
    for result in results {
        for element in &result.elements {
            by_element
                .entry((element.resource_id.clone(), element.element_id.clone()))
                .or_default()
                .push((&result.extractor_id, element));
        }
    }

    let mut merged = MergedElements::default();
    let mut collided: BTreeSet<(ResourceId, String)> = BTreeSet::new();
    for ((resource_id, element_id), mut claimants) in by_element {
        claimants.sort_by(|left, right| left.0.cmp(right.0));
        let first = claimants[0].1;
        let agreed = claimants.iter().all(|(_, element)| {
            element.kind == first.kind
                && element.name == first.name
                && element.attributes == first.attributes
        });
        if agreed {
            merged.elements.push(first.clone());
        } else {
            collided.insert((resource_id.clone(), element_id.clone()));
            merged.collisions.push(ElementCollision {
                resource_id,
                element_id,
                extractors: claimants
                    .iter()
                    .map(|(extractor, _)| (*extractor).clone())
                    .collect(),
            });
        }
    }

    // A relation whose endpoint collided has no well-defined meaning, so it is
    // dropped with the element rather than pointing at an unresolved id.
    let collided_ids: BTreeSet<&String> = collided.iter().map(|(_, id)| id).collect();
    for result in results {
        for relation in &result.relations {
            if collided_ids.contains(&relation.from) || collided_ids.contains(&relation.to) {
                continue;
            }
            merged.relations.push(relation.clone());
        }
    }
    merged.relations.sort_by(|left, right| {
        (&left.relation_kind, &left.from, &left.to).cmp(&(
            &right.relation_kind,
            &right.from,
            &right.to,
        ))
    });
    merged.relations.dedup();
    merged
}

/// Whether an element satisfies a contributed predicate.
///
/// Attribute lookups are exact and untyped-coercion-free: a rule written for a
/// text attribute never matches a number that happens to render the same way.
pub fn element_matches(
    predicate: &ElementPredicate,
    element: &ResourceElement,
    relations: &[ElementRelation],
) -> bool {
    match predicate {
        ElementPredicate::All { of } => of.iter().all(|p| element_matches(p, element, relations)),
        ElementPredicate::Any { of } => of.iter().any(|p| element_matches(p, element, relations)),
        ElementPredicate::Not { of } => !element_matches(of, element, relations),
        ElementPredicate::KindIs { equals } => element.kind.as_ref() == Some(equals),
        ElementPredicate::AttributeIs { name, matches } => element
            .attributes
            .get(name)
            .is_some_and(|value| attribute_matches(matches, value)),
        ElementPredicate::RelationExists {
            relation_kind,
            direction,
        } => relations.iter().any(|relation| {
            &relation.relation_kind == relation_kind
                && match direction {
                    RelationDirection::Outgoing => relation.from == element.element_id,
                    RelationDirection::Incoming => relation.to == element.element_id,
                    RelationDirection::Either => {
                        relation.from == element.element_id || relation.to == element.element_id
                    }
                }
        }),
    }
}

fn attribute_matches(
    rule: &draft_extension_contract::AttributeMatch,
    value: &AttributeValue,
) -> bool {
    use draft_extension_contract::AttributeMatch as Match;
    match (rule, value) {
        (Match::Equals { value: expected }, actual) => expected == actual,
        (Match::Prefix { value: prefix }, AttributeValue::Text(text)) => text.starts_with(prefix),
        (Match::Suffix { value: suffix }, AttributeValue::Text(text)) => text.ends_with(suffix),
        (Match::Glob { pattern }, AttributeValue::Text(text)) => {
            crate::support::glob::matches(pattern, text)
        }
        (Match::Range { at_least, at_most }, AttributeValue::Integer(number)) => {
            at_least.is_none_or(|bound| *number >= bound)
                && at_most.is_none_or(|bound| *number <= bound)
        }
        _ => false,
    }
}

/// The impact index bound to a project.
pub struct ImpactIndex {
    conn: Connection,
}

impl ImpactIndex {
    /// Open (creating if needed) the project's impact database.
    pub fn open(paths: &DraftLayout) -> DraftResult<Self> {
        crate::support::fsutil::ensure_dir(&paths.impact_dir())?;
        let conn = Connection::open(paths.impact_index_db())
            .map_err(|e| DraftError::storage(format!("open impact db: {e}")))?;
        let index = ImpactIndex { conn };
        index.ensure_schema()?;
        Ok(index)
    }

    /// Open an in-memory index (tests).
    pub fn open_memory() -> DraftResult<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| DraftError::storage(format!("open impact memory db: {e}")))?;
        let index = ImpactIndex { conn };
        index.ensure_schema()?;
        Ok(index)
    }

    fn ensure_schema(&self) -> DraftResult<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS elements(
                    resource_id TEXT, element_id TEXT, kind TEXT, name TEXT,
                    extractor_id TEXT, package_digest TEXT);
                 CREATE TABLE IF NOT EXISTS element_attributes(
                    resource_id TEXT, element_id TEXT, name TEXT, value TEXT);
                 CREATE TABLE IF NOT EXISTS element_relations(
                    relation_kind TEXT, from_element TEXT, to_element TEXT,
                    extractor_id TEXT);
                 CREATE TABLE IF NOT EXISTS revision_elements(
                    revision_id TEXT, resource_id TEXT, element_id TEXT);
                 CREATE INDEX IF NOT EXISTS idx_rev_revision ON revision_elements(revision_id);
                 CREATE INDEX IF NOT EXISTS idx_rev_element ON revision_elements(element_id);
                 CREATE INDEX IF NOT EXISTS idx_rel_to ON element_relations(to_element);
                 CREATE INDEX IF NOT EXISTS idx_rel_from ON element_relations(from_element);",
            )
            .map_err(|e| DraftError::storage(format!("impact schema: {e}")))?;
        Ok(())
    }

    /// Record the merged elements a sealed revision touches.
    ///
    /// Idempotent: re-indexing the same revision replaces its rows rather than
    /// accumulating duplicates. Every table the revision contributes to is
    /// cleared first — leaving the element rows behind while replacing the
    /// membership rows would let a re-index report elements no extractor
    /// currently finds.
    pub fn index_revision(&self, revision_id: &str, merged: &MergedElements) -> DraftResult<usize> {
        let resources: BTreeSet<&str> = merged
            .elements
            .iter()
            .map(|element| element.resource_id.as_str())
            .collect();
        self.run(
            "DELETE FROM revision_elements WHERE revision_id = ?1",
            [revision_id],
        )?;
        for resource in &resources {
            self.run("DELETE FROM elements WHERE resource_id = ?1", [*resource])?;
            self.run(
                "DELETE FROM element_attributes WHERE resource_id = ?1",
                [*resource],
            )?;
        }

        for element in &merged.elements {
            self.run(
                "INSERT INTO elements(resource_id,element_id,kind,name,extractor_id,package_digest)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                rusqlite::params![
                    element.resource_id.as_str(),
                    element.element_id,
                    element.kind.as_ref().map(NamespacedId::qualified),
                    element.name,
                    element.producer.extension_id,
                    element.producer.package_digest,
                ],
            )?;
            for (name, value) in &element.attributes {
                self.run(
                    "INSERT INTO element_attributes(resource_id,element_id,name,value)
                     VALUES(?1,?2,?3,?4)",
                    rusqlite::params![
                        element.resource_id.as_str(),
                        element.element_id,
                        name,
                        crate::support::hashing::canonical_json(
                            &serde_json::to_value(value).unwrap_or_default()
                        ),
                    ],
                )?;
            }
            // The membership row. Without it `elements_touched_by` would
            // answer every question with silence, and impact review would
            // report a revision that touched a hundred elements as touching
            // none.
            self.run(
                "INSERT INTO revision_elements(revision_id,resource_id,element_id)
                 VALUES(?1,?2,?3)",
                rusqlite::params![
                    revision_id,
                    element.resource_id.as_str(),
                    element.element_id
                ],
            )?;
        }

        for relation in &merged.relations {
            self.run(
                "DELETE FROM element_relations
                 WHERE relation_kind = ?1 AND from_element = ?2 AND to_element = ?3
                   AND extractor_id = ?4",
                rusqlite::params![
                    relation.relation_kind.qualified(),
                    relation.from,
                    relation.to,
                    relation.producer.extension_id
                ],
            )?;
            self.run(
                "INSERT INTO element_relations(relation_kind,from_element,to_element,extractor_id)
                 VALUES(?1,?2,?3,?4)",
                rusqlite::params![
                    relation.relation_kind.qualified(),
                    relation.from,
                    relation.to,
                    relation.producer.extension_id
                ],
            )?;
        }
        Ok(merged.elements.len())
    }

    /// Elements a sealed revision touches.
    pub fn elements_touched_by(&self, revision_id: &str) -> DraftResult<Vec<String>> {
        self.query_column(
            "SELECT DISTINCT element_id FROM revision_elements
             WHERE revision_id = ?1 ORDER BY element_id",
            [revision_id],
        )
    }

    /// Resources reachable from `elements` through any recorded relation.
    ///
    /// Direction-agnostic on purpose: which way a domain's relation points is the
    /// domain's business, and impact review wants everything connected either
    /// way.
    pub fn resources_related_by_relations(
        &self,
        elements: &[String],
    ) -> DraftResult<Vec<ResourceId>> {
        if elements.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = elements.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT DISTINCT e.resource_id FROM elements e
             JOIN element_relations r
               ON e.element_id = r.from_element OR e.element_id = r.to_element
             WHERE (r.to_element IN ({placeholders}) OR r.from_element IN ({placeholders}))
             ORDER BY e.resource_id"
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::new();
        for _ in 0..2 {
            for element in elements {
                params.push(element as &dyn rusqlite::ToSql);
            }
        }
        let mut statement = self
            .conn
            .prepare(&sql)
            .map_err(|e| DraftError::storage(e.to_string()))?;
        let rows = statement
            .query_map(params.as_slice(), |row| row.get::<_, String>(0))
            .map_err(|e| DraftError::storage(e.to_string()))?;
        Ok(rows
            .flatten()
            .filter_map(|value| ResourceId::parse(value).ok())
            .collect())
    }

    /// Other revisions touching any of `elements`.
    pub fn revisions_touching(&self, elements: &[String]) -> DraftResult<Vec<String>> {
        if elements.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = elements.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT DISTINCT revision_id FROM revision_elements
             WHERE element_id IN ({placeholders}) ORDER BY revision_id"
        );
        let params: Vec<&dyn rusqlite::ToSql> = elements
            .iter()
            .map(|element| element as &dyn rusqlite::ToSql)
            .collect();
        let mut statement = self
            .conn
            .prepare(&sql)
            .map_err(|e| DraftError::storage(e.to_string()))?;
        let rows = statement
            .query_map(params.as_slice(), |row| row.get::<_, String>(0))
            .map_err(|e| DraftError::storage(e.to_string()))?;
        Ok(rows.flatten().collect())
    }

    /// Elements two revisions both touch — a candidate for interference.
    pub fn shared_elements(&self, left: &str, right: &str) -> DraftResult<Vec<String>> {
        let left: BTreeSet<String> = self.elements_touched_by(left)?.into_iter().collect();
        let right: BTreeSet<String> = self.elements_touched_by(right)?.into_iter().collect();
        Ok(left.intersection(&right).cloned().collect())
    }

    /// Execute one statement, reporting a storage failure rather than a
    /// SQLite error nobody above this layer can read.
    fn run<P: rusqlite::Params>(&self, sql: &str, params: P) -> DraftResult<()> {
        self.conn
            .execute(sql, params)
            .map(|_| ())
            .map_err(|e| DraftError::storage(e.to_string()))
    }

    fn query_column<P: rusqlite::Params>(&self, sql: &str, params: P) -> DraftResult<Vec<String>> {
        let mut statement = self
            .conn
            .prepare(sql)
            .map_err(|e| DraftError::storage(e.to_string()))?;
        let rows = statement
            .query_map(params, |row| row.get::<_, String>(0))
            .map_err(|e| DraftError::storage(e.to_string()))?;
        Ok(rows.flatten().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn producer(extension: &str) -> ProducerRef {
        ProducerRef {
            extension_id: extension.into(),
            extension_version: "1.0.0".into(),
            package_digest: format!("sha256:{extension}"),
            attestation_digest: format!("sha256:att-{extension}"),
        }
    }

    fn id(value: &str) -> NamespacedId {
        NamespacedId::parse(value).unwrap()
    }

    fn element(
        resource: &str,
        element_id: &str,
        kind: &str,
        extension: &str,
        attributes: &[(&str, AttributeValue)],
    ) -> ResourceElement {
        ResourceElement {
            element_id: element_id.into(),
            resource_id: ResourceId::parse(resource).unwrap(),
            kind: Some(id(kind)),
            name: Some(element_id.into()),
            attributes: attributes
                .iter()
                .map(|(name, value)| ((*name).to_string(), value.clone()))
                .collect(),
            producer: producer(extension),
        }
    }

    #[test]
    fn an_element_carries_no_built_in_visibility_or_reference_semantics() {
        let encoded = serde_json::to_value(element(
            "res_1",
            "el_1",
            "ex.pub/thing",
            "ex.pub",
            &[("visibility", AttributeValue::Text("public".into()))],
        ))
        .unwrap();
        let fields: Vec<&str> = encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for forbidden in ["public", "exported", "visibility", "is_public"] {
            assert!(
                !fields.contains(&forbidden),
                "Core must not give {forbidden} structural meaning"
            );
        }
        // Visibility is contributed data, reachable only through attributes.
        assert_eq!(encoded["attributes"]["visibility"], "public");
    }

    #[test]
    fn complementary_extractors_compose() {
        // The case a winner-takes-all model got wrong: one extractor finds
        // structure, another finds relations, and both are right.
        let structural = ExtractorResult {
            extractor_id: id("ex.structure/scan"),
            resource_id: ResourceId::parse("res_1").unwrap(),
            elements: vec![element(
                "res_1",
                "el_a",
                "ex.pub/thing",
                "ex.structure",
                &[],
            )],
            relations: vec![],
            producer: producer("ex.structure"),
        };
        let relational = ExtractorResult {
            extractor_id: id("ex.links/scan"),
            resource_id: ResourceId::parse("res_1").unwrap(),
            elements: vec![element("res_1", "el_b", "ex.pub/thing", "ex.links", &[])],
            relations: vec![ElementRelation {
                relation_kind: id("ex.links/uses"),
                from: "el_b".into(),
                to: "el_a".into(),
                producer: producer("ex.links"),
            }],
            producer: producer("ex.links"),
        };
        let merged = merge_extractor_results(&[structural, relational]);
        assert_eq!(merged.elements.len(), 2);
        assert_eq!(merged.relations.len(), 1);
        assert!(merged.collisions.is_empty());
    }

    #[test]
    fn a_disagreement_collides_only_for_that_element() {
        let first = ExtractorResult {
            extractor_id: id("ex.alpha/scan"),
            resource_id: ResourceId::parse("res_1").unwrap(),
            elements: vec![
                element("res_1", "shared", "ex.pub/one", "ex.alpha", &[]),
                element("res_1", "alpha-only", "ex.pub/thing", "ex.alpha", &[]),
            ],
            relations: vec![],
            producer: producer("ex.alpha"),
        };
        let second = ExtractorResult {
            extractor_id: id("ex.zed/scan"),
            resource_id: ResourceId::parse("res_1").unwrap(),
            elements: vec![element("res_1", "shared", "ex.pub/other", "ex.zed", &[])],
            relations: vec![ElementRelation {
                relation_kind: id("ex.zed/uses"),
                from: "alpha-only".into(),
                to: "shared".into(),
                producer: producer("ex.zed"),
            }],
            producer: producer("ex.zed"),
        };
        let merged = merge_extractor_results(&[first, second]);
        assert_eq!(merged.collisions.len(), 1);
        assert_eq!(merged.collisions[0].element_id, "shared");
        assert_eq!(
            merged.collisions[0].extractors,
            vec![id("ex.alpha/scan"), id("ex.zed/scan")]
        );
        // One publisher's disagreement must not blind Draft to the other's
        // correct results.
        assert_eq!(merged.elements.len(), 1);
        assert_eq!(merged.elements[0].element_id, "alpha-only");
        // A relation whose endpoint is unresolved would point at nothing.
        assert!(merged.relations.is_empty());
    }

    #[test]
    fn nothing_is_indexed_without_a_contributed_extractor() {
        let index = ImpactIndex::open_memory().unwrap();
        let indexed = index
            .index_revision("rev_000000000001", &MergedElements::default())
            .unwrap();
        assert_eq!(
            indexed, 0,
            "with no contributed extraction Draft finds nothing rather than guessing"
        );
        assert!(index
            .elements_touched_by("rev_000000000001")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn impact_queries_follow_contributed_relations() {
        let index = ImpactIndex::open_memory().unwrap();
        let merged = merge_extractor_results(&[ExtractorResult {
            extractor_id: id("ex.pub/scan"),
            resource_id: ResourceId::parse("res_1").unwrap(),
            elements: vec![
                element("res_1", "el_a", "ex.pub/thing", "ex.pub", &[]),
                element("res_2", "el_b", "ex.pub/thing", "ex.pub", &[]),
            ],
            relations: vec![ElementRelation {
                relation_kind: id("ex.pub/uses"),
                from: "el_b".into(),
                to: "el_a".into(),
                producer: producer("ex.pub"),
            }],
            producer: producer("ex.pub"),
        }]);
        index.index_revision("rev_left", &merged).unwrap();

        let touched = index.elements_touched_by("rev_left").unwrap();
        assert_eq!(touched, vec!["el_a".to_string(), "el_b".to_string()]);

        // `el_b` relates to `el_a`, so a change to `el_a` reaches res_2.
        let related = index
            .resources_related_by_relations(&["el_a".to_string()])
            .unwrap();
        assert!(related.contains(&ResourceId::parse("res_2").unwrap()));

        index.index_revision("rev_right", &merged).unwrap();
        let shared = index.shared_elements("rev_left", "rev_right").unwrap();
        assert_eq!(shared, vec!["el_a".to_string(), "el_b".to_string()]);
    }

    #[test]
    fn element_predicates_read_contributed_attributes_only() {
        use draft_extension_contract::AttributeMatch;
        let public = element(
            "res_1",
            "el_a",
            "ex.pub/thing",
            "ex.pub",
            &[("visibility", AttributeValue::Text("public".into()))],
        );
        let private = element(
            "res_1",
            "el_b",
            "ex.pub/thing",
            "ex.pub",
            &[("visibility", AttributeValue::Text("private".into()))],
        );
        let rule = ElementPredicate::AttributeIs {
            name: "visibility".into(),
            matches: AttributeMatch::Equals {
                value: AttributeValue::Text("public".into()),
            },
        };
        assert!(element_matches(&rule, &public, &[]));
        assert!(!element_matches(&rule, &private, &[]));

        let relation = ElementRelation {
            relation_kind: id("ex.pub/uses"),
            from: "el_b".into(),
            to: "el_a".into(),
            producer: producer("ex.pub"),
        };
        let incoming = ElementPredicate::RelationExists {
            relation_kind: id("ex.pub/uses"),
            direction: RelationDirection::Incoming,
        };
        assert!(element_matches(
            &incoming,
            &public,
            std::slice::from_ref(&relation)
        ));
        assert!(!element_matches(&incoming, &private, &[relation]));
    }
}
