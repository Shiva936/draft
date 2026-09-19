//! What may be collected, and — far more important — what may not.
//!
//! GC removes unreachable storage artifacts and derived state. It does **not**
//! delete canonical accepted or history facts because they are old. Age is not
//! a reason; unreachability is.
//!
//! # Why this is a reachability model and not a list of rules
//!
//! Rules of the form "collect staged attempts older than N" are wrong in a way
//! that only shows up after the deletion. The safe question is not "does this
//! look disposable?" but "can anything still reach it?" — and answering that
//! needs the root set written down in one place, where a domain that gains a
//! new durable artifact is forced to add it or see its artifacts vanish.
//!
//! # The asymmetry that decides every judgement call
//!
//! Retaining something collectible costs disk. Collecting something reachable
//! destroys history that cannot be rebuilt — an outcome nobody can re-derive,
//! a receipt nobody can re-sign, an audit trail with a hole in it.
//!
//! So every uncertain case is retained, and [`Reachability::Unknown`] exists
//! rather than being folded into either answer: "we could not tell" and "it is
//! garbage" must never be the same value.
//!
//! # Journals and outboxes root everything they mention
//!
//! An active journal is a statement that a transaction is unfinished. Anything
//! it references is input to finishing that transaction, so collecting it
//! would turn a recoverable interruption into an unrecoverable one — the exact
//! situation the journal exists to prevent.
//!
//! This is why a staged, never-dispatched attempt is *not* immediately
//! collectible even though nothing external ever saw it: its journal and
//! control recovery must complete first, because until they do, that artifact
//! is how recovery learns what was attempted (Scenarios AV, DP).

use std::collections::{BTreeMap, BTreeSet};

/// One durable artifact GC can reason about.
///
/// Identified by an opaque key rather than a typed id, because the root set
/// spans every domain and a union of every id type would make this module a
/// second place each domain has to be edited.
pub type ArtifactKey = String;

/// Why an artifact is retained.
///
/// Kept as a reason rather than a boolean so a person asking "why is this
/// still here?" gets an answer, and so a wrong retention is debuggable rather
/// than merely conservative.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetentionReason {
    /// Named directly in the root set.
    Root,
    /// Referenced by something retained.
    ReachableFrom(ArtifactKey),
    /// Referenced by an unfinished transaction.
    ///
    /// Distinct from ordinary reachability because it is temporary and
    /// self-clearing: when the journal finalizes, this reason disappears and
    /// the artifact becomes collectible if nothing else holds it.
    ActiveJournal(ArtifactKey),
    /// Referenced by an audit fact that has not been drained.
    UndrainedOutbox(ArtifactKey),
}

/// What GC concluded about one artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reachability {
    /// Retained, with the reason.
    Retained(RetentionReason),
    /// Nothing reaches it. Safe to collect.
    Collectible,
    /// GC could not determine reachability.
    ///
    /// Never collected. An unreadable journal, a reference GC cannot resolve,
    /// a store it could not enumerate — all mean the same thing here, which is
    /// that the question was not answered and a deletion would be a guess.
    Unknown { detail: String },
}

impl Reachability {
    /// Whether this artifact may be deleted.
    ///
    /// The only place the decision is made, so no caller can treat `Unknown`
    /// as collectible by writing `!= Retained`.
    pub fn may_collect(&self) -> bool {
        matches!(self, Self::Collectible)
    }
}

/// The graph GC walks.
#[derive(Debug, Clone, Default)]
pub struct RootGraph {
    /// Artifacts that are roots in themselves.
    pub roots: BTreeSet<ArtifactKey>,
    /// Artifact → the artifacts it references.
    pub references: BTreeMap<ArtifactKey, BTreeSet<ArtifactKey>>,
    /// Unfinished transactions → what they reference.
    ///
    /// Separate from `references` so the reason survives into the answer, and
    /// so "retained because a transaction is mid-flight" can be distinguished
    /// from "retained because it is history".
    pub active_journals: BTreeMap<ArtifactKey, BTreeSet<ArtifactKey>>,
    /// Undrained audit facts → what they reference.
    pub undrained_outbox: BTreeMap<ArtifactKey, BTreeSet<ArtifactKey>>,
    /// Artifacts GC could not resolve, with why.
    pub unresolved: BTreeMap<ArtifactKey, String>,
}

