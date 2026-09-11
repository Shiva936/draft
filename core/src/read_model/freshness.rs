//! Whether a view is still current, and what to do when it is not.
//!
//! # The failure this exists to stop
//!
//! A person reads a screen, decides, and acts. Between the read and the act,
//! the project moves. Without a check, their decision lands on state they
//! never saw — they approve a change whose gate has since failed, or promote a
//! Baseline that is no longer the accepted one.
//!
//! Nothing about that looks like an error at the time. It looks like the
//! action worked.
//!
//! So every state-sensitive mutation carries the revisions its decision was
//! made against, and those are re-checked inside the lock that commits. A
//! caller acting on a stale view is refused with [`ActionOutcome::Stale`],
//! which names what moved.
//!
//! # Why invalidation is a map and not a version counter
//!
//! One global counter would be correct and useless: every write would
//! invalidate every view, so a busy project would refuse actions constantly
//! and people would learn to retry blindly until something stuck. That is
//! worse than no check, because it trains the habit that defeats it.
//!
//! The map is therefore specific about what each source of change actually
//! affects — and, just as deliberately, about what it does **not**. A
//! `ProviderBinding` reprofile changes where new work would be sent; it cannot
//! change what a Baseline was composed from, because that was fixed when the
//! Baseline was accepted. Invalidating historical projections on a binding
//! change would be a lie about which facts can move.
//!
//! # Why staleness and conflict are different answers
//!
//! `Stale` means the caller's view aged out: re-read and decide again, and the
//! decision may well be the same. `Conflict` means somebody else committed a
//! competing change: re-reading is not enough, because the two intents have to
//! be reconciled.
//!
//! Collapsing them would tell a person to "refresh and retry" in the one case
//! where refreshing loses the other person's work.

use std::collections::BTreeMap;

/// A durable store whose generation a view may depend on.
///
/// Named rather than free-text, so a view cannot declare a dependency on a
/// store nobody updates and appear fresh forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StoreKey {
    ProjectControl,
    Change,
    ProviderBinding,
    Evidence,
    Gate,
    Decision,
    PublicationControl,
    Task,
}

/// What a view was derived from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadModelWatermark {
    pub project_control_generation: u64,
    pub activity_tail_hash: String,
    pub store_generations: BTreeMap<StoreKey, u64>,
}

impl ReadModelWatermark {
    /// The generation this view saw for `store`, if it depended on it.
    pub fn generation(&self, store: StoreKey) -> Option<u64> {
        self.store_generations.get(&store).copied()
    }

    /// Whether `current` has moved past this watermark in any way this view
    /// depended on.
    ///
    /// A store the view never read cannot make it stale: a view of Changes is
    /// not invalidated by a Task write, and treating it as though it were is
    /// the global-counter mistake in a smaller form.
    pub fn is_stale_against(&self, current: &ReadModelWatermark) -> bool {
        !self.moved_since(current).is_empty()
    }

    /// Everything this view depended on that has since moved.
    pub fn moved_since(&self, current: &ReadModelWatermark) -> Vec<StaleReason> {
        let mut moved = Vec::new();
        if current.project_control_generation != self.project_control_generation {
            moved.push(StaleReason::ProjectControlAdvanced {
                seen: self.project_control_generation,
                current: current.project_control_generation,
            });
        }
        if current.activity_tail_hash != self.activity_tail_hash {
            moved.push(StaleReason::ActivityAdvanced);
        }
        for (store, seen) in &self.store_generations {
            match current.store_generations.get(store) {
                Some(now) if now == seen => {}
                Some(now) => moved.push(StaleReason::StoreAdvanced {
                    store: *store,
                    seen: *seen,
                    current: *now,
                }),
                // The store the view depended on is no longer reported. Treated
                // as movement rather than as "unchanged": an absent generation
                // is an unanswered question, and answering it optimistically is
                // how a stale action gets through.
                None => moved.push(StaleReason::StoreUnknown { store: *store }),
            }
        }
        moved
    }
}

