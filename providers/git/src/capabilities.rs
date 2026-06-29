//! Git capability declaration.

use draft_core::vcs::capabilities::ProviderCapabilities;

pub fn git_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        has_staging_area: true,
        has_mutable_working_change: false,
        has_operation_log: false,
        has_change_ids: false,
        supports_patch_identity: false,
        supports_multiple_workspaces: true, // git worktrees
        supports_local_checkpoints: true,   // via stash snapshots
        supports_history_rewrite: true,
        supports_phases_or_publish_state: false,
        supports_remote_publish: true,
        supports_locks: false,
        supports_partial_checkout: true,
        supports_binary_merge_detection: true,
        supports_finalization: true,
    }
}
