//! Resource state semantics: one identifier, one contract, forever.
//!
//! A [`ResourceStateSemanticsId`] does not name a loose family of
//! interpretations. It identifies the **complete canonical material-state
//! interpretation contract**: which fields bear state, how the locator
//! participates, how the content and semantic digests are to be read, how
//! attributes are interpreted and normalized, and what presence and absence
//! mean.
//!
//! Changing any of that requires a **new identifier**. Within a trusted catalog
//! and the project history that accepted it:
//!
//! ```text
//! ResourceStateSemanticsId  ->  EXACTLY ONE ResourceStateSemanticsContractDigest
//!
//! same id + same digest       -> the same contract
//! same id + DIFFERENT digest  -> SEMANTIC IDENTITY CONFLICT -> reject
//! ```
//!
//! Without that rule, a vendor could redefine what "unchanged" means beneath an
//! id that historical baselines already committed to, and every `BaselineId`
//! that used it would silently change meaning while its bytes stayed valid.
//!
//! The contract object is **retained**: it is a GC root and travels in
//! DraftPack exports, so verifying historical state semantics never requires
//! the extension that contributed it to still be installed.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::digest::{canonical_digest, Digest};
use crate::identifier::NamespacedId;
use crate::{FormatError, FormatResult};

/// The frozen domain separator for a semantics contract digest.
pub const SEMANTICS_CONTRACT_DIGEST_DOMAIN: &str = "draft.dcg.resource-state-semantics-contract/v1";

/// Identifies a complete material-state interpretation contract.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceStateSemanticsId(NamespacedId);

impl ResourceStateSemanticsId {
    pub fn parse(value: &str) -> FormatResult<Self> {
        Ok(Self(NamespacedId::parse(value)?))
    }

    pub fn as_namespaced(&self) -> &NamespacedId {
        &self.0
    }

    pub fn is_reserved(&self) -> bool {
        self.0.namespace() == "draft" || self.0.namespace().starts_with("draft.")
    }
}

impl std::fmt::Display for ResourceStateSemanticsId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Whether a Resource's locator is part of its material state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocatorStateRole {
    /// Where the resource is *is* part of what it is, so moving it changes
    /// state. A filesystem provider works this way.
    StateBearing,
    /// The locator is provenance only: it records where the resource was
    /// found, and moving it leaves material state unchanged.
    Informational,
}

/// How a digest field is to be read, if at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigestInterpretation {
    /// The field must be present and participates in material state.
    Required,
    /// The field may be present; when it is, it participates.
    Optional,
    /// The field is never present under this contract.
    Absent,
}

/// What it means for a state-bearing field to be absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceAbsenceSemantics {
    /// Absence is a distinct, meaningful state — the thing is known not to be
    /// there. Distinct from `Unknown`: proved absent is not the same fact as
    /// never looked.
    MeaningfulAbsence,
    /// Absence means the observer established nothing, and coverage evidence
    /// must justify it.
    Unknown,
}

/// How an attribute participates in material state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributeInterpretation {
    /// The attribute is part of material state; changing it changes the digest.
    StateBearing,
    /// The attribute is recorded but not part of material state.
    Informational,
}

/// How values are normalized before they are hashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizationRule {
    /// No normalization: bytes are compared exactly as observed.
    ///
    /// The safe default. Every other rule makes two genuinely different
    /// observations compare equal, which is a claim about the domain that only
    /// the contract's author can make.
    None,
    /// Trailing whitespace on each line is removed before hashing.
    TrimTrailingWhitespace,
    /// Line endings are normalized to `\n` before hashing.
    NormalizeLineEndings,
}

/// The complete, canonical, content-addressed interpretation contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceStateSemanticsContract {
    /// The identifier this contract defines. One id, one contract, forever.
    pub id: ResourceStateSemanticsId,
    /// Whether the locator bears state.
    pub locator_state_role: LocatorStateRole,
    /// Which named attributes bear state, and how each is read.
    ///
    /// An attribute absent from this map is informational: it is recorded but
    /// does not enter `ResourceStateDigest`.
    pub attribute_interpretation: BTreeMap<String, AttributeInterpretation>,
    /// How the content digest is read.
    pub content_digest_interpretation: DigestInterpretation,
    /// How the semantic digest is read.
    pub semantic_digest_interpretation: DigestInterpretation,
    /// Normalization applied before hashing, in the order listed.
    pub normalization: Vec<NormalizationRule>,
    /// What absence of a state-bearing field means.
    pub presence_absence_semantics: PresenceAbsenceSemantics,
}

impl ResourceStateSemanticsContract {
    /// Validate the contract's internal coherence.
    pub fn validate(&self) -> FormatResult<()> {
        // A contract that establishes no material state at all cannot
        // distinguish two states, which makes every digest under it identical.
        let has_digest = self.content_digest_interpretation != DigestInterpretation::Absent
            || self.semantic_digest_interpretation != DigestInterpretation::Absent;
        let has_attribute = self
            .attribute_interpretation
            .values()
            .any(|interpretation| *interpretation == AttributeInterpretation::StateBearing);
        let has_locator = self.locator_state_role == LocatorStateRole::StateBearing;
        if !has_digest && !has_attribute && !has_locator {
            return Err(FormatError::Consistency(format!(
                "semantics contract '{}' declares nothing state-bearing, so every resource \
                 under it would have the same state digest",
                self.id
            )));
        }
        // Duplicate normalization rules would apply twice and are more likely a
        // mistake than an intent.
        let unique: BTreeSet<&NormalizationRule> = self.normalization.iter().collect();
        if unique.len() != self.normalization.len() {
            return Err(FormatError::Consistency(format!(
                "semantics contract '{}' repeats a normalization rule",
                self.id
            )));
        }
        Ok(())
    }

