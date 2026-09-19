use draft_ipc::console_application::{
    CanonicalRevisions, ConsoleActionInvocation, ConsoleHandshakeRequest, ConsoleModelRequest,
    ConsoleProtocolVersion, ConsoleSubject, ConsoleWatchRequest, CONSOLE_CAPABILITIES,
};
use draft_ipc::{socket_path, HandshakeRequest, Request, IPC_CAPABILITIES, IPC_PROTOCOL};
use draft_sessions::{ManualClock, SessionManager};
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

/// Serialize environment mutation, recovering from a poisoned lock.
///
/// The lock guards `DRAFT_GLOBAL_HOME`, which is process-global. A test that
/// panics while holding it has not corrupted that variable — the guard restores
/// it on unwind — so every other test in this file failing with `PoisonError`
/// would hide the one real failure rather than report it.
fn serialized() -> std::sync::MutexGuard<'static, ()> {
    env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
    let _env_lock = serialized();
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

    // The Change Graph chain, through the daemon: open, seal, establish, judge,
    // gate, decide, promote. `dcg_surface` proves what each step means; this
    // proves the dispatcher routes every one of them.
    let resp = call(
        &store,
        &sessions,
        "9",
        "dcg.change_pack.open",
        json!({
            "path": workspace.path().display().to_string(),
            "intent": "update app",
            "scope": ["app.txt"]
        }),
    );
    assert!(resp.ok, "{:?}", resp.error);
    let change_pack_id = resp.result.as_ref().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // A revision proposes a change, so there has to be one to propose.
    std::fs::write(workspace.path().join("app.txt"), "v2\n").unwrap();
    let resp = call(
        &store,
        &sessions,
        "10",
        "dcg.revision_pack.seal",
        json!({
            "path": workspace.path().display().to_string(),
            "change_pack_id": change_pack_id
        }),
    );
    assert!(resp.ok, "{:?}", resp.error);
    let revision_id = resp.result.as_ref().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = call(
        &store,
        &sessions,
        "11",
        "dcg.gate.evaluate",
        json!({
            "path": workspace.path().display().to_string(),
            "revision_pack_id": revision_id
        }),
    );
    assert!(resp.ok, "{:?}", resp.error);
    let gate_id = resp.result.as_ref().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    for (id, method, extra) in [
        ("12", "dcg.project", json!({})),
        ("13", "dcg.baseline", json!({})),
        ("14", "dcg.change_pack.list", json!({})),
        (
            "15",
            "dcg.evidence.record",
            json!({ "revision_pack_id": revision_id }),
        ),
        (
            "16",
            "dcg.assessment.record",
            json!({ "revision_pack_id": revision_id, "risk": "low", "rationale": "reviewed" }),
        ),
        (
            "17",
            "dcg.authorization",
            json!({ "change_pack_id": change_pack_id, "revision_pack_id": revision_id }),
        ),
        ("18", "receipt.list", json!({})),
        ("19", "events.list", json!({})),
        ("20", "events.verify", json!({})),
        ("21", "events.replay", json!({})),
        ("22", "index.rebuild", json!({})),
        ("23", "rollback.run", json!({ "target": snapshot_id })),
        ("24", "execution.list", json!({})),
    ] {
        let mut params = extra;
        params["path"] = json!(workspace.path().display().to_string());
        let resp = call(&store, &sessions, id, method, params);
        assert!(resp.ok, "{method} failed: {:?}", resp.error);
    }
    let _ = (&task_id, &gate_id);

    let summaries = call(
        &store,
        &sessions,
        "change-views",
        "dcg.change_pack.list",
        json!({ "path": workspace.path().display().to_string() }),
    );
    assert!(summaries.ok, "{:?}", summaries.error);
    let opened = summaries
        .result
        .as_ref()
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["change_pack"] == change_pack_id)
        .expect("the ChangePack the daemon opened is listed");
    assert_eq!(opened["lifecycle"], "active");
    assert!(
        opened["revisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|revision| revision["id"] == revision_id),
        "the sealed revision is listed against its ChangePack"
    );

    let receipts = call(
        &store,
        &sessions,
        "24",
        "receipt.list",
        json!({ "path": workspace.path().display().to_string() }),
    );
    assert!(receipts.ok, "{:?}", receipts.error);
    let receipt_values = receipts.result.as_ref().unwrap().as_array().unwrap();
    // A checkpoint is *not* receipted. A v1 receipt attests a Promotion or an
    // external effect — the two acts whose consequences reach outside the
    // moment they happened. A local, reversible action is recorded in the
    // Activity Ledger, which is hash-chained and verified; minting a signed
    // attestation for it as well would give a reader two records of one act
    // and no rule for which is authoritative.
    assert!(
        !receipt_values
            .iter()
            .any(|receipt| receipt["event_type"] == "CheckpointCreated"),
        "a checkpoint must not be receipted: {receipt_values:?}"
    );

    // It is in Activity instead, which is where a reader is meant to find it.
    let events = call(
        &store,
        &sessions,
        "24b",
        "events.list",
        json!({ "path": workspace.path().display().to_string() }),
    );
    assert!(events.ok, "{:?}", events.error);
    let rendered = serde_json::to_string(events.result.as_ref().unwrap()).unwrap();
    assert!(
        rendered.contains("CheckpointCreated"),
        "the checkpoint must be recorded in Activity: {rendered}"
    );

    // And the chain that carries it verifies, which is what makes the Activity
    // record worth reading in a receipt's place.
    let verified = call(
        &store,
        &sessions,
        "24c",
        "events.verify",
        json!({ "path": workspace.path().display().to_string() }),
    );
    assert!(verified.ok, "{:?}", verified.error);

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
    // Same reasoning as the recovery poll below: wait generously so a loaded
    // machine does not read as a failed job.
    for _ in 0..600 {
        if resp.result.as_ref().unwrap()["status"] == "completed" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
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
    let _env_lock = serialized();
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
    // Recovery runs on a worker thread, so this polls rather than assuming a
    // deadline. The budget is generous because a loaded CI runner can take far
    // longer than a quiet one, and a slow machine is not a failed recovery.
    for _ in 0..600 {
        let current = store.load_job(&queued.id).unwrap().unwrap();
        if current.status == ServiceJobStatus::Completed {
            assert_eq!(current.attempt, 2);
            assert!(current.recovered_at.is_some());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
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
    let _env_lock = serialized();
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
fn console_application_handshake_negotiates_minor_and_rolls_sessions() {
    let _env_lock = serialized();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let request = |id: &str, major: u16| {
        Request::new(
            id,
            "console.handshake",
            serde_json::to_value(ConsoleHandshakeRequest {
                protocol: ConsoleProtocolVersion { major, minor: 99 },
                client_name: "test-tui".into(),
                client_version: "test".into(),
                client_instance_id: "same-client".into(),
                requested_capabilities: vec!["revisioned_models".into(), "unknown".into()],
            })
            .unwrap(),
        )
    };
    let first = draftd::dispatch(&store, &sessions, request("first", 1));
    assert!(first.ok, "{:?}", first.error);
    assert_eq!(first.result.as_ref().unwrap()["protocol"]["minor"], 0);
    assert_eq!(
        first.result.as_ref().unwrap()["negotiated_capabilities"],
        json!(["revisioned_models"])
    );
    let old_session = first.result.unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let second = draftd::dispatch(&store, &sessions, request("second", 1));
    let new_session = second.result.unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(old_session, new_session);
    assert!(sessions.application(&old_session).is_none());
    assert!(CONSOLE_CAPABILITIES.contains(&"revisioned_models"));

    let incompatible = draftd::dispatch(&store, &sessions, request("future", 2));
    assert!(!incompatible.ok);
    assert_eq!(
        incompatible.error.unwrap().code,
        "INCOMPATIBLE_CONSOLE_PROTOCOL_MAJOR"
    );
}

#[test]
fn console_global_snapshot_uses_fixed_navigation() {
    let _env_lock = serialized();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let handshake = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "handshake",
            "console.handshake",
            serde_json::to_value(ConsoleHandshakeRequest {
                protocol: ConsoleProtocolVersion::default(),
                client_name: "test".into(),
                client_version: "test".into(),
                client_instance_id: "snapshot-test".into(),
                requested_capabilities: vec!["revisioned_models".into()],
            })
            .unwrap(),
        ),
    );
    let session = handshake.result.unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let snapshot = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "snapshot",
            "console.snapshot",
            serde_json::to_value(ConsoleModelRequest {
                application_session_id: session.clone(),
                subject: ConsoleSubject::global(),
            })
            .unwrap(),
        ),
    );
    assert!(snapshot.ok, "{:?}", snapshot.error);
    assert_eq!(
        snapshot.result.unwrap()["navigation"],
        json!([
            {"label": "Overview", "children": []},
            {"label": "Projects", "children": []},
            {"label": "Inbox", "children": []},
            {"label": "Doctor", "children": []},
            {"label": "Extensions", "children": []},
            {"label": "Settings", "children": []},
        ])
    );

    let watch = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "watch",
            "console.watch",
            serde_json::to_value(ConsoleWatchRequest {
                application_session_id: session,
                after_cursor: None,
                subjects: vec![ConsoleSubject::global()],
            })
            .unwrap(),
        ),
    );
    assert!(!watch.ok);
    assert_eq!(watch.error.unwrap().code, "UNSUPPORTED_SCHEMA");
}

