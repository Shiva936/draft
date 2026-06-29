use std::fs;
use std::path::PathBuf;
use draft_core::models::*;
use draft_core::errors::DraftError;
use draft_core::storage::DraftStorage;
use draft_core::diff_analyzer::DiffAnalyzer;
use draft_core::risk_engine::RiskEngine;
use tempfile::TempDir;

// ─── Models serialization ────────────────────────────────────────────────────

#[test]
fn risk_assessment_roundtrip() {
    let ra = RiskAssessment {
        level: RiskLevel::High,
        reasons: vec![RiskReason {
            code: "SECURITY_CODE".to_string(),
            message: "Touched auth.rs".to_string(),
            path: Some(PathBuf::from("src/auth.rs")),
        }],
    };
    let json = serde_json::to_string(&ra).unwrap();
    let decoded: RiskAssessment = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.level, RiskLevel::High);
    assert_eq!(decoded.reasons[0].code, "SECURITY_CODE");
}

#[test]
fn checkpoint_serialization_roundtrip() {
    let cp = Checkpoint {
        checkpoint_id: "cp-001".to_string(),
        session_id: "sess-001".to_string(),
        repo_head: "abc1234".to_string(),
        message: "pre-commit".to_string(),
        created_at: chrono::Utc::now(),
        files: vec![CheckpointFile {
            path: PathBuf::from("src/main.rs"),
            content_hash: "deadbeef".to_string(),
            file_status: FileStatus::Modified,
        }],
    };
    let json = serde_json::to_string(&cp).unwrap();
    let decoded: Checkpoint = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.checkpoint_id, "cp-001");
    assert_eq!(decoded.files[0].content_hash, "deadbeef");
}

#[test]
fn verification_evidence_serialization() {
    let ev = VerificationEvidence {
        verification_id: "v-001".to_string(),
        command: "cargo test".to_string(),
        exit_code: Some(0),
        status: VerificationStatus::Passed,
        started_at: chrono::Utc::now(),
        finished_at: chrono::Utc::now(),
        duration_ms: 1234,
        stdout_summary: "test result: ok".to_string(),
        stderr_summary: String::new(),
    };
    let json = serde_json::to_string(&ev).unwrap();
    let decoded: VerificationEvidence = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.status, VerificationStatus::Passed);
    assert_eq!(decoded.duration_ms, 1234);
}

// ─── Storage roundtrip ───────────────────────────────────────────────────────

#[test]
fn storage_init_creates_directories() {
    let dir = TempDir::new().unwrap();
    let storage = DraftStorage::init(dir.path()).unwrap();
    assert!(storage.root.exists());
    assert!(storage.root.join("checkpoints").exists());
    assert!(storage.root.join("objects/blobs").exists());
    assert!(storage.root.join("verification").exists());
    assert!(storage.root.join("receipts").exists());
    assert!(storage.root.join("logs").exists());
}

#[test]
fn storage_open_fails_without_init() {
    let dir = TempDir::new().unwrap();
    let result = DraftStorage::open(dir.path());
    assert!(matches!(result, Err(DraftError::StorageError(_))));
}

#[test]
fn storage_json_roundtrip() {
    let dir = TempDir::new().unwrap();
    let storage = DraftStorage::init(dir.path()).unwrap();

    let val = serde_json::json!({"key": "value", "num": 42});
    storage.write_json(std::path::Path::new("test.json"), &val).unwrap();
    let read: serde_json::Value = storage.read_json(std::path::Path::new("test.json")).unwrap();
    assert_eq!(read["key"], "value");
    assert_eq!(read["num"], 42);
}

#[test]
fn storage_blob_roundtrip() {
    let dir = TempDir::new().unwrap();
    let storage = DraftStorage::init(dir.path()).unwrap();

    let data = b"hello world blob content";
    let hash = storage.write_blob(data).unwrap();
    assert!(!hash.is_empty());

    let read = storage.read_blob(&hash).unwrap();
    assert_eq!(read, data);
}

#[test]
fn storage_blob_deduplicates() {
    let dir = TempDir::new().unwrap();
    let storage = DraftStorage::init(dir.path()).unwrap();

    let data = b"same content";
    let hash1 = storage.write_blob(data).unwrap();
    let hash2 = storage.write_blob(data).unwrap();
    assert_eq!(hash1, hash2);
}

