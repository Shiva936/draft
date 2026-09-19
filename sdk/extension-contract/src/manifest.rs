//! Extension identity and the declarative manifest.
//!
//! A manifest declares what an extension *contributes* — records, metadata,
//! documentation and static assets. It cannot declare code, an entrypoint, or
//! anything Draft would execute on the package's behalf.

use crate::{FormatError, FormatResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Canonical extension identity.
///
/// Lowercase ASCII, digits, `-` and `.`, at most 64 characters. The newtype is
/// transparent on the wire, so it is byte-compatible with the plain string the
/// format has always used, while making it impossible to pass a package version
/// or a source key where an extension id is meant.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExtensionId(String);

impl ExtensionId {
    pub const MAX_LENGTH: usize = 64;

    /// Accept `value` as an extension id, or explain why it is not one.
    pub fn parse(value: impl Into<String>) -> FormatResult<Self> {
        let value = value.into();
        let acceptable = !value.is_empty()
            && value.len() <= Self::MAX_LENGTH
            && value.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '-' | '.')
            });
        if acceptable {
            Ok(Self(value))
        } else {
            Err(FormatError::Identity(format!(
                "extension id '{value}' must be 1-{} characters of lowercase ASCII, digits, '-' or '.'",
                Self::MAX_LENGTH
            )))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for ExtensionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for ExtensionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for ExtensionId {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for ExtensionId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for ExtensionId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for ExtensionId {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

impl PartialEq<ExtensionId> for String {
    fn eq(&self, other: &ExtensionId) -> bool {
        self == &other.0
    }
}

impl PartialEq<ExtensionId> for str {
    fn eq(&self, other: &ExtensionId) -> bool {
        self == other.0.as_str()
    }
}

/// What a contribution supplies. Every kind names a validated static record;
/// none names something Draft would run on the package's behalf.
///
/// The vocabulary is domain-neutral throughout: a kind describes a *role* in
/// Draft's lifecycle — how resources are reached, classified, compared,
/// verified, presented, transformed — never a file, a language or a toolchain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionContributionKind {
    /// A resource backend for one locator scheme.
    ResourceAdapter,
    /// Assigns namespaced classes to the resources it recognizes.
    ResourceClassification,
    /// Explains how a resource changed.
    Comparison,
    /// Finds the elements inside a resource.
    ElementExtraction,
    /// Binds a generic platform rendering engine to a schema, class or predicate.
    Presentation,
    /// A transformation or inspection Draft may invoke.
    ToolAction,
    /// Independently named verification checks.
    Verification,
    /// Weighted risk rules over neutral conditions.
    RiskRule,
    /// View rules and control policy, digested separately.
    PolicyPreset,
    /// Domain intents Draft stores and compares but never interprets.
    IntentVocabulary,
    /// A task template.
    TaskTemplate,
    /// A candidate preset. Data only: it can never itself cause execution.
    CandidatePreset,
    /// Prose. Consumed as metadata only, by design.
    Documentation,
}

impl ExtensionContributionKind {
    /// Every kind this format revision defines.
    pub const ALL: &'static [Self] = &[
        Self::ResourceAdapter,
        Self::ResourceClassification,
        Self::Comparison,
        Self::ElementExtraction,
        Self::Presentation,
        Self::ToolAction,
        Self::Verification,
        Self::RiskRule,
        Self::PolicyPreset,
        Self::IntentVocabulary,
        Self::TaskTemplate,
        Self::CandidatePreset,
        Self::Documentation,
    ];

    /// The capability Draft would exercise for a contribution of this kind.
    ///
    /// `None` for the kinds that are **data rather than behaviour**. Draft
    /// never asks a policy preset, an intent vocabulary, a task template, a
    /// candidate preset or a documentation record to *do* anything: it reads
    /// them. A capability names what an extension may be asked to perform, so
    /// giving those a capability would misdescribe them — and minting a
    /// `draft.*` name for them is exactly what the reserved namespace forbids.
    ///
    /// This is the distinction the open model makes visible that the closed
    /// enum could not: "kind of contribution" conflated invocable behaviour
    /// with inert data, and they need different handling at dispatch.
    pub fn capability(self) -> Option<&'static str> {
        Some(match self {
            Self::ResourceAdapter => "draft.resource.observe/v1",
            Self::ResourceClassification => "draft.resource.detect/v1",
            Self::ElementExtraction => "draft.resource.fingerprint/v1",
            Self::Comparison => "draft.compare/v1",
            Self::Presentation => "draft.change.represent/v1",
            Self::ToolAction => "draft.change.operate/v1",
            Self::Verification => "draft.validate/v1",
            Self::RiskRule => "draft.assess/v1",
            Self::PolicyPreset
            | Self::IntentVocabulary
            | Self::TaskTemplate
            | Self::CandidatePreset
            | Self::Documentation => return None,
        })
    }

    /// The stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ResourceAdapter => "resource_adapter",
            Self::ResourceClassification => "resource_classification",
            Self::Comparison => "comparison",
            Self::ElementExtraction => "element_extraction",
            Self::Presentation => "presentation",
            Self::ToolAction => "tool_action",
            Self::Verification => "verification",
            Self::RiskRule => "risk_rule",
            Self::PolicyPreset => "policy_preset",
            Self::IntentVocabulary => "intent_vocabulary",
            Self::TaskTemplate => "task_template",
            Self::CandidatePreset => "candidate_preset",
            Self::Documentation => "documentation",
        }
    }

    /// Whether this kind is consumed for its data alone, with no subsystem
    /// acting on it. Exactly one kind is, and it says so here rather than
    /// silently going unread.
    pub fn is_metadata_only(self) -> bool {
        matches!(self, Self::Documentation)
    }
}

