use draft_core::{App, ChangepackStatus};
use std::path::Path;

fn setup() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var(
        "DRAFT_GLOBAL_HOME",
        std::env::temp_dir().join(format!("draft-core-test-global-{}", std::process::id())),
    );
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

#[test]
fn proto_contract_files_are_present_and_parseable() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let proto = repo.join("proto");
    for rel in [
        "specs/changepack.md",
        "specs/receipt.md",
        "specs/event-ledger.md",
        "specs/signing.md",
        "specs/canonicalization.md",
        "specs/composition.md",
        "specs/project-state.md",
        "specs/stability.md",
        "specs/save-finalization.md",
        "specs/rollback.md",
        "specs/close.md",
        "specs/gc.md",
        "specs/import-export.md",
        "specs/path-safety.md",
        "specs/compatibility.md",
        "specs/future-drafthub-readiness.md",
    ] {
        let path = proto.join(rel);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("missing/readable proto spec {}: {e}", path.display()));
        assert!(
            text.starts_with("# "),
            "{} must have a title",
            path.display()
        );
        assert!(
            text.trim().len() > 80,
            "{} should contain a real protocol contract, not an empty placeholder",
            path.display()
        );
    }

    for rel in [
        "schemas/changepack.schema.json",
        "schemas/receipt.schema.json",
        "schemas/event.schema.json",
        "schemas/project-state.schema.json",
        "schemas/stable-head.schema.json",
        "schemas/composition.schema.json",
        "schemas/verification.schema.json",
        "schemas/config.schema.json",
    ] {
        let path = proto.join(rel);
        let value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path)
                .unwrap_or_else(|e| panic!("missing/readable schema {}: {e}", path.display())),
        )
        .unwrap_or_else(|e| panic!("schema {} must be valid JSON: {e}", path.display()));
        assert_eq!(
            value["type"],
            "object",
            "{} must define an object",
            path.display()
        );
        assert!(
            value.get("properties").is_some(),
            "{} must declare schema properties",
            path.display()
        );
    }

    for name in [
        "valid-pack",
        "invalid-signature",
        "tampered-receipt",
        "conflicting-packs",
        "independent-packs",
        "dependent-packs",
        "stable-composition",
        "unstable-composition",
        "save-merge-and-dispose",
        "save-dispose-only",
        "close-clean-repo",
        "close-with-pending-pack",
        "gc-disposed-pack-cleanup",
    ] {
        let path = proto.join("test-vectors").join(name).join("vector.json");
        let value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path)
                .unwrap_or_else(|e| panic!("missing/readable vector {}: {e}", path.display())),
        )
        .unwrap_or_else(|e| panic!("vector {} must be valid JSON: {e}", path.display()));
        assert_eq!(
            value["name"],
            name,
            "{} has wrong vector name",
            path.display()
        );
        assert!(
            value.get("expect").is_some(),
            "{} must declare expected outcome",
            path.display()
        );
        // Schema-driven conformance (NFR-MT-005, NFR-TQ-001): every vector
        // carries a payload that must validate against its declared schema,
        // and negative fixtures must fail validation.
        let schema_name = value["payload_schema"]
            .as_str()
            .unwrap_or_else(|| panic!("{} must declare payload_schema", path.display()));
        let schema: serde_json::Value = serde_json::from_slice(
            &std::fs::read(proto.join("schemas").join(schema_name)).unwrap(),
        )
        .unwrap();
        let payload = value
            .get("payload")
            .unwrap_or_else(|| panic!("{} must carry a payload fixture", path.display()));
        let errors = validate_against_schema(&schema, payload);
        assert!(
            errors.is_empty(),
            "{} payload must validate against {schema_name}: {errors:?}",
            path.display()
        );
        if let Some(invalid) = value.get("invalid_payload") {
            let errors = validate_against_schema(&schema, invalid);
            assert!(
                !errors.is_empty(),
                "{} invalid_payload must fail validation against {schema_name}",
                path.display()
            );
        }
    }
}

/// Minimal JSON-Schema validator covering the subset used by proto/schemas:
/// `type` (incl. union with null), `required`, `const`, `enum`, `pattern`,
/// and `additionalProperties: false`. Enough to keep vectors honest without
/// pulling in a schema engine.
fn validate_against_schema(schema: &serde_json::Value, value: &serde_json::Value) -> Vec<String> {
    use serde_json::Value;
    let mut errors = Vec::new();
    if schema["type"] == "object" && !value.is_object() {
        return vec!["expected an object".to_string()];
    }
    let obj = match value.as_object() {
        Some(map) => map,
        None => return errors,
    };
    if let Some(required) = schema["required"].as_array() {
        for key in required.iter().filter_map(Value::as_str) {
            if !obj.contains_key(key) {
                errors.push(format!("missing required field '{key}'"));
            }
        }
    }
    let props = schema["properties"].as_object();
    if schema["additionalProperties"] == false {
        if let Some(props) = props {
            for key in obj.keys() {
                if !props.contains_key(key) {
                    errors.push(format!("unexpected field '{key}'"));
                }
            }
        }
    }
    let Some(props) = props else {
        return errors;
    };
    for (key, rule) in props {
        let Some(actual) = obj.get(key) else {
            continue;
        };
        if let Some(expected) = rule.get("const") {
            if actual != expected {
                errors.push(format!("field '{key}' must equal {expected}"));
            }
        }
        if let Some(allowed) = rule.get("enum").and_then(Value::as_array) {
            if !allowed.contains(actual) {
                errors.push(format!("field '{key}' value {actual} not in enum"));
            }
        }
        if let Some(types) = rule.get("type") {
            let names: Vec<&str> = match types {
                Value::String(s) => vec![s.as_str()],
                Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            if !names.is_empty() {
                let matches = names.iter().any(|t| match *t {
                    "string" => actual.is_string(),
                    "object" => actual.is_object(),
                    "array" => actual.is_array(),
                    "number" => actual.is_number(),
                    "integer" => actual.is_i64() || actual.is_u64(),
                    "boolean" => actual.is_boolean(),
                    "null" => actual.is_null(),
                    _ => true,
                });
                if !matches {
                    errors.push(format!("field '{key}' has wrong type"));
                }
            }
        }
        if let (Some(pattern), Some(text)) = (rule["pattern"].as_str(), actual.as_str()) {
            if !simple_pattern_matches(pattern, text) {
                errors.push(format!("field '{key}' does not match pattern {pattern}"));
            }
        }
        // Recurse into nested object rules (e.g. config.schema.json's [save]).
        if rule.get("properties").is_some() && actual.is_object() {
            for nested in validate_against_schema(rule, actual) {
                errors.push(format!("{key}.{nested}"));
            }
        }
    }
    errors
}

/// Match the two anchored pattern shapes proto/schemas use
/// (`^prefix_[A-Za-z0-9_-]+$` and `^sha256:[0-9a-f]{64}$`) without a regex
/// dependency.
fn simple_pattern_matches(pattern: &str, text: &str) -> bool {
    match pattern {
        "^sha256:[0-9a-f]{64}$" => text.strip_prefix("sha256:").is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        }),
        _ => {
            // ^<prefix>[A-Za-z0-9_-]+$ shapes (pck_/rcp_/cmp_/evt_ ids).
            let Some(body) = pattern
                .strip_prefix('^')
                .and_then(|p| p.strip_suffix("[A-Za-z0-9_-]+$"))
            else {
                return true; // unknown pattern shapes are not enforced here
            };
            text.strip_prefix(body).is_some_and(|rest| {
                !rest.is_empty()
                    && rest
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            })
        }
    }
}