/// Why a view is no longer current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleReason {
    ProjectControlAdvanced {
        seen: u64,
        current: u64,
    },
    ActivityAdvanced,
    StoreAdvanced {
        store: StoreKey,
        seen: u64,
        current: u64,
    },
    StoreUnknown {
        store: StoreKey,
    },
    /// The view never recorded a generation for a store its own projection
    /// depends on, so there is nothing to compare and no way to call it fresh.
    StoreUndeclared {
        store: StoreKey,
    },
}

/// A view whose freshness can be judged.
///
/// The map from §2.56, as data. Each projection declares what it read, so
/// invalidation follows from the declaration rather than from a rule somebody
/// has to remember to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Projection {
    /// The currently accepted Baseline, policy and lifecycle.
    CurrentBaseline,
    /// A specific historical Baseline, and what it was composed from.
    ///
    /// Depends on nothing mutable. It was fixed when the Baseline was
    /// accepted, and no later configuration change can alter it.
    HistoricalBaselineComposition,
    /// Whether the current configuration can route to a target.
    CurrentRoutability,
    /// A Change's summary, scope and planning state.
    ChangeSummary,
    /// A sealed revision's projection. Sealed content does not move.
    SealedRevision,
    /// Whether a Change may currently advance.
    ChangeEligibility,
    /// A historical gate evaluation or decision, as it was made.
    HistoricalEvaluation,
    /// Per-target publication status.
    PublicationStatus,
    /// Whether another external delivery may currently be started.
    ///
    /// Its own row rather than `PublicationStatus`, because §2.56 lists a
    /// `ProviderBinding` change as invalidating *publication eligibility*
    /// while leaving prior attempt and outcome facts alone. Status is what
    /// happened; eligibility is what may happen next, and the route and the
    /// project's lifecycle decide the second without touching the first.
    PublicationEligibility,
    /// This project's provider bindings, definitions and profiles.
    ///
    /// Bindings are mutable and profiles and definitions are not, so the row
    /// names only the binding store: adding a definition cannot invalidate a
    /// view of what is bound, and re-reading on every immutable write would
    /// make an act on a binding stale for no reason.
    ProviderCatalog,
    /// Extension-management state, which lives in the installation registry
    /// rather than in any project store.
    ///
    /// Depends on no project generation deliberately: the registry has its own
    /// authoritative revision, compared against current state where the action
    /// runs. Folding it in here would give the same fact two owners.
    InstallationState,
    /// Prior attempt and outcome facts, which are immutable.
    PublicationHistory,
    /// Anything derived from the Activity ledger.
    ActivityDerived,
}

impl Projection {
    /// The stores this projection reads.
    ///
    /// The empty set is a real answer, not an oversight: a projection of
    /// immutable history depends on no mutable store and is never stale.
    pub fn depends_on(&self) -> &'static [StoreKey] {
        match self {
            Self::CurrentBaseline => &[StoreKey::ProjectControl],
            Self::ChangeSummary => &[StoreKey::Change],
            Self::CurrentRoutability => &[StoreKey::ProviderBinding, StoreKey::ProjectControl],
            Self::ChangeEligibility => &[
                StoreKey::Change,
                StoreKey::ProjectControl,
                StoreKey::ProviderBinding,
                StoreKey::Evidence,
                StoreKey::Gate,
                StoreKey::Decision,
            ],
            Self::ProviderCatalog => &[StoreKey::ProviderBinding],
            Self::PublicationStatus => &[StoreKey::PublicationControl],
            Self::PublicationEligibility => &[
                StoreKey::PublicationControl,
                StoreKey::ProviderBinding,
                StoreKey::ProjectControl,
            ],
            // Fixed at acceptance, or immutable by construction. A binding
            // reprofile changes where new work goes; it cannot change what an
            // accepted Baseline was composed from.
            Self::HistoricalBaselineComposition
            | Self::SealedRevision
            | Self::HistoricalEvaluation
            | Self::PublicationHistory
            | Self::InstallationState => &[],
            Self::ActivityDerived => &[],
        }
    }

    /// Whether advancing the Activity tail invalidates this projection.
    pub fn follows_activity(&self) -> bool {
        matches!(self, Self::ActivityDerived)
    }

    /// Whether any mutable state can invalidate this projection.
    pub fn is_historical(&self) -> bool {
        self.depends_on().is_empty() && !self.follows_activity()
    }

    /// Whether `change` invalidates this projection.
    pub fn invalidated_by(&self, change: StoreKey) -> bool {
        self.depends_on().contains(&change)
    }
}

