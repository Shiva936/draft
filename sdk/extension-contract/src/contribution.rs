//! What a contribution actually says.
//!
//! Every payload here is data. The most powerful thing an extension can declare
//! is a [`MechanismOperation`] — a schema-bound request, a schema-bound
//! response, a byte bound, and either a generic platform engine or a
//! [`StructuredCommand`] — and even that is inert until Draft holds a matching
//! authorization for the exact installed artifact. Draft runs the program
//! itself, through its own hardened runner, directly on `program` and argv.
//! There is no shell, no script, and no extension code.
//!
//! Nothing in this module names a file extension, a language, a toolchain or a
//! diff. Where a domain needs vocabulary — a resource class, a coordinate
//! space, a check name — it contributes a namespaced identifier that Draft
//! stores, compares for equality and never interprets.

use crate::identifier::{IdentifierClass, NamespacedId};
use crate::schema::SchemaRef;
use crate::{FormatError, FormatResult};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Permissions and command execution
// ---------------------------------------------------------------------------

/// A permission an extension asks for.
///
/// The set is closed and small on purpose: it grows only when a real
/// contribution needs something it cannot express today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ExtensionPermission {
    /// Draft may run the programs this extension declares.
    ///
    /// The wire name is spelled explicitly rather than derived, so the manifest
    /// form, the JSON schema, the CLI flag and [`ExtensionPermission::as_str`]
    /// cannot drift apart.
    #[serde(rename = "process.execute")]
    ProcessExecute,
}

impl ExtensionPermission {
    /// The stable wire name, as it appears in a manifest and on the CLI.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProcessExecute => "process.execute",
        }
    }

    /// Parse a permission by its wire name.
    pub fn parse(value: &str) -> FormatResult<Self> {
        match value {
            "process.execute" => Ok(Self::ProcessExecute),
            other => Err(FormatError::Identity(format!(
                "unknown extension permission '{other}'"
            ))),
        }
    }

    /// Every permission this format revision defines.
    pub const ALL: &'static [Self] = &[Self::ProcessExecute];
}

impl std::fmt::Display for ExtensionPermission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A program an extension declares for Draft to run on its behalf.
///
/// `args` may contain `{{name}}` placeholders, which Draft substitutes strictly
/// — an unknown placeholder is an error, never a literal passed through to the
/// program. The argument vector is passed to the process directly, so quoting,
/// globbing, redirection and command chaining are not available to an
/// extension: there is no shell in this path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredCommand {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Subdirectory of the operation's runtime scope to run in. `None` means the
    /// scope root. This is never the project root: Draft materializes the inputs
    /// an operation is entitled to and runs the command against those.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl StructuredCommand {
    /// The permission running this command requires.
    pub const fn required_permission() -> ExtensionPermission {
        ExtensionPermission::ProcessExecute
    }

    pub fn validate(&self) -> FormatResult<()> {
        if self.program.trim().is_empty() {
            return Err(FormatError::Identity(
                "a declared command must name a program".into(),
            ));
        }
        // A program name, not a command line: an extension cannot smuggle a
        // shell invocation into the program slot.
        if self
            .program
            .contains(|character: char| character.is_whitespace())
        {
            return Err(FormatError::Identity(format!(
                "declared program '{}' must be a single program name, not a command line",
                self.program
            )));
        }
        if let Some(cwd) = &self.cwd {
            crate::package::validate_relative_path(cwd)?;
        }
        if self.timeout_ms == Some(0) {
            return Err(FormatError::Identity(
                "a declared command timeout must be greater than zero".into(),
            ));
        }
        Ok(())
    }

    /// A stable single-line rendering, for evidence and human output.
    pub fn display(&self) -> String {
        let mut rendered = String::from(&self.program);
        for argument in &self.args {
            rendered.push(' ');
            rendered.push_str(argument);
        }
        rendered
    }
}

// ---------------------------------------------------------------------------
// Mechanisms: one operation, one authoritative contract declaration
// ---------------------------------------------------------------------------

/// The bounded set of generic platform engines.
///
/// Each is a real, domain-free algorithm whose *meaning* is entirely
/// contributed: the engine knows tokens, records and attributes, never lines,
/// text or symbols. `revision` exists so changing an engine's implementation
/// semantics can never silently reuse a previous observation or derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineId {
    /// Compares before/after state digests and claims the whole resource.
    WholeResource,
    /// Aligns two opaque token streams and claims regions of a contributed
    /// one-dimensional coordinate space.
    SequenceAlignment,
    /// Compares canonical `(key, value-digest)` record sets and claims opaque
    /// keys in a contributed key space.
    KeyedRecordSet,
    /// Projects typed intrinsic attributes into elements and relations.
    AttributeProjection,
    /// Enumerates and describes resources for a contributed locator scheme.
    ResourceEnumeration,
}

impl EngineId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WholeResource => "whole_resource",
            Self::SequenceAlignment => "sequence_alignment",
            Self::KeyedRecordSet => "keyed_record_set",
            Self::AttributeProjection => "attribute_projection",
            Self::ResourceEnumeration => "resource_enumeration",
        }
    }
}

impl std::fmt::Display for EngineId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How an operation is carried out.
///
/// Note what is *not* here: neither variant carries a request or response
/// contract. Those live on [`MechanismOperation`], exactly once, so two nested
/// declarations can never contradict each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Executor {
    Engine {
        engine: EngineId,
        /// The engine implementation revision this contribution was authored
        /// against.
        engine_revision: u32,
        #[serde(default)]
        config: serde_json::Value,
    },
    Command {
        command: StructuredCommand,
    },
}

