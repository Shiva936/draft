//! Provider capability declarations.
//!
//! Core branches behavior on **capabilities**, never on provider names
//! (TDD §4.2).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub has_staging_area: bool,
    pub has_mutable_working_change: bool,
    pub has_operation_log: bool,
    pub has_change_ids: bool,
    pub supports_patch_identity: bool,
    pub supports_multiple_workspaces: bool,
    pub supports_local_checkpoints: bool,
    pub supports_history_rewrite: bool,
    pub supports_phases_or_publish_state: bool,
    pub supports_remote_publish: bool,
    pub supports_locks: bool,
    pub supports_partial_checkout: bool,
    pub supports_binary_merge_detection: bool,
    /// Whether the provider can create provider-native finalized objects
    /// (e.g. commits). Experimental providers set this to `false`.
    pub supports_finalization: bool,
}

impl ProviderCapabilities {
    /// A conservative all-false baseline that experimental providers can start
    /// from and flip individual capabilities on.
    pub const NONE: ProviderCapabilities = ProviderCapabilities {
        has_staging_area: false,
        has_mutable_working_change: false,
        has_operation_log: false,
        has_change_ids: false,
        supports_patch_identity: false,
        supports_multiple_workspaces: false,
        supports_local_checkpoints: false,
        supports_history_rewrite: false,
        supports_phases_or_publish_state: false,
        supports_remote_publish: false,
        supports_locks: false,
        supports_partial_checkout: false,
        supports_binary_merge_detection: false,
        supports_finalization: false,
    };

    /// Render capability names that are enabled, for CLI display.
    pub fn enabled_names(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        macro_rules! push_if {
            ($field:ident, $name:literal) => {
                if self.$field {
                    v.push($name);
                }
            };
        }
        push_if!(has_staging_area, "staging-area");
        push_if!(has_mutable_working_change, "mutable-working-change");
        push_if!(has_operation_log, "operation-log");
        push_if!(has_change_ids, "change-ids");
        push_if!(supports_patch_identity, "patch-identity");
        push_if!(supports_multiple_workspaces, "multiple-workspaces");
        push_if!(supports_local_checkpoints, "local-checkpoints");
        push_if!(supports_history_rewrite, "history-rewrite");
        push_if!(supports_phases_or_publish_state, "phases-or-publish");
        push_if!(supports_remote_publish, "remote-publish");
        push_if!(supports_locks, "locks");
        push_if!(supports_partial_checkout, "partial-checkout");
        push_if!(supports_binary_merge_detection, "binary-merge-detection");
        push_if!(supports_finalization, "finalization");
        v
    }
}
