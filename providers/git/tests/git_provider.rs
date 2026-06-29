//! Git provider contract + Git-specific tests (TDD §14.2).

use std::path::Path;
use std::process::Command;

use draft_core::common::WorkspacePath;
use draft_core::vcs::detection::DetectionConfidence;
use draft_core::vcs::traits::VcsProvider;
use draft_core::vcs::types::*;
use draft_core::workspace;
use draft_provider_git::{provider_id, GitProvider};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "initial"]);
}

fn open_repo(root: &Path) -> Box<dyn draft_core::vcs::traits::VcsRepository> {
    let ws = workspace::initialize(root, root, provider_id(), false).unwrap();
    let repo = GitProvider::new().open(&ws).unwrap();
    // Mirror the real init flow: exclude .draft/ from provider history so it
    // does not show up in status/diff.
    repo.ignore_rules().unwrap();
    repo
}

#[test]
fn detect_reports_exact_in_repo() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let det = GitProvider::new().detect(dir.path()).unwrap();
    assert_eq!(det.provider_id, provider_id());
    assert_eq!(det.confidence, DetectionConfidence::Exact);
}

#[test]
fn detect_reports_none_outside_repo() {
    let dir = tempfile::tempdir().unwrap();
    let det = GitProvider::new().detect(dir.path()).unwrap();
    assert_eq!(det.confidence, DetectionConfidence::None);
}

#[test]
fn capabilities_declare_git_shape() {
    let caps = GitProvider::new().capabilities();
    assert!(caps.has_staging_area);
    assert!(caps.supports_finalization);
    assert!(!caps.has_change_ids);
}

#[test]
fn status_and_diff_reflect_changes() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "hello\nworld\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "new file\n").unwrap();
    let repo = open_repo(dir.path());

    let status = repo.status().unwrap();
    let modified = status
        .entries
        .iter()
        .any(|e| e.path.as_str() == "a.txt" && e.status == FileStatus::Modified);
    let untracked = status
        .entries
        .iter()
        .any(|e| e.path.as_str() == "b.txt" && e.status == FileStatus::Untracked);
    assert!(modified, "a.txt should be modified: {:?}", status.entries);
    assert!(untracked, "b.txt should be untracked: {:?}", status.entries);

    let delta = repo.diff(DiffInput::WorkingTree).unwrap();
    assert!(delta.base.is_some());
    assert_eq!(delta.files.len(), 2);
    assert!(delta.stats.additions >= 2);
}

#[test]
fn conflicts_empty_on_clean_tree() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let repo = open_repo(dir.path());
    assert!(repo.conflicts().unwrap().is_empty());
}

#[test]
fn checkpoint_snapshot_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "hello\nchange\n").unwrap();
    let repo = open_repo(dir.path());
    let cp = repo
        .create_checkpoint(CheckpointInput {
            description: Some("test".into()),
        })
        .unwrap();
    assert_eq!(cp.kind, ProviderCheckpointKind::WorkingSnapshot);
    assert!(cp.restore_token.is_some());
}

#[test]
fn finalization_creates_commit_and_undo_is_safe() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let before = git(dir.path(), &["rev-parse", "HEAD"]);
    std::fs::write(dir.path().join("a.txt"), "hello\nfinal\n").unwrap();
    let repo = open_repo(dir.path());

    let plan = repo
        .prepare_finalization(ProviderFinalizationInput {
            include_paths: vec![WorkspacePath::new("a.txt")],
            message: "draft: finalize a.txt".into(),
            trailers: vec!["Co-authored-by: Agent <agent@example.com>".into()],
        })
        .unwrap();
    let result = repo.finalize(plan).unwrap();
    assert_eq!(result.object.kind, "commit");

    // A real commit exists and is HEAD.
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    assert_eq!(head, result.object.object_id.as_str());
    assert_ne!(head, before);
    let log = git(dir.path(), &["log", "-1", "--pretty=%B"]);
    assert!(log.contains("finalize a.txt"));
    assert!(log.contains("Co-authored-by"));

    // Undo: soft reset, content preserved, history back to `before`.
    let undo = repo
        .undo_provider_action(ProviderUndoInput {
            object: Some(result.object.clone()),
            checkpoint: None,
        })
        .unwrap();
    assert!(undo.undone);
    assert!(undo.provider_history_changed);
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), before);
    assert!(std::fs::read_to_string(dir.path().join("a.txt"))
        .unwrap()
        .contains("final"));
}

#[test]
fn unborn_repo_has_zero_base() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "t@e.com"]);
    git(dir.path(), &["config", "user.name", "T"]);
    std::fs::write(dir.path().join("x.txt"), "x\n").unwrap();
    let repo = open_repo(dir.path());
    let view = repo.current_view().unwrap();
    assert!(view.revision.is_none());
    assert!(view.is_dirty);
}
