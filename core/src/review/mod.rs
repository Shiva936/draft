//! Review sessions and decisions (FR-REV-001/002/003).

use serde::{Deserialize, Serialize};

use crate::common::{now, DraftChangeId, ReviewSessionId, Timestamp, WorkspaceId};
use crate::error::DraftResult;
use crate::fsutil::{list_with_extension, read_json, write_json};
use crate::identity::ActorRef;
use crate::workspace::layout::DraftLayout;

/// Per-change review state (stored on each [`crate::changes::DraftChange`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewState {
    Pending,
    Approved,
    Rejected,
    Deferred,
    NeedsChanges,
}

impl ReviewState {
    pub fn label(&self) -> &'static str {
        match self {
            ReviewState::Pending => "pending",
            ReviewState::Approved => "approved",
            ReviewState::Rejected => "rejected",
            ReviewState::Deferred => "deferred",
            ReviewState::NeedsChanges => "needs-changes",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewDecisionKind {
    Approved,
    Rejected,
    Deferred,
    NeedsChanges,
}

impl From<ReviewDecisionKind> for ReviewState {
    fn from(k: ReviewDecisionKind) -> Self {
        match k {
            ReviewDecisionKind::Approved => ReviewState::Approved,
            ReviewDecisionKind::Rejected => ReviewState::Rejected,
            ReviewDecisionKind::Deferred => ReviewState::Deferred,
            ReviewDecisionKind::NeedsChanges => ReviewState::NeedsChanges,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewDecision {
    pub change_id: DraftChangeId,
    pub kind: ReviewDecisionKind,
    pub comment: Option<String>,
    pub actor: ActorRef,
    pub decided_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSession {
    pub id: ReviewSessionId,
    pub workspace_id: WorkspaceId,
    pub change_ids: Vec<DraftChangeId>,
    pub actor: ActorRef,
    pub started_at: Timestamp,
    pub completed_at: Option<Timestamp>,
    pub decisions: Vec<ReviewDecision>,
}

impl ReviewSession {
    pub fn start(
        workspace_id: WorkspaceId,
        change_ids: Vec<DraftChangeId>,
        actor: ActorRef,
    ) -> Self {
        ReviewSession {
            id: ReviewSessionId::generate(),
            workspace_id,
            change_ids,
            actor,
            started_at: now(),
            completed_at: None,
            decisions: Vec::new(),
        }
    }

    pub fn record(
        &mut self,
        change_id: DraftChangeId,
        kind: ReviewDecisionKind,
        comment: Option<String>,
        actor: ActorRef,
    ) {
        self.decisions.push(ReviewDecision {
            change_id,
            kind,
            comment,
            actor,
            decided_at: now(),
        });
    }

    /// The latest decision recorded for a given change, if any.
    pub fn latest_for(&self, change_id: &DraftChangeId) -> Option<&ReviewDecision> {
        self.decisions
            .iter()
            .rev()
            .find(|d| &d.change_id == change_id)
    }

    pub fn path(&self, layout: &DraftLayout) -> std::path::PathBuf {
        layout
            .reviews_dir()
            .join(format!("review_{}.json", self.id))
    }

    pub fn save(&self, layout: &DraftLayout) -> DraftResult<()> {
        write_json(&self.path(layout), self)
    }
}

/// Load the most recent review session, if any.
pub fn latest(layout: &DraftLayout) -> DraftResult<Option<ReviewSession>> {
    let mut latest: Option<ReviewSession> = None;
    for p in list_with_extension(&layout.reviews_dir(), "json")? {
        if let Ok(s) = read_json::<ReviewSession>(&p) {
            match &latest {
                None => latest = Some(s),
                Some(cur) if s.started_at > cur.started_at => latest = Some(s),
                _ => {}
            }
        }
    }
    Ok(latest)
}
