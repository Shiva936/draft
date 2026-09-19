//! `ResourceState` — what a Resource materially *is*, and nothing else.
//!
//! The exclusions are the design. `ResourceState` contains:
//!
//! * **no `ResourceId`** — so two Resources in identical states have identical
//!   state digests, and identity stays separate from state;
//! * **no provider identity** — so re-observing the same state through a
//!   different provider does not invent a change;
//! * **no `ObservationStability`** — stability is a statement about how much an
//!   observer trusts what it saw, not about what is there.
//!
//! Those three, plus timing, are what would otherwise make "the state changed"
//! and "we looked again" indistinguishable.
//!
//! What state *means* is not decided here: it is decided by the
//! [`ResourceStateSemanticsRef`] this state carries, which names an exact
//! immutable contract. Two states are comparable only under the same contract.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::attribute::{AttributeValue, ResourceLocator};
use crate::digest::{canonical_digest, Digest};
use crate::kinds::ResourceKindId;
use crate::semantics::{
    DigestInterpretation, LocatorStateRole, ResourceStateSemanticsContract,
    ResourceStateSemanticsRef,
};
use crate::{FormatError, FormatResult};

/// The frozen domain separator for a resource state digest.
pub const RESOURCE_STATE_DIGEST_DOMAIN: &str = "draft.dcg.resource-state/v1";

/// The canonical material state of one Resource.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceState {
    /// What kind of thing this is.
    pub resource_kind: ResourceKindId,
    /// The exact contract under which these fields are to be interpreted.
    pub state_semantics: ResourceStateSemanticsRef,
    /// Where it was found — present only when the contract says the locator
    /// bears state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<ResourceLocator>,
    /// The digest of the resource's content, where it has content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<Digest>,
    /// The digest of the resource's semantic form, where the contract defines
    /// one — a structural reading that ignores incidental byte differences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_digest: Option<Digest>,
    /// Intrinsic attributes, canonically ordered.
    pub state_attributes: BTreeMap<String, AttributeValue>,
}

impl ResourceState {
    /// This state's canonical digest.
    pub fn digest(&self) -> FormatResult<ResourceStateDigest> {
        Ok(ResourceStateDigest(canonical_digest(
            RESOURCE_STATE_DIGEST_DOMAIN,
            self,
        )?))
    }

    /// Check this state against the contract its reference names.
    ///
    /// The reference is verified first: interpreting a state under a contract
    /// that has been redefined beneath its identifier would be worse than not
    /// checking at all.
    pub fn validate_against(&self, contract: &ResourceStateSemanticsContract) -> FormatResult<()> {
        self.state_semantics.verify(contract)?;

        match contract.locator_state_role {
            LocatorStateRole::StateBearing if self.locator.is_none() => {
                return Err(FormatError::Consistency(format!(
                    "semantics '{}' makes the locator state-bearing, so it must be present",
                    contract.id
                )));
            }
            LocatorStateRole::Informational if self.locator.is_some() => {
                return Err(FormatError::Consistency(format!(
                    "semantics '{}' treats the locator as informational, so it must not appear \
                     in material state",
                    contract.id
                )));
            }
            _ => {}
        }

        check_digest_field(
            "content_digest",
            contract.content_digest_interpretation,
            self.content_digest.is_some(),
            contract,
        )?;
        check_digest_field(
            "semantic_digest",
            contract.semantic_digest_interpretation,
            self.semantic_digest.is_some(),
            contract,
        )?;
        Ok(())
    }
}

fn check_digest_field(
    field: &str,
    interpretation: DigestInterpretation,
    present: bool,
    contract: &ResourceStateSemanticsContract,
) -> FormatResult<()> {
    match (interpretation, present) {
        (DigestInterpretation::Required, false) => Err(FormatError::Consistency(format!(
            "semantics '{}' requires {field}",
            contract.id
        ))),
        (DigestInterpretation::Absent, true) => Err(FormatError::Consistency(format!(
            "semantics '{}' defines no {field}, so it must not be present",
            contract.id
        ))),
        _ => Ok(()),
    }
}

/// The canonical digest of a [`ResourceState`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceStateDigest(Digest);

impl ResourceStateDigest {
    pub fn new(digest: Digest) -> Self {
        Self(digest)
    }

    pub fn digest(&self) -> &Digest {
        &self.0
    }
}