impl RootGraph {
    /// Every artifact mentioned anywhere in the graph.
    fn universe(&self) -> BTreeSet<ArtifactKey> {
        let mut all: BTreeSet<ArtifactKey> = self.roots.clone();
        for map in [
            &self.references,
            &self.active_journals,
            &self.undrained_outbox,
        ] {
            for (owner, referenced) in map {
                all.insert(owner.clone());
                all.extend(referenced.iter().cloned());
            }
        }
        all.extend(self.unresolved.keys().cloned());
        all
    }
}

/// Classify every artifact in the graph.
///
/// Roots and journal/outbox references are expanded transitively: an artifact
/// reachable through three hops from a root is as retained as one named
/// directly, because deleting it breaks the same chain.
pub fn classify(graph: &RootGraph) -> BTreeMap<ArtifactKey, Reachability> {
    let mut retained: BTreeMap<ArtifactKey, RetentionReason> = BTreeMap::new();
    let mut frontier: Vec<ArtifactKey> = Vec::new();

    for root in &graph.roots {
        retained.insert(root.clone(), RetentionReason::Root);
        frontier.push(root.clone());
    }
    for (journal, referenced) in &graph.active_journals {
        // The journal itself is retained too: it is the record of what has to
        // be finished, and collecting it would lose the instructions.
        retained
            .entry(journal.clone())
            .or_insert(RetentionReason::Root);
        frontier.push(journal.clone());
        for artifact in referenced {
            retained
                .entry(artifact.clone())
                .or_insert_with(|| RetentionReason::ActiveJournal(journal.clone()));
            frontier.push(artifact.clone());
        }
    }
    for (fact, referenced) in &graph.undrained_outbox {
        retained
            .entry(fact.clone())
            .or_insert(RetentionReason::Root);
        frontier.push(fact.clone());
        for artifact in referenced {
            retained
                .entry(artifact.clone())
                .or_insert_with(|| RetentionReason::UndrainedOutbox(fact.clone()));
            frontier.push(artifact.clone());
        }
    }

    while let Some(current) = frontier.pop() {
        let Some(referenced) = graph.references.get(&current) else {
            continue;
        };
        for artifact in referenced {
            if !retained.contains_key(artifact) {
                retained.insert(
                    artifact.clone(),
                    RetentionReason::ReachableFrom(current.clone()),
                );
                frontier.push(artifact.clone());
            }
        }
    }

    let mut answers = BTreeMap::new();
    for artifact in graph.universe() {
        let answer = if let Some(reason) = retained.get(&artifact) {
            // Retention owed to an unfinished transaction is the one §2.57
            // names separately: it is what tells an operator that recovery
            // work, rather than ordinary history, is holding storage.
            if matches!(
                reason,
                RetentionReason::ActiveJournal(_) | RetentionReason::UndrainedOutbox(_)
            ) {
                crate::support::telemetry::Counter::GcRecoveryRootsPreserved.increment();
            }
            Reachability::Retained(reason.clone())
        } else if let Some(detail) = graph.unresolved.get(&artifact) {
            Reachability::Unknown {
                detail: detail.clone(),
            }
        } else {
            // Marked, not yet collected. The two are counted separately
            // because a mark that never becomes a collection is exactly the
            // symptom of a sweep that keeps failing.
            crate::support::telemetry::Counter::GcObjectsMarked.increment();
            Reachability::Collectible
        };
        answers.insert(artifact, answer);
    }
    answers
}