impl std::fmt::Display for ExtensionContributionKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One declared contribution: a stable id, its kind, and the static file that
/// carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionContribution {
    pub id: String,
    pub kind: ExtensionContributionKind,
    pub path: String,
}

/// The declarative extension manifest, `extension.json` at a package root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifest {
    pub schema_version: u32,
    pub id: ExtensionId,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub draft_api: String,
    pub contributions: Vec<ExtensionContribution>,
    /// Result schemas this package owns and ships.
    ///
    /// Their bytes live under `schemas/` and are therefore covered by the
    /// package content hash and its signature. That is what lets a payload
    /// produced years ago be re-validated against the exact schema that
    /// produced it, rather than against whichever revision happens to be
    /// installed now.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemas: Vec<crate::schema::PackageSchema>,
    /// Permissions this package asks for. Absent means it asks for none, which
    /// is the normal case: only a package declaring commands needs anything.
    #[serde(default)]
    pub permissions: Vec<crate::contribution::ExtensionPermission>,
    /// One line describing what the package is for.
    ///
    /// This and `keywords` exist so a catalog's searchable metadata can be
    /// *derived* from the manifest rather than maintained a second time
    /// alongside it, where the two could drift apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    pub documentation: Vec<String>,
    pub licenses: Vec<String>,
    pub assets: Vec<String>,
}

