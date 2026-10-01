//! Local IPC over Unix domain sockets, JSON lines. Nothing listens on TCP.
//!
//! Two kinds of socket live in the private runtime dir:
//! * one per Claude session, receiving hook and status-line events,
//!   authenticated by peer uid *and* a random per-session token;
//! * one control socket for `promptly-ctl new-session|notify|...`,
//!   authenticated by peer uid.

use crate::util::ct_eq;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Max bytes in one message. Hook payloads with large tool inputs fit
/// comfortably; anything larger is dropped rather than buffered.
const MAX_MESSAGE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Envelope {
    /// A Claude Code hook payload, forwarded verbatim.
    Hook { token: String, payload: Value },
    /// Claude Code status line input (cost, model, context).
    Statusline { token: String, payload: Value },
    /// Control requests from `promptly-ctl`.
    Control { request: ControlRequest },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum ControlRequest {
    NewSession {
        #[serde(default)]
        cwd: Option<PathBuf>,
        #[serde(default)]
        claude: bool,
        #[serde(default)]
        worktree: Option<String>,
        #[serde(default)]
        prompt: Option<String>,
    },
    Notify {
        title: String,
        #[serde(default)]
        body: String,
    },
    ListSessions,
    /// Run a command-palette action by name, e.g. `usage`, `toggle_review`.
    Action {
        name: String,
    },
    Focus {
        session: String,
    },
}

#[derive(Debug, Clone)]
pub enum Incoming {
    Hook(Value),
    Statusline(Value),
    Control(ControlRequest, Responder),
}

/// Lets the app answer a control request on the same connection.
#[derive(Debug, Clone)]
pub struct Responder(Arc<parking::Slot>);

mod parking {
    use std::sync::{Condvar, Mutex};
    #[derive(Debug, Default)]
    pub struct Slot {
        pub value: Mutex<Option<serde_json::Value>>,
        pub cv: Condvar,
    }
}

impl Responder {
    pub fn reply(&self, v: Value) {
        *self.0.value.lock().unwrap() = Some(v);
        self.0.cv.notify_all();
    }
}

/// Owns a listening socket; removes the socket file when dropped.
pub struct Server {
    path: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblock accept().
        let _ = UnixStream::connect(&self.path);
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Server {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bind `path` and deliver authenticated messages to `sink`. When
    /// `token` is set, hook/statusline messages must carry it. Control
    /// messages are only accepted when `token` is `None`.
    pub fn spawn(
        path: PathBuf,
        token: Option<String>,
        sink: impl Fn(Incoming) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let sink = Arc::new(sink);
        let token = Arc::new(token);
        std::thread::Builder::new()
            .name(format!("ipc {}", path.display()))
            .spawn(move || {
                for conn in listener.incoming() {
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(conn) = conn else { continue };
                    if peer_uid(&conn).ok() != Some(crate::paths::current_uid()) {
                        continue;
                    }
                    let sink = sink.clone();
                    let token = token.clone();
                    std::thread::spawn(move || {
                        let _ = handle_conn(conn, token.as_deref(), &*sink);
                    });
                }
            })?;
        Ok(Self { path, stop })
    }
}

fn handle_conn(conn: UnixStream, token: Option<&str>, sink: &dyn Fn(Incoming)) -> io::Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut writer = conn.try_clone()?;
    let mut reader = BufReader::new(conn).take(MAX_MESSAGE as u64);
    let mut line = String::new();
    while reader.read_line(&mut line)? > 0 {
        let msg: Result<Envelope, _> = serde_json::from_str(line.trim_end());
        line.clear();
        let Ok(msg) = msg else { continue };
        match (msg, token) {
            (Envelope::Hook { token: t, payload }, Some(want))
                if ct_eq(t.as_bytes(), want.as_bytes()) =>
            {
                sink(Incoming::Hook(payload));
                break;
            }
            (Envelope::Statusline { token: t, payload }, Some(want))
                if ct_eq(t.as_bytes(), want.as_bytes()) =>
            {
                sink(Incoming::Statusline(payload));
                break;
            }
            (Envelope::Control { request }, None) => {
                let slot = Arc::new(parking::Slot::default());
                sink(Incoming::Control(request, Responder(slot.clone())));
                let guard = slot.value.lock().unwrap();
                let (mut guard, _) = slot
                    .cv
                    .wait_timeout_while(guard, Duration::from_secs(5), |v| v.is_none())
                    .unwrap();
                let reply = guard
                    .take()
                    .unwrap_or(serde_json::json!({"ok": false, "error": "timeout"}));
                writeln!(writer, "{reply}")?;
            }
            _ => break, // wrong token or wrong socket: drop the connection
        }
    }
    Ok(())
}

/// Send used by hooks. The client keeps its end open until the server has
/// checked peer credentials and closed the connection (on macOS the peer's
/// credentials are unavailable once it has gone away). Bounded by `timeout`
/// end to end, so a hung app can never stall Claude Code.
pub fn send_oneshot(path: &Path, msg: &Envelope, timeout: Duration) -> io::Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    let mut s = UnixStream::connect(path)?;
    s.set_write_timeout(Some(timeout))?;
    let mut body = serde_json::to_vec(msg)?;
    body.push(b'\n');
    s.write_all(&body)?;
    let left = deadline.saturating_duration_since(std::time::Instant::now());
    s.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
    let mut sink = [0u8; 16];
    // EOF means delivered; a timeout means the app is slow and we give up.
    let _ = s.read(&mut sink);
    Ok(())
}