// ─── Diff parser ─────────────────────────────────────────────────────────────

#[test]
fn diff_parser_parses_modification() {
    let dir = TempDir::new().unwrap();
    let diff = r#"diff --git a/src/main.rs b/src/main.rs
index abc..def 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!("hello");
+    println!("hello world");
+    // added
 }
"#;
    let status = "M  src/main.rs\n";
    let changes = DiffAnalyzer::analyze(dir.path(), diff, status).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, PathBuf::from("src/main.rs"));
    assert_eq!(changes[0].additions, 2);
    assert_eq!(changes[0].deletions, 1);
}

#[test]
fn diff_parser_handles_added_file() {
    let dir = TempDir::new().unwrap();
    let diff = r#"diff --git a/new_file.rs b/new_file.rs
new file mode 100644
--- /dev/null
+++ b/new_file.rs
@@ -0,0 +1,2 @@
+fn new_fn() {}
+// new
"#;
    let status = "A  new_file.rs\n";
    let changes = DiffAnalyzer::analyze(dir.path(), diff, status).unwrap();
    let change = changes.iter().find(|c| c.path == PathBuf::from("new_file.rs")).unwrap();
    assert_eq!(change.status, FileStatus::Added);
    assert_eq!(change.additions, 2);
}

#[test]
fn diff_parser_handles_binary() {
    let dir = TempDir::new().unwrap();
    let diff = "diff --git a/image.png b/image.png\nBinary files a/image.png and b/image.png differ\n";
    let status = "M  image.png\n";
    let changes = DiffAnalyzer::analyze(dir.path(), diff, status).unwrap();
    let change = changes.iter().find(|c| c.path == PathBuf::from("image.png")).unwrap();
    assert!(change.is_binary);
}

// ─── Verification command inference ─────────────────────────────────────────

#[test]
fn verify_infer_cargo_test() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
    let cmd = draft_core::verification_engine::VerificationEngine::infer_command(dir.path());
    assert_eq!(cmd, Some("cargo test".to_string()));
}

#[test]
fn verify_infer_go_test() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("go.mod"), "module x").unwrap();
    let cmd = draft_core::verification_engine::VerificationEngine::infer_command(dir.path());
    assert_eq!(cmd, Some("go test ./...".to_string()));
}

#[test]
fn verify_infer_none_for_empty_dir() {
    let dir = TempDir::new().unwrap();
    let cmd = draft_core::verification_engine::VerificationEngine::infer_command(dir.path());
    assert_eq!(cmd, None);
}

// ─── Receipt serialization ───────────────────────────────────────────────────

#[test]
fn receipt_serialization_roundtrip() {
    let receipt = CommitReceipt {
        receipt_id: "r-001".to_string(),
        draft_version: "0.1.0".to_string(),
        repo_id: "repo-001".to_string(),
        session_id: "sess-001".to_string(),
        commit_hash: "abc1234".to_string(),
        commit_message: "Fix auth".to_string(),
        branch: Some("main".to_string()),
        head_before: "aaa".to_string(),
        head_after: "bbb".to_string(),
        included_files: vec![PathBuf::from("src/auth.rs")],
        excluded_files: vec![],
        risk_summary: RiskAssessment { level: RiskLevel::High, reasons: vec![] },
        verification: None,
        checkpoint_id: "cp-001".to_string(),
        identity: None,
        coauthors: vec![],
        created_at: chrono::Utc::now(),
    };
    let json = serde_json::to_string(&receipt).unwrap();
    let decoded: CommitReceipt = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.commit_hash, "abc1234");
    assert_eq!(decoded.draft_version, "0.1.0");
    assert_eq!(decoded.included_files, vec![PathBuf::from("src/auth.rs")]);
}

// ─── Conflict marker detection ───────────────────────────────────────────────

#[test]
fn conflict_engine_detects_markers() {
    use draft_core::conflict_engine::ConflictEngine;
    use draft_core::models::{FileChange, FileStatus, RepoContext};

    let dir = TempDir::new().unwrap();
    let conflicted = dir.path().join("file.rs");
    fs::write(&conflicted, "<<<<<<< HEAD\nfn a() {}\n=======\nfn b() {}\n>>>>>>> branch\n").unwrap();

    let ctx = RepoContext {
        repo_root: dir.path().to_path_buf(),
        git_dir: dir.path().join(".git"),
        branch: Some("main".to_string()),
        head: "abc".to_string(),
        is_dirty: true,
        is_detached_head: false,
        has_unmerged_conflicts: false,
        identity: None,
    };

    let changes = vec![FileChange {
        path: PathBuf::from("file.rs"),
        status: FileStatus::Modified,
        additions: 1,
        deletions: 1,
        is_binary: false,
        hunks: vec![],
    }];

    let report = ConflictEngine::detect(&ctx, &changes).unwrap();
    assert!(report.has_conflicts);
    assert!(!report.files.is_empty());
}

