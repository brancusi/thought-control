//! A running TUI's protocol server: a Unix socket, one thread per client feeding one queue,
//! and a discovery file so `thc ui` finds the session. The terminal loop drains the queue
//! between frames, so every input (the person's keys and every client's requests) is applied
//! in one order, the order the trace records.
//!
//! Socket: `$TMPDIR/thc-ui-<pid>.sock`, or `/tmp/thc-ui-<uid>/<pid>.sock` when `$TMPDIR` is too
//! long for a socket path. Discovery: `$TMPDIR/thc-ui/<pid>.json`. Both go when the TUI exits.

use crate::session::Session;
use crate::ui_proto::{self, Change, Control, Host, Subscription};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);

enum Input {
    Connect { client: u64, out: Sender<String> },
    Line { client: u64, line: String },
    Disconnect { client: u64 },
}

struct Client {
    out: Sender<String>,
    sub: Option<Subscription>,
}

pub struct Server {
    rx: Receiver<Input>,
    queue: VecDeque<Input>,
    clients: HashMap<u64, Client>,
    pub socket: PathBuf,
    discovery: Option<PathBuf>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
        if let Some(d) = &self.discovery {
            let _ = std::fs::remove_file(d);
        }
    }
}

/// A TUI killed outright leaves its socket and discovery file: remove those whose process is
/// gone (each named by its pid), so they don't pile up in the temp dir.
fn prune_dead() {
    let alive = |pid: i32| unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) };
    if let Ok(entries) = std::fs::read_dir(discovery_dir()) {
        for e in entries.flatten() {
            let p = e.path();
            let Some(pid) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<i32>().ok()) else { continue };
            if p.extension().is_some_and(|x| x == "json") && !alive(pid) {
                if let Some(sock) = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["socket"].as_str().map(PathBuf::from)) {
                    let _ = std::fs::remove_file(sock);
                }
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

/// Where discovery files live.
pub fn discovery_dir() -> PathBuf {
    std::env::temp_dir().join("thc-ui")
}

const MAX_SOCKET_PATH: usize = if cfg!(target_os = "linux") { 107 } else { 103 };

fn socket_path() -> Result<PathBuf, String> {
    let pid = std::process::id();
    let preferred = std::env::temp_dir().join(format!("thc-ui-{pid}.sock"));
    if preferred.as_os_str().len() <= MAX_SOCKET_PATH {
        return Ok(preferred);
    }
    let dir = PathBuf::from(format!("/tmp/thc-ui-{}", unsafe { libc::getuid() }));
    {
        use std::os::unix::fs::DirBuilderExt;
        let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
    }
    let path = dir.join(format!("{pid}.sock"));
    if path.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(format!("{}: too long for a socket path", path.display()));
    }
    Ok(path)
}

