//! Building the GC root graph from what the project actually holds.
//!
//! [`crate::app::gc`] owns the reachability question; this owns the answer's
//! inputs. Keeping them apart matters because the two fail differently: a bug
//! in the classifier is a wrong verdict about a graph, and a bug here is a
//! *missing edge* — an artifact that looks unreachable because nobody wrote
//! down what reaches it.
//!
//! # Enumeration failure is never absence
//!
//! Every store this reads can fail to enumerate. When one does, the artifacts
//! it would have named are recorded as [`unresolved`] rather than omitted,
//! because an unreadable store and an empty store look identical from here and
//! only one of them is safe to collect against.
//!
//! [`unresolved`]: crate::app::gc::RootGraph::unresolved
//!
//! # The Publication family is the hard case
//!
//! An attempt that never dispatched has no outcome, no receipt and no external
//! trace — it looks like garbage from every angle except the one that matters:
//! while its journal is unresolved, that artifact is how recovery learns what
//! was attempted. So an unresolved journal roots the attempt it names, and only
//! a terminal journal releases it.

use std::collections::{BTreeMap, BTreeSet};

use crate::app::gc::{ArtifactKey, RootGraph};
use crate::project::layout::DraftLayout;
use crate::support::error::DraftResult;

/// A namespaced artifact key.
///
/// Namespaced because the key space is shared: two domains may legitimately
/// mint the same opaque id, and a collision would make one domain's root
/// retain the other's garbage — or, far worse, make one domain's garbage look
/// like the other's root.
fn key(kind: &str, id: impl AsRef<str>) -> ArtifactKey {
    format!("{kind}:{}", id.as_ref())
}

/// Build the root graph for one project.
pub fn build(layout: &DraftLayout) -> DraftResult<RootGraph> {
    let mut graph = RootGraph::default();

    baselines(layout, &mut graph)?;
    changes(layout, &mut graph)?;
    judgements(layout, &mut graph);
    semantics(layout, &mut graph);
    publications(layout, &mut graph);
    Ok(graph)
}

/// The accepted Baseline, its lineage, and everything the roots reference.
fn baselines(layout: &DraftLayout, graph: &mut RootGraph) -> DraftResult<()> {
    let store = crate::dcg::baseline::BaselineStore::new(layout.baselines_dir());
    let Some(current) = crate::dcg::baseline::current_baseline(layout)? else {
        return Ok(());
    };
    // The whole lineage, not only the accepted node. A Baseline's parent is
    // what makes its history readable; collecting an ancestor would leave a
    // chain that verifies to nowhere.
    for baseline in store.lineage(&current)? {
        let baseline_key = key("baseline", baseline.to_string());
        graph.roots.insert(baseline_key.clone());
        let mut referenced = BTreeSet::new();
        match store.manifest(&baseline) {
            Ok(Some(manifest)) => {
                referenced.insert(key(
                    "state-root",
                    manifest.project_state_root.digest().as_str(),
                ));
                referenced.insert(key(
                    "evidence-root",
                    manifest.state_evidence_root.digest().as_str(),
                ));
                referenced.insert(key(
                    "coverage-root",
                    manifest.coverage_evidence_root.digest().as_str(),
                ));
                if let Some(parent) = &manifest.parent_baseline_id {
                    referenced.insert(key("baseline", parent.to_string()));
                }
            }
            Ok(None) | Err(_) => {
                graph.unresolved.insert(
                    baseline_key.clone(),
                    "the accepted Baseline's manifest could not be read".into(),
                );
            }
        }
        match store.composition(&baseline) {
            Ok(Some(composition)) => {
                for provenance in composition.resource_provenance.values() {
                    referenced.insert(key("provider-binding", provenance.binding.as_str()));
                    referenced.insert(key(
                        "semantic-definition",
                        provenance.semantic_definition.digest().as_str(),
                    ));
                }
                for state in composition.accepted_state.values() {
                    referenced.insert(key("resource-state", state.digest().as_str()));
                }
            }
            Ok(None) | Err(_) => {
                graph.unresolved.insert(
                    key("baseline-composition", baseline.to_string()),
                    "a Baseline's composition could not be read".into(),
                );
            }
        }
        graph.references.insert(baseline_key, referenced);
    }
    Ok(())
}

