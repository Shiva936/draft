//! What installed extensions currently contribute, and what is missing.
//!
//! Draft Core reads contributed rules through one port. With nothing installed
//! the port yields nothing and Draft behaves as a generic platform: commands
//! still run, evidence is still written, and the absence of domain knowledge is
//! reported rather than hidden.
//!
//! Core deliberately learns extension identities only at runtime, from the
//! port. It has no compile-time knowledge of any particular extension, and a
//! [`CapabilityGap`] carries none either — naming an installable package is the
//! job of the layer that can search a catalog.
//!
//! Nothing here interprets a locator body, a class name, a coordinate space or a
//! check id. Predicates are evaluated against the intrinsic facts an adapter
//! observed; contributed identifiers are compared for equality and stored.

use draft_extension_contract::{
    CandidatePreset, ComparisonContribution, ElementExtractionContribution,
    ExtensionContributionKind, ExtensionPermission, IntentVocabulary, NamespacedId, PolicyPreset,
    PresentationContribution, ResourceAdapterContribution, ResourceClassificationRule,
    ResourcePredicate, RiskRuleSet, TaskTemplateContribution, VerificationContribution,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The contribution kinds this Draft build consumes.
///
/// The portable format defines the whole vocabulary an extension author may
/// write. This is the narrower question of what *this* platform does something
/// with — and it is the list installation is checked against, so a package can
/// never be installed, enabled and reported healthy while one of its declared
/// contributions is quietly ignored.
///
/// Every kind is here. `Documentation` is consumed as metadata only, and says so
/// through [`ExtensionContributionKind::is_metadata_only`] rather than by
/// silently going unread.
pub const SUPPORTED_CONTRIBUTION_KINDS: &[ExtensionContributionKind] =
    ExtensionContributionKind::ALL;

/// Whether this build has a subsystem that consumes `kind`.
pub fn supports_contribution(kind: ExtensionContributionKind) -> bool {
    SUPPORTED_CONTRIBUTION_KINDS.contains(&kind)
}

/// A kind of domain knowledge an extension can supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionCapabilityKind {
    ResourceAdapter,
    Classification,
    Comparison,
    ElementExtraction,
    Presentation,
    ToolAction,
    Verification,
    Risk,
    Policy,
    IntentVocabulary,
    TaskTemplate,
    CandidatePreset,
    Documentation,
}

impl ExtensionCapabilityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ResourceAdapter => "resource_adapter",
            Self::Classification => "classification",
            Self::Comparison => "comparison",
            Self::ElementExtraction => "element_extraction",
            Self::Presentation => "presentation",
            Self::ToolAction => "tool_action",
            Self::Verification => "verification",
            Self::Risk => "risk",
            Self::Policy => "policy",
            Self::IntentVocabulary => "intent_vocabulary",
            Self::TaskTemplate => "task_template",
            Self::CandidatePreset => "candidate_preset",
            Self::Documentation => "documentation",
        }
    }

    /// The capability a contribution of `kind` supplies.
    pub fn of_contribution(kind: ExtensionContributionKind) -> Self {
        use ExtensionContributionKind as Contribution;
        match kind {
            Contribution::ResourceAdapter => Self::ResourceAdapter,
            Contribution::ResourceClassification => Self::Classification,
            Contribution::Comparison => Self::Comparison,
            Contribution::ElementExtraction => Self::ElementExtraction,
            Contribution::Presentation => Self::Presentation,
            Contribution::ToolAction => Self::ToolAction,
            Contribution::Verification => Self::Verification,
            Contribution::RiskRule => Self::Risk,
            Contribution::PolicyPreset => Self::Policy,
            Contribution::IntentVocabulary => Self::IntentVocabulary,
            Contribution::TaskTemplate => Self::TaskTemplate,
            Contribution::CandidatePreset => Self::CandidatePreset,
            Contribution::Documentation => Self::Documentation,
        }
    }
}

