//! Conflict detection across provider and Draft metadata sources (Phase 11).

use serde::{Deserialize, Serialize};

use crate::error::DraftResult;
use crate::vcs::traits::VcsRepository;
use crate::vcs::types::{Conflict, ConflictKind, ConflictSet};
use crate::workspace::layout::DraftLayout;

/// Combined conflict report used by status and the finalization gate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictReport {
    pub conflicts: Vec<Conflict>,
}

impl ConflictReport {
    pub fn is_empty(&self) -> bool {
        self.conflicts.is_empty()
    }
    pub fn provider_count(&self) -> usize {
        self.conflicts
            .iter()
            .filter(|c| c.kind == ConflictKind::Provider)
            .count()
    }
}

/// Detect provider conflicts (via the repository) plus Draft metadata conflicts.
pub fn detect(repo: &dyn VcsRepository, layout: &DraftLayout) -> DraftResult<ConflictReport> {
    let mut report = ConflictReport::default();

    // 1. Provider conflicts.
    let provider: ConflictSet = repo.conflicts()?;
    report.conflicts.extend(provider.conflicts);

    // 2. Draft metadata conflict: a present operation-log lock implies another
    //    writer may be mid-append; surface it so callers can wait/abort.
    let op_lock = layout.locks_dir().join("operation-log.lock");
    if op_lock.exists() {
        if let Ok(meta) = std::fs::metadata(&op_lock) {
            if let Ok(modified) = meta.modified() {
                // Only flag fresh locks (stale ones are harmless / taken over).
                if modified
                    .elapsed()
                    .map(|d| d.as_secs() < 30)
                    .unwrap_or(false)
                {
                    report.conflicts.push(Conflict {
                        path: crate::common::WorkspacePath::new(".draft/operations"),
                        kind: ConflictKind::DraftMetadata,
                        message: "another Draft operation is in progress".to_string(),
                    });
                }
            }
        }
    }

    Ok(report)
}
