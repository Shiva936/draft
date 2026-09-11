//! A product catalogue: a domain Draft knows nothing about.
//!
//! The adapter here addresses SKUs — `A-100`, `B-200` — which are not paths,
//! have no hierarchy, and contain no separator Core could split even if it
//! wanted to. Its universe divides into two regions of its own naming. Its
//! resources have no byte stream in the filesystem sense; their state is a
//! declared document.
//!
//! Nothing in Core is taught any of that. The adapter is a declarative
//! `resource_adapter` contribution whose operations are real declared commands,
//! executed through the same single process boundary every other contributed
//! mechanism crosses, carrying the same authorization decision. If this works,
//! generality is a property of the contract rather than a claim in a document.
//!
//! Unix-only because it needs a real executable on disk. The path under test is
//! the production one on every platform; what varies is the ability to write a
//! script from a test.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use draft_core::dcg::observation::AdapterBindingId;
use draft_core::execution::command_adapter::CommandResourceSource;
use draft_core::extension::provenance::ProducerRef;
use draft_core::project::layout::DraftLayout;
use draft_core::project::object_store::ObjectStore;
use draft_extension_contract::{
    AdapterCapabilities, Executor, MechanismOperation, ObservationConsistency,
    RecoveryContribution, ResourceAdapterContribution, SchemaRef, StructuredCommand,
};

/// The catalogue this adapter speaks for.
///
/// Two regions, four SKUs, one of them deliberately in the second region so a
/// coverage domain other than the first is exercised.
pub const SEED: &str = r#"{
  "north": {"A-100": {"name": "widget", "price": 100},
            "A-200": {"name": "gasket", "price": 40}},
  "south": {"B-100": {"name": "flange", "price": 250},
            "B-200": {"name": "bracket", "price": 75}}
}"#;

/// The adapter itself: a real program implementing Draft's adapter protocol.
///
/// It reads the canonical request document Draft writes, and answers on stdout.
/// It knows about SKUs and regions; Draft knows about neither.
pub const ADAPTER: &str = r#"#!/usr/bin/env python3
import base64, json, sys, os

STORE = os.environ.get("CATALOG_STORE") or STORE_PATH

def load():
    with open(STORE) as handle:
        return json.load(handle)

def save(data):
    with open(STORE, "w") as handle:
        json.dump(data, handle)

def resources(data):
    out = []
    for region, entries in sorted(data.items()):
        for sku, body in sorted(entries.items()):
            out.append({
                "resource_id": "sku:" + sku,
                "body": sku,
                "form": "logical",
                "state": body,
                "generation": "rev-" + str(body.get("price")),
                "domain": region,
            })
    return out

request = json.load(open(sys.argv[1]))
operation = request["operation"]
data = load()

if operation == "enumerate":
    print(json.dumps({
        "resources": resources(data),
        "domains": [{"local_id": r, "complete": True} for r in sorted(data)],
        "gaps": [],
        "untrackable": [],
    }))
elif operation == "describe":
    for entry in resources(data):
        if entry["body"] == request["body"]:
            print(json.dumps(entry))
            break
    else:
        sys.exit("no such sku")
elif operation == "content":
    for region, entries in data.items():
        if request["body"] in entries:
            raw = json.dumps(entries[request["body"]], sort_keys=True).encode()
            print(json.dumps({"bytes": base64.b64encode(raw).decode()}))
            break
    else:
        sys.exit("no such sku")
elif operation == "mutate":
    changed = []
    for step in request["steps"]:
        sku = step.get("body")
        for region, entries in data.items():
            if sku in entries:
                if step["step"] == "remove":
                    del entries[sku]
                elif step["step"] == "set_content":
                    entries[sku] = json.loads(base64.b64decode(step["content"]))
                changed.append(sku)
                break
    save(data)
    print(json.dumps({"changed": changed}))