impl std::fmt::Display for ExtensionCapabilityKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Domain knowledge Draft needed and did not have.
///
/// It names the capability and the subjects in play, and nothing else. There is
/// deliberately no field for an extension id, a source or a package: Core cannot
/// know which package would fill a gap, and inventing one here would put catalog
/// knowledge inside the domain model. Higher layers enrich this with installable
/// suggestions when they can, and Draft works without them when they cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityGap {
    pub capability: ExtensionCapabilityKind,
    /// The resource classes Draft could not act on. Empty when the subject was
    /// never classified at all, which is a different fact from "classified and
    /// uncovered" and is reported as such.
    pub resource_classes: Vec<String>,
    pub reason: String,
}

impl CapabilityGap {
    pub fn new(
        capability: ExtensionCapabilityKind,
        resource_classes: impl IntoIterator<Item = String>,
        reason: impl Into<String>,
    ) -> Self {
        let mut resource_classes: Vec<String> = resource_classes.into_iter().collect();
        resource_classes.sort();
        resource_classes.dedup();
        Self {
            capability,
            resource_classes,
            reason: reason.into(),
        }
    }

    /// A stable identity for this gap.
    ///
    /// Derived from what the gap *is* — the capability and the classes it covers
    /// — so the same shortfall carries the same id across reads, and a frontend
    /// can be handed a remediation keyed to it instead of matching on rendered
    /// text. Deliberately not derived from `reason`, which is prose and may be
    /// reworded.
    pub fn gap_id(&self) -> String {
        gap_identity(
            "capability",
            self.capability,
            &self.resource_classes.join(","),
        )
    }
}

/// The shared identity rule for gaps and withheld capabilities.
fn gap_identity(prefix: &str, capability: ExtensionCapabilityKind, subject: &str) -> String {
    let digest = crate::support::hashing::sha256_hex(
        format!("{prefix}:{}:{subject}", capability.as_str()).as_bytes(),
    );
    format!("gap_{}", &digest[..16])
}

/// A capability an installed extension declares but is not authorized to use.
///
/// Unlike a [`CapabilityGap`], this *does* name the extension: the package is
/// installed, so its identity is a runtime fact Draft already holds, and the
/// user needs it to act. Nothing here is a compile-time dependency on any
/// particular extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithheldCapability {
    pub extension_id: String,
    pub capability: ExtensionCapabilityKind,
    pub missing_permissions: Vec<ExtensionPermission>,
}

impl WithheldCapability {
    /// A stable identity for this shortfall, keyed to the extension holding it.
    pub fn gap_id(&self) -> String {
        gap_identity("withheld", self.capability, &self.extension_id)
    }
}

/// One contributed rule together with the extension that supplied it.
///
/// Provenance travels with the rule because resolution needs it: when several
/// extensions cover the same subject, Draft has to say which ones agreed, and
/// when they disagree it has to name the candidates rather than pick one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contributed<T> {
    pub extension_id: String,
    pub value: T,
}

impl<T> Contributed<T> {
    pub fn new(extension_id: impl Into<String>, value: T) -> Self {
        Self {
            extension_id: extension_id.into(),
            value,
        }
    }
}

/// What resolving a contributed rule for one subject produced.
///
/// Draft never resolves by iteration order. Contributions that mean the same
/// thing coalesce into one answer carrying every contributor; contributions that
/// mean different things stay unresolved, because silently preferring one
/// publisher over another is exactly the failure this type exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution<'a, T> {
    /// Nothing installed covers this subject.
    NoMatch,
    /// Exactly one semantic answer, contributed by one or more extensions.
    Resolved {
        value: &'a T,
        /// Every contributing extension id, sorted. More than one means they
        /// agreed, not that one was chosen.
        contributors: Vec<&'a str>,
    },
    /// Several incompatible answers. Draft acts on none of them.
    Ambiguous { candidates: Vec<&'a Contributed<T>> },
}

