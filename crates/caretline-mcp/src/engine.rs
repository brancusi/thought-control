//! A connection to one caretline engine: a live editor over its Unix socket, or a headless
//! engine in process. Both answer the same JSON requests (docs/caretline/protocol.md) and
//! feed the same event buffer, so the tools above never care which one they have.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use caretline::protocol::Subscription;
use caretline::{Effect, Msg, Session};
use serde_json::{json, Value};

/// A protocol error: the engine's `kind` (`stale`, `bad_keys`, …) and its message.
#[derive(Debug, Clone)]
pub struct ProtoError {
    pub kind: String,
    pub message: String,
}

impl ProtoError {
    pub fn new(kind: &str, message: impl Into<String>) -> ProtoError {
        ProtoError { kind: kind.into(), message: message.into() }
    }
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

/// How many events a session keeps for `watch`.
const EVENT_LIMIT: usize = 20_000;

/// The events an engine reported, newest last, with a condition variable to wait on.
#[derive(Default)]
pub struct Events {
    inner: Mutex<EventLog>,
    cv: Condvar,
}

#[derive(Default)]
pub struct EventLog {
    pub events: VecDeque<Value>,
    /// Events dropped from the front (to stay under the limit).
    pub dropped_before: Option<u64>,
    /// The editor went away (it quit, or the socket closed).
    pub closed: bool,
}

impl Events {
    pub fn push(&self, event: Value) {
        let mut log = self.inner.lock().unwrap();
        log.events.push_back(event);
        while log.events.len() > EVENT_LIMIT {
            if let Some(old) = log.events.pop_front() {
                log.dropped_before = old["rev"].as_u64();
            }
        }
        self.cv.notify_all();
    }

    pub fn close(&self) {
        self.inner.lock().unwrap().closed = true;
        self.cv.notify_all();
    }

    /// Waits until `done` says the log is complete enough, or until `deadline`.
    pub fn wait_until(&self, deadline: Instant, mut done: impl FnMut(&EventLog) -> bool) -> std::sync::MutexGuard<'_, EventLog> {
        let mut log = self.inner.lock().unwrap();
        loop {
            if done(&log) || log.closed {
                return log;
            }
            let now = Instant::now();
            if now >= deadline {
                return log;
            }
            log = self.cv.wait_timeout(log, deadline - now).unwrap().0;
        }
    }

    pub fn snapshot(&self) -> std::sync::MutexGuard<'_, EventLog> {
        self.inner.lock().unwrap()
    }
}

/// A live editor's discovery file, `$TMPDIR/caretline/<pid>.json`.
#[derive(Debug, Clone)]
pub struct Editor {
    pub pid: u32,
    pub socket: PathBuf,
    pub file: Option<String>,
    pub started_ms: u64,
    pub alive: bool,
}

impl Editor {
    pub fn json(&self) -> Value {
        json!({
            "pid": self.pid,
            "socket": self.socket,
            "file": self.file,
            "started_ms": self.started_ms,
            "alive": self.alive,
        })
    }
}

/// Where listening editors advertise themselves (the same rule as `caretline send`).
pub fn discovery_dir() -> PathBuf {
    std::env::temp_dir().join("caretline")
}

/// Every advertised editor, newest first. `alive` is whether its socket answers.
pub fn list_editors() -> Vec<Editor> {
    let Ok(dir) = std::fs::read_dir(discovery_dir()) else { return Vec::new() };
    let mut out: Vec<Editor> = dir
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let info: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            let socket = PathBuf::from(info["socket"].as_str()?);
            let alive = owned_by_me(&socket).is_ok() && UnixStream::connect(&socket).is_ok();
            Some(Editor {
                pid: info["pid"].as_u64()? as u32,
                socket,
                file: info["file"].as_str().map(String::from),
                started_ms: info["started_ms"].as_u64().unwrap_or(0),
                alive,
            })
        })
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.started_ms));
    out
}