/// What a caller believed when it decided to act.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestPrecondition {
    /// The projection the decision was made from.
    pub projection: Projection,
    /// What that projection was derived from.
    pub watermark: ReadModelWatermark,
}

/// The answer to "may this action proceed?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    /// Nothing the caller depended on has moved.
    Proceed,
    /// The caller's view aged out. Re-read and decide again; the decision may
    /// well be the same.
    Stale { moved: Vec<StaleReason> },
    /// Somebody else committed a competing change to the same subject.
    ///
    /// Re-reading is not sufficient — the two intents have to be reconciled,
    /// and telling the caller to "refresh and retry" would discard the other
    /// person's work.
    Conflict { subject: String, detail: String },
}

impl ActionOutcome {
    pub fn may_proceed(&self) -> bool {
        matches!(self, Self::Proceed)
    }
}

/// Check a precondition against current state.
///
/// Runs inside the lock that commits, never before it: a check performed
/// earlier is a check whose answer can be invalidated before it is used, which
/// is exactly the situation it exists to detect.
///
/// A projection that depends on nothing mutable always proceeds. That is not
/// a shortcut — asking it to be fresh would be asking whether the past has
/// changed.
pub fn check(precondition: &RequestPrecondition, current: &ReadModelWatermark) -> ActionOutcome {
    let projection = precondition.projection;
    // A store the projection depends on but the view never recorded cannot be
    // compared at all, and an unanswerable question is not a passing answer.
    // Without this a caller could present an empty watermark and be judged
    // fresh against every store it forgot to declare.
    let mut moved: Vec<StaleReason> = projection
        .depends_on()
        .iter()
        // `ProjectControl` has its own field on the watermark rather than a map
        // entry, so it is always answered and never undeclared.
        .filter(|store| **store != StoreKey::ProjectControl)
        .filter(|store| precondition.watermark.generation(**store).is_none())
        .map(|store| StaleReason::StoreUndeclared { store: *store })
        .collect();
    moved.extend(
        precondition
            .watermark
            .moved_since(current)
            .into_iter()
            .filter(|reason| match reason {
                StaleReason::ProjectControlAdvanced { .. } => {
                    projection.invalidated_by(StoreKey::ProjectControl)
                }
                StaleReason::ActivityAdvanced => projection.follows_activity(),
                StaleReason::StoreAdvanced { store, .. }
                | StaleReason::StoreUnknown { store }
                | StaleReason::StoreUndeclared { store } => projection.invalidated_by(*store),
            }),
    );
    moved.dedup();

    if moved.is_empty() {
        ActionOutcome::Proceed
    } else {
        // The one place a stale action is refused, so the counter cannot drift
        // from the decision: every caller that receives `Stale` refuses.
        crate::support::telemetry::Counter::ReadModelStaleRejections.increment();
        ActionOutcome::Stale { moved }
    }
}

