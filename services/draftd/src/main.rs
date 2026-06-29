//! `draftd` — the local Draft daemon (FR-SVC-002).
//!
//! It is a thin coordination shell over `core::App`: it owns no product logic
//! (Blueprint §28). It serves local IPC, maintains a workspace registry, and
//! manages locks/sessions. Every request is dispatched to the same provider-
//! neutral `core::App` the CLI uses in embedded mode.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::{json, Value};

use draft_core::app::App;
use draft_core::error::DraftResult;
use draft_ipc::{ErrorObject, Request, Response};
use draft_sessions::SessionManager;
use draft_store::{ServiceStore, WorkspaceRecord};

#[derive(Parser)]
#[command(name = "draftd", version = draft_core::DRAFT_VERSION, about = "Draft local daemon")]
struct Cli {
    #[command(subcommand)]
    command: Option<DaemonCmd>,
    /// Spawn a detached daemon and exit.
    #[arg(long, global = true)]
    detach: bool,
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Run the daemon in the foreground (default).
    Start,
    /// Ask a running daemon to stop.
    Stop,
    /// Report daemon status.
    Status,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();

    if cli.detach {
        return match spawn_detached() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("failed to start draftd: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }

    match cli.command.unwrap_or(DaemonCmd::Start) {
        DaemonCmd::Start => match serve() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("draftd error: {e}");
                std::process::ExitCode::FAILURE
            }
        },
        DaemonCmd::Stop => {
            let sock = draft_ipc::socket_path();
            match draft_ipc::call(
                &sock,
                &Request::new("stop", "service.shutdown", Value::Null),
            ) {
                Ok(_) => {
                    println!("draftd stopped");
                    std::process::ExitCode::SUCCESS
                }
                Err(_) => {
                    println!("draftd is not running");
                    std::process::ExitCode::SUCCESS
                }
            }
        }
        DaemonCmd::Status => {
            let running = draft_ipc::is_running(&draft_ipc::socket_path());
            println!("draftd: {}", if running { "running" } else { "stopped" });
            std::process::ExitCode::SUCCESS
        }
    }
}

fn spawn_detached() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg("start")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

fn pid_path() -> PathBuf {
    draft_store::state_dir().join("draftd.pid")
}

fn serve() -> std::io::Result<()> {
    let store = Arc::new(ServiceStore::open_default());
    let sessions = Arc::new(SessionManager::new());
    let stop = Arc::new(AtomicBool::new(false));

    // Write PID file (best-effort).
    let _ = std::fs::create_dir_all(draft_store::state_dir());
    let _ = std::fs::write(pid_path(), std::process::id().to_string());
    store.log("draftd started");

    let handler_store = store.clone();
    let handler_sessions = sessions.clone();
    let handler: draft_ipc::Handler =
        Arc::new(move |req: Request| dispatch(&handler_store, &handler_sessions, req));

    let sock = draft_ipc::socket_path();
    let result = draft_ipc::serve(&sock, stop, handler);

    let _ = std::fs::remove_file(pid_path());
    store.log("draftd stopped");
    result
}

fn dispatch(store: &ServiceStore, sessions: &SessionManager, req: Request) -> Response {
    let app = App::new(draft_providers::default_registry());
    let id = req.id.clone();
    match req.method.as_str() {
        "service.ping" => Response::ok(id, json!({ "pong": true })),
        "service.shutdown" => Response::ok(id, json!({ "stopping": true })),
        "service.status" => Response::ok(
            id,
            json!({
                "running": true,
                "version": draft_core::DRAFT_VERSION,
                "sessions": sessions.count(),
                "workspaces": store.list().len(),
            }),
        ),
        "provider.list" => to_response(id, Ok(app.providers())),
        "workspace.detect" => with_path(&req, |p| app.detect(p)).into_response(id),
        "workspace.status" => with_path(&req, |p| app.status(p)).into_response(id),
        "receipt.list" => with_path(&req, |p| app.receipt_list(p)).into_response(id),
        "workspace.register" => {
            let path = match req.params.get("path").and_then(|v| v.as_str()) {
                Some(p) => p.to_string(),
                None => {
                    return Response::err(id, ErrorObject::new("INVALID_PARAMS", "missing 'path'"))
                }
            };
            match draft_core::workspace::Workspace::open(std::path::Path::new(&path)) {
                Ok(ws) => {
                    let rec = WorkspaceRecord {
                        id: ws.id.to_string(),
                        path: ws.root.display().to_string(),
                        provider_id: ws.provider_id.to_string(),
                        registered_at: chrono::Utc::now(),
                    };
                    let _ = store.register(rec);
                    Response::ok(
                        id,
                        json!({ "registered": true, "workspace_id": ws.id.to_string() }),
                    )
                }
                Err(e) => Response::err(id, draft_err(&e)),
            }
        }
        other => Response::err(
            id,
            ErrorObject::new("UNKNOWN_METHOD", format!("unknown method: {other}")),
        ),
    }
}

/// Validate and extract a path param, rejecting traversal (NFR §8.6).
fn with_path<T, F>(req: &Request, f: F) -> DraftResult<T>
where
    F: FnOnce(&std::path::Path) -> DraftResult<T>,
{
    let path = req
        .params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            draft_core::error::DraftError::new(
                draft_core::error::DraftErrorKind::IpcError,
                "missing 'path' parameter",
            )
        })?;
    if path.contains("..") {
        return Err(draft_core::error::DraftError::new(
            draft_core::error::DraftErrorKind::IpcError,
            "path traversal is not allowed",
        ));
    }
    f(std::path::Path::new(path))
}

fn to_response<T: Serialize>(id: String, r: DraftResult<T>) -> Response {
    match r {
        Ok(v) => Response::ok(id, serde_json::to_value(v).unwrap_or(Value::Null)),
        Err(e) => Response::err(id, draft_err(&e)),
    }
}

fn draft_err(e: &draft_core::error::DraftError) -> ErrorObject {
    let mut obj = ErrorObject::new(e.code(), e.message.clone());
    obj.details = json!({ "context": e.context, "suggestion": e.suggestion });
    obj
}

/// Extension so `DraftResult<T>` can be turned into a `Response` ergonomically.
trait IntoResponse<T> {
    fn into_response(self, id: String) -> Response;
}
impl<T: Serialize> IntoResponse<T> for DraftResult<T> {
    fn into_response(self, id: String) -> Response {
        to_response(id, self)
    }
}
