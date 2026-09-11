//! What is waiting for a person.
//!
//! The inbox is a derived view, never a store. Every item is recomputed from
//! the facts that made it — an unsatisfied gate, an unanswered request for
//! changes — so an item cannot outlive its cause. A persisted inbox would
//! develop entries for work already done, and an inbox that lies about what is
//! outstanding is worse than none: people stop reading it.
//!
//! Each item names the action that clears it. An entry saying something is
//! wrong without saying what would resolve it is a notification, not a task,
//! and the difference is whether the reader has to go and work it out.

use serde::{Deserialize, Serialize};

/// One thing awaiting attention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxItem {
    pub schema_version: u32,
    pub id: String,
    pub kind: String,
    pub subject_id: String,
    pub status: String,
    pub summary: String,
    /// The command that resolves this item.
    pub next_action: String,
}

impl crate::contracts::VersionedContract for InboxItem {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::InboxItem;
}

impl InboxItem {
    /// The version an `InboxItem` is written at.
    ///
    /// Read from this item's own contract, not from whichever contract the
    /// item happens to describe. An item stamped with a neighbouring
    /// contract's version reads correctly for exactly as long as the two
    /// versions coincide, and then silently does not.
    pub fn current_schema_version() -> u32 {
        crate::contracts::current_version(crate::contracts::ContractId::InboxItem)
    }
}

fn item(
    id: String,
    kind: &str,
    subject: &str,
    status: &str,
    summary: String,
    next: String,
) -> InboxItem {
    InboxItem {
        schema_version: InboxItem::current_schema_version(),
        id,
        kind: kind.to_string(),
        subject_id: subject.to_string(),
        status: status.to_string(),
        summary,
        next_action: next,
    }
}

/// What one sealed revision's recorded facts say about attention.
///
/// A flat summary rather than the authorization view itself, so the derivation
/// below stays a pure fold with no opinion about where its inputs came from —
/// and so a read model never has to reach up into the application layer that
/// assembles them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionAttention<'a> {
    pub change: &'a str,
    pub revision: &'a str,
    /// An approving decision citing a satisfied gate exists.
    pub approved: bool,
    /// Somebody asked for changes on this exact revision.
    pub changes_requested: bool,
    /// The conditions an unsatisfied gate names, if one is unsatisfied.
    pub unsatisfied_conditions: Vec<String>,
}

/// Derive what one sealed revision is waiting for.
///
/// There is no second workflow store to consult — Evidence, Assessments,
/// Gates and Decisions each bind one revision and never carry, so the graph is
/// the only place an outstanding question can live.
pub fn derive(attention: &RevisionAttention<'_>) -> Vec<InboxItem> {
    let RevisionAttention {
        change,
        revision,
        approved,
        changes_requested,
        unsatisfied_conditions,
    } = attention;
    let (approved, changes_requested) = (*approved, *changes_requested);
    let mut items = Vec::new();

    // A revision somebody asked for changes on, with no later approval, is
    // still a question waiting for an answer.
    if changes_requested && !approved {
        items.push(item(
            format!("inbox:changes_requested:{revision}"),
            "changes_requested",
            change,
            "changes_requested",
            format!("Revision {revision} was returned with requested changes"),
            format!("draft change review {change}"),
        ));
    }

    if !approved {
        // An unsatisfied gate names what is missing; a satisfied one with no
        // decision names who has to look at it. Both are actionable, and the
        // difference is which action clears them.
        if !unsatisfied_conditions.is_empty() {
            items.push(item(
                format!("inbox:gate_unsatisfied:{revision}"),
                "gate_unsatisfied",
                change,
                "blocked",
                format!(
                    "Revision {revision} does not satisfy {}",
                    unsatisfied_conditions.join(", ")
                ),
                format!("draft change gates evaluate {change} {revision}"),
            ));
        } else if !changes_requested {
            items.push(item(
                format!("inbox:change_review:{change}"),
                "change_review",
                change,
                "review_needed",
                format!("Change {change} has a revision awaiting a decision"),
                format!("draft change gates list {change} {revision}"),
            ));
        }
    }

    items.sort_by(|left, right| left.id.cmp(&right.id));
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{current_version, ContractId, VersionedContract};

    #[test]
    fn an_item_is_versioned_by_its_own_contract() {
        // The two contracts agree today, which is exactly why taking the
        // version from the wrong one would go unnoticed until they diverge.
        assert_eq!(
            InboxItem::current_schema_version(),
            current_version(InboxItem::CONTRACT)
        );
        assert_eq!(InboxItem::CONTRACT, ContractId::InboxItem);
    }
}