#[test]
fn conflict_engine_clean_file_no_conflict() {
    use draft_core::conflict_engine::ConflictEngine;
    use draft_core::models::{FileChange, FileStatus, RepoContext};

    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("clean.rs"), "fn main() {}\n").unwrap();

    let ctx = RepoContext {
        repo_root: dir.path().to_path_buf(),
        git_dir: dir.path().join(".git"),
        branch: Some("main".to_string()),
        head: "abc".to_string(),
        is_dirty: true,
        is_detached_head: false,
        has_unmerged_conflicts: false,
        identity: None,
    };

    let changes = vec![FileChange {
        path: PathBuf::from("clean.rs"),
        status: FileStatus::Modified,
        additions: 1,
        deletions: 0,
        is_binary: false,
        hunks: vec![],
    }];

    let report = ConflictEngine::detect(&ctx, &changes).unwrap();
    assert!(!report.has_conflicts);
}

fn make_group(title: &str, files: Vec<&str>, kind: ChangeGroupKind) -> ChangeGroup {
    ChangeGroup {
        group_id: "test".to_string(),
        title: title.to_string(),
        description: None,
        files: files.iter().map(|f| PathBuf::from(f)).collect(),
        hunks: vec![],
        risk: RiskAssessment { level: RiskLevel::Low, reasons: vec![] },
        group_kind: kind,
        included: true,
    }
}

fn clean_ctx() -> draft_core::models::RepoContext {
    draft_core::models::RepoContext {
        repo_root: PathBuf::from("/tmp/test"),
        git_dir: PathBuf::from("/tmp/test/.git"),
        branch: Some("main".to_string()),
        head: "abc1234".to_string(),
        is_dirty: true,
        is_detached_head: false,
        has_unmerged_conflicts: false,
        identity: None,
    }
}

#[test]
fn risk_engine_auth_file_is_high() {
    let group = make_group("Auth", vec!["src/auth.rs"], ChangeGroupKind::SourceChange);
    let ctx = clean_ctx();
    let assessed = RiskEngine::assess(&[group], &ctx).unwrap();
    assert_eq!(assessed[0].risk.level, RiskLevel::High);
}

#[test]
fn risk_engine_lockfile_is_high() {
    let group = make_group("Deps", vec!["Cargo.lock"], ChangeGroupKind::DependencyChange);
    let ctx = clean_ctx();
    let assessed = RiskEngine::assess(&[group], &ctx).unwrap();
    assert_eq!(assessed[0].risk.level, RiskLevel::High);
}

#[test]
fn risk_engine_test_only_is_low() {
    let group = make_group("Tests", vec!["tests/integration.rs"], ChangeGroupKind::TestChange);
    let ctx = clean_ctx();
    let assessed = RiskEngine::assess(&[group], &ctx).unwrap();
    assert_eq!(assessed[0].risk.level, RiskLevel::Low);
}

#[test]
fn risk_engine_conflicts_is_blocked() {
    let group = make_group("Source", vec!["src/lib.rs"], ChangeGroupKind::SourceChange);
    let mut ctx = clean_ctx();
    ctx.has_unmerged_conflicts = true;
    let assessed = RiskEngine::assess(&[group], &ctx).unwrap();
    assert_eq!(assessed[0].risk.level, RiskLevel::Blocked);
}

#[test]
fn risk_engine_summarize_picks_highest() {
    let low_group = make_group("Tests", vec!["tests/a.rs"], ChangeGroupKind::TestChange);
    let high_group = make_group("Auth", vec!["src/auth.rs"], ChangeGroupKind::SourceChange);
    let ctx = clean_ctx();
    let assessed = RiskEngine::assess(&[low_group, high_group], &ctx).unwrap();
    let summary = RiskEngine::summarize(&assessed);
    assert_eq!(summary.level, RiskLevel::High);
}