elif operation == "capture":
    for region, entries in data.items():
        if request["body"] in entries:
            print(json.dumps({
                "material": {"region": region, "entry": entries[request["body"]]},
                "referenced_objects": [],
            }))
            break
    else:
        sys.exit("no such sku")
elif operation == "restore":
    changed = []
    for target in request["targets"]:
        material = target["material"]
        data.setdefault(material["region"], {})[target["body"]] = material["entry"]
        changed.append(target["body"])
    for body in request["absences"]:
        for entries in data.values():
            entries.pop(body, None)
        changed.append(body)
    save(data)
    print(json.dumps({"changed": changed}))
else:
    sys.exit("unknown operation " + operation)
"#;

pub struct Fixture {
    pub _project: tempfile::TempDir,
    pub root: PathBuf,
    pub store_path: PathBuf,
    pub program: PathBuf,
}

pub fn fixture() -> Fixture {
    let project = tempfile::tempdir().unwrap();
    let root = project.path().to_path_buf();
    std::fs::create_dir_all(root.join(".draft")).unwrap();
    DraftLayout::for_root(&root).create_all().unwrap();

    let store_path = root.join("catalog.json");
    std::fs::write(&store_path, SEED).unwrap();

    // The adapter knows where its backend lives, as any real one would.
    let program = root.join("catalog-adapter");
    let source = ADAPTER.replace(
        "STORE_PATH",
        &format!("{:?}", store_path.to_string_lossy().to_string()),
    );
    std::fs::write(&program, source).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

    Fixture {
        _project: project,
        root,
        store_path,
        program,
    }
}

pub fn operation(program: &Path, name: &str) -> MechanismOperation {
    MechanismOperation {
        request_contract: SchemaRef::new(
            draft_extension_contract::NamespacedId::parse(&format!(
                "draft.core/adapter-{name}-request"
            ))
            .unwrap(),
            1,
        ),
        response_contract: SchemaRef::new(
            draft_extension_contract::NamespacedId::parse(&format!(
                "draft.core/adapter-{name}-response"
            ))
            .unwrap(),
            1,
        ),
        max_response_bytes: 64 * 1024,
        executor: Executor::Command {
            command: StructuredCommand {
                program: program.to_string_lossy().into_owned(),
                args: vec!["{{request}}".to_string()],
                cwd: None,
                // Generous on purpose. What these tests prove is what the
                // adapter contract *means*; the timeout is fixture
                // configuration, and a value tight enough to be hit by
                // scheduling noise would make a green run stop being evidence.
                timeout_ms: Some(600_000),
            },
        },
    }
}

pub fn contribution(program: &Path) -> ResourceAdapterContribution {
    ResourceAdapterContribution {
        scheme: "catalog".into(),
        capabilities: AdapterCapabilities {
            observation_consistency: ObservationConsistency::DigestRevalidation,
            supports_ranged_read: false,
            supports_mutation: true,
            asserts_external_identity: true,
        },
        enumerate: operation(program, "enumerate"),
        describe: operation(program, "describe"),
        content: operation(program, "content"),
        mutate: Some(operation(program, "mutate")),
        recovery: RecoveryContribution::AdapterManaged {
            capture: operation(program, "capture"),
            restore: operation(program, "restore"),
        },
    }
}

pub fn source(fixture: &Fixture, authorized: bool) -> CommandResourceSource {
    CommandResourceSource::new(
        contribution(&fixture.program),
        AdapterBindingId("ext.catalog".into()),
        // Distinct per fixture: runtime scopes are rooted at the workspace id,
        // and tests running in parallel must not share a parent directory.
        format!(
            "ws_catalog_{}",
            fixture.root.file_name().unwrap().to_string_lossy()
        ),
        ProducerRef {
            extension_id: "example.catalog".into(),
            extension_version: "1.0.0".into(),
            package_digest: "sha256:package".into(),
            attestation_digest: "sha256:attestation".into(),
        },
        authorized.then(|| "sha256:decision".to_string()),
        ObjectStore::new(DraftLayout::for_root(&fixture.root)),
    )
}
