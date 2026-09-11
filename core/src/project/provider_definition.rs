//! Immutable provider definitions and operational profiles.
//!
//! Two independent immutable facts, deliberately separate because they answer
//! different questions and changing one must not imply the other:
//!
//! * a **semantic definition** says what a provider's namespace, roots and
//!   endpoints *mean*, and which resource-state semantics its observations are
//!   interpreted under. Changing it changes what an observation is claiming;
//! * an **operational profile** says how the provider is *operated* — its
//!   capabilities, merge behaviour, concurrency policy, delivery semantics and
//!   limits. Changing it changes how an action would be performed.
//!
//! That split is why a Baseline's accepted provenance names a definition and
//! never a profile. Re-tuning how a provider is driven cannot retroactively
//! change what it once observed, so an operational change must leave every
//! historical Baseline byte-identical.
//!
//! # Both are created once and retained
//!
//! Definitions and profiles are never edited: a change mints a new fact with a
//! new digest, and the old one stays. Historical verification, reads, GC
//! reachability and explicit recovery all need the exact fact that was in force
//! at the time, and none of them can be served by whatever the binding points
//! at now.
//!
//! Retention is not permission, though. That a historical definition still
//! exists and would still work is **never** sufficient authority to perform a
//! new external side effect — that requires the binding to still select it
//! exactly, which is `provider.rs`'s job.

use std::collections::BTreeSet;

use draft_dcg_contract::semantics::LocatorStateRole;
use draft_dcg_contract::{
    CapabilityId, ProviderKindId, ProviderOperationalProfileDigest,
    ProviderSemanticDefinitionDigest, ResourceStateSemanticsContract, ResourceStateSemanticsRef,
};
use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use crate::support::immutable_store::{ImmutableFactStore, StoreOutcome};

/// How a provider handles two concurrent changes to one resource.
///
/// Independent of [`MergeCapability`]: whether a provider *can* merge and
/// whether it *permits* concurrency are different facts, and every combination
/// is legitimate. A store that cannot merge may still safely allow optimistic
/// concurrency by detecting conflicts and refusing; one that merges perfectly
/// may still require an exclusive lease for unrelated reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyPolicy {
    Concurrent,
    OptimisticConflictDetection,
    ExclusiveLeaseRequired,
    ProviderDefined,
}

/// What a provider can do when two changes must be combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeCapability {
    TextualThreeWay,
    Semantic,
    Structural,
    OperationCommutation,
    Crdt,
    Manual,
    ChooseOne,
    Replace,
    Unsupported,
}

/// What a provider guarantees about a repeated delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationDelivery {
    IdempotentByKey,
    ReconcileByClientKey,
    QueryByClientKey,
    NonIdempotent,
}

/// What a provider's namespace, roots and endpoints mean.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSemanticDefinition {
    pub kind: ProviderKindId,
    /// The semantics its observations are interpreted under.
    pub produces_state_semantics: ResourceStateSemanticsRef,
    /// The locator role this definition claims.
    ///
    /// Duplicated from the referenced contract deliberately, and validated
    /// against it: a definition that disagreed with its own contract would
    /// interpret every observation it produced under a rule the contract does
    /// not state.
    pub locator_state_role: LocatorStateRole,
    /// Provider-specific interpretation rules, opaque to Core.
    pub interpretation: serde_json::Value,
}

impl ProviderSemanticDefinition {
    /// This definition's canonical digest.
    pub fn digest(&self) -> DraftResult<ProviderSemanticDefinitionDigest> {
        let digest = try_canonical_hash(self)?;
        Ok(ProviderSemanticDefinitionDigest::new(
            draft_dcg_contract::Digest::parse(digest)
                .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
        ))
    }

    /// Check the definition against the semantics contract it names.
    pub fn validate_against(&self, contract: &ResourceStateSemanticsContract) -> DraftResult<()> {
        self.produces_state_semantics
            .verify(contract)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;
        if self.locator_state_role != contract.locator_state_role {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "provider definition claims locator role {:?} but semantics contract '{}' \
                     states {:?}; every observation it produced would be interpreted under a \
                     rule the contract does not state",
                    self.locator_state_role, contract.id, contract.locator_state_role
                ),
            ));
        }
        Ok(())
    }
}

