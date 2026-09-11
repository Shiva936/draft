//! Retained resource-state semantics contracts, one per identifier, forever.
//!
//! A `ResourceStateSemanticsId` names a complete interpretation contract: which
//! fields bear state, how the locator participates, how digests are read, how
//! attributes are normalized. Baselines commit to those identifiers, so the
//! meaning behind one cannot be allowed to move.
//!
//! ```text
//! same id + same digest       -> the same contract
//! same id + DIFFERENT digest  -> SEMANTIC IDENTITY CONFLICT -> reject
//! ```
//!
//! Without that rule a vendor could keep an identifier and change what it means
//! — redefining what counts as "unchanged" beneath every historical Baseline
//! that cited it. Every one of those Baselines would still verify, byte for
//! byte, while silently meaning something else. Requiring a new identifier is
//! what keeps a historical claim readable as the claim that was made.
//!
//! # Retention is what makes history verifiable later
//!
//! Contracts are **retained**, not resolved on demand from whatever is
//! installed. They are GC roots and travel in DraftPack exports, so verifying
//! what a five-year-old Baseline meant never requires the extension that
//! contributed it to still exist — or to still say the same thing.
//!
//! The create-once storage binding does the enforcing: an identifier is bound
//! to its contract's canonical digest on first registration, and the binding
//! can never be replaced. Re-registering identical bytes converges, which is
//! what lets an install be retried; different bytes are refused.

use draft_dcg_contract::{
    ResourceStateSemanticsContract, ResourceStateSemanticsId, ResourceStateSemanticsRef,
};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::immutable_store::{ImmutableFactStore, StoreOutcome};

/// What registering a contract did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// The identifier was bound to this contract for the first time.
    Registered,
    /// The identifier was already bound to this exact contract.
    ///
    /// Not an error: reinstalling a package, or retrying an install after a
    /// crash, must converge rather than fail against itself.
    AlreadyRegistered,
}

/// The contracts a project has accepted, retained for the life of its history.
#[derive(Debug, Clone)]
pub struct SemanticsContractRegistry {
    contracts: ImmutableFactStore<ResourceStateSemanticsContract>,
}

impl SemanticsContractRegistry {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            contracts: ImmutableFactStore::new(directory),
        }
    }

    /// Accept a contributed contract.
    ///
    /// A conflict here is a **semantic identity conflict**, not a version skew
    /// to be resolved by preferring one side: the two contracts disagree about
    /// what an identifier already in use means. A vendor changing semantics
    /// mints a new namespaced identifier, and the same refusal applies to any
    /// package or catalog attempting redefinition.
    pub fn register(
        &self,
        contract: &ResourceStateSemanticsContract,
    ) -> DraftResult<RegistrationOutcome> {
        // Validated before it is stored: an incoherent contract that nothing
        // could interpret must not become the permanent meaning of an id.
        contract
            .validate()
            .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?;

        let key = storage_key(&contract.id);
        match self.contracts.put(&key, contract) {
            Ok(StoreOutcome::Created) => Ok(RegistrationOutcome::Registered),
            Ok(StoreOutcome::AlreadyIdentical) => Ok(RegistrationOutcome::AlreadyRegistered),
            Err(error) if error.kind == DraftErrorKind::CorruptData => {
                crate::support::telemetry::Counter::SemanticsContractConflicts.increment();
                Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "semantics identifier '{}' is already bound to a different contract. One \
                     identifier means exactly one contract: publish the changed semantics under \
                     a new identifier instead.",
                    contract.id
                ),
            )
            .with_suggestion(
                "Redefining an accepted identifier would change what every Baseline that cited \
                 it means.",
            ))
            }
            Err(error) => Err(error),
        }
    }

    /// The contract an identifier is bound to, if any.
    pub fn contract(
        &self,
        id: &ResourceStateSemanticsId,
    ) -> DraftResult<Option<ResourceStateSemanticsContract>> {
        self.contracts.get(&storage_key(id))
    }

    /// Resolve an exact reference, verifying both halves.
    ///
    /// The digest is checked as well as the identifier, so a contract whose
    /// stored bytes have moved is caught here rather than silently
    /// reinterpreting the state that cited it.
    pub fn resolve(
        &self,
        reference: &ResourceStateSemanticsRef,
    ) -> DraftResult<ResourceStateSemanticsContract> {
        let Some(contract) = self.contract(&reference.id)? else {
            return Err(DraftError::new(
                DraftErrorKind::NotFound,
                format!(
                    "semantics contract '{}' is not retained; historical state cannot be \
                     interpreted without it",
                    reference.id
                ),
            ));
        };
        reference
            .verify(&contract)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;
        Ok(contract)
    }
}