impl ExtensionManifest {
    /// Validate everything about a manifest that can be decided from the
    /// manifest alone: identity, schema marker, API compatibility against
    /// `api_version`, contribution id uniqueness, and the namespace and file
    /// type of every declared path.
    ///
    /// This is the format's own rule set. Draft additionally resolves each
    /// declared path against the real filesystem under its own path guard
    /// before trusting a package; neither check replaces the other.
    pub fn validate_shape(&self, api_version: &str) -> FormatResult<()> {
        if self.schema_version != crate::FORMAT_REVISION {
            return Err(FormatError::Identity(format!(
                "extension manifest schema_version {} is not the supported format revision {}",
                self.schema_version,
                crate::FORMAT_REVISION
            )));
        }
        // Re-parse so a manifest decoded straight from JSON is held to the same
        // identity rule as one built through `ExtensionId::parse`.
        ExtensionId::parse(self.id.as_str())?;
        if self.name.trim().is_empty() || self.publisher.trim().is_empty() {
            return Err(FormatError::Identity(
                "extension manifest name and publisher must not be blank".into(),
            ));
        }
        if self.version.trim().is_empty() {
            return Err(FormatError::Identity(
                "extension manifest version must not be blank".into(),
            ));
        }
        if !crate::draft_api_compatible(&self.draft_api, api_version) {
            return Err(FormatError::Compatibility(format!(
                "extension requires Draft API '{}', which {api_version} does not satisfy",
                self.draft_api
            )));
        }

        let mut declared_permissions = BTreeSet::new();
        for permission in &self.permissions {
            if !declared_permissions.insert(permission) {
                return Err(FormatError::Identity(format!(
                    "extension declares permission '{permission}' more than once"
                )));
            }
        }

        let mut contribution_ids = BTreeSet::new();
        for contribution in &self.contributions {
            if contribution.id.trim().is_empty() || !contribution_ids.insert(&contribution.id) {
                return Err(FormatError::Identity(
                    "extension contribution ids must be non-empty and unique".into(),
                ));
            }
            crate::package::validate_declared_path(
                &contribution.path,
                crate::package::CONTRIBUTIONS_PREFIX,
                crate::package::CONTRIBUTION_EXTENSIONS,
            )?;
        }
        // Every schema is namespaced to this package and declared exactly once
        // per revision: a publisher cannot redefine what a revision means, and
        // no two packages can claim the same identifier.
        let mut schema_revisions = BTreeSet::new();
        for schema in &self.schemas {
            if !schema.schema_id.is_owned_by(self.id.as_str()) {
                return Err(FormatError::Identity(format!(
                    "schema '{}' is not namespaced to the declaring extension '{}'",
                    schema.schema_id, self.id
                )));
            }
            if !schema_revisions.insert((schema.schema_id.clone(), schema.revision)) {
                return Err(FormatError::Identity(format!(
                    "schema '{}' declares revision {} more than once",
                    schema.schema_id, schema.revision
                )));
            }
            crate::package::validate_declared_path(
                &schema.path,
                crate::package::SCHEMAS_PREFIX,
                crate::package::SCHEMA_EXTENSIONS,
            )?;
            if !schema.path.ends_with(".schema.json") {
                return Err(FormatError::Path(format!(
                    "declared schema path '{}' must end in '.schema.json'",
                    schema.path
                )));
            }
        }
        for path in &self.documentation {
            crate::package::validate_declared_path(
                path,
                crate::package::DOCS_PREFIX,
                crate::package::DOC_EXTENSIONS,
            )?;
        }
        for path in &self.licenses {
            crate::package::validate_declared_path(path, "", crate::package::DOC_EXTENSIONS)?;
            crate::package::validate_license_name(path)?;
        }
        for path in &self.assets {
            crate::package::validate_declared_path(
                path,
                crate::package::ASSETS_PREFIX,
                crate::package::ASSET_EXTENSIONS,
            )?;
        }
        Ok(())
    }

    /// The capability names this package contributes, derived from its
    /// declared contribution kinds.
    pub fn capabilities(&self) -> Vec<String> {
        let mut capabilities: Vec<String> = self
            .contributions
            .iter()
            .map(|contribution| contribution.kind.as_str().to_string())
            .collect();
        capabilities.sort();
        capabilities.dedup();
        capabilities
    }