impl Executor {
    pub fn command(&self) -> Option<&StructuredCommand> {
        match self {
            Self::Command { command } => Some(command),
            Self::Engine { .. } => None,
        }
    }
}

/// One operation, with exactly one authoritative declaration of its contracts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MechanismOperation {
    pub request_contract: SchemaRef,
    pub response_contract: SchemaRef,
    /// Hard bound on the response Draft will accept. Anything larger is refused
    /// before it can enter Draft state.
    pub max_response_bytes: u64,
    pub executor: Executor,
}

/// Largest response bound a contribution may declare.
pub const MAX_DECLARABLE_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

impl MechanismOperation {
    pub fn validate(&self) -> FormatResult<()> {
        if self.max_response_bytes == 0 {
            return Err(FormatError::Identity(
                "a declared operation must allow a non-zero response".into(),
            ));
        }
        if self.max_response_bytes > MAX_DECLARABLE_RESPONSE_BYTES {
            return Err(FormatError::Limit(format!(
                "declared max_response_bytes {} exceeds the {MAX_DECLARABLE_RESPONSE_BYTES} byte ceiling",
                self.max_response_bytes
            )));
        }
        if let Some(command) = self.executor.command() {
            command.validate()?;
        }
        Ok(())
    }

    /// The permissions this operation needs before Draft may run it.
    pub fn required_permissions(&self) -> Vec<ExtensionPermission> {
        match self.executor {
            Executor::Command { .. } => vec![StructuredCommand::required_permission()],
            Executor::Engine { .. } => Vec::new(),
        }
    }

    /// Every schema this operation references.
    pub fn schema_refs(&self) -> Vec<&SchemaRef> {
        vec![&self.request_contract, &self.response_contract]
    }
}

// ---------------------------------------------------------------------------
// Resource description and selection
// ---------------------------------------------------------------------------

/// The intrinsic shape an observer saw, and typed intrinsic attribute values.
///
/// Both are re-exported from the portable DCG contract rather than redefined
/// here. They appear inside `ResourceState`, whose digest is part of Baseline
/// identity, so a second definition in this crate could drift and make the same
/// observed fact hash differently depending on which crate described it.
pub use draft_dcg_contract::{AttributeValue, ResourceForm};

/// How an attribute or locator string is matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "match", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttributeMatch {
    Equals {
        value: AttributeValue,
    },
    Prefix {
        value: String,
    },
    Suffix {
        value: String,
    },
    Glob {
        pattern: String,
    },
    Range {
        at_least: Option<i64>,
        at_most: Option<i64>,
    },
}

/// A predicate over the *intrinsic* facts an adapter observed.
///
/// This is what a classification selector may use, and it structurally has no
/// class variant: classification cannot predicate on the class it is trying to
/// produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "predicate", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawResourcePredicate {
    All {
        of: Vec<RawResourcePredicate>,
    },
    Any {
        of: Vec<RawResourcePredicate>,
    },
    Not {
        of: Box<RawResourcePredicate>,
    },
    LocatorScheme {
        equals: String,
    },
    /// A glob over the opaque locator body. Core never gives the body structure;
    /// the pattern is matched as a plain string.
    LocatorPattern {
        glob: String,
    },
    MediaType {
        equals: String,
    },
    Form {
        equals: ResourceForm,
    },
    Attribute {
        name: String,
        matches: AttributeMatch,
    },
    ContentSize {
        at_least: Option<u64>,
        at_most: Option<u64>,
    },
    /// Optional filesystem predicates. They are meaningful only for
    /// `file`-scheme locators and never match any other scheme.
    PathGlob {
        glob: String,
    },
    PathSuffix {
        suffix: String,
    },
}

/// A predicate usable downstream of classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "predicate", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourcePredicate {
    All {
        of: Vec<ResourcePredicate>,
    },
    Any {
        of: Vec<ResourcePredicate>,
    },
    Not {
        of: Box<ResourcePredicate>,
    },
    /// Resolved against the current classification bundle, never against a field
    /// embedded in authoritative resource state.
    HasClass {
        class_id: NamespacedId,
    },
    /// Any intrinsic predicate.
    Raw {
        of: RawResourcePredicate,
    },
}

/// A predicate over extracted elements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "predicate", rename_all = "snake_case", deny_unknown_fields)]
pub enum ElementPredicate {
    All {
        of: Vec<ElementPredicate>,
    },
    Any {
        of: Vec<ElementPredicate>,
    },
    Not {
        of: Box<ElementPredicate>,
    },
    KindIs {
        equals: NamespacedId,
    },
    AttributeIs {
        name: String,
        matches: AttributeMatch,
    },
    RelationExists {
        relation_kind: NamespacedId,
        direction: RelationDirection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationDirection {
    Outgoing,
    Incoming,
    Either,
}

// ---------------------------------------------------------------------------
// Contribution payloads
// ---------------------------------------------------------------------------

/// How a resource backend is supplied, declaratively.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAdapterContribution {
    /// The locator scheme this adapter owns. Never `file`, which Core provides.
    pub scheme: String,
    pub capabilities: AdapterCapabilities,
    pub enumerate: MechanismOperation,
    pub describe: MechanismOperation,
    pub content: MechanismOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutate: Option<MechanismOperation>,
    #[serde(default)]
    pub recovery: RecoveryContribution,
}

impl ResourceAdapterContribution {
    /// Whether this adapter needs `process.execute` to function.
    ///
    /// An engine-backed operation runs Draft's own code and needs no grant; a
    /// command-backed one launches a process and does. Every declared
    /// operation counts — including recovery capture and restore, which are
    /// separate authorized operations rather than a side effect of observing.
    pub fn requires_execution(&self) -> bool {
        self.operations()
            .into_iter()
            .any(|operation| operation.executor.command().is_some())
    }

