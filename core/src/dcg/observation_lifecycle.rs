//! Control over *what Draft is allowed to see*, and when that may change.
//!
//! Observation semantics decide what a snapshot even contains. If they could
//! change the instant a package was installed, a project's history would
//! silently acquire additions and removals nobody made — a dependency cache
//! entering the universe looks exactly like someone adding ten thousand files.
//!
//! So the semantics in force are **persisted and pinned**. Installing, updating,
//! disabling or removing anything that would change them produces a
//! [`PendingObservationContext`]: a candidate, sitting beside the active one,
//! changing nothing. A person previews what adopting it would do, and adoption
//! is an explicit, atomic, audited act that establishes a new baseline.
//!
//! One case is deliberately *not* a transition. A package update whose effective
//! observation semantics are byte-for-byte equivalent — same schemas, same
//! configuration, same coverage partition, same executable identity — changes
//! only *who observed*, never *what is observable*. That is the common upgrade,
//! and forcing a rebaseline for it would train people to click through the
//! thing that exists to make them look.

use serde::{Deserialize, Serialize};

use crate::dcg::observation::ObservationContext;
use crate::support::common::{now, SnapshotId, Timestamp};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing;

crate::id_newtype!(
    /// Identifies one adoption of a new observation context.
    ObservationTransitionId, "obt_");

/// The observation semantics in force, and the baseline taken under them.
///
/// The view rules travel with it on purpose. A context digest commits to their
/// *semantics*, but reproducing an observation needs the rules themselves — and
/// keeping observing under the adopted rules is exactly what makes a pending
/// change inert until somebody adopts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveObservationContext {
    pub schema_version: u32,
    pub context: ObservationContext,
    /// The exact exclusions the adopted view-rule bindings resolve to.
    #[serde(default)]
    pub view_rules: Vec<draft_extension_contract::ResourceRule>,
    /// The authoritative state observed when this context was adopted.
    pub baseline_snapshot_digest: String,
    pub baseline_snapshot_id: SnapshotId,
    pub adopted_at: Timestamp,
    /// The transition that installed this. Absent for a project's first
    /// context, which supersedes nothing because there was nothing before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_id: Option<ObservationTransitionId>,
}

impl crate::contracts::VersionedContract for ActiveObservationContext {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::ActiveObservationContext;
}

/// Why a candidate context appeared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingReason {
    /// A contributed view rule was added, changed or removed.
    ViewSemanticsChanged,
    /// An adapter's effective mechanism changed: a different executable,
    /// engine revision, configuration, schema or coverage partition.
    AdapterSemanticsChanged,
    /// An adapter binding appeared or disappeared entirely.
    AdapterSetChanged,
}

impl PendingReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ViewSemanticsChanged => "view_semantics_changed",
            Self::AdapterSemanticsChanged => "adapter_semantics_changed",
            Self::AdapterSetChanged => "adapter_set_changed",
        }
    }
}

/// A candidate context, waiting for someone to decide about it.
///
/// Recording one changes nothing observable. That is the whole point: the
/// project keeps being observed under the semantics it was observed under
/// yesterday until a person says otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingObservationContext {
    pub schema_version: u32,
    pub candidate: ObservationContext,
    #[serde(default)]
    pub candidate_view_rules: Vec<draft_extension_contract::ResourceRule>,
    /// The context this would replace.
    pub active_context_digest: String,
    pub reasons: Vec<PendingReason>,
    pub detected_at: Timestamp,
}

impl crate::contracts::VersionedContract for PendingObservationContext {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::PendingObservationContext;
}

/// What adopting a candidate would do, computed without doing any of it.
///
/// Every field here is derived from a *trial* observation that is never
/// persisted: no snapshot file, no provenance record, no anchor set, no change
/// to the active pointer. Previewing a change must not be a way of making it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationContextPreview {
    pub active_context_digest: String,
    pub candidate_context_digest: String,
    pub reasons: Vec<PendingReason>,
    /// Bindings the candidate adds, drops or redefines.
    pub added_bindings: Vec<String>,
    pub removed_bindings: Vec<String>,
    pub changed_bindings: Vec<String>,
    /// Resources that would enter the observed universe.
    pub would_enter: Vec<crate::dcg::resource::ResourceLocator>,
    /// Resources that would leave it — not deletions: they stop being project
    /// state, which is a different and much quieter thing.
    pub would_leave: Vec<crate::dcg::resource::ResourceLocator>,
    /// Work that would be superseded, and would need re-deriving.
    pub would_supersede: Vec<SupersededWork>,
}

