use draft_core::{App, ChangepackStatus};

fn setup() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::new();
    app.init(dir.path()).unwrap();
    (dir, app)
}

#[test]
fn pack_create_uses_previous_snapshot_and_generates_text_hunks() {
    let (dir, app) = setup();
    let file = dir.path().join("app.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    app.checkpoint(dir.path(), "base").unwrap();
    std::fs::write(&file, "one\nTWO\nthree\n").unwrap();

    let pack = app
        .pack_create(dir.path(), Some("edit".to_string()), None, true)
        .unwrap();
    let report = app.pack_show(dir.path(), pack.id.as_str()).unwrap();

    assert_eq!(report.patch.files.len(), 1);
    assert_eq!(report.patch.files[0].path.as_str(), "app.txt");
    assert_eq!(report.patch.files[0].hunks.len(), 1);
    assert!(report.patch.files[0].hunks[0].id.starts_with("hunk_"));
    assert_eq!(report.patch.files[0].hunks[0].old_start, 2);
    assert_eq!(report.patch.files[0].hunks[0].new_start, 2);
}

#[test]
fn compare_and_compose_allow_same_file_non_overlapping_hunks() {
    let (dir, app) = setup();
    let file = dir.path().join("app.txt");
    std::fs::write(&file, "one\ntwo\nthree\nfour\n").unwrap();
    app.checkpoint(dir.path(), "base").unwrap();

    std::fs::write(&file, "ONE\ntwo\nthree\nfour\n").unwrap();
    let left = app
        .pack_create(dir.path(), Some("left".to_string()), None, true)
        .unwrap();

    std::fs::write(&file, "one\ntwo\nthree\nfour\n").unwrap();
    app.checkpoint(dir.path(), "base again").unwrap();
    std::fs::write(&file, "one\ntwo\nTHREE\nfour\n").unwrap();
    let right = app
        .pack_create(dir.path(), Some("right".to_string()), None, true)
        .unwrap();

    let cmp = app
        .compare(dir.path(), left.id.as_str(), right.id.as_str())
        .unwrap();
    assert_eq!(cmp.overlapping_files.len(), 1);
    assert!(cmp.overlapping_hunks.is_empty());
    assert!(cmp.compatible);

    let composed = app
        .compose(dir.path(), left.id.as_str(), right.id.as_str(), "combined")
        .unwrap();
    assert!(composed.compatible);
    assert_eq!(composed.files, 2);
    let pack = app
        .pack_show(dir.path(), &composed.output_pack_id)
        .unwrap()
        .pack;
    assert_eq!(pack.status, ChangepackStatus::Draft);
    assert_eq!(pack.source_pack_ids.len(), 2);
}

#[test]
fn compare_blocks_overlapping_hunks() {
    let (dir, app) = setup();
    let file = dir.path().join("app.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    app.checkpoint(dir.path(), "base").unwrap();

    std::fs::write(&file, "one\nTWO\nthree\n").unwrap();
    let left = app
        .pack_create(dir.path(), Some("left".to_string()), None, true)
        .unwrap();

    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    app.checkpoint(dir.path(), "base again").unwrap();
    std::fs::write(&file, "one\nsecond\nthree\n").unwrap();
    let right = app
        .pack_create(dir.path(), Some("right".to_string()), None, true)
        .unwrap();

    let cmp = app
        .compare(dir.path(), left.id.as_str(), right.id.as_str())
        .unwrap();
    assert!(!cmp.compatible);
    assert_eq!(cmp.overlapping_hunks.len(), 1);
    assert!(app
        .compose(dir.path(), left.id.as_str(), right.id.as_str(), "bad")
        .is_err());
}

#[test]
fn event_replay_summarizes_and_verifies_chain() {
    let (dir, app) = setup();
    let report = app.replay_events(dir.path()).unwrap();
    assert!(report.chain_ok);
    assert!(report.events >= 1);
    assert_eq!(report.by_type["repo.initialized"], 1);
}

#[test]
fn durable_events_redact_common_secret_shapes() {
    let (dir, app) = setup();
    app.task_spawn(
        dir.path(),
        "secret-task",
        None,
        vec![],
        None,
        vec![
            "token=abc123".to_string(),
            "Authorization: Bearer eyJhbGciOi.fake.sig".to_string(),
            "postgres://user:pass@example.com/db".to_string(),
            "-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----".to_string(),
        ],
    )
    .unwrap();

    let events = app.events(dir.path()).unwrap();
    let payload = events
        .iter()
        .find(|event| event.event_type == "task.spawned")
        .unwrap()
        .payload
        .to_string();
    assert!(!payload.contains("abc123"));
    assert!(!payload.contains("eyJhbGciOi.fake.sig"));
    assert!(!payload.contains("user:pass"));
    assert!(!payload.contains("PRIVATE KEY-----"));
}

#[cfg(unix)]
#[test]
fn rollback_rejects_symlink_parent_escape() {
    use std::os::unix::fs::symlink;

    let (dir, app) = setup();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("safe")).unwrap();
    symlink(outside.path(), dir.path().join("safe/link")).unwrap();
    std::fs::write(dir.path().join("safe/link/file.txt"), "outside\n").unwrap();

    let mut snapshot = app.checkpoint(dir.path(), "base").unwrap();
    let mut snap: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            dir.path()
                .join(".draft/snapshots")
                .join(format!("{}.json", snapshot.snapshot_id)),
        )
        .unwrap(),
    )
    .unwrap();
    snap["files"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": "safe/link/escape.txt",
            "file_kind": "text",
            "content_hash": null,
            "size_bytes": 0,
            "modified_time": null,
            "executable": null
        }));
    snapshot.snapshot_id = "chk_escape".to_string();
    snap["id"] = serde_json::json!(snapshot.snapshot_id);
    std::fs::write(
        dir.path()
            .join(".draft/snapshots")
            .join(format!("{}.json", snapshot.snapshot_id)),
        serde_json::to_string_pretty(&snap).unwrap(),
    )
    .unwrap();

    let err = app
        .rollback(dir.path(), &snapshot.snapshot_id, true)
        .unwrap_err()
        .to_string();
    assert!(err.contains("escapes workspace") || err.contains("unsafe workspace path"));
}