/// Every Change, its current definition, and its sealed revisions.
fn changes(layout: &DraftLayout, graph: &mut RootGraph) -> DraftResult<()> {
    let store = crate::dcg::change::ChangeStore::new(layout.changes_dir());
    let revisions = crate::dcg::revision::RevisionStore::new(layout.revisions_dir());
    let changes = match store.list() {
        Ok(changes) => changes,
        Err(error) => {
            graph
                .unresolved
                .insert(key("changes", "all"), error.message);
            return Ok(());
        }
    };
    for change in changes {
        let change_key = key("change", change.id.to_string());
        // Every Change is a root, terminal ones included. Abandoning is a
        // statement about the future; the record of work that was tried and
        // stopped is frequently the part worth keeping.
        graph.roots.insert(change_key.clone());
        let mut referenced =
            BTreeSet::from([key("change-definition", change.current_definition.as_str())]);
        if let Ok(sealed) = revisions.list() {
            for revision in sealed.iter().filter(|value| value.change == change.id) {
                let revision_key = key("revision", revision.id.to_string());
                referenced.insert(revision_key.clone());
                let mut from_revision = BTreeSet::from([
                    key("change-definition", revision.definition.as_str()),
                    key("scope-resolution", revision.scope.as_str()),
                    key("baseline", revision.base_baseline.to_string()),
                    key("representation", revision.id.to_string()),
                ]);
                for resource in &revision.touched {
                    from_revision.insert(key("resource", resource.to_string()));
                }
                graph.references.insert(revision_key, from_revision);
            }
        }
        graph.references.insert(change_key, referenced);
    }
    Ok(())
}

/// Evidence, assessments, gates and decisions, each rooted by the revision it
/// binds.
///
/// Rooted rather than merely referenced: a judgement about a revision is
/// history in its own right, and a Change that is later abandoned does not make
/// the review that happened collectible.
fn judgements(layout: &DraftLayout, graph: &mut RootGraph) {
    let stores = crate::app::authorization::AuthorizationStores::for_layout(layout);
    let mut edges: BTreeMap<ArtifactKey, BTreeSet<ArtifactKey>> = BTreeMap::new();

    match stores.evidence.list() {
        Ok(all) => {
            for evidence in all {
                let evidence_key = key("evidence", evidence.id.to_string());
                graph.roots.insert(evidence_key.clone());
                let mut referenced =
                    BTreeSet::from([key("revision", evidence.revision.to_string())]);
                for input in &evidence.inputs {
                    referenced.insert(key("observation", input.id.to_string()));
                }
                edges.insert(evidence_key, referenced);
            }
        }
        Err(error) => {
            graph
                .unresolved
                .insert(key("evidence", "all"), error.message);
        }
    }

    let mut simple = |kind: &str, ids: DraftResult<Vec<(String, String)>>| match ids {
        Ok(all) => {
            for (id, revision) in all {
                let artifact = key(kind, id);
                graph.roots.insert(artifact.clone());
                edges.insert(artifact, BTreeSet::from([key("revision", revision)]));
            }
        }
        Err(error) => {
            graph.unresolved.insert(key(kind, "all"), error.message);
        }
    };
    simple(
        "assessment",
        stores.assessments.list().map(|all| {
            all.into_iter()
                .map(|value| (value.id.to_string(), value.revision.to_string()))
                .collect()
        }),
    );
    simple(
        "decision",
        stores.decisions.list().map(|all| {
            all.into_iter()
                .map(|value| (value.id.to_string(), value.revision.to_string()))
                .collect()
        }),
    );
    simple(
        "gate-evaluation",
        stores.gates.list().map(|all| {
            all.into_iter()
                .map(|value| (value.id.clone(), value.revision.to_string()))
                .collect()
        }),
    );

    graph.references.extend(edges);
}

/// Retained semantics contracts.
///
/// Roots in themselves, and deliberately not merely reachable from the
/// definitions that cite them: verifying what a historical state *meant* must
/// not require an installed extension, so the contract outlives every provider
/// that ever used it.
fn semantics(layout: &DraftLayout, graph: &mut RootGraph) {
    let directory = layout.semantics_contracts_dir();
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return;
    };
    for entry in entries.flatten() {
        if let Some(name) = entry.path().file_stem().and_then(|stem| stem.to_str()) {
            graph.roots.insert(key("semantics-contract", name));
        }
    }
}

