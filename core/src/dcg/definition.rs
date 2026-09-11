//! `ChangeDefinition` and `ScopeResolution` — what a Change intends, and what
//! that intent actually reached.
//!
//! Both are immutable facts under the create-once discipline, so "the exact
//! definition" and "the exact scope" are mechanical guarantees rather than
//! phrases: the bytes beneath an id cannot be replaced, and every load verifies
//! it.
//!
//! # Why scope is resolved once
//!
//! A declaration says what a Change is *allowed* to touch. Resolution turns
//! that into the exact set it *does* touch, against an exact base Baseline.
//!
//! Re-resolving later would silently widen it. A declaration naming a
//! collection resolves to whatever that collection holds — and if the set were
//! recomputed at seal time, a resource added in between would be swept in
//! without anyone approving it. Resolving once, against a named Baseline, is
//! what makes the reviewed scope and the sealed scope the same thing.
//!
//! That is also why the resolution records its base: a scope is only meaningful
//! relative to the state it was resolved against, and a resolution whose base
//! has moved has to be redone rather than reinterpreted.

use std::collections::BTreeSet;

use draft_dcg_contract::ids::{ActorId, ChangeId, ResourceId};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::{BaselineId, Digest};
use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use crate::support::immutable_store::ImmutableFactStore;

/// What a Change intends, and what it is permitted to touch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeDefinition {
    pub change: ChangeId,
    /// What the Change is for, in the author's words. Opaque to Core.
    pub intent: String,
    /// What it may touch. Resolved into an exact set by [`ScopeResolution`].
    pub scope_declaration: BTreeSet<ResourceId>,
    pub created_by: ActorId,
    pub created_at: Timestamp,
}

impl ChangeDefinition {
    /// This definition's canonical digest.
    pub fn digest(&self) -> DraftResult<Digest> {
        self.validate()?;
        Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
    }

    pub fn validate(&self) -> DraftResult<()> {
        if self.intent.trim().is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "a change definition must say what the change is for",
            ));
        }
        if self.scope_declaration.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                "a change definition must declare what it may touch; an unbounded scope is not a \
                 scope",
            ));
        }
        Ok(())
    }
}

/// The exact set a Change reached, against an exact base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeResolution {
    pub change: ChangeId,
    /// The exact definition this resolves.
    pub definition: Digest,
    /// The Baseline it was resolved against.
    ///
    /// A scope is only meaningful relative to the state it was resolved
    /// against; a resolution whose base has moved is redone, not reinterpreted.
    pub base_baseline: BaselineId,
    /// The exact resources in scope.
    pub resources: BTreeSet<ResourceId>,
    pub resolved_at: Timestamp,
}

impl ScopeResolution {
    pub fn digest(&self) -> DraftResult<Digest> {
        Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
    }

    /// Resolve a declaration once, against `base`.
    ///
    /// The resolved set may narrow the declaration — a declared resource that
    /// does not exist in the base is simply not in scope — but it may never
    /// exceed it. Widening here would let resolution grant reach that the
    /// definition never asked for.
    /// `resolvable` is every Resource the Change could legitimately land on:
    /// what the Baseline accepts, and what the project holds now. Both, because
    /// a Change that introduces a Resource is as ordinary as one that edits an
    /// accepted one — narrowing to the Baseline alone would make "add a file"
    /// unrepresentable, and silently drop it from the scope a reviewer reads.
    /// What is excluded is a declaration that names nothing at all.
    pub fn resolve(
        definition: &ChangeDefinition,
        base: BaselineId,
        resolvable: &BTreeSet<ResourceId>,
        resolved_at: Timestamp,
    ) -> DraftResult<Self> {
        let resources: BTreeSet<ResourceId> = definition
            .scope_declaration
            .intersection(resolvable)
            .cloned()
            .collect();
        Ok(Self {
            change: definition.change.clone(),
            definition: definition.digest()?,
            base_baseline: base,
            resources,
            resolved_at,
        })
    }

