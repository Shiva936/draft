//! The sole authoritative map of a project-local `.draft/` store.

use crate::support::error::DraftResult;
use crate::support::fsutil::ensure_dir;
use crate::support::hidden::{self, HiddenStatus};
use std::path::{Path, PathBuf};

/// Handle to a project `.draft/` store and its canonical layout.
#[derive(Debug, Clone)]
pub struct DraftLayout {
    pub draft_dir: PathBuf,
}

impl DraftLayout {
    /// Build the layout for a project root (the directory that contains `.draft`).
    pub fn for_root(root: &Path) -> Self {
        DraftLayout {
            draft_dir: root.join(".draft"),
        }
    }

    /// Build the layout given the `.draft` directory directly.
    pub fn at(draft_dir: impl Into<PathBuf>) -> Self {
        DraftLayout {
            draft_dir: draft_dir.into(),
        }
    }

    pub fn draft_dir(&self) -> &Path {
        &self.draft_dir
    }

    pub fn root(&self) -> PathBuf {
        self.draft_dir
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    }

    // ---- Top-level files -------------------------------------------------

    /// The project's own metadata.
    ///
    /// Named for what it holds. The old `workspace.json` predates the
    /// distinction between a *project* and a Change *workspace*, and keeping it
    /// would leave the two most easily confused concepts sharing a filename.
    pub fn project_json(&self) -> PathBuf {
        self.draft_dir.join("project.json")
    }
    pub fn config_toml(&self) -> PathBuf {
        self.draft_dir.join("config.toml")
    }
    pub fn ignore_file(&self) -> PathBuf {
        self.draft_dir.join(".ignore")
    }
    pub fn verify_toml(&self) -> PathBuf {
        self.draft_dir.join("verify.toml")
    }
    pub fn risk_toml(&self) -> PathBuf {
        self.draft_dir.join("risk.toml")
    }
    pub fn selected_change_file(&self) -> PathBuf {
        self.draft_dir.join("selected-change")
    }
    pub fn policy_toml(&self) -> PathBuf {
        self.draft_dir.join("policy.toml")
    }

    // ---- Events ----------------------------------------------------------

    pub fn events_dir(&self) -> PathBuf {
        self.draft_dir.join("events")
    }
    /// The sole authoritative Activity file.
    ///
    /// Framed records, not one JSON object per line — so the `.jsonl` suffix
    /// that a previous ledger carried would have misled every tool that met
    /// the file. There is exactly one of these and no compatibility reader.
    pub fn activity_log(&self) -> PathBuf {
        self.events_dir().join("events.log")
    }
    pub fn activity_index(&self) -> PathBuf {
        self.events_dir().join("events.index")
    }

    // ---- Receipts & transparency ----------------------------------------

    pub fn receipts_dir(&self) -> PathBuf {
        self.draft_dir.join("receipts")
    }
    pub fn receipt_file(&self, receipt_id: &str) -> PathBuf {
        self.receipts_dir().join(format!("{receipt_id}.json"))
    }
    /// Signed DCG receipt envelopes, stored create-once under their `rcp_` id.
    ///
    /// Separate from `receipts/` so the immutable-fact discipline of §2.45
    /// applies to the whole directory: every file in here is one canonical
    /// envelope bound to its digest, and nothing rewrites one.
    pub fn receipt_envelopes_dir(&self) -> PathBuf {
        self.receipts_dir().join("envelopes")
    }
    pub fn transparency_dir(&self) -> PathBuf {
        self.draft_dir.join("transparency")
    }
    pub fn transparency_chain(&self) -> PathBuf {
        self.transparency_dir().join("chain.log")
    }

    // ---- Accepted state / indexes / operation state ---------------------

