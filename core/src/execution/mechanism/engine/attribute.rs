//! Attribute projection: elements derived from typed intrinsic facts.
//!
//! Some domains carry their structure in their metadata rather than in their
//! bytes — a catalog entry, a timeline event, a record in an external system.
//! This engine turns declared attributes into elements and relations without
//! reading any content at all, which is what makes it usable for a resource
//! whose bytes Draft cannot or should not fetch.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::dcg::impact::{ElementRelation, ResourceElement};
use crate::dcg::resource::ResourceId;
use crate::extension::provenance::ProducerRef;
use crate::support::error::{DraftError, DraftResult};
use draft_extension_contract::{AttributeValue, NamespacedId};

pub const REVISION: u32 = 1;

/// How one attribute becomes an element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementMapping {
    /// The attribute whose value identifies the element.
    pub attribute: String,
    /// The namespaced kind to record. Opaque to Core.
    pub kind: NamespacedId,
    /// Attributes copied onto the element.
    #[serde(default)]
    pub carry: Vec<String>,
}

/// How one attribute becomes a relation to another element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationMapping {
    /// The attribute naming the element this one points at.
    pub attribute: String,
    pub relation_kind: NamespacedId,
    /// The element mapping this relation originates from.
    pub from_attribute: String,
}

/// The contributed configuration this engine runs under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionConfig {
    pub elements: Vec<ElementMapping>,
    #[serde(default)]
    pub relations: Vec<RelationMapping>,
}

impl ProjectionConfig {
    pub fn parse(config: &serde_json::Value) -> DraftResult<Self> {
        serde_json::from_value(config.clone()).map_err(|error| {
            DraftError::invalid_config(format!("invalid attribute_projection config: {error}"))
        })
    }
}

/// What one projection produced.
#[derive(Debug, Clone, Default)]
pub struct Projection {
    pub elements: Vec<ResourceElement>,
    pub relations: Vec<ElementRelation>,
}

/// Project one resource's attributes into elements and relations.
pub fn project(
    config: &ProjectionConfig,
    resource_id: &ResourceId,
    attributes: &BTreeMap<String, AttributeValue>,
    producer: &ProducerRef,
) -> Projection {
    let mut elements = Vec::new();
    let mut by_attribute: BTreeMap<&str, String> = BTreeMap::new();

    for mapping in &config.elements {
        let Some(value) = attributes.get(&mapping.attribute) else {
            continue;
        };
        let element_id = scalar(value);
        by_attribute.insert(mapping.attribute.as_str(), element_id.clone());
        elements.push(ResourceElement {
            element_id,
            resource_id: resource_id.clone(),
            kind: Some(mapping.kind.clone()),
            name: Some(scalar(value)),
            attributes: mapping
                .carry
                .iter()
                .filter_map(|name| {
                    attributes
                        .get(name)
                        .map(|carried| (name.clone(), carried.clone()))
                })
                .collect(),
            producer: producer.clone(),
        });
    }

    let mut relations = Vec::new();
    for mapping in &config.relations {
        let (Some(from), Some(target)) = (
            by_attribute.get(mapping.from_attribute.as_str()),
            attributes.get(&mapping.attribute),
        ) else {
            continue;
        };
        relations.push(ElementRelation {
            relation_kind: mapping.relation_kind.clone(),
            from: from.clone(),
            to: scalar(target),
            producer: producer.clone(),
        });
    }

    Projection {
        elements,
        relations,
    }
}

/// A stable string for any attribute value.
fn scalar(value: &AttributeValue) -> String {
    match value {
        AttributeValue::Text(text) => text.clone(),
        AttributeValue::Integer(number) => number.to_string(),
        AttributeValue::Boolean(flag) => flag.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> NamespacedId {
        NamespacedId::parse(value).unwrap()
    }

    fn producer() -> ProducerRef {
        ProducerRef {
            extension_id: "example.catalog".into(),
            extension_version: "1.0.0".into(),
            package_digest: "sha256:pkg".into(),
            attestation_digest: "sha256:att".into(),
        }
    }

    #[test]
    fn elements_come_from_metadata_with_no_content_read() {
        let config = ProjectionConfig {
            elements: vec![ElementMapping {
                attribute: "sku".into(),
                kind: id("example.catalog/item"),
                carry: vec!["supplier".into()],
            }],
            relations: vec![RelationMapping {
                attribute: "parent_sku".into(),
                relation_kind: id("example.catalog/belongs-to"),
                from_attribute: "sku".into(),
            }],
        };
        let attributes = BTreeMap::from([
            ("sku".to_string(), AttributeValue::Text("A-100".into())),
            ("supplier".to_string(), AttributeValue::Text("acme".into())),
            (
                "parent_sku".to_string(),
                AttributeValue::Text("A-000".into()),
            ),
        ]);
        let projection = project(
            &config,
            &ResourceId::parse("res_catalog1").unwrap(),
            &attributes,
            &producer(),
        );
        assert_eq!(projection.elements.len(), 1);
        assert_eq!(projection.elements[0].element_id, "A-100");
        assert_eq!(
            projection.elements[0].attributes.get("supplier"),
            Some(&AttributeValue::Text("acme".into()))
        );
        assert_eq!(projection.relations.len(), 1);
        assert_eq!(projection.relations[0].to, "A-000");
    }

    #[test]
    fn a_missing_attribute_projects_nothing_rather_than_an_empty_element() {
        let config = ProjectionConfig {
            elements: vec![ElementMapping {
                attribute: "sku".into(),
                kind: id("example.catalog/item"),
                carry: vec![],
            }],
            relations: vec![],
        };
        let projection = project(
            &config,
            &ResourceId::parse("res_1").unwrap(),
            &BTreeMap::new(),
            &producer(),
        );
        assert!(projection.elements.is_empty());
    }
}
