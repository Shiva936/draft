//! Unit tests for the core engines (operations, changes, risk, review,
//! verification, checkpoint, finalization, receipts).

use std::path::Path;

use crate::changes::{self, DraftChange, FileChangeRef, GroupingSource};
use crate::checkpoint;
use crate::common::WorkspacePath;
use crate::conflict::ConflictReport;
use crate::finalization::{self, FinalizationContext, FinalizationRequest};
use crate::identity::ActorRef;
use crate::operations::{NewOperation, OperationKind, OperationLog};
use crate::review::{ReviewDecisionKind, ReviewSession, ReviewState};
use crate::risk;
use crate::vcs::testing::FakeProvider;
use crate::vcs::traits::VcsProvider;
use crate::vcs::types::*;
use crate::verification::{self, VerificationCommand, VerificationPlan};
use crate::workspace::{self, Workspace};

fn make_ws(root: &Path) -> Workspace {
    workspace::initialize(root, root, FakeProvider::id(), true).unwrap()
}

fn log_for(ws: &Workspace) -> OperationLog {
    OperationLog::new(ws.layout(), ws.id.clone())
}

#[test]
fn operation_log_append_read_integrity_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let ws = make_ws(dir.path());
    let log = log_for(&ws);
    for _ in 0..3 {
        log.append(NewOperation::new(
            OperationKind::ChangeScanned,
            ActorRef::unknown(),
            ws.provider_id.clone(),
        ))
        .unwrap();
    }
    let all = log.read_all().unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].seq, 1);
    assert_eq!(all[2].seq, 3);
    // parent linkage
    assert_eq!(all[1].parent_ids, vec![all[0].id.clone()]);
    // integrity holds
    assert!(crate::operations::integrity::verify(&all[2]));
    // index rebuild matches
    let idx = log.rebuild_index().unwrap();
    assert_eq!(idx.last_seq, 3);
    assert!(log.contains_kind(OperationKind::ChangeScanned).unwrap());
    assert!(!log
        .contains_kind(OperationKind::FinalizationCompleted)
        .unwrap());
}

#[test]
fn risk_detects_secret_and_large_diff() {
    let cfg = crate::workspace::config::RiskConfig::default();
    let mut delta = ProviderDelta::default();
    delta.files.push(FileDelta {
        path: WorkspacePath::new("config.rs"),
        old_path: None,
        status: FileStatus::Modified,
        hunks: vec![DiffHunk {
            old_start: 1,
            old_lines: 0,
            new_start: 1,
            new_lines: 1,
            header: "@@".into(),
            lines: vec![DiffLine::Added(
                "let api_key = \"SUPERSECRETVALUE123\";".into(),
            )],
        }],
        binary: false,
        summarized: false,
        additions: 1,
        deletions: 0,
    });
    let summary = risk::evaluate(&delta, &cfg);
    assert_eq!(summary.level, risk::RiskLevel::Critical);
    assert!(summary
        .findings
        .iter()
        .any(|f| f.kind == risk::RiskFindingKind::SecretLikePattern));
    // The raw secret value is never stored in a finding message.
    assert!(summary
        .findings
        .iter()
        .all(|f| !f.message.contains("SUPERSECRETVALUE123")));
}

#[test]
fn changes_grouping_and_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let ws = make_ws(dir.path());
    let mut delta = ProviderDelta::default();
    for (p, _) in [("src/main.rs", ()), ("tests/a.rs", ()), ("Cargo.toml", ())] {
        delta.files.push(FileDelta {
            path: WorkspacePath::new(p),
            old_path: None,
            status: FileStatus::Modified,
            hunks: vec![],
            binary: false,
            summarized: false,
            additions: 1,
            deletions: 0,
        });
    }
    let groups = changes::group_delta(&ws.id, &delta);
    assert_eq!(groups.len(), 3); // Source, Tests, Dependencies

    // Change IDs must be STABLE across rescans so review decisions still match
    // at finalization time.
    let groups2 = changes::group_delta(&ws.id, &delta);
    let ids1: Vec<_> = groups.iter().map(|g| g.id.clone()).collect();
    let ids2: Vec<_> = groups2.iter().map(|g| g.id.clone()).collect();
    assert_eq!(ids1, ids2, "group ids must be deterministic across scans");
    for g in &groups {
        changes::save_change(&ws.layout(), g).unwrap();
    }
    let loaded = changes::load_changes(&ws.layout()).unwrap();
    assert_eq!(loaded.len(), 3);
}

#[test]
fn review_session_records_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let ws = make_ws(dir.path());
    let change = DraftChange::new(
        ws.id.clone(),
        Some("Source".into()),
        vec![FileChangeRef {
            path: WorkspacePath::new("src/main.rs"),
            old_path: None,
            status: FileStatus::Modified,
            additions: 1,
            deletions: 0,
            binary: false,
        }],
        GroupingSource::Automatic,
    );
    let mut session =
        ReviewSession::start(ws.id.clone(), vec![change.id.clone()], ActorRef::unknown());
    session.record(
        change.id.clone(),
        ReviewDecisionKind::Approved,
        Some("looks good".into()),
        ActorRef::unknown(),
    );
    session.save(&ws.layout()).unwrap();
    let latest = crate::review::latest(&ws.layout()).unwrap().unwrap();
    assert_eq!(latest.decisions.len(), 1);
    assert_eq!(
        ReviewState::from(latest.latest_for(&change.id).unwrap().kind),
        ReviewState::Approved
    );
}

