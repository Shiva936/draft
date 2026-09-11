//! Local-only IPC for `draftd`. Newline-delimited JSON over a Unix
//! domain socket (Linux/macOS); a localhost-loopback fallback is used on other
//! platforms. Blocking std sockets + a thread per connection — no async runtime.

pub mod console_application;
pub mod console_contracts;
pub mod protocol;

/// Contract-specific versions consumed by transport adapters that should not
/// depend on Draft domain crates directly.
pub mod contract_versions {
    use draft_core::contracts::{current_version, ContractId};

    pub const IPC_HANDSHAKE_REQUEST: u32 = current_version(ContractId::IpcHandshakeRequest);
    pub const CONSOLE_API_ENVELOPE: u32 = current_version(ContractId::ConsoleApiEnvelope);
    pub const CONSOLE_API_FAILURE: u32 = current_version(ContractId::ConsoleApiFailure);
    pub const CONSOLE_SESSION: u32 = current_version(ContractId::ConsoleSession);
    pub const CONSOLE_MUTATION_REQUEST: u32 = current_version(ContractId::ConsoleMutationRequest);
    pub const CONSOLE_JOBS_EVENT: u32 = current_version(ContractId::ConsoleJobsEvent);
    pub const CONSOLE_DAEMON_EVENT: u32 = current_version(ContractId::ConsoleDaemonEvent);
}

pub use protocol::{
    ErrorObject, HandshakeRequest, HandshakeResponse, ProgressEvent, Request, Response,
    IPC_CAPABILITIES, IPC_PROTOCOL,
};

use std::io;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
#[cfg(unix)]
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// A request handler: maps a [`Request`] to a [`Response`].
pub type Handler = Arc<dyn Fn(Request) -> Response + Send + Sync>;

/// The conventional socket path (`$XDG_RUNTIME_DIR/draft/draftd.sock`, falling
/// back to `~/.local/state/draft/draftd.sock`). On Unix, the XDG runtime
/// directory is used only when it satisfies the ownership and permissions
/// required by the XDG Base Directory specification.
pub fn socket_path() -> PathBuf {
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        if runtime_dir_is_usable(&runtime_dir) {
            return runtime_dir.join("draft").join("draftd.sock");
        }
    }
    fallback_socket_path()
}

fn fallback_socket_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/state/draft/draftd.sock")
}

#[cfg(unix)]
fn runtime_dir_is_usable(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let mode = metadata.mode() & 0o777;
    // SAFETY: `geteuid` has no arguments or caller-side safety requirements.
    let effective_uid = unsafe { libc::geteuid() };
    metadata.is_dir() && metadata.uid() == effective_uid && mode == 0o700
}

