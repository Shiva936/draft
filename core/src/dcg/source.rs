//! The port every resource adapter implements, Draft's own included.
//!
//! Draft observes a project through adapters. There is deliberately no
//! privileged path: the filesystem adapter is Core code rather than an
//! extension, but it reaches Draft through exactly this trait, so nothing above
//! this line can be written in a way that only works for files. If a future
//! adapter cannot express something the filesystem adapter relies on, that is a
//! gap in *this* contract and gets fixed here — not worked around upstream.
//!
//! Three rules hold for every implementation:
//!
//! * **The locator body is the adapter's.** Core never parses, splits or
//!   compares it for ancestry. An adapter may mean a path, a row key, a
//!   timeline offset or an opaque handle by it.
//! * **Coverage domains are adapter-local and adapter-scoped.** An adapter
//!   partitions its own universe and names the parts; Core compares those names
//!   only within the binding that minted them.
//! * **Every access is fenced.** Reads, materialization, mutation and anchor
//!   capture all carry the [`ObservedRef`] of the generation they belong to, so
//!   content can never be attributed to state observed at a different moment.

use std::collections::BTreeMap;

use draft_extension_contract::{AdapterCapabilities, ResourceRule};

use crate::dcg::anchor::RecoveryAnchor;
use crate::dcg::observation::{ObservationCoverage, ObservationGap};
use crate::dcg::resource::{
    ContentAccess, ObservedRef, RawObservedResource, ResourceLocator, Untrackable,
};
use crate::support::error::DraftResult;

/// What is in scope for an enumeration.
///
/// These are the contributed view rules in force. They decide what belongs to
/// the observed universe at all, which is why they participate in the
/// observation context rather than being a display filter.
#[derive(Debug, Clone, Default)]
pub struct ViewRules {
    pub exclusions: Vec<ResourceRule>,
}

impl ViewRules {
    pub fn new(exclusions: Vec<ResourceRule>) -> Self {
        Self { exclusions }
    }

    pub fn is_empty(&self) -> bool {
        self.exclusions.is_empty()
    }
}

/// What one enumeration established.
///
/// `coverage` and `gaps` are not optional colour: they are how absence becomes
/// provable. An adapter that returns fewer resources without saying which part
/// of its universe it failed to establish would let a later comparison read the
/// shortfall as a deletion.
#[derive(Debug, Default, Clone)]
pub struct EnumerationOutcome {
    pub resources: Vec<RawObservedResource>,
    pub coverage: Vec<ObservationCoverage>,
    pub gaps: Vec<ObservationGap>,
    pub untrackable: Vec<Untrackable>,
    /// Resources a view rule removed from the universe. Reported as a count
    /// only: they are not part of project state, so nothing downstream may
    /// reason about them.
    pub excluded_count: usize,
}

/// Bytes an adapter placed into a runtime scope for a declared operation.
#[derive(Debug, Clone)]
pub struct MaterializedInput {
    /// The name the operation will find the input under, inside `input/`.
    pub name: String,
    pub length: u64,
}

/// What Draft asks an adapter to retain so a state can be put back.
#[derive(Debug, Clone)]
pub struct AnchorRequest {
    /// The run this capture belongs to, so the anchor can name the exact
    /// observation it was taken during.
    pub observation_run_id: crate::dcg::observation::ObservationRunId,
}

/// One step of a Draft-authored mutation.
///
/// Only Draft constructs these. An extension proposes effects; the operation
/// id, attribution and preconditions are Draft's, which is what keeps authority
/// out of a package's reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationStep {
    SetContent {
        locator: ResourceLocator,
        content: Vec<u8>,
    },
    CreateCollection {
        locator: ResourceLocator,
    },
    Relocate {
        from: ResourceLocator,
        to: ResourceLocator,
    },
    Remove {
        locator: ResourceLocator,
        recursive: bool,
    },
}

impl MutationStep {
    /// Every locator this step touches, for precondition and protection checks.
    pub fn locators(&self) -> Vec<&ResourceLocator> {
        match self {
            Self::SetContent { locator, .. }
            | Self::CreateCollection { locator }
            | Self::Remove { locator, .. } => vec![locator],
            Self::Relocate { from, to } => vec![from, to],
        }
    }
}

/// What must still be true when a mutation is applied.
///
/// Checked by the adapter immediately before acting, not by Draft a moment
/// earlier: the gap between the two is exactly where a concurrent writer lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationPrecondition {
    /// This resource is still in the generation Draft observed.
    StateEquals(ObservedRef),
    /// Nothing exists at this locator.
    MustNotExist(ResourceLocator),
    /// The container is still in the generation Draft observed.
    ParentStateEquals(ObservedRef),
    /// A relocation destination is free.
    DestinationAvailable(ResourceLocator),
}

