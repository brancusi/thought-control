//! The daemon socket protocol (SPEC §6.4), version 1.
//!
//! Newline-delimited JSON over a Unix socket. A client sends requests and receives responses
//! with the same `id`; after `hello` with topics, the daemon also pushes events (no `id`).
//!
//! ```text
//! → {"id":1,"method":"hello","params":{"client":"thoughtbar","version":"0.1.0","topics":["changed","alerts"],"deliver":true}}
//! ← {"id":1,"result":{"daemon":"0.1.0","proto":1,"device":"mbp-7f3a"}}
//! ← {"event":"changed","data":{"ids":["k3f9a2mq7x1c"],"tx":"01K…","actor":"agent:claude","dev":"studio-mini"}}
//! → {"id":2,"method":"alert.delivered","params":{"alerts":["xr7pd…"]}}
//! ```
//!
//! Methods: `hello`, `status`, `query`, `today`, `capture`, `complete`, `set`, `snooze`, `ack`,
//! `undo`, `alert.delivered`, `shutdown`. Events: `changed`, `conflict`, `alert.fire`,
//! `alert.withdraw`, `hello`.
//!
//! Discovery: the daemon writes `<cache>/daemon.json` with its pid, socket path and versions.
//! Unix socket paths are length-limited (104 bytes on macOS), so the socket lives at
//! `/tmp/thc-<uid>/<hash of cache dir>.sock` (directory mode 0700).

use crate::vault::Paths;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const PROTO_VERSION: u32 = 1;
/// Additive changes within v1 (clients ignore what they don't know):
/// 2 = `parse`, TodayPanel `badge` / `upcoming_more` / `to_review` / `conflict_ids` / `presets`,
/// Node `rev` / `place`, the `shutdown` event, DaemonStatus `exe`.
/// 3 = live usage `tokens` snapshots and the `tokens` subscription topic/event.
pub const PROTO_MINOR: u32 = 3;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventMsg {
    pub event: String,
    #[serde(default)]
    pub data: Value,
}

/// Anything the daemon can send on a connection.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Incoming {
    Response(Response),
    Event(EventMsg),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub pid: u32,
    pub socket: PathBuf,
    pub version: String,
    pub proto: u32,
    pub started_ms: i64,
    pub vault: PathBuf,
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Short, per-cache socket path (see module docs).
pub fn socket_path(paths: &Paths) -> PathBuf {
    if let Ok(p) = std::env::var("THC_SOCKET") {
        return PathBuf::from(p);
    }
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/thc-{uid}")).join(format!("{:016x}.sock", fnv(&paths.cache.to_string_lossy())))
}

pub fn info_path(paths: &Paths) -> PathBuf {
    paths.cache.join("daemon.json")
}

pub fn read_info(paths: &Paths) -> Option<DaemonInfo> {
    let s = std::fs::read_to_string(info_path(paths)).ok()?;
    serde_json::from_str(&s).ok()
}

/// Which file a binary path names right now: `dev:inode`. A daemon records its own at start, so
/// a client can tell it runs a copy that was since replaced (an install renames a new file over
/// the old one, which keeps the path and the version string when the version didn't change).
pub fn exe_identity(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some(format!("{}:{}", m.dev(), m.ino()))
}

/// This process's binary as it was when first asked (call it early: on Linux the path of a
/// replaced binary gains " (deleted)").
pub fn own_exe_identity() -> Option<String> {
    static ID: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    ID.get_or_init(|| std::env::current_exe().ok().and_then(|p| exe_identity(&p))).clone()
}

pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks for existence/permission.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// A blocking client connection.
pub struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
    /// Events received while waiting for a response.
    pub pending_events: Vec<EventMsg>,
}

impl Client {
    pub fn connect_path(path: &Path, timeout: Duration) -> Result<Client> {
        let stream = UnixStream::connect(path).with_context(|| format!("connecting to {}", path.display()))?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Client { stream, reader, next_id: 1, pending_events: vec![] })
    }

    /// Connect to the daemon for this vault, if one is running. Fails fast (no daemon).
    pub fn connect(paths: &Paths) -> Option<Client> {
        let path = socket_path(paths);
        if !path.exists() {
            return None;
        }
        Client::connect_path(&path, Duration::from_millis(2000)).ok()
    }

    pub fn send(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let line = serde_json::to_string(&Request { id, method: method.into(), params })?;
        self.stream.write_all(line.as_bytes())?;
        self.stream.write_all(b"\n")?;
        self.stream.flush()?;
        Ok(id)
    }

    /// Next message (response or event). `Ok(None)` on timeout or closed connection.
    pub fn read(&mut self) -> Result<Option<Incoming>> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(serde_json::from_str(line.trim()).with_context(|| format!("bad message from daemon: {line}"))?)),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.send(method, params)?;
        loop {
            match self.read()? {
                Some(Incoming::Response(r)) if r.id == id => {
                    return match (r.result, r.error) {
                        (_, Some(e)) => Err(match e.kind.as_str() {
                            "not_found" => crate::error::ThcError::NotFound(e.message).into(),
                            "validation" => crate::error::ThcError::Validation(e.message).into(),
                            "conflict" => crate::error::ThcError::Conflict(e.message).into(),
                            "usage" => crate::error::ThcError::Usage(e.message).into(),
                            _ => anyhow!(e.message),
                        }),
                        (Some(v), None) => Ok(v),
                        (None, None) => Ok(Value::Null),
                    };
                }
                Some(Incoming::Response(_)) => continue,
                Some(Incoming::Event(ev)) => self.pending_events.push(ev),
                None => bail!("the daemon didn't answer"),
            }
        }
    }

    pub fn hello(&mut self, client: &str, topics: &[&str], deliver: bool) -> Result<Value> {
        self.call("hello", json!({ "client": client, "version": VERSION, "proto": PROTO_VERSION, "topics": topics, "deliver": deliver }))
    }

    pub fn set_read_timeout(&self, t: Option<Duration>) -> Result<()> {
        self.stream.set_read_timeout(t)?;
        Ok(())
    }
}

/// Serialize an error for the wire, keeping the CLI's error kinds.
pub fn rpc_error(e: &anyhow::Error) -> RpcError {
    match e.downcast_ref::<crate::error::ThcError>() {
        Some(t) => RpcError { kind: t.kind().into(), message: format!("{e:#}") },
        None => RpcError { kind: "error".into(), message: format!("{e:#}") },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_roundtrip() {
        let r: Incoming = serde_json::from_str(r#"{"id":3,"result":{"ok":true}}"#).unwrap();
        assert!(matches!(r, Incoming::Response(Response { id: 3, .. })));
        let e: Incoming = serde_json::from_str(r#"{"event":"changed","data":{"ids":["a"]}}"#).unwrap();
        assert!(matches!(e, Incoming::Event(EventMsg { ref event, .. }) if event == "changed"));
    }

    #[test]
    fn socket_path_is_short() {
        let p = Paths { vault: PathBuf::from("/v"), cache: PathBuf::from("/a/very/long/cache/path/".repeat(10)) };
        assert!(socket_path(&p).to_string_lossy().len() < 64);
    }
}
