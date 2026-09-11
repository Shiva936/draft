//! Durable records of what an execution actually ran.

use crate::support::common::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookResult {
    pub hook_name: String,
    pub hook_phase: String,
    pub shell: String,
    pub working_dir: String,
    pub command_hash: String,
    pub exit_code: i32,
    pub stdout_ref: String,
    pub stderr_ref: String,
    pub started_at: Timestamp,
    pub ended_at: Timestamp,
    pub env_keys: Vec<String>,
}
