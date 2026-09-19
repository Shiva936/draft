//! Protection policy: what a change is never allowed to touch.
//!
//! Core owns exactly one protection, and it is structural: `.draft/**`, its own
//! control plane, which no configuration and no extension can unprotect.
//! Everything else — credentials, key material, registry tokens — is domain
//! judgement, contributed by an extension's `control_policy` or written by the
//! project. A bare Draft protects its own state and nothing else, and says so
//! rather than pretending a list of software filenames is universal.

use crate::project::home::DraftGlobalStore;
use crate::project::layout::DraftLayout;
use crate::support::common::WorkspacePath;
use crate::support::error::{DraftError, DraftResult};
use crate::support::pathguard;
use crate::support::predicate::{matches_raw, ResourceView, FILE_SCHEME};
use draft_extension_contract::RawResourcePredicate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Where a protection came from. Kept on the rule so a refusal can say who
/// asked for it, and so an operator can tell a contributed protection from one
/// their own project wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ProtectionSource {
    /// Draft's own control plane. Not overridable.
    Core,
    /// `~/.draft/config.toml`.
    Global,
    /// `<root>/.draft/config.toml`.
    Project,
    /// An installed extension's `control_policy`.
    Extension { extension_id: String },
}

impl ProtectionSource {
    pub fn label(&self) -> String {
        match self {
            Self::Core => "draft".to_string(),
            Self::Global => "global config".to_string(),
            Self::Project => "project config".to_string(),
            Self::Extension { extension_id } => extension_id.clone(),
        }
    }
}

/// One protection rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectionRule {
    /// The condition, over intrinsic resource facts only. A project's
    /// `protected_resources` glob is exactly a [`RawResourcePredicate::PathGlob`].
    pub predicate: RawResourcePredicate,
    pub reason: String,
    #[serde(flatten)]
    pub source: ProtectionSource,
}

impl ProtectionRule {
    /// A short rendering for errors and for the agent task briefing.
    pub fn describe(&self) -> String {
        describe_predicate(&self.predicate)
    }
}

fn describe_predicate(predicate: &RawResourcePredicate) -> String {
    match predicate {
        RawResourcePredicate::PathGlob { glob } | RawResourcePredicate::LocatorPattern { glob } => {
            glob.clone()
        }
        RawResourcePredicate::PathSuffix { suffix } => format!("*{suffix}"),
        RawResourcePredicate::LocatorScheme { equals } => format!("{equals}:*"),
        RawResourcePredicate::MediaType { equals } => format!("media-type {equals}"),
        RawResourcePredicate::Form { equals } => format!("form {equals:?}"),
        RawResourcePredicate::Attribute { name, .. } => format!("attribute {name}"),
        RawResourcePredicate::ContentSize { .. } => "content size".to_string(),
        RawResourcePredicate::Not { of } => format!("not({})", describe_predicate(of)),
        RawResourcePredicate::All { of } => format!(
            "all({})",
            of.iter()
                .map(describe_predicate)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        RawResourcePredicate::Any { of } => format!(
            "any({})",
            of.iter()
                .map(describe_predicate)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedResourceViolation {
    pub path: String,
    pub pattern: String,
    pub reason: String,
    pub source: String,
}

/// The protections Core itself defines: its own control plane, and nothing
/// else.
///
/// The `.draft/**` refusal is enforced structurally in [`first_violation`] as
/// well, so it holds even for a caller that assembled its own rule list.
pub fn core_rules() -> Vec<ProtectionRule> {
    vec![ProtectionRule {
        predicate: RawResourcePredicate::PathGlob {
            glob: ".draft/**".to_string(),
        },
        reason: "Draft's control plane is never project content".to_string(),
        source: ProtectionSource::Core,
    }]
}

/// The project's effective protections: Core's, the global and project config's,
/// and every installed extension's `control_policy` protections.
///
/// Composition is a conservative union — the algebra declared for
/// `policy_preset`. Two extensions protecting the same thing is not a conflict,
/// and no layer can remove a protection another layer added.
/// `contributed` carries protections that came from installed extensions,
/// already reduced to rules. They arrive as a parameter rather than being
/// gathered here because a project's protections must not depend on the
/// extension layer: `project` sits below it, and inverting the direction is
/// what makes that structural rather than a convention. The caller — which
/// already knows what is installed — performs the conversion.
pub fn rules_for_project(
    root: &Path,
    contributed: &[ProtectionRule],
) -> DraftResult<Vec<ProtectionRule>> {
    let mut rules = core_rules();
    let home = DraftGlobalStore::locate()?;
    extend_from_config(&mut rules, &home.config_toml(), ProtectionSource::Global)?;
    extend_from_config(
        &mut rules,
        &DraftLayout::for_root(root).config_toml(),
        ProtectionSource::Project,
    )?;
    rules.extend(contributed.iter().cloned());
    Ok(rules)
}

/// Whether any rule protects the given `file`-scheme path.
///
/// Only the locator is known here, so a rule predicating on media type, form,
/// attributes or size does not match. That is the conservative direction for a
/// *check*, and the observation paths that do know those facts use
/// [`matches_resource`] instead.
pub fn matches_rules(rules: &[ProtectionRule], path: &str) -> bool {
    first_violation(path, rules).is_some()
}

/// Whether any rule protects a fully observed resource.
pub fn matches_resource<'a>(
    rules: &'a [ProtectionRule],
    resource: &ResourceView<'_>,
) -> Option<&'a ProtectionRule> {
    if resource.locator_scheme == FILE_SCHEME && pathguard::is_draft_path(resource.locator_body) {
        return rules.first();
    }
    rules
        .iter()
        .find(|rule| matches_raw(&rule.predicate, resource))
}

pub fn violations<'a>(
    rules: &[ProtectionRule],
    paths: impl IntoIterator<Item = &'a WorkspacePath>,
) -> Vec<ProtectedResourceViolation> {
    paths
        .into_iter()
        .filter_map(|path| first_violation(path.as_str(), rules))
        .collect()
}

pub fn ensure_allowed(rules: &[ProtectionRule], path: &WorkspacePath) -> DraftResult<()> {
    if let Some(v) = first_violation(path.as_str(), rules) {
        return Err(DraftError::new(
            crate::support::error::DraftErrorKind::ProtectedResourceAccess,
            format!("protected resource access blocked: {}", v.path),
        )
        .with_context(format!(
            "matched '{}' from {} ({})",
            v.pattern, v.source, v.reason
        ))
        .with_suggestion("change the ChangePack or editor scope, or update protection policy"));
    }
    Ok(())
}

fn extend_from_config(
    rules: &mut Vec<ProtectionRule>,
    path: &Path,
    source: ProtectionSource,
) -> DraftResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let (protected, _) = crate::project::config::rules(path)?;
    for rule in protected.protected_resources {
        rules.push(ProtectionRule {
            predicate: RawResourcePredicate::PathGlob { glob: rule.pattern },
            reason: rule
                .reason
                .unwrap_or_else(|| "configured protected resource".to_string()),
            source: source.clone(),
        });
    }
    Ok(())
}

