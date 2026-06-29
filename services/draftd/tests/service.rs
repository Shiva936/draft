//! Service tests (TEST-004): daemon start/stop/status, IPC round-trip,
//! workspace registry, and graceful shutdown.

#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::{Child, Command};
#[cfg(unix)]
use std::time::{Duration, Instant};

use draft_ipc::socket_path;
#[cfg(unix)]
use draft_ipc::{call, is_running, Request};
#[cfg(unix)]
use serde_json::Value;

#[cfg(unix)]
use draft_core::vcs::types::ProviderId;

/// Spawn draftd with an isolated runtime dir; returns the child + socket path.
#[cfg(unix)]
fn spawn_daemon(runtime: &Path, home: &Path) -> (Child, std::path::PathBuf) {
    let exe = env!("CARGO_BIN_EXE_draftd");
    let child = Command::new(exe)
        .arg("start")
        .env("XDG_RUNTIME_DIR", runtime)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("HOME", home)
        .spawn()
        .expect("spawn draftd");
    // The socket path is derived from XDG_RUNTIME_DIR.
    let sock = runtime.join("draft").join("draftd.sock");
    let start = Instant::now();
    while !sock.exists() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    (child, sock)
}

#[test]
#[cfg(unix)]
fn daemon_start_ipc_roundtrip_and_shutdown() {
    let rt = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    // Avoid colliding with a real user daemon.
    std::env::set_var("XDG_RUNTIME_DIR", rt.path());

    let (mut child, sock) = spawn_daemon(rt.path(), home.path());
    assert!(sock.exists(), "daemon did not create its socket");

    // ping
    assert!(is_running(&sock), "daemon should answer ping");

    // service.status returns structured info
    let resp = call(&sock, &Request::new("1", "service.status", Value::Null)).unwrap();
    assert!(resp.ok);
    let result = resp.result.unwrap();
    assert_eq!(result["running"], true);
    assert_eq!(result["version"], draft_core_version());

    // provider.list over IPC
    let resp = call(&sock, &Request::new("2", "provider.list", Value::Null)).unwrap();
    assert!(resp.ok);
    let arr = resp.result.unwrap();
    assert!(arr.as_array().unwrap().iter().any(|p| p["id"] == "git"));

    // unknown method => structured error
    let resp = call(&sock, &Request::new("3", "nope.method", Value::Null)).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.error.unwrap().code, "UNKNOWN_METHOD");

    // path traversal rejected
    let resp = call(
        &sock,
        &Request::new(
            "4",
            "workspace.status",
            serde_json::json!({"path": "../etc"}),
        ),
    )
    .unwrap();
    assert!(!resp.ok);

    // workspace registration updates service status.
    let workspace = tempfile::tempdir().unwrap();
    draft_core::workspace::initialize(
        workspace.path(),
        workspace.path(),
        ProviderId::new("git"),
        false,
    )
    .unwrap();
    let resp = call(
        &sock,
        &Request::new(
            "5",
            "workspace.register",
            serde_json::json!({"path": workspace.path().display().to_string()}),
        ),
    )
    .unwrap();
    assert!(resp.ok);
    let resp = call(&sock, &Request::new("6", "service.status", Value::Null)).unwrap();
    assert!(resp.ok);
    assert_eq!(resp.result.unwrap()["workspaces"], 1);

    // shutdown
    let _ = call(&sock, &Request::new("7", "service.shutdown", Value::Null));
    let start = Instant::now();
    while sock.exists() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!is_running(&sock), "daemon should be stopped");
    let _ = child.kill();
    let _ = child.wait();
}

fn draft_core_version() -> &'static str {
    // Keep in sync with the crate version embedded by draftd.
    "0.2.0"
}

#[test]
fn socket_path_is_local() {
    std::env::remove_var("XDG_RUNTIME_DIR");
    let p = socket_path();
    assert!(p.to_string_lossy().ends_with("draftd.sock"));
}
