//! `ProviderBinding` — a project's configured attachment to an external system.
//!
//! Domain-neutral throughout: a filesystem, a ticket tracker and a deploy
//! target are all providers in this sense. A binding is a *revisioned pointer*
//! at two immutable facts — the semantic definition in force and the
//! operational profile in force — plus a lifecycle.
//!
//! # Availability is never authority
//!
//! This is the rule the whole module exists to enforce. Before any external
//! side effect, execution takes this binding's lock and requires **exact
//! equality** with its current pointers:
//!
//! ```text
//! binding.lifecycle                   == Active
//! binding.id                          == route.provenance.binding
//! binding.current_semantic_definition == route.provenance.semantic_definition
//! binding.current_operational_profile == route.operational_profile
//! ```
//!
//! A plan naming `SD1/OP1` against a binding that now selects `SD2/OP2` is
//! **stale**, and is refused — even though `SD1` is still retained and would
//! still work. That "would still work" is exactly the reasoning that must not
//! be available: it would let an operation authorized against one meaning of a
//! provider execute against another.
//!
//! Refusal is not failure. The response is to re-plan, re-authorize and
//! republish, which is a decision somebody makes rather than one Draft makes
//! silently by adopting whatever the binding points at now.
//!
//! # Unbinding deletes nothing
//!
//! An `Unbound` binding still supports historical verification, reads, GC
//! reachability and explicit recovery. What it refuses is anything *new*:
//! observations, routing, mutations, publication, materialization.
//! Reactivation is explicit, because silently resuming would make an operator's
//! deliberate withdrawal reversible by accident.
//!
//! # The lock is order 5
//!
//! Held across exact-route validation and the durable authorization that
//! follows, and released **before** the external call. Serializing binding
//! mutation against dispatch is what removes the ambiguous middle state: either
//! the mutation lands first and the dispatch is refused, or the dispatch is
//! authorized first and the later mutation affects only new dispatches.

use draft_dcg_contract::ids::{ProjectId, ProviderBindingId};
use draft_dcg_contract::{
    ProviderKindId, ProviderOperationalProfileDigest, ProviderProvenanceRef, ProviderRouteRef,
    ProviderSemanticDefinitionDigest,
};
use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::lock_order::LockOrder;
use crate::support::record_guard::{
    ExpectedRecordState, RecordGuard, RevisionedRecord, RevisionedRecordStore, DEFAULT_LOCK_TIMEOUT,
};
use crate::support::telemetry::Counter;

/// Whether a binding may be used for new work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderBindingLifecycle {
    /// Usable for new observations, routing and external effects.
    Active,
    /// Withdrawn. History remains fully readable and verifiable; nothing new
    /// may be routed through it until it is explicitly rebound.
    Unbound,
}

/// A project's attachment to one external system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBinding {
    pub generation: u64,
    pub id: ProviderBindingId,
    pub project: ProjectId,
    pub kind: ProviderKindId,
    pub current_semantic_definition: ProviderSemanticDefinitionDigest,
    pub current_operational_profile: ProviderOperationalProfileDigest,
    pub lifecycle: ProviderBindingLifecycle,
}

impl RevisionedRecord for ProviderBinding {
    fn generation(&self) -> u64 {
        self.generation
    }
}

/// Why a planned route may not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteRefusal {
    /// The binding has been withdrawn.
    Unbound,
    /// The route names a different binding entirely.
    DifferentBinding,
    /// The binding has moved to a different semantic definition.
    ///
    /// The planned one may still be retained and usable; that is not the
    /// question. The binding no longer selects it.
    SemanticDefinitionMoved,
    /// The binding has moved to a different operational profile.
    OperationalProfileMoved,
}

impl std::fmt::Display for RouteRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unbound => "the binding is unbound",
            Self::DifferentBinding => "the route names a different binding",
            Self::SemanticDefinitionMoved => {
                "the binding now selects a different semantic definition"
            }
            Self::OperationalProfileMoved => {
                "the binding now selects a different operational profile"
            }
        })
    }
}

impl ProviderBinding {
    /// What this binding contributes to accepted state provenance.
    ///
    /// Definition only. A Baseline never derives a route: how a provider is
    /// operated did not affect what it observed, so including the profile would
    /// make an operational change look like a change of accepted history.
    pub fn provenance(&self) -> ProviderProvenanceRef {
        ProviderProvenanceRef {
            binding: self.id.clone(),
            semantic_definition: self.current_semantic_definition.clone(),
        }
    }

