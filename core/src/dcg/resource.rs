//! What Draft tracks, and how it knows a tracked thing is still the same thing.
//!
//! A resource is an identity, an opaque locator and a deterministic state
//! digest. It is deliberately *not* a file with bytes: a track, a clip, a scene,
//! a design node, a CAD assembly, a dataset partition and a remote asset are all
//! representable here without a Core change, and the filesystem is one adapter
//! among them rather than the definition.
//!
//! Three things are kept rigorously apart, because conflating any pair of them
//! is how a change-control system starts lying:
//!
//! * **Authoritative state** — [`RawResourceState`]. What was observed. This is
//!   what snapshot and change identity are computed over.
//! * **Live fencing** — [`ObservationToken`] and [`ObservedRef`]. Which
//!   *generation* an observation belonged to, so content read later cannot be
//!   attributed to state observed earlier. Transient; never persisted as truth,
//!   never an input to any digest.
//! * **Provenance** — [`ResourceIdentityProof`]. *Why* Draft believes two
//!   observations describe the same resource. Auditable, but it does not
//!   redefine what the resource's state is.

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing;
use draft_extension_contract::{AttributeValue, ResourceForm};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// A Resource's identity is the SDK's `ResourceId`. Core does not define a
// second one: one canonical value has exactly one Rust type, and a Core-local
// copy would drift from the contract every fact is written against.
pub use draft_dcg_contract::ids::ResourceId;

/// Derive a Resource's identity from a locator that bears state.
///
/// Deterministic, so the same locator always names the same Resource and two
/// observations of one path agree about what they observed. Derived rather
/// than the locator itself because a `ResourceId` is `res_`-prefixed and
/// opaque: identity is a Draft concept, and embedding a path in it would make
/// every id a claim about where the thing lives.
///
/// Locator-stable, not content-stable: an adapter that cannot prove continuity
/// across a move it did not perform reports a relocated resource as a
/// different one, until something asserts otherwise.
pub fn resource_id_for_locator(locator: &str) -> ResourceId {
    let digest = draft_dcg_contract::Digest::of_bytes(locator.as_bytes());
    let hex: String = digest
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect();
    ResourceId::parse(format!("res_{hex}")).expect("a derived resource id is well-formed")
}

/// One way in which a resource's observable state differs.
///
/// A single transition may carry several: a resource can be relocated *and*
/// content-changed *and* have its attributes changed, and collapsing that into
/// one mutually exclusive verdict would lose information a reviewer needs.
///
/// There is deliberately no `KindChanged`: a class is derived interpretation, so
/// a reclassification is Draft learning something new about unchanged state, not
/// a project change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeAspect {
    Added,
    Removed,
    ContentChanged,
    MetadataChanged,
    Relocated,
    FormChanged,
    AttributesChanged,
}

impl ChangeAspect {
    /// The contract's own name for this aspect.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::ContentChanged => "content_changed",
            Self::MetadataChanged => "metadata_changed",
            Self::Relocated => "relocated",
            Self::FormChanged => "form_changed",
            Self::AttributesChanged => "attributes_changed",
        }
    }
}

/// Where a resource lives, in terms only its owning adapter understands.
///
/// `scheme` selects the adapter. `body` is **opaque to Core**: it is never
/// parsed, split, resolved or compared for ancestry. A pattern may be matched
/// against it as a plain string, and that is all.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLocator {
    pub scheme: String,
    pub body: String,
}

impl ResourceLocator {
    pub fn new(scheme: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            scheme: scheme.into(),
            body: body.into(),
        }
    }

    /// A locator addressed by Draft's built-in filesystem adapter.
    pub fn file(body: impl Into<String>) -> Self {
        Self::new(crate::support::predicate::FILE_SCHEME, body)
    }

    pub fn is_file(&self) -> bool {
        self.scheme == crate::support::predicate::FILE_SCHEME
    }
}

impl std::fmt::Display for ResourceLocator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}://{}", self.scheme, self.body)
    }
}

/// PERSISTED AUTHORITATIVE TRUTH: domain-independent observable state only.
///
/// Note what is absent, and why:
///
/// * no contributed class — classification is derived interpretation, and a
///   reclassification must never look like a project change;
/// * no identity basis — that is provenance, and two observations differing only
///   in *why* Draft believes they match must have identical state identity;
/// * no observation token — that is live fencing, meaningless once persisted;
/// * no coverage membership — that is observation metadata about the snapshot,
///   not about the resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawResourceState {
    pub resource_id: ResourceId,
    pub locator: ResourceLocator,
    /// Intrinsic shape the adapter observed. Never an extension classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<ResourceForm>,
    /// Intrinsic media type — an OS attribute, an HTTP header, an object-store
    /// field. Never an extension classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Intrinsic adapter-observed attributes only.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, AttributeValue>,
    /// MANDATORY. The deterministic canonical identity of this resource's
    /// Draft-observable state. Not necessarily a hash of bytes: a logical
    /// resource with no byte stream still has one.
    pub state_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_size: Option<u64>,
}

