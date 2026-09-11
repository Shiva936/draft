//! The parts of one Change that §8.3 shows as separate views.
//!
//! Each of these answers a different question about the same Change, and they
//! are separate types because they have different authorities. Intent is what
//! the author declared; scope is what that declaration resolved to against an
//! exact Baseline; recovery is where an interrupted promotion of it stands.
//! Folding them together would let a reader mistake a declared scope for a
//! resolved one, which is the difference between what a Change may touch and
//! what it was sealed over.
//!
//! # Why these live in Core
//!
//! Every one of them is a domain judgement. `draftd` composes them into a
//! Console model and neither frontend recomputes any of it — a frontend that
//! decided for itself whether a promotion had committed would be a second
//! answer to a question the restart table already owns.

use std::collections::BTreeSet;

use draft_dcg_contract::ids::{ChangeId, ChangeRevisionId, ResourceId};
use draft_dcg_contract::Digest;
use serde::Serialize;

use crate::dcg::change::{Change, ChangeLifecycle};
use crate::dcg::definition::{ChangeDefinition, ScopeResolution};
use crate::project::Workspace;
use crate::promotion::journal::{ChangeMatch, ControlMatch, PromotionResolution};
use crate::promotion::record::PromotionJournal;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// What a Change is for, and which definition currently says so.
#[derive(Debug, Clone, Serialize)]
pub struct ChangeIntentView {
    pub change: ChangeId,
    pub lifecycle: ChangeLifecycle,
    /// The author's words. Opaque to Core, rendered verbatim.
    pub intent: String,
    /// The exact definition in force, by digest.
    pub current_definition: Digest,
    /// The Change record's generation, which advances on every amendment and
    /// lifecycle move.
    pub generation: u64,
    pub created_by: String,
    pub created_at: String,
    /// Whether the definition in force is still the one the Change was created
    /// with, taken from the ledger rather than guessed from the generation.
    ///
    /// A generation advances on lifecycle moves too, so counting it would
    /// report an abandoned-and-reopened Change as amended.
    pub amendment_count: usize,
    /// Whether an amendment is currently permitted at all.
    pub amendable: bool,
}

/// What a Change may touch, and what that resolved to.
///
/// Both are reported because they are different facts. The declaration is the
/// author's claim; the resolution is that claim narrowed against an exact
/// Baseline, and a Resource that is declared but not resolved is a Resource
/// the Baseline does not hold yet.
#[derive(Debug, Clone, Serialize)]
pub struct ChangeScopeView {
    pub change: ChangeId,
    pub declared: BTreeSet<ResourceId>,
    /// Present only once a revision has been sealed: a Change with no sealed
    /// revision has a declaration and no resolution, and saying so is more
    /// useful than an empty set that looks like "nothing in scope".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<ScopeResolution>,
    /// The revision the resolution belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_for: Option<ChangeRevisionId>,
    /// Declared Resources the resolution did not include.
    pub declared_but_unresolved: BTreeSet<ResourceId>,
}

/// Where an interrupted promotion of this Change stands.
///
/// The four postures are the restart table's answers, not a frontend's reading
/// of which files exist. A missing journal means no promotion was ever
/// prepared; it never means a promotion succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPosture {
    /// Nothing to recover: no promotion of this Change is mid-flight.
    NotRequired,
    /// A promotion is interrupted and can be carried to completion.
    Recoverable,
    /// Two records no legal sequence produces. A person has to look.
    Inconsistent,
    /// A promotion that was interrupted has since been finalized.
    Completed,
}

