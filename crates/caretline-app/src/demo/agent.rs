//! `caretline demo agent`: a scripted agent that co-edits the document over the editor's
//! socket. It is an ordinary protocol client: it opens a view of its own (the pane under the
//! person's), and types into its section of the document one guarded write at a time.
//!
//! Each keystroke reads the rev and the agent's caret (`view.list`), then sends the
//! character as a change from elsewhere (`external`, so the person's undo never takes it
//! back) with `if_rev`, and moves its caret past it (a change from elsewhere leaves every
//! caret before the text it puts in). If the person typed in between, the editor answers `stale` and the
//! agent reads again.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::Progress;

pub const CONNECTING: &str = "agent · connecting…";

/// What the agent types, line by line, into its own section.
const SCRIPT: &[&str] = &[
    "- Hello! You're in “Yours”; I'll stay down here.",
    "- Draft the release notes",
    "- Check the installer on Linux",
    "  - and on macOS, under Rosetta",
    "- Each keystroke of mine is a guarded write: when you type between my read and my write, the editor refuses mine as stale and I read again.",
];

/// A protocol connection: one request, one response line.
pub(crate) struct Conn {
    w: UnixStream,
    r: BufReader<UnixStream>,
    line: String,
}

/// The answer to a request: its result, or the error's kind and message.
pub(crate) enum Reply {
    Ok(Value),
    Err { kind: String, message: String },
}

impl Conn {
    pub fn connect(path: &Path) -> Result<Conn, String> {
        let s = UnixStream::connect(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Conn { w: s.try_clone().map_err(|e| e.to_string())?, r: BufReader::new(s), line: String::new() })
    }