    /// Every path the manifest declares, in declaration order.
    pub fn declared_paths(&self) -> Vec<&str> {
        self.contributions
            .iter()
            .map(|contribution| contribution.path.as_str())
            .chain(self.schemas.iter().map(|schema| schema.path.as_str()))
            .chain(self.documentation.iter().map(String::as_str))
            .chain(self.licenses.iter().map(String::as_str))
            .chain(self.assets.iter().map(String::as_str))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ExtensionManifest {
        ExtensionManifest {
            schema_version: crate::FORMAT_REVISION,
            id: ExtensionId::parse("example.docs").unwrap(),
            name: "Example docs".into(),
            version: "1.0.0".into(),
            publisher: "example".into(),
            draft_api: "^0.3.4".into(),
            contributions: vec![ExtensionContribution {
                id: "task-review".into(),
                kind: ExtensionContributionKind::TaskTemplate,
                path: "contributions/task.json".into(),
            }],
            schemas: vec![],
            permissions: vec![],
            description: None,
            keywords: vec![],
            documentation: vec!["docs/readme.md".into()],
            licenses: vec!["LICENSE.txt".into()],
            assets: vec![],
        }
    }

    #[test]
    fn identity_rules_are_enforced() {
        assert!(ExtensionId::parse("draft.language.rust").is_ok());
        for rejected in ["", "Upper", "has space", "under_score", &"x".repeat(65)] {
            assert!(
                ExtensionId::parse(rejected).is_err(),
                "{rejected} should be rejected"
            );
        }
    }

    #[test]
    fn extension_id_is_transparent_on_the_wire() {
        let id = ExtensionId::parse("draft.language.rust").unwrap();
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"draft.language.rust\""
        );
        let decoded: ExtensionId = serde_json::from_str("\"draft.language.rust\"").unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn a_well_formed_manifest_validates() {
        manifest().validate_shape("0.3.4").unwrap();
    }

    #[test]
    fn incompatible_api_and_duplicate_contributions_are_rejected() {
        let mut incompatible = manifest();
        incompatible.draft_api = "^9.0.0".into();
        assert!(matches!(
            incompatible.validate_shape("0.3.4"),
            Err(FormatError::Compatibility(_))
        ));

        let mut duplicated = manifest();
        duplicated.contributions.push(ExtensionContribution {
            id: "task-review".into(),
            kind: ExtensionContributionKind::PolicyPreset,
            path: "contributions/policy.json".into(),
        });
        assert!(matches!(
            duplicated.validate_shape("0.3.4"),
            Err(FormatError::Identity(_))
        ));
    }

    #[test]
    fn declared_paths_must_stay_in_their_namespace() {
        let mut escaping = manifest();
        escaping.contributions[0].path = "docs/task.json".into();
        assert!(matches!(
            escaping.validate_shape("0.3.4"),
            Err(FormatError::Path(_))
        ));
    }

    fn schema(id: &str, revision: u32, path: &str) -> crate::schema::PackageSchema {
        crate::schema::PackageSchema {
            schema_id: crate::identifier::NamespacedId::parse(id).unwrap(),
            revision,
            path: path.into(),
        }
    }

    #[test]
    fn a_package_may_only_declare_schemas_it_owns() {
        let mut owned = manifest();
        owned.schemas = vec![schema(
            "example.docs/result",
            1,
            "schemas/result.schema.json",
        )];
        owned.validate_shape("0.3.4").unwrap();
        // The schema path joins the declared set, so an undeclared or stray
        // file under `schemas/` cannot ride along unnoticed.
        assert!(owned
            .declared_paths()
            .contains(&"schemas/result.schema.json"));

        // Squatting another publisher's namespace is refused outright.
        let mut squatting = manifest();
        squatting.schemas = vec![schema("other.pub/result", 1, "schemas/result.schema.json")];
        assert!(matches!(
            squatting.validate_shape("0.3.4"),
            Err(FormatError::Identity(_))
        ));
    }

    #[test]
    fn a_schema_revision_is_declared_exactly_once() {
        let mut repeated = manifest();
        repeated.schemas = vec![
            schema("example.docs/result", 1, "schemas/a.schema.json"),
            schema("example.docs/result", 1, "schemas/b.schema.json"),
        ];
        assert!(matches!(
            repeated.validate_shape("0.3.4"),
            Err(FormatError::Identity(_))
        ));

        // Two revisions of the same schema are fine; one revision meaning two
        // different documents is not.
        let mut distinct = manifest();
        distinct.schemas = vec![
            schema("example.docs/result", 1, "schemas/a.schema.json"),
            schema("example.docs/result", 2, "schemas/b.schema.json"),
        ];
        distinct.validate_shape("0.3.4").unwrap();
    }

    #[test]
    fn a_schema_path_stays_in_its_namespace_and_names_itself() {
        let mut misplaced = manifest();
        misplaced.schemas = vec![schema("example.docs/result", 1, "docs/result.schema.json")];
        assert!(matches!(
            misplaced.validate_shape("0.3.4"),
            Err(FormatError::Path(_))
        ));

        let mut mistyped = manifest();
        mistyped.schemas = vec![schema("example.docs/result", 1, "schemas/result.json")];
        assert!(matches!(
            mistyped.validate_shape("0.3.4"),
            Err(FormatError::Path(_))
        ));
    }

    #[test]
    fn every_contribution_kind_is_named_and_only_documentation_is_metadata_only() {
        let metadata_only: Vec<_> = ExtensionContributionKind::ALL
            .iter()
            .filter(|kind| kind.is_metadata_only())
            .collect();
        assert_eq!(
            metadata_only,
            vec![&ExtensionContributionKind::Documentation]
        );
        // Wire names round-trip, so the manifest form, the schema and the CLI
        // cannot drift apart.
        for kind in ExtensionContributionKind::ALL {
            let encoded = serde_json::to_value(kind).unwrap();
            assert_eq!(encoded, serde_json::Value::String(kind.as_str().into()));
        }
    }

    #[test]
    fn a_stale_schema_marker_is_rejected() {
        let mut stale = manifest();
        stale.schema_version = crate::FORMAT_REVISION + 1;
        assert!(matches!(
            stale.validate_shape("0.3.4"),
            Err(FormatError::Identity(_))
        ));
    }
}

#[cfg(test)]
mod capability_mapping_tests {
    use super::*;
    use draft_dcg_contract::capability::RESERVED_CAPABILITIES;
    use draft_dcg_contract::CapabilityId;

