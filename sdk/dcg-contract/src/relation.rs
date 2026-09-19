//! Relations: edges between Resources, and the proof that an edge bears state.
//!
//! Two questions are kept apart throughout.
//!
//! * **What is the edge?** A [`RelationState`] — source, type, target and its
//!   attributes. Identical canonical state fields are *one logical edge*;
//!   genuinely parallel edges are distinguished by a
//!   [`RelationInstanceKey`], never by accident.
//! * **Why do we believe it, and does it count as state?** A
//!   [`RelationRecord`] pairs the state with its role and provenance.
//!
//! Only `StateBearing` relation *states* enter `ProjectStateRoot`. A `Derived`
//! edge — one a producer inferred rather than observed — cannot become
//! state-bearing on its own say-so. Promoting it requires an explicit,
//! immutable [`StateBearingDeclaration`] carrying an **exact** authorizing
//! grant, so "a tool computed this" can never quietly become "the project's
//! accepted state says this".

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::attribute::AttributeValue;
use crate::digest::{canonical_digest, Digest};
use crate::identifier::ScopedId;
use crate::ids::{ActorId, ProviderBindingId, ResourceId};
use crate::kinds::RelationTypeId;
use crate::observation::ObservationRef;
use crate::producer::ProducerIdentity;
use crate::provider::ProviderSemanticDefinitionDigest;
use crate::security::{PolicyDigest, SecurityFactRef};
use crate::value::Timestamp;
use crate::{FormatError, FormatResult};

/// The frozen domain separator for a relation state digest.
pub const RELATION_STATE_DIGEST_DOMAIN: &str = "draft.dcg.relation-state/v1";
/// The frozen domain separator for a relation record digest.
pub const RELATION_RECORD_DIGEST_DOMAIN: &str = "draft.dcg.relation-record/v1";
/// The frozen domain separator for a state-bearing declaration digest.
pub const STATE_BEARING_DECLARATION_DIGEST_DOMAIN: &str = "draft.dcg.state-bearing-declaration/v1";

/// Distinguishes genuinely parallel edges between the same two Resources.
///
/// Absent for the ordinary case, where identical canonical state fields mean
/// one logical edge. Present only when a domain really does have two distinct
/// edges of the same type between the same endpoints.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelationInstanceKey(ScopedId);

