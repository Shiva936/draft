//! Durable receipts for important Draft actions, especially finalization
//! (Phase 9, FR-RCP-001/002/003).

use serde::{Deserialize, Serialize};

use crate::common::{
    now, CheckpointId, DraftChangeId, OperationId, ReceiptId, Timestamp, WorkspaceId,
};
use crate::error::{DraftError, DraftErrorKind, DraftResult};
use crate::finalization::FinalizationSummary;
use crate::fsutil::{list_with_extension, read_json, write_json};
use crate::identity::ActorRef;
use crate::risk::RiskSummary;
use crate::vcs::types::{ProviderId, ProviderObjectRef};
use crate::verification::VerificationSummary;
use crate::workspace::layout::DraftLayout;

/// A hint describing how a finalized action could be undone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoHint {
    pub description: String,
    pub provider_object: Option<ProviderObjectRef>,
    pub checkpoint: Option<CheckpointId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftReceipt {
    pub id: ReceiptId,
    pub draft_version: String,
    pub workspace_id: WorkspaceId,
    pub provider_id: ProviderId,
    pub actor: ActorRef,
    pub operation_ids: Vec<OperationId>,
    pub change_ids: Vec<DraftChangeId>,
    pub provider_objects: Vec<ProviderObjectRef>,
    pub risk_summary: Option<RiskSummary>,
    pub verification_summary: Option<VerificationSummary>,
    pub finalization_summary: Option<FinalizationSummary>,
    pub checkpoint_refs: Vec<CheckpointId>,
    pub undo_hint: Option<UndoHint>,
    pub created_at: Timestamp,
}

impl DraftReceipt {
    pub fn builder(
        workspace_id: WorkspaceId,
        provider_id: ProviderId,
        actor: ActorRef,
    ) -> DraftReceipt {
        DraftReceipt {
            id: ReceiptId::generate(),
            draft_version: crate::DRAFT_VERSION.to_string(),
            workspace_id,
            provider_id,
            actor,
            operation_ids: vec![],
            change_ids: vec![],
            provider_objects: vec![],
            risk_summary: None,
            verification_summary: None,
            finalization_summary: None,
            checkpoint_refs: vec![],
            undo_hint: None,
            created_at: now(),
        }
    }

    /// Render a human-readable view (FR-RCP-003).
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("Receipt {}\n", self.id));
        s.push_str(&format!("  workspace: {}\n", self.workspace_id));
        s.push_str(&format!("  provider:  {}\n", self.provider_id));
        s.push_str(&format!(
            "  actor:     {} ({})\n",
            self.actor.display_name,
            self.actor.kind.label()
        ));
        s.push_str(&format!("  created:   {}\n", self.created_at.to_rfc3339()));
        if !self.change_ids.is_empty() {
            s.push_str(&format!(
                "  changes:   {}\n",
                self.change_ids
                    .iter()
                    .map(|c| c.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for obj in &self.provider_objects {
            s.push_str(&format!(
                "  {} -> {}{}\n",
                obj.kind,
                obj.object_id,
                obj.label
                    .as_ref()
                    .map(|l| format!(" ({l})"))
                    .unwrap_or_default()
            ));
        }
        if let Some(r) = &self.risk_summary {
            s.push_str(&format!("  risk:      {}\n", r.level.label()));
        }
        if let Some(v) = &self.verification_summary {
            s.push_str(&format!("  verify:    {}\n", v.status.label()));
        }
        if let Some(u) = &self.undo_hint {
            s.push_str(&format!("  undo:      {}\n", u.description));
        }
        s
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReceiptIndex {
    pub entries: Vec<ReceiptIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptIndexEntry {
    pub id: ReceiptId,
    pub created_at: Timestamp,
    pub provider_objects: Vec<String>,
}

fn receipt_path(layout: &DraftLayout, id: &ReceiptId) -> std::path::PathBuf {
    layout.receipts_dir().join(format!("receipt_{id}.json"))
}

fn index_path(layout: &DraftLayout) -> std::path::PathBuf {
    layout.receipts_dir().join("index.json")
}

/// Persist a receipt and update the index.
pub fn create(layout: &DraftLayout, receipt: &DraftReceipt) -> DraftResult<()> {
    write_json(&receipt_path(layout, &receipt.id), receipt)
        .map_err(|e| DraftError::new(DraftErrorKind::ReceiptWriteFailed, e.message))?;
    let mut index: ReceiptIndex = read_json(&index_path(layout)).unwrap_or_default();
    index.entries.push(ReceiptIndexEntry {
        id: receipt.id.clone(),
        created_at: receipt.created_at,
        provider_objects: receipt
            .provider_objects
            .iter()
            .map(|o| o.object_id.to_string())
            .collect(),
    });
    write_json(&index_path(layout), &index)?;
    Ok(())
}

pub fn load(layout: &DraftLayout, id: &ReceiptId) -> DraftResult<DraftReceipt> {
    read_json(&receipt_path(layout, id))
        .map_err(|_| DraftError::not_found(format!("receipt {id} not found")))
}

pub fn list(layout: &DraftLayout) -> DraftResult<Vec<DraftReceipt>> {
    let mut out = Vec::new();
    for p in list_with_extension(&layout.receipts_dir(), "json")? {
        if p.file_name().and_then(|n| n.to_str()) == Some("index.json") {
            continue;
        }
        if let Ok(r) = read_json::<DraftReceipt>(&p) {
            out.push(r);
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    Ok(out)
}

/// Rebuild the receipt index from the receipt files (DR-003).
pub fn rebuild_index(layout: &DraftLayout) -> DraftResult<ReceiptIndex> {
    let mut index = ReceiptIndex::default();
    for r in list(layout)? {
        index.entries.push(ReceiptIndexEntry {
            id: r.id.clone(),
            created_at: r.created_at,
            provider_objects: r
                .provider_objects
                .iter()
                .map(|o| o.object_id.to_string())
                .collect(),
        });
    }
    write_json(&index_path(layout), &index)?;
    Ok(index)
}