/// The Publication family, and the journals that root unfinished work.
fn publications(layout: &DraftLayout, graph: &mut RootGraph) {
    let root = layout.publication_dir();
    let publications = crate::publication::store::PublicationStore::for_layout(layout);
    match publications.list() {
        Ok(all) => {
            for publication in all {
                let publication_key = key("publication", publication.to_string());
                graph.roots.insert(publication_key.clone());
                if let Ok(Some(record)) = publications.get(&publication) {
                    graph.references.insert(
                        publication_key,
                        BTreeSet::from([
                            key("promotion", record.promotion.to_string()),
                            key("baseline", record.baseline.to_string()),
                            key("provider-binding", record.route.binding().as_str()),
                        ]),
                    );
                }
            }
        }
        Err(error) => {
            graph
                .unresolved
                .insert(key("publication", "all"), error.message);
        }
    }

    // Every attempt artifact on disk enters the graph, whether or not anything
    // still names it. An artifact absent from the universe is not "collectible"
    // — it is unclassified, and GC would leave it forever.
    let attempt_store = crate::publication::store::PublicationAttemptStore::for_layout(layout);
    match attempt_store.list() {
        Ok(all) => {
            for attempt in all {
                graph
                    .references
                    .entry(key("attempt", attempt.to_string()))
                    .or_default();
            }
        }
        Err(error) => {
            graph
                .unresolved
                .insert(key("attempt", "all"), error.message);
        }
    }

    // Resolutions and retry authorizations are immutable history in their own
    // right. A superseded resolution keeps its receipt valid, and a spent
    // authorization is the record of why a duplicate risk was accepted —
    // neither becomes collectible by being finished with.
    let resolutions = crate::publication::resolve::ResolutionStore::for_layout(layout);
    match resolutions.list() {
        Ok(all) => {
            for id in all {
                graph.roots.insert(key("resolution", id));
            }
        }
        Err(error) => {
            graph
                .unresolved
                .insert(key("resolution", "all"), error.message);
        }
    }
    let authorizations = crate::publication::retry::RetryAuthorizationStore::for_layout(layout);
    match authorizations.list() {
        Ok(all) => {
            for id in all {
                graph.roots.insert(key("retry-authorization", id));
            }
        }
        Err(error) => {
            graph
                .unresolved
                .insert(key("retry-authorization", "all"), error.message);
        }
    }

    let journals = crate::publication::journal::PublicationJournalStore::new(root.join("journal"));
    let outcomes = crate::publication::outcome::PublicationOutcomeStore::new(root.join("outcome"));
    let attempts = match journals.list() {
        Ok(attempts) => attempts,
        Err(error) => {
            graph
                .unresolved
                .insert(key("attempt-journal", "all"), error.message);
            return;
        }
    };
    for attempt in attempts {
        let journal_key = key("attempt-journal", attempt.to_string());
        let attempt_key = key("attempt", attempt.to_string());
        let record = match journals.read_unlocked(&attempt) {
            Ok(Some(record)) => record,
            Ok(None) => continue,
            Err(error) => {
                // An unreadable journal is the one case where guessing is
                // worst: it may name an attempt that reached a provider.
                graph.unresolved.insert(journal_key, error.message);
                graph.unresolved.insert(
                    attempt_key,
                    "its journal could not be read, so what it reached is unknown".into(),
                );
                continue;
            }
        };

        let mut referenced = BTreeSet::from([
            attempt_key.clone(),
            key("publication", record.publication.to_string()),
        ]);
        // The outcome is reachable only through the exact attempt reference the
        // journal recorded, so an attempt that never dispatched cannot acquire
        // one by accident.
        let dispatched = record
            .state
            .dispatched_attempt()
            .or_else(|| record.state.concluded_attempt());
        if let Some(reference) = dispatched {
            match outcomes.primary_outcome(reference) {
                Ok(Some(outcome)) => {
                    if let Ok(digest) = outcome.digest() {
                        let outcome_key = key("outcome", digest.digest().as_str());
                        referenced.insert(outcome_key.clone());
                        graph.references.insert(
                            outcome_key,
                            BTreeSet::from([
                                key("attempt", attempt.to_string()),
                                key("receipt", outcome.receipt_id.to_string()),
                            ]),
                        );
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    graph
                        .unresolved
                        .insert(key("outcome", attempt.to_string()), error.message);
                }
            }
        }

        if record.state.is_terminal() {
            // Locally terminal: the disposition is durable and carries its own
            // evidence, so nothing further depends on the journal remaining.
            // A `Finalized` outcome path still roots its outcome as history; a
            // terminal `Abandoned` candidate roots nothing, which is what
            // finally releases a staged attempt that never dispatched.
            graph.roots.insert(journal_key.clone());
            if record.state.concluded_attempt().is_some() {
                graph.references.insert(journal_key, referenced);
            } else {
                graph.references.insert(
                    journal_key,
                    BTreeSet::from([key("publication", record.publication.to_string())]),
                );
            }
        } else {
            // Unresolved. Everything it names is input to finishing it.
            graph.active_journals.insert(journal_key, referenced);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication::control::PublicationControl;
    use crate::publication::journal::{
        AttemptJournal, AttemptJournalState, ControlTransition, NonCommitEvidence,
        PublicationJournalStore,
    };
    use draft_dcg_contract::ids::{PublicationAttemptId, PublicationId};

    fn layout(directory: &tempfile::TempDir) -> DraftLayout {
        DraftLayout::at(directory.path().join(".draft"))
    }

    fn publication() -> PublicationId {
        PublicationId::parse("pub_000000000001").unwrap()
    }

    fn control() -> PublicationControl {
        PublicationControl::initial(publication())
    }

    fn journal(attempt: &str, state: AttemptJournalState) -> AttemptJournal {
        AttemptJournal {
            generation: 0,
            attempt: PublicationAttemptId::parse(attempt).unwrap(),
            publication: publication(),
            state,
        }
    }

    fn prepared() -> AttemptJournalState {
        AttemptJournalState::AttemptPrepared {
            candidate_attempt_number: 1,
            allocation: ControlTransition {
                expected: control(),
                planned: control().advanced(|next| {
                    next.next_attempt_number = 2;
                }),
            },
            retry_authorization: None,
        }
    }

    fn write(layout: &DraftLayout, record: &AttemptJournal) {
        let store = PublicationJournalStore::new(layout.publication_dir().join("journal"));
        std::fs::create_dir_all(layout.publication_dir().join("journal")).unwrap();
        store
            .with_locked_attempt(&record.attempt, |guard| guard.open(record))
            .unwrap();
    }

    /// The case the whole root graph exists for.
    ///
    /// A staged attempt has no outcome, no receipt and no external trace. It
    /// looks like garbage from every angle except the one that matters: while
    /// its journal is unresolved, that artifact is how recovery learns what was
    /// attempted.
    #[test]
    fn an_unresolved_journal_roots_the_attempt_it_names() {
        let directory = tempfile::tempdir().unwrap();
        let layout = layout(&directory);
        let attempts = layout.publication_dir().join("attempts");
        std::fs::create_dir_all(&attempts).unwrap();
        std::fs::write(attempts.join("pat_000000000001.json"), b"{}").unwrap();
        write(&layout, &journal("pat_000000000001", prepared()));

        let graph = build(&layout).unwrap();
        let held = graph
            .active_journals
            .get("attempt-journal:pat_000000000001")
            .expect("an unresolved journal is an active transaction");
        assert!(held.contains("attempt:pat_000000000001"));

        let answers = crate::app::gc::classify(&graph);
        assert!(!answers["attempt:pat_000000000001"].may_collect());
    }

    /// And the case that finally releases it.
    ///
    /// `Abandoned` is terminal: the disposition is durable and carries its own
    /// classification evidence, so nothing further depends on the journal
    /// remaining. The attempt it once named becomes collectible.
    #[test]
    fn a_terminal_abandoned_journal_releases_its_staged_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let layout = layout(&directory);
        let attempt = PublicationAttemptId::parse("pat_000000000001").unwrap();
        write(&layout, &journal("pat_000000000001", prepared()));

        let store = PublicationJournalStore::new(layout.publication_dir().join("journal"));
        store
            .with_locked_attempt(&attempt, |guard| {
                guard.transition_locked(
                    &prepared(),
                    AttemptJournalState::Abandoned {
                        evidence: NonCommitEvidence {
                            candidate_attempt_number: 1,
                            observed_control: control(),
                            classified_at: draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
                        },
                    },
                )
            })
            .unwrap();

        // The staged artifact exists on disk, which is what makes the
        // question interesting: nothing external ever saw it, and only the
        // journal decided whether it could go.
        let attempts = layout.publication_dir().join("attempts");
        std::fs::create_dir_all(&attempts).unwrap();
        std::fs::write(attempts.join("pat_000000000001.json"), b"{}").unwrap();

        let graph = build(&layout).unwrap();
        assert!(
            !graph
                .active_journals
                .contains_key("attempt-journal:pat_000000000001"),
            "a terminal journal is no longer an unfinished transaction"
        );
        let answers = crate::app::gc::classify(&graph);
        assert!(
            answers["attempt:pat_000000000001"].may_collect(),
            "the staged attempt is released once its journal is terminal"
        );
        assert!(
            !answers["attempt-journal:pat_000000000001"].may_collect(),
            "the terminal record itself is history and stays"
        );
    }

    /// An unreadable store is not an empty one.
    #[test]
    fn a_journal_that_cannot_be_read_is_unresolved_rather_than_absent() {
        let directory = tempfile::tempdir().unwrap();
        let layout = layout(&directory);
        let journals = layout.publication_dir().join("journal");
        std::fs::create_dir_all(&journals).unwrap();
        std::fs::write(journals.join("pat_000000000001.json"), b"not json").unwrap();

        let graph = build(&layout).unwrap();
        assert!(graph.unresolved.contains_key("attempt:pat_000000000001"));
        let answers = crate::app::gc::classify(&graph);
        assert!(!answers["attempt:pat_000000000001"].may_collect());
    }
}
