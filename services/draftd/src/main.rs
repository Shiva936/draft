//! `draftd` — the local Draft daemon (FR-SVC-002).
//!
//! It is a thin coordination shell over `core::App`: it owns no product logic
//! (Blueprint §28). It serves local IPC, maintains a workspace registry, and
//! manages locks/sessions. Every request is dispatched to the same Draft-native
//! `core::App` the CLI uses in embedded mode.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use serde_json::Value;

use draft_ipc::Request;
use draft_sessions::SessionManager;
use draft_store::ServiceStore;

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
        Arc::new(move |req: Request| draftd::dispatch(&handler_store, &handler_sessions, req));

    let sock = draft_ipc::socket_path();
    let result = draft_ipc::serve(&sock, stop, handler);

    let _ = std::fs::remove_file(pid_path());
    store.log("draftd stopped");
    result
}
