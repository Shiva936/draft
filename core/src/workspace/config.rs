//! Workspace configuration persisted to `.draft/config.toml`.

use serde::{Deserialize, Serialize};

use crate::vcs::types::ProviderId;

/// Top-level workspace configuration (TDD §6.1, §13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub version: String,
    pub provider: ProviderBinding,
    #[serde(default)]
    pub verification: VerificationConfig,
    #[serde(default)]
    pub risk: RiskConfig,
    #[serde(default)]
    pub finalization: FinalizationConfig,
}

impl WorkspaceConfig {
    pub fn new(provider_id: ProviderId) -> Self {
        WorkspaceConfig {
            version: crate::DRAFT_VERSION.to_string(),
            provider: ProviderBinding {
                provider_id,
                experimental_ack: false,
            },
            verification: VerificationConfig::default(),
            risk: RiskConfig::default(),
            finalization: FinalizationConfig::default(),
        }
    }
}

/// Binds a workspace to a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderBinding {
    pub provider_id: ProviderId,
    /// User acknowledged using an experimental provider.
    #[serde(default)]
    pub experimental_ack: bool,
}

/// Configured verification commands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationConfig {
    #[serde(default)]
    pub commands: Vec<VerificationCommandConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationCommandConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Risk engine tuning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskConfig {
    pub detect_secrets: bool,
    pub large_diff_threshold_lines: usize,
    pub deletion_threshold_files: usize,
    pub many_files_threshold: usize,
}

impl Default for RiskConfig {
    fn default() -> Self {
        RiskConfig {
            detect_secrets: true,
            large_diff_threshold_lines: 1000,
            deletion_threshold_files: 10,
            many_files_threshold: 25,
        }
    }
}

/// Finalization policy gates (FR-FIN-003).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizationConfig {
    pub require_review: bool,
    pub require_verification: bool,
    pub block_on_high_risk: bool,
    pub allow_unverified: bool,
    pub allow_high_risk_with_confirmation: bool,
}

impl Default for FinalizationConfig {
    fn default() -> Self {
        // Conservative but usable defaults: preserve v0.1.0 ergonomics
        // (verification not strictly required) while structuring the gates.
        FinalizationConfig {
            require_review: false,
            require_verification: false,
            block_on_high_risk: true,
            allow_unverified: true,
            allow_high_risk_with_confirmation: true,
        }
    }
}
