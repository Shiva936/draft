//! Draft's own filesystem provider, as a real ProviderBinding.
//!
//! Every observation Draft makes has to name the binding that made it, because
//! a Baseline's `StateEvidenceRoot` records provenance and provenance means
//! "which configured attachment established this". Draft's filesystem observer
//! is not an exception to that model — it is the first instance of it.
//!
//! # Why Core declares a semantics contract at all
//!
//! Without one, `ResourceStateDigest` would be whatever the scanner happened
//! to hash, and two Draft versions could disagree about whether a mode change
//! altered material state while both called the result "the state digest".
//!
//! The contract makes those choices explicit and checkable:
//!
//! * a path **is** part of what a file is, so moving a file changes its state;
//! * content is required — a filesystem resource without content is not a
//!   filesystem resource;
//! * nothing is normalized, because normalizing would make two genuinely
//!   different files compare equal, and that is a claim about the domain that
//!   only a contract author may make;
//! * absence means the observer established nothing, so coverage evidence has
//!   to justify it rather than the absence speaking for itself.
//!
//! # Why the binding is created at initialization
//!
//! A project with no binding could observe nothing, so there is no useful
//! state in which the binding is missing. Creating it as part of the initial
//! project state means the invariant "a project always has somewhere to
//! observe from" holds from the first moment, rather than being established
//! lazily by whichever command happens to run first.

use std::collections::{BTreeMap, BTreeSet};

use draft_dcg_contract::capability::CapabilityId;
use draft_dcg_contract::ids::{ProjectId, ProviderBindingId};
use draft_dcg_contract::semantics::{
    AttributeInterpretation, DigestInterpretation, LocatorStateRole, NormalizationRule,
    PresenceAbsenceSemantics, ResourceStateSemanticsContract, ResourceStateSemanticsId,
    ResourceStateSemanticsRef,
};
use draft_dcg_contract::ProviderKindId;

use crate::project::provider::{ProviderBinding, ProviderBindingLifecycle};
use crate::project::provider_definition::{
    ConcurrencyPolicy, MergeCapability, ProviderDefinitionStore, ProviderOperationalProfile,
    ProviderSemanticDefinition, PublicationDelivery,
};
use crate::support::error::DraftResult;

/// The provider kind Draft's filesystem observer implements.
pub const FILESYSTEM_PROVIDER_KIND: &str = "draft.filesystem/local";

/// The semantics filesystem observations are interpreted under.
pub const FILESYSTEM_SEMANTICS_ID: &str = "draft.filesystem/state.v1";

/// The one binding id Draft's own filesystem provider uses.
///
/// Fixed rather than minted: it is Core's own provider, present in every
/// project, and a per-project id would make the same provider incomparable
/// across projects for no gain.
pub const FILESYSTEM_BINDING_ID: &str = "pbd_draftfilesystem";

/// The binding id for Draft's filesystem provider.
pub fn filesystem_binding_id() -> ProviderBindingId {
    ProviderBindingId::parse(FILESYSTEM_BINDING_ID)
        .expect("the built-in filesystem binding id is well-formed")
}

/// The semantics contract filesystem observations are read under.
pub fn semantics_contract() -> ResourceStateSemanticsContract {
    ResourceStateSemanticsContract {
        id: ResourceStateSemanticsId::parse(FILESYSTEM_SEMANTICS_ID)
            .expect("the built-in semantics id is well-formed"),
        // A path is part of what a file is: moving it changes state.
        locator_state_role: LocatorStateRole::StateBearing,
        attribute_interpretation: BTreeMap::from([
            // Whether a file is executable changes what it is.
            (
                "executable".to_string(),
                AttributeInterpretation::StateBearing,
            ),
            // Size and timing describe the observation, not the file's
            // identity — the content digest already covers what changed.
            ("size".to_string(), AttributeInterpretation::Informational),
            (
                "modified_at".to_string(),
                AttributeInterpretation::Informational,
            ),
        ]),
        content_digest_interpretation: DigestInterpretation::Required,
        // Semantic digests come from contributed extractors, not from Core's
        // own observation of bytes.
        semantic_digest_interpretation: DigestInterpretation::Absent,
        // Nothing. Normalizing would make two genuinely different files
        // compare equal, which Core has no standing to assert.
        normalization: vec![NormalizationRule::None],
        presence_absence_semantics: PresenceAbsenceSemantics::Unknown,
    }
}