/// The storage key for an identifier.
///
/// A namespaced id contains `/` and `.`, neither of which may become a path
/// component here — a contract must not be able to write outside the registry
/// by virtue of its own name.
fn storage_key(id: &ResourceStateSemanticsId) -> String {
    id.as_namespaced()
        .qualified()
        .replace(['/', '.', '\\'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::semantics::{
        AttributeInterpretation, DigestInterpretation, LocatorStateRole, NormalizationRule,
        PresenceAbsenceSemantics,
    };
    use std::collections::BTreeMap;

    fn contract(id: &str, role: LocatorStateRole) -> ResourceStateSemanticsContract {
        ResourceStateSemanticsContract {
            id: ResourceStateSemanticsId::parse(id).unwrap(),
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

    fn registry(directory: &tempfile::TempDir) -> SemanticsContractRegistry {
        SemanticsContractRegistry::new(directory.path())
    }

    #[test]
    fn a_contributed_contract_is_retained_and_resolvable() {
        let directory = tempfile::tempdir().unwrap();
        let registry = registry(&directory);
        let contract = contract("acme.crm/record.v1", LocatorStateRole::StateBearing);

        assert_eq!(
            registry.register(&contract).unwrap(),
            RegistrationOutcome::Registered
        );
        let reference = contract.reference().unwrap();
        assert_eq!(registry.resolve(&reference).unwrap(), contract);
    }

    #[test]
    fn reinstalling_the_same_contract_converges() {
        // Retrying an install after a crash must not fail against itself.
        let directory = tempfile::tempdir().unwrap();
        let registry = registry(&directory);
        let contract = contract("acme.crm/record.v1", LocatorStateRole::StateBearing);
        registry.register(&contract).unwrap();
        for _ in 0..3 {
            assert_eq!(
                registry.register(&contract).unwrap(),
                RegistrationOutcome::AlreadyRegistered
            );
        }
    }

    #[test]
    fn redefining_an_accepted_identifier_is_a_semantic_identity_conflict() {
        // The scenario the rule exists for. Every Baseline that cited this id
        // would still verify byte for byte while silently meaning something
        // else.
        let directory = tempfile::tempdir().unwrap();
        let registry = registry(&directory);
        registry
            .register(&contract(
                "acme.crm/record.v1",
                LocatorStateRole::StateBearing,
            ))
            .unwrap();

        let error = registry
            .register(&contract(
                "acme.crm/record.v1",
                LocatorStateRole::Informational,
            ))
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);

        // And the original meaning survives the attempt.
        let original = contract("acme.crm/record.v1", LocatorStateRole::StateBearing);
        assert_eq!(
            registry.contract(&original.id).unwrap().unwrap(),
            original,
            "a refused redefinition must not have altered what the id means"
        );
    }

    #[test]
    fn a_vendor_may_publish_changed_semantics_under_a_new_identifier() {
        // The supported path: mint a new name, leave the old meaning intact.
        let directory = tempfile::tempdir().unwrap();
        let registry = registry(&directory);
        registry
            .register(&contract(
                "acme.crm/record.v1",
                LocatorStateRole::StateBearing,
            ))
            .unwrap();
        registry
            .register(&contract(
                "acme.crm/record-relocatable.v1",
                LocatorStateRole::Informational,
            ))
            .unwrap();

        let original = contract("acme.crm/record.v1", LocatorStateRole::StateBearing);
        assert_eq!(registry.contract(&original.id).unwrap().unwrap(), original);
    }

    #[test]
    fn an_incoherent_contract_never_becomes_the_meaning_of_an_id() {
        let directory = tempfile::tempdir().unwrap();
        let registry = registry(&directory);
        let mut empty = contract("acme.crm/record.v1", LocatorStateRole::Informational);
        empty.content_digest_interpretation = DigestInterpretation::Absent;
        empty.semantic_digest_interpretation = DigestInterpretation::Absent;
        empty.attribute_interpretation.clear();

        assert_eq!(
            registry.register(&empty).unwrap_err().kind,
            DraftErrorKind::Validation
        );
        assert!(registry.contract(&empty.id).unwrap().is_none());
    }

    #[test]
    fn an_unretained_contract_is_reported_rather_than_guessed_at() {
        // Historical state cannot be interpreted without it, and inventing an
        // interpretation would be worse than saying so.
        let directory = tempfile::tempdir().unwrap();
        let contract = contract("acme.crm/record.v1", LocatorStateRole::StateBearing);
        let error = registry(&directory)
            .resolve(&contract.reference().unwrap())
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::NotFound);
    }

    #[test]
    fn a_reference_whose_digest_does_not_match_is_refused() {
        // Both halves are checked: an id that resolves is not enough.
        let directory = tempfile::tempdir().unwrap();
        let registry = registry(&directory);
        registry
            .register(&contract(
                "acme.crm/record.v1",
                LocatorStateRole::StateBearing,
            ))
            .unwrap();

        let mismatched = contract("acme.crm/record.v1", LocatorStateRole::Informational)
            .reference()
            .unwrap();
        assert_eq!(
            registry.resolve(&mismatched).unwrap_err().kind,
            DraftErrorKind::CorruptData
        );
    }

    #[test]
    fn an_identifier_cannot_escape_the_registry_through_its_own_name() {
        // Namespaced ids contain `/` and `.`; neither may become a path
        // component.
        let key = storage_key(&ResourceStateSemanticsId::parse("acme.crm/record.v1").unwrap());
        assert!(!key.contains('/') && !key.contains('.') && !key.contains('\\'));
    }

    #[test]
    fn two_identifiers_are_retained_independently() {
        let directory = tempfile::tempdir().unwrap();
        let registry = registry(&directory);
        let first = contract("acme.crm/record.v1", LocatorStateRole::StateBearing);
        let second = contract("acme.docs/page.v1", LocatorStateRole::Informational);
        registry.register(&first).unwrap();
        registry.register(&second).unwrap();

        assert_eq!(
            registry.resolve(&first.reference().unwrap()).unwrap(),
            first
        );
        assert_eq!(
            registry.resolve(&second.reference().unwrap()).unwrap(),
            second
        );
    }
}
