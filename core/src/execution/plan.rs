//! Planned Operations, and the route they are authorized against.
//!
//! An Operation constructs an **explicit** [`ProviderRouteRef`] at planning
//! time and carries it. The plan is then frozen: it is never re-resolved
//! against whatever the binding points at when it comes to run.
//!
//! # Why the plan does not follow the binding
//!
//! Re-resolving would be the convenient behaviour and the wrong one. An
//! Operation is planned, reviewed and authorized against a specific
//! interpretation of a provider. If execution silently adopted the binding's
//! current route, the thing that ran would not be the thing that was approved —
//! and nothing in the record would show the difference.
//!
//! So before the first external side effect, execution takes the binding's lock
//! and requires exact equality with its current pointers. If either has moved,
//! the plan is stale and the Operation is refused. That the planned definition
//! is still retained and would still work is not a reason to proceed; it is the
//! reasoning the rule exists to forbid.
//!
//! Refusal is recoverable: re-plan against the current route, re-authorize, and
//! run. What is not available is proceeding as though nothing changed.

use draft_dcg_contract::ids::{OperationId, RevisionPackId};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::{OperationKindId, ProviderRouteRef};
use serde::{Deserialize, Serialize};

use crate::project::provider::{ProviderBindingGuard, ProviderBindingStore};
use crate::support::error::DraftResult;

/// An Operation that has been planned but not yet performed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedOperation {
    pub id: OperationId,
    /// The revision this Operation belongs to.
    pub change_revision: RevisionPackId,
    pub kind: OperationKindId,
    /// The exact route this was planned and authorized against.
    ///
    /// Frozen. Execution validates it; it never re-derives it.
    pub route: ProviderRouteRef,
    pub planned_at: Timestamp,
}

/// The durable fact that one exact route was authorized for one Operation.
///
/// Records the binding generation it was validated against, so a later reader
/// can tell *which* state of the binding authorized the effect rather than
/// having to assume the current one did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAuthorization {
    pub operation: OperationId,
    pub change_revision: RevisionPackId,
    pub route: ProviderRouteRef,
    pub binding_generation: u64,
}

impl PlannedOperation {
    /// Validate this plan against the binding's current pointers.
    ///
    /// Must be called with the binding's lock held, and the lock must stay held
    /// through the durable authorization that follows — otherwise the binding
    /// could move between the check and the commit, which is the whole window
    /// being closed.
    pub fn authorize(&self, guard: &ProviderBindingGuard<'_>) -> DraftResult<()> {
        ProviderBindingStore::require_selects(guard, &self.route)?;
        Ok(())
    }

