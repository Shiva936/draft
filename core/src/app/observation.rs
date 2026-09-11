//! Observing the project and turning what was seen into facts.
//!
//! Split out of `app/mod.rs`; these are `App` methods and behave
//! identically to when they lived there.

use super::*;

impl App {
    /// The observation semantics currently in force.
    /// The candidate semantics waiting for a decision, if any.
    pub fn observation_pending(
        &self,
        cwd: &Path,
    ) -> DraftResult<Option<crate::dcg::observation_lifecycle::PendingObservationContext>> {
        let ws = self.open(cwd)?;
        let active = self.ensure_active_context(&ws)?;
        self.refresh_pending_context(&ws, &active)
    }

    /// What adopting the pending candidate would do — without doing any of it.
    ///
    /// The trial observation here is thrown away. Nothing is written: no
    /// snapshot file, no provenance record, no anchors, no change to the active
    /// pointer. Previewing a change must never be a way of making it, or the
    /// step that exists to let somebody look before deciding would itself be
    /// the decision.
    pub fn observation_preview(
        &self,
        cwd: &Path,
    ) -> DraftResult<crate::dcg::observation_lifecycle::ObservationContextPreview> {
        use crate::dcg::observation_lifecycle::{binding_changes, ObservationContextPreview};
        let ws = self.open(cwd)?;
        let active = self.ensure_active_context(&ws)?;
        let Some(pending) = self.refresh_pending_context(&ws, &active)? else {
            return Err(DraftError::not_found(
                "no pending observation context; the installed extensions observe exactly as the \
                 adopted semantics do",
            ));
        };

        // Two enumerations, neither persisted: one under the semantics in
        // force, one under the candidate's. Both go through the adapter port,
        // exactly as a real observation does, because a preview that used a
        // different code path would be a preview of something else.
        let source = crate::dcg::filesystem_source::FilesystemSource::new(&ws);
        let current = crate::dcg::source::ResourceSource::enumerate(
            &source,
            &crate::dcg::source::ViewRules {
                exclusions: active.view_rules.clone(),
            },
        )?;
        let candidate = crate::dcg::source::ResourceSource::enumerate(
            &source,
            &crate::dcg::source::ViewRules {
                exclusions: pending.candidate_view_rules.clone(),
            },
        )?;

        let current_locators: std::collections::BTreeSet<_> = current
            .resources
            .iter()
            .map(|observed| observed.state.locator.clone())
            .collect();
        let candidate_locators: std::collections::BTreeSet<_> = candidate
            .resources
            .iter()
            .map(|observed| observed.state.locator.clone())
            .collect();

        let (added_bindings, removed_bindings, changed_bindings) =
            binding_changes(&active.context, &pending.candidate);

        Ok(ObservationContextPreview {
            active_context_digest: active.context.context_digest.clone(),
            candidate_context_digest: pending.candidate.context_digest.clone(),
            reasons: pending.reasons.clone(),
            added_bindings,
            removed_bindings,
            changed_bindings,
            would_enter: candidate_locators
                .difference(&current_locators)
                .cloned()
                .collect(),
            would_leave: current_locators
                .difference(&candidate_locators)
                .cloned()
                .collect(),
            would_supersede: self.context_sensitive_work(&ws, &active.context.context_digest)?,
        })
    }