/// One interrupted promotion, and what the restart table says about it.
#[derive(Debug, Clone, Serialize)]
pub struct ChangeRecoveryEntry {
    pub promotion: String,
    pub revision: ChangeRevisionId,
    pub baseline: String,
    pub journal_state: String,
    pub posture: RecoveryPosture,
    /// The restart table's own words for what happens next.
    pub resolution: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Recovery for one Change, folded over every promotion journal naming it.
#[derive(Debug, Clone, Serialize)]
pub struct ChangeRecoveryView {
    pub change: ChangeId,
    /// The worst posture among the entries, which is what a reader must act
    /// on: one inconsistent promotion is not cancelled by three clean ones.
    pub posture: RecoveryPosture,
    pub promotions: Vec<ChangeRecoveryEntry>,
    /// Checkpoints this Change could be recovered toward, newest first.
    ///
    /// Snapshot identity only. Planning a restore is a separate act with its
    /// own destructive-preview step, and listing targets must not look like
    /// having chosen one.
    pub recovery_targets: Vec<String>,
}

impl RecoveryPosture {
    fn of(resolution: &PromotionResolution) -> Self {
        match resolution {
            PromotionResolution::DidNotCommit => Self::NotRequired,
            PromotionResolution::AlreadyFinalized => Self::Completed,
            PromotionResolution::ContinueFinalization
            | PromotionResolution::CompleteChangeThenFinalize => Self::Recoverable,
            PromotionResolution::Inconsistent { .. } => Self::Inconsistent,
        }
    }

    /// Which of two postures a reader has to act on first.
    fn severity(self) -> u8 {
        match self {
            Self::NotRequired => 0,
            Self::Completed => 1,
            Self::Recoverable => 2,
            Self::Inconsistent => 3,
        }
    }
}

fn change_record(workspace: &Workspace, change: &ChangeId) -> DraftResult<Change> {
    crate::dcg::change::ChangeStore::new(workspace.layout.changes_dir())
        .read_unlocked(change)?
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                format!("no Change '{change}' in this project"),
            )
        })
}

fn definitions(workspace: &Workspace) -> crate::dcg::definition::DefinitionStore {
    crate::dcg::definition::DefinitionStore::new(
        workspace.layout.definitions_dir(),
        workspace.layout.scope_resolutions_dir(),
    )
}

fn current_definition(workspace: &Workspace, record: &Change) -> DraftResult<ChangeDefinition> {
    definitions(workspace)
        .definition(&record.current_definition)?
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "Change '{}' names definition {}, which this project does not hold",
                    record.id, record.current_definition
                ),
            )
        })
}

/// What this Change is for, from the definition currently in force.
pub fn intent(workspace: &Workspace, change: &ChangeId) -> DraftResult<ChangeIntentView> {
    let record = change_record(workspace, change)?;
    let definition = current_definition(workspace, &record)?;
    // Amendment is a recorded act, so it is counted from the ledger. Deriving
    // it from the generation would report a reopened Change as amended.
    let amendment_count = crate::read_model::activity::entries(workspace.events()?.log())?
        .into_iter()
        .filter(|entry| {
            entry.kind == "ChangeDefinitionAmended"
                && entry.subject.as_deref() == Some(change.as_str())
        })
        .count();
    Ok(ChangeIntentView {
        change: change.clone(),
        lifecycle: record.lifecycle,
        intent: definition.intent,
        current_definition: record.current_definition.clone(),
        generation: record.generation,
        created_by: definition.created_by.to_string(),
        created_at: definition.created_at.to_string(),
        amendment_count,
        amendable: record.lifecycle.accepts_work(),
    })
}

/// What this Change may touch, declared and resolved.
pub fn scope(workspace: &Workspace, change: &ChangeId) -> DraftResult<ChangeScopeView> {
    let record = change_record(workspace, change)?;
    let definition = current_definition(workspace, &record)?;
    let newest = crate::app::workflow::change_views(workspace)?
        .into_iter()
        .find(|view| &view.change == change)
        .and_then(|view| view.revisions.first().cloned());
    let resolution = match &newest {
        Some(revision) => definitions(workspace).resolution(&revision.scope)?,
        None => None,
    };
    let resolved: BTreeSet<ResourceId> = resolution
        .as_ref()
        .map(|value| value.resources.clone())
        .unwrap_or_default();
    // Only meaningful once something resolved: with no resolution at all the
    // right answer is "not resolved yet", not "everything is missing".
    let declared_but_unresolved = match &resolution {
        Some(_) => definition
            .scope_declaration
            .difference(&resolved)
            .cloned()
            .collect(),
        None => BTreeSet::new(),
    };
    Ok(ChangeScopeView {
        change: change.clone(),
        declared: definition.scope_declaration,
        resolved_for: newest.map(|revision| revision.id),
        resolution,
        declared_but_unresolved,
    })
}

