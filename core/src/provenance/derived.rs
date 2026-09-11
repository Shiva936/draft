//! What a derived artifact was computed from, and who computed it.
//!
//! These are two different questions, and one header answering both would be
//! wrong for most artifacts:
//!
//! * **Semantic dependencies** — the artifact bytes that influenced this result.
//!   This is what caching and recomputation key on.
//! * **Production provenance** — which producer made a particular result, under
//!   which schema, authorization and executable. This is what historical audit
//!   follows.
//!
//! Crucially, provenance belongs to the *result that was produced*, not to the
//! bundle that collects results. A classification bundle aggregates assignments
//! from several publishers; a verification evidence set aggregates independently
//! named checks; an acceptance evaluation is produced by Draft itself and has no
//! extension producer at all. Forcing a single `ProducerRef` onto those would
//! either invent an author or discard the real ones.

use crate::extension::provenance::ProducerRef;
use crate::support::hashing;
use draft_extension_contract::SchemaRef;
use serde::{Deserialize, Serialize};

/// The revision of Draft's own dependency-graph canonicalization.
pub const DERIVED_DAG_REVISION: u32 = 1;

/// What a derived artifact is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "subject", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubjectRef {
    Snapshot { snapshot_digest: String },
    ChangeSet { change_set_digest: String },
}

/// A kind of derived artifact, for dependency references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivedArtifactKind {
    ClassificationBundle,
    ChangeRepresentationBundle,
    ImpactIndex,
    VerificationEvidence,
    RiskAssessment,
    SnapshotObservationState,
    ChangeSetDerivationState,
    RecoveryAnchorSet,
}

/// One upstream artifact this result actually consumed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedArtifactRef {
    pub kind: DerivedArtifactKind,
    pub digest: String,
}

/// The semantic dependencies of one derived artifact.
///
/// Producer-neutral by design: this says what the result was computed *from*,
/// and says nothing about who computed it. An artifact records only what it
/// genuinely read, so an unrelated bundle changing cannot invalidate it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivationInputs {
    pub subject: SubjectRef,
    /// Sorted and deduplicated; exactly what was consumed, never a blanket list.
    pub upstream: Vec<DerivedArtifactRef>,
    pub dag_revision: u32,
}

impl DerivationInputs {
    pub fn new(
        subject: SubjectRef,
        upstream: impl IntoIterator<Item = DerivedArtifactRef>,
    ) -> Self {
        let mut upstream: Vec<DerivedArtifactRef> = upstream.into_iter().collect();
        upstream.sort();
        upstream.dedup();
        Self {
            subject,
            upstream,
            dag_revision: DERIVED_DAG_REVISION,
        }
    }

    /// The cache key for a result derived from these inputs.
    ///
    /// Everything that semantically determines the result participates, and
    /// nothing else does: a producer's version string is not enough to
    /// distinguish two builds, and an unconsumed artifact must not invalidate.
    pub fn cache_key(
        &self,
        mechanism_identity: &MechanismIdentity,
        extra: &serde_json::Value,
    ) -> String {
        hashing::canonical_hash(&serde_json::json!({
            "inputs": self,
            "mechanism": mechanism_identity,
            "extra": extra,
        }))
    }

    pub fn consumed(&self, kind: DerivedArtifactKind) -> Option<&str> {
        self.upstream
            .iter()
            .find(|reference| reference.kind == kind)
            .map(|reference| reference.digest.as_str())
    }
}

/// Which implementation produced a result, in enough detail to reproduce the
/// decision to recompute it.
///
/// A package version is not enough: the same declaration backed by a different
/// executable or a different engine revision is a different computation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mechanism", rename_all = "snake_case", deny_unknown_fields)]
pub enum MechanismIdentity {
    Engine {
        engine: draft_extension_contract::EngineId,
        engine_revision: u32,
        config_digest: String,
    },
    Command {
        command_config_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        executable_digest: Option<String>,
    },
}

/// Provenance for a result an extension produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionProductionProvenance {
    pub producer: ProducerRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<SchemaRef>,
    /// The decision that permitted this run, when the mechanism was
    /// command-backed. Absent for engine-backed results, which need no
    /// execution permission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_identity: Option<String>,
    pub mechanism_identity: MechanismIdentity,
}

/// Provenance for a result Draft itself produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreDerivationProvenance {
    pub component: String,
    /// The Core semantics that produced this result. Present because a
    /// contract revision alone does not determine an aggregation's meaning.
    pub implementation_revision: u32,
}

/// Who produced one result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "produced_by", rename_all = "snake_case", deny_unknown_fields)]
pub enum DerivationProvenance {
    Core(CoreDerivationProvenance),
    Extension(Box<ExtensionProductionProvenance>),
}

impl DerivationProvenance {
    pub fn core(component: impl Into<String>, implementation_revision: u32) -> Self {
        Self::Core(CoreDerivationProvenance {
            component: component.into(),
            implementation_revision,
        })
    }