    /// Connects, retrying for up to `wait` while the editor starts.
    pub fn connect_within(path: &Path, wait: Duration) -> Result<Conn, String> {
        let until = Instant::now() + wait;
        loop {
            match Conn::connect(path) {
                Ok(c) => return Ok(c),
                Err(e) if Instant::now() >= until => return Err(e),
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    /// Sends one raw request line and reads the response line.
    pub fn send(&mut self, req: &str) -> Result<&str, String> {
        self.w.write_all(req.as_bytes()).and_then(|_| self.w.write_all(b"\n")).map_err(|e| format!("send: {e}"))?;
        self.line.clear();
        self.r.read_line(&mut self.line).map_err(|e| format!("read: {e}"))?;
        if self.line.is_empty() {
            return Err("the editor closed the connection".into());
        }
        Ok(&self.line)
    }

    pub fn request(&mut self, req: Value) -> Result<Reply, String> {
        let line = self.send(&req.to_string())?;
        let v: Value = serde_json::from_str(line).map_err(|e| format!("reply: {e}"))?;
        Ok(match v.get("error") {
            Some(e) => Reply::Err {
                kind: e["kind"].as_str().unwrap_or("").to_string(),
                message: e["message"].as_str().unwrap_or("").to_string(),
            },
            None => Reply::Ok(v["result"].clone()),
        })
    }

    pub fn ask(&mut self, req: Value) -> Result<Value, String> {
        match self.request(req)? {
            Reply::Ok(v) => Ok(v),
            Reply::Err { kind, message } => Err(format!("{kind}: {message}")),
        }
    }
}

/// How the agent paces itself.
pub(crate) struct Pace {
    /// Before the first keystroke.
    pub start: Duration,
    /// Between reading the rev and writing (the agent "thinking"), per character.
    pub key: Duration,
    /// Extra pause at the end of a line.
    pub line: Duration,
}

/// The agent's run, in numbers.
#[derive(Debug, Default, Clone)]
pub(crate) struct Report {
    /// The text it typed, and where it started.
    pub typed: String,
    pub at: usize,
    pub writes: u64,
    pub stale: u64,
}

/// Runs the script against the editor at `conn`. `between` runs between each read and its
/// write (the headless test types there, to force stale writes).
pub(crate) fn run(
    conn: &mut Conn,
    pace: &Pace,
    progress: &Arc<Mutex<Progress>>,
    between: &mut dyn FnMut(u64),
) -> Result<Report, String> {
    let hello = conn.ask(json!({"op": "hello"}))?;
    if hello["proto"].as_u64() != Some(1) {
        return Err(format!("unexpected protocol: {hello}"));
    }
    // A view of its own, its caret at the end of the document: its section.
    let view = conn.ask(json!({"op": "view.open", "w": 80, "h": 8}))?["view"].as_u64().ok_or("view.open: no view")?;
    conn.ask(json!({"op": "msgs", "view": view, "msgs": [
        {"msg": "move", "dir": "forward", "by": "doc_end"},
        {"msg": "show_status", "text": "agent · here, about to type"},
    ]}))?;
    std::thread::sleep(pace.start);
    if let Ok(mut p) = progress.lock() {
        p.started = true;
    }

    let mut report = Report::default();
    let mut first = true;
    let mut lines: Vec<String> = SCRIPT.iter().map(|s| s.to_string()).collect();
    lines.push(String::new()); // the summary, written last with the real numbers
    let count = lines.len();
    for (i, mut line) in lines.into_iter().enumerate() {
        if i + 1 == count {
            line = format!(
                "- Done. Before this line: {} guarded writes, {} refused as stale and retried. Now press ⌃Z: your edits go, mine stay.",
                report.writes, report.stale
            );
        }
        let mut chars: Vec<char> = Vec::new();
        chars.push('\n');
        chars.extend(line.chars());
        for c in chars {
            let mut attempt = 0;
            let (rev, caret) = loop {
                attempt += 1;
                if attempt > 1000 {
                    return Err("the document kept changing: gave up after 1000 tries".into());
                }
                let views = conn.ask(json!({"op": "view.list"}))?;
                let rev = views["rev"].as_u64().ok_or("view.list: no rev")?;
                let caret = views["views"]
                    .as_array()
                    .and_then(|vs| vs.iter().find(|v| v["view"].as_u64() == Some(view)))
                    .and_then(|v| v["caret"].as_u64())
                    .ok_or("the agent's view is gone")? as usize;
                if first {
                    report.at = caret;
                    first = false;
                }
                std::thread::sleep(pace.key);
                if attempt == 1 {
                    between(report.writes);
                }
                // A change from elsewhere leaves every caret before what it puts in, this
                // view's too, so the agent moves its own caret past it in the same request
                // (which clears the status: it is set again).
                let status = format!("agent · typing · {} writes · {} stale, retried", report.writes + 1, report.stale);
                let write = json!({"op": "msgs", "view": view, "if_rev": rev, "msgs": [
                    {"msg": "external", "changes": [{"change": "replace", "from": caret, "to": caret, "text": c.to_string()}]},
                    {"msg": "move", "dir": "forward", "by": "doc_end"},
                    {"msg": "show_status", "text": status},
                ]});
                match conn.request(write)? {
                    Reply::Ok(_) => break (rev, caret),
                    Reply::Err { kind, .. } if kind == "stale" => {
                        report.stale += 1;
                        if let Ok(mut p) = progress.lock() {
                            p.stale = report.stale;
                        }
                    }
                    Reply::Err { kind, message } => return Err(format!("{kind}: {message}")),
                }
            };
            let _ = (rev, caret);
            report.writes += 1;
            report.typed.push(c);
            if let Ok(mut p) = progress.lock() {
                p.writes = report.writes;
            }
        }
        std::thread::sleep(pace.line);
    }
    let text = format!("agent · done · {} writes · {} refused as stale and retried", report.writes, report.stale);
    conn.ask(json!({"op": "msgs", "view": view, "msgs": [{"msg": "show_status", "text": text}]}))?;
    if let Ok(mut p) = progress.lock() {
        p.done = true;
    }
    Ok(report)
}

/// Starts the agent on a thread: it connects to the editor's socket once the editor is up.
pub(crate) fn spawn(socket: PathBuf, progress: Arc<Mutex<Progress>>) {
    std::thread::spawn(move || {
        let result = Conn::connect_within(&socket, Duration::from_secs(5)).and_then(|mut c| {
            let pace = Pace { start: Duration::from_millis(1200), key: Duration::from_millis(45), line: Duration::from_millis(500) };
            run(&mut c, &pace, &progress, &mut |_| {})
        });
        if let Err(e) = result
            && let Ok(mut p) = progress.lock()
        {
            p.error = Some(e);
        }
    });
}

/// `caretline demo agent --headless`: the agent against a headless editor on a real socket,
/// with a person typing between some of its reads and writes. Checks that every write
/// landed, that the interleaved ones were refused and retried, and that the person's undo
/// takes back only the person's text. Prints a JSON report.
pub(crate) fn headless() -> Result<(), String> {
    let state = super::initial_state(super::Kind::Agent, None, caretline::Viewport { width: 80, height: 24 });
    let initial = state.doc.text.to_string();
    let socket = std::env::temp_dir().join(format!("caretline-demo-{}.sock", std::process::id()));
    let socket = if socket.as_os_str().len() > 100 { PathBuf::from(format!("/tmp/caretline-demo-{}.sock", std::process::id())) } else { socket };
    let hub = crate::hub::Hub::new(caretline::Session::new(state), None);
    let (tx, rx) = std::sync::mpsc::channel();
    let listening = crate::hub::listen(&socket, tx)?;
    std::thread::spawn(move || crate::hub::serve(hub, rx, false));

    let mut person = Conn::connect_within(&socket, Duration::from_secs(5))?;
    let mut agent = Conn::connect_within(&socket, Duration::from_secs(5))?;
    let progress = Arc::new(Mutex::new(Progress::default()));
    let pace = Pace { start: Duration::ZERO, key: Duration::ZERO, line: Duration::ZERO };
    let mut typed_by_person = 0u64;
    let mut person_err: Option<String> = None;
    let report = run(&mut agent, &pace, &progress, &mut |writes| {
        // Every seventh keystroke, the person types between the agent's read and write.
        if writes % 7 == 3 {
            match person.ask(json!({"op": "keys", "keys": "x"})) {
                Ok(_) => typed_by_person += 1,
                Err(e) => person_err = Some(e),
            }
        }
    })?;
    if let Some(e) = person_err {
        return Err(e);
    }
    let text_of = |c: &mut Conn| -> Result<String, String> {
        let s = c.ask(json!({"op": "state.get", "history": false}))?;
        Ok(s["state"]["text"].as_str().unwrap_or("").to_string())
    };
    let after_both = text_of(&mut person)?;

    // What the document should be with only the agent's text in it.
    let mut want: Vec<char> = initial.chars().collect();
    let at = report.at.min(want.len());
    want.splice(at..at, report.typed.chars());
    let want: String = want.into_iter().collect();

    // The person undoes until nothing changes.
    let mut undos = 0;
    let mut text = after_both.clone();
    while undos < 1000 {
        person.ask(json!({"op": "msgs", "msgs": [{"msg": "undo"}]}))?;
        let next = text_of(&mut person)?;
        if next == text {
            break;
        }
        text = next;
        undos += 1;
    }
    let agent_text_landed = SCRIPT.iter().all(|l| after_both.contains(l));
    let ok = report.stale > 0 && agent_text_landed && text == want && report.writes as usize == report.typed.chars().count();
    let out = json!({
        "ok": ok,
        "agent": {"writes": report.writes, "stale_retried": report.stale, "chars": report.typed.chars().count()},
        "person": {"keys": typed_by_person, "undos": undos},
        "agent_text_landed": agent_text_landed,
        "undo_kept_only_the_agents_text": text == want,
        "text": text,
    });
    println!("{}", serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?);
    drop(listening);
    if ok { Ok(()) } else { Err("the agent demo's checks failed (see the report)".into()) }
}