    /// Adopt the pending semantics: one atomic, audited act.
    ///
    /// Runs under the project lease, so two callers cannot adopt at once and
    /// leave the project with two baselines. The new baseline is observed and
    /// made durable *before* the active pointer moves, so a crash can only ever
    /// leave the old semantics with the old baseline, or the new with the new —
    /// never a mixture.
    pub fn observation_adopt(
        &self,
        cwd: &Path,
    ) -> DraftResult<crate::dcg::observation_lifecycle::ObservationContextTransition> {
        use crate::dcg::observation_lifecycle::{transition, ActiveObservationContext};
        let ws = self.open(cwd)?;
        let operation_id = crate::support::common::OperationId::generate();
        let leases = crate::execution::lease::LeaseStore::at(ws.layout.locks_dir());
        let _lease = leases.acquire(
            &format!("workspace-{}", ws.workspace_id),
            operation_id.clone(),
            chrono::Duration::minutes(2),
        )?;

        // Re-read inside the lease. Another caller may have adopted while this
        // one waited, and adopting a candidate that is no longer pending would
        // rebaseline for no reason.
        let active = self.ensure_active_context(&ws)?;
        let Some(pending) = self.refresh_pending_context(&ws, &active)? else {
            return Err(DraftError::not_found(
                "no pending observation context to adopt",
            ));
        };

        let superseded = self.context_sensitive_work(&ws, &active.context.context_digest)?;

        // The new baseline, observed under the newly adopted semantics and
        // durable before anything points at it.
        let (baseline, _) = Snapshotter::new(&ws, pending.candidate_view_rules.clone())?
            .create_snapshot(
                resolve_actor(&ws.layout.draft_dir)?,
                &pending.candidate.context_digest,
            )?;

        let record = transition(
            active.context.context_digest.clone(),
            pending.candidate.context_digest.clone(),
            &baseline,
            pending.reasons.clone(),
            superseded,
        );
        let adopted = ActiveObservationContext {
            schema_version: current_version(ContractId::ActiveObservationContext),
            context: pending.candidate.clone(),
            view_rules: pending.candidate_view_rules.clone(),
            baseline_snapshot_digest: baseline.snapshot_digest.clone(),
            baseline_snapshot_id: baseline.id.clone(),
            adopted_at: now(),
            transition_id: Some(record.transition_id.clone()),
        };
        crate::dcg::observation_store::adopt(&ws, &record, &adopted)?;

        ws.events()?.append(
            crate::activity::EventKind::ResourceObserved,
            Some(record.transition_id.to_string()),
            serde_json::json!({
                "from": record.from_context_digest,
                "to": record.to_context_digest,
                "baseline": record.baseline_snapshot_digest,
                "superseded": record.superseded.len(),
            }),
        )?;
        Ok(record)
    }

    /// Every adoption this project has made.
    pub fn observation_transitions(
        &self,
        cwd: &Path,
    ) -> DraftResult<Vec<crate::dcg::observation_lifecycle::ObservationContextTransition>> {
        let ws = self.open(cwd)?;
        crate::dcg::observation_store::transitions(&ws)
    }

    pub fn observation_context(&self, cwd: &Path) -> DraftResult<ObservationContext> {
        let ws = self.open(cwd)?;
        // The semantics *in force*, which is not the same question as what the
        // installed extensions would observe under. Reading the second and
        // calling it the first is exactly what lets an install silently rewrite
        // what a project claims it saw; the difference between them is a
        // pending context, and it is answered by `observation_pending`.
        Ok(self.ensure_active_context(&ws)?.context)
    }

    /// Which domains one observation covered, and what it could not see.
    pub fn observation_coverage(&self, cwd: &Path) -> DraftResult<serde_json::Value> {
        let ws = self.open(cwd)?;
        let snapshot = self.observe(&ws)?;
        Ok(serde_json::json!({
            "snapshot_id": snapshot.id.as_str(),
            "snapshot_digest": snapshot.snapshot_digest,
            "observation_context_digest": snapshot.observation_context_digest,
            "domains": snapshot.observation_map.domains,
            "resource_membership": snapshot.observation_map.resource_membership,
            "gaps": snapshot.gaps,
            "status": snapshot.observation_status(),
        }))
    }

    /// Every historical observation record for one observed state.
    ///
    /// A list, not a record: the same authoritative state observed again later,
    /// or by a semantics-equivalent build, is a *different* historical
    /// observation, and a receipt that relied on the first must keep pointing at
    /// the first.
    pub fn observation_provenance(
        &self,
        cwd: &Path,
        snapshot_digest: Option<&str>,
    ) -> DraftResult<Vec<crate::dcg::observation::ObservationRunProvenance>> {
        let ws = self.open(cwd)?;
        let digest = match snapshot_digest {
            Some(digest) => digest.to_string(),
            None => self.observe(&ws)?.snapshot_digest,
        };
        let directory = ws.layout.observation_provenance_dir(&digest);
        let Ok(entries) = std::fs::read_dir(&directory) else {
            return Ok(Vec::new());
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort();
        let mut out: Vec<crate::dcg::observation::ObservationRunProvenance> =
            Vec::with_capacity(paths.len());
        for path in paths {
            out.push(crate::contracts::read_persisted(&path)?);
        }
        out.sort_by(|left, right| {
            (left.assembled_at, &left.provenance_digest)
                .cmp(&(right.assembled_at, &right.provenance_digest))
        });
        Ok(out)
    }
}