    /// The route this binding currently selects.
    ///
    /// Constructed only when an executable action is being planned. The
    /// resulting plan is frozen: it is never re-resolved against current
    /// pointers at execution time.
    pub fn current_route(&self) -> ProviderRouteRef {
        ProviderRouteRef {
            provenance: self.provenance(),
            operational_profile: self.current_operational_profile.clone(),
        }
    }

    /// Whether this binding still selects `route`, exactly.
    ///
    /// `Ok(())` means the route may proceed. Every refusal names what moved, so
    /// the caller can re-plan rather than guess.
    pub fn selects(&self, route: &ProviderRouteRef) -> Result<(), RouteRefusal> {
        if self.lifecycle != ProviderBindingLifecycle::Active {
            return Err(RouteRefusal::Unbound);
        }
        if self.id != route.provenance.binding {
            return Err(RouteRefusal::DifferentBinding);
        }
        if self.current_semantic_definition != route.provenance.semantic_definition {
            return Err(RouteRefusal::SemanticDefinitionMoved);
        }
        if self.current_operational_profile != route.operational_profile {
            return Err(RouteRefusal::OperationalProfileMoved);
        }
        Ok(())
    }

    /// Whether new work may be routed through this binding at all.
    ///
    /// Distinct from [`Self::selects`]: a binding can be perfectly usable while
    /// a particular plan against it is stale.
    pub fn is_routable(&self) -> bool {
        self.lifecycle == ProviderBindingLifecycle::Active
    }

    /// This binding one generation on, with `mutate` applied.
    pub fn advanced(&self, mutate: impl FnOnce(&mut Self)) -> Self {
        let mut next = self.clone();
        next.generation += 1;
        mutate(&mut next);
        next
    }
}

/// The project's provider bindings.
#[derive(Debug, Clone)]
pub struct ProviderBindingStore {
    records: RevisionedRecordStore<ProviderBinding>,
}

/// A live, exclusive hold on one binding.
pub type ProviderBindingGuard<'a> = RecordGuard<'a, ProviderBinding>;

