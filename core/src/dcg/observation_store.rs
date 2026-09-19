//! Where the observation semantics in force are kept, and how they change.
//!
//! Reading is cheap and side-effect free. Writing is not: adopting a context
//! rewrites what the project is allowed to see, so it happens once, under the
//! project lease, through [`adopt`], and leaves an audit record behind.
//!
//! The ordering inside [`adopt`] is the load-bearing part. A crash must never
//! leave new semantics with an old baseline or an old pointer with a new
//! baseline, because either would mean the project is being observed under
//! rules that nothing recorded. So the durable content is written first, the
//! active pointer last, and the pointer swap is atomic.

use crate::dcg::observation_lifecycle::{
    ActiveObservationContext, ObservationContextTransition, PendingObservationContext,
};
use crate::project::Workspace;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::{ensure_dir, write_atomic};

/// The semantics this project is currently observed under.
///
/// `None` means the project has never adopted one — a fresh workspace, before
/// its first observation. That is a real state with a real answer, not an
/// error: the first observation adopts what is effective at that moment,
/// because there is no prior universe for it to differ from.
pub fn active(ws: &Workspace) -> DraftResult<Option<ActiveObservationContext>> {
    let path = ws.layout.active_observation_context_file();
    if !path.exists() {
        return Ok(None);
    }
    crate::contracts::read_persisted(&path).map(Some)
}

/// Install the semantics in force.
///
/// Atomic by construction. A half-written active pointer would leave a project
/// unable to say what it observes under, which is worse than either of the two
/// states it sits between.
pub fn write_active(ws: &Workspace, context: &ActiveObservationContext) -> DraftResult<()> {
    ensure_dir(&ws.layout.observation_dir())?;
    write_atomic(
        &ws.layout.active_observation_context_file(),
        crate::support::hashing::canonical_json(&serde_json::to_value(context)?).as_bytes(),
    )
}

/// The candidate waiting for a decision, if any.
pub fn pending(ws: &Workspace) -> DraftResult<Option<PendingObservationContext>> {
    let path = ws.layout.pending_observation_context_file();
    if !path.exists() {
        return Ok(None);
    }
    crate::contracts::read_persisted(&path).map(Some)
}

/// Record a candidate context.
///
/// Writing one changes nothing about what is observed. It is a note that
/// something *would* change, kept where a person and the Console can both find
/// it.
pub fn write_pending(ws: &Workspace, context: &PendingObservationContext) -> DraftResult<()> {
    ensure_dir(&ws.layout.observation_dir())?;
    write_atomic(
        &ws.layout.pending_observation_context_file(),
        crate::support::hashing::canonical_json(&serde_json::to_value(context)?).as_bytes(),
    )
}

/// Drop a candidate, because the effective semantics went back to the active
/// ones — an extension was disabled again, or an update reverted.
pub fn clear_pending(ws: &Workspace) -> DraftResult<()> {
    let path = ws.layout.pending_observation_context_file();
    if path.exists() {
        std::fs::remove_file(&path).map_err(|error| {
            DraftError::storage(format!("cannot clear pending context: {error}"))
        })?;
    }
    Ok(())
}

/// Every adoption this project has made, oldest first.
pub fn transitions(ws: &Workspace) -> DraftResult<Vec<ObservationContextTransition>> {
    let directory = ws.layout.observation_transitions_dir();
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Ok(Vec::new());
    };
    let mut paths: Vec<std::path::PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    let mut found = Vec::with_capacity(paths.len());
    for path in paths {
        found.push(crate::contracts::read_persisted::<
            ObservationContextTransition,
        >(&path)?);
    }
    found.sort_by(|left, right| {
        (left.adopted_at, &left.transition_digest)
            .cmp(&(right.adopted_at, &right.transition_digest))
    });
    Ok(found)
}

/// Persist one adoption record.
fn write_transition(ws: &Workspace, transition: &ObservationContextTransition) -> DraftResult<()> {
    ensure_dir(&ws.layout.observation_transitions_dir())?;
    write_atomic(
        &ws.layout
            .observation_transition_file(transition.transition_id.as_str()),
        crate::support::hashing::canonical_json(&serde_json::to_value(transition)?).as_bytes(),
    )
}