#[test]
fn console_actions_are_revision_bound_single_use_and_operation_idempotent() {
    let _env_lock = serialized();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let path = workspace.path().display().to_string();
    assert!(
        call(
            &store,
            &sessions,
            "init",
            "workspace.init",
            json!({"path": path})
        )
        .ok
    );
    let registered = call(
        &store,
        &sessions,
        "register",
        "workspace.register",
        json!({"path": path}),
    );
    let workspace_id = registered.result.unwrap()["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::write(workspace.path().join("app.txt"), "base\n").unwrap();
    assert!(
        call(
            &store,
            &sessions,
            "checkpoint",
            "checkpoint.create",
            json!({"path": path, "message": "base"})
        )
        .ok
    );
    let change = call(
        &store,
        &sessions,
        "change",
        "dcg.change_pack.open",
        json!({ "path": path, "intent": "console action", "scope": ["app.txt"] }),
    );
    assert!(change.ok, "{:?}", change.error);
    let change_pack_id = change.result.unwrap()["id"].as_str().unwrap().to_string();

    std::fs::write(workspace.path().join("app.txt"), "changed\n").unwrap();
    let sealed = call(
        &store,
        &sessions,
        "seal",
        "dcg.revision_pack.seal",
        json!({ "path": path, "change_pack_id": change_pack_id }),
    );
    assert!(sealed.ok, "{:?}", sealed.error);
    let revision_id = sealed.result.unwrap()["id"].as_str().unwrap().to_string();

    let handshake = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "console-handshake",
            "console.handshake",
            serde_json::to_value(ConsoleHandshakeRequest {
                protocol: ConsoleProtocolVersion::default(),
                client_name: "test".into(),
                client_version: "test".into(),
                client_instance_id: "action-test".into(),
                requested_capabilities: vec!["action_capabilities".into()],
            })
            .unwrap(),
        ),
    );
    let application_session_id = handshake.result.unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let snapshot = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "snapshot",
            "console.snapshot",
            serde_json::to_value(ConsoleModelRequest {
                application_session_id: application_session_id.clone(),
                subject: ConsoleSubject::ChangePack {
                    workspace_id: workspace_id.clone(),
                    change_pack_id: change_pack_id.clone(),
                },
            })
            .unwrap(),
        ),
    );
    assert!(snapshot.ok, "{:?}", snapshot.error);
    let model = snapshot.result.unwrap();

    let no_capability_handshake = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "console-handshake-no-actions",
            "console.handshake",
            serde_json::to_value(ConsoleHandshakeRequest {
                protocol: ConsoleProtocolVersion::default(),
                client_name: "test".into(),
                client_version: "test".into(),
                client_instance_id: "action-test-without-capability".into(),
                requested_capabilities: vec!["revisioned_models".into()],
            })
            .unwrap(),
        ),
    );
    let no_capability_session = no_capability_handshake.result.unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let no_capability_snapshot = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "snapshot-no-actions",
            "console.snapshot",
            serde_json::to_value(ConsoleModelRequest {
                application_session_id: no_capability_session,
                subject: ConsoleSubject::ChangePack {
                    workspace_id,
                    change_pack_id,
                },
            })
            .unwrap(),
        ),
    );
    assert!(
        no_capability_snapshot.ok,
        "{:?}",
        no_capability_snapshot.error
    );
    let no_capability_actions = no_capability_snapshot.result.unwrap()["actions"]
        .as_array()
        .unwrap()
        .clone();
    assert!(no_capability_actions
        .iter()
        .all(|action| { action["enabled"] == false && action["invocation_capability"].is_null() }));

    let verify = model["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["action_id"] == "dcg.evidence.record")
        .expect("establishing evidence is an offered Console action");
    assert_eq!(verify["enabled"], true);
    let capability = verify["invocation_capability"]
        .as_str()
        .unwrap()
        .to_string();
    let expected_revisions: CanonicalRevisions =
        serde_json::from_value(model["revisions"].clone()).unwrap();
    let invocation = ConsoleActionInvocation {
        application_session_id,
        invocation_capability: capability,
        operation_id: "op_console_verify".into(),
        expected_revisions,
        // The action establishes evidence about one exact revision, and there
        // is no default for which — a Console that omitted it would be asking
        // Draft to pick what gets verified.
        arguments: [("revision_pack_id".to_string(), json!(revision_id))]
            .into_iter()
            .collect(),
    };
    let request = || {
        Request::new(
            "invoke",
            "console.action.invoke",
            serde_json::to_value(invocation.clone()).unwrap(),
        )
        .with_operation_id("op_console_verify")
    };
    let first = draftd::dispatch(&store, &sessions, request());
    assert!(first.ok, "{:?}", first.error);
    let replay = draftd::dispatch(&store, &sessions, request());
    assert!(replay.ok, "{:?}", replay.error);
    assert_eq!(first.result, replay.result);

    let reused = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "reuse",
            "console.action.invoke",
            serde_json::to_value(ConsoleActionInvocation {
                operation_id: "op_console_verify_reuse".into(),
                ..invocation
            })
            .unwrap(),
        )
        .with_operation_id("op_console_verify_reuse"),
    );
    assert!(!reused.ok);
    assert_eq!(reused.error.unwrap().code, "STALE_CONSOLE_ACTION");
}