/// One piece of mutable work that an adoption strands.
///
/// Superseded work stays readable. What it loses is the right to be mutated or
/// promoted, because it was derived under semantics the project no longer uses
/// and nothing can honestly compare it to what is observed now.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupersededWork {
    pub kind: SupersededKind,
    pub id: String,
    /// The context it was created under.
    pub context_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupersededKind {
    Change,
    ResourceSession,
}

impl SupersededKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Change => "change",
            Self::ResourceSession => "resource_session",
        }
    }
}

/// The immutable record of one adoption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationContextTransition {
    pub schema_version: u32,
    pub transition_id: ObservationTransitionId,
    pub from_context_digest: String,
    pub to_context_digest: String,
    /// The new baseline observed under the newly adopted semantics.
    pub baseline_snapshot_digest: String,
    pub baseline_snapshot_id: SnapshotId,
    pub reasons: Vec<PendingReason>,
    pub superseded: Vec<SupersededWork>,
    pub adopted_at: Timestamp,
    pub transition_digest: String,
}

impl crate::contracts::VersionedContract for ObservationContextTransition {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::ObservationContextTransition;
}

impl ObservationContextTransition {
    /// Seal the record, deriving its immutable identity from its content.
    pub fn seal(mut self) -> Self {
        self.transition_digest.clear();
        self.transition_digest = hashing::canonical_hash(&serde_json::json!({
            "transition_id": self.transition_id,
            "from_context_digest": self.from_context_digest,
            "to_context_digest": self.to_context_digest,
            "baseline_snapshot_digest": self.baseline_snapshot_digest,
            "reasons": self.reasons,
            "superseded": self.superseded,
            "adopted_at": self.adopted_at,
        }));
        self
    }
}

/// The refusal returned when work was created under semantics no longer in
/// force.
///
/// Deliberately not an error about the work being *wrong*. It is still exactly
/// what it was; there is simply no longer a shared frame in which to compare it
/// to the project, and pretending otherwise would let a promotion carry a change
/// set derived against a universe that no longer exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextSuperseded {
    pub expected_context_digest: String,
    pub active_context_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_id: Option<ObservationTransitionId>,
}

impl ContextSuperseded {
    pub fn into_error(self) -> DraftError {
        DraftError::new(
            DraftErrorKind::ContextSuperseded,
            format!(
                "this work was derived under observation context {} and the project now observes \
                 under {}; it remains readable, but must be re-derived before it can be changed \
                 or promoted",
                short(&self.expected_context_digest),
                short(&self.active_context_digest)
            ),
        )
        .with_suggestion("re-create the Change from the current baseline")
    }
}

fn short(digest: &str) -> &str {
    if digest.len() > 12 {
        &digest[..12]
    } else {
        digest
    }
}

/// Compare two contexts and say why they differ, or that they do not.
///
/// Returning an empty list is the fast path that matters: a package update whose
/// observation semantics are unchanged produces no reasons, and therefore no
/// pending context, no preview, no adoption and no rebaseline.
pub fn diff_contexts(
    active: &ObservationContext,
    candidate: &ObservationContext,
) -> Vec<PendingReason> {
    let mut reasons = Vec::new();
    if active.context_digest == candidate.context_digest {
        return reasons;
    }

    let active_adapters: std::collections::BTreeMap<_, _> = active
        .adapter_bindings
        .iter()
        .map(|binding| (binding.binding_id.clone(), binding))
        .collect();
    let candidate_adapters: std::collections::BTreeMap<_, _> = candidate
        .adapter_bindings
        .iter()
        .map(|binding| (binding.binding_id.clone(), binding))
        .collect();

    if active_adapters.keys().ne(candidate_adapters.keys()) {
        reasons.push(PendingReason::AdapterSetChanged);
    }
    for (id, active_binding) in &active_adapters {
        if let Some(candidate_binding) = candidate_adapters.get(id) {
            if active_binding != candidate_binding {
                reasons.push(PendingReason::AdapterSemanticsChanged);
                break;
            }
        }
    }

    if active.view_rule_bindings != candidate.view_rule_bindings {
        reasons.push(PendingReason::ViewSemanticsChanged);
    }

    // The digests differ, so something did. Naming nothing would leave a
    // pending context nobody could explain.
    if reasons.is_empty() {
        reasons.push(PendingReason::AdapterSemanticsChanged);
    }
    reasons.sort_by_key(|reason| reason.as_str());
    reasons.dedup();
    reasons
}