/// How a provider is operated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderOperationalProfile {
    pub capabilities: BTreeSet<CapabilityId>,
    pub merge_capability: MergeCapability,
    pub concurrency_policy: ConcurrencyPolicy,
    pub publication_delivery: PublicationDelivery,
    /// Provider-specific operational limits, opaque to Core.
    pub limits: serde_json::Value,
}

impl ProviderOperationalProfile {
    pub fn digest(&self) -> DraftResult<ProviderOperationalProfileDigest> {
        let digest = try_canonical_hash(self)?;
        Ok(ProviderOperationalProfileDigest::new(
            draft_dcg_contract::Digest::parse(digest)
                .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
        ))
    }

    /// Whether this profile permits `capability`.
    pub fn permits(&self, capability: &CapabilityId) -> bool {
        self.capabilities.contains(capability)
    }
}

/// Create-once storage for definitions and profiles.
///
/// Each has its own creation journal because each is an independent fact. A
/// definition audited only by a later binding mutation could be created and
/// never recorded, which is the failure this separation prevents.
#[derive(Debug, Clone)]
pub struct ProviderDefinitionStore {
    definitions: ImmutableFactStore<ProviderSemanticDefinition>,
    profiles: ImmutableFactStore<ProviderOperationalProfile>,
}

impl ProviderDefinitionStore {
    pub fn new(directory: impl AsRef<std::path::Path>) -> Self {
        let directory = directory.as_ref();
        Self {
            definitions: ImmutableFactStore::new(directory.join("semantic-definitions")),
            profiles: ImmutableFactStore::new(directory.join("operational-profiles")),
        }
    }

    /// Store a definition under its own digest.
    ///
    /// Content-addressed, so re-adding an identical definition converges and a
    /// changed one is simply a different fact rather than a conflict — there is
    /// no identifier for it to disagree about.
    pub fn add_definition(
        &self,
        definition: &ProviderSemanticDefinition,
    ) -> DraftResult<ProviderSemanticDefinitionDigest> {
        let digest = definition.digest()?;
        self.definitions.put(&key(digest.digest()), definition)?;
        Ok(digest)
    }

    pub fn definition(
        &self,
        digest: &ProviderSemanticDefinitionDigest,
    ) -> DraftResult<Option<ProviderSemanticDefinition>> {
        self.definitions.get(&key(digest.digest()))
    }

    pub fn add_profile(
        &self,
        profile: &ProviderOperationalProfile,
    ) -> DraftResult<ProviderOperationalProfileDigest> {
        let digest = profile.digest()?;
        self.profiles.put(&key(digest.digest()), profile)?;
        Ok(digest)
    }

    pub fn profile(
        &self,
        digest: &ProviderOperationalProfileDigest,
    ) -> DraftResult<Option<ProviderOperationalProfile>> {
        self.profiles.get(&key(digest.digest()))
    }

    /// Every semantic definition this project holds, by digest.
    ///
    /// Content-addressed facts have no separate index, so this reads the store
    /// itself. A definition that exists but is listed nowhere would be a
    /// definition a reader cannot audit.
    pub fn list_definitions(&self) -> DraftResult<Vec<ProviderSemanticDefinition>> {
        let mut definitions = Vec::new();
        for id in self.definitions.list_ids()? {
            if let Some(definition) = self.definitions.get(&id)? {
                definitions.push(definition);
            }
        }
        definitions.sort_by_key(|definition| definition.kind.to_string());
        Ok(definitions)
    }

    /// Every operational profile this project holds, by digest.
    pub fn list_profiles(&self) -> DraftResult<Vec<ProviderOperationalProfile>> {
        let mut profiles = Vec::new();
        for id in self.profiles.list_ids()? {
            if let Some(profile) = self.profiles.get(&id)? {
                profiles.push(profile);
            }
        }
        Ok(profiles)
    }

    /// Whether an exact definition is still loadable.
    ///
    /// For historical verification and recovery. Availability is deliberately
    /// **not** an authorization: see the module docs.
    pub fn definition_is_retained(
        &self,
        digest: &ProviderSemanticDefinitionDigest,
    ) -> DraftResult<bool> {
        Ok(self.definition(digest)?.is_some())
    }
}

/// A digest's storage key. `sha256:` carries a colon, which is not a filename.
fn key(digest: &draft_dcg_contract::Digest) -> String {
    digest.as_str().replace(':', "_")
}