impl RawResourceState {
    /// A borrowed view for predicate evaluation.
    ///
    /// Classification is passed separately by the caller, because it is derived
    /// and must not travel as though it were observed.
    pub fn view(&self) -> crate::support::predicate::ResourceView<'_> {
        crate::support::predicate::ResourceView {
            locator_scheme: &self.locator.scheme,
            locator_body: &self.locator.body,
            media_type: self.media_type.as_deref(),
            form: self.form,
            attributes: &self.attributes,
            content_size: self.content_size,
        }
    }
}

/// LIVE OPERATIONAL VALUE: which generation an observation belonged to.
///
/// Opaque to Core, compared only by the adapter that minted it. It may be an
/// inode generation, an ETag, an object version, an MVCC revision or an asset
/// revision — values that are machine-, mount- or session-specific and unstable
/// across restores, which is exactly why they must never become historical
/// identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObservationToken(pub String);

impl ObservationToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One resource as it was just observed: authoritative state plus the live fence
/// that state was read under.
///
/// Snapshots persist the `state` half only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawObservedResource {
    pub state: RawResourceState,
    pub observation_token: ObservationToken,
    /// Which of the adapter's coverage domains this resource belongs to.
    pub coverage_domain: super::observation::CoverageDomainRef,
}

impl RawObservedResource {
    /// A transient reference for reading, materializing or mutating this exact
    /// generation.
    pub fn observed_ref(&self) -> ObservedRef {
        ObservedRef {
            resource_id: self.state.resource_id.clone(),
            locator: self.state.locator.clone(),
            expected_state_digest: self.state.state_digest.clone(),
            observation_token: self.observation_token.clone(),
        }
    }
}

/// A transient, operation-scoped handle to one observed generation.
///
/// Every read, materialization, mutation precondition and anchor capture carries
/// one. If either the digest or the token has moved, the adapter refuses with
/// [`stale_observation`] rather than serving content from a different generation
/// under the old state's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedRef {
    pub resource_id: ResourceId,
    pub locator: ResourceLocator,
    pub expected_state_digest: String,
    pub observation_token: ObservationToken,
}

/// The refusal an adapter returns when the generation it was handed has moved.
pub fn stale_observation(locator: &ResourceLocator, expected: &str, found: &str) -> DraftError {
    DraftError::new(
        DraftErrorKind::ConflictDetected,
        format!(
            "resource {locator} changed since it was observed \
             (expected state {expected}, found {found}); re-observe before continuing"
        ),
    )
}

/// PROVENANCE, NOT STATE: why Draft believes two observations describe the same
/// resource.
///
/// This is deliberately outside the state hash domain. Two observations with the
/// same id, locator and state but different continuity evidence are the *same*
/// authoritative state and must produce the same snapshot digest; only their
/// provenance records differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceIdentityProof {
    pub resource_id: ResourceId,
    pub basis: IdentityBasis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    pub recorded_at: crate::support::common::Timestamp,
}

/// How continuity of a [`ResourceId`] was established.
///
/// If none of these holds, an externally moved resource is modelled
/// conservatively as removal plus addition with distinct ids. Identity is never
/// inferred from equal or similar content: two resources that happen to contain
/// the same bytes are not thereby the same resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "basis", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityBasis {
    /// A relocation Draft itself performed; the operation is on the ledger.
    DraftRecorded {
        operation_id: crate::support::common::OperationId,
    },
    /// The adapter exposes a trustworthy stable external identity and asserts
    /// continuity on that basis.
    AdapterAsserted { external_identity: String },
    /// Same scheme, same locator body.
    LocatorStable,
}

/// A resource an adapter could see but cannot describe deterministically.
///
/// Reported rather than skipped: silently omitting it would let a later
/// comparison read the absence as a deletion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Untrackable {
    pub locator: ResourceLocator,
    pub reason: String,
}

