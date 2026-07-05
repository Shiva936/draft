use draft_ipc::{socket_path, Request};
use draft_sessions::SessionManager;
use draft_store::ServiceStore;
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};

fn call(
    store: &ServiceStore,
    sessions: &SessionManager,
    id: &str,
    method: &str,
    params: Value,
) -> draft_ipc::Response {
    draftd::dispatch(store, sessions, Request::new(id, method, params))
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::path::Path>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value.as_ref());
        EnvVarGuard { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

#[test]
fn daemon_dispatcher_covers_control_plane() {
    let _env_lock = env_lock().lock().unwrap();
    let state = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf());
    let sessions = SessionManager::new();
    let path = workspace.path().display().to_string();

    let resp = call(&store, &sessions, "1", "service.ping", Value::Null);
    assert!(resp.ok);
    assert_eq!(resp.result.unwrap()["pong"], true);

    let resp = call(
        &store,
        &sessions,
        "2",
        "workspace.init",
        json!({ "path": path }),
    );
    assert!(resp.ok, "{:?}", resp.error);

    let resp = call(
        &store,
        &sessions,
        "3",
        "workspace.register",
        json!({ "path": workspace.path().display().to_string() }),
    );
    assert!(resp.ok, "{:?}", resp.error);

    let resp = call(&store, &sessions, "4", "service.status", Value::Null);
    assert!(resp.ok);
    assert_eq!(resp.result.unwrap()["workspaces"], 1);

    std::fs::write(workspace.path().join("app.txt"), "v1\n").unwrap();
    let resp = call(
        &store,
        &sessions,
        "5",
        "workspace.status",
        json!({ "path": workspace.path().display().to_string() }),
    );
    assert!(resp.ok, "{:?}", resp.error);

    let resp = call(
        &store,
        &sessions,
        "6",
        "checkpoint.create",
        json!({ "path": workspace.path().display().to_string(), "message": "base" }),
    );
    assert!(resp.ok, "{:?}", resp.error);
    let snapshot_id = resp.result.as_ref().unwrap()["snapshot_id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = call(
        &store,
        &sessions,
        "7",
        "task.create",
        json!({ "path": workspace.path().display().to_string(), "title": "update app" }),
    );
    assert!(resp.ok, "{:?}", resp.error);
    let task_id = resp.result.as_ref().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = call(
        &store,
        &sessions,
        "8",
        "task.list",
        json!({ "path": workspace.path().display().to_string() }),
    );
    assert!(resp.ok);
    assert_eq!(resp.result.unwrap().as_array().unwrap().len(), 1);

    std::fs::write(workspace.path().join("app.txt"), "v2\n").unwrap();
    let resp = call(
        &store,
        &sessions,
        "9",
        "pack.create",
        json!({
            "path": workspace.path().display().to_string(),
            "name": "candidate",
            "task": task_id,
            "from_working_tree": true
        }),
    );
    assert!(resp.ok, "{:?}", resp.error);
    let pack_id = resp.result.as_ref().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    for (id, method, extra) in [
        ("10", "pack.list", json!({})),
        ("11", "pack.show", json!({ "pack": pack_id })),
        ("12", "verify.run", json!({ "pack": pack_id })),
        ("13", "risk.assess", json!({ "pack": pack_id })),
        ("14", "review.start", json!({ "pack": pack_id })),
        (
            "15",
            "decision.approve",
            json!({ "pack": pack_id, "reason": "reviewed" }),
        ),
        ("16", "save.run", json!({ "pack": pack_id })),
        ("17", "receipt.list", json!({})),
        ("18", "events.list", json!({})),
        ("19", "events.verify", json!({})),
        ("20", "events.replay", json!({})),
        ("21", "index.rebuild", json!({})),
        ("22", "rollback.run", json!({ "target": snapshot_id })),
        ("24", "run.list", json!({})),
    ] {
        let mut params = extra;
        params["path"] = json!(workspace.path().display().to_string());
        let resp = call(&store, &sessions, id, method, params);
        assert!(resp.ok, "{method} failed: {:?}", resp.error);
    }

    let receipts = call(
        &store,
        &sessions,
        "24",
        "receipt.list",
        json!({ "path": workspace.path().display().to_string() }),
    );
    assert!(receipts.ok, "{:?}", receipts.error);
    let receipt_values = receipts.result.as_ref().unwrap().as_array().unwrap();
    for event_type in ["PackVerified", "PackApproved", "PackSaved"] {
        assert!(
            receipt_values.iter().any(|r| r["event_type"] == event_type),
            "missing canonical {event_type} receipt in {receipt_values:?}"
        );
    }
    let save_receipt = receipt_values
        .iter()
        .find(|r| r["event_type"] == "PackSaved")
        .unwrap();
    let receipt_id = save_receipt["id"]
        .as_str()
        .or_else(|| save_receipt["receipt_id"].as_str())
        .unwrap()
        .to_string();
    let resp = call(
        &store,
        &sessions,
        "25",
        "receipt.show",
        json!({ "path": workspace.path().display().to_string(), "receipt_id": receipt_id }),
    );
    assert!(resp.ok, "{:?}", resp.error);

    let resp = call(
        &store,
        &sessions,
        "job-1",
        "job.submit",
        json!({
            "path": workspace.path().display().to_string(),
            "kind": "scan"
        }),
    );
    assert!(resp.ok, "{:?}", resp.error);
    let job_id = resp.result.as_ref().unwrap()["id"].as_str().unwrap();
    assert_eq!(resp.result.as_ref().unwrap()["status"], "completed");

    let resp = call(
        &store,
        &sessions,
        "job-2",
        "job.status",
        json!({ "job_id": job_id }),
    );
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.result.unwrap()["kind"], "scan");

    let resp = call(&store, &sessions, "job-3", "job.list", Value::Null);
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.result.unwrap().as_array().unwrap().len(), 1);

    let resp = call(
        &store,
        &sessions,
        "26",
        "workspace.status",
        json!({ "path": "../etc" }),
    );
    assert!(!resp.ok);
    assert_eq!(resp.error.unwrap().code, "IPC_ERROR");

    let resp = call(&store, &sessions, "27", "nope.method", Value::Null);
    assert!(!resp.ok);
    assert_eq!(resp.error.unwrap().code, "UNKNOWN_METHOD");

    let resp = call(&store, &sessions, "28", "service.shutdown", Value::Null);
    assert!(resp.ok);
}

#[test]
fn socket_path_is_local() {
    let _env_lock = env_lock().lock().unwrap();
    std::env::remove_var("XDG_RUNTIME_DIR");
    let p = socket_path();
    assert!(p.to_string_lossy().ends_with("draftd.sock"));
}