impl<'a, T> Resolution<'a, T> {
    /// The agreed value, or `None` when nothing matched *or* the answer is
    /// ambiguous. A caller that cannot distinguish those cases must not act.
    pub fn value(&self) -> Option<&'a T> {
        match self {
            Self::Resolved { value, .. } => Some(value),
            _ => None,
        }
    }

    pub fn is_ambiguous(&self) -> bool {
        matches!(self, Self::Ambiguous { .. })
    }

    /// The extension ids behind this outcome, sorted; empty for `NoMatch`.
    pub fn contributors(&self) -> Vec<&'a str> {
        match self {
            Self::NoMatch => Vec::new(),
            Self::Resolved { contributors, .. } => contributors.clone(),
            Self::Ambiguous { candidates } => {
                candidates.iter().map(|c| c.extension_id.as_str()).collect()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Predicate evaluation
// ---------------------------------------------------------------------------

// The predicate evaluator and the view it reads now live in `support`, below
// both this layer and `project` — a project's protections must not depend on
// what is installed. Re-exported so contributed-rule callers read naturally.
pub use crate::support::predicate::{matches_raw, ResourceView, FILE_SCHEME};

/// Whether a presentation binding covers this resource.
///
/// An `ExactSchema` binding names a representation schema, which a raw resource
/// view cannot answer for — Draft does not guess. Such a binding is selected
/// only where the caller resolves against a known representation, so here it
/// never matches.
fn presentation_applies(
    binding: &draft_extension_contract::PresentationBinding,
    resource: &ResourceView<'_>,
    classes: &BTreeSet<NamespacedId>,
) -> bool {
    use draft_extension_contract::PresentationBinding as Binding;
    match binding {
        Binding::ExactSchema { .. } => false,
        Binding::ResourceClass { class_id } => classes.contains(class_id),
        Binding::Predicate { of } => matches(of, resource, classes),
    }
}

/// Evaluate a downstream predicate, resolving classes against the assignments
/// derived for this resource.
pub fn matches(
    predicate: &ResourcePredicate,
    resource: &ResourceView<'_>,
    classes: &BTreeSet<NamespacedId>,
) -> bool {
    match predicate {
        ResourcePredicate::All { of } => of.iter().all(|p| matches(p, resource, classes)),
        ResourcePredicate::Any { of } => of.iter().any(|p| matches(p, resource, classes)),
        ResourcePredicate::Not { of } => !matches(of, resource, classes),
        ResourcePredicate::HasClass { class_id } => classes.contains(class_id),
        ResourcePredicate::Raw { of } => matches_raw(of, resource),
    }
}
// ---------------------------------------------------------------------------
// The port
// ---------------------------------------------------------------------------

/// Everything installed, enabled and authorized extensions currently
/// contribute.
///
/// Command-bearing contributions arrive with their operations already withheld
/// when the artifact is not authorized to run them, so Core never holds an
/// operation it may not perform. What was withheld is reported in `withheld`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveContributions {
    #[serde(default)]
    pub resource_adapters: Vec<Contributed<ResourceAdapterContribution>>,
    #[serde(default)]
    pub classifications: Vec<Contributed<ResourceClassificationRule>>,
    #[serde(default)]
    pub comparisons: Vec<Contributed<ComparisonContribution>>,
    #[serde(default)]
    pub element_extractors: Vec<Contributed<ElementExtractionContribution>>,
    #[serde(default)]
    pub presentations: Vec<Contributed<PresentationContribution>>,
    #[serde(default)]
    pub tool_actions: Vec<Contributed<draft_extension_contract::ToolActionContribution>>,
    #[serde(default)]
    pub verifications: Vec<Contributed<VerificationContribution>>,
    #[serde(default)]
    pub risk_rules: Vec<Contributed<RiskRuleSet>>,
    #[serde(default)]
    pub policies: Vec<Contributed<PolicyPreset>>,
    #[serde(default)]
    pub intent_vocabularies: Vec<Contributed<IntentVocabulary>>,
    #[serde(default)]
    pub task_templates: Vec<Contributed<TaskTemplateContribution>>,
    #[serde(default)]
    pub candidate_presets: Vec<Contributed<CandidatePreset>>,
    #[serde(default)]
    pub withheld: Vec<WithheldCapability>,
    /// The artifact each contributing extension was accepted under, keyed by
    /// extension id.
    ///
    /// Carried alongside the contributions so every derived result can name the
    /// exact artifact that produced it — after that extension has been updated
    /// or removed, and without Core going back to the installation layer to ask.
    #[serde(default)]
    pub attestations: BTreeMap<String, super::provenance::ProducerRef>,
}

impl ActiveContributions {
    /// Whether any extension contributes anything at all.
    pub fn is_empty(&self) -> bool {
        self.resource_adapters.is_empty()
            && self.classifications.is_empty()
            && self.comparisons.is_empty()
            && self.element_extractors.is_empty()
            && self.presentations.is_empty()
            && self.tool_actions.is_empty()
            && self.verifications.is_empty()
            && self.risk_rules.is_empty()
            && self.policies.is_empty()
            && self.intent_vocabularies.is_empty()
            && self.task_templates.is_empty()
            && self.candidate_presets.is_empty()
    }

    /// Every class assigned to a resource, by every extension that recognizes
    /// it.
    ///
    /// Classification is a keyed union: a resource is legitimately both a text
    /// document and a language source, and neither assignment makes the other
    /// ambiguous. Only two incompatible definitions of the *same* class collide,
    /// and that collision is scoped to that class alone.
    pub fn classes_for(&self, resource: &ResourceView<'_>) -> ClassificationOutcome {
        let applicable: Vec<&Contributed<ResourceClassificationRule>> = self
            .classifications
            .iter()
            .filter(|rule| matches_raw(&rule.value.applies_to, resource))
            .collect();
        let union = super::algebra::union_by_key(
            applicable,
            |rule| rule.class_id.clone(),
            // Two rules agree when they assign the same class with the same
            // attributes. A differing display name is presentation, not
            // disagreement about anything Draft acts on.
            |left, right| left.class_id == right.class_id && left.attributes == right.attributes,
        );
        let mut assigned = BTreeSet::new();
        let mut collisions = Vec::new();
        for (class_id, entry) in union {
            match entry {
                super::algebra::KeyedEntry::Resolved { .. } => {
                    assigned.insert(class_id);
                }
                super::algebra::KeyedEntry::Collision { candidates } => {
                    collisions.push(ClassCollision {
                        class_id,
                        contributors: candidates
                            .iter()
                            .map(|candidate| candidate.extension_id.clone())
                            .collect(),
                    });
                }
            }
        }
        ClassificationOutcome {
            assigned,
            collisions,
        }
    }

    /// The comparison strategy for a resource, when exactly one is agreed.
    ///
    /// Unique resolution: a resource has one primary explanation of how it
    /// changed, so two extensions offering different strategies is an ambiguity
    /// Draft reports rather than arbitrates.
    pub fn comparison_for(
        &self,
        resource: &ResourceView<'_>,
        classes: &BTreeSet<NamespacedId>,
    ) -> Resolution<'_, ComparisonContribution> {
        super::algebra::resolve_unique(
            self.comparisons
                .iter()
                .filter(|contribution| matches(&contribution.value.applies_to, resource, classes))
                .collect(),
            |left, right| left == right,
        )
    }

    /// The adapter owning a locator scheme, when exactly one claims it.
    pub fn adapter_for_scheme(&self, scheme: &str) -> Resolution<'_, ResourceAdapterContribution> {
        super::algebra::resolve_unique(
            self.resource_adapters
                .iter()
                .filter(|contribution| contribution.value.scheme == scheme)
                .collect(),
            |left, right| left == right,
        )
    }

    /// How to present a resource or a change, when something claims to know.
    ///
    /// Selection is by declared specificity — an exact schema binding beats a
    /// class binding, which beats a predicate — and a tie within the winning
    /// tier is `Ambiguous` rather than arbitrated. `NoMatch` is not a failure:
    /// the caller renders the universal neutral fallback, which is always
    /// available and never contributed.
    pub fn presentation_for(
        &self,
        surface: draft_extension_contract::PresentationSurface,
        resource: &ResourceView<'_>,
        classes: &BTreeSet<NamespacedId>,
    ) -> Resolution<'_, PresentationContribution> {
        super::algebra::resolve_by_specificity(
            self.presentations
                .iter()
                .filter(|contribution| contribution.value.surface == surface)
                .filter(|contribution| {
                    presentation_applies(&contribution.value.binding, resource, classes)
                })
                .collect(),
            |contribution| contribution.binding.specificity(),
            |left, right| left.engine == right.engine && left.config == right.config,
        )
    }

    /// Every extractor covering a resource, keyed by its namespaced id.
    ///
    /// Complementary extractors compose: one may find structural elements while
    /// another finds references, and both results are kept.
    pub fn extractors_for(
        &self,
        resource: &ResourceView<'_>,
        classes: &BTreeSet<NamespacedId>,
    ) -> BTreeMap<NamespacedId, super::algebra::KeyedEntry<'_, ElementExtractionContribution>> {
        super::algebra::union_by_key(
            self.element_extractors
                .iter()
                .filter(|contribution| matches(&contribution.value.applies_to, resource, classes)),
            |contribution| contribution.extractor_id.clone(),
            |left, right| left == right,
        )
    }

    /// Every applicable check, keyed by its namespaced id.
    ///
    /// Verification is compositional: independently named checks all run, and
    /// their evidence aggregates. Only a repeated check id is ambiguous, and
    /// only for that id.
    pub fn checks_for(
        &self,
        resource: &ResourceView<'_>,
        classes: &BTreeSet<NamespacedId>,
    ) -> BTreeMap<NamespacedId, ContributedCheck<'_>> {
        let mut grouped: BTreeMap<NamespacedId, Vec<ContributedCheck<'_>>> = BTreeMap::new();
        for contribution in &self.verifications {
            for check in &contribution.value.checks {
                if matches(&check.applies_to, resource, classes) {
                    grouped
                        .entry(check.check_id.clone())
                        .or_default()
                        .push(ContributedCheck {
                            extension_id: contribution.extension_id.as_str(),
                            check,
                            ambiguous: false,
                        });
                }
            }
        }
        grouped
            .into_iter()
            .map(|(id, mut candidates)| {
                candidates.sort_by_key(|candidate| candidate.extension_id);
                let first = candidates[0];
                let agreed = candidates
                    .iter()
                    .all(|candidate| candidate.check == first.check);
                (
                    id,
                    ContributedCheck {
                        ambiguous: !agreed,
                        ..first
                    },
                )
            })
            .collect()
    }

    /// Every contributed intent, keyed by its namespaced id.
    pub fn intents(&self) -> BTreeMap<NamespacedId, &draft_extension_contract::IntentDeclaration> {
        let mut sources: Vec<&Contributed<IntentVocabulary>> =
            self.intent_vocabularies.iter().collect();
        sources.sort_by(|a, b| a.extension_id.cmp(&b.extension_id));
        sources
            .into_iter()
            .flat_map(|source| source.value.intents.iter())
            .map(|intent| (intent.intent_id.clone(), intent))
            .collect()
    }

    /// Every contributed candidate preset, keyed by its namespaced id.
    pub fn candidate_presets(&self) -> BTreeMap<NamespacedId, &CandidatePreset> {
        let mut sources: Vec<&Contributed<CandidatePreset>> =
            self.candidate_presets.iter().collect();
        sources.sort_by(|a, b| a.extension_id.cmp(&b.extension_id));
        sources
            .into_iter()
            .map(|source| (source.value.preset_id.clone(), &source.value))
            .collect()
    }

    /// Every contributed task template, keyed by its namespaced id.
    pub fn task_templates(&self) -> BTreeMap<NamespacedId, &TaskTemplateContribution> {
        let mut sources: Vec<&Contributed<TaskTemplateContribution>> =
            self.task_templates.iter().collect();
        sources.sort_by(|a, b| a.extension_id.cmp(&b.extension_id));
        sources
            .into_iter()
            .map(|source| (source.value.template_id.clone(), &source.value))
            .collect()
    }
}