    /// Check this resolution against the definition it claims to resolve.
    pub fn validate_against(&self, definition: &ChangeDefinition) -> DraftResult<()> {
        let expected = definition.digest()?;
        if self.definition != expected {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "scope resolution names definition {} but the loaded definition computes to \
                     {expected}",
                    self.definition
                ),
            ));
        }
        if self.change != definition.change {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "scope resolution and definition name different changes".to_string(),
            ));
        }
        // Narrowing is expected; exceeding the declaration is not.
        if let Some(outside) = self
            .resources
            .difference(&definition.scope_declaration)
            .next()
        {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "resource '{outside}' is in the resolved scope but was never declared; \
                     resolution may narrow a declaration, never widen it"
                ),
            ));
        }
        Ok(())
    }

    /// Whether `resource` is in scope.
    pub fn covers(&self, resource: &ResourceId) -> bool {
        self.resources.contains(resource)
    }
}

/// Create-once storage for [`ChangeDefinition`] and [`ScopeResolution`].
///
/// Both are immutable facts under §2.45: the bytes beneath a logical id cannot
/// be replaced, and every load re-derives the canonical digest and compares it
/// against the stored binding. A mismatch is corruption, never a warning and
/// never something to repair in place.
///
/// They are separate stores because they answer different questions and are
/// written at different moments — the declaration when the work is defined,
/// the resolution when it is bounded against a named Baseline.
pub struct DefinitionStore {
    definitions: ImmutableFactStore<ChangeDefinition>,
    resolutions: ImmutableFactStore<ScopeResolution>,
}