fn _assert_outcome_is_used(outcome: StoreOutcome) -> bool {
    matches!(outcome, StoreOutcome::Created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::semantics::{
        AttributeInterpretation, DigestInterpretation, NormalizationRule, PresenceAbsenceSemantics,
    };
    use draft_dcg_contract::ResourceStateSemanticsId;
    use std::collections::BTreeMap;

    fn contract(role: LocatorStateRole) -> ResourceStateSemanticsContract {
        ResourceStateSemanticsContract {
            id: ResourceStateSemanticsId::parse("draft.filesystem/file.v1").unwrap(),
            locator_state_role: role,
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

    fn definition(role: LocatorStateRole) -> ProviderSemanticDefinition {
        ProviderSemanticDefinition {
            kind: ProviderKindId::parse("draft.filesystem/local").unwrap(),
            produces_state_semantics: contract(role).reference().unwrap(),
            locator_state_role: role,
            interpretation: serde_json::json!({"root": "/srv"}),
        }
    }

    fn profile(
        merge: MergeCapability,
        concurrency: ConcurrencyPolicy,
    ) -> ProviderOperationalProfile {
        ProviderOperationalProfile {
            capabilities: BTreeSet::from([
                CapabilityId::parse("draft.resource.observe/v1").unwrap()
            ]),
            merge_capability: merge,
            concurrency_policy: concurrency,
            publication_delivery: PublicationDelivery::IdempotentByKey,
            limits: serde_json::json!({"max_entries": 1000}),
        }
    }

    fn store(directory: &tempfile::TempDir) -> ProviderDefinitionStore {
        ProviderDefinitionStore::new(directory.path())
    }

    /// Every merge capability, paired with every concurrency policy.
    const MERGE_CAPABILITIES: [MergeCapability; 9] = [
        MergeCapability::TextualThreeWay,
        MergeCapability::Semantic,
        MergeCapability::Structural,
        MergeCapability::OperationCommutation,
        MergeCapability::Crdt,
        MergeCapability::Manual,
        MergeCapability::ChooseOne,
        MergeCapability::Replace,
        MergeCapability::Unsupported,
    ];

    const CONCURRENCY_POLICIES: [ConcurrencyPolicy; 4] = [
        ConcurrencyPolicy::Concurrent,
        ConcurrencyPolicy::OptimisticConflictDetection,
        ConcurrencyPolicy::ExclusiveLeaseRequired,
        ConcurrencyPolicy::ProviderDefined,
    ];

    #[test]
    fn every_merge_and_concurrency_pairing_is_a_distinct_storable_profile() {
        // The two axes are independent facts, so no pairing may be rejected,
        // silently normalised, or collapsed into another. Storing all 36 and
        // requiring 36 distinct digests proves both halves: each is accepted,
        // and none quietly becomes a different profile on the way in.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let mut digests = BTreeSet::new();

        for merge in MERGE_CAPABILITIES {
            for concurrency in CONCURRENCY_POLICIES {
                let digest = store
                    .add_profile(&profile(merge, concurrency))
                    .unwrap_or_else(|error| {
                        panic!("{merge:?} + {concurrency:?} was refused: {error:?}")
                    });
                assert!(
                    digests.insert(digest),
                    "{merge:?} + {concurrency:?} collapsed onto another pairing"
                );
            }
        }
        assert_eq!(digests.len(), 36);
    }

    #[test]
    fn a_store_that_cannot_merge_may_still_choose_either_concurrency_answer() {
        // The pairings the independence claim actually rests on. "Cannot
        // merge" is a statement about combining two changes; it says nothing
        // about whether two may be in flight. A store that detects conflicts
        // and refuses is safe without merging, and one that demands an
        // exclusive lease is safe by excluding the case entirely.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);

        for concurrency in [
            ConcurrencyPolicy::OptimisticConflictDetection,
            ConcurrencyPolicy::ExclusiveLeaseRequired,
        ] {
            let stored = profile(MergeCapability::Unsupported, concurrency);
            let digest = store.add_profile(&stored).unwrap();
            assert_eq!(store.profile(&digest).unwrap().unwrap(), stored);
        }
    }

    #[test]
    fn a_definition_must_agree_with_the_contract_it_names() {
        let stated = LocatorStateRole::StateBearing;
        definition(stated)
            .validate_against(&contract(stated))
            .unwrap();

        // A definition disagreeing with its own contract would interpret every
        // observation it produced under a rule the contract does not state.
        let mut disagreeing = definition(stated);
        disagreeing.locator_state_role = LocatorStateRole::Informational;
        let error = disagreeing.validate_against(&contract(stated)).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);
    }

    #[test]
    fn a_definition_will_not_validate_against_a_redefined_contract() {
        // The reference carries the contract digest, so a contract whose
        // meaning moved is caught before anything is interpreted under it.
        let definition = definition(LocatorStateRole::StateBearing);
        let error = definition
            .validate_against(&contract(LocatorStateRole::Informational))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn definitions_and_profiles_are_stored_and_retained_independently() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);

        let definition_digest = store
            .add_definition(&definition(LocatorStateRole::StateBearing))
            .unwrap();
        let profile_digest = store
            .add_profile(&profile(
                MergeCapability::TextualThreeWay,
                ConcurrencyPolicy::Concurrent,
            ))
            .unwrap();

        assert_eq!(
            store.definition(&definition_digest).unwrap().unwrap(),
            definition(LocatorStateRole::StateBearing)
        );
        assert!(store.definition_is_retained(&definition_digest).unwrap());
        assert!(store.profile(&profile_digest).unwrap().is_some());
    }

    #[test]
    fn re_adding_an_identical_definition_converges() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let definition = definition(LocatorStateRole::StateBearing);
        let first = store.add_definition(&definition).unwrap();
        let second = store.add_definition(&definition).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn a_changed_definition_is_a_different_fact_rather_than_a_conflict() {
        // Content-addressed, so there is no identifier for two versions to
        // disagree about — and both stay retained.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let original = store
            .add_definition(&definition(LocatorStateRole::StateBearing))
            .unwrap();
        let changed = store
            .add_definition(&definition(LocatorStateRole::Informational))
            .unwrap();

        assert_ne!(original, changed);
        assert!(store.definition_is_retained(&original).unwrap());
        assert!(store.definition_is_retained(&changed).unwrap());
    }

    #[test]
    fn an_operational_change_does_not_touch_the_semantic_definition() {
        // The separation that keeps historical Baselines byte-identical when a
        // provider is merely re-tuned.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let definition_digest = store
            .add_definition(&definition(LocatorStateRole::StateBearing))
            .unwrap();

        let first = store
            .add_profile(&profile(
                MergeCapability::TextualThreeWay,
                ConcurrencyPolicy::Concurrent,
            ))
            .unwrap();
        let retuned = store
            .add_profile(&profile(
                MergeCapability::Manual,
                ConcurrencyPolicy::ExclusiveLeaseRequired,
            ))
            .unwrap();

        assert_ne!(first, retuned);
        assert_eq!(
            definition_digest,
            definition(LocatorStateRole::StateBearing).digest().unwrap(),
            "the definition is untouched by any operational change"
        );
    }

    #[test]
    fn merge_capability_and_concurrency_policy_are_independent() {
        // Every combination is legitimate: a store that cannot merge may still
        // safely detect conflicts and refuse, and one that merges perfectly may
        // still require a lease for unrelated reasons.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        for concurrency in [
            ConcurrencyPolicy::Concurrent,
            ConcurrencyPolicy::OptimisticConflictDetection,
            ConcurrencyPolicy::ExclusiveLeaseRequired,
            ConcurrencyPolicy::ProviderDefined,
        ] {
            store
                .add_profile(&profile(MergeCapability::Unsupported, concurrency))
                .unwrap_or_else(|error| panic!("{concurrency:?}: {error}"));
        }
    }

    #[test]
    fn a_profile_states_which_capabilities_it_permits() {
        let profile = profile(MergeCapability::Replace, ConcurrencyPolicy::Concurrent);
        assert!(profile.permits(&CapabilityId::parse("draft.resource.observe/v1").unwrap()));
        assert!(!profile.permits(&CapabilityId::parse("draft.publish/v1").unwrap()));
    }

    #[test]
    fn an_absent_definition_is_reported_as_absent() {
        let directory = tempfile::tempdir().unwrap();
        let missing = definition(LocatorStateRole::StateBearing).digest().unwrap();
        assert!(!store(&directory).definition_is_retained(&missing).unwrap());
    }
}
