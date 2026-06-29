//! Finalization policy gates (FR-FIN-003).

use serde::{Deserialize, Serialize};

use crate::changes::DraftChange;
use crate::conflict::ConflictReport;
use crate::error::DraftErrorKind;
use crate::review::ReviewState;
use crate::risk::{RiskLevel, RiskSummary};
use crate::verification::{VerificationStatus, VerificationSummary};
use crate::workspace::config::FinalizationConfig;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyBlock {
    pub kind: DraftErrorKind,
    pub reason: String,
    pub suggestion: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizationPolicyResult {
    pub allowed: bool,
    pub blocks: Vec<PolicyBlock>,
    pub warnings: Vec<String>,
}

impl FinalizationPolicyResult {
    pub fn first_block(&self) -> Option<&PolicyBlock> {
        self.blocks.first()
    }
}

/// Evaluate all gates. `confirm_high_risk` reflects an explicit user
/// confirmation to proceed despite a high/critical risk finding.
pub fn evaluate_policy(
    config: &FinalizationConfig,
    changes: &[DraftChange],
    risk: &RiskSummary,
    verification: Option<&VerificationSummary>,
    conflicts: &ConflictReport,
    confirm_high_risk: bool,
) -> FinalizationPolicyResult {
    let mut blocks = Vec::new();
    let mut warnings = Vec::new();

    // Conflicts always block.
    if !conflicts.is_empty() {
        blocks.push(PolicyBlock {
            kind: DraftErrorKind::ConflictDetected,
            reason: format!(
                "{} unresolved conflict(s) present",
                conflicts.conflicts.len()
            ),
            suggestion: "Resolve conflicts, then retry finalization.".to_string(),
        });
    }

    // Review gate.
    if config.require_review {
        let unapproved = changes
            .iter()
            .filter(|c| c.review_state != ReviewState::Approved)
            .count();
        if unapproved > 0 {
            blocks.push(PolicyBlock {
                kind: DraftErrorKind::ReviewRequired,
                reason: format!("{unapproved} change(s) are not approved"),
                suggestion: "Run `draft review` and approve the changes.".to_string(),
            });
        }
    }

    // Verification gate.
    if config.require_verification {
        match verification {
            Some(v) if v.status == VerificationStatus::Passed => {}
            Some(v) => blocks.push(PolicyBlock {
                kind: DraftErrorKind::VerificationFailed,
                reason: format!("verification status is {}", v.status.label()),
                suggestion: "Fix the failing checks and run `draft verify` again.".to_string(),
            }),
            None if config.allow_unverified => {
                warnings.push("finalizing without verification".to_string());
            }
            None => blocks.push(PolicyBlock {
                kind: DraftErrorKind::VerificationFailed,
                reason: "no verification has been run".to_string(),
                suggestion: "Run `draft verify` before finalizing.".to_string(),
            }),
        }
    }

    // Risk gate.
    if config.block_on_high_risk && risk.level >= RiskLevel::High {
        if config.allow_high_risk_with_confirmation && confirm_high_risk {
            warnings.push(format!(
                "proceeding despite {} risk (confirmed)",
                risk.level.label()
            ));
        } else {
            blocks.push(PolicyBlock {
                kind: DraftErrorKind::RiskPolicyBlocked,
                reason: format!("risk level is {}", risk.level.label()),
                suggestion: if config.allow_high_risk_with_confirmation {
                    "Re-run with explicit confirmation (e.g. `--yes`/`--allow-high-risk`)."
                        .to_string()
                } else {
                    "Reduce the risk of this change before finalizing.".to_string()
                },
            });
        }
    }

    FinalizationPolicyResult {
        allowed: blocks.is_empty(),
        blocks,
        warnings,
    }
}