impl std::fmt::Display for ResourceStateDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantics::{
        AttributeInterpretation, NormalizationRule, PresenceAbsenceSemantics,
        ResourceStateSemanticsId,
    };

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

    fn state() -> ResourceState {
        ResourceState {
            resource_kind: ResourceKindId::parse("draft.filesystem/file").unwrap(),
            state_semantics: contract().reference().unwrap(),
            locator: Some(ResourceLocator::parse("app.txt").unwrap()),
            content_digest: Some(Digest::of_bytes(b"hello\n")),
            semantic_digest: None,
            state_attributes: BTreeMap::from([(
                "executable".to_string(),
                AttributeValue::Boolean(false),
            )]),
        }
    }

    #[test]
    fn a_valid_state_checks_against_its_contract() {
        state().validate_against(&contract()).unwrap();
    }

    #[test]
    fn two_resources_in_the_same_state_share_a_state_digest() {
        // There is no ResourceId in here, and this is the observable
        // consequence: identity is not state.
        let left = state();
        let mut right = state();
        right.locator = Some(ResourceLocator::parse("app.txt").unwrap());
        assert_eq!(left.digest().unwrap(), right.digest().unwrap());
    }

    #[test]
    fn changing_content_changes_the_state_digest() {
        let before = state().digest().unwrap();
        let mut after = state();
        after.content_digest = Some(Digest::of_bytes(b"hello world\n"));
        assert_ne!(before, after.digest().unwrap());
    }

    #[test]
    fn a_state_bearing_locator_is_part_of_identity() {
        let before = state().digest().unwrap();
        let mut moved = state();
        moved.locator = Some(ResourceLocator::parse("renamed.txt").unwrap());
        assert_ne!(before, moved.digest().unwrap());
    }

    #[test]
    fn the_semantics_reference_is_part_of_the_state_digest() {
        // Two states with identical content but different interpretation
        // contracts are not the same state, and must not compare equal.
        let before = state().digest().unwrap();
        let mut other_contract = contract();
        other_contract.id = ResourceStateSemanticsId::parse("acme.crm/record.v1").unwrap();
        let mut reinterpreted = state();
        reinterpreted.state_semantics = other_contract.reference().unwrap();
        assert_ne!(before, reinterpreted.digest().unwrap());
    }

    #[test]
    fn a_missing_required_digest_is_refused() {
        let mut missing = state();
        missing.content_digest = None;
        assert!(matches!(
            missing.validate_against(&contract()),
            Err(FormatError::Consistency(_))
        ));
    }

    #[test]
    fn a_field_the_contract_does_not_define_is_refused() {
        let mut extra = state();
        extra.semantic_digest = Some(Digest::of_bytes(b"x"));
        assert!(extra.validate_against(&contract()).is_err());
    }

    #[test]
    fn a_locator_role_cannot_drift_from_its_contract() {
        let mut informational = contract();
        informational.locator_state_role = LocatorStateRole::Informational;
        // The state still carries a locator, so it no longer matches — and the
        // reference check fires first, because the contract was redefined.
        assert!(state().validate_against(&informational).is_err());

        // Under a genuinely different, correctly referenced contract, the
        // locator's absence is what is required.
        let mut without = state();
        without.state_semantics = informational.reference().unwrap();
        without.locator = None;
        without.content_digest = Some(Digest::of_bytes(b"hello\n"));
        without.validate_against(&informational).unwrap();
    }

    #[test]
    fn attribute_order_does_not_affect_identity() {
        let mut forward = state();
        forward
            .state_attributes
            .insert("zebra".into(), AttributeValue::Integer(1));
        forward
            .state_attributes
            .insert("alpha".into(), AttributeValue::Integer(2));
        let mut reversed = state();
        reversed
            .state_attributes
            .insert("alpha".into(), AttributeValue::Integer(2));
        reversed
            .state_attributes
            .insert("zebra".into(), AttributeValue::Integer(1));
        assert_eq!(forward.digest().unwrap(), reversed.digest().unwrap());
    }

    #[test]
    fn the_wire_form_round_trips_and_omits_absent_fields() {
        let state = state();
        let encoded = serde_json::to_string(&state).unwrap();
        assert!(!encoded.contains("semantic_digest"), "{encoded}");
        assert_eq!(
            serde_json::from_str::<ResourceState>(&encoded).unwrap(),
            state
        );
    }
}