/// A Draft-authored mutation.
#[derive(Debug, Clone)]
pub struct ResourceMutationPlan {
    pub operation_id: crate::support::common::OperationId,
    pub attribution: crate::support::common::EditAttribution,
    pub preconditions: Vec<MutationPrecondition>,
    pub steps: Vec<MutationStep>,
}

/// What a mutation or restore actually did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MutationOutcome {
    pub resources_changed: Vec<ResourceLocator>,
}

/// One adapter, as Draft sees it.
///
/// Object-safe on purpose: adapters are held behind `dyn` in a registry keyed
/// by scheme, and the filesystem adapter is one entry in that registry rather
/// than a branch above it.
pub trait ResourceSource: Send + Sync {
    /// The locator scheme this adapter owns.
    fn scheme(&self) -> &str;

    /// The binding this adapter observes under. Coverage domains are scoped to
    /// it, which is what stops two adapters that both call a domain `root` from
    /// ever comparing equal.
    fn binding_id(&self) -> crate::dcg::observation::AdapterBindingId;

    /// What this adapter can actually promise. Declared rather than assumed, so
    /// Draft can revalidate digests around access for weaker fencing.
    fn capabilities(&self) -> AdapterCapabilities;

    /// Establish the observable universe, or say which part of it could not be
    /// established.
    fn enumerate(&self, rules: &ViewRules) -> DraftResult<EnumerationOutcome>;

    /// Re-observe one resource.
    fn describe(&self, locator: &ResourceLocator) -> DraftResult<RawObservedResource>;

    /// How, and whether, Draft may reach this resource's content.
    fn content_access(&self, observed: &ObservedRef) -> DraftResult<ContentAccess>;

    /// Read a bounded range. Fenced: a moved generation is refused rather than
    /// served under the old state's identity.
    fn read_range(&self, observed: &ObservedRef, offset: u64, length: u64) -> DraftResult<Vec<u8>>;

    /// Place a resource's content into a runtime scope for a declared
    /// operation, within the scope's bounds.
    fn materialize(
        &self,
        observed: &ObservedRef,
        scope: &crate::support::runtime_scope::RuntimeScope,
    ) -> DraftResult<MaterializedInput>;

    /// Apply a Draft-authored mutation under its own preconditions.
    fn mutate(&self, plan: &ResourceMutationPlan) -> DraftResult<MutationOutcome>;

    /// Retain what is needed to put this exact observed state back.
    ///
    /// `None` is a legitimate answer and means the state is observable but not
    /// restorable — which Draft reports rather than hiding. A stale capture
    /// must yield `None` rather than an anchor describing a different moment.
    fn capture_anchor(
        &self,
        observed: &RawObservedResource,
        request: &AnchorRequest,
    ) -> DraftResult<Option<RecoveryAnchor>>;

    /// Materialize retained anchors back into domain state.
    fn restore(
        &self,
        plan: &crate::dcg::anchor::ResourceRestorePlan,
        anchors: &crate::dcg::anchor::RecoveryAnchorSet,
    ) -> DraftResult<MutationOutcome>;
}

/// The adapters in force for one project, keyed by the scheme each owns.
///
/// A scheme has exactly one adapter. Two contributions claiming the same scheme
/// is a conflict Draft reports rather than resolving by installation order.
pub struct ResourceSourceRegistry {
    sources: BTreeMap<String, Box<dyn ResourceSource>>,
}

impl std::fmt::Debug for ResourceSourceRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResourceSourceRegistry")
            .field("schemes", &self.schemes())
            .finish()
    }
}

impl ResourceSourceRegistry {
    pub fn new() -> Self {
        Self {
            sources: BTreeMap::new(),
        }
    }

    /// Register an adapter, refusing a second claim on one scheme.
    pub fn register(&mut self, source: Box<dyn ResourceSource>) -> DraftResult<()> {
        let scheme = source.scheme().to_string();
        if self.sources.contains_key(&scheme) {
            return Err(crate::support::error::DraftError::new(
                crate::support::error::DraftErrorKind::ConflictDetected,
                format!(
                    "two adapters claim the '{scheme}' scheme; Draft will not choose between them"
                ),
            )
            .with_suggestion("disable one of the extensions contributing this scheme"));
        }
        self.sources.insert(scheme, source);
        Ok(())
    }

    /// The adapter owning a scheme, if one is installed.
    pub fn for_scheme(&self, scheme: &str) -> Option<&dyn ResourceSource> {
        self.sources.get(scheme).map(AsRef::as_ref)
    }