    #[test]
    fn every_invocable_kind_names_a_capability_this_build_implements() {
        for kind in ExtensionContributionKind::ALL {
            let Some(name) = kind.capability() else {
                continue;
            };
            let capability =
                CapabilityId::parse(name).unwrap_or_else(|error| panic!("{kind:?}: {error}"));
            assert!(
                capability.is_recognised_reserved(),
                "{kind:?} names '{name}', which is not a reserved capability"
            );
        }
    }

    #[test]
    fn no_two_kinds_claim_the_same_capability() {
        // Two kinds sharing a capability would make dispatch ambiguous: Draft
        // would not know which contribution it was invoking.
        let mut seen = std::collections::BTreeSet::new();
        for kind in ExtensionContributionKind::ALL {
            if let Some(name) = kind.capability() {
                assert!(seen.insert(name), "{name} is claimed by two kinds");
            }
        }
    }

    #[test]
    fn data_only_kinds_deliberately_name_no_capability() {
        // Draft reads these; it never asks them to do anything. Giving them a
        // capability would misdescribe them, and minting a `draft.*` name for
        // them is what the reserved namespace forbids.
        for kind in [
            ExtensionContributionKind::PolicyPreset,
            ExtensionContributionKind::IntentVocabulary,
            ExtensionContributionKind::TaskTemplate,
            ExtensionContributionKind::CandidatePreset,
            ExtensionContributionKind::Documentation,
        ] {
            assert!(kind.capability().is_none(), "{kind:?}");
        }
    }

    #[test]
    fn the_reserved_vocabulary_covers_more_than_todays_kinds() {
        // materialize, merge, lock, publish and recover are reserved but not
        // yet reachable from a contribution kind. That is expected: the
        // capability vocabulary describes what Draft can ask for, and the
        // subsystems that ask land in later stages.
        let claimed: std::collections::BTreeSet<&str> = ExtensionContributionKind::ALL
            .iter()
            .filter_map(|kind| kind.capability())
            .collect();
        let unclaimed: Vec<&&str> = RESERVED_CAPABILITIES
            .iter()
            .filter(|reserved| !claimed.contains(**reserved))
            .collect();
        assert!(
            !unclaimed.is_empty(),
            "if every reserved capability is claimed, this test has stopped saying anything"
        );
    }
}