/// Read the authoritative current watermark for one project.
///
/// # Why this reads stores rather than a cache
///
/// The whole point of a precondition is that it is judged against what is
/// true *now*, immediately before the mutation. Comparing an issued
/// descriptor with the numbers a client sent back proves only that the client
/// echoed what it was given; it says nothing about whether the project moved
/// in between. So every generation here comes from the authoritative record,
/// read at check time.
///
/// # Why some stores report a fold rather than a counter
///
/// `ProjectControl` and a single `Change` have their own generation. Evidence,
/// gate evaluations and decisions are create-once immutable facts with no
/// generation at all — but they are never deleted, so their count is
/// monotonic and moves exactly when a new one lands, which is precisely when
/// eligibility changes. Provider bindings, publication controls and tasks are
/// many revisioned records, so the fold is the sum of `generation + 1`: the
/// `+ 1` makes creating a record move the number even at generation zero.
pub fn current_watermark(
    layout: &crate::project::layout::DraftLayout,
    project: &draft_dcg_contract::ids::ProjectId,
    change: Option<&draft_dcg_contract::ids::ChangeId>,
) -> crate::support::error::DraftResult<ReadModelWatermark> {
    let control = crate::project::control::ProjectControlStore::new(layout.project_control_dir())
        .read_unlocked()?;

    let changes = crate::dcg::change::ChangeStore::new(layout.changes_dir());
    let change_generation = match change {
        // The exact Change the action is about. A sibling Change moving is not
        // this action going stale.
        Some(id) => changes
            .read_unlocked(id)?
            .map(|record| record.generation)
            .unwrap_or_default(),
        None => fold(changes.list()?.iter().map(|record| record.generation)),
    };

    let bindings =
        crate::project::provider::ProviderBindingStore::new(layout.provider_bindings_dir());
    let binding_generation = fold(bindings.list()?.iter().map(|binding| binding.generation));

    let publication_root = layout.publication_dir();
    let controls =
        crate::publication::control::PublicationControlStore::new(publication_root.join("control"));
    let publication_generation = fold(controls.list()?.iter().map(|control| control.generation));

    let tasks = crate::task::TaskStore::for_root(&layout.root());
    let task_generation = fold(tasks.list()?.iter().map(|task| task.generation));

    let evidence = crate::evidence::EvidenceStore::new(layout.evidence_dir())
        .list()?
        .len() as u64;
    let gates = crate::gate::GateEvaluationStore::new(layout.gates_dir())
        .list()?
        .len() as u64;
    let decisions = crate::dcg::decision::DecisionStore::new(layout.decisions_dir())
        .list()?
        .len() as u64;

    let log = crate::activity::ActivityLog::new(layout.events_dir(), project.to_string());

    Ok(ReadModelWatermark {
        project_control_generation: control.map(|state| state.generation).unwrap_or_default(),
        activity_tail_hash: log.tail_hash()?,
        store_generations: [
            (StoreKey::Change, change_generation),
            (StoreKey::ProviderBinding, binding_generation),
            (StoreKey::PublicationControl, publication_generation),
            (StoreKey::Task, task_generation),
            (StoreKey::Evidence, evidence),
            (StoreKey::Gate, gates),
            (StoreKey::Decision, decisions),
        ]
        .into_iter()
        .collect(),
    })
}