impl RelationInstanceKey {
    pub fn parse(value: impl Into<String>) -> FormatResult<Self> {
        Ok(Self(ScopedId::parse(value)?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for RelationInstanceKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// The canonical material state of one edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationState {
    pub source: ResourceId,
    pub relation_type: RelationTypeId,
    pub target: ResourceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_key: Option<RelationInstanceKey>,
    pub state_attributes: BTreeMap<String, AttributeValue>,
}

impl RelationState {
    pub fn digest(&self) -> FormatResult<RelationStateDigest> {
        Ok(RelationStateDigest(canonical_digest(
            RELATION_STATE_DIGEST_DOMAIN,
            self,
        )?))
    }
}

/// The canonical digest of a [`RelationState`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelationStateDigest(Digest);

impl RelationStateDigest {
    pub fn new(digest: Digest) -> Self {
        Self(digest)
    }

    pub fn digest(&self) -> &Digest {
        &self.0
    }
}

impl std::fmt::Display for RelationStateDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Whether an edge counts as accepted project state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationRole {
    /// The edge is part of the project's material state.
    StateBearing,
    /// The edge was inferred. Useful, but not accepted state until an explicit
    /// declaration says otherwise.
    Derived,
}

/// Why an edge is believed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "provenance", rename_all = "snake_case")]
pub enum RelationProvenance {
    /// A provider observed the edge directly.
    Authoritative {
        binding: ProviderBindingId,
        semantic_definition: ProviderSemanticDefinitionDigest,
        /// The exact observation that established it.
        observation: ObservationRef,
    },
    /// A producer computed the edge from other observations.
    Derived {
        producer: ProducerIdentity,
        /// The exact observations the derivation consumed.
        inputs: Vec<ObservationRef>,
        /// The digest of the derivation rule that was applied.
        derivation: Digest,
    },
}

/// One edge together with its role and provenance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationRecord {
    pub state: RelationState,
    pub role: RelationRole,
    pub provenance: RelationProvenance,
}

impl RelationRecord {
    /// Enforce the role/provenance rule.
    ///
    /// A `StateBearing` record must be `Authoritative`. A derived edge becomes
    /// state-bearing only through a [`StateBearingDeclaration`], never by
    /// asserting the role on itself.
    pub fn validate(&self) -> FormatResult<()> {
        if self.role == RelationRole::StateBearing
            && matches!(self.provenance, RelationProvenance::Derived { .. })
        {
            return Err(FormatError::Consistency(
                "a derived relation cannot declare itself state-bearing; it requires an \
                 authorized StateBearingDeclaration"
                    .into(),
            ));
        }
        if let RelationProvenance::Derived { inputs, .. } = &self.provenance {
            if inputs.is_empty() {
                return Err(FormatError::Consistency(
                    "a derived relation must name the observations it was derived from".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> FormatResult<RelationRecordDigest> {
        self.validate()?;
        Ok(RelationRecordDigest(canonical_digest(
            RELATION_RECORD_DIGEST_DOMAIN,
            self,
        )?))
    }
}

/// The canonical digest of a [`RelationRecord`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelationRecordDigest(Digest);

impl RelationRecordDigest {
    pub fn new(digest: Digest) -> Self {
        Self(digest)
    }

    pub fn digest(&self) -> &Digest {
        &self.0
    }
}

impl std::fmt::Display for RelationRecordDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// The immutable, authorized promotion of a derived edge to accepted state.
///
/// Every field is load-bearing. The declaration names the exact relation state
/// it promotes, the exact derived record it promotes it from, the exact policy
/// in force, and an **exact** authorizing grant. Anything less would let one
/// declaration be replayed to authorize a different edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateBearingDeclaration {
    /// The relation state being promoted.
    pub relation_state: RelationStateDigest,
    /// The exact derived record it is promoted from.
    pub source_relation_record: RelationRecordDigest,
    /// Who declared it.
    pub producer: ProducerIdentity,
    /// The policy in force when the declaration was made.
    pub policy_digest: PolicyDigest,
    /// The exact grant that authorized *this* declaration.
    ///
    /// An exact ref, so widening the grant beneath its id cannot retroactively
    /// authorize declarations it never covered.
    pub authorizing_grant: SecurityFactRef,
    pub actor: ActorId,
    pub declared_at: Timestamp,
}

impl StateBearingDeclaration {
    pub fn digest(&self) -> FormatResult<StateBearingDeclarationDigest> {
        Ok(StateBearingDeclarationDigest(canonical_digest(
            STATE_BEARING_DECLARATION_DIGEST_DOMAIN,
            self,
        )?))
    }

    /// Verify this declaration actually promotes the record it claims to.
    ///
    /// The record must be the exact one named, it must be `Derived` (promoting
    /// an already-authoritative edge is meaningless), and the state it carries
    /// must be the exact state being promoted.
    pub fn verify_promotes(&self, record: &RelationRecord) -> FormatResult<()> {
        let record_digest = record.digest()?;
        if record_digest != self.source_relation_record {
            return Err(FormatError::Integrity(format!(
                "declaration names source record {} but the loaded record computes to {}",
                self.source_relation_record, record_digest
            )));
        }
        if record.role != RelationRole::Derived {
            return Err(FormatError::Consistency(
                "a StateBearingDeclaration promotes a Derived record; this one is not derived"
                    .into(),
            ));
        }
        let state_digest = record.state.digest()?;
        if state_digest != self.relation_state {
            return Err(FormatError::Consistency(format!(
                "declaration promotes relation state {} but its source record carries {}",
                self.relation_state, state_digest
            )));
        }
        Ok(())
    }
}

/// The canonical digest of a [`StateBearingDeclaration`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StateBearingDeclarationDigest(Digest);

impl StateBearingDeclarationDigest {
    pub fn new(digest: Digest) -> Self {
        Self(digest)
    }

    pub fn digest(&self) -> &Digest {
        &self.0
    }
}

impl std::fmt::Display for StateBearingDeclarationDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::NamespacedId;
    use crate::ids::ObservationId;
    use crate::observation::ObservationDigest;
    use crate::security::SecurityControlKindId;

    fn observation_ref(seed: &[u8]) -> ObservationRef {
        ObservationRef {
            id: ObservationId::parse("obs_a1b2c3").unwrap(),
            digest: ObservationDigest::new(Digest::of_bytes(seed)),
        }
    }

    fn state() -> RelationState {
        RelationState {
            source: ResourceId::parse("res_source").unwrap(),
            relation_type: RelationTypeId::parse("acme.crm/owns").unwrap(),
            target: ResourceId::parse("res_target").unwrap(),
            instance_key: None,
            state_attributes: BTreeMap::new(),
        }
    }

    fn producer() -> ProducerIdentity {
        ProducerIdentity::new(NamespacedId::parse("acme.tools/deriver").unwrap(), "1.0").unwrap()
    }

    fn derived_record() -> RelationRecord {
        RelationRecord {
            state: state(),
            role: RelationRole::Derived,
            provenance: RelationProvenance::Derived {
                producer: producer(),
                inputs: vec![observation_ref(b"input")],
                derivation: Digest::of_bytes(b"rule"),
            },
        }
    }

    fn authoritative_record() -> RelationRecord {
        RelationRecord {
            state: state(),
            role: RelationRole::StateBearing,
            provenance: RelationProvenance::Authoritative {
                binding: ProviderBindingId::parse("pbd_a1").unwrap(),
                semantic_definition: ProviderSemanticDefinitionDigest::new(Digest::of_bytes(
                    b"SD1",
                )),
                observation: observation_ref(b"observed"),
            },
        }
    }

    fn grant() -> SecurityFactRef {
        SecurityFactRef::new(
            SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap(),
            Some(ScopedId::parse("auth_1").unwrap()),
            Digest::of_bytes(b"grant"),
        )
    }

    fn declaration() -> StateBearingDeclaration {
        StateBearingDeclaration {
            relation_state: state().digest().unwrap(),
            source_relation_record: derived_record().digest().unwrap(),
            producer: producer(),
            policy_digest: PolicyDigest::new(Digest::of_bytes(b"policy")),
            authorizing_grant: grant(),
            actor: ActorId::parse("act_a1").unwrap(),
            declared_at: Timestamp::from_unix_nanos(1_000),
        }
    }

    #[test]
    fn a_derived_relation_cannot_declare_itself_state_bearing() {
        let mut overreaching = derived_record();
        overreaching.role = RelationRole::StateBearing;
        assert!(matches!(
            overreaching.validate(),
            Err(FormatError::Consistency(_))
        ));
        authoritative_record().validate().unwrap();
    }

    #[test]
    fn a_derived_relation_must_name_its_inputs() {
        let mut groundless = derived_record();
        groundless.provenance = RelationProvenance::Derived {
            producer: producer(),
            inputs: vec![],
            derivation: Digest::of_bytes(b"rule"),
        };
        assert!(groundless.validate().is_err());
    }

    #[test]
    fn a_declaration_promotes_exactly_the_record_it_names() {
        declaration().verify_promotes(&derived_record()).unwrap();
    }

    #[test]
    fn a_declaration_cannot_be_replayed_against_another_relation() {
        // The scenario the exact digests exist for: one authorized promotion
        // must not authorize a second, different edge.
        let mut other_edge = derived_record();
        other_edge.state.target = ResourceId::parse("res_elsewhere").unwrap();
        let error = declaration().verify_promotes(&other_edge).unwrap_err();
        assert!(matches!(error, FormatError::Integrity(_)), "{error}");
    }

    #[test]
    fn a_declaration_will_not_promote_an_already_authoritative_record() {
        let mut declaration = declaration();
        declaration.source_relation_record = authoritative_record().digest().unwrap();
        let error = declaration
            .verify_promotes(&authoritative_record())
            .unwrap_err();
        assert!(matches!(error, FormatError::Consistency(_)), "{error}");
    }

    #[test]
    fn substituting_the_authorizing_grant_changes_the_declaration() {
        let original = declaration().digest().unwrap();
        let mut widened = declaration();
        widened.authorizing_grant = SecurityFactRef::new(
            SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap(),
            Some(ScopedId::parse("auth_1").unwrap()),
            Digest::of_bytes(b"grant-widened"),
        );
        assert_ne!(original, widened.digest().unwrap());
    }

    #[test]
    fn identical_edges_are_one_logical_edge_and_a_key_separates_parallels() {
        let first = state().digest().unwrap();
        let duplicate = state().digest().unwrap();
        assert_eq!(first, duplicate, "one logical edge");

        let mut parallel = state();
        parallel.instance_key = Some(RelationInstanceKey::parse("second").unwrap());
        assert_ne!(first, parallel.digest().unwrap());
    }

    #[test]
    fn the_wire_forms_round_trip() {
        let record = derived_record();
        let encoded = serde_json::to_string(&record).unwrap();
        assert_eq!(
            serde_json::from_str::<RelationRecord>(&encoded).unwrap(),
            record
        );
        let declaration = declaration();
        let encoded = serde_json::to_string(&declaration).unwrap();
        assert_eq!(
            serde_json::from_str::<StateBearingDeclaration>(&encoded).unwrap(),
            declaration
        );
    }
}
