//! Actor identity types (FR-ID-001).

use serde::{Deserialize, Serialize};

use crate::common::ActorId;

/// What kind of entity performed a Draft action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActorKind {
    Human,
    Agent,
    Service,
    Unknown,
}

impl ActorKind {
    pub fn label(&self) -> &'static str {
        match self {
            ActorKind::Human => "human",
            ActorKind::Agent => "agent",
            ActorKind::Service => "service",
            ActorKind::Unknown => "unknown",
        }
    }
}

/// A reference to an actor, embedded in operations and receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorRef {
    pub id: ActorId,
    pub kind: ActorKind,
    pub display_name: String,
}

impl ActorRef {
    pub fn unknown() -> Self {
        ActorRef {
            id: ActorId::new("act_unknown"),
            kind: ActorKind::Unknown,
            display_name: "unknown".to_string(),
        }
    }

    pub fn service() -> Self {
        ActorRef {
            id: ActorId::new("act_service"),
            kind: ActorKind::Service,
            display_name: "draftd".to_string(),
        }
    }
}