/// Request/response used by `promptly-ctl` control commands.
pub fn request(path: &Path, req: ControlRequest) -> io::Result<Value> {
    let mut s = UnixStream::connect(path)?;
    s.set_read_timeout(Some(Duration::from_secs(6)))?;
    let mut body = serde_json::to_vec(&Envelope::Control { request: req })?;
    body.push(b'\n');
    s.write_all(&body)?;
    s.shutdown(std::net::Shutdown::Write)?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line)?;
    serde_json::from_str(&line).map_err(io::Error::other)
}

pub fn control_socket_path() -> io::Result<PathBuf> {
    Ok(crate::paths::runtime_dir()?.join("control.sock"))
}

/// Uid of the process on the other end (SO_PEERCRED / LOCAL_PEERCRED).
pub fn peer_uid(s: &UnixStream) -> io::Result<u32> {
    use std::os::fd::AsRawFd;
    let fd = s.as_raw_fd();
    #[cfg(target_os = "linux")]
    {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: cred and len are valid for writes of the declared size.
        let r = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut _,
                &mut len,
            )
        };
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(cred.uid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: getpeereid writes to the two out-params only.
        let r = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(uid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn token_is_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let _srv = Server::spawn(path.clone(), Some("secret".into()), move |m| {
            tx.lock().unwrap().send(m).unwrap();
        })
        .unwrap();
        let bad = Envelope::Hook {
            token: "nope".into(),
            payload: serde_json::json!({"a": 1}),
        };
        send_oneshot(&path, &bad, Duration::from_millis(50)).unwrap();
        let good = Envelope::Hook {
            token: "secret".into(),
            payload: serde_json::json!({"a": 2}),
        };
        send_oneshot(&path, &good, Duration::from_millis(50)).unwrap();
        match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            Incoming::Hook(v) => assert_eq!(v["a"], 2),
            other => panic!("unexpected {other:?}"),
        }
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn control_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.sock");
        let _srv = Server::spawn(path.clone(), None, |m| {
            if let Incoming::Control(ControlRequest::ListSessions, r) = m {
                r.reply(serde_json::json!({"ok": true, "sessions": []}));
            }
        })
        .unwrap();
        let v = request(&path, ControlRequest::ListSessions).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn peer_uid_is_ours() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), crate::paths::current_uid());
    }
}
