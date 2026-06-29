//! Draft-owned, append-only operation log (Phase 5).
//!
//! Independent of any provider-native history (Git reflog, jj op log, ...). The
//! append protocol (Blueprint §11.5): acquire append lock → allocate next seq →
//! write temp → fsync → atomic rename → update index → release lock.

pub mod integrity;
pub mod types;

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::common::{now, OperationId, ReceiptId, WorkspaceId};
use crate::error::{DraftError, DraftErrorKind, DraftResult};
use crate::fsutil::{read_json, write_json};
use crate::identity::ActorRef;
use crate::lock::FileGuard;
use crate::risk::RiskSummary;
use crate::vcs::types::{ProviderId, ProviderView};
use crate::verification::VerificationSummary;
use crate::workspace::layout::DraftLayout;

pub use types::{DraftOperation, ObjectKind, ObjectRef, OperationIntegrity, OperationKind};

/// Inputs for a new operation (everything except seq/id/parents/integrity,
/// which the log assigns).
#[derive(Debug, Clone)]
pub struct NewOperation {
    pub kind: OperationKind,
    pub actor: ActorRef,
    pub provider_id: ProviderId,
    pub observed_provider_view: Option<ProviderView>,
    pub input_refs: Vec<ObjectRef>,
    pub output_refs: Vec<ObjectRef>,
    pub risk_summary: Option<RiskSummary>,
    pub verification_summary: Option<VerificationSummary>,
    pub receipt_refs: Vec<ReceiptId>,
    pub message: Option<String>,
}

impl NewOperation {
    pub fn new(kind: OperationKind, actor: ActorRef, provider_id: ProviderId) -> Self {
        NewOperation {
            kind,
            actor,
            provider_id,
            observed_provider_view: None,
            input_refs: Vec::new(),
            output_refs: Vec::new(),
            risk_summary: None,
            verification_summary: None,
            receipt_refs: Vec::new(),
            message: None,
        }
    }
    pub fn message(mut self, m: impl Into<String>) -> Self {
        self.message = Some(m.into());
        self
    }
    pub fn output(mut self, r: ObjectRef) -> Self {
        self.output_refs.push(r);
        self
    }
    pub fn input(mut self, r: ObjectRef) -> Self {
        self.input_refs.push(r);
        self
    }
    pub fn risk(mut self, r: RiskSummary) -> Self {
        self.risk_summary = Some(r);
        self
    }
    pub fn verification(mut self, v: VerificationSummary) -> Self {
        self.verification_summary = Some(v);
        self
    }
    pub fn receipt(mut self, r: ReceiptId) -> Self {
        self.receipt_refs.push(r);
        self
    }
    pub fn view(mut self, v: ProviderView) -> Self {
        self.observed_provider_view = Some(v);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub seq: u64,
    pub id: OperationId,
    pub kind: OperationKind,
    pub timestamp: crate::common::Timestamp,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperationIndex {
    pub last_seq: u64,
    pub entries: Vec<IndexEntry>,
}

/// The append-only operation log for a workspace.
pub struct OperationLog {
    layout: DraftLayout,
    workspace_id: WorkspaceId,
}

impl OperationLog {
    pub fn new(layout: DraftLayout, workspace_id: WorkspaceId) -> Self {
        OperationLog {
            layout,
            workspace_id,
        }
    }

    fn dir(&self) -> PathBuf {
        self.layout.operations_dir()
    }
    fn index_path(&self) -> PathBuf {
        self.dir().join("index.json")
    }
    fn op_path(&self, seq: u64) -> PathBuf {
        self.dir().join(format!("{seq:016}.operation.json"))
    }
    fn lock_path(&self) -> PathBuf {
        self.layout.locks_dir().join("operation-log.lock")
    }

    fn load_index(&self) -> OperationIndex {
        read_json::<OperationIndex>(&self.index_path()).unwrap_or_default()
    }

    /// Append an operation atomically under the append lock.
    pub fn append(&self, new: NewOperation) -> DraftResult<DraftOperation> {
        let _guard =
            FileGuard::acquire(&self.lock_path(), Duration::from_secs(10)).map_err(|_| {
                DraftError::new(
                    DraftErrorKind::OperationLogLocked,
                    "operation log is locked by another writer",
                )
            })?;

        let mut index = self.load_index();
        let seq = index.last_seq + 1;
        let parent_ids = index
            .entries
            .last()
            .map(|e| vec![e.id.clone()])
            .unwrap_or_default();

        let mut op = DraftOperation {
            id: OperationId::new(format!("op_{seq:016}")),
            seq,
            workspace_id: self.workspace_id.clone(),
            parent_ids,
            actor: new.actor,
            provider_id: new.provider_id,
            observed_provider_view: new.observed_provider_view,
            timestamp: now(),
            kind: new.kind,
            input_refs: new.input_refs,
            output_refs: new.output_refs,
            risk_summary: new.risk_summary,
            verification_summary: new.verification_summary,
            receipt_refs: new.receipt_refs,
            message: new.message,
            integrity: OperationIntegrity {
                algorithm: integrity::ALGORITHM.to_string(),
                content_sha256: String::new(),
            },
        };
        op.integrity = integrity::compute(&op);

        write_json(&self.op_path(seq), &op)?;

        index.last_seq = seq;
        index.entries.push(IndexEntry {
            seq,
            id: op.id.clone(),
            kind: op.kind,
            timestamp: op.timestamp,
        });
        write_json(&self.index_path(), &index)?;

        Ok(op)
    }

    /// Read all operations in sequence order, verifying integrity. A corrupt
    /// record aborts with `OperationLogCorrupt`.
    pub fn read_all(&self) -> DraftResult<Vec<DraftOperation>> {
        let index = self.load_index();
        let mut out = Vec::new();
        for seq in 1..=index.last_seq {
            let path = self.op_path(seq);
            if !path.exists() {
                continue; // tolerate gaps; index rebuild can repair
            }
            let op: DraftOperation = read_json(&path)?;
            if !integrity::verify(&op) {
                return Err(DraftError::new(
                    DraftErrorKind::OperationLogCorrupt,
                    format!("operation {seq} failed integrity check"),
                ));
            }
            out.push(op);
        }
        Ok(out)
    }

    /// Read the most recent `n` operations.
    pub fn read_recent(&self, n: usize) -> DraftResult<Vec<DraftOperation>> {
        let mut all = self.read_all()?;
        let len = all.len();
        if len > n {
            all.drain(0..len - n);
        }
        Ok(all)
    }

    /// Rebuild `index.json` by scanning the operation files (DR-003).
    pub fn rebuild_index(&self) -> DraftResult<OperationIndex> {
        let mut entries = Vec::new();
        let mut last_seq = 0;
        for path in crate::fsutil::list_with_extension(&self.dir(), "json")? {
            if path.file_name().and_then(|n| n.to_str()) == Some("index.json") {
                continue;
            }
            if let Ok(op) = read_json::<DraftOperation>(&path) {
                last_seq = last_seq.max(op.seq);
                entries.push(IndexEntry {
                    seq: op.seq,
                    id: op.id.clone(),
                    kind: op.kind,
                    timestamp: op.timestamp,
                });
            }
        }
        entries.sort_by_key(|e| e.seq);
        let index = OperationIndex { last_seq, entries };
        write_json(&self.index_path(), &index)?;
        Ok(index)
    }

    /// Whether any operation of the given kind exists (used in tests/E2E).
    pub fn contains_kind(&self, kind: OperationKind) -> DraftResult<bool> {
        Ok(self.load_index().entries.iter().any(|e| e.kind == kind))
    }
}