/// Sum `generation + 1` across many records.
///
/// Monotonic under both creation and mutation, which a bare sum of generations
/// is not: a store whose only record sits at generation zero would report the
/// same number as an empty one.
fn fold(generations: impl Iterator<Item = u64>) -> u64 {
    generations.fold(0u64, |total, generation| {
        total.saturating_add(generation.saturating_add(1))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watermark(control: u64, tail: &str, stores: &[(StoreKey, u64)]) -> ReadModelWatermark {
        ReadModelWatermark {
            project_control_generation: control,
            activity_tail_hash: tail.into(),
            store_generations: stores.iter().copied().collect(),
        }
    }

    fn precondition(projection: Projection, watermark: ReadModelWatermark) -> RequestPrecondition {
        RequestPrecondition {
            projection,
            watermark,
        }
    }

    #[test]
    fn an_unchanged_world_lets_the_action_proceed() {
        let seen = watermark(3, "tail-a", &[(StoreKey::Change, 7)]);
        assert_eq!(
            check(
                &precondition(Projection::ChangeSummary, seen.clone()),
                &seen
            ),
            ActionOutcome::Proceed
        );
    }

    #[test]
    fn a_control_advance_stops_an_action_that_depended_on_it() {
        let seen = watermark(3, "tail-a", &[]);
        let now = watermark(4, "tail-a", &[]);
        match check(&precondition(Projection::CurrentBaseline, seen), &now) {
            ActionOutcome::Stale { moved } => assert_eq!(
                moved,
                vec![StaleReason::ProjectControlAdvanced {
                    seen: 3,
                    current: 4
                }]
            ),
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    #[test]
    fn a_write_to_an_unrelated_store_does_not_invalidate_a_view() {
        // The global-counter mistake, in miniature. If every write invalidated
        // every view, people would learn to retry blindly until something
        // stuck — which is worse than no check at all.
        let seen = watermark(3, "tail-a", &[(StoreKey::Change, 7)]);
        let now = watermark(3, "tail-a", &[(StoreKey::Change, 7), (StoreKey::Task, 99)]);
        assert_eq!(
            check(&precondition(Projection::ChangeSummary, seen), &now),
            ActionOutcome::Proceed
        );
    }

    #[test]
    fn a_binding_catalog_view_tracks_bindings_and_nothing_else() {
        // Both directions, because both are wrong in their own way. Missing a
        // binding change would let an unbind act on a view of the binding set
        // that no longer exists; treating a sealed revision as a binding change
        // would make every such action stale the moment anybody worked on
        // anything.
        let seen = watermark(
            3,
            "tail-a",
            &[(StoreKey::ProviderBinding, 1), (StoreKey::Change, 4)],
        );

        let rebound = watermark(
            3,
            "tail-a",
            &[(StoreKey::ProviderBinding, 2), (StoreKey::Change, 4)],
        );
        assert!(!check(
            &precondition(Projection::ProviderCatalog, seen.clone()),
            &rebound
        )
        .may_proceed());

        let sealed = watermark(
            3,
            "tail-a",
            &[(StoreKey::ProviderBinding, 1), (StoreKey::Change, 5)],
        );
        assert_eq!(
            check(
                &precondition(Projection::ProviderCatalog, seen.clone()),
                &sealed
            ),
            ActionOutcome::Proceed
        );

        // Nor does the ledger advancing: the catalog is read from the stores,
        // not folded from events.
        let appended = watermark(
            3,
            "tail-b",
            &[(StoreKey::ProviderBinding, 1), (StoreKey::Change, 4)],
        );
        assert_eq!(
            check(&precondition(Projection::ProviderCatalog, seen), &appended),
            ActionOutcome::Proceed
        );
    }

    #[test]
    fn a_binding_reprofile_never_invalidates_what_a_baseline_was_composed_from() {
        // Scenario CR. Changing the operational profile changes where new work
        // would be sent. It cannot change what was already accepted, and
        // reporting otherwise would be a lie about which facts can move.
        let seen = watermark(3, "tail-a", &[(StoreKey::ProviderBinding, 1)]);
        let reprofiled = watermark(3, "tail-a", &[(StoreKey::ProviderBinding, 2)]);

        assert_eq!(
            check(
                &precondition(Projection::HistoricalBaselineComposition, seen.clone()),
                &reprofiled
            ),
            ActionOutcome::Proceed
        );
        // And the same change does invalidate the view that is about current
        // configuration, so the distinction is doing work rather than being
        // uniformly permissive.
        assert!(!check(
            &precondition(Projection::CurrentRoutability, seen),
            &reprofiled
        )
        .may_proceed());
    }

    #[test]
    fn historical_projections_are_never_stale() {
        // Asking whether a sealed revision or a past decision is fresh is
        // asking whether the past has changed.
        let seen = watermark(3, "tail-a", &[(StoreKey::Change, 7)]);
        let moved = watermark(99, "tail-z", &[(StoreKey::Change, 4242)]);
        for projection in [
            Projection::HistoricalBaselineComposition,
            Projection::SealedRevision,
            Projection::HistoricalEvaluation,
            Projection::PublicationHistory,
        ] {
            assert!(projection.is_historical());
            assert_eq!(
                check(&precondition(projection, seen.clone()), &moved),
                ActionOutcome::Proceed,
                "{projection:?} must not be invalidated by mutable state"
            );
        }
    }

    #[test]
    fn a_store_that_stops_reporting_is_movement_not_agreement() {
        // An absent generation is an unanswered question. Reading it as
        // "unchanged" is how a stale action gets through.
        let seen = watermark(3, "tail-a", &[(StoreKey::Change, 7)]);
        let silent = watermark(3, "tail-a", &[]);
        match check(&precondition(Projection::ChangeSummary, seen), &silent) {
            ActionOutcome::Stale { moved } => assert_eq!(
                moved,
                vec![StaleReason::StoreUnknown {
                    store: StoreKey::Change
                }]
            ),
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    #[test]
    fn activity_advancement_touches_only_activity_derived_views() {
        let seen = watermark(3, "tail-a", &[(StoreKey::Change, 7)]);
        let appended = watermark(3, "tail-b", &[(StoreKey::Change, 7)]);

        assert_eq!(
            check(
                &precondition(Projection::ActivityDerived, seen.clone()),
                &appended
            ),
            ActionOutcome::Stale {
                moved: vec![StaleReason::ActivityAdvanced]
            }
        );
        assert_eq!(
            check(&precondition(Projection::ChangeSummary, seen), &appended),
            ActionOutcome::Proceed
        );
    }

    #[test]
    fn eligibility_depends_on_every_input_that_can_change_the_answer() {
        // A gate answer rests on the change, the project, the binding and the
        // judgements. Missing one would let a view go stale invisibly in
        // exactly the dimension nobody thought to declare.
        let seen = watermark(
            1,
            "tail-a",
            &[
                (StoreKey::Change, 1),
                (StoreKey::ProviderBinding, 1),
                (StoreKey::Evidence, 1),
                (StoreKey::Gate, 1),
                (StoreKey::Decision, 1),
            ],
        );
        for store in [
            StoreKey::Change,
            StoreKey::ProviderBinding,
            StoreKey::Evidence,
            StoreKey::Gate,
            StoreKey::Decision,
        ] {
            let mut now = seen.clone();
            now.store_generations.insert(store, 2);
            assert!(
                !check(
                    &precondition(Projection::ChangeEligibility, seen.clone()),
                    &now
                )
                .may_proceed(),
                "a {store:?} advance must invalidate eligibility"
            );
        }
        let mut control_moved = seen.clone();
        control_moved.project_control_generation = 2;
        assert!(!check(
            &precondition(Projection::ChangeEligibility, seen),
            &control_moved
        )
        .may_proceed());
    }

    #[test]
    fn a_conflict_is_not_reported_as_staleness() {
        // "Refresh and retry" is the right advice for one and destroys the
        // other person's work for the other.
        let conflict = ActionOutcome::Conflict {
            subject: "chg_000000000001".into(),
            detail: "another decision was recorded for this revision".into(),
        };
        assert!(!conflict.may_proceed());
        assert!(!matches!(conflict, ActionOutcome::Stale { .. }));
    }

    #[test]
    fn publication_status_moves_with_control_but_history_does_not() {
        let seen = watermark(1, "tail-a", &[(StoreKey::PublicationControl, 1)]);
        let now = watermark(1, "tail-a", &[(StoreKey::PublicationControl, 2)]);
        assert!(!check(
            &precondition(Projection::PublicationStatus, seen.clone()),
            &now
        )
        .may_proceed());
        assert_eq!(
            check(&precondition(Projection::PublicationHistory, seen), &now),
            ActionOutcome::Proceed
        );
    }
}
