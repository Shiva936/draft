use draft_ipc::{socket_path, HandshakeRequest, Request, IPC_CAPABILITIES, IPC_PROTOCOL};
use draft_sessions::SessionManager;
use draft_store::{ServiceJobRecord, ServiceJobStatus, ServiceStore};
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
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
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
        json!({
            "path": workspace.path().display().to_string(),
            "name": "update-app",
            "goal": "update app",
            "success_criteria": ["app contains the requested update"]
        }),
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
        ("16", "submit.run", json!({ "pack": pack_id })),
        ("17", "receipt.list", json!({})),
        ("18", "events.list", json!({})),
        ("19", "events.verify", json!({})),
        ("20", "events.replay", json!({})),
        ("21", "index.rebuild", json!({})),
        ("22", "rollback.run", json!({ "target": snapshot_id })),
        ("24", "execution.list", json!({})),
    ] {
        let mut params = extra;
        params["path"] = json!(workspace.path().display().to_string());
        let resp = call(&store, &sessions, id, method, params);
        assert!(resp.ok, "{method} failed: {:?}", resp.error);
    }

    let summaries = call(
        &store,
        &sessions,
        "canonical-pack-summaries",
        "pack.canonical.list",
        json!({ "path": workspace.path().display().to_string() }),
    );
    assert!(summaries.ok, "{:?}", summaries.error);
    let candidate = summaries
        .result
        .as_ref()
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|pack| pack["pack_id"] == pack_id)
        .unwrap();
    assert_eq!(candidate["name"], "candidate");
    assert_eq!(candidate["submit_state"], "submitted");
    assert!(candidate["valid_actions"].is_array());

    let receipts = call(
        &store,
        &sessions,
        "24",
        "receipt.list",
        json!({ "path": workspace.path().display().to_string() }),
    );
    assert!(receipts.ok, "{:?}", receipts.error);
    let receipt_values = receipts.result.as_ref().unwrap().as_array().unwrap();
    for event_type in ["PackVerified", "PackApproved", "PackSubmitted"] {
        assert!(
            receipt_values.iter().any(|r| r["event_type"] == event_type),
            "missing canonical {event_type} receipt in {receipt_values:?}"
        );
    }
    let save_receipt = receipt_values
        .iter()
        .find(|r| r["event_type"] == "PackSubmitted")
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
    let job_id = resp.result.as_ref().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(resp.result.as_ref().unwrap()["status"], "queued");

    let mut resp = call(
        &store,
        &sessions,
        "job-2",
        "job.status",
        json!({ "job_id": job_id }),
    );
    for _ in 0..100 {
        if resp.result.as_ref().unwrap()["status"] == "completed" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        resp = call(
            &store,
            &sessions,
            "job-2",
            "job.status",
            json!({ "job_id": job_id }),
        );
    }
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.result.as_ref().unwrap()["status"], "completed");
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
fn durable_jobs_recover_and_honor_cancellation() {
    let _env_lock = env_lock().lock().unwrap();
    let state = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let path = workspace.path().display().to_string();
    assert!(
        call(
            &store,
            &sessions,
            "recover-init",
            "workspace.init",
            json!({ "path": path }),
        )
        .ok
    );

    let queued = ServiceJobRecord {
        schema_version: draft_core::contracts::current_version(
            draft_core::contracts::ContractId::ServiceJob,
        ),
        id: "job_recovery_test".into(),
        kind: "scan".into(),
        workspace_path: path.clone(),
        status: ServiceJobStatus::Running,
        submitted_at: chrono::Utc::now(),
        started_at: Some(chrono::Utc::now()),
        ended_at: None,
        result: None,
        error: None,
        operation_id: Some("op_recovery_test".into()),
        workspace_id: None,
        phase: "executing".into(),
        progress_completed: 0,
        progress_total: Some(1),
        cancellation_requested: false,
        params: json!({ "path": path, "kind": "scan" }),
        correlation_id: "cor_recovery_test".into(),
        attempt: 1,
        recovered_at: None,
    };
    store.save_job(&queued).unwrap();
    assert_eq!(draftd::recover_jobs(&store).unwrap(), 1);
    for _ in 0..100 {
        let current = store.load_job(&queued.id).unwrap().unwrap();
        if current.status == ServiceJobStatus::Completed {
            assert_eq!(current.attempt, 2);
            assert!(current.recovered_at.is_some());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        store.load_job(&queued.id).unwrap().unwrap().status,
        ServiceJobStatus::Completed
    );

    let mut cancelled = queued;
    cancelled.id = "job_cancel_test".into();
    cancelled.status = ServiceJobStatus::Queued;
    cancelled.started_at = None;
    cancelled.attempt = 0;
    cancelled.cancellation_requested = false;
    store.save_job(&cancelled).unwrap();
    let response = call(
        &store,
        &sessions,
        "cancel",
        "job.cancel",
        json!({ "job_id": cancelled.id }),
    );
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.result.unwrap()["status"], "cancelled");
    assert_eq!(draftd::recover_jobs(&store).unwrap(), 0);
}