/// Where any interrupted promotion of this Change stands.
pub fn recovery(workspace: &Workspace, change: &ChangeId) -> DraftResult<ChangeRecoveryView> {
    let journals = crate::promotion::store::PromotionJournalStore::new(
        workspace.layout.promotion_journals_dir(),
    );
    let control = current_control_digest(workspace)?;
    let mut promotions = Vec::new();
    for id in journals.list()? {
        let Some(record) = journals.read_unlocked(&id)? else {
            continue;
        };
        if &record.journal.change != change {
            continue;
        }
        let resolution = crate::promotion::journal::classify(
            record.state(),
            ControlMatch::classify(
                &control,
                &record.journal.expected_control,
                &record.journal.planned_control,
            ),
            change_match(workspace, &record.journal)?,
        );
        promotions.push(ChangeRecoveryEntry {
            promotion: record.journal.promotion.to_string(),
            revision: record.journal.revision.clone(),
            baseline: record.journal.baseline.to_string(),
            journal_state: format!("{:?}", record.state()),
            posture: RecoveryPosture::of(&resolution),
            resolution: describe(&resolution).to_string(),
            detail: match &resolution {
                PromotionResolution::Inconsistent { detail } => Some((*detail).to_string()),
                _ => None,
            },
        });
    }
    promotions.sort_by(|left, right| left.promotion.cmp(&right.promotion));
    let posture = promotions
        .iter()
        .map(|entry| entry.posture)
        .max_by_key(|posture| posture.severity())
        .unwrap_or(RecoveryPosture::NotRequired);
    Ok(ChangeRecoveryView {
        change: change.clone(),
        posture,
        promotions,
        recovery_targets: recovery_targets(workspace)?,
    })
}

/// Checkpoints a recovery could be aimed at, newest first.
///
/// Identity only. Listing a target is not planning a restore: the plan is
/// what says which Resources would be removed, and a list that looked like a
/// plan would invite acting without that preview.
fn recovery_targets(workspace: &Workspace) -> DraftResult<Vec<String>> {
    let directory = workspace.layout.snapshots_dir();
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(DraftError::storage(format!(
                "cannot list snapshots in {}: {error}",
                directory.display()
            )))
        }
    };
    let mut snapshots: Vec<crate::dcg::state::Snapshot> = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            snapshots.push(crate::contracts::read_persisted(&path)?);
        }
    }
    snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.created_at));
    Ok(snapshots
        .into_iter()
        .map(|snapshot| snapshot.id.to_string())
        .collect())
}

/// The restart table's answer, in the words a reader can act on.
fn describe(resolution: &PromotionResolution) -> &'static str {
    match resolution {
        PromotionResolution::DidNotCommit => {
            "the commit never landed; nothing was accepted and nothing needs finishing"
        }
        PromotionResolution::AlreadyFinalized => {
            "the promotion finalized; its Baseline, receipt and events are history"
        }
        PromotionResolution::ContinueFinalization => {
            "the Baseline was accepted; finalization has yet to run"
        }
        PromotionResolution::CompleteChangeThenFinalize => {
            "the Baseline was accepted; the Change completion and finalization have yet to run"
        }
        PromotionResolution::Inconsistent { .. } => {
            "no legal sequence produces these two records; this needs a person"
        }
    }
}

/// The control digest promotion compares against, derived the one way
/// `app::promotion` derives it.
fn current_control_digest(workspace: &Workspace) -> DraftResult<Digest> {
    Ok(Digest::of_bytes(
        crate::dcg::baseline::current_baseline(&workspace.layout)?
            .map_or_else(|| "none".to_string(), |baseline| baseline.to_string())
            .as_bytes(),
    ))
}

/// The same whole-value comparison the recovery boundary performs.
fn change_match(workspace: &Workspace, journal: &PromotionJournal) -> DraftResult<ChangeMatch> {
    let store = crate::dcg::change::ChangeStore::new(workspace.layout.changes_dir());
    let Some(current) = store.read_unlocked(&journal.change)? else {
        return Ok(ChangeMatch::Neither);
    };
    if crate::app::promotion::change_digest(&current)? == journal.planned_change {
        return Ok(ChangeMatch::PlannedCompleted);
    }
    Ok(if current.lifecycle == ChangeLifecycle::Active {
        ChangeMatch::ExpectedActive
    } else {
        ChangeMatch::Neither
    })
}