impl DefinitionStore {
    pub fn new(
        definitions: impl Into<std::path::PathBuf>,
        resolutions: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            definitions: ImmutableFactStore::new(definitions),
            resolutions: ImmutableFactStore::new(resolutions),
        }
    }

    /// Record a definition under its own digest as the logical id.
    ///
    /// Storing it by digest makes the create-once binding trivially true for
    /// this family: a different definition is a different id. The binding is
    /// still written and still verified on load, so the family satisfies the
    /// same rule as every other immutable fact rather than a weaker one that
    /// happens to hold.
    pub fn put_definition(&self, definition: &ChangeDefinition) -> DraftResult<Digest> {
        let digest = definition.digest()?;
        self.definitions.put(digest.as_str(), definition)?;
        Ok(digest)
    }

    pub fn definition(&self, digest: &Digest) -> DraftResult<Option<ChangeDefinition>> {
        self.definitions.get(digest.as_str())
    }

    pub fn put_resolution(&self, resolution: &ScopeResolution) -> DraftResult<Digest> {
        let digest = resolution.digest()?;
        self.resolutions.put(digest.as_str(), resolution)?;
        Ok(digest)
    }

    pub fn resolution(&self, digest: &Digest) -> DraftResult<Option<ScopeResolution>> {
        self.resolutions.get(digest.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(name: &str) -> ResourceId {
        ResourceId::parse(format!("res_{name}")).unwrap()
    }

    fn definition(scope: &[&str]) -> ChangeDefinition {
        ChangeDefinition {
            change: ChangeId::parse("chg_000000000001").unwrap(),
            intent: "update the catalogue".into(),
            scope_declaration: scope.iter().map(|name| resource(name)).collect(),
            created_by: ActorId::parse("act_000000000001").unwrap(),
            created_at: Timestamp::from_unix_nanos(1_000),
        }
    }

    fn baseline(seed: &[u8]) -> BaselineId {
        BaselineId::new(Digest::of_bytes(seed))
    }

    fn present(names: &[&str]) -> BTreeSet<ResourceId> {
        names.iter().map(|name| resource(name)).collect()
    }

    #[test]
    fn a_definition_must_say_what_it_is_for_and_what_it_may_touch() {
        definition(&["a"]).validate().unwrap();

        let mut aimless = definition(&["a"]);
        aimless.intent = "  ".into();
        assert!(aimless.validate().is_err());

        let mut unbounded = definition(&["a"]);
        unbounded.scope_declaration.clear();
        assert!(unbounded.validate().is_err());
    }

    #[test]
    fn resolution_narrows_a_declaration_to_what_the_base_actually_holds() {
        let resolution = ScopeResolution::resolve(
            &definition(&["a", "b", "missing"]),
            baseline(b"base"),
            &present(&["a", "b", "unrelated"]),
            Timestamp::from_unix_nanos(2_000),
        )
        .unwrap();

        assert_eq!(resolution.resources, present(&["a", "b"]));
        assert!(resolution.covers(&resource("a")));
        assert!(!resolution.covers(&resource("unrelated")), "never widened");
        assert!(!resolution.covers(&resource("missing")));
    }

    #[test]
    fn a_resolution_may_never_exceed_its_declaration() {
        // The check that keeps the reviewed scope and the effective scope the
        // same thing.
        let mut widened = ScopeResolution::resolve(
            &definition(&["a"]),
            baseline(b"base"),
            &present(&["a"]),
            Timestamp::from_unix_nanos(2_000),
        )
        .unwrap();
        widened.resources.insert(resource("smuggled"));

        let error = widened.validate_against(&definition(&["a"])).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);
    }

    #[test]
    fn resolving_once_is_what_stops_a_later_addition_being_swept_in() {
        // The scenario: a resource appears after the scope was reviewed. A
        // re-resolution would include it; the recorded resolution does not.
        let resolved = ScopeResolution::resolve(
            &definition(&["a", "b"]),
            baseline(b"base"),
            &present(&["a"]),
            Timestamp::from_unix_nanos(2_000),
        )
        .unwrap();
        assert_eq!(resolved.resources, present(&["a"]));

        let re_resolved = ScopeResolution::resolve(
            &definition(&["a", "b"]),
            baseline(b"base"),
            &present(&["a", "b"]),
            Timestamp::from_unix_nanos(3_000),
        )
        .unwrap();
        assert_ne!(
            resolved.resources, re_resolved.resources,
            "re-resolution would have widened the reviewed scope"
        );
        assert_ne!(resolved.digest().unwrap(), re_resolved.digest().unwrap());
    }

    #[test]
    fn an_amended_definition_invalidates_its_resolution() {
        // Scenario J. Amending what a Change may touch must not leave a
        // resolution silently claiming to resolve the new definition.
        let original = definition(&["a"]);
        let resolution = ScopeResolution::resolve(
            &original,
            baseline(b"base"),
            &present(&["a"]),
            Timestamp::from_unix_nanos(2_000),
        )
        .unwrap();
        resolution.validate_against(&original).unwrap();

        let amended = definition(&["a", "b"]);
        let error = resolution.validate_against(&amended).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn a_resolution_records_the_base_it_was_taken_against() {
        // A scope is only meaningful relative to a state, so two resolutions
        // over different bases are different facts even with the same set.
        let over_one = ScopeResolution::resolve(
            &definition(&["a"]),
            baseline(b"one"),
            &present(&["a"]),
            Timestamp::from_unix_nanos(2_000),
        )
        .unwrap();
        let over_two = ScopeResolution::resolve(
            &definition(&["a"]),
            baseline(b"two"),
            &present(&["a"]),
            Timestamp::from_unix_nanos(2_000),
        )
        .unwrap();
        assert_eq!(over_one.resources, over_two.resources);
        assert_ne!(over_one.digest().unwrap(), over_two.digest().unwrap());
    }

    #[test]
    fn a_resolution_for_another_change_is_refused() {
        let mut foreign = ScopeResolution::resolve(
            &definition(&["a"]),
            baseline(b"base"),
            &present(&["a"]),
            Timestamp::from_unix_nanos(2_000),
        )
        .unwrap();
        foreign.change = ChangeId::parse("chg_999999999999").unwrap();
        assert!(foreign.validate_against(&definition(&["a"])).is_err());
    }

    #[test]
    fn selector_stability_a_declaration_resolves_the_same_way_twice() {
        // Scenario N. Same declaration, same base, same answer — otherwise a
        // scope could drift between reading it and acting on it.
        let first = ScopeResolution::resolve(
            &definition(&["a", "b"]),
            baseline(b"base"),
            &present(&["a", "b"]),
            Timestamp::from_unix_nanos(2_000),
        )
        .unwrap();
        let second = ScopeResolution::resolve(
            &definition(&["a", "b"]),
            baseline(b"base"),
            &present(&["a", "b"]),
            Timestamp::from_unix_nanos(2_000),
        )
        .unwrap();
        assert_eq!(first.digest().unwrap(), second.digest().unwrap());
    }
}