/// A socket must belong to this user: a discovery file in a shared directory could otherwise
/// point an agent at someone else's server.
pub fn owned_by_me(socket: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(socket).map_err(|e| format!("{}: {e}", socket.display()))?;
    let me = unsafe { libc::getuid() };
    if meta.uid() != me {
        return Err(format!("{} belongs to another user (uid {}); refusing to connect", socket.display(), meta.uid()));
    }
    Ok(())
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// What a session talks to.
pub enum Conn {
    Live { writer: UnixStream, reader: BufReader<UnixStream>, socket: PathBuf },
    Headless { session: Box<Session>, path: Option<PathBuf> },
}

pub struct Engine {
    pub conn: Conn,
    pub events: Arc<Events>,
    next_id: u64,
}

impl Engine {
    /// Attaches to a live editor's socket: one connection for requests and one subscribed to
    /// every change, read on a thread into the event buffer.
    pub fn attach(socket: &Path) -> Result<Engine, String> {
        owned_by_me(socket)?;
        let connect = || UnixStream::connect(socket).map_err(|e| format!("{}: {e}", socket.display()));
        let writer = connect()?;
        writer.set_read_timeout(Some(Duration::from_secs(30))).ok();
        let reader = BufReader::new(writer.try_clone().map_err(|e| e.to_string())?);
        let events = Arc::new(Events::default());

        let mut sub = connect()?;
        sub.write_all(b"{\"id\":\"sub\",\"op\":\"subscribe\",\"with_msgs\":true}\n").map_err(|e| e.to_string())?;
        let mut sub_reader = BufReader::new(sub.try_clone().map_err(|e| e.to_string())?);
        let mut first = String::new();
        sub_reader.read_line(&mut first).map_err(|e| format!("subscribe: {e}"))?;
        let first: Value = serde_json::from_str(&first).map_err(|e| format!("subscribe: {e}"))?;
        if first.get("error").is_some() {
            return Err(format!("subscribe: {}", first["error"]));
        }
        let ev = events.clone();
        std::thread::spawn(move || {
            let _keep = sub;
            for line in sub_reader.lines() {
                let Ok(line) = line else { break };
                if let Ok(v) = serde_json::from_str::<Value>(&line)
                    && v.get("event").is_some() {
                        ev.push(v);
                    }
            }
            ev.close();
        });
        Ok(Engine { conn: Conn::Live { writer, reader, socket: socket.to_path_buf() }, events, next_id: 1 })
    }

    /// A headless engine in process on `state`; `path` is where `save` writes.
    pub fn headless(state: caretline::State, path: Option<PathBuf>) -> Engine {
        Engine { conn: Conn::Headless { session: Box::new(Session::new(state)), path }, events: Arc::new(Events::default()), next_id: 1 }
    }

    pub fn is_live(&self) -> bool {
        matches!(self.conn, Conn::Live { .. })
    }

    /// Sends one request and returns its `result`, or the engine's error.
    pub fn call(&mut self, mut req: Value) -> Result<Value, ProtoError> {
        let id = self.next_id;
        self.next_id += 1;
        req["id"] = json!(id);
        let line = req.to_string();
        let resp: Value = match &mut self.conn {
            Conn::Live { writer, reader, .. } => {
                writer
                    .write_all(format!("{line}\n").as_bytes())
                    .map_err(|e| ProtoError::new("closed", format!("the editor is gone: {e}")))?;
                loop {
                    let mut buf = String::new();
                    let n = reader.read_line(&mut buf).map_err(|e| ProtoError::new("closed", format!("the editor is gone: {e}")))?;
                    if n == 0 {
                        return Err(ProtoError::new("closed", "the editor is gone (it quit or closed the socket)"));
                    }
                    let v: Value = serde_json::from_str(&buf).map_err(|e| ProtoError::new("parse", e.to_string()))?;
                    if v["id"] == json!(id) {
                        break v;
                    }
                }
            }
            Conn::Headless { session, path } => {
                let apply = req["apply_effects"].as_bool() == Some(true);
                let target = path.clone();
                let mut exec = move |e: &Effect| -> Option<Msg> {
                    match e {
                        Effect::WriteFile { text, .. } => {
                            let Some(p) = &target else {
                                return Some(Msg::SaveFailed { err: "this session has no file".into() });
                            };
                            Some(match std::fs::write(p, text) {
                                Ok(()) => Msg::Saved,
                                Err(e) => Msg::SaveFailed { err: e.to_string() },
                            })
                        }
                        _ => None,
                    }
                };
                let handled = session.handle_at(&line, if apply { Some(&mut exec) } else { None }, Some(now_ms()));
                if let Some(change) = &handled.change {
                    let sub = Subscription { msgs: true, frame: None, state: false };
                    let ev = caretline::protocol::event_line(session, change, &sub, Some("client"));
                    if let Ok(v) = serde_json::from_str(&ev) {
                        self.events.push(v);
                    }
                }
                serde_json::from_str(&handled.response).map_err(|e| ProtoError::new("parse", e.to_string()))?
            }
        };
        if let Some(err) = resp.get("error") {
            return Err(ProtoError::new(err["kind"].as_str().unwrap_or("error"), err["message"].as_str().unwrap_or("").to_string()));
        }
        Ok(resp["result"].clone())
    }

    pub fn socket(&self) -> Option<&Path> {
        match &self.conn {
            Conn::Live { socket, .. } => Some(socket),
            Conn::Headless { .. } => None,
        }
    }
}