/// The definition Draft's filesystem provider observes under.
pub fn semantic_definition() -> DraftResult<ProviderSemanticDefinition> {
    let contract = semantics_contract();
    Ok(ProviderSemanticDefinition {
        kind: ProviderKindId::parse(FILESYSTEM_PROVIDER_KIND)
            .expect("the built-in provider kind is well-formed"),
        produces_state_semantics: reference_to(&contract)?,
        locator_state_role: contract.locator_state_role,
        interpretation: serde_json::json!({}),
    })
}

fn reference_to(
    contract: &ResourceStateSemanticsContract,
) -> DraftResult<ResourceStateSemanticsRef> {
    contract.reference().map_err(|error| {
        crate::support::error::DraftError::new(
            crate::support::error::DraftErrorKind::CorruptData,
            error.to_string(),
        )
    })
}

/// What Draft's filesystem provider can operationally do.
pub fn operational_profile() -> ProviderOperationalProfile {
    ProviderOperationalProfile {
        capabilities: BTreeSet::from([CapabilityId::parse("draft.resource.observe/v1")
            .expect("the built-in observe capability is well-formed")]),
        // Files merge textually where they are text; Draft does not claim
        // semantic merge for content it does not interpret.
        merge_capability: MergeCapability::TextualThreeWay,
        // A local filesystem tolerates concurrent work and detects conflicts
        // by comparing state, rather than requiring an exclusive lease.
        concurrency_policy: ConcurrencyPolicy::OptimisticConflictDetection,
        publication_delivery: PublicationDelivery::IdempotentByKey,
        limits: serde_json::json!({}),
    }
}

/// Create Draft's filesystem binding, retaining its definition and profile.
///
/// Idempotent: the definition and profile are content-addressed, so re-running
/// initialization converges rather than failing. The binding itself is written
/// through the ordinary guarded transaction against `Absent`.
pub fn bind(
    project: ProjectId,
    definitions: &ProviderDefinitionStore,
) -> DraftResult<ProviderBinding> {
    let definition = semantic_definition()?;
    let profile = operational_profile();
    let semantic_definition_digest = definitions.add_definition(&definition)?;
    let operational_profile_digest = definitions.add_profile(&profile)?;

    Ok(ProviderBinding {
        generation: 0,
        id: filesystem_binding_id(),
        project,
        kind: definition.kind.clone(),
        current_semantic_definition: semantic_definition_digest,
        current_operational_profile: operational_profile_digest,
        lifecycle: ProviderBindingLifecycle::Active,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectId {
        ProjectId::parse("prj_000000000001").unwrap()
    }

    #[test]
    fn the_contract_declares_something_state_bearing() {
        // A contract that establishes no material state gives every resource
        // the same digest, which the SDK refuses outright.
        semantics_contract().validate().unwrap();
    }

    #[test]
    fn a_definition_agrees_with_the_contract_it_names() {
        let definition = semantic_definition().unwrap();
        definition.validate_against(&semantics_contract()).unwrap();
    }

    #[test]
    fn a_path_is_state_bearing_so_moving_a_file_changes_its_state() {
        // The choice that makes filesystem state mean what people expect: a
        // file moved to another directory is not the same resource state.
        assert_eq!(
            semantics_contract().locator_state_role,
            LocatorStateRole::StateBearing
        );
    }

    #[test]
    fn observation_timing_is_informational_not_material() {
        // Re-observing an unchanged file must not change its state digest
        // merely because time moved.
        let contract = semantics_contract();
        assert_eq!(
            contract.attribute_interpretation.get("modified_at"),
            Some(&AttributeInterpretation::Informational)
        );
        assert_eq!(
            contract.attribute_interpretation.get("executable"),
            Some(&AttributeInterpretation::StateBearing)
        );
    }

    #[test]
    fn absence_must_be_justified_by_coverage_rather_than_speaking_for_itself() {
        assert_eq!(
            semantics_contract().presence_absence_semantics,
            PresenceAbsenceSemantics::Unknown
        );
    }

    #[test]
    fn binding_is_idempotent_and_active() {
        let directory = tempfile::tempdir().unwrap();
        let definitions = ProviderDefinitionStore::new(directory.path());

        let first = bind(project(), &definitions).unwrap();
        let second = bind(project(), &definitions).unwrap();

        assert_eq!(first, second, "re-initialization converges");
        assert_eq!(first.lifecycle, ProviderBindingLifecycle::Active);
        assert_eq!(first.id, filesystem_binding_id());
        assert!(definitions
            .definition_is_retained(&first.current_semantic_definition)
            .unwrap());
    }
}