/// How Draft may reach a resource's content, if at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "access", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentAccess {
    /// A logical resource with no byte stream.
    None,
    /// Content-addressed in Draft's object store.
    Object {
        digest: String,
        length: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
    /// The adapter supports bounded range reads without materializing the whole
    /// object.
    Ranged {
        length: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
}

impl ContentAccess {
    pub fn length(&self) -> Option<u64> {
        match self {
            Self::None => None,
            Self::Object { length, .. } | Self::Ranged { length, .. } => Some(*length),
        }
    }
}

/// Compute the canonical state digest for a filesystem-style resource.
///
/// Everything that participates in the resource's observable state participates
/// here, so a mode change or a retargeted link is a state change rather than an
/// invisible one.
pub fn filesystem_state_digest(
    locator: &ResourceLocator,
    form: Option<ResourceForm>,
    content_digest: Option<&str>,
    executable: bool,
    symlink_target: Option<&str>,
) -> String {
    hashing::canonical_hash(&serde_json::json!({
        "scheme": locator.scheme,
        "body": locator.body,
        "form": form,
        "content_digest": content_digest,
        "executable": executable,
        "symlink_target": symlink_target,
    }))
}

/// Compute the canonical state digest for a resource whose state is a declared
/// document rather than a byte stream.
pub fn declared_state_digest(
    locator: &ResourceLocator,
    form: Option<ResourceForm>,
    state: &serde_json::Value,
) -> String {
    hashing::canonical_hash(&serde_json::json!({
        "scheme": locator.scheme,
        "body": locator.body,
        "form": form,
        "state": state,
    }))
}

/// Reject a resource state that carries no deterministic identity.
pub fn require_state_digest(state: &RawResourceState) -> DraftResult<()> {
    if state.state_digest.trim().is_empty() {
        return Err(DraftError::new(
            DraftErrorKind::CorruptData,
            format!(
                "resource {} has no state digest; an adapter that cannot establish \
                 deterministic state identity must report it as untrackable",
                state.locator
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(digest: &str) -> RawResourceState {
        RawResourceState {
            resource_id: ResourceId::parse("res_1").unwrap(),
            locator: ResourceLocator::file("notes.txt"),
            form: Some(ResourceForm::Bytes),
            media_type: None,
            attributes: BTreeMap::new(),
            state_digest: digest.into(),
            content_digest: Some("sha256:abc".into()),
            metadata_digest: None,
            content_size: Some(12),
        }
    }

    #[test]
    fn authoritative_state_carries_no_class_token_or_identity_basis() {
        let encoded = serde_json::to_value(state("d")).unwrap();
        let fields: Vec<&str> = encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for forbidden in [
            "class_id",
            "resource_class",
            "resource_kind",
            "observation_token",
            "identity_basis",
            "coverage_domain",
        ] {
            assert!(
                !fields.contains(&forbidden),
                "authoritative state must not carry {forbidden}"
            );
        }
    }

    #[test]
    fn the_locator_body_is_opaque_but_still_matchable() {
        let catalog = ResourceLocator::new("catalog", "row/17");
        assert!(!catalog.is_file());
        assert!(ResourceLocator::file("a/b").is_file());
        // Display is for humans; nothing parses it back.
        assert_eq!(catalog.to_string(), "catalog://row/17");
    }

    #[test]
    fn a_state_digest_covers_mode_and_link_target() {
        let locator = ResourceLocator::file("bin/tool");
        let plain = filesystem_state_digest(
            &locator,
            Some(ResourceForm::Bytes),
            Some("sha256:abc"),
            false,
            None,
        );
        let executable = filesystem_state_digest(
            &locator,
            Some(ResourceForm::Bytes),
            Some("sha256:abc"),
            true,
            None,
        );
        // Identical bytes, different observable state: making a file executable
        // is a change, and a digest over content alone would miss it.
        assert_ne!(plain, executable);

        let linked = filesystem_state_digest(
            &locator,
            Some(ResourceForm::Reference),
            None,
            false,
            Some("../elsewhere"),
        );
        let relinked = filesystem_state_digest(
            &locator,
            Some(ResourceForm::Reference),
            None,
            false,
            Some("../other"),
        );
        assert_ne!(linked, relinked);
    }

    #[test]
    fn a_logical_resource_has_state_identity_without_bytes() {
        let locator = ResourceLocator::new("timeline", "clip/3");
        let digest = declared_state_digest(
            &locator,
            Some(ResourceForm::Logical),
            &serde_json::json!({"in": 120, "out": 480}),
        );
        assert!(!digest.is_empty());
        // Deterministic: the same declared state always yields the same digest.
        assert_eq!(
            digest,
            declared_state_digest(
                &locator,
                Some(ResourceForm::Logical),
                &serde_json::json!({"out": 480, "in": 120})
            )
        );
    }

    #[test]
    fn a_state_without_a_digest_is_refused() {
        assert!(require_state_digest(&state("d")).is_ok());
        let error = require_state_digest(&state("  ")).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn an_observed_ref_carries_the_fence_but_the_state_does_not() {
        let observed = RawObservedResource {
            state: state("digest-1"),
            observation_token: ObservationToken("gen-7".into()),
            coverage_domain: super::super::observation::CoverageDomainRef::new(
                super::super::observation::AdapterBindingId("core.filesystem".into()),
                "root",
            ),
        };
        let reference = observed.observed_ref();
        assert_eq!(reference.expected_state_digest, "digest-1");
        assert_eq!(reference.observation_token.as_str(), "gen-7");
        // The persisted half is the state alone.
        let encoded = serde_json::to_string(&observed.state).unwrap();
        assert!(!encoded.contains("gen-7"));
    }
}