/// Which bindings were added, removed and redefined between two contexts.
pub fn binding_changes(
    active: &ObservationContext,
    candidate: &ObservationContext,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    let active_adapters: std::collections::BTreeMap<_, _> = active
        .adapter_bindings
        .iter()
        .map(|binding| (binding.binding_id.0.clone(), binding))
        .collect();
    let candidate_adapters: std::collections::BTreeMap<_, _> = candidate
        .adapter_bindings
        .iter()
        .map(|binding| (binding.binding_id.0.clone(), binding))
        .collect();
    for (id, binding) in &candidate_adapters {
        match active_adapters.get(id) {
            None => added.push(id.clone()),
            Some(existing) if existing != binding => changed.push(id.clone()),
            Some(_) => {}
        }
    }
    for id in active_adapters.keys() {
        if !candidate_adapters.contains_key(id) {
            removed.push(id.clone());
        }
    }

    let active_views: std::collections::BTreeMap<_, _> = active
        .view_rule_bindings
        .iter()
        .map(|binding| (binding.binding_id.0.clone(), binding))
        .collect();
    let candidate_views: std::collections::BTreeMap<_, _> = candidate
        .view_rule_bindings
        .iter()
        .map(|binding| (binding.binding_id.0.clone(), binding))
        .collect();
    for (id, binding) in &candidate_views {
        match active_views.get(id) {
            None => added.push(id.clone()),
            Some(existing) if existing != binding => changed.push(id.clone()),
            Some(_) => {}
        }
    }
    for id in active_views.keys() {
        if !candidate_views.contains_key(id) {
            removed.push(id.clone());
        }
    }

    added.sort();
    removed.sort();
    changed.sort();
    (added, removed, changed)
}

/// Refuse to proceed when work belongs to a context that is no longer active.
pub fn require_current_context(
    expected_context_digest: &str,
    active: &ActiveObservationContext,
) -> DraftResult<()> {
    if expected_context_digest.is_empty()
        || expected_context_digest == active.context.context_digest
    {
        return Ok(());
    }
    Err(ContextSuperseded {
        expected_context_digest: expected_context_digest.to_string(),
        active_context_digest: active.context.context_digest.clone(),
        transition_id: active.transition_id.clone(),
    }
    .into_error())
}