#[test]
fn mutation_operation_ids_replay_identical_results_and_reject_parameter_reuse() {
    let _env_lock = serialized();
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

/// Global-scope extension actions are offered, typed, and server-validated.
///
/// This is the boundary the TUI needs: extension management arrives as
/// `ActionPresentation`s carrying their own input contract, so a terminal
/// frontend renders the same authoritative workflow the browser does without
/// knowing anything about extensions.
#[test]
fn extension_actions_are_offered_with_a_server_owned_input_contract() {
    let _env_lock = serialized();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();

    let handshake = call(
        &store,
        &sessions,
        "handshake",
        "console.handshake",
        serde_json::to_value(ConsoleHandshakeRequest {
            protocol: ConsoleProtocolVersion::default(),
            client_name: "test".into(),
            client_version: "test".into(),
            client_instance_id: "extension-actions".into(),
            requested_capabilities: CONSOLE_CAPABILITIES
                .iter()
                .map(|capability| (*capability).to_string())
                .collect(),
        })
        .unwrap(),
    );
    let session = handshake.result.unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let snapshot = call(
        &store,
        &sessions,
        "snapshot",
        "console.snapshot",
        serde_json::to_value(ConsoleModelRequest {
            application_session_id: session.clone(),
            subject: ConsoleSubject::global(),
        })
        .unwrap(),
    );
    assert!(snapshot.ok, "{:?}", snapshot.error);
    let model = snapshot.result.unwrap();
    let actions = model["actions"].as_array().unwrap().clone();
    assert!(
        !actions.is_empty(),
        "the Global scope must offer extension management"
    );

    let add_source = actions
        .iter()
        .find(|action| action["action_id"] == "extension.source.add")
        .expect("adding a catalog source is an authoritative action");

    // The action carries its own inputs, each with a stable id distinct from
    // its label, plus the digest a capability is bound to.
    let inputs = add_source["inputs"].as_array().unwrap();
    let ids: Vec<&str> = inputs
        .iter()
        .map(|input| input["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["source_id", "location"]);
    // The kind is a nested tagged object, not a bare string: the contract holds
    // it as a named field so it generates cleanly for the browser too.
    assert!(inputs
        .iter()
        .all(|input| input["id"] != input["label"] && input["kind"]["type"] == "text"));
    assert!(!add_source["input_contract_digest"]
        .as_str()
        .unwrap()
        .is_empty());
    assert!(add_source["enabled"].as_bool().unwrap());

    let revisions: CanonicalRevisions = serde_json::from_value(model["revisions"].clone()).unwrap();
    let capability = |action: &Value| {
        action["invocation_capability"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let invoke = |token: String, arguments: serde_json::Map<String, Value>, id: &str| {
        call(
            &store,
            &sessions,
            id,
            "console.action.invoke",
            serde_json::to_value(ConsoleActionInvocation {
                application_session_id: session.clone(),
                invocation_capability: token,
                operation_id: format!("op-{id}"),
                expected_revisions: revisions.clone(),
                arguments: arguments.into_iter().collect(),
            })
            .unwrap(),
        )
    };

    // An argument the action never declared is refused by the server.
    let undeclared = invoke(
        capability(add_source),
        serde_json::Map::from_iter([
            ("source_id".to_string(), json!("acme")),
            ("location".to_string(), json!("/tmp/acme")),
            ("smuggled".to_string(), json!("value")),
        ]),
        "undeclared",
    );
    assert!(!undeclared.ok);
    assert!(undeclared
        .error
        .unwrap()
        .message
        .contains("does not declare an input named 'smuggled'"));

    // A required input that is missing is refused.
    let refreshed = call(
        &store,
        &sessions,
        "snapshot-2",
        "console.snapshot",
        serde_json::to_value(ConsoleModelRequest {
            application_session_id: session.clone(),
            subject: ConsoleSubject::global(),
        })
        .unwrap(),
    )
    .result
    .unwrap();
    let add_again = refreshed["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["action_id"] == "extension.source.add")
        .unwrap()
        .clone();
    let missing = invoke(
        capability(&add_again),
        serde_json::Map::from_iter([("source_id".to_string(), json!("acme"))]),
        "missing",
    );
    assert!(!missing.ok);
    assert!(missing
        .error
        .unwrap()
        .message
        .contains("'location' is required"));

    // A declared input of the wrong shape is refused.
    let refreshed = call(
        &store,
        &sessions,
        "snapshot-3",
        "console.snapshot",
        serde_json::to_value(ConsoleModelRequest {
            application_session_id: session.clone(),
            subject: ConsoleSubject::global(),
        })
        .unwrap(),
    )
    .result
    .unwrap();
    let add_third = refreshed["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["action_id"] == "extension.source.add")
        .unwrap()
        .clone();
    let wrong_type = invoke(
        capability(&add_third),
        serde_json::Map::from_iter([
            ("source_id".to_string(), json!(7)),
            ("location".to_string(), json!("/tmp/acme")),
        ]),
        "wrong-type",
    );
    assert!(!wrong_type.ok);
    assert!(wrong_type.error.unwrap().message.contains("expects text"));
}

/// Eligibility is the server's decision, not a frontend's.
#[test]
fn extension_action_eligibility_follows_authoritative_state() {
    let _env_lock = serialized();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();

    let handshake = call(
        &store,
        &sessions,
        "handshake",
        "console.handshake",
        serde_json::to_value(ConsoleHandshakeRequest {
            protocol: ConsoleProtocolVersion::default(),
            client_name: "test".into(),
            client_version: "test".into(),
            client_instance_id: "eligibility".into(),
            requested_capabilities: CONSOLE_CAPABILITIES
                .iter()
                .map(|capability| (*capability).to_string())
                .collect(),
        })
        .unwrap(),
    );
    let session = handshake.result.unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let actions = call(
        &store,
        &sessions,
        "snapshot",
        "console.snapshot",
        serde_json::to_value(ConsoleModelRequest {
            application_session_id: session.clone(),
            subject: ConsoleSubject::global(),
        })
        .unwrap(),
    )
    .result
    .unwrap()["actions"]
        .as_array()
        .unwrap()
        .clone();

    // With no source configured there is nothing to install from, so the
    // install action is not offered at all — the frontend is never left to
    // work that out from raw records.
    assert!(
        !actions
            .iter()
            .any(|action| action["action_id"] == "extension.install"),
        "installing needs a configured source"
    );

    // Every offered action that is disabled says why, and carries no
    // capability that could be replayed.
    for action in &actions {
        if !action["enabled"].as_bool().unwrap() {
            assert!(action["disabled_reason"].is_string());
            assert!(action["invocation_capability"].is_null());
        }
    }
}

/// An extension action invoked from the Console reaches the authoritative
/// operation, and its descriptor is spent once.
///
/// This is the property that makes TUI parity real rather than cosmetic: the
/// action the terminal invokes is the same `extension.source.add` the CLI and
/// the browser call, with no second implementation behind it.
#[test]
fn an_extension_action_runs_through_its_authoritative_operation_once() {
    let _env_lock = serialized();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    // A clock that only this test moves. What is being proved here is that an
    // authorized action crosses its authoritative operation exactly once — not
    // how much real time the surrounding catalog work happened to take. With
    // the wall clock, a loaded machine could expire the capability mid-test and
    // fail for a reason that has nothing to do with the invariant.
    let sessions = SessionManager::with_clock(std::sync::Arc::new(ManualClock::default()));
    let catalog = global.path().join("catalog");
    std::fs::create_dir_all(&catalog).unwrap();

    let handshake = call(
        &store,
        &sessions,
        "handshake",
        "console.handshake",
        serde_json::to_value(ConsoleHandshakeRequest {
            protocol: ConsoleProtocolVersion::default(),
            client_name: "test".into(),
            client_version: "test".into(),
            client_instance_id: "invoke-once".into(),
            requested_capabilities: CONSOLE_CAPABILITIES
                .iter()
                .map(|capability| (*capability).to_string())
                .collect(),
        })
        .unwrap(),
    );
    let session = handshake.result.unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let snapshot = call(
        &store,
        &sessions,
        "snapshot",
        "console.snapshot",
        serde_json::to_value(ConsoleModelRequest {
            application_session_id: session.clone(),
            subject: ConsoleSubject::global(),
        })
        .unwrap(),
    )
    .result
    .unwrap();
    let revisions: CanonicalRevisions =
        serde_json::from_value(snapshot["revisions"].clone()).unwrap();
    let token = snapshot["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["action_id"] == "extension.source.add")
        .unwrap()["invocation_capability"]
        .as_str()
        .unwrap()
        .to_string();

    let invocation = |token: String, id: &str| ConsoleActionInvocation {
        application_session_id: session.clone(),
        invocation_capability: token,
        operation_id: format!("op-{id}"),
        expected_revisions: revisions.clone(),
        arguments: [
            ("source_id".to_string(), json!("acme")),
            (
                "location".to_string(),
                json!(catalog.to_string_lossy().to_string()),
            ),
        ]
        .into_iter()
        .collect(),
    };

    let invoked = call(
        &store,
        &sessions,
        "invoke",
        "console.action.invoke",
        serde_json::to_value(invocation(token.clone(), "one")).unwrap(),
    );
    assert!(invoked.ok, "{:?}", invoked.error);

    // The source really exists now: the action went through the same operation
    // `extension.source.add` performs for every other client. Asked for by id,
    // so the assertion does not depend on whatever else is configured.
    let shown = call(
        &store,
        &sessions,
        "show",
        "extension.source.show",
        json!({ "source_id": "acme" }),
    );
    assert!(shown.ok, "{:?}", shown.error);
    assert_eq!(shown.result.unwrap()["source"]["id"], "acme");

    // The descriptor is spent; replaying it conflicts rather than acting twice.
    let replayed = call(
        &store,
        &sessions,
        "replay",
        "console.action.invoke",
        serde_json::to_value(invocation(token, "two")).unwrap(),
    );
    assert!(!replayed.ok, "an invocation capability is single-use");
    // The Console surfaces a spent descriptor as its own stale-action code,
    // which is the refresh-and-act-again signal every other Console conflict
    // uses.
    assert_eq!(replayed.error.unwrap().code, "STALE_CONSOLE_ACTION");
}

/// Source actions are gated by authoritative trust and built-in state — the
/// rules the browser used to enforce for itself.
#[test]
fn source_actions_carry_their_authoritative_gates() {
    let _env_lock = serialized();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let catalog = global.path().join("catalog");
    std::fs::create_dir_all(&catalog).unwrap();

    let added = call(
        &store,
        &sessions,
        "add",
        "extension.source.add",
        json!({ "source_id": "acme", "location": catalog.to_string_lossy() }),
    );
    assert!(added.ok, "{:?}", added.error);

    let session = call(
        &store,
        &sessions,
        "handshake",
        "console.handshake",
        serde_json::to_value(ConsoleHandshakeRequest {
            protocol: ConsoleProtocolVersion::default(),
            client_name: "test".into(),
            client_version: "test".into(),
            client_instance_id: "source-gates".into(),
            requested_capabilities: CONSOLE_CAPABILITIES
                .iter()
                .map(|capability| (*capability).to_string())
                .collect(),
        })
        .unwrap(),
    )
    .result
    .unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let snapshot = call(
        &store,
        &sessions,
        "snapshot",
        "console.snapshot",
        serde_json::to_value(ConsoleModelRequest {
            application_session_id: session,
            subject: ConsoleSubject::global(),
        })
        .unwrap(),
    )
    .result
    .unwrap();

    let action = |id: &str| {
        snapshot["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["action_id"] == id && action["target"]["id"] == "acme")
            .unwrap_or_else(|| panic!("{id} is offered for the configured source"))
            .clone()
    };

    // The source is configured but its signed root was never accepted, so
    // refreshing is refused here rather than in the browser.
    let refresh = action("extension.source.refresh");
    assert!(!refresh["enabled"].as_bool().unwrap());
    assert!(refresh["disabled_reason"]
        .as_str()
        .unwrap()
        .contains("signed root"));

    // A user-configured source is removable; a built-in one would not be.
    let remove = action("extension.source.remove");
    assert!(remove["enabled"].as_bool().unwrap());

    // The Global model carries the authorization projection, so the Console
    // sees exactly what the CLI does.
    let installed = snapshot["content"]["extensions"]["installed"]
        .as_array()
        .unwrap();
    assert!(installed.is_empty());

    // Nothing is installed, so Update All is offered but disabled, saying why.
    let update_all = snapshot["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["action_id"] == "extension.update_all")
        .expect("update-all is a global action")
        .clone();
    assert!(!update_all["enabled"].as_bool().unwrap());
    assert!(update_all["disabled_reason"].as_str().is_some());
}

/// The race a self-consistent request hides.
///
/// A descriptor and the revisions a client sends back can agree perfectly
/// while the project has moved underneath both of them. Comparing the two
/// proves only that the client echoed what it was given, so this exercises the
/// sequence that check cannot see: issue a capability, commit a real mutation
/// through the ordinary authoritative path, then invoke the old capability.
///
/// Both halves matter. The first proves an evidence fact landing invalidates a
/// descriptor whose offer rested on it; the second proves the same for the
/// ChangePack's own lifecycle. A build that only compared descriptor to request
/// would accept both.
#[test]
fn a_descriptor_is_refused_after_authoritative_state_moves_beneath_it() {
    let _env_lock = serialized();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let path = workspace.path().display().to_string();

    assert!(
        call(
            &store,
            &sessions,
            "init",
            "workspace.init",
            json!({ "path": path })
        )
        .ok
    );
    let registered = call(
        &store,
        &sessions,
        "register",
        "workspace.register",
        json!({ "path": path }),
    );
    let workspace_id = registered.result.unwrap()["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::write(workspace.path().join("app.txt"), "base\n").unwrap();
    assert!(
        call(
            &store,
            &sessions,
            "checkpoint",
            "checkpoint.create",
            json!({ "path": path, "message": "base" })
        )
        .ok
    );
    let change = call(
        &store,
        &sessions,
        "change",
        "dcg.change_pack.open",
        json!({ "path": path, "intent": "stale action", "scope": ["app.txt"] }),
    );
    assert!(change.ok, "{:?}", change.error);
    let change_pack_id = change.result.unwrap()["id"].as_str().unwrap().to_string();
    std::fs::write(workspace.path().join("app.txt"), "changed\n").unwrap();
    let sealed = call(
        &store,
        &sessions,
        "seal",
        "dcg.revision_pack.seal",
        json!({ "path": path, "change_pack_id": change_pack_id }),
    );
    assert!(sealed.ok, "{:?}", sealed.error);
    let revision_id = sealed.result.unwrap()["id"].as_str().unwrap().to_string();

    let open_session = |instance: &str| {
        let handshake = draftd::dispatch(
            &store,
            &sessions,
            Request::new(
                "console-handshake",
                "console.handshake",
                serde_json::to_value(ConsoleHandshakeRequest {
                    protocol: ConsoleProtocolVersion::default(),
                    client_name: "test".into(),
                    client_version: "test".into(),
                    client_instance_id: instance.into(),
                    requested_capabilities: vec!["action_capabilities".into()],
                })
                .unwrap(),
            ),
        );
        handshake.result.unwrap()["application_session_id"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let snapshot = |session: &str| {
        let response = draftd::dispatch(
            &store,
            &sessions,
            Request::new(
                "snapshot",
                "console.snapshot",
                serde_json::to_value(ConsoleModelRequest {
                    application_session_id: session.to_string(),
                    subject: ConsoleSubject::ChangePack {
                        workspace_id: workspace_id.clone(),
                        change_pack_id: change_pack_id.clone(),
                    },
                })
                .unwrap(),
            ),
        );
        assert!(response.ok, "{:?}", response.error);
        response.result.unwrap()
    };

    let descriptor = |model: &Value, action_id: &str| {
        let action = model["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["action_id"] == action_id)
            .unwrap_or_else(|| panic!("{action_id} is an offered Console action"))
            .clone();
        assert_eq!(action["enabled"], true, "{action_id} must be offered");
        (
            action["invocation_capability"]
                .as_str()
                .unwrap()
                .to_string(),
            serde_json::from_value::<CanonicalRevisions>(model["revisions"].clone()).unwrap(),
        )
    };

    // --- an evidence fact lands between the offer and the invocation --------
    let session = open_session("stale-evidence");
    let (capability, expected_revisions) = descriptor(&snapshot(&session), "dcg.evidence.record");

    // The mutation, through the same authoritative operation the Console
    // action would reach. Nothing about the held descriptor changes.
    let recorded = call(
        &store,
        &sessions,
        "record",
        "dcg.evidence.record",
        json!({ "path": path, "revision_pack_id": revision_id }),
    );
    assert!(recorded.ok, "{:?}", recorded.error);

    let refused = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "invoke-stale-evidence",
            "console.action.invoke",
            serde_json::to_value(ConsoleActionInvocation {
                application_session_id: session,
                invocation_capability: capability,
                operation_id: "op_stale_evidence".into(),
                // Deliberately the revisions the descriptor was issued with, so
                // the descriptor-versus-request comparison passes and only the
                // authoritative recheck can refuse this.
                expected_revisions,
                arguments: [("revision_pack_id".to_string(), json!(revision_id))]
                    .into_iter()
                    .collect(),
            })
            .unwrap(),
        )
        .with_operation_id("op_stale_evidence"),
    );
    assert!(
        !refused.ok,
        "a descriptor issued before the evidence fact landed must not act"
    );
    let error = refused.error.unwrap();
    assert_eq!(error.code, "STALE_CONSOLE_ACTION");
    assert!(
        error.message.contains("since moved"),
        "the refusal must name that state moved, got {:?}",
        error.message
    );

    // --- the ChangePack's own lifecycle moves between offer and invocation ------
    let session = open_session("stale-change");
    let (capability, expected_revisions) = descriptor(&snapshot(&session), "dcg.gate.evaluate");
    let abandoned = call(
        &store,
        &sessions,
        "abandon",
        "dcg.change_pack.abandon",
        json!({ "path": path, "change_pack_id": change_pack_id }),
    );
    assert!(abandoned.ok, "{:?}", abandoned.error);

    let refused = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "invoke-stale-change",
            "console.action.invoke",
            serde_json::to_value(ConsoleActionInvocation {
                application_session_id: session,
                invocation_capability: capability,
                operation_id: "op_stale_change".into(),
                expected_revisions,
                arguments: [("revision_pack_id".to_string(), json!(revision_id))]
                    .into_iter()
                    .collect(),
            })
            .unwrap(),
        )
        .with_operation_id("op_stale_change"),
    );
    assert!(
        !refused.ok,
        "a descriptor issued before the ChangePack was abandoned must not act"
    );
    let error = refused.error.unwrap();
    assert_eq!(error.code, "STALE_CONSOLE_ACTION");
    assert!(
        error.message.contains("since moved"),
        "the refusal must come from the authoritative recheck, got {:?}",
        error.message
    );
}

/// The Console's ChangePack navigation names the acts the ontology distinguishes.
///
/// `draftd` is the authority for what a frontend may render, so a stale entry
/// here is a stale entry everywhere. The retired names are asserted absent
/// individually: a single "Submit" step, "Approvals", "Risk" and "Rollback"
/// are precisely the conflations the Change Graph took apart, and one of them
/// reappearing would let a reader mistake an approval for a promotion.
#[test]
fn console_change_navigation_names_the_acts_and_not_the_retired_ones() {
    let _env_lock = serialized();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let path = workspace.path().display().to_string();

    assert!(
        call(
            &store,
            &sessions,
            "init",
            "workspace.init",
            json!({ "path": path })
        )
        .ok
    );
    let registered = call(
        &store,
        &sessions,
        "register",
        "workspace.register",
        json!({ "path": path }),
    );
    let workspace_id = registered.result.unwrap()["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::write(workspace.path().join("app.txt"), "base\n").unwrap();
    assert!(
        call(
            &store,
            &sessions,
            "checkpoint",
            "checkpoint.create",
            json!({ "path": path, "message": "base" })
        )
        .ok
    );
    let change = call(
        &store,
        &sessions,
        "change",
        "dcg.change_pack.open",
        json!({ "path": path, "intent": "navigation", "scope": ["app.txt"] }),
    );
    assert!(change.ok, "{:?}", change.error);
    let change_pack_id = change.result.unwrap()["id"].as_str().unwrap().to_string();
    // Seal one, so the per-revision views have something to be about. They are
    // deliberately absent rather than empty before a revision exists: an empty
    // impact report would claim a revision touches nothing.
    std::fs::write(workspace.path().join("app.txt"), "navigated\n").unwrap();
    let sealed = call(
        &store,
        &sessions,
        "seal",
        "dcg.revision_pack.seal",
        json!({ "path": path, "change_pack_id": change_pack_id }),
    );
    assert!(sealed.ok, "{:?}", sealed.error);

    let handshake = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "handshake",
            "console.handshake",
            serde_json::to_value(ConsoleHandshakeRequest {
                protocol: ConsoleProtocolVersion::default(),
                client_name: "test".into(),
                client_version: "test".into(),
                client_instance_id: "navigation-test".into(),
                requested_capabilities: vec!["revisioned_models".into()],
            })
            .unwrap(),
        ),
    );
    let session = handshake.result.unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let snapshot = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "snapshot",
            "console.snapshot",
            serde_json::to_value(ConsoleModelRequest {
                application_session_id: session,
                subject: ConsoleSubject::ChangePack {
                    workspace_id,
                    change_pack_id,
                },
            })
            .unwrap(),
        ),
    );
    assert!(snapshot.ok, "{:?}", snapshot.error);
    let model = snapshot.result.unwrap();
    // §8.3's fourteen ChangePack views, in the plan's order.
    assert_eq!(
        model["navigation"],
        json!([
            {"label": "Summary", "children": []},
            {"label": "Intent", "children": []},
            {"label": "Scope", "children": []},
            {"label": "Revisions", "children": []},
            {"label": "Impact", "children": []},
            {"label": "Representations", "children": []},
            {"label": "Evidence", "children": []},
            {"label": "Assessments", "children": []},
            {"label": "Review", "children": []},
            {"label": "Decisions", "children": []},
            {"label": "Gates", "children": []},
            {"label": "Promotion", "children": []},
            {"label": "Receipts", "children": []},
            {"label": "Recovery", "children": []},
        ])
    );
    let labels = model["navigation"].to_string();
    for retired in ["Submit", "Approvals", "Risk", "Rollback", "Verify"] {
        assert!(
            !labels.contains(retired),
            "the ChangePack scope still offers the retired '{retired}' view"
        );
    }

    // And each of those views has real backing data, not a label over an
    // empty key. A tab that renders nothing is a tab that lies about what the
    // model holds.
    let content = &model["content"];
    for view in [
        "summary",
        "intent",
        "scope",
        "revisions",
        "impact",
        "representations",
        "authorization",
        "receipts",
        "recovery",
        "activity",
    ] {
        assert!(
            !content[view].is_null(),
            "the ChangePack model carries nothing for '{view}': {content}"
        );
    }
    // Impact is the Stage 11 report, reached through the application API
    // rather than re-derived here; coverage rides with it because "evidence
    // exists" and "this Resource is proved" are different claims.
    assert!(content["impact"]["revision_pack"].is_string(), "{content}");
    assert!(!content["coverage"].is_null(), "{content}");
    // The representation is the neutral rendering, recorded at seal.
    assert!(
        content["representations"]["revision_pack"].is_string(),
        "{content}"
    );
    // Authorization carries the gate/decision/evidence facts as their own
    // fields, so no frontend has to work out which is which.
    for field in ["evidence", "assessments", "reviews", "gates", "decisions"] {
        assert!(
            content["authorization"][field].is_array(),
            "authorization carries no '{field}': {content}"
        );
    }
    // Intent comes from the canonical definition, not from anything the
    // frontend could have reconstructed.
    assert_eq!(content["intent"]["intent"], json!("navigation"));
    assert_eq!(content["intent"]["amendment_count"], json!(0));
    assert!(content["intent"]["current_definition"]
        .as_str()
        .is_some_and(|digest| digest.starts_with("sha256:")));
    // Scope reports the declaration; nothing has been sealed, so there is no
    // resolution — which is a different answer from an empty one.
    assert!(!content["scope"]["declared"].as_array().unwrap().is_empty());
    // A revision is sealed, so the declaration has been resolved against an
    // exact Baseline and the resolution says which.
    assert!(
        content["scope"]["resolution"]["base_baseline"].is_string(),
        "{content}"
    );
    assert!(content["scope"]["resolved_for"].is_string(), "{content}");
    // Nothing has been promoted, so recovery is not required — established by
    // the restart table, never by the absence of a journal file.
    assert_eq!(content["recovery"]["posture"], json!("not_required"));
    assert!(content["recovery"]["promotions"]
        .as_array()
        .unwrap()
        .is_empty());
}

/// The project navigation is exactly §8.3's, nesting included.
///
/// Asserted against the daemon's own model rather than against
/// `navigation_for` directly: what matters is what a frontend receives, and a
/// test that called the function would pass even if the model stopped carrying
/// it.
#[test]
fn console_project_navigation_is_the_frozen_information_architecture() {
    let _env_lock = serialized();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let path = workspace.path().display().to_string();

    assert!(
        call(
            &store,
            &sessions,
            "init",
            "workspace.init",
            json!({ "path": path })
        )
        .ok
    );
    let registered = call(
        &store,
        &sessions,
        "register",
        "workspace.register",
        json!({ "path": path }),
    );
    let workspace_id = registered.result.unwrap()["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();

    let handshake = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "handshake",
            "console.handshake",
            serde_json::to_value(ConsoleHandshakeRequest {
                protocol: ConsoleProtocolVersion::default(),
                client_name: "test".into(),
                client_version: "test".into(),
                client_instance_id: "project-ia-test".into(),
                requested_capabilities: vec!["revisioned_models".into()],
            })
            .unwrap(),
        ),
    );
    let session = handshake.result.unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let snapshot = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "snapshot",
            "console.snapshot",
            serde_json::to_value(ConsoleModelRequest {
                application_session_id: session,
                subject: ConsoleSubject::Project { workspace_id },
            })
            .unwrap(),
        ),
    );
    assert!(snapshot.ok, "{:?}", snapshot.error);
    let model = snapshot.result.unwrap();

    // Work owns Tasks and ChangePacks. Flattening them to the top level would make
    // them look like peers of Baselines, which is the distinction §8.3 draws.
    assert_eq!(
        model["navigation"],
        json!([
            {"label": "Overview", "children": []},
            {"label": "Work", "children": ["Tasks", "Packs"]},
            {"label": "Resources", "children": ["Resources", "Observation"]},
            {"label": "Baselines", "children": ["Baselines", "Publications"]},
            {"label": "Activity", "children": []},
            {"label": "Providers", "children": []},
            {"label": "Extensions", "children": ["Extensions", "Tools"]},
        ])
    );

    // Every section has a content key with something under it, so no tab is a
    // label over an absent model.
    let content = &model["content"];
    for section in [
        "overview",
        "work",
        "resources",
        "baselines",
        "activity",
        "providers",
        "extensions",
    ] {
        assert!(
            !content[section].is_null(),
            "the project model carries nothing for '{section}': {content}"
        );
    }
    assert!(content["work"]["tasks"].is_array(), "{content}");
    assert!(content["work"]["packs"].is_array(), "{content}");
    assert!(content["resources"]["resources"].is_object(), "{content}");
    assert!(content["extensions"]["extensions"].is_array(), "{content}");

    // Providers reach the Console as the canonical catalog: mutable bindings
    // beside the immutable definitions and profiles they point at.
    for field in ["bindings", "definitions", "profiles"] {
        assert!(
            content["providers"][field].is_array(),
            "the provider catalog carries no '{field}': {content}"
        );
    }

    // Publication is rendered from within the Baselines section and is never
    // folded into a Baseline: a delivery that failed leaves the accepted
    // Baseline exactly as it was.
    assert!(content["baselines"]["publications"].is_array(), "{content}");
    assert!(
        content["baselines"].get("current").is_some(),
        "the model names the accepted Baseline: {content}"
    );

    // The consolidated pre-§8.3 sections are gone from the top level.
    // `Tools` and `Observation` still exist — nested where the ontology puts
    // them — so this checks placement, not mere presence.
    let top_level: Vec<&str> = model["navigation"]
        .as_array()
        .unwrap()
        .iter()
        .map(|section| section["label"].as_str().unwrap())
        .collect();
    for retired in ["Graph", "Authorization", "Events", "Tools", "Observation"] {
        assert!(
            !top_level.contains(&retired),
            "'{retired}' is still a top-level project section: {top_level:?}"
        );
    }
    let rendered = model.to_string();
    // retired-architecture-ok: naming the retired ontology is how this test
    // proves it never reaches a live Console model.
    for retired in ["pck_", "stable_head", "EditSession"] {
        assert!(
            !rendered.contains(retired),
            "the retired '{retired}' ontology reached a live Console model"
        );
    }
}

/// A Baseline is its own §8.3 scope, and it is read-only.
#[test]
fn console_baseline_scope_separates_the_three_roots_and_offers_no_mutation() {
    let _env_lock = serialized();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let path = workspace.path().display().to_string();

    assert!(
        call(
            &store,
            &sessions,
            "init",
            "workspace.init",
            json!({ "path": path })
        )
        .ok
    );
    let registered = call(
        &store,
        &sessions,
        "register",
        "workspace.register",
        json!({ "path": path }),
    );
    let workspace_id = registered.result.unwrap()["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();

    // The accepted Baseline, named by the authority rather than constructed.
    let baselines = call(
        &store,
        &sessions,
        "baselines",
        "dcg.baseline.list",
        json!({ "path": path }),
    );
    assert!(baselines.ok, "{:?}", baselines.error);
    let listed = baselines.result.unwrap();
    let baseline_id = listed[0]["baseline"].as_str().unwrap().to_string();

    let handshake = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "handshake",
            "console.handshake",
            serde_json::to_value(ConsoleHandshakeRequest {
                protocol: ConsoleProtocolVersion::default(),
                client_name: "test".into(),
                client_version: "test".into(),
                client_instance_id: "baseline-scope-test".into(),
                requested_capabilities: vec!["revisioned_models".into()],
            })
            .unwrap(),
        ),
    );
    let session = handshake.result.unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let snapshot = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "snapshot",
            "console.snapshot",
            serde_json::to_value(ConsoleModelRequest {
                application_session_id: session,
                subject: ConsoleSubject::Baseline {
                    workspace_id,
                    baseline_id: baseline_id.clone(),
                },
            })
            .unwrap(),
        ),
    );
    assert!(snapshot.ok, "{:?}", snapshot.error);
    let model = snapshot.result.unwrap();

    assert_eq!(
        model["navigation"],
        json!([
            {"label": "Summary", "children": []},
            {"label": "State root", "children": []},
            {"label": "Evidence root", "children": []},
            {"label": "Coverage", "children": []},
            {"label": "Lineage", "children": []},
            {"label": "Composition", "children": []},
            {"label": "Recoverability", "children": []},
            {"label": "Receipts", "children": []},
            {"label": "Publications", "children": []},
        ])
    );

    let baseline = &model["content"]["baseline"];
    assert_eq!(baseline["baseline"], json!(baseline_id));
    // Three roots, three separate answers. Collapsing any pair would lose the
    // distinction between what state is accepted, what establishes it, and
    // what justifies absence.
    let manifest = &baseline["manifest"];
    assert!(!manifest["project_state_root"].is_null(), "{manifest}");
    assert!(!manifest["state_evidence_root"].is_null(), "{manifest}");
    assert!(!manifest["coverage_evidence_root"].is_null(), "{manifest}");
    assert_ne!(
        manifest["project_state_root"],
        manifest["state_evidence_root"]
    );
    assert!(baseline["lineage"].is_array(), "{baseline}");
    assert!(
        baseline["recoverability"]["targets"].is_array(),
        "{baseline}"
    );
    assert!(
        baseline["recoverability"]["summary"].is_string(),
        "recoverability says what it means in words: {baseline}"
    );
    assert!(baseline["publications"].is_array(), "{baseline}");

    // A Baseline is accepted history. Nothing about it is editable, so the
    // scope offers no action at all — an action here would imply otherwise.
    assert_eq!(model["actions"], json!([]));
    assert_eq!(model["read_only"], json!(true));
}

/// A provider action survives mutations it never depended on.
///
/// The permissive direction of a freshness bug is dangerous and the
/// restrictive direction is merely infuriating, but both are wrong. A provider
/// offer reads the binding store and nothing else, so sealing a revision three
/// ChangePacks away must not invalidate it — that is exactly the "sibling ChangePack
/// moving" property the invalidation map exists to preserve.
#[test]
fn an_unrelated_mutation_does_not_stale_a_provider_action() {
    let _env_lock = serialized();
    let state = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _global_home = EnvVarGuard::set("DRAFT_GLOBAL_HOME", global.path().join(".draft"));
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let path = workspace.path().display().to_string();

    assert!(
        call(
            &store,
            &sessions,
            "init",
            "workspace.init",
            json!({ "path": path })
        )
        .ok
    );
    let registered = call(
        &store,
        &sessions,
        "register",
        "workspace.register",
        json!({ "path": path }),
    );
    let workspace_id = registered.result.unwrap()["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::write(workspace.path().join("app.txt"), "base\n").unwrap();
    assert!(
        call(
            &store,
            &sessions,
            "checkpoint",
            "checkpoint.create",
            json!({ "path": path, "message": "base" })
        )
        .ok
    );

    let handshake = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "handshake",
            "console.handshake",
            serde_json::to_value(ConsoleHandshakeRequest {
                protocol: ConsoleProtocolVersion::default(),
                client_name: "test".into(),
                client_version: "test".into(),
                client_instance_id: "provider-freshness".into(),
                requested_capabilities: vec![
                    "revisioned_models".into(),
                    "action_capabilities".into(),
                ],
            })
            .unwrap(),
        ),
    );
    let session = handshake.result.unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let snapshot = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "snapshot",
            "console.snapshot",
            serde_json::to_value(ConsoleModelRequest {
                application_session_id: session.clone(),
                subject: ConsoleSubject::Project { workspace_id },
            })
            .unwrap(),
        ),
    );
    assert!(snapshot.ok, "{:?}", snapshot.error);
    let model = snapshot.result.unwrap();
    let action = model["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["action_id"] == "project.provider.unbind")
        .expect("the Providers section offers unbind")
        .clone();
    let capability = action["invocation_capability"]
        .as_str()
        .unwrap()
        .to_string();
    let expected_revisions: CanonicalRevisions =
        serde_json::from_value(model["revisions"].clone()).unwrap();

    // A ChangePack lands, which moves the ChangePack store. The provider offer read
    // neither that store nor anything derived from it.
    let change = call(
        &store,
        &sessions,
        "change",
        "dcg.change_pack.open",
        json!({ "path": path, "intent": "unrelated", "scope": ["app.txt"] }),
    );
    assert!(change.ok, "{:?}", change.error);

    let response = draftd::dispatch(
        &store,
        &sessions,
        Request::new(
            "invoke-provider",
            "console.action.invoke",
            serde_json::to_value(ConsoleActionInvocation {
                application_session_id: session,
                invocation_capability: capability,
                operation_id: "op_provider_unbind".into(),
                expected_revisions,
                arguments: [("binding".to_string(), json!("pbd_000000000000"))]
                    .into_iter()
                    .collect(),
            })
            .unwrap(),
        )
        .with_operation_id("op_provider_unbind"),
    );

    // It fails — there is no such binding — but it must reach the operation to
    // find that out. A `STALE_CONSOLE_ACTION` here would mean the offer
    // declared a dependency it never had.
    let error = response.error.expect("no such binding exists");
    assert_ne!(
        error.code, "STALE_CONSOLE_ACTION",
        "an unrelated ChangePack invalidated a provider action: {:?}",
        error.message
    );
    assert!(
        error.message.contains("pbd_000000000000") || error.message.contains("binding"),
        "the refusal is about the binding, not about freshness: {:?}",
        error.message
    );
}