    /// Accepted Baselines: their manifests and acceptance records.
    pub fn baselines_dir(&self) -> PathBuf {
        self.draft_dir.join("baselines")
    }
    /// Canonical observations and the runs that produced them.
    pub fn observations_dir(&self) -> PathBuf {
        self.draft_dir.join("observations")
    }
    pub fn index_dir(&self) -> PathBuf {
        self.draft_dir.join("index")
    }
    /// The derived summary the last collection wrote about the Change graph.
    pub fn change_graph_index(&self) -> PathBuf {
        self.index_dir().join("change-graph.json")
    }
    pub fn affected_path_index(&self) -> PathBuf {
        self.index_dir().join("affected-paths.json")
    }
    pub fn verification_cache_manifest(&self) -> PathBuf {
        self.index_dir().join("verification-cache.json")
    }
    pub fn workspace_hash_cache(&self) -> PathBuf {
        self.cache_sub("hashes").join("workspace-hash.json")
    }
    pub fn tmp_dir(&self) -> PathBuf {
        self.draft_dir.join("tmp")
    }
    pub fn op_tmp_dir(&self, op_id: &str) -> PathBuf {
        self.tmp_dir().join(op_id)
    }
    pub fn locks_dir(&self) -> PathBuf {
        self.draft_dir.join("locks")
    }
    pub fn objects_dir(&self) -> PathBuf {
        self.draft_dir.join("objects/blake3")
    }
    /// Compacted object storage.
    ///
    /// "Segments" rather than "packs": this is a storage-compaction detail and
    /// has never had anything to do with the retired Pack ontology. Sharing the
    /// word made an implementation concern look like a domain concept.
    pub fn object_segments_dir(&self) -> PathBuf {
        self.draft_dir.join("objects/segments")
    }
    /// Where the historical observation-provenance records for one observed
    /// state live.
    ///
    /// A directory rather than a file, because one authoritative
    /// `snapshot_digest` may have *many* immutable provenance records: the same
    /// state observed again later, or by a semantics-equivalent build, is a
    /// different historical observation and must not overwrite the first. Each
    /// record is named by its own `provenance_digest`, which makes the store an
    /// append-only multimap and re-recording an identical assembly idempotent.
    pub fn observation_provenance_dir(&self, snapshot_digest: &str) -> PathBuf {
        self.draft_dir
            .join("observation-provenance")
            .join(snapshot_digest.replace(':', "_"))
    }

    pub fn observation_provenance_file(
        &self,
        snapshot_digest: &str,
        provenance_digest: &str,
    ) -> PathBuf {
        self.observation_provenance_dir(snapshot_digest)
            .join(format!("{}.json", provenance_digest.replace(':', "_")))
    }

    // ---- Observation control -------------------------------------------

    /// Where the semantics in force, and any candidate for them, live.
    pub fn observation_dir(&self) -> PathBuf {
        self.draft_dir.join("observation")
    }

    /// The observation semantics this project is currently observed under.
    ///
    /// A single file, deliberately: there is exactly one answer to "what may
    /// Draft see", and a project with two would have no way to say which
    /// snapshot belonged to which.
    pub fn active_observation_context_file(&self) -> PathBuf {
        self.observation_dir().join("active-context.json")
    }

    /// A candidate context, waiting for a person to adopt or discard it.
    pub fn pending_observation_context_file(&self) -> PathBuf {
        self.observation_dir().join("pending-context.json")
    }

    /// The append-only history of adoptions.
    pub fn observation_transitions_dir(&self) -> PathBuf {
        self.observation_dir().join("transitions")
    }

    pub fn observation_transition_file(&self, transition_id: &str) -> PathBuf {
        self.observation_transitions_dir()
            .join(format!("{transition_id}.json"))
    }

    // ---- Acceptance ------------------------------------------------------

    /// Where acceptance evaluations and the provenance of the policy behind
    /// them live.
    pub fn acceptance_dir(&self) -> PathBuf {
        self.draft_dir.join("acceptance")
    }

    /// The latest acceptance evaluation for one Change.
    pub fn acceptance_evaluation_file(&self, change_id: &str) -> PathBuf {
        self.acceptance_dir().join(format!("{change_id}.json"))
    }

    /// Which policy artifacts an acceptance context was assembled from.
    ///
    /// Separate from the context itself, and keyed by its own digest rather
    /// than the context's. That is the whole point of the record: two package
    /// revisions with identical policy semantics produce the *same* context
    /// digest — so prior decisions stay valid — while their provenance differs.
    /// Keying these by context digest would make the second assembly overwrite
    /// the first, and a receipt could no longer name the exact artifacts it
    /// relied on. One context digest may therefore have many provenance
    /// records, and none of them is ever rewritten.
    pub fn acceptance_provenance_dir(&self) -> PathBuf {
        self.acceptance_dir().join("provenance")
    }

    pub fn acceptance_provenance_file(&self, provenance_digest: &str) -> PathBuf {
        self.acceptance_provenance_dir()
            .join(format!("{}.json", provenance_digest.replace(':', "_")))
    }

    /// Where the recovery anchors for one observed state live.
    ///
    /// Keyed by the snapshot's authoritative digest rather than its record id:
    /// two records of the same state share one set of anchors, because they
    /// describe the same thing.
    pub fn recovery_anchor_file(&self, snapshot_digest: &str) -> PathBuf {
        self.recovery_anchors_dir()
            .join(format!("{}.json", snapshot_digest.replace(':', "_")))
    }

