//! Persistence for Draft changes (`.draft/changes/`).

use serde::{Deserialize, Serialize};

use crate::common::DraftChangeId;
use crate::error::DraftResult;
use crate::fsutil::{list_with_extension, read_json, write_json};
use crate::workspace::layout::DraftLayout;

use super::{DraftChange, GroupingSource};

fn change_path(layout: &DraftLayout, id: &DraftChangeId) -> std::path::PathBuf {
    layout.changes_dir().join(format!("change_{id}.json"))
}

pub fn save_change(layout: &DraftLayout, change: &DraftChange) -> DraftResult<()> {
    write_json(&change_path(layout, &change.id), change)
}

pub fn load_change(layout: &DraftLayout, id: &DraftChangeId) -> DraftResult<DraftChange> {
    read_json(&change_path(layout, id))
}

pub fn load_changes(layout: &DraftLayout) -> DraftResult<Vec<DraftChange>> {
    let mut out = Vec::new();
    for p in list_with_extension(&layout.changes_dir(), "json")? {
        // Skip the group index file.
        if p.file_name().and_then(|n| n.to_str()) == Some("groups.json") {
            continue;
        }
        if let Ok(c) = read_json::<DraftChange>(&p) {
            out.push(c);
        }
    }
    out.sort_by_key(|c| c.created_at);
    Ok(out)
}

/// A lightweight index describing the current grouping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupIndex {
    pub source: GroupingSource,
    pub change_ids: Vec<DraftChangeId>,
}

pub fn save_group_index(layout: &DraftLayout, index: &GroupIndex) -> DraftResult<()> {
    write_json(&layout.changes_dir().join("groups.json"), index)
}
