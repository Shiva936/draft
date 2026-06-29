//! Finalization engine (Phase 8) — converts reviewed Draft changes into a
//! provider-native object (e.g. a Git commit). Replaces the v0.1.0 commit
//! engine. `draft commit` routes here.

pub mod policy;

use serde::{Deserialize, Serialize};

use crate::changes::DraftChange;
use crate::common::{
    now, CheckpointId, DraftChangeId, FinalizationPlanId, FinalizationResultId, Timestamp,
};
use crate::conflict::ConflictReport;
use crate::error::{DraftError, DraftErrorKind, DraftResult};
use crate::identity::ActorRef;
use crate::operations::{NewOperation, ObjectKind, ObjectRef, OperationKind, OperationLog};
use crate::receipts::{self, DraftReceipt, UndoHint};
use crate::review::ReviewState;
use crate::risk::RiskSummary;
use crate::vcs::traits::VcsRepository;
use crate::vcs::types::{ProviderFinalizationInput, ProviderFinalizationPlan, ProviderObjectRef};
use crate::verification::VerificationSummary;
use crate::workspace::Workspace;

pub use policy::{evaluate_policy, FinalizationPolicyResult, PolicyBlock};

/// Compact finalization summary embedded in receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizationSummary {
    pub object: Option<ProviderObjectRef>,
    pub change_count: usize,
    pub message_title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizationPlan {
    pub id: FinalizationPlanId,
    pub workspace_id: crate::common::WorkspaceId,
    pub provider_id: crate::vcs::types::ProviderId,
    pub change_ids: Vec<DraftChangeId>,
    pub message: String,
    pub policy_result: FinalizationPolicyResult,
    pub provider_plan: ProviderFinalizationPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizationResult {
    pub id: FinalizationResultId,
    pub plan_id: FinalizationPlanId,
    pub object: ProviderObjectRef,
    pub receipt_id: crate::common::ReceiptId,
    pub completed_at: Timestamp,
}

/// Everything the engine needs, gathered by the orchestration layer.
pub struct FinalizationContext<'a> {
    pub ws: &'a Workspace,
    pub repo: &'a dyn VcsRepository,
    pub log: &'a OperationLog,
    pub actor: ActorRef,
    pub changes: Vec<DraftChange>,
    pub risk: RiskSummary,
    pub verification: Option<VerificationSummary>,
    pub conflicts: ConflictReport,
    pub checkpoint: Option<CheckpointId>,
}

#[derive(Debug, Clone)]
pub struct FinalizationRequest {
    pub message: String,
    pub trailers: Vec<String>,
    /// User explicitly confirmed proceeding despite high risk.
    pub confirm_high_risk: bool,
}

/// Build a finalization plan: gather included paths, run policy gates, and ask
/// the provider to prepare (but not execute) finalization. Appends
/// `FinalizationPlanned`.
pub fn build_plan(
    ctx: &FinalizationContext,
    request: &FinalizationRequest,
) -> DraftResult<FinalizationPlan> {
    if request.message.trim().is_empty() {
        return Err(DraftError::new(
            DraftErrorKind::FinalizationFailed,
            "finalization message cannot be empty",
        ));
    }

    let policy_result = evaluate_policy(
        &ctx.ws.config.finalization,
        &ctx.changes,
        &ctx.risk,
        ctx.verification.as_ref(),
        &ctx.conflicts,
        request.confirm_high_risk,
    );

    let include_paths: Vec<_> = ctx.changes.iter().flat_map(|c| c.paths()).collect();
    if include_paths.is_empty() {
        return Err(DraftError::new(
            DraftErrorKind::FinalizationFailed,
            "no file changes to finalize",
        ));
    }

    let provider_plan = ctx.repo.prepare_finalization(ProviderFinalizationInput {
        include_paths,
        message: request.message.clone(),
        trailers: request.trailers.clone(),
    })?;

    let plan = FinalizationPlan {
        id: FinalizationPlanId::generate(),
        workspace_id: ctx.ws.id.clone(),
        provider_id: ctx.ws.provider_id.clone(),
        change_ids: ctx.changes.iter().map(|c| c.id.clone()).collect(),
        message: request.message.clone(),
        policy_result,
        provider_plan,
    };

    ctx.log.append(
        NewOperation::new(
            OperationKind::FinalizationPlanned,
            ctx.actor.clone(),
            ctx.ws.provider_id.clone(),
        )
        .risk(ctx.risk.clone())
        .message(plan.message.clone()),
    )?;

    Ok(plan)
}