    pub fn recovery_anchors_dir(&self) -> PathBuf {
        self.draft_dir.join("recovery-anchors")
    }

    pub fn snapshot_file(&self, snapshot_id: &str) -> PathBuf {
        self.snapshots_dir().join(format!("{snapshot_id}.json"))
    }
    pub fn snapshots_dir(&self) -> PathBuf {
        self.draft_dir.join("snapshots")
    }
    /// Mutable workspaces used while deriving immutable Change revisions.
    pub fn change_workspaces_dir(&self) -> PathBuf {
        self.draft_dir.join("change-workspaces")
    }
    pub fn change_workspace_dir(&self, change_id: impl ToString) -> PathBuf {
        self.change_workspaces_dir().join(change_id.to_string())
    }
    pub fn index_file(&self) -> PathBuf {
        self.indexes_dir().join("draft.sqlite")
    }

    pub fn tasks_dir(&self) -> PathBuf {
        self.draft_dir.join("tasks")
    }
    pub fn task_file(&self, id: &str) -> PathBuf {
        self.tasks_dir().join(format!("{id}.json"))
    }
    pub fn executions_dir(&self) -> PathBuf {
        self.draft_dir.join("executions")
    }
    pub fn execution_file(&self, id: &str) -> PathBuf {
        self.executions_dir().join(format!("{id}.json"))
    }
    /// Per-execution runtime state: task contract, logs, isolated workspace.
    pub fn runtime_dir(&self) -> PathBuf {
        self.draft_dir.join("runtime")
    }
    pub fn execution_runtime_dir(&self, execution_id: &str) -> PathBuf {
        self.runtime_dir().join(execution_id)
    }
    pub fn execution_contract_file(&self, execution_id: &str) -> PathBuf {
        self.execution_runtime_dir(execution_id)
            .join("task_contract.json")
    }
    pub fn execution_work_dir(&self, execution_id: &str) -> PathBuf {
        self.execution_runtime_dir(execution_id).join("work")
    }
    /// Canonical derived indexes tree (`.draft/indexes/`).
    pub fn indexes_dir(&self) -> PathBuf {
        self.draft_dir.join("indexes")
    }
    pub fn task_indexes_dir(&self) -> PathBuf {
        self.indexes_dir().join("tasks")
    }
    pub fn task_name_index(&self) -> PathBuf {
        self.task_indexes_dir().join("by-name.json")
    }
    pub fn evidence_dir(&self) -> PathBuf {
        self.draft_dir.join("evidence")
    }
    pub fn decisions_dir(&self) -> PathBuf {
        self.draft_dir.join("decisions")
    }
    pub fn waivers_dir(&self) -> PathBuf {
        self.draft_dir.join("waivers")
    }
    /// Mutable Workspaces: where a Change's work happens between resolving
    /// its scope and sealing a revision.
    pub fn workspaces_dir(&self) -> PathBuf {
        self.draft_dir.join("workspaces")
    }
    pub fn recovery_dir(&self) -> PathBuf {
        self.draft_dir.join("recovery")
    }
    pub fn backups_dir(&self) -> PathBuf {
        self.draft_dir.join("backups")
    }
    pub fn lock_file(&self, name: &str) -> PathBuf {
        self.locks_dir().join(format!("{name}.lock"))
    }

    // ---- Change content -----------------------------------------------------------

    pub fn changes_content_dir(&self) -> PathBuf {
        self.draft_dir.join("changes")
    }
    pub fn change_content_dir(&self, change_id: impl ToString) -> PathBuf {
        self.changes_content_dir().join(change_id.to_string())
    }
    pub fn change_manifest(&self, change_id: &str) -> PathBuf {
        self.change_content_dir(change_id).join("manifest.json")
    }
    pub fn change_lock(&self, change_id: &str) -> PathBuf {
        self.change_content_dir(change_id).join("change.lock.json")
    }
    /// Where authoritative Change records live.
    ///
    /// Separate from the Change directory on purpose: a Change outlives any one
    /// revision of the work, so its record cannot be stored inside one.
    pub fn changes_dir(&self) -> PathBuf {
        self.draft_dir.join("graph/changes")
    }

    /// Immutable Change definitions, under the §2.45 create-once binding.
    pub fn definitions_dir(&self) -> PathBuf {
        self.draft_dir.join("definitions")
    }

    /// Immutable scope resolutions. Separate from definitions because a scope
    /// is resolved once *against a named Baseline* — the resolution is a
    /// different fact from the declaration it resolves.
    pub fn scope_resolutions_dir(&self) -> PathBuf {
        self.draft_dir.join("resolutions")
    }