/// One check, with the extension that supplied it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContributedCheck<'a> {
    pub extension_id: &'a str,
    pub check: &'a draft_extension_contract::VerificationCheck,
    /// Set when several extensions claimed this exact check id with different
    /// definitions. The check does not run; every other check is unaffected.
    pub ambiguous: bool,
}

/// What classifying one resource produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassificationOutcome {
    /// Every class agreed for this resource. A resource may carry several.
    pub assigned: BTreeSet<NamespacedId>,
    /// Classes several extensions defined incompatibly. Scoped: only these
    /// classes are unusable.
    pub collisions: Vec<ClassCollision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassCollision {
    pub class_id: NamespacedId,
    pub contributors: Vec<String>,
}

/// Where Draft reads contributed domain knowledge from.
///
/// The default implementation contributes nothing, which is Core-only mode:
/// Draft builds, runs and passes its own tests with no extensions present.
pub trait ExtensionContributionSource: Send + Sync {
    fn active_contributions(&self) -> ActiveContributions;
}

/// The source used when no extensions are wired in.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoExtensions;

impl ExtensionContributionSource for NoExtensions {
    fn active_contributions(&self) -> ActiveContributions {
        ActiveContributions::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_extension_contract::{
        AttributeMatch, AttributeValue, RawResourcePredicate, ResourceForm,
    };
    use draft_extension_contract::{
        EngineId, Executor, MechanismOperation, SchemaRef, VerificationCheck,
    };

    fn id(value: &str) -> NamespacedId {
        NamespacedId::parse(value).unwrap()
    }

    fn attributes() -> BTreeMap<String, AttributeValue> {
        BTreeMap::new()
    }

    fn view<'a>(
        scheme: &'a str,
        body: &'a str,
        media_type: Option<&'a str>,
        attributes: &'a BTreeMap<String, AttributeValue>,
    ) -> ResourceView<'a> {
        ResourceView {
            locator_scheme: scheme,
            locator_body: body,
            media_type,
            form: Some(ResourceForm::Bytes),
            attributes,
            content_size: Some(64),
        }
    }

    fn classification(class: &str, predicate: RawResourcePredicate) -> ResourceClassificationRule {
        ResourceClassificationRule {
            class_id: id(class),
            display_name: class.into(),
            applies_to: predicate,
            attributes: vec![],
        }
    }

    fn operation() -> MechanismOperation {
        MechanismOperation {
            request_contract: SchemaRef::new(id("draft.core/request"), 1),
            response_contract: SchemaRef::new(id("example.pub/response"), 1),
            max_response_bytes: 4096,
            executor: Executor::Engine {
                engine: EngineId::WholeResource,
                engine_revision: 1,
                config: serde_json::json!({}),
            },
        }
    }

    fn check(check_id: &str, class: &str) -> VerificationCheck {
        VerificationCheck {
            check_id: id(check_id),
            display_name: check_id.into(),
            applies_to: ResourcePredicate::HasClass {
                class_id: id(class),
            },
            requirement: draft_extension_contract::CheckRequirement::Required,
            selection: draft_extension_contract::CheckSelection::Whole,
            operation: operation(),
        }
    }

    #[test]
    fn nothing_is_contributed_by_default() {
        let empty = NoExtensions.active_contributions();
        assert!(empty.is_empty());
        let attributes = attributes();
        let resource = view("file", "src/main.rs", None, &attributes);
        assert!(empty.classes_for(&resource).assigned.is_empty());
        assert_eq!(
            empty.comparison_for(&resource, &BTreeSet::new()),
            Resolution::NoMatch
        );
        assert!(empty.checks_for(&resource, &BTreeSet::new()).is_empty());
        assert_eq!(empty.adapter_for_scheme("catalog"), Resolution::NoMatch);
    }

    #[test]
    fn every_declared_contribution_kind_is_consumed() {
        // A package may not install and report healthy while one of its declared
        // contributions is silently ignored, so the supported set is the whole
        // vocabulary.
        for kind in ExtensionContributionKind::ALL {
            assert!(supports_contribution(*kind), "{kind} must be consumed");
        }
        assert_eq!(
            SUPPORTED_CONTRIBUTION_KINDS.len(),
            ExtensionContributionKind::ALL.len()
        );
    }

    #[test]
    fn a_resource_may_carry_several_classes_at_once() {
        // The case a single-valued model got wrong: a source file is genuinely
        // both a text document and a language source, and neither publisher is
        // mistaken.
        let active = ActiveContributions {
            classifications: vec![
                Contributed::new(
                    "draft.text.document",
                    classification(
                        "draft.text.document/document",
                        RawResourcePredicate::MediaType {
                            equals: "text/plain".into(),
                        },
                    ),
                ),
                Contributed::new(
                    "draft.language.rust",
                    classification(
                        "draft.language.rust/source",
                        RawResourcePredicate::PathSuffix {
                            suffix: ".rs".into(),
                        },
                    ),
                ),
            ],
            ..ActiveContributions::default()
        };
        let attributes = attributes();
        let resource = view("file", "src/main.rs", Some("text/plain"), &attributes);
        let outcome = active.classes_for(&resource);
        assert_eq!(
            outcome.assigned,
            BTreeSet::from([
                id("draft.text.document/document"),
                id("draft.language.rust/source")
            ])
        );
        assert!(
            outcome.collisions.is_empty(),
            "coexisting classes are not a conflict"
        );
    }

    #[test]
    fn only_a_repeated_class_collides_and_only_for_that_class() {
        let active = ActiveContributions {
            classifications: vec![
                Contributed::new(
                    "ex.alpha",
                    ResourceClassificationRule {
                        attributes: vec![draft_extension_contract::ClassAttribute {
                            name: "flavour".into(),
                            value: AttributeValue::Text("a".into()),
                        }],
                        ..classification(
                            "shared/kind",
                            RawResourcePredicate::Form {
                                equals: ResourceForm::Bytes,
                            },
                        )
                    },
                ),
                Contributed::new(
                    "ex.zed",
                    ResourceClassificationRule {
                        attributes: vec![draft_extension_contract::ClassAttribute {
                            name: "flavour".into(),
                            value: AttributeValue::Text("b".into()),
                        }],
                        ..classification(
                            "shared/kind",
                            RawResourcePredicate::Form {
                                equals: ResourceForm::Bytes,
                            },
                        )
                    },
                ),
                Contributed::new(
                    "ex.zed",
                    classification(
                        "ex.zed/other",
                        RawResourcePredicate::Form {
                            equals: ResourceForm::Bytes,
                        },
                    ),
                ),
            ],
            ..ActiveContributions::default()
        };
        let attributes = attributes();
        let resource = view("catalog", "row/17", None, &attributes);
        let outcome = active.classes_for(&resource);
        assert_eq!(outcome.collisions.len(), 1);
        assert_eq!(outcome.collisions[0].class_id, id("shared/kind"));
        assert_eq!(
            outcome.collisions[0].contributors,
            vec!["ex.alpha".to_string(), "ex.zed".to_string()]
        );
        // The unrelated class survives the collision.
        assert_eq!(outcome.assigned, BTreeSet::from([id("ex.zed/other")]));
    }

    #[test]
    fn independently_named_checks_all_apply() {
        let active = ActiveContributions {
            verifications: vec![
                Contributed::new(
                    "ex.alpha",
                    VerificationContribution {
                        checks: vec![check("ex.alpha/one", "shared/kind")],
                    },
                ),
                Contributed::new(
                    "ex.zed",
                    VerificationContribution {
                        checks: vec![check("ex.zed/two", "shared/kind")],
                    },
                ),
            ],
            ..ActiveContributions::default()
        };
        let attributes = attributes();
        let resource = view("catalog", "row/17", None, &attributes);
        let classes = BTreeSet::from([id("shared/kind")]);
        let selected = active.checks_for(&resource, &classes);
        assert_eq!(selected.len(), 2, "both checks must run");
        assert!(selected.values().all(|check| !check.ambiguous));
    }

    #[test]
    fn a_duplicated_check_id_is_ambiguous_only_for_itself() {
        let mut conflicting = check("shared/one", "shared/kind");
        conflicting.display_name = "different meaning".into();
        let active = ActiveContributions {
            verifications: vec![
                Contributed::new(
                    "ex.alpha",
                    VerificationContribution {
                        checks: vec![
                            check("shared/one", "shared/kind"),
                            check("ex.alpha/safe", "shared/kind"),
                        ],
                    },
                ),
                Contributed::new(
                    "ex.zed",
                    VerificationContribution {
                        checks: vec![conflicting],
                    },
                ),
            ],
            ..ActiveContributions::default()
        };
        let attributes = attributes();
        let resource = view("catalog", "row/17", None, &attributes);
        let classes = BTreeSet::from([id("shared/kind")]);
        let selected = active.checks_for(&resource, &classes);
        assert!(selected[&id("shared/one")].ambiguous);
        assert!(
            !selected[&id("ex.alpha/safe")].ambiguous,
            "one bad id must not disable the rest"
        );
    }

    #[test]
    fn comparison_resolves_uniquely_and_never_picks_a_winner() {
        let strategy = |id_value: &str| ComparisonContribution {
            strategy_id: id(id_value),
            applies_to: ResourcePredicate::Raw {
                of: RawResourcePredicate::LocatorScheme {
                    equals: "catalog".into(),
                },
            },
            result_contract: SchemaRef::new(id("example.pub/result"), 1),
            operation: operation(),
        };
        let active = ActiveContributions {
            comparisons: vec![
                Contributed::new("ex.zed", strategy("ex.zed/keys")),
                Contributed::new("ex.alpha", strategy("ex.alpha/keys")),
            ],
            ..ActiveContributions::default()
        };
        let attributes = attributes();
        let resource = view("catalog", "row/17", None, &attributes);
        let resolved = active.comparison_for(&resource, &BTreeSet::new());
        assert!(resolved.is_ambiguous());
        assert_eq!(resolved.value(), None);
        assert_eq!(resolved.contributors(), vec!["ex.alpha", "ex.zed"]);
    }

    #[test]
    fn filesystem_predicates_never_match_another_scheme() {
        // A rule written for files must not capture a catalog resource whose
        // opaque body happens to contain slashes and a dot.
        let attributes = attributes();
        let catalog = view("catalog", "src/main.rs", None, &attributes);
        let file = view("file", "src/main.rs", None, &attributes);
        for predicate in [
            RawResourcePredicate::PathSuffix {
                suffix: ".rs".into(),
            },
            RawResourcePredicate::PathGlob {
                glob: "src/**".into(),
            },
        ] {
            assert!(matches_raw(&predicate, &file));
            assert!(!matches_raw(&predicate, &catalog));
        }
        // The scheme-neutral locator predicate matches both, which is the point
        // of having both forms.
        let neutral = RawResourcePredicate::LocatorPattern {
            glob: "src/**".into(),
        };
        assert!(matches_raw(&neutral, &file));
        assert!(matches_raw(&neutral, &catalog));
    }

    #[test]
    fn attribute_rules_do_not_coerce_across_types() {
        let mut attributes = BTreeMap::new();
        attributes.insert("count".to_string(), AttributeValue::Integer(7));
        attributes.insert("role".to_string(), AttributeValue::Text("stem".into()));
        let resource = view("timeline", "clip/3", None, &attributes);

        assert!(matches_raw(
            &RawResourcePredicate::Attribute {
                name: "role".into(),
                matches: AttributeMatch::Prefix { value: "st".into() },
            },
            &resource
        ));
        // A text rule against a number is not a match, rather than a coercion.
        assert!(!matches_raw(
            &RawResourcePredicate::Attribute {
                name: "count".into(),
                matches: AttributeMatch::Prefix { value: "7".into() },
            },
            &resource
        ));
        assert!(matches_raw(
            &RawResourcePredicate::Attribute {
                name: "count".into(),
                matches: AttributeMatch::Range {
                    at_least: Some(5),
                    at_most: Some(9)
                },
            },
            &resource
        ));
        // A missing attribute is absence, never a match.
        assert!(!matches_raw(
            &RawResourcePredicate::Attribute {
                name: "absent".into(),
                matches: AttributeMatch::Equals {
                    value: AttributeValue::Boolean(true)
                },
            },
            &resource
        ));
    }

    #[test]
    fn a_gap_names_no_package_and_normalizes_its_classes() {
        let gap = CapabilityGap::new(
            ExtensionCapabilityKind::Comparison,
            [
                "zz.pub/kind".to_string(),
                "aa.pub/kind".to_string(),
                "zz.pub/kind".to_string(),
            ],
            "no installed extension compares these resource classes",
        );
        assert_eq!(
            gap.resource_classes,
            vec!["aa.pub/kind".to_string(), "zz.pub/kind".to_string()]
        );
        let encoded = serde_json::to_value(&gap).unwrap();
        let mut fields: Vec<&str> = encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort();
        assert_eq!(fields, ["capability", "reason", "resource_classes"]);
        // Identity is derived from what the gap is, not from its wording.
        let reworded = CapabilityGap::new(
            ExtensionCapabilityKind::Comparison,
            ["aa.pub/kind".to_string(), "zz.pub/kind".to_string()],
            "different prose entirely",
        );
        assert_eq!(gap.gap_id(), reworded.gap_id());
    }
}
