//! Project policy, and the snapshot that freezes it for a decision.
//!
//! Policy lives in `project` because it is part of what a project *is*: the
//! rules it applies to its own work. Everything that evaluates against policy
//! — review, gates, promotion readiness — sits above it and reads it, which is
//! why the dependency runs this way and not the other.
//!
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use draft_dcg_contract::security::PolicyDigest;
use draft_dcg_contract::Digest;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A decision produced by evaluating policy against a proposed action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Warn(String),
    Deny(String),
    RequireApproval,
    RequireReverify,
    RequireFullVerify,
    RequireFuzz,
}

/// The resolved, effective policy for a workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub schema_version: u32,
    /// A submission is blocked unless the ChangePack is explicitly approved.
    pub require_approval_for_promotion: bool,
    /// An unresolved `critical` risk blocks promotion.
    pub block_on_critical_risk: bool,
    /// A `high` risk ChangePack requires approval before submission.
    pub require_approval_on_high_risk: bool,
    /// If the workspace hash changed since verification, re-verify before submission.
    pub require_reverify_on_workspace_change: bool,
    /// Imported ChangePacks must be locally re-verified before they can be promoted.
    pub require_local_verify_for_imports: bool,
    /// Intents requiring the full check set rather than the change-scoped
    /// selection. Namespaced intent ids, contributed or project-configured.
    pub require_full_verify_intents: Vec<String>,
    /// Intents requiring the exploratory checks too.
    pub require_fuzz_intents: Vec<String>,
}

impl crate::contracts::VersionedContract for Policy {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::Policy;
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            schema_version: crate::contracts::current_version(crate::contracts::ContractId::Policy),
            require_approval_for_promotion: true,
            block_on_critical_risk: true,
            require_approval_on_high_risk: true,
            require_reverify_on_workspace_change: true,
            require_local_verify_for_imports: true,
            // No intent escalations by default: an intent id Core invented
            // would name a vocabulary nothing installed declares, and could
            // never match.
            require_full_verify_intents: Vec::new(),
            require_fuzz_intents: Vec::new(),
        }
    }
}

/// A partially specified policy layer: only the fields present in the file
/// override lower-precedence layers.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialPolicy {
    schema_version: u32,
    require_approval_for_promotion: Option<bool>,
    block_on_critical_risk: Option<bool>,
    require_approval_on_high_risk: Option<bool>,
    require_reverify_on_workspace_change: Option<bool>,
    require_local_verify_for_imports: Option<bool>,
    require_full_verify_intents: Option<Vec<String>>,
    require_fuzz_intents: Option<Vec<String>>,
}

impl PartialPolicy {
    fn overlay(self, base: &mut Policy) {
        if let Some(v) = self.require_approval_for_promotion {
            base.require_approval_for_promotion = v;
        }
        if let Some(v) = self.block_on_critical_risk {
            base.block_on_critical_risk = v;
        }
        if let Some(v) = self.require_approval_on_high_risk {
            base.require_approval_on_high_risk = v;
        }
        if let Some(v) = self.require_reverify_on_workspace_change {
            base.require_reverify_on_workspace_change = v;
        }
        if let Some(v) = self.require_local_verify_for_imports {
            base.require_local_verify_for_imports = v;
        }
        if let Some(v) = self.require_full_verify_intents {
            base.require_full_verify_intents = v;
        }
        if let Some(v) = self.require_fuzz_intents {
            base.require_fuzz_intents = v;
        }
    }
}

impl Policy {
    /// The built-in safe default policy.
    pub fn safe_default() -> Self {
        Policy::default()
    }

    /// Resolve the effective policy, failing closed when any present layer is
    /// unreadable, malformed, or unsupported.
    pub fn resolve(
        project_policy: Option<&Path>,
        global_policy: Option<&Path>,
    ) -> DraftResult<Self> {
        let mut effective = Policy::safe_default();
        if let Some(path) = global_policy {
            if let Some(g) = load_partial(path)? {
                g.overlay(&mut effective);
            }
        }
        if let Some(path) = project_policy {
            if let Some(p) = load_partial(path)? {
                p.overlay(&mut effective);
            }
        }
        Ok(effective)
    }

    pub fn intent_requires_full_verify(&self, intent: &str) -> bool {
        self.require_full_verify_intents.iter().any(|i| i == intent)
    }

    pub fn intent_requires_fuzz(&self, intent: &str) -> bool {
        self.require_fuzz_intents.iter().any(|i| i == intent)
    }