/// The artifacts that may be deleted.
pub fn collectible(graph: &RootGraph) -> BTreeSet<ArtifactKey> {
    classify(graph)
        .into_iter()
        .filter(|(_, answer)| answer.may_collect())
        .map(|(artifact, _)| artifact)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(names: &[&str]) -> BTreeSet<ArtifactKey> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn edge(from: &str, to: &[&str]) -> (ArtifactKey, BTreeSet<ArtifactKey>) {
        (from.to_string(), keys(to))
    }

    #[test]
    fn history_reachable_from_a_root_is_retained_however_far_away() {
        // baseline → revision → evidence → observation. Nothing here is
        // recent, and every hop is as retained as the root.
        let graph = RootGraph {
            roots: keys(&["control"]),
            references: [
                edge("control", &["baseline"]),
                edge("baseline", &["revision"]),
                edge("revision", &["evidence"]),
                edge("evidence", &["observation"]),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        assert!(collectible(&graph).is_empty());
    }

    #[test]
    fn an_artifact_nothing_reaches_is_collectible() {
        let graph = RootGraph {
            roots: keys(&["control"]),
            references: [edge("control", &["baseline"]), edge("orphan", &[])]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        assert_eq!(collectible(&graph), keys(&["orphan"]));
    }

    #[test]
    fn a_staged_attempt_is_held_by_its_journal_and_freed_when_it_finalizes() {
        // The Scenario AV/DP case. Nothing external ever saw this attempt, so
        // it looks disposable — and until its journal finalizes, that journal
        // is how recovery learns what was attempted.
        let mid_flight = RootGraph {
            roots: keys(&["control"]),
            active_journals: [edge("pat_a.journal", &["pat_a"])].into_iter().collect(),
            ..Default::default()
        };
        assert!(collectible(&mid_flight).is_empty());
        assert_eq!(
            classify(&mid_flight)["pat_a"],
            Reachability::Retained(RetentionReason::ActiveJournal("pat_a.journal".into()))
        );

        // The journal finalized, so it is no longer active. Nothing else holds
        // the staged attempt, and the retention reason clears itself.
        let settled = RootGraph {
            roots: keys(&["control"]),
            references: [edge("pat_a", &[])].into_iter().collect(),
            ..Default::default()
        };
        assert_eq!(collectible(&settled), keys(&["pat_a"]));
    }

    #[test]
    fn a_terminal_abandoned_candidate_journal_is_released() {
        // `Abandoned` is terminal: no number consumed, no authorization spent,
        // nothing dispatched, nothing to drain. Once it is no longer an active
        // journal, nothing roots it.
        let graph = RootGraph {
            roots: keys(&["control"]),
            references: [edge("pat_a.journal", &["pat_a"])].into_iter().collect(),
            ..Default::default()
        };
        assert_eq!(collectible(&graph), keys(&["pat_a", "pat_a.journal"]));
    }

    #[test]
    fn a_dispatched_publication_stays_reachable_through_its_refs() {
        // The chain that must never break: registry → publication → attempt →
        // outcome → receipt. A collected outcome is an external effect nobody
        // can account for afterwards.
        let graph = RootGraph {
            roots: keys(&["publication.registry"]),
            references: [
                edge("publication.registry", &["pub_a"]),
                edge("pub_a", &["pat_a"]),
                edge("pat_a", &["outcome_a"]),
                edge("outcome_a", &["receipt_a", "resolution_a"]),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        assert!(collectible(&graph).is_empty());
    }

    #[test]
    fn an_undrained_audit_fact_holds_what_it_describes() {
        let graph = RootGraph {
            roots: keys(&["control"]),
            undrained_outbox: [edge("fact_a", &["pat_a"])].into_iter().collect(),
            ..Default::default()
        };
        assert!(collectible(&graph).is_empty());
        assert_eq!(
            classify(&graph)["pat_a"],
            Reachability::Retained(RetentionReason::UndrainedOutbox("fact_a".into()))
        );
    }

    #[test]
    fn an_unresolvable_artifact_is_never_collected() {
        // The asymmetry, made into a value: retaining costs disk, collecting
        // destroys what cannot be rebuilt. "We could not tell" must not be
        // reachable from `!= Retained`.
        let graph = RootGraph {
            roots: keys(&["control"]),
            unresolved: [("pat_a".to_string(), "its journal is unreadable".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        assert!(collectible(&graph).is_empty());
        let answer = &classify(&graph)["pat_a"];
        assert!(matches!(answer, Reachability::Unknown { .. }));
        assert!(!answer.may_collect());
    }

    #[test]
    fn a_journal_holds_an_artifact_even_when_something_else_would_free_it() {
        // Two claims on one artifact. The journal's claim is temporary, so the
        // reported reason must be the one that will actually clear.
        let graph = RootGraph {
            roots: keys(&["control"]),
            references: [edge("control", &[])].into_iter().collect(),
            active_journals: [edge("journal_a", &["pat_a"])].into_iter().collect(),
            ..Default::default()
        };
        assert_eq!(
            classify(&graph)["pat_a"],
            Reachability::Retained(RetentionReason::ActiveJournal("journal_a".into()))
        );
    }

    #[test]
    fn a_reference_cycle_terminates() {
        // Retained facts legitimately point at each other; a walk that
        // revisited them would not finish.
        let graph = RootGraph {
            roots: keys(&["a"]),
            references: [edge("a", &["b"]), edge("b", &["a", "c"]), edge("c", &["b"])]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        assert!(collectible(&graph).is_empty());
    }

    #[test]
    fn an_empty_graph_collects_nothing_rather_than_everything() {
        assert!(collectible(&RootGraph::default()).is_empty());
    }
}