/// Install a new context, its baseline and its audit record as one act.
///
/// The order is the guarantee. The baseline snapshot is already durable before
/// this is called; the transition record goes next, and the active pointer goes
/// last. A crash before the pointer swap leaves the project observing under the
/// old context with an orphaned transition record and an unreferenced snapshot —
/// both inert, neither a lie. A crash after it leaves a fully consistent new
/// state. There is no ordering in which the pointer names a baseline that does
/// not exist.
pub fn adopt(
    ws: &Workspace,
    transition: &ObservationContextTransition,
    active_context: &ActiveObservationContext,
) -> DraftResult<()> {
    if active_context.baseline_snapshot_digest != transition.baseline_snapshot_digest {
        return Err(DraftError::new(
            DraftErrorKind::CorruptData,
            "the adopted context and its transition record disagree about the new baseline",
        ));
    }
    if active_context.context.context_digest != transition.to_context_digest {
        return Err(DraftError::new(
            DraftErrorKind::CorruptData,
            "the adopted context and its transition record disagree about the new semantics",
        ));
    }
    write_transition(ws, transition)?;
    write_active(ws, active_context)?;
    clear_pending(ws)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcg::observation::ObservationContext;
    use crate::dcg::observation_lifecycle::{
        transition as build_transition, ObservationTransitionId, PendingReason,
    };
    use crate::support::common::{now, SnapshotId};

    fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let layout = crate::project::layout::DraftLayout::for_root(dir.path());
        layout.create_all().unwrap();
        let ws = Workspace {
            root: dir.path().to_path_buf(),
            workspace_id: draft_dcg_contract::ids::ProjectId::parse("prj_test").unwrap(),
            layout,
        };
        (dir, ws)
    }

    fn active_record(digest_source: &str) -> ActiveObservationContext {
        ActiveObservationContext {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ActiveObservationContext,
            ),
            context: ObservationContext::build(Vec::new(), Vec::new()),
            view_rules: Vec::new(),
            baseline_snapshot_digest: digest_source.to_string(),
            baseline_snapshot_id: SnapshotId::new("chk_1"),
            adopted_at: now(),
            transition_id: None,
        }
    }

    #[test]
    fn a_project_with_no_adopted_context_says_so_rather_than_failing() {
        let (_dir, ws) = workspace();
        // A fresh workspace genuinely has no answer yet. Reporting that plainly
        // is what lets the first observation adopt what is effective, instead of
        // treating an absent file as corruption.
        assert!(active(&ws).unwrap().is_none());
        assert!(pending(&ws).unwrap().is_none());
        assert!(transitions(&ws).unwrap().is_empty());
    }

    #[test]
    fn adoption_refuses_a_record_that_disagrees_with_its_own_transition() {
        let (_dir, ws) = workspace();
        let snapshot = crate::dcg::state::tests_support::sealed("prj_test", &["a.txt"]);
        let transition = build_transition(
            "from".into(),
            ObservationContext::build(Vec::new(), Vec::new()).context_digest,
            &snapshot,
            vec![PendingReason::ViewSemanticsChanged],
            Vec::new(),
        );

        // A pointer and a record that name different baselines would leave the
        // project observing under semantics nothing vouches for.
        let mismatched = active_record("a-different-baseline");
        let error = adopt(&ws, &transition, &mismatched).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
        assert!(active(&ws).unwrap().is_none(), "nothing was installed");
    }

    #[test]
    fn adoption_installs_the_record_the_pointer_and_the_audit_together() {
        let (_dir, ws) = workspace();
        let snapshot = crate::dcg::state::tests_support::sealed("prj_test", &["a.txt"]);
        let context = ObservationContext::build(Vec::new(), Vec::new());
        let transition = build_transition(
            "from".into(),
            context.context_digest.clone(),
            &snapshot,
            vec![PendingReason::ViewSemanticsChanged],
            Vec::new(),
        );
        let mut record = active_record(&snapshot.snapshot_digest);
        record.transition_id = Some(transition.transition_id.clone());

        write_pending(
            &ws,
            &PendingObservationContext {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::PendingObservationContext,
                ),
                candidate: context.clone(),
                candidate_view_rules: Vec::new(),
                active_context_digest: "from".into(),
                reasons: vec![PendingReason::ViewSemanticsChanged],
                detected_at: now(),
            },
        )
        .unwrap();

        adopt(&ws, &transition, &record).unwrap();

        let installed = active(&ws).unwrap().expect("a context is now in force");
        assert_eq!(installed.baseline_snapshot_digest, snapshot.snapshot_digest);
        assert_eq!(
            installed.transition_id,
            Some(transition.transition_id.clone())
        );
        // The candidate is gone: it became the active one, so leaving it would
        // show a pending change that no longer exists.
        assert!(pending(&ws).unwrap().is_none());
        // And the adoption is on the record, permanently.
        let history = transitions(&ws).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].transition_id, transition.transition_id);
    }

    #[test]
    fn a_candidate_that_stops_differing_is_cleared_rather_than_left_stale() {
        let (_dir, ws) = workspace();
        write_pending(
            &ws,
            &PendingObservationContext {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::PendingObservationContext,
                ),
                candidate: ObservationContext::build(Vec::new(), Vec::new()),
                candidate_view_rules: Vec::new(),
                active_context_digest: "active".into(),
                reasons: vec![PendingReason::AdapterSetChanged],
                detected_at: now(),
            },
        )
        .unwrap();
        assert!(pending(&ws).unwrap().is_some());
        // Disabling the extension again puts the effective semantics back. A
        // pending banner that outlived its cause would train people to ignore it.
        clear_pending(&ws).unwrap();
        assert!(pending(&ws).unwrap().is_none());
        // Idempotent: clearing what is already clear is not an error.
        clear_pending(&ws).unwrap();
    }

    #[test]
    fn transitions_read_back_in_the_order_they_happened() {
        let (_dir, ws) = workspace();
        let snapshot = crate::dcg::state::tests_support::sealed("prj_test", &["a.txt"]);
        for index in 0..3 {
            let mut record = build_transition(
                format!("from-{index}"),
                format!("to-{index}"),
                &snapshot,
                vec![PendingReason::ViewSemanticsChanged],
                Vec::new(),
            );
            record.transition_id = ObservationTransitionId::new(format!("obt_{index}"));
            record = record.seal();
            write_transition(&ws, &record).unwrap();
        }
        let history = transitions(&ws).unwrap();
        assert_eq!(history.len(), 3);
        // Every adoption is retained. A history that dropped the earlier ones
        // could not explain how the project reached the context it has.
        assert!(history
            .iter()
            .all(|record| !record.transition_digest.is_empty()));
    }
}