    /// The extension that produced this, if any. A Core-produced result has
    /// none, and does not pretend otherwise.
    pub fn producer(&self) -> Option<&ProducerRef> {
        match self {
            Self::Core(_) => None,
            Self::Extension(provenance) => Some(&provenance.producer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_extension_contract::{EngineId, NamespacedId};

    fn change_set() -> SubjectRef {
        SubjectRef::ChangeSet {
            change_set_digest: "sha256:change".into(),
        }
    }

    fn reference(kind: DerivedArtifactKind, digest: &str) -> DerivedArtifactRef {
        DerivedArtifactRef {
            kind,
            digest: digest.into(),
        }
    }

    fn engine() -> MechanismIdentity {
        MechanismIdentity::Engine {
            engine: EngineId::WholeResource,
            engine_revision: 1,
            config_digest: "sha256:config".into(),
        }
    }

    #[test]
    fn a_dependency_header_names_no_producer() {
        // The header answers "what did this depend on", not "who made it": an
        // aggregate has many producers and a Core result has none.
        let inputs = DerivationInputs::new(change_set(), []);
        let encoded = serde_json::to_value(&inputs).unwrap();
        let fields: Vec<&str> = encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for forbidden in ["producer", "schema", "authorization_decision"] {
            assert!(!fields.contains(&forbidden));
        }
    }

    #[test]
    fn a_core_produced_result_has_no_extension_producer() {
        let provenance = DerivationProvenance::core("acceptance-evaluator", 1);
        assert!(provenance.producer().is_none());
        let encoded = serde_json::to_value(&provenance).unwrap();
        assert!(encoded.get("producer").is_none());
        assert_eq!(encoded["produced_by"], "core");
    }

    #[test]
    fn dependencies_are_canonical_and_order_independent() {
        let forwards = DerivationInputs::new(
            change_set(),
            [
                reference(DerivedArtifactKind::ClassificationBundle, "sha256:c"),
                reference(DerivedArtifactKind::VerificationEvidence, "sha256:v"),
            ],
        );
        let backwards = DerivationInputs::new(
            change_set(),
            [
                reference(DerivedArtifactKind::VerificationEvidence, "sha256:v"),
                reference(DerivedArtifactKind::ClassificationBundle, "sha256:c"),
            ],
        );
        assert_eq!(forwards, backwards);
        assert_eq!(
            forwards.cache_key(&engine(), &serde_json::json!({})),
            backwards.cache_key(&engine(), &serde_json::json!({}))
        );
    }

    #[test]
    fn a_consumed_dependency_invalidates_and_an_unconsumed_one_does_not() {
        let consuming = DerivationInputs::new(
            change_set(),
            [reference(
                DerivedArtifactKind::VerificationEvidence,
                "sha256:v1",
            )],
        );
        let after_change = DerivationInputs::new(
            change_set(),
            [reference(
                DerivedArtifactKind::VerificationEvidence,
                "sha256:v2",
            )],
        );
        let key = |inputs: &DerivationInputs| inputs.cache_key(&engine(), &serde_json::json!({}));
        assert_ne!(key(&consuming), key(&after_change));

        // An artifact this result never read cannot invalidate it, whatever it
        // does.
        assert_eq!(
            key(&consuming),
            key(&DerivationInputs::new(
                change_set(),
                [reference(
                    DerivedArtifactKind::VerificationEvidence,
                    "sha256:v1"
                )]
            ))
        );
        assert_eq!(
            consuming.consumed(DerivedArtifactKind::VerificationEvidence),
            Some("sha256:v1")
        );
        assert_eq!(
            consuming.consumed(DerivedArtifactKind::ClassificationBundle),
            None
        );
    }

    #[test]
    fn the_mechanism_participates_so_a_rebuilt_observer_invalidates() {
        let inputs = DerivationInputs::new(change_set(), []);
        let first = inputs.cache_key(&engine(), &serde_json::json!({}));
        let bumped = inputs.cache_key(
            &MechanismIdentity::Engine {
                engine: EngineId::WholeResource,
                engine_revision: 2,
                config_digest: "sha256:config".into(),
            },
            &serde_json::json!({}),
        );
        assert_ne!(first, bumped);

        // A different executable behind the same declaration is a different
        // computation, even though the package did not change.
        let one = inputs.cache_key(
            &MechanismIdentity::Command {
                command_config_digest: "sha256:cmd".into(),
                executable_digest: Some("sha256:bin-a".into()),
            },
            &serde_json::json!({}),
        );
        let other = inputs.cache_key(
            &MechanismIdentity::Command {
                command_config_digest: "sha256:cmd".into(),
                executable_digest: Some("sha256:bin-b".into()),
            },
            &serde_json::json!({}),
        );
        assert_ne!(one, other);
    }

    #[test]
    fn extension_provenance_carries_its_authorization_only_when_command_backed() {
        let engine_backed = ExtensionProductionProvenance {
            producer: ProducerRef {
                extension_id: "ex.pub".into(),
                extension_version: "1.0.0".into(),
                package_digest: "sha256:pkg".into(),
                attestation_digest: "sha256:att".into(),
            },
            schema: Some(SchemaRef::new(
                NamespacedId::parse("ex.pub/result").unwrap(),
                1,
            )),
            authorization_decision: None,
            executable_identity: None,
            mechanism_identity: engine(),
        };
        let encoded = serde_json::to_value(&engine_backed).unwrap();
        assert!(encoded.get("authorization_decision").is_none());
        assert_eq!(
            DerivationProvenance::Extension(Box::new(engine_backed))
                .producer()
                .map(|p| p.extension_id.clone()),
            Some("ex.pub".to_string())
        );
    }
}