    /// Sealed Change revisions.
    pub fn revisions_dir(&self) -> PathBuf {
        self.draft_dir.join("revisions")
    }

    /// The project control record and its stable lock sidecar.
    pub fn project_control_dir(&self) -> PathBuf {
        self.draft_dir.join("project")
    }

    /// Provider bindings: revisioned pointers, each on its own sidecar.
    pub fn provider_bindings_dir(&self) -> PathBuf {
        self.draft_dir.join("provider-bindings")
    }

    /// The immutable semantic definitions and operational profiles bindings
    /// point at. Retained forever: a Baseline's provenance names one.
    pub fn provider_definitions_dir(&self) -> PathBuf {
        self.draft_dir.join("provider-definitions")
    }

    /// Retained `ResourceStateSemanticsContract` objects.
    ///
    /// A GC root in their own right: verifying what a historical state *meant*
    /// must not require an installed extension.
    pub fn semantics_contracts_dir(&self) -> PathBuf {
        self.draft_dir.join("semantics-contracts")
    }

    /// Derived explanations, one per sealed revision.
    pub fn representations_dir(&self) -> PathBuf {
        self.draft_dir.join("representations")
    }

    /// Recorded gate evaluations.
    pub fn gates_dir(&self) -> PathBuf {
        self.draft_dir.join("gates")
    }

    /// Recorded reviews — the act of looking, distinct from a Decision.
    pub fn reviews_dir(&self) -> PathBuf {
        self.draft_dir.join("reviews")
    }

    /// Recorded risk assessments.
    pub fn assessments_dir(&self) -> PathBuf {
        self.draft_dir.join("assessments")
    }

    /// The whole Publication subtree: registry, attempts, journals, control,
    /// outcome and resolution heads, and retry authorizations.
    pub fn publication_dir(&self) -> PathBuf {
        self.draft_dir.join("publication")
    }

    /// Issued authority grants.
    pub fn authority_grants_dir(&self) -> PathBuf {
        self.draft_dir.join("security/grants")
    }

    /// Immutable authority revocations.
    pub fn authority_revocations_dir(&self) -> PathBuf {
        self.draft_dir.join("security/revocations")
    }

    /// `ProjectSecurityState` objects, stored by their own digest.
    pub fn security_states_dir(&self) -> PathBuf {
        self.draft_dir.join("security/states")
    }

    /// The immutable Publication objects the registry maps request keys onto.
    pub fn publications_dir(&self) -> PathBuf {
        self.publication_dir().join("publications")
    }

    /// Immutable `PublicationAttempt` artifacts.
    ///
    /// An artifact here is never proof that anything was sent: only a journal
    /// that durably reached `Dispatching` is.
    pub fn publication_attempts_dir(&self) -> PathBuf {
        self.publication_dir().join("attempts")
    }

    /// Immutable resolutions and the per-outcome heads naming the one in force.
    pub fn publication_resolutions_dir(&self) -> PathBuf {
        self.publication_dir().join("resolutions")
    }

    /// Promotion journals and the immutable records they finalize into.
    pub fn promotion_journals_dir(&self) -> PathBuf {
        self.draft_dir.join("promotions/journal")
    }

    pub fn promotion_records_dir(&self) -> PathBuf {
        self.draft_dir.join("promotions/records")
    }

    /// Product-lease state, owned by `services/locks`.
    pub fn leases_dir(&self) -> PathBuf {
        self.draft_dir.join("leases")
    }

    /// Where the trust registry the `TrustReadFence` guards lives.
    pub fn trust_registry_dir(&self) -> PathBuf {
        self.draft_dir.join("trust-registry")
    }

    /// Recorded Operations.
    pub fn operations_dir(&self) -> PathBuf {
        self.draft_dir.join("graph/operations")
    }

    /// Per-store `MutationJournal` records.
    ///
    /// One directory for every audited record family: a journal is looked up
    /// by record key, and keys are already globally distinct.
    pub fn journals_dir(&self) -> PathBuf {
        self.draft_dir.join("journals")
    }

    pub fn change_changes(&self, change_id: &str) -> PathBuf {
        self.change_content_dir(change_id).join("changes.json")
    }
    pub fn change_risk(&self, change_id: &str) -> PathBuf {
        self.change_content_dir(change_id).join("risk.json")
    }
    pub fn change_verify(&self, change_id: &str) -> PathBuf {
        self.change_content_dir(change_id).join("verify.json")
    }
    pub fn change_impact(&self, change_id: &str) -> PathBuf {
        self.change_content_dir(change_id).join("impact.json")
    }
    pub fn change_receipts(&self, change_id: &str) -> PathBuf {
        self.change_content_dir(change_id).join("receipts.json")
    }