#[test]
fn socket_path_is_local() {
    let _env_lock = env_lock().lock().unwrap();
    std::env::remove_var("XDG_RUNTIME_DIR");
    let p = socket_path();
    assert!(p.to_string_lossy().ends_with("draftd.sock"));
}

#[test]
fn ipc_negotiates_capabilities_and_rejects_invalid_contracts() {
    let store_root = tempfile::tempdir().unwrap();
    let store = ServiceStore::open(store_root.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let request = Request::new(
        "handshake",
        "service.handshake",
        serde_json::to_value(HandshakeRequest {
            protocol: IPC_PROTOCOL.into(),
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::IpcHandshakeRequest,
            ),
            requested_capabilities: vec!["cancellation".into(), "unknown".into()],
            client_name: "test".into(),
            client_version: "0.3.4".into(),
        })
        .unwrap(),
    );
    let response = draftd::dispatch(&store, &sessions, request);
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.result.as_ref().unwrap()["protocol"], IPC_PROTOCOL);
    assert_eq!(
        response.result.as_ref().unwrap()["capabilities"],
        json!(["cancellation"])
    );
    assert!(IPC_CAPABILITIES.contains(&"cancellation"));

    let mut future = Request::new("future", "service.ping", Value::Null);
    future.protocol = "draft-ipc-other".into();
    let response = draftd::dispatch(&store, &sessions, future);
    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");

    let mut future = Request::new("future", "service.ping", Value::Null);
    future.schema_version = 2;
    let response = draftd::dispatch(&store, &sessions, future);
    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_SCHEMA");
}

#[test]
fn mutation_operation_ids_replay_identical_results_and_reject_parameter_reuse() {
    let _env_lock = env_lock().lock().unwrap();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let operation_id = "op_integration_replay";
    let request = || {
        Request::new(
            "init",
            "workspace.init",
            json!({ "path": workspace.path().display().to_string() }),
        )
        .with_operation_id(operation_id)
    };
    let first = draftd::dispatch(&store, &sessions, request());
    assert!(first.ok, "{:?}", first.error);
    let replay = draftd::dispatch(&store, &sessions, request());
    assert!(replay.ok, "{:?}", replay.error);
    assert_eq!(first.result, replay.result);

    let conflicting = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "other",
            "workspace.init",
            json!({ "path": other.path().display().to_string() }),
        )
        .with_operation_id(operation_id),
    );
    assert!(!conflicting.ok);
    assert_eq!(conflicting.error.unwrap().code, "CONFLICT_DETECTED");
    assert!(!other.path().join(".draft").exists());
}

#[cfg(unix)]
#[test]
fn daemon_falls_back_when_xdg_runtime_directory_is_missing() {
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    struct ChildGuard(Option<Child>);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(child) = &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let missing_runtime = root.path().join("missing-runtime");
    let executable = env!("CARGO_BIN_EXE_draftd");

    let child = Command::new(executable)
        .arg("start")
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &missing_runtime)
        .env_remove("XDG_STATE_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut daemon = ChildGuard(Some(child));

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut running = false;
    while Instant::now() < deadline {
        if let Some(status) = daemon.0.as_mut().unwrap().try_wait().unwrap() {
            panic!("draftd exited before answering status: {status}");
        }
        let status = Command::new(executable)
            .arg("status")
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &missing_runtime)
            .env_remove("XDG_STATE_HOME")
            .output()
            .unwrap();
        if status.status.success()
            && String::from_utf8_lossy(&status.stdout).contains("draftd: running")
        {
            running = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(running, "draftd did not answer on the fallback socket");
    assert!(!missing_runtime.exists());
    assert!(home.join(".local/state/draft/draftd.sock").exists());

    let stopped = Command::new(executable)
        .arg("stop")
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &missing_runtime)
        .env_remove("XDG_STATE_HOME")
        .output()
        .unwrap();
    assert!(stopped.status.success());

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if daemon.0.as_mut().unwrap().try_wait().unwrap().is_some() {
            daemon.0.take();
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("draftd did not stop after the shutdown request");
}