    /// Validate the plan and durably commit its authorization, under a single
    /// hold of the binding lock.
    ///
    /// # Why the commit is inside the lock and the effect is outside it
    ///
    /// Validating and then committing under separate holds would reopen the
    /// window this rule exists to close: the binding could be retargeted in
    /// between, and the effect would proceed authorized against a route the
    /// project no longer selects.
    ///
    /// Holding the lock across the external effect would be worse in a
    /// different way. A provider's correctness lock would then be held for
    /// however long an arbitrary external system takes, so one slow provider
    /// would block every binding mutation in the project — and a hung one
    /// would block them forever.
    ///
    /// So the ordering is: validate, commit the authorization, release, then
    /// act. §2.20's two cases follow from it and there is no third:
    ///
    /// - the binding moves first → validation fails → the provider is never
    ///   invoked;
    /// - authorization commits first → the exact authorized route proceeds,
    ///   and a later mutation governs only new plans.
    pub fn authorize_and_commit<T>(
        &self,
        guard: &ProviderBindingGuard<'_>,
        commit: impl FnOnce(&ExecutionAuthorization) -> DraftResult<T>,
    ) -> DraftResult<T> {
        let binding = ProviderBindingStore::require_selects(guard, &self.route)?;
        commit(&ExecutionAuthorization {
            operation: self.id.clone(),
            change_revision: self.change_revision.clone(),
            route: self.route.clone(),
            binding_generation: binding.generation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::provider::{ProviderBinding, ProviderBindingLifecycle};
    use draft_dcg_contract::ids::{ProjectId, ProviderBindingId};
    use draft_dcg_contract::{
        Digest, ProviderKindId, ProviderOperationalProfileDigest, ProviderProvenanceRef,
        ProviderSemanticDefinitionDigest,
    };

    fn binding_id() -> ProviderBindingId {
        ProviderBindingId::parse("pbd_000000000001").unwrap()
    }

    fn definition(seed: &[u8]) -> ProviderSemanticDefinitionDigest {
        ProviderSemanticDefinitionDigest::new(Digest::of_bytes(seed))
    }

    fn profile(seed: &[u8]) -> ProviderOperationalProfileDigest {
        ProviderOperationalProfileDigest::new(Digest::of_bytes(seed))
    }

    fn binding(definition_seed: &[u8], profile_seed: &[u8]) -> ProviderBinding {
        ProviderBinding {
            generation: 0,
            id: binding_id(),
            project: ProjectId::parse("prj_000000000001").unwrap(),
            kind: ProviderKindId::parse("draft.filesystem/local").unwrap(),
            current_semantic_definition: definition(definition_seed),
            current_operational_profile: profile(profile_seed),
            lifecycle: ProviderBindingLifecycle::Active,
        }
    }

    fn planned() -> PlannedOperation {
        PlannedOperation {
            id: OperationId::parse("op_000000000001").unwrap(),
            change_revision: RevisionPackId::parse("rpk_000000000001").unwrap(),
            kind: OperationKindId::parse("draft.change/edit").unwrap(),
            route: ProviderRouteRef {
                provenance: ProviderProvenanceRef {
                    binding: binding_id(),
                    semantic_definition: definition(b"SD1"),
                },
                operational_profile: profile(b"OP1"),
            },
            planned_at: Timestamp::from_unix_nanos(1_000),
        }
    }

    fn store(directory: &tempfile::TempDir) -> ProviderBindingStore {
        ProviderBindingStore::new(directory.path())
    }

    #[test]
    fn a_plan_matching_the_current_route_is_authorized() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.bind(&binding(b"SD1", b"OP1")).unwrap();

        store
            .with_locked_record(&binding_id(), |guard| planned().authorize(guard))
            .unwrap();
    }

    #[test]
    fn a_plan_is_refused_once_the_binding_has_moved() {
        // The convenient behaviour would be to adopt the binding's new route.
        // Then the thing that ran would not be the thing that was approved, and
        // nothing in the record would show it.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.bind(&binding(b"SD1", b"OP1")).unwrap();
        store
            .retarget(&binding_id(), definition(b"SD2"), profile(b"OP1"))
            .unwrap();

        let error = store
            .with_locked_record(&binding_id(), |guard| planned().authorize(guard))
            .unwrap_err();
        assert!(
            error.message.contains("still retained does not authorize"),
            "{}",
            error.message
        );
    }

    #[test]
    fn case_a_a_binding_that_moves_first_stops_the_provider_being_invoked() {
        // §2.20 Case A. The binding mutation commits before dispatch, so the
        // planned route is no longer current and the authorization is refused
        // *before* anything durable is written. The provider is never reached.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.bind(&binding(b"SD1", b"OP1")).unwrap();
        store
            .retarget(&binding_id(), definition(b"SD2"), profile(b"OP1"))
            .unwrap();

        let mut committed = false;
        let error = store
            .with_locked_record(&binding_id(), |guard| {
                planned().authorize_and_commit(guard, |_| {
                    committed = true;
                    Ok(())
                })
            })
            .unwrap_err();

        assert!(
            !committed,
            "a stale route must be refused before the authorization is committed"
        );
        assert!(error.message.contains("stale"), "{}", error.message);
    }

    #[test]
    fn case_b_an_authorization_committed_first_binds_the_generation_it_validated() {
        // §2.20 Case B. Authorization commits while the lock is held, so a
        // later retarget governs only new plans — and the committed fact names
        // the generation it was validated against, so a reader can tell which
        // state of the binding authorized the effect rather than assuming the
        // current one did.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.bind(&binding(b"SD1", b"OP1")).unwrap();

        let authorized = store
            .with_locked_record(&binding_id(), |guard| {
                planned().authorize_and_commit(guard, |authorization| Ok(authorization.clone()))
            })
            .unwrap();
        assert_eq!(authorized.binding_generation, 0);
        assert_eq!(authorized.route, planned().route);

        // The later mutation does not reach back and invalidate what was
        // already authorized; it only makes the *next* plan stale.
        store
            .retarget(&binding_id(), definition(b"SD2"), profile(b"OP1"))
            .unwrap();
        assert_eq!(authorized.binding_generation, 0);
        assert!(store
            .with_locked_record(&binding_id(), |guard| planned().authorize(guard))
            .is_err());
    }

    #[test]
    fn an_operational_change_alone_also_makes_a_plan_stale() {
        // The profile is part of the route because it changes how the action
        // would be performed, even though it changes no observation.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.bind(&binding(b"SD1", b"OP1")).unwrap();
        store
            .retarget(&binding_id(), definition(b"SD1"), profile(b"OP2"))
            .unwrap();

        assert!(store
            .with_locked_record(&binding_id(), |guard| planned().authorize(guard))
            .is_err());
    }

    #[test]
    fn an_unbound_binding_refuses_an_otherwise_exact_plan() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.bind(&binding(b"SD1", b"OP1")).unwrap();
        store.unbind(&binding_id()).unwrap();

        assert!(store
            .with_locked_record(&binding_id(), |guard| planned().authorize(guard))
            .is_err());
    }

    #[test]
    fn re_planning_against_the_current_route_recovers() {
        // Refusal is not a dead end: the operator re-plans, re-authorizes and
        // proceeds. What is unavailable is carrying on as though nothing moved.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.bind(&binding(b"SD1", b"OP1")).unwrap();
        store
            .retarget(&binding_id(), definition(b"SD2"), profile(b"OP2"))
            .unwrap();

        let current = store.read_unlocked(&binding_id()).unwrap().unwrap();
        let replanned = PlannedOperation {
            route: current.current_route(),
            ..planned()
        };
        store
            .with_locked_record(&binding_id(), |guard| replanned.authorize(guard))
            .unwrap();
    }

    #[test]
    fn the_plan_carries_its_route_rather_than_deriving_one() {
        // Structural: the route is a field, so there is no code path that could
        // silently recompute it at execution time.
        let plan = planned();
        let encoded = serde_json::to_string(&plan).unwrap();
        assert!(encoded.contains("route"));
        assert_eq!(
            serde_json::from_str::<PlannedOperation>(&encoded).unwrap(),
            plan
        );
    }
}