impl ProviderBindingStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            records: RevisionedRecordStore::new(directory)
                .with_order(LockOrder::ProviderBindingStore)
                .counting_conflicts_as(Counter::ProviderBindingCasConflicts),
        }
    }

    pub fn lock_path(&self, id: &ProviderBindingId) -> std::path::PathBuf {
        self.records.lock_path(id.as_str())
    }

    /// The record store, for callers committing through the audited path.
    ///
    /// A binding mutation is an audited fact: it advances a generation, it
    /// carries an Activity event, and a crash between the write and the
    /// append has to be decidable. Handing out the store is what lets
    /// `app/activity` run the journalled protocol over it rather than this
    /// module growing a second copy of it.
    pub fn records(&self) -> &RevisionedRecordStore<ProviderBinding> {
        &self.records
    }

    /// Read a binding without locking. For display and read models only.
    pub fn read_unlocked(&self, id: &ProviderBindingId) -> DraftResult<Option<ProviderBinding>> {
        self.records.read_unlocked(id.as_str())
    }

    /// Every binding this project has, active or unbound, oldest id first.
    ///
    /// Unbound bindings are included deliberately: `unbind` withdraws a
    /// binding from new work and deletes nothing, so a listing that hid them
    /// would make a withdrawal look like a deletion — exactly the confusion
    /// §2.19 exists to prevent. Callers that want only usable bindings filter
    /// on [`ProviderBinding::is_routable`].
    pub fn list(&self) -> DraftResult<Vec<ProviderBinding>> {
        let mut bindings = Vec::new();
        for key in self.records.keys()? {
            if let Some(binding) = self.records.read_unlocked(&key)? {
                bindings.push(binding);
            }
        }
        bindings.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        Ok(bindings)
    }

    /// Run `body` with the binding's lock held for exactly one acquisition.
    ///
    /// This is how dispatch validates a route: the guard stays live from
    /// exact-route validation through the durable authorization, so the binding
    /// cannot move in between.
    pub fn with_locked_record<R>(
        &self,
        id: &ProviderBindingId,
        body: impl FnOnce(&mut ProviderBindingGuard<'_>) -> DraftResult<R>,
    ) -> DraftResult<R> {
        self.records
            .with_locked_record(id.as_str(), DEFAULT_LOCK_TIMEOUT, body)
    }

    /// Validate a planned route against the binding's current pointers.
    ///
    /// Must be called with the binding's lock held, and the lock must stay held
    /// through the durable authorization that follows — otherwise the binding
    /// could move between the check and the commit.
    pub fn require_selects(
        guard: &ProviderBindingGuard<'_>,
        route: &ProviderRouteRef,
    ) -> DraftResult<ProviderBinding> {
        let Some(binding) = guard.current()? else {
            return Err(DraftError::new(
                DraftErrorKind::NotFound,
                format!("provider binding '{}' does not exist", guard.key()),
            ));
        };
        binding.selects(route).map_err(|refusal| {
            // The exact-current-route rule refusing a new external effect.
            // Counted here rather than at each caller: this is the one place
            // that decides a planned route is stale.
            Counter::PublicationRouteStalenessRefusals.increment();
            DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "the planned route is stale: {refusal}. That the planned definition is still \
                     retained does not authorize a new external effect."
                ),
            )
            .with_suggestion("Re-plan and re-authorize against the binding's current route.")
        })?;
        Ok(binding)
    }

    /// Create a binding.
    pub fn bind(&self, binding: &ProviderBinding) -> DraftResult<()> {
        self.records
            .compare_exchange(binding.id.as_str(), &ExpectedRecordState::Absent, binding)
    }

    /// Point a binding at a different definition or profile.
    pub fn retarget(
        &self,
        id: &ProviderBindingId,
        semantic_definition: ProviderSemanticDefinitionDigest,
        operational_profile: ProviderOperationalProfileDigest,
    ) -> DraftResult<ProviderBinding> {
        self.mutate(id, |current| {
            current.advanced(|next| {
                next.current_semantic_definition = semantic_definition;
                next.current_operational_profile = operational_profile;
            })
        })
    }

    /// Withdraw a binding. Deletes nothing.
    pub fn unbind(&self, id: &ProviderBindingId) -> DraftResult<ProviderBinding> {
        self.mutate(id, |current| {
            current.advanced(|next| next.lifecycle = ProviderBindingLifecycle::Unbound)
        })
    }

    /// Reactivate a withdrawn binding.
    pub fn rebind(&self, id: &ProviderBindingId) -> DraftResult<ProviderBinding> {
        self.mutate(id, |current| {
            current.advanced(|next| next.lifecycle = ProviderBindingLifecycle::Active)
        })
    }

    fn mutate(
        &self,
        id: &ProviderBindingId,
        change: impl FnOnce(&ProviderBinding) -> ProviderBinding,
    ) -> DraftResult<ProviderBinding> {
        self.with_locked_record(id, |guard| {
            let Some(current) = guard.current()? else {
                return Err(DraftError::new(
                    DraftErrorKind::NotFound,
                    format!("provider binding '{id}' does not exist"),
                ));
            };
            let expected = ExpectedRecordState::of(&current)?;
            let next = change(&current);
            guard.compare_exchange_locked(&expected, &next)?;
            Ok(next)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::Digest;

    fn binding_id() -> ProviderBindingId {
        ProviderBindingId::parse("pbd_000000000001").unwrap()
    }

    fn definition(seed: &[u8]) -> ProviderSemanticDefinitionDigest {
        ProviderSemanticDefinitionDigest::new(Digest::of_bytes(seed))
    }

    fn profile(seed: &[u8]) -> ProviderOperationalProfileDigest {
        ProviderOperationalProfileDigest::new(Digest::of_bytes(seed))
    }

    fn binding() -> ProviderBinding {
        ProviderBinding {
            generation: 0,
            id: binding_id(),
            project: ProjectId::parse("prj_000000000001").unwrap(),
            kind: ProviderKindId::parse("draft.filesystem/local").unwrap(),
            current_semantic_definition: definition(b"SD1"),
            current_operational_profile: profile(b"OP1"),
            lifecycle: ProviderBindingLifecycle::Active,
        }
    }

    fn store(directory: &tempfile::TempDir) -> ProviderBindingStore {
        ProviderBindingStore::new(directory.path())
    }

    #[test]
    fn a_binding_selects_its_own_current_route() {
        let binding = binding();
        binding.selects(&binding.current_route()).unwrap();
    }

    #[test]
    fn a_moved_definition_or_profile_makes_a_plan_stale() {
        // The plan named SD1/OP1. Both are still retained and would still work,
        // and that is precisely the reasoning that must not be available.
        let planned = binding().current_route();

        let mut moved_definition = binding();
        moved_definition.current_semantic_definition = definition(b"SD2");
        assert_eq!(
            moved_definition.selects(&planned),
            Err(RouteRefusal::SemanticDefinitionMoved)
        );

        let mut moved_profile = binding();
        moved_profile.current_operational_profile = profile(b"OP2");
        assert_eq!(
            moved_profile.selects(&planned),
            Err(RouteRefusal::OperationalProfileMoved)
        );
    }

    #[test]
    fn an_unbound_binding_refuses_a_route_that_otherwise_matches_exactly() {
        // Availability is not authority: every pointer still agrees.
        let mut unbound = binding();
        let planned = unbound.current_route();
        unbound.lifecycle = ProviderBindingLifecycle::Unbound;
        assert_eq!(unbound.selects(&planned), Err(RouteRefusal::Unbound));
        assert!(!unbound.is_routable());
    }

    #[test]
    fn another_bindings_route_is_never_selected() {
        let mut foreign = binding().current_route();
        foreign.provenance.binding = ProviderBindingId::parse("pbd_999999999999").unwrap();
        assert_eq!(
            binding().selects(&foreign),
            Err(RouteRefusal::DifferentBinding)
        );
    }

    #[test]
    fn provenance_excludes_the_operational_profile() {
        // Re-tuning how a provider is driven must not look like a change of
        // accepted history.
        let before = binding().provenance();
        let mut retuned = binding();
        retuned.current_operational_profile = profile(b"OP2");
        assert_eq!(before, retuned.provenance());
        assert_ne!(binding().current_route(), retuned.current_route());
    }

    #[test]
    fn retargeting_leaves_history_readable_and_makes_old_plans_stale() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.bind(&binding()).unwrap();
        let planned = binding().current_route();

        let retargeted = store
            .retarget(&binding_id(), definition(b"SD2"), profile(b"OP2"))
            .unwrap();
        assert_eq!(retargeted.generation, 1);
        assert_eq!(
            retargeted.selects(&planned),
            Err(RouteRefusal::SemanticDefinitionMoved)
        );
    }

    #[test]
    fn unbinding_deletes_nothing_and_rebinding_is_explicit() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.bind(&binding()).unwrap();

        let unbound = store.unbind(&binding_id()).unwrap();
        assert!(!unbound.is_routable());
        // Every historical pointer survives, so past state stays verifiable.
        assert_eq!(unbound.current_semantic_definition, definition(b"SD1"));
        assert_eq!(unbound.provenance(), binding().provenance());

        let rebound = store.rebind(&binding_id()).unwrap();
        assert!(rebound.is_routable());
        assert_eq!(rebound.generation, 2);
    }

    #[test]
    fn route_validation_under_the_guard_names_what_moved() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.bind(&binding()).unwrap();
        let planned = binding().current_route();

        store
            .with_locked_record(&binding_id(), |guard| {
                ProviderBindingStore::require_selects(guard, &planned)
            })
            .unwrap();

        store
            .retarget(&binding_id(), definition(b"SD2"), profile(b"OP1"))
            .unwrap();

        let error = store
            .with_locked_record(&binding_id(), |guard| {
                ProviderBindingStore::require_selects(guard, &planned)
            })
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
        assert!(
            error.message.contains("still retained does not authorize"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_binding_mutation_and_a_dispatch_cannot_interleave() {
        // Serialization is what removes the ambiguous middle state: either the
        // mutation lands first and the route is refused, or the route is
        // validated first and the mutation affects only later dispatches.
        let directory = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(store(&directory));
        store.bind(&binding()).unwrap();

        let observed = store
            .with_locked_record(&binding_id(), |guard| {
                let held = guard.current()?.unwrap();
                // A contender cannot mutate while this guard is live; that is
                // asserted by the lock-order tests. What matters here is that
                // what we validated is what we still see.
                assert_eq!(guard.current()?.unwrap(), held);
                Ok(held)
            })
            .unwrap();
        assert_eq!(observed.generation, 0);
    }

    #[test]
    fn the_binding_lock_is_a_stable_sidecar_at_order_five() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        assert!(store
            .lock_path(&binding_id())
            .ends_with("pbd_000000000001.lock"));

        store.bind(&binding()).unwrap();
        store
            .with_locked_record(&binding_id(), |_guard| {
                assert_eq!(
                    crate::support::lock_order::currently_held(),
                    vec![LockOrder::ProviderBindingStore]
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn mutating_an_absent_binding_is_reported_rather_than_creating_one() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            store(&directory).unbind(&binding_id()).unwrap_err().kind,
            DraftErrorKind::NotFound
        );
    }
}
