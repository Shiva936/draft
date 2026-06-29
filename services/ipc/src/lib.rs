//! Local-only IPC for `draftd` (TDD §8.3). Newline-delimited JSON over a Unix
//! domain socket (Linux/macOS); a localhost-loopback fallback is used on other
//! platforms. Blocking std sockets + a thread per connection — no async runtime.

pub mod protocol;

pub use protocol::{ErrorObject, Request, Response};

use std::io;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(unix)]
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// A request handler: maps a [`Request`] to a [`Response`].
pub type Handler = Arc<dyn Fn(Request) -> Response + Send + Sync>;

/// The conventional socket path (`$XDG_RUNTIME_DIR/draft/draftd.sock`, falling
/// back to `~/.local/state/draft/draftd.sock`).
pub fn socket_path() -> PathBuf {
    if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
        if !rt.is_empty() {
            return PathBuf::from(rt).join("draft").join("draftd.sock");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local/state/draft/draftd.sock")
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
                    ErrorObject::new("IPC_ERROR", format!("invalid request: {e}")),
                ),
            };
            let mut buf = serde_json::to_string(&response).unwrap_or_default();
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
        Ok(resp)
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;
    // Minimal localhost-loopback fallback marker for non-unix platforms.
    pub fn serve(
        _path: &std::path::Path,
        _stop: Arc<AtomicBool>,
        _handler: Handler,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "IPC transport not yet implemented on this platform",
        ))
    }
    pub fn call(_path: &std::path::Path, _req: &Request) -> io::Result<Response> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "IPC unavailable",
        ))
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