    /// This contract's canonical digest.
    pub fn digest(&self) -> FormatResult<ResourceStateSemanticsContractDigest> {
        self.validate()?;
        Ok(ResourceStateSemanticsContractDigest(canonical_digest(
            SEMANTICS_CONTRACT_DIGEST_DOMAIN,
            self,
        )?))
    }

    /// The exact reference to this contract.
    pub fn reference(&self) -> FormatResult<ResourceStateSemanticsRef> {
        Ok(ResourceStateSemanticsRef {
            id: self.id.clone(),
            contract_digest: self.digest()?,
        })
    }
}

/// The canonical digest of a [`ResourceStateSemanticsContract`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceStateSemanticsContractDigest(Digest);

impl ResourceStateSemanticsContractDigest {
    pub fn new(digest: Digest) -> Self {
        Self(digest)
    }

    pub fn digest(&self) -> &Digest {
        &self.0
    }
}

impl std::fmt::Display for ResourceStateSemanticsContractDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// An exact reference to the semantics a `ResourceState` was interpreted under.
///
/// Carries the digest, not just the id, because the id alone would let the
/// contract's meaning be replaced beneath every state that cited it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceStateSemanticsRef {
    pub id: ResourceStateSemanticsId,
    pub contract_digest: ResourceStateSemanticsContractDigest,
}

impl ResourceStateSemanticsRef {
    /// Verify this reference against the contract it claims to name.
    ///
    /// Both halves must agree. A matching id with a different digest is a
    /// **semantic identity conflict**, not a stale cache: it means someone
    /// redefined what an accepted identifier means.
    pub fn verify(&self, contract: &ResourceStateSemanticsContract) -> FormatResult<()> {
        if contract.id != self.id {
            return Err(FormatError::Integrity(format!(
                "semantics contract names '{}' but the reference names '{}'",
                contract.id, self.id
            )));
        }
        let recomputed = contract.digest()?;
        if recomputed != self.contract_digest {
            return Err(FormatError::Integrity(format!(
                "semantics id '{}' is bound to contract {} but the stored contract computes to \
                 {}; one identifier may name exactly one contract",
                self.id, self.contract_digest, recomputed
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> ResourceStateSemanticsContract {
        ResourceStateSemanticsContract {
            id: ResourceStateSemanticsId::parse("draft.filesystem/file.v1").unwrap(),
            locator_state_role: LocatorStateRole::StateBearing,
            attribute_interpretation: BTreeMap::from([(
                "executable".to_string(),
                AttributeInterpretation::StateBearing,
            )]),
            content_digest_interpretation: DigestInterpretation::Required,
            semantic_digest_interpretation: DigestInterpretation::Absent,
            normalization: vec![NormalizationRule::None],
            presence_absence_semantics: PresenceAbsenceSemantics::MeaningfulAbsence,
        }
    }

    #[test]
    fn a_reference_verifies_against_its_own_contract() {
        let contract = contract();
        contract.reference().unwrap().verify(&contract).unwrap();
    }

    #[test]
    fn one_id_may_name_exactly_one_contract() {
        // The scenario this rule exists for: a vendor keeps the identifier and
        // changes what it means. Every historical baseline that cited the id
        // would otherwise silently change meaning.
        let original = contract();
        let reference = original.reference().unwrap();

        let mut redefined = contract();
        redefined.locator_state_role = LocatorStateRole::Informational;

        let error = reference.verify(&redefined).unwrap_err();
        assert!(matches!(error, FormatError::Integrity(_)), "{error}");
    }

    #[test]
    fn a_reference_will_not_verify_against_another_identifier() {
        let reference = contract().reference().unwrap();
        let mut other = contract();
        other.id = ResourceStateSemanticsId::parse("acme.crm/record.v1").unwrap();
        assert!(reference.verify(&other).is_err());
    }

    #[test]
    fn every_interpretation_field_is_part_of_the_identity() {
        let base = contract().digest().unwrap();
        let mut changed = contract();
        changed.content_digest_interpretation = DigestInterpretation::Optional;
        assert_ne!(base, changed.digest().unwrap());

        let mut changed = contract();
        changed.normalization = vec![NormalizationRule::NormalizeLineEndings];
        assert_ne!(base, changed.digest().unwrap());

        let mut changed = contract();
        changed.presence_absence_semantics = PresenceAbsenceSemantics::Unknown;
        assert_ne!(base, changed.digest().unwrap());

        let mut changed = contract();
        changed
            .attribute_interpretation
            .insert("mode".into(), AttributeInterpretation::StateBearing);
        assert_ne!(base, changed.digest().unwrap());
    }

    #[test]
    fn a_contract_that_bears_no_state_is_refused() {
        let mut empty = contract();
        empty.locator_state_role = LocatorStateRole::Informational;
        empty.content_digest_interpretation = DigestInterpretation::Absent;
        empty.semantic_digest_interpretation = DigestInterpretation::Absent;
        empty.attribute_interpretation.clear();
        assert!(matches!(empty.validate(), Err(FormatError::Consistency(_))));
    }

    #[test]
    fn a_repeated_normalization_rule_is_refused() {
        let mut repeated = contract();
        repeated.normalization = vec![NormalizationRule::None, NormalizationRule::None];
        assert!(repeated.validate().is_err());
    }

    #[test]
    fn the_wire_form_round_trips() {
        let contract = contract();
        let encoded = serde_json::to_string(&contract).unwrap();
        assert_eq!(
            serde_json::from_str::<ResourceStateSemanticsContract>(&encoded).unwrap(),
            contract
        );
    }
}
