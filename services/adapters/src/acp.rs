//! ACP adapter — approval-workflow operations (TDD §42).
//!
//! Approvals and rejections flow through core (`cockpit_decide`), so they emit
//! signed PackApproved/PackRejected receipts and update the canonical manifest.
//! Nothing here bypasses the strict save/verify gates.

use draft_core::App;
use serde_json::{json, Value};
use std::path::Path;

/// An ACP operation.
pub enum AcpOp<'a> {
    /// Show the evidence a reviewer needs to decide on a pack.
    RequestApproval { pack_id: &'a str },
    /// Approve a pack (emits a signed PackApproved receipt).
    Approve {
        pack_id: &'a str,
        reason: Option<String>,
    },
    /// Reject a pack (emits a signed PackRejected receipt).
    Reject {
        pack_id: &'a str,
        reason: Option<String>,
    },
    /// List packs awaiting approval (verified but not yet approved).
    ListPending,
}

/// Run an ACP operation against the workspace at `root`.
pub fn run(root: &Path, op: AcpOp<'_>) -> Result<Value, String> {
    let _ = crate::ensure_adapter_config("acp");
    let app = App::new();
    match op {
        AcpOp::RequestApproval { pack_id } => {
            let report = app.pack_inspect(root, pack_id).map_err(|e| e.message)?;
            serde_json::to_value(report).map_err(|e| e.to_string())
        }
        AcpOp::Approve { pack_id, reason } => {
            let id = app
                .cockpit_decide(root, pack_id, true, reason)
                .map_err(|e| e.message)?;
            Ok(json!({"approved": id}))
        }
        AcpOp::Reject { pack_id, reason } => {
            let id = app
                .cockpit_decide(root, pack_id, false, reason)
                .map_err(|e| e.message)?;
            Ok(json!({"rejected": id}))
        }
        AcpOp::ListPending => {
            let pending: Vec<Value> = app
                .list_canonical_packs(root)
                .map_err(|e| e.message)?
                .into_iter()
                .filter(|m| {
                    m.is_verified()
                        && matches!(m.approval_state, draft_core::pack::ApprovalState::Pending)
                })
                .map(|m| json!({"pack_id": m.pack_id, "name": m.name, "intent": m.intent}))
                .collect();
            Ok(json!({"pending": pending}))
        }
    }
}
