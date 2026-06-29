//! `draft service` subcommands + service-aware routing helpers.
//!
//! The daemon (`draftd`) is optional (NFR-006). Safe commands always fall back
//! to embedded mode when it is not running (FR-CLI-003).

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use draft_core::error::DraftError;
use draft_ipc::{call, is_running, socket_path, Request};

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
                &Request::new("cli", "service.shutdown", serde_json::Value::Null),
            );
            output::success("Requested draftd shutdown.");
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
        &Request::new(
            "cli",
            "workspace.register",
            serde_json::json!({ "path": path.display().to_string() }),
        ),
    );
}

/// Spawn `draftd --detach`, trying PATH first then a binary next to `draft`.
fn spawn_daemon() -> std::io::Result<()> {
    if std::process::Command::new("draftd")
        .arg("--detach")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    // Fall back to a sibling binary (cargo target dir layout).
    let sibling = std::env::current_exe()?
        .parent()
        .map(|d| d.join("draftd"))
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no sibling dir"))?;
    std::process::Command::new(sibling)
        .arg("--detach")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}