    /// Every operation this adapter declares.
    pub fn operations(&self) -> Vec<&MechanismOperation> {
        let mut operations = vec![&self.enumerate, &self.describe, &self.content];
        operations.extend(self.mutate.as_ref());
        operations.extend(self.recovery.operations());
        operations
    }
}

/// What an adapter can actually promise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterCapabilities {
    pub observation_consistency: ObservationConsistency,
    #[serde(default)]
    pub supports_ranged_read: bool,
    #[serde(default)]
    pub supports_mutation: bool,
    /// Whether the adapter can assert a trustworthy stable external identity for
    /// a resource, which is what lets Draft prove continuity across a
    /// relocation it did not perform.
    #[serde(default)]
    pub asserts_external_identity: bool,
}

/// How strong an adapter's generation fencing actually is.
///
/// Declared rather than assumed, so Draft can revalidate digests around access
/// for adapters whose fencing token may alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationConsistency {
    /// A monotonic, stable generation primitive the adapter trusts on its own.
    StrongGeneration,
    /// A generation-like value that may alias; Draft revalidates the digest.
    BestEffortGeneration,
    /// No generation primitive at all; Draft revalidates before and after.
    DigestRevalidation,
}

/// What an adapter can restore, and how.
///
/// Capture and restore are declared together: a command-backed adapter cannot
/// produce anchors without a declared, schema-bound, authorized capture
/// operation, and permission to capture never implies permission to restore.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "capability", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryContribution {
    #[default]
    None,
    /// Content-plus-state anchors. `capture` may be omitted only when the
    /// adapter's declared content access already yields complete material.
    ContentRestore {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capture: Option<MechanismOperation>,
        restore: MechanismOperation,
    },
    VersionRestore {
        capture: MechanismOperation,
        restore: MechanismOperation,
    },
    AdapterManaged {
        capture: MechanismOperation,
        restore: MechanismOperation,
    },
}

impl RecoveryContribution {
    pub fn capture(&self) -> Option<&MechanismOperation> {
        match self {
            Self::None => None,
            Self::ContentRestore { capture, .. } => capture.as_ref(),
            Self::VersionRestore { capture, .. } | Self::AdapterManaged { capture, .. } => {
                Some(capture)
            }
        }
    }

    pub fn restore(&self) -> Option<&MechanismOperation> {
        match self {
            Self::None => None,
            Self::ContentRestore { restore, .. }
            | Self::VersionRestore { restore, .. }
            | Self::AdapterManaged { restore, .. } => Some(restore),
        }
    }

    /// Both declared operations, when they exist.
    ///
    /// Capture and restore are listed together because both are authorized
    /// operations in their own right: permission to take an anchor never
    /// implies permission to put one back.
    pub fn operations(&self) -> Vec<&MechanismOperation> {
        self.capture().into_iter().chain(self.restore()).collect()
    }
}

/// One class an extension assigns to the resources it recognizes.
///
/// Assignments compose: a resource may carry a text class and a language class
/// at once, and neither is "the" class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceClassificationRule {
    pub class_id: NamespacedId,
    pub display_name: String,
    /// Intrinsic facts only — classification cannot predicate on a class.
    pub applies_to: RawResourcePredicate,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<ClassAttribute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassAttribute {
    pub name: String,
    pub value: AttributeValue,
}

/// How change to a resource is explained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonContribution {
    pub strategy_id: NamespacedId,
    pub applies_to: ResourcePredicate,
    /// The schema the produced representation payload validates against.
    pub result_contract: SchemaRef,
    pub operation: MechanismOperation,
}

impl ComparisonContribution {
    /// Whether this contribution needs `process.execute` to function.
    ///
    /// An engine-backed contribution runs Draft's own code and needs no grant;
    /// a command-backed one launches a process and does.
    pub fn requires_execution(&self) -> bool {
        self.operation.executor.command().is_some()
    }
}

/// How the elements inside a resource are found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementExtractionContribution {
    pub extractor_id: NamespacedId,
    pub applies_to: ResourcePredicate,
    pub result_contract: SchemaRef,
    pub operation: MechanismOperation,
}

impl ElementExtractionContribution {
    /// Whether this contribution needs `process.execute` to function.
    ///
    /// An engine-backed contribution runs Draft's own code and needs no grant;
    /// a command-backed one launches a process and does.
    pub fn requires_execution(&self) -> bool {
        self.operation.executor.command().is_some()
    }
}

/// One independently named check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationCheck {
    pub check_id: NamespacedId,
    pub display_name: String,
    pub applies_to: ResourcePredicate,
    pub requirement: CheckRequirement,
    pub selection: CheckSelection,
    pub operation: MechanismOperation,
}

/// Whether a check's outcome can block acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckRequirement {
    Required,
    Optional,
}

/// When a check runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckSelection {
    /// Once per changed resource the predicate matches.
    PerResource,
    /// Once for the whole change set, if any resource matches.
    Whole,
    /// Randomized or exploratory checking, run only when asked for.
    Exploratory,
    /// A probe whose output identifies the environment, so an environment change
    /// deterministically invalidates cached verification.
    EnvironmentProbe,
}

/// A set of contributed verification checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationContribution {
    pub checks: Vec<VerificationCheck>,
}

/// A weighted rule contributing to risk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskRule {
    pub code: NamespacedId,
    pub weight: i32,
    pub when: RiskCondition,
    pub explanation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_action: Option<String>,
}

