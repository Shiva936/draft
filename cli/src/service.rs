//! `draft daemon` subcommands + service-aware routing helpers.
//!
//! The daemon (`draftd`) is optional (NFR-006). Safe commands always fall back
//! to embedded mode when it is not running (FR-CLI-003).

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use draft_core::support::error::DraftError;
use draft_ipc::{
    call, is_running, socket_path, HandshakeRequest, HandshakeResponse, Request, IPC_PROTOCOL,
};

use crate::{output, ServiceAction};

/// True if a daemon is answering on the local socket.
pub fn daemon_running() -> bool {
    is_running(&socket_path())
}

/// Try to satisfy a request via the daemon, returning `None` to fall back to
/// embedded mode. `params` is the JSON params object.
pub fn handle(action: ServiceAction, cwd: &Path) -> Result<(), DraftError> {
    match action {
        ServiceAction::Start => {
            let already_running = daemon_running();
            if already_running {
                output::warn("draftd is already running.");
            } else {
                match spawn_daemon() {
                    Ok(()) => {
                        if wait_for_daemon(Duration::from_secs(5)) {
                            output::success("Started draftd.");
                        } else {
                            output::warn("Started draftd, but it did not answer before timeout.");
                        }
                    }
                    Err(e) => output::warn(&format!(
                        "Could not start draftd ({e}); Draft runs in embedded mode."
                    )),
                }
            }
            if daemon_running() {
                register_workspace(cwd);
            }
            Ok(())
        }
        ServiceAction::Stop => {
            if !daemon_running() {
                output::warn("draftd is not running.");
                return Ok(());
            }
            let _ = call(
                &socket_path(),
                &mutation_request("service.shutdown", serde_json::Value::Null),
            );
            output::success("Requested draftd shutdown.");
            Ok(())
        }
        ServiceAction::Restart => {
            if daemon_running() {
                let _ = call(
                    &socket_path(),
                    &mutation_request("service.shutdown", serde_json::Value::Null),
                );
                let deadline = Instant::now() + Duration::from_secs(5);
                while daemon_running() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            ensure_daemon()?;
            register_workspace(cwd);
            output::success("Restarted draftd.");
            Ok(())
        }
        ServiceAction::Status { json } => {
            let running = daemon_running();
            let daemon = if running {
                call(
                    &socket_path(),
                    &Request::new("cli", "service.status", serde_json::Value::Null),
                )
                .ok()
                .and_then(|r| if r.ok { r.result } else { None })
            } else {
                None
            };
            let workspaces = daemon
                .as_ref()
                .and_then(|v| v.get("workspaces"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let report = serde_json::json!({
                "running": running,
                "mode": if running { "service" } else { "embedded" },
                "socket": socket_path().display().to_string(),
                "workspaces": workspaces,
                "daemon": daemon,
            });
            if json {
                output::print_json(&report);
            } else {
                output::header("Service");
                output::field("Running", if running { "yes" } else { "no" });
                output::field("Mode", if running { "service" } else { "embedded" });
                output::field("Socket", &socket_path().display().to_string());
                if running {
                    output::field("Workspaces", &workspaces.to_string());
                }
            }
            Ok(())
        }
    }
}

/// Ensure the daemon is available for a Console session.
pub fn ensure_daemon() -> Result<(), DraftError> {
    if daemon_running() {
        if daemon_console_compatible() {
            return Ok(());
        }
        let _ = call(
            &socket_path(),
            &mutation_request("service.shutdown", serde_json::Value::Null),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while daemon_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if daemon_running() {
            return Err(DraftError::new(
                draft_core::support::error::DraftErrorKind::ServiceUnavailable,
                "the running draftd is incompatible with Console and could not be replaced",
            ));
        }
    }
    spawn_daemon().map_err(|error| {
        DraftError::new(
            draft_core::support::error::DraftErrorKind::ServiceUnavailable,
            format!("could not start draftd: {error}"),
        )
    })?;
    if wait_for_daemon(Duration::from_secs(5)) && daemon_console_compatible() {
        Ok(())
    } else {
        Err(DraftError::new(
            draft_core::support::error::DraftErrorKind::ServiceUnavailable,
            "draftd did not become ready before timeout",
        ))
    }
}

fn daemon_console_compatible() -> bool {
    let request = Request::new(
        "console-handshake",
        "service.handshake",
        serde_json::to_value(HandshakeRequest {
            protocol: IPC_PROTOCOL.into(),
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::IpcHandshakeRequest,
            ),
            requested_capabilities: vec!["console_http".into()],
            client_name: "draft-console".into(),
            client_version: draft_core::DRAFT_VERSION.into(),
        })
        .unwrap_or(serde_json::Value::Null),
    );
    call(&socket_path(), &request)
        .ok()
        .filter(|response| {
            response.ok
                && response.protocol == IPC_PROTOCOL
                && draft_core::contracts::supports_version(
                    draft_core::contracts::ContractId::IpcResponse,
                    response.schema_version,
                )
        })
        .and_then(|response| response.result)
        .and_then(|value| serde_json::from_value::<HandshakeResponse>(value).ok())
        .is_some_and(|handshake| {
            handshake.protocol == IPC_PROTOCOL
                && draft_core::contracts::supports_version(
                    draft_core::contracts::ContractId::IpcHandshakeResponse,
                    handshake.schema_version,
                )
                && handshake
                    .capabilities
                    .iter()
                    .any(|capability| capability == "console_http")
        })
}

fn wait_for_daemon(timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if daemon_running() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn register_workspace(path: &Path) {
    let _ = call(
        &socket_path(),
        &mutation_request(
            "workspace.register",
            serde_json::json!({ "path": path.display().to_string() }),
        ),
    );
}

fn mutation_request(method: &str, params: serde_json::Value) -> Request {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let operation_id = format!(
        "op_cli_{}_{}_{}",
        std::process::id(),
        nonce,
        method.replace('.', "_")
    );
    Request::new(format!("req_{operation_id}"), method, params).with_operation_id(operation_id)
}

/// Spawn `draftd --detach`, preferring the binary shipped next to this CLI so
/// Console cannot accidentally pair with an older installation from PATH.
fn spawn_daemon() -> std::io::Result<()> {
    let sibling = std::env::current_exe()?
        .parent()
        .map(|d| {
            d.join(if cfg!(windows) {
                "draftd.exe"
            } else {
                "draftd"
            })
        })
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no sibling dir"))?;
    if sibling.is_file()
        && std::process::Command::new(&sibling)
            .arg("--detach")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    {
        return Ok(());
    }
    std::process::Command::new("draftd")
        .arg("--detach")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}
