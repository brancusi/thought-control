//! The protocol server: one [`Session`] and the clients talking to it.
//!
//! Every input (a request line from any client, a connect or disconnect, and in the
//! interactive editor a terminal event) arrives on one channel, so the session sees them
//! in a single, deterministic order, and that order is what the trace records.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use caretline_next::protocol::{event_line, Change, Control, Executor, Subscription};
use caretline_next::Session;

pub type ClientId = u64;

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);

/// One input to the server loop.
pub enum Input {
    Connect { client: ClientId, out: Sender<String> },
    Line { client: ClientId, line: String },
    Disconnect { client: ClientId },
    /// A terminal event (the interactive editor only).
    Terminal(crossterm::event::Event),
}

struct Client {
    out: Sender<String>,
    sub: Option<Subscription>,
}

pub struct Hub {
    pub session: Session,
    clients: HashMap<ClientId, Client>,
    trace: Option<File>,
    traced: usize,
}

impl Hub {
    pub fn new(session: Session, trace: Option<File>) -> Hub {
        let mut hub = Hub { session, clients: HashMap::new(), trace, traced: 0 };
        hub.flush_trace();
        hub
    }

    pub fn connect(&mut self, client: ClientId, out: Sender<String>) {
        self.clients.insert(client, Client { out, sub: None });
    }

    /// Forgets a client; dropping its sender lets its writer finish and close the stream.
    pub fn disconnect(&mut self, client: ClientId) {
        self.clients.remove(&client);
    }

    pub fn has_clients(&self) -> bool {
        !self.clients.is_empty()
    }

    /// Answers one request from `client`, then notifies subscribers of any change.
    pub fn request(&mut self, client: ClientId, line: &str, exec: Option<Executor<'_>>) -> Option<Change> {
        if line.trim().is_empty() {
            return None;
        }
        let handled = self.session.handle(line, exec);
        if let Some(c) = self.clients.get_mut(&client) {
            match handled.control {
                Some(Control::Subscribe(sub)) => c.sub = Some(sub),
                Some(Control::Unsubscribe) => c.sub = None,
                None => {}
            }
            let _ = c.out.send(handled.response);
        }
        if let Some(change) = &handled.change {
            self.changed(change, "client");
        }
        handled.change
    }

    /// Records a change in the trace file and sends an event to every subscriber.
    pub fn changed(&mut self, change: &Change, source: &str) {
        self.flush_trace();
        for c in self.clients.values() {
            if let Some(sub) = &c.sub {
                let _ = c.out.send(event_line(&self.session, change, sub, Some(source)));
            }
        }
    }

    /// Appends trace lines not yet written to the trace file.
    fn flush_trace(&mut self) {
        let lines = &self.session.trace()[self.traced..];
        if let Some(f) = &mut self.trace {
            if !lines.is_empty() {
                let mut buf = String::new();
                for l in lines {
                    buf.push_str(&l.to_line());
                    buf.push('\n');
                }
                let _ = f.write_all(buf.as_bytes());
                let _ = f.flush();
            }
        }
        self.traced = self.session.trace().len();
    }
}

/// Starts a writer thread for `w`: every line sent is written, and the stream is flushed
/// whenever the queue runs dry. The thread ends when every sender is dropped.
fn spawn_writer(mut w: impl Write + Send + 'static) -> (Sender<String>, thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<String>();
    let handle = thread::spawn(move || {
        while let Ok(line) = rx.recv() {
            let mut ok = w.write_all(line.as_bytes()).and_then(|_| w.write_all(b"\n")).is_ok();
            while ok {
                match rx.try_recv() {
                    Ok(line) => ok = w.write_all(line.as_bytes()).and_then(|_| w.write_all(b"\n")).is_ok(),
                    Err(_) => break,
                }
            }
            if !ok || w.flush().is_err() {
                break;
            }
        }
    });
    (tx, handle)
}

/// Feeds a reader's lines into the hub as one client. Returns the writer thread, which
/// ends once the hub forgets the client and every response is written.
fn spawn_client(
    reader: impl io::Read + Send + 'static,
    writer: impl Write + Send + 'static,
    tx: &Sender<Input>,
) -> thread::JoinHandle<()> {
    let client = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
    let (out, handle) = spawn_writer(writer);
    if tx.send(Input::Connect { client, out }).is_err() {
        return handle;
    }
    let tx = tx.clone();
    thread::spawn(move || {
        let reader = BufReader::new(reader);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if tx.send(Input::Line { client, line }).is_err() {
                return;
            }
        }
        let _ = tx.send(Input::Disconnect { client });
    });
    handle
}

/// stdin and stdout as one client. Join the handle before exiting so every response is out.
pub fn spawn_stdio(tx: &Sender<Input>) -> thread::JoinHandle<()> {
    spawn_client(io::stdin(), io::stdout(), tx)
}

/// A bound socket; removes its file when dropped.
pub struct Listening {
    pub path: PathBuf,
    discovery: Option<PathBuf>,
}

impl Drop for Listening {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        if let Some(d) = &self.discovery {
            let _ = std::fs::remove_file(d);
        }
    }
}

impl Listening {
    /// Writes `$TMPDIR/caretline/<pid>.json` so `caretline send --pid/--latest` finds us.
    pub fn advertise(&mut self, file: Option<&str>) -> io::Result<PathBuf> {
        let dir = discovery_dir();
        std::fs::create_dir_all(&dir)?;
        let pid = std::process::id();
        let path = dir.join(format!("{pid}.json"));
        let info = serde_json::json!({
            "pid": pid,
            "socket": self.path,
            "file": file,
            "proto": caretline_next::protocol::PROTO,
            "started_ms": crate::runtime::now_ms(),
        });
        std::fs::write(&path, info.to_string() + "\n")?;
        self.discovery = Some(path.clone());
        Ok(path)
    }
}

pub fn discovery_dir() -> PathBuf {
    std::env::temp_dir().join("caretline")
}

pub fn default_socket_path() -> PathBuf {
    std::env::temp_dir().join(format!("caretline-{}.sock", std::process::id()))
}

/// Binds `path` and accepts clients on a thread, each one feeding `tx`. A leftover socket
/// file nobody answers on is replaced; a live one is an error.
pub fn listen(path: &Path, tx: Sender<Input>) -> Result<Listening, String> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(format!("{}: another server is listening there", path.display()));
        }
        let _ = std::fs::remove_file(path);
    }
    let listener = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let Ok(write) = stream.try_clone() else { continue };
            let _ = spawn_client(stream, write, &tx);
        }
    });
    Ok(Listening { path: path.to_path_buf(), discovery: None })
}

/// The headless server loop: answers requests until the input channel closes (or, for
/// stdio, until stdin ends and its responses are written).
pub fn serve(mut hub: Hub, rx: Receiver<Input>, exit_when_idle: bool) {
    let mut seen_client = false;
    for input in rx {
        match input {
            Input::Connect { client, out } => {
                seen_client = true;
                hub.connect(client, out);
            }
            Input::Line { client, line } => {
                hub.request(client, &line, None);
            }
            Input::Disconnect { client } => {
                hub.disconnect(client);
                if exit_when_idle && seen_client && !hub.has_clients() {
                    break;
                }
            }
            Input::Terminal(_) => {}
        }
    }
}