/// What a risk rule tests. Every variant is domain-neutral: the vocabulary a
/// domain cares about arrives as contributed predicates, metric keys and intent
/// identifiers that Draft never interprets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "condition", rename_all = "snake_case", deny_unknown_fields)]
pub enum RiskCondition {
    All {
        of: Vec<RiskCondition>,
    },
    Any {
        of: Vec<RiskCondition>,
    },
    Not {
        of: Box<RiskCondition>,
    },
    ResourcesMatching {
        predicate: ResourcePredicate,
        at_least: u32,
    },
    AspectCount {
        aspect: ChangeAspectName,
        at_least: u32,
    },
    ResourceCount {
        at_least: Option<u64>,
        at_most: Option<u64>,
    },
    /// Reads a contributed metric out of a representation summary. Draft does
    /// not know what the key means.
    ChangeMetric {
        metric: String,
        at_least: i64,
    },
    ElementsMatching {
        predicate: ElementPredicate,
        at_least: u32,
    },
    EvidenceState {
        verification: VerificationStateName,
    },
    IntentIs {
        intent: NamespacedId,
    },
    Provenance {
        imported: Option<bool>,
        agent_produced: Option<bool>,
    },
    /// Expressed in permille rather than as a float: a contract value that
    /// participates in a canonical hash must have exactly one byte form, and
    /// `0.3` does not.
    CandidateHistory {
        rollback_rate_permille_at_least: u32,
    },
    IdentityUncertain {
        at_least: u32,
    },
    ObservationGaps {
        at_least: u32,
    },
    DerivationGaps {
        at_least: u32,
    },
    RecoveryUnanchored {
        at_least: u32,
    },
    BoundaryViolation {
        at_least: u32,
    },
}

/// The neutral change aspects a risk rule may count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeAspectName {
    Added,
    Removed,
    ContentChanged,
    MetadataChanged,
    Relocated,
    FormChanged,
    AttributesChanged,
}

/// The verification states a risk rule may test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStateName {
    Passed,
    Failed,
    Unavailable,
    NotEvaluated,
    NotApplicable,
}

/// A contributed set of risk rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskRuleSet {
    pub rules: Vec<RiskRule>,
}

/// Project policy, split into the half that changes what Draft observes and the
/// half that changes what Draft requires before accepting a change.
///
/// The split is not cosmetic: view rules participate in the observation context
/// and therefore in snapshot identity, while control policy participates only in
/// the acceptance context and can never manufacture a project change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyPreset {
    #[serde(default)]
    pub view_rules: ViewRulePolicy,
    #[serde(default)]
    pub control_policy: ControlPolicy,
}

/// What is part of the observed universe at all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewRulePolicy {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclusions: Vec<ResourceRule>,
}

/// What Draft requires before a change may be accepted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlPolicy {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protections: Vec<ResourceRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_thresholds: Option<RiskThresholds>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewability_budget: Option<ReviewabilityBudget>,
    /// Intents whose packs must be verified more thoroughly before they can be
    /// accepted.
    ///
    /// Draft does not know that a `security` change deserves more scrutiny than
    /// a `docs` one — that is exactly the domain judgement the vocabulary's
    /// owner holds, so the same package that declares an intent declares what
    /// verifying it requires.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verification_escalations: Vec<VerificationEscalation>,
}

