use draft_core::app::App;
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
fn event_replay_summarizes_and_verifies_chain() {
    let (dir, app) = setup();
    let report = app.replay_events(dir.path()).unwrap();
    assert!(report.chain_ok);
    assert!(report.events >= 1);
    assert_eq!(report.by_kind["ProjectCreated"], 1);
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
        .find(|event| event.kind == "TaskUpdated")
        .unwrap()
        .metadata
        .to_string();
    assert!(!payload.contains("abc123"));
    assert!(!payload.contains("eyJhbGciOi.fake.sig"));
    assert!(!payload.contains("user:pass"));
    assert!(!payload.contains("PRIVATE KEY-----"));
}

#[cfg(unix)]
#[test]
fn canonical_snapshot_rejects_symlink_parent_escape() {
    use std::os::unix::fs::symlink;

    let (dir, app) = setup();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("safe")).unwrap();
    symlink(outside.path(), dir.path().join("safe/link")).unwrap();
    std::fs::write(dir.path().join("safe/link/file.txt"), "outside\n").unwrap();

    let err = app.checkpoint(dir.path(), "base").unwrap_err().to_string();
    assert!(err.contains("symlink target") || err.contains("absolute paths are not allowed"));
}

#[test]
fn proto_contract_files_are_present_and_parseable() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let proto = repo.join("proto");
    for rel in [
        "specs/change-pack.md",
        "specs/receipt.md",
        "specs/event-ledger.md",
        "specs/signing.md",
        "specs/canonicalization.md",
        "specs/compatibility.md",
        "specs/composition.md",
        "specs/publication.md",
        "specs/coverage.md",
        "specs/stability.md",
        "specs/recovery.md",
        "specs/close.md",
        "specs/gc.md",
        "specs/path-safety.md",
        "specs/future-readiness.md",
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

    let mut schema_paths = std::fs::read_dir(proto.join("schemas"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".schema.json"))
        })
        .collect::<Vec<_>>();
    schema_paths.sort();
    assert!(
        !schema_paths.is_empty(),
        "at least one schema must be committed"
    );
    // Schemas describing the portable canonical DCG types, rather than a
    // Draft-owned persisted contract.
    //
    // They are exempt from the `ContractId` registry and from the
    // `schema_version` rule for one reason: the types they describe carry no
    // per-object version. A portable canonical object is versioned by
    // `DCG_FORMAT_REVISION` and identified by its digest, and adding a
    // `schema_version` field to make this check pass would change the
    // canonical bytes of every Publication ever written.
    //
    // The exemption is a named list, not a pattern, so a new schema cannot
    // slip past the registry by choosing a filename — and each one is required
    // below to be exercised by a committed vector, which is the coverage the
    // registry would otherwise have provided.
    const PORTABLE_SCHEMAS: &[&str] = &[
        "coverage-evidence.schema.json",
        "project-state-root.schema.json",
        "publication-attempt.schema.json",
        "publication-control.schema.json",
        "publication-outcome.schema.json",
        "publication-resolution.schema.json",
        "publication-retry-authorization.schema.json",
        "publication.schema.json",
        "state-evidence-entry.schema.json",
    ];

    let registered_schemas = draft_core::contracts::ContractId::ALL
        .iter()
        .filter_map(|contract| contract.metadata().schema)
        .collect::<std::collections::BTreeSet<_>>();
    let committed_schemas = schema_paths
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect::<std::collections::BTreeSet<_>>();
    for schema in &committed_schemas {
        if PORTABLE_SCHEMAS.contains(&schema.as_str()) {
            continue;
        }
        assert!(
            registered_schemas.contains(schema.as_str()),
            "schema {schema} has no closed ContractId registry entry"
        );
    }
    for schema in registered_schemas {
        assert!(
            committed_schemas.contains(schema),
            "registered schema {schema} is not committed"
        );
    }

    // A portable schema nothing exercises is a schema nobody would notice
    // drifting from the type it claims to describe.
    let vector_schemas = std::fs::read_dir(proto.join("test-vectors"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("vector.json"))
        .filter(|path| path.is_file())
        .filter_map(|path| std::fs::read(path).ok())
        .filter_map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter_map(|value| value["payload_schema"].as_str().map(ToString::to_string))
        .collect::<std::collections::BTreeSet<_>>();
    for schema in PORTABLE_SCHEMAS {
        assert!(
            committed_schemas.contains(*schema),
            "portable schema {schema} is named but not committed"
        );
        assert!(
            vector_schemas.contains(*schema),
            "portable schema {schema} is exempt from the contract registry, so a committed              vector must exercise it"
        );
    }

    for path in schema_paths {
        let filename = path.file_name().unwrap().to_string_lossy().into_owned();
        if PORTABLE_SCHEMAS.contains(&filename.as_str()) {
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(
                value["$id"],
                format!("https://draft.dev/schemas/{filename}"),
                "{} must have a stable unversioned id",
                path.display()
            );
            assert!(value["title"].as_str().is_some_and(|t| !t.is_empty()));
            assert_local_refs_resolve(&value, &value, &path);
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path)
                .unwrap_or_else(|e| panic!("missing/readable schema {}: {e}", path.display())),
        )
        .unwrap_or_else(|e| panic!("schema {} must be valid JSON: {e}", path.display()));
        let filename = path.file_name().unwrap().to_string_lossy();
        assert_eq!(
            value["$id"],
            format!("https://draft.dev/schemas/{filename}"),
            "{} must have a stable unversioned id",
            path.display()
        );
        assert!(value["title"]
            .as_str()
            .is_some_and(|title| !title.is_empty()));
        assert_local_refs_resolve(&value, &value, &path);
        assert!(
            schema_has_literal_version(&value),
            "{} must declare literal numeric schema_version 1",
            path.display()
        );
    }

    let mut vector_paths = std::fs::read_dir(proto.join("test-vectors"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("vector.json"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    vector_paths.sort();
    for path in vector_paths {
        let name = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy();
        let value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path)
                .unwrap_or_else(|e| panic!("missing/readable vector {}: {e}", path.display())),
        )
        .unwrap_or_else(|e| panic!("vector {} must be valid JSON: {e}", path.display()));
        assert_eq!(
            value["name"],
            name.as_ref(),
            "{} has wrong vector name",
            path.display()
        );
        assert!(
            value.get("expect").is_some(),
            "{} must declare expected outcome",
            path.display()
        );
        // Schema-driven conformance: every vector
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

fn assert_local_refs_resolve(root: &serde_json::Value, node: &serde_json::Value, path: &Path) {
    match node {
        serde_json::Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(serde_json::Value::as_str) {
                let pointer = reference.strip_prefix('#').unwrap_or_else(|| {
                    panic!(
                        "{} contains non-local reference {reference}",
                        path.display()
                    )
                });
                assert!(
                    root.pointer(pointer).is_some(),
                    "{} contains unresolved reference {reference}",
                    path.display()
                );
            }
            for value in object.values() {
                assert_local_refs_resolve(root, value, path);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                assert_local_refs_resolve(root, value, path);
            }
        }
        _ => {}
    }
}

/// Validate the recursive schema subset used by Draft's committed vectors,
/// including local references, objects, arrays, and tagged alternatives.
fn validate_against_schema(schema: &serde_json::Value, value: &serde_json::Value) -> Vec<String> {
    validate_schema_node(schema, schema, value)
}

fn validate_schema_node(
    root: &serde_json::Value,
    schema: &serde_json::Value,
    value: &serde_json::Value,
) -> Vec<String> {
    use serde_json::Value;

    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let Some(pointer) = reference.strip_prefix('#') else {
            return vec![format!("external reference is not supported: {reference}")];
        };
        let Some(resolved) = root.pointer(pointer) else {
            return vec![format!("unresolved local reference: {reference}")];
        };
        return validate_schema_node(root, resolved, value);
    }

    if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
        let successes = branches
            .iter()
            .filter(|branch| validate_schema_node(root, branch, value).is_empty())
            .count();
        return if successes == 1 {
            Vec::new()
        } else {
            vec![format!(
                "expected exactly one matching schema branch, got {successes}"
            )]
        };
    }

    let mut errors = Vec::new();
    if let Some(expected) = schema.get("const") {
        if value != expected {
            errors.push(format!("must equal {expected}"));
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            errors.push(format!("value {value} is not in enum"));
        }
    }
    if let Some(types) = schema.get("type") {
        let names: Vec<&str> = match types {
            Value::String(name) => vec![name],
            Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let matches = names.iter().any(|name| match *name {
            "string" => value.is_string(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            "number" => value.is_number(),
            "integer" => value.is_i64() || value.is_u64(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => false,
        });
        if !names.is_empty() && !matches {
            return vec![format!("wrong type; expected {}", names.join(" or "))];
        }
    }
    if let (Some(pattern), Some(text)) = (
        schema.get("pattern").and_then(Value::as_str),
        value.as_str(),
    ) {
        if !simple_pattern_matches(pattern, text) {
            errors.push(format!("does not match pattern {pattern}"));
        }
    }

    if let Some(items) = value.as_array() {
        if let Some(item_schema) = schema.get("items") {
            for (index, item) in items.iter().enumerate() {
                for error in validate_schema_node(root, item_schema, item) {
                    errors.push(format!("[{index}].{error}"));
                }
            }
        }
    }

    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(key) {
                    errors.push(format!("missing required field '{key}'"));
                }
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for (key, actual) in object {
            if let Some(rule) = properties.and_then(|rules| rules.get(key)) {
                for error in validate_schema_node(root, rule, actual) {
                    errors.push(format!("{key}.{error}"));
                }
            } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                errors.push(format!("unexpected field '{key}'"));
            } else if let Some(rule) = schema
                .get("additionalProperties")
                .filter(|rule| rule.is_object())
            {
                for error in validate_schema_node(root, rule, actual) {
                    errors.push(format!("{key}.{error}"));
                }
            }
        }
    }
    errors
}

/// Every schema must pin its own version somewhere a reader can find it.
///
/// Two shapes are legitimate, and only two. A Draft-owned persisted or wire
/// artifact carries its own `schema_version`. A portable canonical value from
/// `draft-dcg-contract` does not — its version is the crate's
/// `DCG_FORMAT_REVISION`, and adding a per-artifact field would mean the same
/// value serialized two different ways depending on which side wrote it. Those
/// schemas say so explicitly with `x-draft-format-revision`, so "pins its
/// version at the format level" stays distinguishable from "forgot to".
fn schema_has_literal_version(value: &serde_json::Value) -> bool {
    if value.get("x-draft-format-revision") == Some(&serde_json::json!(1)) {
        return true;
    }
    match value {
        serde_json::Value::Object(object) => {
            object
                .get("schema_version")
                .is_some_and(|schema| schema.get("const") == Some(&serde_json::json!(1)))
                || object.values().any(schema_has_literal_version)
        }
        serde_json::Value::Array(items) => items.iter().any(schema_has_literal_version),
        _ => false,
    }
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
            // ^<prefix>[A-Za-z0-9_-]+$ shapes (cpk_/rcp_/cmp_/evt_ ids).
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