#[cfg(not(unix))]
fn runtime_dir_is_usable(path: &Path) -> bool {
    !path.as_os_str().is_empty()
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};

    pub fn serve(
        path: &std::path::Path,
        stop: Arc<AtomicBool>,
        handler: Handler,
    ) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            // Restrict the directory to the current user.
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
        // Remove a stale socket file from a previous run.
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        listener.set_nonblocking(true)?;

        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).ok();
                    let h = handler.clone();
                    let s = stop.clone();
                    std::thread::spawn(move || handle_conn(stream, h, s));
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
                Err(_) => break,
            }
        }
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    fn handle_conn(stream: UnixStream, handler: Handler, stop: Arc<AtomicBool>) {
        let mut writer = match stream.try_clone() {
            Ok(w) => w,
            Err(_) => return,
        };
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Request>(&line) {
                Ok(req) => {
                    let shutdown = req.method == "service.shutdown";
                    let resp = handler(req);
                    if shutdown {
                        stop.store(true, Ordering::Relaxed);
                    }
                    resp
                }
                Err(e) => Response::err(
                    "",
                    ErrorObject::new("VALIDATION_ERROR", format!("invalid request: {e}")),
                ),
            };
            let Ok(mut buf) = serde_json::to_string(&response) else {
                break;
            };
            buf.push('\n');
            if writer.write_all(buf.as_bytes()).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    }

    pub fn call(path: &std::path::Path, req: &Request) -> io::Result<Response> {
        let stream = UnixStream::connect(path)?;
        let mut writer = stream.try_clone()?;
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        writer.write_all(line.as_bytes())?;
        writer.flush()?;
        let mut reader = BufReader::new(stream);
        let mut resp_line = String::new();
        reader.read_line(&mut resp_line)?;
        let resp: Response = serde_json::from_str(resp_line.trim())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        resp.validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.message))?;
        Ok(resp)
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    fn addr() -> String {
        std::env::var("DRAFT_IPC_ADDR").unwrap_or_else(|_| "127.0.0.1:48357".to_string())
    }

    pub fn serve(
        _path: &std::path::Path,
        stop: Arc<AtomicBool>,
        handler: Handler,
    ) -> io::Result<()> {
        let listener = TcpListener::bind(addr())?;
        listener.set_nonblocking(true)?;
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let h = handler.clone();
                    let s = stop.clone();
                    std::thread::spawn(move || handle_conn(stream, h, s));
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(40));
                }
                Err(_) => break,
            }
        }
        Ok(())
    }

    fn handle_conn(stream: TcpStream, handler: Handler, stop: Arc<AtomicBool>) {
        let mut writer = match stream.try_clone() {
            Ok(w) => w,
            Err(_) => return,
        };
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Request>(&line) {
                Ok(req) => {
                    let shutdown = req.method == "service.shutdown";
                    let resp = handler(req);
                    if shutdown {
                        stop.store(true, Ordering::Relaxed);
                    }
                    resp
                }
                Err(e) => Response::err(
                    "",
                    ErrorObject::new("VALIDATION_ERROR", format!("invalid request: {e}")),
                ),
            };
            let Ok(mut buf) = serde_json::to_string(&response) else {
                break;
            };
            buf.push('\n');
            if writer.write_all(buf.as_bytes()).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    }

    pub fn call(_path: &std::path::Path, req: &Request) -> io::Result<Response> {
        let stream = TcpStream::connect(addr())?;
        let mut writer = stream.try_clone()?;
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        writer.write_all(line.as_bytes())?;
        writer.flush()?;
        let mut reader = BufReader::new(stream);
        let mut resp_line = String::new();
        reader.read_line(&mut resp_line)?;
        let resp: Response = serde_json::from_str(resp_line.trim())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        resp.validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.message))?;
        Ok(resp)
    }
}

/// Serve requests until `stop` is set (e.g. by a `service.shutdown` request).
pub fn serve(path: &std::path::Path, stop: Arc<AtomicBool>, handler: Handler) -> io::Result<()> {
    imp::serve(path, stop, handler)
}

/// Send a single request and await the response.
pub fn call(path: &std::path::Path, req: &Request) -> io::Result<Response> {
    imp::call(path, req)
}

/// Returns true if a daemon answers a ping on the default socket.
pub fn is_running(path: &std::path::Path) -> bool {
    matches!(
        call(path, &Request::new("ping", "service.ping", serde_json::Value::Null)),
        Ok(r) if r.ok
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            EnvVarGuard { key, previous }
        }

        fn unset(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            std::env::remove_var(key);
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
    fn socket_path_uses_only_valid_xdg_runtime_directories() {
        let _env_lock = env_lock().lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let _home = EnvVarGuard::set("HOME", &home);
        let fallback = home.join(".local/state/draft/draftd.sock");

        let _runtime = EnvVarGuard::unset("XDG_RUNTIME_DIR");
        assert_eq!(socket_path(), fallback);

        std::env::set_var("XDG_RUNTIME_DIR", "");
        assert_eq!(socket_path(), fallback);

        std::env::set_var("XDG_RUNTIME_DIR", root.path().join("missing"));
        assert_eq!(socket_path(), fallback);

        let insecure = root.path().join("insecure");
        std::fs::create_dir(&insecure).unwrap();
        std::fs::set_permissions(&insecure, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", &insecure);
        assert_eq!(socket_path(), fallback);

        let valid = root.path().join("runtime");
        std::fs::create_dir(&valid).unwrap();
        std::fs::set_permissions(&valid, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", &valid);
        assert_eq!(socket_path(), valid.join("draft/draftd.sock"));
    }
}