    // ---- Checkpoints / import / export ----------------------------------

    pub fn checkpoints_dir(&self) -> PathBuf {
        self.draft_dir.join("checkpoints")
    }
    pub fn imports_dir(&self) -> PathBuf {
        self.draft_dir.join("imports")
    }
    pub fn quarantine_dir(&self) -> PathBuf {
        self.imports_dir().join("quarantine")
    }
    pub fn exports_dir(&self) -> PathBuf {
        self.draft_dir.join("exports")
    }

    // ---- Impact index ------------------------------------------------------------

    pub fn impact_dir(&self) -> PathBuf {
        self.draft_dir.join("impact")
    }
    pub fn impact_index_db(&self) -> PathBuf {
        self.impact_dir().join("index.db")
    }
    pub fn impact_elements_db(&self) -> PathBuf {
        self.impact_dir().join("elements.db")
    }

    // ---- Cache & adapters ------------------------------------------------

    pub fn cache_dir(&self) -> PathBuf {
        self.draft_dir.join("cache")
    }
    pub fn cache_sub(&self, name: &str) -> PathBuf {
        self.cache_dir().join(name)
    }
    pub fn adapters_dir(&self) -> PathBuf {
        self.draft_dir.join("adapters")
    }
    pub fn adapter_overrides_dir(&self) -> PathBuf {
        self.adapters_dir().join("project-overrides")
    }

    /// Create the full v0.3.4 project tree and mark `.draft/` hidden.
    /// Idempotent; leaves existing files untouched.
    pub fn create_all(&self) -> DraftResult<HiddenStatus> {
        for dir in [
            self.draft_dir.clone(),
            self.objects_dir(),
            self.object_segments_dir(),
            self.events_dir(),
            self.receipts_dir(),
            self.transparency_dir(),
            self.baselines_dir(),
            self.observations_dir(),
            self.changes_content_dir(),
            self.changes_dir(),
            self.definitions_dir(),
            self.scope_resolutions_dir(),
            self.revisions_dir(),
            self.representations_dir(),
            self.operations_dir(),
            self.project_control_dir(),
            self.provider_bindings_dir(),
            self.provider_definitions_dir(),
            self.semantics_contracts_dir(),
            self.gates_dir(),
            self.reviews_dir(),
            self.assessments_dir(),
            self.publication_dir(),
            self.authority_grants_dir(),
            self.authority_revocations_dir(),
            self.security_states_dir(),
            self.promotion_journals_dir(),
            self.promotion_records_dir(),
            self.leases_dir(),
            self.journals_dir(),
            self.snapshots_dir(),
            self.checkpoints_dir(),
            self.index_dir(),
            self.tmp_dir(),
            self.locks_dir(),
            self.tasks_dir(),
            self.executions_dir(),
            self.runtime_dir(),
            self.indexes_dir(),
            self.task_indexes_dir(),
            self.evidence_dir(),
            self.decisions_dir(),
            self.waivers_dir(),
            self.workspaces_dir(),
            self.recovery_dir(),
            self.backups_dir(),
            self.imports_dir(),
            self.quarantine_dir(),
            self.exports_dir(),
            self.impact_dir(),
            self.cache_dir(),
            self.cache_sub("hashes"),
            self.cache_sub("risk"),
            self.cache_sub("verify"),
            self.cache_sub("test-selection"),
            self.cache_sub("fuzz-selection"),
            self.adapters_dir(),
            self.adapter_overrides_dir(),
            self.observation_dir(),
            self.observation_transitions_dir(),
            self.acceptance_dir(),
            self.acceptance_provenance_dir(),
        ] {
            ensure_dir(&dir)?;
        }
        Ok(hidden::ensure_hidden(&self.draft_dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_all_builds_canonical_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DraftLayout::for_root(tmp.path());
        assert!(p.create_all().unwrap().is_ok());
        assert!(p.events_dir().is_dir());
        assert!(p.transparency_dir().is_dir());
        assert!(p.quarantine_dir().is_dir());
        assert!(p.impact_dir().is_dir());
        assert_eq!(
            p.change_manifest("chg_abc"),
            p.draft_dir().join("changes/chg_abc/manifest.json")
        );
        assert_eq!(p.activity_log(), p.draft_dir().join("events/events.log"));
    }
}