impl Server {
    /// Listen for this TUI and advertise it. `vault`: the vault's name and real path.
    pub fn start(vault: &str, vault_path: &Path) -> Result<Server, String> {
        prune_dead();
        let path = socket_path()?;
        if path.exists() {
            if UnixStream::connect(&path).is_ok() {
                return Err(format!("{}: another TUI is listening there", path.display()));
            }
            let _ = std::fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        // Gone at exit even when the Server isn't dropped (std::process::exit).
        thc_core::scratch::remove_at_exit(&path);
        let (tx, rx) = mpsc::channel::<Input>();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                // Only this user's processes (the socket is 0600, and the peer is checked).
                if !same_user(&stream) {
                    continue;
                }
                let Ok(write) = stream.try_clone() else { continue };
                spawn_client(stream, write, &tx);
            }
        });
        let mut s = Server { rx, queue: VecDeque::new(), clients: HashMap::new(), socket: path, discovery: None };
        let dir = discovery_dir();
        let _ = std::fs::create_dir_all(&dir);
        let pid = std::process::id();
        let file = dir.join(format!("{pid}.json"));
        let tty = unsafe {
            let p = libc::ttyname(libc::STDIN_FILENO);
            (!p.is_null()).then(|| std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
        };
        let info = json!({
            "pid": pid,
            "socket": s.socket,
            "vault": vault,
            "vault_path": vault_path,
            "tty": tty,
            "proto": ui_proto::PROTO,
            "version": env!("CARGO_PKG_VERSION"),
            "started_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
        });
        std::fs::write(&file, info.to_string() + "\n").map_err(|e| format!("{}: {e}", file.display()))?;
        thc_core::scratch::remove_at_exit(&file);
        s.discovery = Some(file);
        Ok(s)
    }

    /// The TUI switched vaults: say so in the discovery file.
    pub fn readvertise(&mut self, vault: &str, vault_path: &Path) {
        let Some(file) = &self.discovery else { return };
        let Some(mut v) = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { return };
        v["vault"] = json!(vault);
        v["vault_path"] = json!(vault_path);
        let _ = std::fs::write(file, v.to_string() + "\n");
    }

    /// Something to answer right now.
    pub fn ready(&mut self) -> bool {
        self.fill();
        !self.queue.is_empty()
    }

    fn fill(&mut self) {
        while let Ok(i) = self.rx.try_recv() {
            self.queue.push_back(i);
        }
    }

    /// Answer everything queued. Changes go to every subscriber.
    pub fn pump(&mut self, session: &mut Session) {
        self.fill();
        let host = Host { clock: true, effects: true };
        while let Some(input) = self.queue.pop_front() {
            match input {
                Input::Connect { client, out } => {
                    self.clients.insert(client, Client { out, sub: None });
                }
                Input::Disconnect { client } => {
                    self.clients.remove(&client);
                }
                Input::Line { client, line } => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    // The person's own changes since the last event go out first, in order.
                    session.sync_external();
                    self.announce(session, "terminal");
                    let handled = ui_proto::handle(session, &line, &host);
                    session.take_unannounced();
                    if let Some(c) = self.clients.get_mut(&client) {
                        match handled.control {
                            Some(Control::Subscribe(sub)) => c.sub = Some(sub),
                            Some(Control::Unsubscribe) => c.sub = None,
                            None => {}
                        }
                        let _ = c.out.send(handled.response);
                    }
                    if let Some(change) = &handled.change {
                        self.notify(session, change, "client");
                    }
                }
            }
        }
    }

    /// Send what changed outside a request (keys at the terminal, ticks, resizes) to
    /// subscribers.
    pub fn announce(&mut self, session: &mut Session, source: &str) {
        let msgs = session.take_unannounced();
        if msgs.is_empty() {
            return;
        }
        let change = Change { rev: session.rev, msgs, state_set: false };
        self.notify(session, &change, source);
    }

    fn notify(&mut self, session: &mut Session, change: &Change, source: &str) {
        for c in self.clients.values() {
            if let Some(sub) = &c.sub {
                let _ = c.out.send(ui_proto::event_line(session, change, sub, source));
            }
        }
    }
}

fn same_user(stream: &UnixStream) -> bool {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: getpeereid on a connected socket's fd, writing two plain integers.
    let r = unsafe { libc::getpeereid(std::os::unix::io::AsRawFd::as_raw_fd(stream), &mut uid, &mut gid) };
    r == 0 && uid == unsafe { libc::getuid() }
}

/// A writer thread per client: every line sent is written and flushed.
fn spawn_client(read: UnixStream, mut write: UnixStream, tx: &Sender<Input>) {
    let client = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
    let (out, lines) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        while let Ok(line) = lines.recv() {
            if write.write_all(line.as_bytes()).and_then(|_| write.write_all(b"\n")).and_then(|_| write.flush()).is_err() {
                break;
            }
        }
    });
    if tx.send(Input::Connect { client, out }).is_err() {
        return;
    }
    let tx = tx.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(read).lines() {
            let Ok(line) = line else { break };
            if tx.send(Input::Line { client, line }).is_err() {
                return;
            }
        }
        let _ = tx.send(Input::Disconnect { client });
    });
}

/// The running TUIs this machine advertises, newest first: (pid, the discovery JSON). Files
/// whose TUI is gone are removed.
pub fn sessions() -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir(discovery_dir()) else { return out };
    for e in dir.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(v) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { continue };
        let pid = v["pid"].as_i64().unwrap_or(0) as i32;
        // SAFETY: signal 0 only checks the process exists.
        let alive = pid > 0 && unsafe { libc::kill(pid, 0) } == 0;
        let answers = v["socket"].as_str().is_some_and(|s| UnixStream::connect(s).is_ok());
        if !alive || !answers {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        out.push(v);
    }
    out.sort_by_key(|v| std::cmp::Reverse(v["started_ms"].as_u64().unwrap_or(0)));
    out
}