    /// The adapter owning a locator, or an explicit refusal naming the scheme.
    ///
    /// "No adapter for this scheme" is a real answer with a fix, and is kept
    /// distinct from "this resource does not exist".
    pub fn resolve(&self, locator: &ResourceLocator) -> DraftResult<&dyn ResourceSource> {
        self.for_scheme(&locator.scheme).ok_or_else(|| {
            crate::support::error::DraftError::new(
                crate::support::error::DraftErrorKind::CapabilityUnavailable,
                format!("no installed adapter owns the '{}' scheme", locator.scheme),
            )
            .with_suggestion(
                "install and authorize an extension contributing a resource_adapter for it",
            )
        })
    }

    /// Every registered scheme, sorted.
    pub fn schemes(&self) -> Vec<&str> {
        self.sources.keys().map(String::as_str).collect()
    }

    /// Every adapter, in scheme order.
    pub fn all(&self) -> impl Iterator<Item = &dyn ResourceSource> {
        self.sources.values().map(AsRef::as_ref)
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

impl Default for ResourceSourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcg::observation::AdapterBindingId;

    #[derive(Debug)]
    struct StubSource(&'static str);

    impl ResourceSource for StubSource {
        fn scheme(&self) -> &str {
            self.0
        }
        fn binding_id(&self) -> AdapterBindingId {
            AdapterBindingId(format!("binding.{}", self.0))
        }
        fn capabilities(&self) -> AdapterCapabilities {
            AdapterCapabilities {
                observation_consistency:
                    draft_extension_contract::ObservationConsistency::DigestRevalidation,
                supports_ranged_read: false,
                supports_mutation: false,
                asserts_external_identity: false,
            }
        }
        fn enumerate(&self, _rules: &ViewRules) -> DraftResult<EnumerationOutcome> {
            Ok(EnumerationOutcome::default())
        }
        fn describe(&self, _locator: &ResourceLocator) -> DraftResult<RawObservedResource> {
            unimplemented!("not exercised by registry tests")
        }
        fn content_access(&self, _observed: &ObservedRef) -> DraftResult<ContentAccess> {
            Ok(ContentAccess::None)
        }
        fn read_range(
            &self,
            _observed: &ObservedRef,
            _offset: u64,
            _length: u64,
        ) -> DraftResult<Vec<u8>> {
            Ok(Vec::new())
        }
        fn materialize(
            &self,
            _observed: &ObservedRef,
            _scope: &crate::support::runtime_scope::RuntimeScope,
        ) -> DraftResult<MaterializedInput> {
            unimplemented!("not exercised by registry tests")
        }
        fn mutate(&self, _plan: &ResourceMutationPlan) -> DraftResult<MutationOutcome> {
            Ok(MutationOutcome::default())
        }
        fn capture_anchor(
            &self,
            _observed: &RawObservedResource,
            _request: &AnchorRequest,
        ) -> DraftResult<Option<RecoveryAnchor>> {
            Ok(None)
        }
        fn restore(
            &self,
            _plan: &crate::dcg::anchor::ResourceRestorePlan,
            _anchors: &crate::dcg::anchor::RecoveryAnchorSet,
        ) -> DraftResult<MutationOutcome> {
            Ok(MutationOutcome::default())
        }
    }

    #[test]
    fn a_scheme_has_exactly_one_adapter() {
        let mut registry = ResourceSourceRegistry::new();
        registry.register(Box::new(StubSource("catalog"))).unwrap();
        // Two claims on one scheme is a conflict Draft reports. Picking by
        // installation order would make what a project observes depend on the
        // sequence packages happened to be installed in.
        let error = registry
            .register(Box::new(StubSource("catalog")))
            .unwrap_err();
        assert_eq!(
            error.kind,
            crate::support::error::DraftErrorKind::ConflictDetected
        );
    }

    #[test]
    fn an_unowned_scheme_is_reported_as_a_missing_capability() {
        let registry = ResourceSourceRegistry::new();
        // Not "no such resource": nothing was even asked, because nothing could
        // ask. That distinction is what tells the reader installing something
        // would change the answer.
        let Err(error) = registry.resolve(&ResourceLocator::new("timeline", "clip/3")) else {
            panic!("an unowned scheme must not resolve");
        };
        assert_eq!(
            error.kind,
            crate::support::error::DraftErrorKind::CapabilityUnavailable
        );
        assert!(error.to_string().contains("timeline"));
    }

    #[test]
    fn registered_adapters_are_addressed_by_their_own_scheme() {
        let mut registry = ResourceSourceRegistry::new();
        registry.register(Box::new(StubSource("catalog"))).unwrap();
        registry.register(Box::new(StubSource("timeline"))).unwrap();
        assert_eq!(registry.schemes(), vec!["catalog", "timeline"]);
        let resolved = registry
            .resolve(&ResourceLocator::new("timeline", "anything at all"))
            .unwrap();
        assert_eq!(resolved.scheme(), "timeline");
        // The body was never looked at to get here.
        assert_eq!(
            resolved.binding_id(),
            AdapterBindingId("binding.timeline".into())
        );
    }
}