/// Execute a previously built plan: enforce policy, run provider finalize,
/// create a receipt, and append `FinalizationCompleted`.
pub fn execute(
    ctx: &FinalizationContext,
    plan: FinalizationPlan,
) -> DraftResult<FinalizationResult> {
    // Hard gate: refuse to execute a blocked plan.
    if let Some(block) = plan.policy_result.first_block() {
        return Err(DraftError::new(block.kind, block.reason.clone())
            .with_suggestion(block.suggestion.clone()));
    }

    let provider_result = ctx.repo.finalize(plan.provider_plan.clone())?;

    // Build and persist the receipt mapping Draft changes -> provider object.
    let mut receipt = DraftReceipt::builder(
        ctx.ws.id.clone(),
        ctx.ws.provider_id.clone(),
        ctx.actor.clone(),
    );
    receipt.change_ids = plan.change_ids.clone();
    receipt.provider_objects = vec![provider_result.object.clone()];
    receipt.risk_summary = Some(ctx.risk.clone());
    receipt.verification_summary = ctx.verification.clone();
    receipt.finalization_summary = Some(FinalizationSummary {
        object: Some(provider_result.object.clone()),
        change_count: plan.change_ids.len(),
        message_title: plan.message.lines().next().unwrap_or("").to_string(),
    });
    if let Some(cp) = &ctx.checkpoint {
        receipt.checkpoint_refs = vec![cp.clone()];
    }
    receipt.undo_hint = Some(UndoHint {
        description: "run `draft undo` to reverse this finalization".to_string(),
        provider_object: Some(provider_result.object.clone()),
        checkpoint: ctx.checkpoint.clone(),
    });
    receipts::create(&ctx.ws.layout(), &receipt)?;

    let op = ctx.log.append(
        NewOperation::new(
            OperationKind::FinalizationCompleted,
            ctx.actor.clone(),
            ctx.ws.provider_id.clone(),
        )
        .output(ObjectRef {
            kind: ObjectKind::ProviderObject,
            id: provider_result.object.object_id.to_string(),
            provider_id: Some(ctx.ws.provider_id.clone()),
        })
        .receipt(receipt.id.clone())
        .verification_opt(ctx.verification.clone())
        .message(plan.message.clone()),
    )?;
    let _ = op;

    Ok(FinalizationResult {
        id: FinalizationResultId::generate(),
        plan_id: plan.id,
        object: provider_result.object,
        receipt_id: receipt.id,
        completed_at: now(),
    })
}

/// Convenience: plan then execute, failing if policy blocks.
pub fn finalize(
    ctx: &FinalizationContext,
    request: &FinalizationRequest,
) -> DraftResult<(FinalizationPlan, FinalizationResult)> {
    let plan = build_plan(ctx, request)?;
    let result = execute(ctx, plan.clone())?;
    Ok((plan, result))
}

/// Count of changes whose review state is `Approved`.
pub fn approved_count(changes: &[DraftChange]) -> usize {
    changes
        .iter()
        .filter(|c| c.review_state == ReviewState::Approved)
        .count()
}

// Small extension so the engine can attach an optional verification summary
// without an awkward `match` at the call site.
trait NewOperationExt {
    fn verification_opt(self, v: Option<VerificationSummary>) -> Self;
}
impl NewOperationExt for NewOperation {
    fn verification_opt(mut self, v: Option<VerificationSummary>) -> Self {
        if let Some(v) = v {
            self.verification_summary = Some(v);
        }
        self
    }
}