/// One intent, and the verification scope accepting it requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationEscalation {
    /// The intent this applies to. It must be one the declaring package's own
    /// `intent_vocabulary` declares.
    pub intent: NamespacedId,
    /// Require the full check set rather than the change-scoped selection.
    #[serde(default)]
    pub require_full: bool,
    /// Require the exploratory checks too.
    #[serde(default)]
    pub require_exploratory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRule {
    pub predicate: RawResourcePredicate,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskThresholds {
    pub medium: u32,
    pub high: u32,
    pub critical: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewabilityBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_resources: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_review_units: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub max_change_metrics: Vec<ChangeMetricBudget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeMetricBudget {
    pub metric: String,
    pub limit: i64,
}

/// One intent a domain defines. Draft stores and compares it; it never
/// interprets what a `bug-fix` or a `colour-grade` means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentDeclaration {
    pub intent_id: NamespacedId,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A contributed vocabulary of intents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentVocabulary {
    pub intents: Vec<IntentDeclaration>,
}

/// Which generic platform engine renders something, and how it is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationEngineId {
    MetadataSummary,
    ByteSummary,
    StructuredJson,
    TableTree,
    TextEditor,
    UnitChangeView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationSurface {
    ResourceView,
    ChangeView,
}

/// What a presentation applies to. Selection is by specificity — an exact schema
/// binding beats a class binding, which beats a predicate — and there is
/// deliberately no priority number an extension could use to capture the primary
/// human view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "binding", rename_all = "snake_case", deny_unknown_fields)]
pub enum PresentationBinding {
    ExactSchema { schema: SchemaRef },
    ResourceClass { class_id: NamespacedId },
    Predicate { of: ResourcePredicate },
}

impl PresentationBinding {
    /// Higher is more specific. Ties are ambiguous, never arbitrated.
    pub fn specificity(&self) -> u8 {
        match self {
            Self::ExactSchema { .. } => 3,
            Self::ResourceClass { .. } => 2,
            Self::Predicate { .. } => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresentationContribution {
    pub presentation_id: NamespacedId,
    pub surface: PresentationSurface,
    pub binding: PresentationBinding,
    pub engine: PresentationEngineId,
    #[serde(default)]
    pub config: serde_json::Value,
}

/// A transformation or inspection an extension offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolActionContribution {
    pub action_id: NamespacedId,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub applies_to: ResourcePredicate,
    pub operation: MechanismOperation,
    pub effect: ToolEffect,
}

/// What a tool action is allowed to do.
///
/// Note that neither variant lets the extension author an operation: a
/// transformation returns *proposed* effects, and Draft builds the
/// authority-bearing plan from them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolEffect {
    /// Produces a result document only.
    Inspect,
    /// May write into the operation's authorized output scope and may propose
    /// resource mutations.
    Transform {
        #[serde(default)]
        output_scope: OutputScope,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputScope {
    /// Bytes the operation may write into its output root.
    pub max_output_bytes: u64,
    /// Whether the tool may propose mutations at all, or only produce objects.
    #[serde(default)]
    pub may_propose_mutations: bool,
}

impl Default for OutputScope {
    fn default() -> Self {
        Self {
            max_output_bytes: 64 * 1024 * 1024,
            may_propose_mutations: false,
        }
    }
}

/// How a candidate runs.
///
/// A preset is pure data and can never itself cause execution: delegating to a
/// tool action is the only way to reach a process, and that action carries its
/// own permission and its own authorization decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "execution", rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateExecution {
    Manual,
    ToolAction { action_id: NamespacedId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidatePreset {
    pub preset_id: NamespacedId,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub execution: CandidateExecution,
    #[serde(default)]
    pub limits: CandidateLimits,
}

/// Neutral bounds on what a candidate may do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_resources_changed: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub max_change_metrics: Vec<ChangeMetricBudget>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbidden: Vec<ResourceRule>,
}

/// A contributed task template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTemplateContribution {
    pub template_id: NamespacedId,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<NamespacedId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_evidence: Vec<NamespacedId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_questions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success_criteria: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<TaskTemplateStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTemplateStep {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ResourcePredicate>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(name: &str) -> SchemaRef {
        SchemaRef::new(NamespacedId::parse(name).unwrap(), 1)
    }

    fn operation(executor: Executor) -> MechanismOperation {
        MechanismOperation {
            request_contract: schema("draft.core/request"),
            response_contract: schema("example.pub/response"),
            max_response_bytes: 1024,
            executor,
        }
    }

    #[test]
    fn permissions_round_trip_through_their_wire_names() {
        for permission in ExtensionPermission::ALL {
            assert_eq!(
                ExtensionPermission::parse(permission.as_str()).unwrap(),
                *permission
            );
            assert_eq!(
                serde_json::to_value(permission).unwrap(),
                serde_json::Value::String(permission.as_str().to_string()),
            );
        }
        assert!(ExtensionPermission::parse("filesystem.write").is_err());
    }

    #[test]
    fn a_declared_program_cannot_smuggle_a_command_line() {
        let shelled = StructuredCommand {
            program: "sh -c".into(),
            args: vec!["rm -rf /".into()],
            cwd: None,
            timeout_ms: None,
        };
        assert!(matches!(shelled.validate(), Err(FormatError::Identity(_))));

        let honest = StructuredCommand {
            program: "example-verify".into(),
            args: vec!["--all".into()],
            cwd: None,
            timeout_ms: Some(60_000),
        };
        honest.validate().unwrap();
        assert_eq!(honest.display(), "example-verify --all");
    }

    #[test]
    fn a_declared_working_directory_cannot_escape_the_runtime_scope() {
        let escaping = StructuredCommand {
            program: "example-verify".into(),
            args: vec![],
            cwd: Some("../../etc".into()),
            timeout_ms: None,
        };
        assert!(matches!(
            escaping.validate(),
            Err(FormatError::UnsafePath(_))
        ));
    }

    #[test]
    fn only_command_backed_operations_require_a_permission() {
        let engine = operation(Executor::Engine {
            engine: EngineId::WholeResource,
            engine_revision: 1,
            config: json!({}),
        });
        assert!(engine.required_permissions().is_empty());

        let command = operation(Executor::Command {
            command: StructuredCommand {
                program: "example-verify".into(),
                args: vec![],
                cwd: None,
                timeout_ms: None,
            },
        });
        assert_eq!(
            command.required_permissions(),
            vec![ExtensionPermission::ProcessExecute]
        );
    }

    #[test]
    fn one_operation_owns_its_contracts_exactly_once() {
        // The executor carries no contract of its own, so there is no second
        // place a contradictory request or response schema could be declared.
        let encoded = serde_json::to_value(Executor::Command {
            command: StructuredCommand {
                program: "p".into(),
                args: vec![],
                cwd: None,
                timeout_ms: None,
            },
        })
        .unwrap();
        let keys: Vec<&str> = encoded
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(keys, ["command", "kind"]);
        assert!(!keys.contains(&"request_contract"));
        assert!(!keys.contains(&"response_contract"));
        assert!(!keys.contains(&"max_response_bytes"));
    }

    #[test]
    fn a_response_bound_is_required_and_capped() {
        let mut op = operation(Executor::Engine {
            engine: EngineId::WholeResource,
            engine_revision: 1,
            config: json!({}),
        });
        op.validate().unwrap();

        op.max_response_bytes = 0;
        assert!(matches!(op.validate(), Err(FormatError::Identity(_))));

        op.max_response_bytes = MAX_DECLARABLE_RESPONSE_BYTES + 1;
        assert!(matches!(op.validate(), Err(FormatError::Limit(_))));
    }

    #[test]
    fn a_classification_selector_cannot_reach_a_class() {
        // Structural, not a runtime check: RawResourcePredicate has no variant
        // that could name a class, so a self-referential selector is
        // unrepresentable rather than merely rejected.
        let encoded = serde_json::to_string(&RawResourcePredicate::LocatorScheme {
            equals: "file".into(),
        })
        .unwrap();
        assert!(!encoded.contains("has_class"));
        assert!(serde_json::from_str::<RawResourcePredicate>(
            r#"{"predicate":"has_class","class_id":"a.b/c"}"#
        )
        .is_err());
    }

    #[test]
    fn presentation_specificity_is_ordered_and_has_no_priority_field() {
        let exact = PresentationBinding::ExactSchema {
            schema: schema("example.pub/rep"),
        };
        let class = PresentationBinding::ResourceClass {
            class_id: NamespacedId::parse("example.pub/kind").unwrap(),
        };
        let predicate = PresentationBinding::Predicate {
            of: ResourcePredicate::Raw {
                of: RawResourcePredicate::LocatorScheme {
                    equals: "file".into(),
                },
            },
        };
        assert!(exact.specificity() > class.specificity());
        assert!(class.specificity() > predicate.specificity());

        let contribution = PresentationContribution {
            presentation_id: NamespacedId::parse("example.pub/view").unwrap(),
            surface: PresentationSurface::ChangeView,
            binding: exact,
            engine: PresentationEngineId::UnitChangeView,
            config: json!({}),
        };
        let encoded = serde_json::to_value(&contribution).unwrap();
        assert!(encoded.get("priority").is_none());
    }

    #[test]
    fn recovery_pairs_capture_with_restore() {
        let restore = operation(Executor::Command {
            command: StructuredCommand {
                program: "example-restore".into(),
                args: vec![],
                cwd: None,
                timeout_ms: None,
            },
        });
        let capture = operation(Executor::Command {
            command: StructuredCommand {
                program: "example-capture".into(),
                args: vec![],
                cwd: None,
                timeout_ms: None,
            },
        });
        let managed = RecoveryContribution::AdapterManaged {
            capture: capture.clone(),
            restore: restore.clone(),
        };
        assert!(managed.capture().is_some() && managed.restore().is_some());
        assert_eq!(managed.operations().len(), 2);

        // Both halves are command-backed, so both need the permission — and they
        // are separate operations, so authorizing one does not authorize the
        // other.
        for op in managed.operations() {
            assert_eq!(
                op.required_permissions(),
                vec![ExtensionPermission::ProcessExecute]
            );
        }
        assert!(RecoveryContribution::None.capture().is_none());
    }

    #[test]
    fn a_candidate_preset_carries_no_command_of_its_own() {
        let preset = CandidatePreset {
            preset_id: NamespacedId::parse("example.agent/fast").unwrap(),
            display_name: "Fast".into(),
            description: None,
            execution: CandidateExecution::ToolAction {
                action_id: NamespacedId::parse("example.agent/run").unwrap(),
            },
            limits: CandidateLimits::default(),
        };
        let encoded = serde_json::to_value(&preset).unwrap();
        // Delegation names an action; it cannot inline a program.
        assert!(encoded.get("command").is_none());
        assert!(serde_json::to_string(&encoded)
            .unwrap()
            .contains("tool_action"));
    }
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// The decoded body of one contribution.
///
/// The variant is chosen by the contribution's declared kind, never by sniffing
/// the file, so a package cannot supply a payload of one kind under the
/// declaration of another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContributionPayload {
    ResourceAdapter(Box<ResourceAdapterContribution>),
    ResourceClassification(ResourceClassificationRule),
    Comparison(Box<ComparisonContribution>),
    ElementExtraction(Box<ElementExtractionContribution>),
    Presentation(PresentationContribution),
    ToolAction(Box<ToolActionContribution>),
    Verification(Box<VerificationContribution>),
    RiskRules(RiskRuleSet),
    PolicyPreset(PolicyPreset),
    IntentVocabulary(IntentVocabulary),
    TaskTemplate(Box<TaskTemplateContribution>),
    CandidatePreset(Box<CandidatePreset>),
    /// Documentation is consumed as an opaque validated record by the subsystem
    /// that owns it, and is metadata-only by design.
    Documentation(serde_json::Value),
}

impl ContributionPayload {
    /// Decode `bytes` as the payload for `kind`.
    pub fn decode(kind: crate::ExtensionContributionKind, bytes: &[u8]) -> FormatResult<Self> {
        use crate::ExtensionContributionKind as Kind;
        fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8], what: &str) -> FormatResult<T> {
            serde_json::from_slice(bytes)
                .map_err(|error| FormatError::Encoding(format!("invalid {what} payload: {error}")))
        }
        Ok(match kind {
            Kind::ResourceAdapter => Self::ResourceAdapter(parse(bytes, "resource adapter")?),
            Kind::ResourceClassification => {
                Self::ResourceClassification(parse(bytes, "resource classification")?)
            }
            Kind::Comparison => Self::Comparison(parse(bytes, "comparison")?),
            Kind::ElementExtraction => Self::ElementExtraction(parse(bytes, "element extraction")?),
            Kind::Presentation => Self::Presentation(parse(bytes, "presentation")?),
            Kind::ToolAction => Self::ToolAction(parse(bytes, "tool action")?),
            Kind::Verification => Self::Verification(parse(bytes, "verification")?),
            Kind::RiskRule => Self::RiskRules(parse(bytes, "risk rules")?),
            Kind::PolicyPreset => Self::PolicyPreset(parse(bytes, "policy preset")?),
            Kind::IntentVocabulary => Self::IntentVocabulary(parse(bytes, "intent vocabulary")?),
            Kind::TaskTemplate => Self::TaskTemplate(parse(bytes, "task template")?),
            Kind::CandidatePreset => Self::CandidatePreset(parse(bytes, "candidate preset")?),
            Kind::Documentation => Self::Documentation(parse(bytes, "documentation")?),
        })
    }

    /// Every schema-bound operation this payload declares.
    pub fn operations(&self) -> Vec<&MechanismOperation> {
        match self {
            Self::ResourceAdapter(adapter) => {
                let mut operations = vec![&adapter.enumerate, &adapter.describe, &adapter.content];
                operations.extend(adapter.mutate.as_ref());
                operations.extend(adapter.recovery.operations());
                operations
            }
            Self::Comparison(comparison) => vec![&comparison.operation],
            Self::ElementExtraction(extraction) => vec![&extraction.operation],
            Self::ToolAction(action) => vec![&action.operation],
            Self::Verification(verification) => verification
                .checks
                .iter()
                .map(|check| &check.operation)
                .collect(),
            Self::ResourceClassification(_)
            | Self::Presentation(_)
            | Self::RiskRules(_)
            | Self::PolicyPreset(_)
            | Self::IntentVocabulary(_)
            | Self::TaskTemplate(_)
            | Self::CandidatePreset(_)
            | Self::Documentation(_) => Vec::new(),
        }
    }

    /// Every command this payload would have Draft run.
    pub fn commands(&self) -> Vec<&StructuredCommand> {
        self.operations()
            .into_iter()
            .filter_map(|operation| operation.executor.command())
            .collect()
    }

    /// Every schema this payload references, so the manifest can be checked for
    /// declaring each one.
    pub fn schema_refs(&self) -> Vec<&SchemaRef> {
        let mut refs: Vec<&SchemaRef> = self
            .operations()
            .into_iter()
            .flat_map(MechanismOperation::schema_refs)
            .collect();
        match self {
            Self::Comparison(comparison) => refs.push(&comparison.result_contract),
            Self::ElementExtraction(extraction) => refs.push(&extraction.result_contract),
            Self::Presentation(presentation) => {
                if let PresentationBinding::ExactSchema { schema } = &presentation.binding {
                    refs.push(schema);
                }
            }
            _ => {}
        }
        refs
    }

    /// The permissions Draft needs before it may act on all of this payload.
    pub fn required_permissions(&self) -> Vec<ExtensionPermission> {
        let mut permissions: Vec<ExtensionPermission> = self
            .operations()
            .into_iter()
            .flat_map(MechanismOperation::required_permissions)
            .collect();
        permissions.sort();
        permissions.dedup();
        permissions
    }

    /// Every contributed identifier this payload mints, so the manifest can
    /// confirm the declaring extension owns each one.
    pub fn contributed_ids(&self) -> Vec<&NamespacedId> {
        match self {
            Self::ResourceClassification(rule) => vec![&rule.class_id],
            Self::Comparison(comparison) => vec![&comparison.strategy_id],
            Self::ElementExtraction(extraction) => vec![&extraction.extractor_id],
            Self::Presentation(presentation) => vec![&presentation.presentation_id],
            Self::ToolAction(action) => vec![&action.action_id],
            Self::Verification(verification) => verification
                .checks
                .iter()
                .map(|check| &check.check_id)
                .collect(),
            Self::RiskRules(rules) => rules.rules.iter().map(|rule| &rule.code).collect(),
            Self::IntentVocabulary(vocabulary) => vocabulary
                .intents
                .iter()
                .map(|intent| &intent.intent_id)
                .collect(),
            Self::TaskTemplate(template) => vec![&template.template_id],
            Self::CandidatePreset(preset) => vec![&preset.preset_id],
            Self::ResourceAdapter(_) | Self::PolicyPreset(_) | Self::Documentation(_) => Vec::new(),
        }
    }

    /// Validate the payload's own rules.
    pub fn validate(&self) -> FormatResult<()> {
        for operation in self.operations() {
            operation.validate()?;
        }
        match self {
            Self::ResourceAdapter(adapter) => {
                crate::identifier::validate_segment(
                    &adapter.scheme,
                    IdentifierClass::Restricted,
                    "adapter locator scheme",
                )?;
                if adapter.scheme == "file" {
                    return Err(FormatError::Identity(
                        "the 'file' locator scheme is provided by Draft and cannot be contributed"
                            .into(),
                    ));
                }
                if adapter.capabilities.supports_mutation != adapter.mutate.is_some() {
                    return Err(FormatError::Identity(
                        "a resource adapter must declare a mutate operation exactly when it claims mutation support"
                            .into(),
                    ));
                }
                // A declared restore with no way to produce the anchors it would
                // consume is not a recovery capability, it is a dead end.
                match &adapter.recovery {
                    RecoveryContribution::None => {}
                    RecoveryContribution::ContentRestore { .. } => {}
                    RecoveryContribution::VersionRestore { .. }
                    | RecoveryContribution::AdapterManaged { .. } => {
                        if adapter.recovery.capture().is_none() {
                            return Err(FormatError::Identity(
                                "this recovery capability requires a declared capture operation"
                                    .into(),
                            ));
                        }
                    }
                }
            }
            Self::Verification(verification) => {
                if verification.checks.is_empty() {
                    return Err(FormatError::Identity(
                        "a verification contribution must declare at least one check".into(),
                    ));
                }
                let mut seen = std::collections::BTreeSet::new();
                for check in &verification.checks {
                    if !seen.insert(&check.check_id) {
                        return Err(FormatError::Identity(format!(
                            "verification check '{}' is declared more than once",
                            check.check_id
                        )));
                    }
                }
            }
            Self::RiskRules(rules) if rules.rules.is_empty() => {
                return Err(FormatError::Identity(
                    "a risk contribution must declare at least one rule".into(),
                ));
            }
            Self::IntentVocabulary(vocabulary) if vocabulary.intents.is_empty() => {
                return Err(FormatError::Identity(
                    "an intent vocabulary must declare at least one intent".into(),
                ));
            }
            Self::PolicyPreset(preset) => {
                let mut seen = std::collections::BTreeSet::new();
                for escalation in &preset.control_policy.verification_escalations {
                    if !escalation.require_full && !escalation.require_exploratory {
                        return Err(FormatError::Identity(format!(
                            "the verification escalation for '{}' requires nothing; \
                             remove it rather than declaring a rule that cannot fire",
                            escalation.intent
                        )));
                    }
                    if !seen.insert(&escalation.intent) {
                        return Err(FormatError::Identity(format!(
                            "intent '{}' has more than one verification escalation",
                            escalation.intent
                        )));
                    }
                }
            }
            Self::ToolAction(action) => {
                if let ToolEffect::Transform { output_scope } = &action.effect {
                    if output_scope.max_output_bytes == 0 {
                        return Err(FormatError::Identity(
                            "a transforming tool action must allow a non-zero output".into(),
                        ));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod payload_tests {
    use super::*;
    use crate::ExtensionContributionKind;

    #[test]
    fn payloads_decode_by_declared_kind_and_never_by_sniffing() {
        let classification = br#"{
            "class_id": "example.pub/document",
            "display_name": "Document",
            "applies_to": {"predicate": "media_type", "equals": "text/plain"}
        }"#;
        let payload = ContributionPayload::decode(
            ExtensionContributionKind::ResourceClassification,
            classification,
        )
        .unwrap();
        payload.validate().unwrap();
        assert!(matches!(
            payload,
            ContributionPayload::ResourceClassification(_)
        ));
        assert_eq!(
            payload.contributed_ids()[0].qualified(),
            "example.pub/document"
        );

        // The same bytes declared as a different kind are refused rather than
        // reinterpreted.
        assert!(matches!(
            ContributionPayload::decode(ExtensionContributionKind::Comparison, classification),
            Err(FormatError::Encoding(_))
        ));
    }

    #[test]
    fn a_contributed_scheme_cannot_shadow_the_built_in_one() {
        let adapter = ResourceAdapterContribution {
            scheme: "file".into(),
            capabilities: AdapterCapabilities {
                observation_consistency: ObservationConsistency::DigestRevalidation,
                supports_ranged_read: false,
                supports_mutation: false,
                asserts_external_identity: false,
            },
            enumerate: op(),
            describe: op(),
            content: op(),
            mutate: None,
            recovery: RecoveryContribution::None,
        };
        assert!(matches!(
            ContributionPayload::ResourceAdapter(Box::new(adapter)).validate(),
            Err(FormatError::Identity(_))
        ));
    }

    #[test]
    fn a_restore_capability_requires_a_capture_operation() {
        let mut adapter = adapter();
        adapter.recovery = RecoveryContribution::VersionRestore {
            capture: op(),
            restore: op(),
        };
        ContributionPayload::ResourceAdapter(Box::new(adapter.clone()))
            .validate()
            .unwrap();

        // `AdapterManaged` without a capture leaves Draft able to restore
        // anchors it has no way to produce.
        adapter.recovery = RecoveryContribution::ContentRestore {
            capture: None,
            restore: op(),
        };
        ContributionPayload::ResourceAdapter(Box::new(adapter))
            .validate()
            .expect("content restore may rely on declared content access");
    }

    #[test]
    fn mutation_support_and_the_mutate_operation_must_agree() {
        let mut adapter = adapter();
        adapter.capabilities.supports_mutation = true;
        assert!(matches!(
            ContributionPayload::ResourceAdapter(Box::new(adapter)).validate(),
            Err(FormatError::Identity(_))
        ));
    }

    #[test]
    fn command_backed_payloads_report_the_permission_they_need() {
        let mut adapter = adapter();
        adapter.enumerate.executor = Executor::Command {
            command: StructuredCommand {
                program: "example-enumerate".into(),
                args: vec![],
                cwd: None,
                timeout_ms: None,
            },
        };
        let payload = ContributionPayload::ResourceAdapter(Box::new(adapter));
        assert_eq!(
            payload.required_permissions(),
            vec![ExtensionPermission::ProcessExecute]
        );
        assert_eq!(payload.commands().len(), 1);
    }

    fn op() -> MechanismOperation {
        MechanismOperation {
            request_contract: SchemaRef::new(NamespacedId::parse("draft.core/request").unwrap(), 1),
            response_contract: SchemaRef::new(
                NamespacedId::parse("example.pub/response").unwrap(),
                1,
            ),
            max_response_bytes: 4096,
            executor: Executor::Engine {
                engine: EngineId::ResourceEnumeration,
                engine_revision: 1,
                config: serde_json::json!({}),
            },
        }
    }

    fn adapter() -> ResourceAdapterContribution {
        ResourceAdapterContribution {
            scheme: "example".into(),
            capabilities: AdapterCapabilities {
                observation_consistency: ObservationConsistency::DigestRevalidation,
                supports_ranged_read: false,
                supports_mutation: false,
                asserts_external_identity: false,
            },
            enumerate: op(),
            describe: op(),
            content: op(),
            mutate: None,
            recovery: RecoveryContribution::None,
        }
    }
}
