//! Policy resolution with strict precedence (PRD §9.4, TDD §10.2).
//!
//! Precedence, highest first:
//!   1. project policy (`<root>/.draft/policy.toml`)
//!   2. global default policy (`~/.draft/policies/default-policy.toml`)
//!   3. Draft's built-in safe defaults
//!
//! A policy governs the strict save/import/verify gates. The safe default is
//! deliberately conservative (fail closed): approval is required to save, a
//! critical risk blocks, imports must be locally re-verified, and `security`/
//! `migration` intents require full verification.

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
#[serde(default)]
pub struct Policy {
    /// A save is blocked unless the pack is explicitly approved.
    pub require_approval_for_save: bool,
    /// An unresolved `critical` risk blocks save.
    pub block_on_critical_risk: bool,
    /// A `high` risk pack requires approval before save.
    pub require_approval_on_high_risk: bool,
    /// If the workspace hash changed since verification, re-verify before save.
    pub require_reverify_on_workspace_change: bool,
    /// Imported packs must be locally re-verified before they can be saved.
    pub require_local_verify_for_imports: bool,
    /// Intents that require the full verification suite (not just selection).
    pub require_full_verify_intents: Vec<String>,
    /// Intents that require fuzzing as part of verification.
    pub require_fuzz_intents: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            require_approval_for_save: true,
            block_on_critical_risk: true,
            require_approval_on_high_risk: true,
            require_reverify_on_workspace_change: true,
            require_local_verify_for_imports: true,
            require_full_verify_intents: vec!["security".into(), "migration".into()],
            require_fuzz_intents: vec!["security".into()],
        }
    }
}

/// A partially specified policy layer: only the fields present in the file
/// override lower-precedence layers.
#[derive(Debug, Clone, Default, Deserialize)]
struct PartialPolicy {
    require_approval_for_save: Option<bool>,
    block_on_critical_risk: Option<bool>,
    require_approval_on_high_risk: Option<bool>,
    require_reverify_on_workspace_change: Option<bool>,
    require_local_verify_for_imports: Option<bool>,
    require_full_verify_intents: Option<Vec<String>>,
    require_fuzz_intents: Option<Vec<String>>,
}

impl PartialPolicy {
    fn overlay(self, base: &mut Policy) {
        if let Some(v) = self.require_approval_for_save {
            base.require_approval_for_save = v;
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

    /// Resolve the effective policy from the project and global policy files,
    /// each optional. Present fields in a higher-precedence layer win; absent
    /// files fall through to the next layer and finally the safe default.
    pub fn resolve(project_policy: Option<&Path>, global_policy: Option<&Path>) -> Self {
        // Start from the safe default; overlay global, then project.
        let mut effective = Policy::safe_default();
        if let Some(g) = global_policy.and_then(|p| load_partial(p).ok().flatten()) {
            g.overlay(&mut effective);
        }
        if let Some(p) = project_policy.and_then(|p| load_partial(p).ok().flatten()) {
            p.overlay(&mut effective);
        }
        effective
    }

    /// Like [`Policy::resolve`], but fail closed: an unreadable or unparseable
    /// policy file is an error rather than silently falling through to a
    /// lower-precedence layer.
    pub fn resolve_checked(
        project_policy: Option<&Path>,
        global_policy: Option<&Path>,
    ) -> Result<Self, String> {
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
}

fn load_partial(path: &Path) -> Result<Option<PartialPolicy>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read policy file {}: {e}", path.display()))?;
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| format!("invalid policy file {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_default_fails_closed() {
        let p = Policy::safe_default();
        assert!(p.require_approval_for_save);
        assert!(p.block_on_critical_risk);
        assert!(p.require_local_verify_for_imports);
        assert!(p.intent_requires_full_verify("security"));
        assert!(p.intent_requires_fuzz("security"));
        assert!(!p.intent_requires_full_verify("docs"));
    }

    #[test]
    fn project_overrides_global() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("default-policy.toml");
        let project = tmp.path().join("policy.toml");
        std::fs::write(&global, "require_approval_for_save = false\n").unwrap();
        std::fs::write(&project, "require_approval_for_save = true\n").unwrap();

        let resolved = Policy::resolve(Some(&project), Some(&global));
        assert!(resolved.require_approval_for_save);

        // Only global present → global wins over safe default's field value.
        let resolved = Policy::resolve(None, Some(&global));
        assert!(!resolved.require_approval_for_save);
    }

    #[test]
    fn field_level_precedence_project_over_global() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("default-policy.toml");
        let project = tmp.path().join("policy.toml");
        std::fs::write(
            &global,
            "require_approval_for_save = false\nblock_on_critical_risk = false\n",
        )
        .unwrap();
        // The project layer overrides only one field; the global layer's other
        // field must still apply, and unspecified fields fall to safe default.
        std::fs::write(&project, "require_approval_for_save = true\n").unwrap();

        let resolved = Policy::resolve(Some(&project), Some(&global));
        assert!(resolved.require_approval_for_save); // project
        assert!(!resolved.block_on_critical_risk); // global
        assert!(resolved.require_local_verify_for_imports); // safe default
        assert!(resolved.intent_requires_fuzz("security")); // safe default
    }

    #[test]
    fn legacy_policy_config_resolves_to_safe_default() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("policy.toml");
        // Legacy `[save]`-table style file: no canonical keys present.
        std::fs::write(&project, "[save]\nrequire_clean_hooks = true\n").unwrap();
        let resolved = Policy::resolve_checked(Some(&project), None).unwrap();
        assert_eq!(resolved, Policy::safe_default());
    }

    #[test]
    fn malformed_policy_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("policy.toml");
        std::fs::write(&project, "require_approval_for_save = \"not-a-bool").unwrap();
        assert!(Policy::resolve_checked(Some(&project), None).is_err());
        // Missing files are fine.
        assert!(Policy::resolve_checked(Some(&tmp.path().join("nope.toml")), None).is_ok());
    }
}