/// Build the record of one adoption.
pub fn transition(
    from_context_digest: String,
    to_context_digest: String,
    baseline: &crate::dcg::state::Snapshot,
    reasons: Vec<PendingReason>,
    superseded: Vec<SupersededWork>,
) -> ObservationContextTransition {
    ObservationContextTransition {
        schema_version: crate::contracts::current_version(
            crate::contracts::ContractId::ObservationContextTransition,
        ),
        transition_id: ObservationTransitionId::generate(),
        from_context_digest,
        to_context_digest,
        baseline_snapshot_digest: baseline.snapshot_digest.clone(),
        baseline_snapshot_id: baseline.id.clone(),
        reasons,
        superseded,
        adopted_at: now(),
        transition_digest: String::new(),
    }
    .seal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcg::observation::{
        AdapterBindingId, AdapterObservationBinding, EffectiveObservationMechanism,
        ViewRuleBinding, ViewRuleBindingId,
    };

    fn adapter(id: &str, config: &str) -> AdapterObservationBinding {
        AdapterObservationBinding {
            binding_id: AdapterBindingId(id.into()),
            contribution_id: id.into(),
            contribution_semantics_digest: format!("semantics-{config}"),
            mechanism: EffectiveObservationMechanism::Engine {
                engine: draft_extension_contract::EngineId::ResourceEnumeration,
                engine_revision: 1,
                engine_config_digest: config.into(),
                request_schema_digest: String::new(),
                response_schema_digest: String::new(),
                coverage_domain_semantics_digest: "partition-1".into(),
            },
        }
    }

    fn view(id: &str, digest: &str) -> ViewRuleBinding {
        ViewRuleBinding {
            binding_id: ViewRuleBindingId(id.into()),
            contribution_id: id.into(),
            contribution_semantics_digest: digest.into(),
        }
    }

    #[test]
    fn identical_semantics_produce_no_reason_to_transition() {
        // The common upgrade. A new package revision that observes identically
        // must not force a rebaseline, or the prompt that exists to make people
        // look becomes the prompt they learn to dismiss.
        let active = ObservationContext::build(vec![adapter("fs", "cfg")], vec![]);
        let candidate = ObservationContext::build(vec![adapter("fs", "cfg")], vec![]);
        assert_eq!(active.context_digest, candidate.context_digest);
        assert!(diff_contexts(&active, &candidate).is_empty());
    }

    #[test]
    fn a_changed_view_rule_is_named_as_a_view_change() {
        let active = ObservationContext::build(vec![adapter("fs", "cfg")], vec![]);
        let candidate =
            ObservationContext::build(vec![adapter("fs", "cfg")], vec![view("sw", "excl-1")]);
        let reasons = diff_contexts(&active, &candidate);
        assert_eq!(reasons, vec![PendingReason::ViewSemanticsChanged]);
        let (added, removed, changed) = binding_changes(&active, &candidate);
        assert_eq!(added, vec!["sw".to_string()]);
        assert!(removed.is_empty() && changed.is_empty());
    }

    #[test]
    fn a_swapped_adapter_configuration_is_named_as_an_adapter_change() {
        let active = ObservationContext::build(vec![adapter("fs", "cfg-a")], vec![]);
        let candidate = ObservationContext::build(vec![adapter("fs", "cfg-b")], vec![]);
        assert_eq!(
            diff_contexts(&active, &candidate),
            vec![PendingReason::AdapterSemanticsChanged]
        );
        let (added, removed, changed) = binding_changes(&active, &candidate);
        assert!(added.is_empty() && removed.is_empty());
        assert_eq!(changed, vec!["fs".to_string()]);
    }

    #[test]
    fn work_from_a_retired_context_is_superseded_not_destroyed() {
        let active = ActiveObservationContext {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ActiveObservationContext,
            ),
            context: ObservationContext::build(vec![adapter("fs", "new")], vec![]),
            view_rules: Vec::new(),
            baseline_snapshot_digest: "base".into(),
            baseline_snapshot_id: SnapshotId::new("chk_1"),
            adopted_at: now(),
            transition_id: Some(ObservationTransitionId::new("obt_1")),
        };
        // Work from the current context proceeds.
        assert!(require_current_context(&active.context.context_digest, &active).is_ok());
        // Work from an older one is refused with its own kind, so a caller can
        // tell "re-derive this" apart from "this is broken".
        let error = require_current_context("an-older-context", &active).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ContextSuperseded);
        assert!(error.to_string().contains("re-derived"));
    }

    #[test]
    fn a_transition_record_is_identified_by_its_own_content() {
        let snapshot = crate::dcg::state::tests_support::sealed("prj_1", &["a.txt"]);
        let first = transition(
            "from".into(),
            "to".into(),
            &snapshot,
            vec![PendingReason::ViewSemanticsChanged],
            Vec::new(),
        );
        assert!(!first.transition_digest.is_empty());
        // Reformatting the record cannot change what it says.
        let reserialized: ObservationContextTransition =
            serde_json::from_str(&serde_json::to_string(&first).unwrap()).unwrap();
        assert_eq!(
            reserialized.clone().seal().transition_digest,
            first.transition_digest
        );
    }
}
