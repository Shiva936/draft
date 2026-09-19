//! Keyed record-set comparison, for domains with no meaningful ordering.
//!
//! A scene graph, a CAD assembly, a configuration tree and a dependency set all
//! have named things and no natural sequence. Aligning them as a stream would
//! invent adjacency that does not exist and report conflicts between changes
//! that are genuinely independent.
//!
//! This engine compares canonical `(key, value-digest)` records and claims
//! opaque keys in a contributed key space. Two changes touching different keys
//! compose; two touching the same key do not.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{EngineOutput, EngineUnit};
use crate::dcg::representation::{ConflictClaim, ConflictScope};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

pub const REVISION: u32 = 1;

/// The largest record document this engine will parse.
pub const DEFAULT_BYTE_BUDGET: u64 = 4 * 1024 * 1024;

/// The contributed configuration this engine runs under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyedConfig {
    /// The name of the key space the emitted keys live in. Opaque to Core.
    pub key_space: String,
    #[serde(default = "default_budget")]
    pub byte_budget: u64,
}

fn default_budget() -> u64 {
    DEFAULT_BYTE_BUDGET
}

impl KeyedConfig {
    pub fn parse(config: &serde_json::Value) -> DraftResult<Self> {
        serde_json::from_value(config.clone()).map_err(|error| {
            DraftError::invalid_config(format!("invalid keyed_record_set config: {error}"))
        })
    }
}

/// A canonical record set: keys mapped to the digest of their value.
///
/// The extension produces this shape; Core compares it without ever learning
/// what a key names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecordSet(pub BTreeMap<String, String>);

impl RecordSet {
    /// Parse a record document, refusing anything larger than the budget.
    pub fn parse(bytes: Option<&[u8]>, budget: u64) -> DraftResult<Self> {
        let Some(bytes) = bytes else {
            return Ok(Self::default());
        };
        if bytes.len() as u64 > budget {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "record set of {} bytes exceeds the declared budget of {budget}",
                    bytes.len()
                ),
            ));
        }
        serde_json::from_slice(bytes).map_err(|error| {
            DraftError::new(
                DraftErrorKind::Validation,
                format!("record set is not a canonical key/digest document: {error}"),
            )
        })
    }
}

/// Compare two record sets and claim the keys that differ.
pub fn compare(
    config: &KeyedConfig,
    before: Option<&[u8]>,
    after: Option<&[u8]>,
) -> DraftResult<EngineOutput> {
    if config.key_space.trim().is_empty() {
        return Err(DraftError::invalid_config(
            "keyed_record_set requires a contributed key_space",
        ));
    }
    let before = RecordSet::parse(before, config.byte_budget)?;
    let after = RecordSet::parse(after, config.byte_budget)?;

    let keys: BTreeSet<&String> = before.0.keys().chain(after.0.keys()).collect();
    let mut units = Vec::new();
    let mut claims = Vec::new();
    let (mut added, mut removed, mut modified) = (0i64, 0i64, 0i64);

    for key in keys {
        let old = before.0.get(key);
        let new = after.0.get(key);
        if old == new {
            continue;
        }
        match (old, new) {
            (None, Some(_)) => added += 1,
            (Some(_), None) => removed += 1,
            _ => modified += 1,
        }
        let scope = ConflictScope::OpaqueKey {
            key_space: config.key_space.clone(),
            key: key.clone(),
        };
        units.push(EngineUnit {
            unit_id: key.clone(),
            scope: scope.clone(),
            label: key.clone(),
        });
        claims.push(ConflictClaim {
            id: key.clone(),
            scope,
        });
    }

    let mut metrics = BTreeMap::new();
    metrics.insert("draft.core/records_added".to_string(), added);
    metrics.insert("draft.core/records_removed".to_string(), removed);
    metrics.insert("draft.core/records_modified".to_string(), modified);
    Ok(EngineOutput {
        units,
        claims,
        metrics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcg::representation::{reconcile, ClaimRelation};

    fn config(space: &str) -> KeyedConfig {
        KeyedConfig {
            key_space: space.to_string(),
            byte_budget: DEFAULT_BYTE_BUDGET,
        }
    }

    fn records(pairs: &[(&str, &str)]) -> Vec<u8> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        serde_json::to_vec(&map).unwrap()
    }

    #[test]
    fn distinct_keys_are_independent_with_no_ordering_involved() {
        let space = config("example/node");
        let base = records(&[("hull", "d1"), ("mast", "d2"), ("keel", "d3")]);
        let first = compare(
            &space,
            Some(&base),
            Some(&records(&[
                ("hull", "CHANGED"),
                ("mast", "d2"),
                ("keel", "d3"),
            ])),
        )
        .unwrap();
        let second = compare(
            &space,
            Some(&base),
            Some(&records(&[
                ("hull", "d1"),
                ("mast", "d2"),
                ("keel", "CHANGED"),
            ])),
        )
        .unwrap();
        assert_eq!(
            reconcile(&first.claims, &second.claims),
            ClaimRelation::Independent
        );
    }

    #[test]
    fn the_same_key_conflicts() {
        let space = config("example/node");
        let base = records(&[("hull", "d1")]);
        let first = compare(&space, Some(&base), Some(&records(&[("hull", "x")]))).unwrap();
        let second = compare(&space, Some(&base), Some(&records(&[("hull", "y")]))).unwrap();
        assert!(matches!(
            reconcile(&first.claims, &second.claims),
            ClaimRelation::Conflicting { .. }
        ));
    }

    #[test]
    fn two_key_spaces_are_incomparable_rather_than_independent() {
        let base = records(&[("hull", "d1")]);
        let after = records(&[("hull", "d2")]);
        let left = compare(&config("example/node"), Some(&base), Some(&after)).unwrap();
        let right = compare(&config("other/part"), Some(&base), Some(&after)).unwrap();
        assert!(matches!(
            reconcile(&left.claims, &right.claims),
            ClaimRelation::Indeterminate { .. }
        ));
    }

    #[test]
    fn additions_and_removals_are_counted_and_claimed() {
        let output = compare(
            &config("example/node"),
            Some(&records(&[("a", "1"), ("b", "2")])),
            Some(&records(&[("b", "2"), ("c", "3")])),
        )
        .unwrap();
        assert_eq!(output.claims.len(), 2);
        assert_eq!(output.metrics.get("draft.core/records_added"), Some(&1));
        assert_eq!(output.metrics.get("draft.core/records_removed"), Some(&1));
        assert_eq!(output.metrics.get("draft.core/records_modified"), Some(&0));
    }

    #[test]
    fn an_oversized_document_is_refused() {
        let mut space = config("example/node");
        space.byte_budget = 4;
        let error = compare(&space, Some(&records(&[("a", "1")])), None).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);
    }
}