    /// Fold in the escalations contributed by installed `control_policy`
    /// presets.
    ///
    /// Conservative merge, the algebra declared for `policy_preset`: an
    /// escalation can only be added, never removed, so neither a second
    /// extension nor a permissive project file can relax what another already
    /// requires.
    pub fn with_contributed_escalations<'a>(
        mut self,
        escalations: impl IntoIterator<Item = &'a draft_extension_contract::VerificationEscalation>,
    ) -> Self {
        for escalation in escalations {
            let intent = escalation.intent.qualified();
            if escalation.require_full && !self.require_full_verify_intents.contains(&intent) {
                self.require_full_verify_intents.push(intent.clone());
            }
            if escalation.require_exploratory && !self.require_fuzz_intents.contains(&intent) {
                self.require_fuzz_intents.push(intent);
            }
        }
        self.require_full_verify_intents.sort();
        self.require_fuzz_intents.sort();
        self
    }
}

fn load_partial(path: &Path) -> DraftResult<Option<PartialPolicy>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).map_err(|e| {
        DraftError::storage(format!("cannot read policy file {}: {e}", path.display()))
    })?;
    let policy: PartialPolicy = toml::from_str(&text).map_err(|e| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!("invalid policy file {}: {e}", path.display()),
        )
    })?;
    if !crate::contracts::supports_version(
        crate::contracts::ContractId::Policy,
        policy.schema_version,
    ) {
        return Err(DraftError::new(
            DraftErrorKind::UnsupportedSchema,
            format!("policy schema {} is unsupported", policy.schema_version),
        ));
    }
    Ok(Some(policy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_default_fails_closed() {
        let p = Policy::safe_default();
        assert!(p.require_approval_for_promotion);
        assert!(p.block_on_critical_risk);
        assert!(p.require_local_verify_for_imports);
        // No intent escalations of Draft's own: an intent id Core invented
        // would name a vocabulary nothing declares.
        assert!(p.require_full_verify_intents.is_empty());
        assert!(p.require_fuzz_intents.is_empty());
    }

    #[test]
    fn contributed_escalations_only_tighten() {
        use draft_extension_contract::{NamespacedId, VerificationEscalation};
        let intent = |value: &str| NamespacedId::parse(value).unwrap();

        let policy = Policy::safe_default().with_contributed_escalations(&[
            VerificationEscalation {
                intent: intent("draft.software.project/security"),
                require_full: true,
                require_exploratory: true,
            },
            VerificationEscalation {
                intent: intent("draft.software.project/migration"),
                require_full: true,
                require_exploratory: false,
            },
        ]);
        assert!(policy.intent_requires_full_verify("draft.software.project/security"));
        assert!(policy.intent_requires_fuzz("draft.software.project/security"));
        assert!(policy.intent_requires_full_verify("draft.software.project/migration"));
        // Declared, but not for exploratory checks.
        assert!(!policy.intent_requires_fuzz("draft.software.project/migration"));
        // An intent nobody escalated is unaffected.
        assert!(!policy.intent_requires_full_verify("draft.software.project/docs"));

        // A second contributor cannot relax the first: the merge only adds.
        let widened = policy
            .clone()
            .with_contributed_escalations(&[VerificationEscalation {
                intent: intent("draft.software.project/docs"),
                require_full: true,
                require_exploratory: false,
            }]);
        assert!(widened.intent_requires_full_verify("draft.software.project/security"));
        assert!(widened.intent_requires_full_verify("draft.software.project/docs"));
    }

    #[test]
    fn project_overrides_global() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("default-policy.toml");
        let project = tmp.path().join("policy.toml");
        std::fs::write(
            &global,
            "schema_version = 1\nrequire_approval_for_promotion = false\n",
        )
        .unwrap();
        std::fs::write(
            &project,
            "schema_version = 1\nrequire_approval_for_promotion = true\n",
        )
        .unwrap();

        let resolved = Policy::resolve(Some(&project), Some(&global)).unwrap();
        assert!(resolved.require_approval_for_promotion);

        // Only global present → global wins over safe default's field value.
        let resolved = Policy::resolve(None, Some(&global)).unwrap();
        assert!(!resolved.require_approval_for_promotion);
    }

    #[test]
    fn field_level_precedence_project_over_global() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("default-policy.toml");
        let project = tmp.path().join("policy.toml");
        std::fs::write(
            &global,
            "schema_version = 1\nrequire_approval_for_promotion = false\nblock_on_critical_risk = false\n",
        )
        .unwrap();
        // The project layer overrides only one field; the global layer's other
        // field must still apply, and unspecified fields fall to safe default.
        std::fs::write(
            &project,
            "schema_version = 1\nrequire_approval_for_promotion = true\n",
        )
        .unwrap();

        let resolved = Policy::resolve(Some(&project), Some(&global)).unwrap();
        assert!(resolved.require_approval_for_promotion); // project
        assert!(!resolved.block_on_critical_risk); // global
        assert!(resolved.require_local_verify_for_imports); // safe default
    }

    #[test]
    fn unknown_policy_shape_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("policy.toml");
        std::fs::write(
            &project,
            "schema_version = 1\n[unknown]\nrequire_clean_hooks = true\n",
        )
        .unwrap();
        assert!(Policy::resolve(Some(&project), None).is_err());
    }

    #[test]
    fn malformed_policy_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("policy.toml");
        std::fs::write(&project, "require_approval_for_promotion = \"not-a-bool").unwrap();
        assert!(Policy::resolve(Some(&project), None).is_err());
        // Missing files are fine.
        assert!(Policy::resolve(Some(&tmp.path().join("nope.toml")), None).is_ok());
    }

    #[test]
    fn policy_schema_is_required_and_unsupported_versions_are_distinct() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("policy.toml");
        std::fs::write(&project, "require_approval_for_promotion = true\n").unwrap();
        assert_eq!(
            Policy::resolve(Some(&project), None).unwrap_err().kind,
            DraftErrorKind::CorruptData
        );
        std::fs::write(
            &project,
            "schema_version = 2\nrequire_approval_for_promotion = true\n",
        )
        .unwrap();
        assert_eq!(
            Policy::resolve(Some(&project), None).unwrap_err().kind,
            DraftErrorKind::UnsupportedSchema
        );
    }

    fn policy() -> Policy {
        Policy {
            schema_version: crate::contracts::current_version(crate::contracts::ContractId::Policy),
            require_approval_for_promotion: true,
            block_on_critical_risk: true,
            require_approval_on_high_risk: true,
            require_reverify_on_workspace_change: true,
            require_local_verify_for_imports: true,
            require_full_verify_intents: vec!["draft.software.project/security".into()],
            require_fuzz_intents: vec![],
        }
    }

    #[test]
    fn the_same_policy_snapshots_to_the_same_digest() {
        assert_eq!(
            PolicySnapshot::of(policy()).digest().unwrap(),
            PolicySnapshot::of(policy()).digest().unwrap()
        );
    }

    #[test]
    fn relaxing_a_rule_moves_the_digest() {
        // The case that matters: a gate evaluated while approval was required
        // must not still read as satisfied once approval is optional.
        let strict = PolicySnapshot::of(policy());
        let relaxed = Policy {
            require_approval_for_promotion: false,
            ..policy()
        };
        assert_ne!(
            strict.digest().unwrap(),
            PolicySnapshot::of(relaxed.clone()).digest().unwrap()
        );
        assert!(!strict.still_describes(&relaxed).unwrap());
        assert!(strict.still_describes(&policy()).unwrap());
    }

    #[test]
    fn intent_order_is_part_of_the_policy_rather_than_formatting() {
        // Vec order is meaning here, not layout: these are two different
        // configured lists, so they must not collapse to one digest.
        let first = Policy {
            require_full_verify_intents: vec!["a/one".into(), "b/two".into()],
            ..policy()
        };
        let second = Policy {
            require_full_verify_intents: vec!["b/two".into(), "a/one".into()],
            ..policy()
        };
        assert_ne!(
            PolicySnapshot::of(first).digest().unwrap(),
            PolicySnapshot::of(second).digest().unwrap()
        );
    }
}

/// The policy in force at one moment, in canonical form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicySnapshot {
    pub policy: Policy,
}

impl PolicySnapshot {
    pub fn of(policy: Policy) -> Self {
        Self { policy }
    }

    /// This snapshot's digest, as a canonical fact cites it.
    pub fn digest(&self) -> DraftResult<PolicyDigest> {
        let digest = Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;
        Ok(PolicyDigest::new(digest))
    }

    /// Whether this snapshot still describes `current`.
    ///
    /// The question a commit boundary asks: did the rules move since the gate
    /// was evaluated? Answered by digest rather than field comparison so it
    /// stays correct as the policy shape grows.
    pub fn still_describes(&self, current: &Policy) -> DraftResult<bool> {
        Ok(self.digest()? == Self::of(current.clone()).digest()?)
    }
}