#[test]
fn verification_runs_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let ws = make_ws(dir.path());
    let plan = VerificationPlan {
        id: crate::common::VerificationPlanId::generate(),
        commands: vec![VerificationCommand {
            name: "ok".into(),
            command: if cfg!(windows) {
                "cmd".into()
            } else {
                "true".into()
            },
            args: if cfg!(windows) {
                vec!["/C".into(), "exit 0".into()]
            } else {
                vec![]
            },
            timeout_ms: Some(5000),
        }],
    };
    let result = verification::run(&ws.layout(), &ws.root, &plan).unwrap();
    assert_eq!(result.status, verification::VerificationStatus::Passed);
    let latest = verification::latest(&ws.layout()).unwrap().unwrap();
    assert_eq!(latest.id, result.id);
}

#[test]
fn checkpoint_create_via_provider() {
    let dir = tempfile::tempdir().unwrap();
    let ws = make_ws(dir.path());
    let repo = FakeProvider.open(&ws).unwrap();
    let log = log_for(&ws);
    let cp = checkpoint::create(
        &ws,
        repo.as_ref(),
        &log,
        ActorRef::unknown(),
        Some("cp".into()),
    )
    .unwrap();
    assert!(cp.provider_checkpoint.is_some());
    assert!(log.contains_kind(OperationKind::CheckpointCreated).unwrap());
    let latest = checkpoint::latest(&ws.layout()).unwrap().unwrap();
    assert_eq!(latest.id, cp.id);
}

#[test]
fn finalization_full_flow_creates_receipt_and_op() {
    let dir = tempfile::tempdir().unwrap();
    let ws = make_ws(dir.path());
    let repo = FakeProvider.open(&ws).unwrap();
    let log = log_for(&ws);

    let mut change = DraftChange::new(
        ws.id.clone(),
        Some("Source".into()),
        vec![FileChangeRef {
            path: WorkspacePath::new("src/main.rs"),
            old_path: None,
            status: FileStatus::Modified,
            additions: 2,
            deletions: 0,
            binary: false,
        }],
        GroupingSource::Automatic,
    );
    change.review_state = ReviewState::Approved;

    let ctx = FinalizationContext {
        ws: &ws,
        repo: repo.as_ref(),
        log: &log,
        actor: ActorRef::unknown(),
        changes: vec![change],
        risk: risk::RiskSummary::low(),
        verification: None,
        conflicts: ConflictReport::default(),
        checkpoint: None,
    };
    let req = FinalizationRequest {
        message: "draft: finalize".into(),
        trailers: vec![],
        confirm_high_risk: false,
    };
    let (plan, result) = finalization::finalize(&ctx, &req).unwrap();
    assert!(plan.policy_result.allowed);
    assert_eq!(result.object.kind, "snapshot");

    // Receipt persisted and maps to provider object.
    let receipt = crate::receipts::load(&ws.layout(), &result.receipt_id).unwrap();
    assert_eq!(receipt.provider_objects.len(), 1);
    assert_eq!(receipt.change_ids.len(), 1);

    // Operation log records both planned + completed.
    assert!(log
        .contains_kind(OperationKind::FinalizationPlanned)
        .unwrap());
    assert!(log
        .contains_kind(OperationKind::FinalizationCompleted)
        .unwrap());
}

#[test]
fn finalization_blocks_on_high_risk_without_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let ws = make_ws(dir.path());
    let repo = FakeProvider.open(&ws).unwrap();
    let log = log_for(&ws);

    let change = DraftChange::new(
        ws.id.clone(),
        Some("Source".into()),
        vec![FileChangeRef {
            path: WorkspacePath::new("src/main.rs"),
            old_path: None,
            status: FileStatus::Modified,
            additions: 1,
            deletions: 0,
            binary: false,
        }],
        GroupingSource::Automatic,
    );
    let high_risk = risk::RiskSummary {
        level: risk::RiskLevel::Critical,
        score: 50,
        findings: vec![],
    };
    let ctx = FinalizationContext {
        ws: &ws,
        repo: repo.as_ref(),
        log: &log,
        actor: ActorRef::unknown(),
        changes: vec![change],
        risk: high_risk,
        verification: None,
        conflicts: ConflictReport::default(),
        checkpoint: None,
    };
    let req = FinalizationRequest {
        message: "risky".into(),
        trailers: vec![],
        confirm_high_risk: false,
    };
    let err = finalization::finalize(&ctx, &req).unwrap_err();
    assert_eq!(err.kind, crate::error::DraftErrorKind::RiskPolicyBlocked);
}