fn first_violation(path: &str, rules: &[ProtectionRule]) -> Option<ProtectedResourceViolation> {
    let normalized = path.replace('\\', "/");
    // Structural, and deliberately ahead of the rule list: no configuration and
    // no contribution can make Draft's own control plane writable.
    if pathguard::is_draft_path(&normalized) {
        return Some(ProtectedResourceViolation {
            path: normalized,
            pattern: ".draft/**".to_string(),
            reason: "Draft's control plane is never project content".to_string(),
            source: ProtectionSource::Core.label(),
        });
    }
    let attributes = BTreeMap::new();
    let view = ResourceView {
        locator_scheme: FILE_SCHEME,
        locator_body: &normalized,
        media_type: None,
        form: None,
        attributes: &attributes,
        content_size: None,
    };
    rules
        .iter()
        .find(|rule| matches_raw(&rule.predicate, &view))
        .map(|rule| ProtectedResourceViolation {
            path: normalized.clone(),
            pattern: rule.describe(),
            reason: rule.reason.clone(),
            source: rule.source.label(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glob(pattern: &str, extension_id: &str) -> ProtectionRule {
        ProtectionRule {
            predicate: RawResourcePredicate::PathGlob {
                glob: pattern.to_string(),
            },
            reason: "test".to_string(),
            source: ProtectionSource::Extension {
                extension_id: extension_id.to_string(),
            },
        }
    }

    #[test]
    fn core_protects_its_own_control_plane_and_nothing_else() {
        let rules = core_rules();
        // The one protection Draft owns, and it holds at any depth.
        assert!(first_violation(".draft/project.json", &rules).is_some());
        assert!(first_violation("nested/.draft/project.json", &rules).is_some());
        // Credential filenames are domain judgement, not Core's. With nothing
        // installed they are ordinary project state.
        assert!(first_violation(".env", &rules).is_none());
        assert!(first_violation("id_rsa", &rules).is_none());
        assert!(first_violation("src/lib.rs", &rules).is_none());
    }

    #[test]
    fn a_contributed_protection_is_enforced_and_names_its_contributor() {
        let mut rules = core_rules();
        rules.push(glob("**/.env", "draft.filesystem.policy"));

        let violation = first_violation("config/.env", &rules).expect("protected");
        assert_eq!(violation.pattern, "**/.env");
        assert_eq!(violation.source, "draft.filesystem.policy");
        assert!(first_violation("config/app.toml", &rules).is_none());
    }

    #[test]
    fn the_control_plane_stays_protected_with_an_empty_rule_set() {
        // Even a caller that assembled no rules at all cannot reach `.draft/`.
        assert!(first_violation(".draft/config.toml", &[]).is_some());
    }

    #[test]
    fn a_path_predicate_never_matches_another_scheme() {
        let rules = vec![glob("**/*.key", "draft.filesystem.policy")];
        let attributes = BTreeMap::new();
        let catalog = ResourceView {
            locator_scheme: "catalog",
            // A body that merely looks path-like must not be captured by a
            // rule written for files.
            locator_body: "vault/secret.key",
            media_type: None,
            form: None,
            attributes: &attributes,
            content_size: None,
        };
        assert!(matches_resource(&rules, &catalog).is_none());

        let file = ResourceView {
            locator_scheme: FILE_SCHEME,
            locator_body: "vault/secret.key",
            ..catalog
        };
        assert!(matches_resource(&rules, &file).is_some());
    }
}
