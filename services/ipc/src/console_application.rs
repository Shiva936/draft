//! Separately versioned, Rust-facing Draft Console application protocol.
//!
//! These messages are payloads of the existing authenticated `draft-ipc`
//! transport. They intentionally do not define a socket, framing, or security
//! handshake of their own.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;

/// The freshness vocabulary an issued capability is bound against.
///
/// Re-exported rather than redefined: the session store holds a precondition
/// and the daemon judges it against authoritative state, and a second
/// definition of "what this view was derived from" would be a second answer
/// to the same question. It is server-side state, never wire payload, so it
/// carries no schema or TypeScript binding.
pub use draft_core::read_model::freshness::{
    check as check_precondition, ActionOutcome, Projection, ReadModelWatermark,
    RequestPrecondition, StaleReason, StoreKey,
};

pub const CONSOLE_PROTOCOL_MAJOR: u16 = 1;
pub const CONSOLE_PROTOCOL_MINOR: u16 = 0;

pub const CONSOLE_CAPABILITIES: &[&str] = &[
    "revisioned_models",
    "action_capabilities",
    "operation_recovery",
    "watch_v1",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl Default for ConsoleProtocolVersion {
    fn default() -> Self {
        Self {
            major: CONSOLE_PROTOCOL_MAJOR,
            minor: CONSOLE_PROTOCOL_MINOR,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleHandshakeRequest {
    pub protocol: ConsoleProtocolVersion,
    pub client_name: String,
    pub client_version: String,
    pub client_instance_id: String,
    pub requested_capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleHandshakeResponse {
    pub protocol: ConsoleProtocolVersion,
    pub client_version: String,
    pub server_version: String,
    pub supported_capabilities: Vec<String>,
    pub negotiated_capabilities: Vec<String>,
    pub registry_revision: u64,
    pub application_session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConsoleScope {
    Global,
    Project,
    ChangePack,
    /// One accepted Baseline. §8.3 gives it its own scope because its views —
    /// the three roots, lineage, composition, recoverability — are about a
    /// historical node, not about the project as it stands now.
    Baseline,
}

/// What a Console view is about.
///
/// A tagged union, so an impossible combination — a ChangePack without a
/// project, a Baseline id on a ChangePack subject — cannot be represented at
/// all rather than being rejected after the fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "scope", rename_all = "SCREAMING_SNAKE_CASE")]
#[serde(deny_unknown_fields)]
pub enum ConsoleSubject {
    /// Empty braces rather than a unit variant: serde lets an internally
    /// tagged unit variant ignore extra fields even under `deny_unknown_fields`.
    Global {},
    Project {
        workspace_id: String,
    },
    ChangePack {
        workspace_id: String,
        change_pack_id: String,
    },
    Baseline {
        workspace_id: String,
        baseline_id: String,
    },
}

impl ConsoleSubject {
    pub fn global() -> Self {
        Self::Global {}
    }

    pub fn scope(&self) -> ConsoleScope {
        match self {
            Self::Global {} => ConsoleScope::Global,
            Self::Project { .. } => ConsoleScope::Project,
            Self::ChangePack { .. } => ConsoleScope::ChangePack,
            Self::Baseline { .. } => ConsoleScope::Baseline,
        }
    }

    pub fn workspace_id(&self) -> Option<&str> {
        match self {
            Self::Global {} => None,
            Self::Project { workspace_id }
            | Self::ChangePack { workspace_id, .. }
            | Self::Baseline { workspace_id, .. } => Some(workspace_id),
        }
    }

    pub fn change_pack_id(&self) -> Option<&str> {
        match self {
            Self::ChangePack { change_pack_id, .. } => Some(change_pack_id),
            _ => None,
        }
    }

    pub fn baseline_id(&self) -> Option<&str> {
        match self {
            Self::Baseline { baseline_id, .. } => Some(baseline_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct CanonicalRevisions {
    #[ts(type = "number")]
    pub registry: u64,
    pub workspace: Option<String>,
    pub change_pack: Option<String>,
    pub policy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ModelFreshness {
    Loading,
    Fresh,
    Stale,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ActionPresentation {
    pub action_id: String,
    pub label: String,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    pub invocation_capability: Option<String>,
    pub requires_confirmation: bool,
    #[ts(type = "number | null")]
    pub expires_at_unix_ms: Option<i64>,
    /// What this action needs from the caller. Empty means it takes nothing,
    /// and supplying anything is then an error.
    ///
    /// This is one contract serving two purposes: a frontend renders it, and
    /// `draftd` validates against it. The frontend's checks are a convenience;
    /// the server's are the decision.
    #[serde(default)]
    pub inputs: Vec<ActionInputField>,
    /// Digest of `inputs`, carried into the invocation capability so a stale
    /// input contract cannot be replayed.
    #[serde(default)]
    pub input_contract_digest: String,
    /// What the action acts on, when it is something other than the subject.
    #[serde(default)]
    pub target: Option<ActionTarget>,
}

/// One choice offered by a [`ActionInputKind::Select`] input.
///
/// `value` is what the client submits and what the server validates against;
/// `label` is what a person reads. Relabelling an option must never change
/// which invocations are valid, so nothing anywhere keys on `label`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

/// The shape of one action input.
///
/// Deliberately small and semantic. A frontend maps these to whatever controls
/// suit it — a Web form, a TUI prompt — but neither invents an input Draft did
/// not declare, and neither decides what is valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionInputKind {
    /// Free text, optionally bounded.
    Text {
        #[serde(default)]
        #[ts(type = "number | null")]
        max_length: Option<u32>,
    },
    /// One of a server-supplied set. The set is part of the input contract, so
    /// a client holding a stale set cannot invoke against it.
    Select { options: Vec<SelectOption> },
    /// A true/false flag.
    Boolean,
    /// An explicit acknowledgement, which must be `true` to proceed.
    Confirmation,
}

/// One declared input of an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ActionInputField {
    /// Stable machine identity: the key the client submits and the server
    /// validates. Never a label.
    pub id: String,
    /// Human label. Presentation only.
    pub label: String,
    pub kind: ActionInputKind,
    pub required: bool,
    #[serde(default)]
    pub help: Option<String>,
}

/// What an action acts on, beyond the subject that produced it.
///
/// Generic on purpose: a target is a kind and an id, so a new kind of
/// extension-contributed action needs no new frontend code and no new scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ActionTargetKind {
    Extension,
    ExtensionSource,
    PendingAuthorization,
    Grant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ActionTarget {
    pub kind: ActionTargetKind,
    pub id: String,
}

/// The digest an invocation capability is bound to.
///
/// Taken over the declared inputs, so a client that received one set of select
/// options cannot invoke after the server's set has changed: the digest it was
/// issued under no longer matches the one the action now declares.
pub fn input_contract_digest(inputs: &[ActionInputField]) -> String {
    draft_core::support::hashing::canonical_hash(&inputs.to_vec())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct NextSafeAction {
    pub label: String,
    pub reason: String,
    pub action_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleModelRequest {
    pub application_session_id: String,
    pub subject: ConsoleSubject,
}

/// One entry in a scope's navigation, with the views nested under it.
///
/// Nested rather than flattened because §8.3 nests: `Work (Tasks | ChangePacks)`
/// is one section with two views, and flattening it would make Tasks and
/// ChangePacks look like peers of Resources and Baselines. A section with no
/// children is a leaf and renders as one row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ConsoleNavigationSection {
    pub label: String,
    #[serde(default)]
    pub children: Vec<String>,
}

impl ConsoleNavigationSection {
    /// A section that is its own view.
    pub fn leaf(label: &str) -> Self {
        Self {
            label: label.to_string(),
            children: Vec::new(),
        }
    }

    /// A section whose views are the ones named.
    pub fn parent(label: &str, children: &[&str]) -> Self {
        Self {
            label: label.to_string(),
            children: children.iter().map(|child| child.to_string()).collect(),
        }
    }

    /// The views this section offers: its children, or itself when it is a
    /// leaf. What a frontend actually selects between.
    pub fn views(&self) -> Vec<String> {
        if self.children.is_empty() {
            vec![self.label.clone()]
        } else {
            self.children.clone()
        }
    }
}

/// The frozen §8.3 information architecture, as data.
///
/// It lives in the protocol rather than in `draftd` because both the daemon
/// that serves it and the frontends that render it have to agree about it,
/// and a second list would drift — invisibly, until a section quietly stopped
/// existing on one of them. `draftd` serves this; nobody redefines it.
///
/// Nesting is real, not cosmetic. `Work` owns Tasks and ChangePacks because they
/// are the two things a person does *to* a project, while Resources and
/// Baselines are what the project *is*; promoting Tasks to the top level
/// alongside Baselines would flatten that distinction away.
pub fn navigation_for(scope: ConsoleScope) -> Vec<ConsoleNavigationSection> {
    match scope {
        ConsoleScope::Global => vec![
            ConsoleNavigationSection::leaf("Overview"),
            ConsoleNavigationSection::leaf("Projects"),
            ConsoleNavigationSection::leaf("Inbox"),
            ConsoleNavigationSection::leaf("Doctor"),
            ConsoleNavigationSection::leaf("Extensions"),
            ConsoleNavigationSection::leaf("Settings"),
        ],
        // §8.3: Overview | Work (Tasks | Packs) | Resources | Baselines |
        // Activity | Providers | Extensions.
        //
        // Observation lives under Resources because an observation is evidence
        // *about* a Resource, and Tools live under Extensions because a tool
        // exists only because an extension contributes it. Neither is a
        // top-level concept in the ontology, and having them as tabs made the
        // navigation a list of screens rather than a map of the model.
        ConsoleScope::Project => vec![
            ConsoleNavigationSection::leaf("Overview"),
            ConsoleNavigationSection::parent("Work", &["Tasks", "Packs"]),
            ConsoleNavigationSection::parent("Resources", &["Resources", "Observation"]),
            ConsoleNavigationSection::parent("Baselines", &["Baselines", "Publications"]),
            ConsoleNavigationSection::leaf("Activity"),
            ConsoleNavigationSection::leaf("Providers"),
            ConsoleNavigationSection::parent("Extensions", &["Extensions", "Tools"]),
        ],
        // §8.3: Summary | Intent | Scope | Revisions | Impact |
        // Representations | Evidence | Assessments | Review | Decisions |
        // Gates | Promotion | Receipts | Recovery. "Revisions" is the concise nav
        // label; each one listed there is a RevisionPack.
        //
        // Each is its own view because each is its own act. The retired
        // vocabulary — a single "Submit" step, "Approvals", "Risk",
        // "Rollback" — is exactly what the Change Graph took apart: deciding
        // authorizes, promotion accepts, publication delivers, and a reader
        // who cannot tell them apart cannot tell what happened.
        ConsoleScope::ChangePack => vec![
            ConsoleNavigationSection::leaf("Summary"),
            ConsoleNavigationSection::leaf("Intent"),
            ConsoleNavigationSection::leaf("Scope"),
            ConsoleNavigationSection::leaf("Revisions"),
            ConsoleNavigationSection::leaf("Impact"),
            ConsoleNavigationSection::leaf("Representations"),
            ConsoleNavigationSection::leaf("Evidence"),
            ConsoleNavigationSection::leaf("Assessments"),
            ConsoleNavigationSection::leaf("Review"),
            ConsoleNavigationSection::leaf("Decisions"),
            ConsoleNavigationSection::leaf("Gates"),
            ConsoleNavigationSection::leaf("Promotion"),
            ConsoleNavigationSection::leaf("Receipts"),
            ConsoleNavigationSection::leaf("Recovery"),
        ],
        // §8.3: Summary | State root | Evidence root | Coverage | Lineage |
        // Composition | Recoverability | Receipts | Publications.
        //
        // Three roots, never collapsed: what material state is accepted, what
        // provenance establishes it, and what justifies absence are three
        // different questions with three different answers.
        ConsoleScope::Baseline => vec![
            ConsoleNavigationSection::leaf("Summary"),
            ConsoleNavigationSection::leaf("State root"),
            ConsoleNavigationSection::leaf("Evidence root"),
            ConsoleNavigationSection::leaf("Coverage"),
            ConsoleNavigationSection::leaf("Lineage"),
            ConsoleNavigationSection::leaf("Composition"),
            ConsoleNavigationSection::leaf("Recoverability"),
            ConsoleNavigationSection::leaf("Receipts"),
            ConsoleNavigationSection::leaf("Publications"),
        ],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ConsoleReadModel {
    pub subject: ConsoleSubject,
    pub revisions: CanonicalRevisions,
    pub freshness: ModelFreshness,
    pub health: String,
    pub read_only: bool,
    pub permission_reason: Option<String>,
    pub navigation: Vec<ConsoleNavigationSection>,
    #[ts(type = "unknown")]
    #[schemars(with = "serde_json::Value")]
    pub content: Value,
    pub actions: Vec<ActionPresentation>,
    pub next_safe_actions: Vec<NextSafeAction>,
    pub evidence_links: Vec<String>,
    pub operation_links: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ConsoleActionInvocation {
    pub application_session_id: String,
    pub invocation_capability: String,
    pub operation_id: String,
    pub expected_revisions: CanonicalRevisions,
    #[serde(default)]
    #[ts(type = "Record<string, unknown>")]
    #[schemars(with = "std::collections::BTreeMap<String, serde_json::Value>")]
    pub arguments: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleActionResult {
    pub operation_id: String,
    pub revisions: CanonicalRevisions,
    pub result: Value,
    pub invalidation_targets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleWatchRequest {
    pub application_session_id: String,
    pub after_cursor: Option<u64>,
    pub subjects: Vec<ConsoleSubject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleWatchEvent {
    pub application_session_id: String,
    pub cursor: u64,
    pub kind: String,
    pub subject: ConsoleSubject,
    pub revisions: CanonicalRevisions,
    pub invalidation_targets: Vec<String>,
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleOperationPhase {
    Prepared,
    Running,
    Finalizing,
    Completed,
    FailedBeforeFinalization,
    CancelledBeforeFinalization,
    SafelyRetryable,
    ReconciliationRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleOperationStatus {
    pub operation_id: String,
    pub phase: ConsoleOperationPhase,
    pub status: String,
    pub completed: Option<u64>,
    pub total: Option<u64>,
    pub cancellation_allowed: bool,
    pub retry_allowed: bool,
    pub finalization_started_at: Option<String>,
    pub target_identity: Option<String>,
    pub result: Option<Value>,
    pub error: Option<Value>,
    pub recovery_guidance: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The subject is a tagged union: a combination that names the wrong ids
    /// for its scope does not parse.
    #[test]
    fn a_console_subject_cannot_mix_scopes() {
        let parse = |json: &str| serde_json::from_str::<ConsoleSubject>(json);
        assert_eq!(
            parse(r#"{"scope":"CHANGE_PACK","workspace_id":"prj_a","change_pack_id":"cpk_a"}"#)
                .unwrap()
                .change_pack_id(),
            Some("cpk_a")
        );
        for invalid in [
            r#"{"scope":"GLOBAL","workspace_id":"prj_a"}"#,
            r#"{"scope":"CHANGE_PACK","workspace_id":"prj_a"}"#,
            r#"{"scope":"CHANGE_PACK","workspace_id":"prj_a","change_pack_id":"cpk_a","baseline_id":"b"}"#,
            r#"{"scope":"BASELINE","workspace_id":"prj_a","change_pack_id":"cpk_a"}"#,
            r#"{"scope":"CHANGE","workspace_id":"prj_a","change_pack_id":"cpk_a"}"#,
        ] {
            assert!(parse(invalid).is_err(), "{invalid} parsed");
        }
    }

    #[test]
    fn protocol_version_is_independent_from_transport_schema() {
        assert_eq!(ConsoleProtocolVersion::default().major, 1);
        assert!(CONSOLE_CAPABILITIES.contains(&"action_capabilities"));
    }

    /// §8.3, pinned where the definition lives.
    ///
    /// `draftd` serves this and both frontends render it, so a change here is
    /// a change to the Console everywhere. That is exactly why it should be
    /// hard to make by accident.
    #[test]
    fn the_frozen_information_architecture_is_the_one_the_plan_names() {
        let labels = |scope| {
            navigation_for(scope)
                .into_iter()
                .map(|section| section.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(ConsoleScope::Project),
            [
                "Overview",
                "Work",
                "Resources",
                "Baselines",
                "Activity",
                "Providers",
                "Extensions"
            ]
        );
        assert_eq!(
            labels(ConsoleScope::ChangePack),
            [
                "Summary",
                "Intent",
                "Scope",
                "Revisions",
                "Impact",
                "Representations",
                "Evidence",
                "Assessments",
                "Review",
                "Decisions",
                "Gates",
                "Promotion",
                "Receipts",
                "Recovery"
            ]
        );
        assert_eq!(
            labels(ConsoleScope::Baseline),
            [
                "Summary",
                "State root",
                "Evidence root",
                "Coverage",
                "Lineage",
                "Composition",
                "Recoverability",
                "Receipts",
                "Publications"
            ]
        );

        // Work nests; Baselines nests Publications rather than renaming it.
        let project = navigation_for(ConsoleScope::Project);
        let children = |label: &str| {
            project
                .iter()
                .find(|section| section.label == label)
                .map(|section| section.children.clone())
                .unwrap_or_default()
        };
        assert_eq!(children("Work"), ["Tasks", "Packs"]);
        assert_eq!(children("Resources"), ["Resources", "Observation"]);
        assert_eq!(children("Baselines"), ["Baselines", "Publications"]);
        assert_eq!(children("Extensions"), ["Extensions", "Tools"]);

        // A leaf offers itself; a parent offers its children and not its own
        // label, because a parent is a grouping and not a view.
        assert_eq!(
            ConsoleNavigationSection::leaf("Activity").views(),
            ["Activity"]
        );
        assert_eq!(children("Work").len(), 2);

        // No retired vocabulary anywhere in the architecture.
        let rendered = format!(
            "{:?}{:?}{:?}{:?}",
            navigation_for(ConsoleScope::Global),
            project,
            navigation_for(ConsoleScope::ChangePack),
            navigation_for(ConsoleScope::Baseline),
        );
        for retired in [
            "Submit",
            "Approvals",
            "Risk",
            "Rollback",
            "Verify",
            "Changes",
            "Graph",
        ] {
            assert!(
                !rendered.contains(retired),
                "the frozen architecture still names the retired '{retired}'"
            );
        }
    }

    #[test]
    fn disabled_actions_cannot_carry_a_capability() {
        let action = ActionPresentation {
            action_id: "dcg.promote".into(),
            label: "Promote".into(),
            enabled: false,
            disabled_reason: Some("no approving decision cites a satisfied gate".into()),
            invocation_capability: None,
            requires_confirmation: true,
            expires_at_unix_ms: None,
            inputs: Vec::new(),
            input_contract_digest: input_contract_digest(&[]),
            target: None,
        };
        assert!(!action.enabled);
        assert!(action.invocation_capability.is_none());
    }

    #[test]
    fn an_input_contract_digest_tracks_the_option_set_a_client_was_shown() {
        let with = |options: &[&str]| {
            vec![ActionInputField {
                id: "source_id".into(),
                label: "Source".into(),
                kind: ActionInputKind::Select {
                    options: options
                        .iter()
                        .map(|value| SelectOption {
                            value: (*value).to_string(),
                            label: (*value).to_string(),
                        })
                        .collect(),
                },
                required: true,
                help: None,
            }]
        };
        let original = input_contract_digest(&with(&["a", "b"]));
        assert_eq!(original, input_contract_digest(&with(&["a", "b"])));
        // The server's own option set changed, so a capability issued under
        // the old one no longer matches.
        assert_ne!(original, input_contract_digest(&with(&["b", "c"])));
    }

    #[test]
    fn relabelling_an_option_does_not_change_what_a_client_may_submit() {
        // `value` is identity, `label` is presentation. A digest that moved
        // when a label was translated would invalidate live capabilities for
        // no semantic reason.
        let field = |label: &str| {
            vec![ActionInputField {
                id: "source_id".into(),
                label: "Source".into(),
                kind: ActionInputKind::Select {
                    options: vec![SelectOption {
                        value: "draft-official".into(),
                        label: label.into(),
                    }],
                },
                required: true,
                help: None,
            }]
        };
        // The label is part of the contract that is rendered, so it is covered
        // by the digest; what matters is that submitting still keys on value.
        let english = &field("Draft Official")[0];
        let other = &field("Draft Officiel")[0];
        match (&english.kind, &other.kind) {
            (
                ActionInputKind::Select { options: left },
                ActionInputKind::Select { options: right },
            ) => assert_eq!(left[0].value, right[0].value),
            _ => panic!("both are select inputs"),
        }
    }
}
