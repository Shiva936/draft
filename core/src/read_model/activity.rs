//! Reading the Activity Ledger.
//!
//! Every surface that shows what happened — the CLI, the daemon, the Console,
//! Doctor — folds the ledger through here. The projection is deliberately flat
//! and stable: a stored [`LedgerRecord`] carries an opaque payload, and letting
//! each surface reach into that payload with its own field names is how one
//! event ends up rendered three different ways.
//!
//! Reading never appends. This module takes an [`ActivityReader`], not an
//! `ActivityLog`, so a read path cannot write an event by accident (§2.30).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::activity::{ActivityReader, LedgerRecord};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// One Activity event, as every surface sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub event_id: String,
    /// The frozen v1 vocabulary name.
    pub kind: String,
    /// The graph object the event is about, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub actor: String,
    /// When the fact this event records became durable, in nanoseconds since
    /// the epoch. Zero when a record predates the field.
    pub recorded_at: i64,
    pub metadata: Value,
    /// This record's own chain hash.
    pub record_hash: String,
    /// The record it links onto.
    pub previous_hash: String,
}

impl ActivityEntry {
    fn from_record(record: &LedgerRecord) -> Self {
        let field = |name: &str| {
            record
                .payload
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        Self {
            event_id: record.event_id.clone(),
            kind: field("kind").unwrap_or_default(),
            subject: field("subject"),
            actor: field("actor").unwrap_or_default(),
            recorded_at: record
                .payload
                .get("recorded_at")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            metadata: record
                .payload
                .get("metadata")
                .cloned()
                .unwrap_or(Value::Null),
            record_hash: record.record_hash.clone(),
            previous_hash: record.previous_hash.clone(),
        }
    }
}

/// Every event, oldest first.
pub fn entries(reader: &dyn ActivityReader) -> DraftResult<Vec<ActivityEntry>> {
    Ok(reader
        .records()?
        .iter()
        .map(ActivityEntry::from_record)
        .collect())
}

/// One page of Activity.
///
/// `newest_first` decides which end the page is taken from; `filter` matches
/// the event kind and the subject, which is what a person searching a ledger
/// actually has to hand.
pub fn page(
    reader: &dyn ActivityReader,
    newest_first: bool,
    page: Option<usize>,
    limit: Option<usize>,
    filter: Option<&str>,
) -> DraftResult<Vec<ActivityEntry>> {
    let mut all = entries(reader)?;
    if let Some(needle) = filter {
        let needle = needle.to_lowercase();
        all.retain(|entry| {
            entry.kind.to_lowercase().contains(&needle)
                || entry
                    .subject
                    .as_deref()
                    .is_some_and(|subject| subject.to_lowercase().contains(&needle))
        });
    }
    if newest_first {
        all.reverse();
    }
    let limit = limit.unwrap_or(all.len());
    let skip = page.unwrap_or(0).saturating_mul(limit);
    Ok(all.into_iter().skip(skip).take(limit).collect())
}

/// One event by id.
pub fn entry(reader: &dyn ActivityReader, event_id: &str) -> DraftResult<ActivityEntry> {
    entries(reader)?
        .into_iter()
        .find(|entry| entry.event_id == event_id)
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                format!("no Activity event '{event_id}'"),
            )
            .with_suggestion("list events with `draft activity list`")
        })
}

/// What replaying the ledger in memory establishes.
///
/// An in-memory check, never a rewrite: `draft doctor activity --replay`
/// answers "does what is stored still hold together", and repairing an index
/// is `draft maintenance index-rebuild`'s job instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityReplay {
    pub project: String,
    pub events: usize,
    pub by_kind: BTreeMap<String, usize>,
    pub chain_ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Fold the ledger into a replay report.
pub fn replay(
    reader: &dyn ActivityReader,
    project: &str,
    chain: DraftResult<usize>,
) -> DraftResult<ActivityReplay> {
    let events = entries(reader)?;
    let mut by_kind = BTreeMap::new();
    for event in &events {
        *by_kind.entry(event.kind.clone()).or_insert(0usize) += 1;
    }
    let (chain_ok, error) = match chain {
        Ok(_) => (true, None),
        Err(error) => (false, Some(error.message.clone())),
    };
    Ok(ActivityReplay {
        project: project.to_string(),
        events: events.len(),
        by_kind,
        chain_ok,
        error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::ActivityLog;
    use serde_json::json;

    fn log(directory: &tempfile::TempDir) -> ActivityLog {
        ActivityLog::new(directory.path().join("events"), "prj_000000000001")
    }

    // The payload shape `app/activity.rs` writes. Written out here rather than
    // called, because a read model must not reach up into the append path to
    // test itself.
    fn append(log: &ActivityLog, id: &str, kind: crate::activity::EventKind, subject: &str) {
        log.append(
            id,
            &json!({
                "kind": kind.as_str(),
                "subject": subject,
                "actor": "act_000000000001",
                "recorded_at": 0,
                "metadata": { "note": subject },
            }),
        )
        .unwrap();
    }

    #[test]
    fn an_entry_carries_the_vocabulary_name_not_the_raw_payload() {
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        append(
            &log,
            "evt_000000000001",
            crate::activity::EventKind::ChangeCreated,
            "chg_1",
        );

        let entry = entry(&log, "evt_000000000001").unwrap();
        assert_eq!(entry.kind, "ChangeCreated");
        assert_eq!(entry.subject.as_deref(), Some("chg_1"));
        assert_eq!(entry.actor, "act_000000000001");
        assert_eq!(entry.metadata, json!({ "note": "chg_1" }));
    }

    #[test]
    fn a_page_is_taken_from_the_end_the_caller_asked_for() {
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        append(
            &log,
            "evt_000000000001",
            crate::activity::EventKind::ChangeCreated,
            "chg_1",
        );
        append(
            &log,
            "evt_000000000002",
            crate::activity::EventKind::RevisionSealed,
            "chg_2",
        );

        let newest = page(&log, true, None, Some(1), None).unwrap();
        assert_eq!(newest[0].event_id, "evt_000000000002");
        let oldest = page(&log, false, None, Some(1), None).unwrap();
        assert_eq!(oldest[0].event_id, "evt_000000000001");
    }

    #[test]
    fn a_filter_matches_the_kind_or_the_subject() {
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        append(
            &log,
            "evt_000000000001",
            crate::activity::EventKind::ChangeCreated,
            "chg_1",
        );
        append(
            &log,
            "evt_000000000002",
            crate::activity::EventKind::RevisionSealed,
            "chg_2",
        );

        assert_eq!(
            page(&log, false, None, None, Some("sealed")).unwrap().len(),
            1
        );
        assert_eq!(
            page(&log, false, None, None, Some("chg_1")).unwrap().len(),
            1
        );
        assert_eq!(
            page(&log, false, None, None, Some("nothing"))
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn a_replay_counts_by_vocabulary_name() {
        let directory = tempfile::tempdir().unwrap();
        let log = log(&directory);
        append(
            &log,
            "evt_000000000001",
            crate::activity::EventKind::ChangeCreated,
            "chg_1",
        );
        append(
            &log,
            "evt_000000000002",
            crate::activity::EventKind::ChangeCreated,
            "chg_2",
        );

        let report = replay(&log, "prj_000000000001", log.verify_chain()).unwrap();
        assert_eq!(report.events, 2);
        assert_eq!(report.by_kind.get("ChangeCreated"), Some(&2));
        assert!(report.chain_ok);
    }
}
