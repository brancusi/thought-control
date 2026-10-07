//! Live attach: the interactive editor on a pseudo-terminal with `--listen`, driven over
//! its socket and its keyboard at once.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const ROWS: u16 = 12;
const COLS: u16 = 60;

struct Pty {
    master: std::fs::File,
    out: Arc<Mutex<Vec<u8>>>,
    child: std::process::Child,
}

impl Drop for Pty {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Pty {
    fn spawn(mut c: Command) -> Pty {
        let (mut m, mut s) = (0, 0);
        let mut ws = libc::winsize { ws_row: ROWS, ws_col: COLS, ws_xpixel: 0, ws_ypixel: 0 };
        assert_eq!(
            unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) },
            0
        );
        let slave = unsafe { OwnedFd::from_raw_fd(s) };
        c.env("TERM", "xterm-256color");
        let sfd = slave.as_raw_fd();
        c.stdin(slave.try_clone().unwrap()).stdout(slave.try_clone().unwrap()).stderr(slave);
        unsafe {
            c.pre_exec(move || {
                libc::setsid();
                libc::ioctl(sfd, libc::TIOCSCTTY as _, 0);
                Ok(())
            });
        }
        let child = c.spawn().unwrap();
        let master = unsafe { std::fs::File::from_raw_fd(m) };
        let out = Arc::new(Mutex::new(Vec::new()));
        let (mut r, o) = (master.try_clone().unwrap(), out.clone());
        let mut w = master.try_clone().unwrap();
        std::thread::spawn(move || {
            let mut b = [0u8; 65536];
            while let Ok(n) = r.read(&mut b) {
                if n == 0 {
                    break;
                }
                // Answer the keyboard-protocol query (no kitty support) so start-up is quick.
                if b[..n].windows(4).any(|x| x == b"\x1b[?u") {
                    let _ = w.write_all(b"\x1b[?62;22c");
                }
                o.lock().unwrap().extend_from_slice(&b[..n]);
            }
        });
        Pty { master, out, child }
    }

    fn send(&mut self, b: &[u8]) {
        self.master.write_all(b).unwrap();
    }

    fn screen(&self) -> String {
        screen(&self.out.lock().unwrap(), ROWS as usize, COLS as usize).join("\n")
    }

    fn wait(&self, what: &str, ok: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let s = self.screen();
            if ok(&s) {
                return s;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; screen:\n{s}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// The screen a byte stream draws: cursor moves (`CSI r;c H`), clears and text.
fn screen(out: &[u8], rows: usize, cols: usize) -> Vec<String> {
    let mut grid = vec![vec![' '; cols]; rows];
    let (mut r, mut c) = (0usize, 0usize);
    let s = String::from_utf8_lossy(out);
    let mut it = s.chars().peekable();
    while let Some(ch) = it.next() {
        match ch {
            '\x1b' => match it.next() {
                Some('[') => {
                    let mut params = String::new();
                    while let Some(&n) = it.peek() {
                        it.next();
                        if ('@'..='~').contains(&n) {
                            if n == 'H' {
                                let mut p = params.trim_start_matches('?').split(';').map(|x| x.parse::<usize>().unwrap_or(1));
                                r = p.next().unwrap_or(1).saturating_sub(1);
                                c = p.next().unwrap_or(1).saturating_sub(1);
                            } else if n == 'J' && params == "2" {
                                grid = vec![vec![' '; cols]; rows];
                            }
                            break;
                        }
                        params.push(n);
                    }
                }
                Some(']') => {
                    while let Some(n) = it.next() {
                        if n == '\x07' || (n == '\x1b' && it.peek() == Some(&'\\')) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => c = 0,
            '\n' => r += 1,
            ch if ch >= ' ' => {
                if r < rows && c < cols {
                    grid[r][c] = ch;
                }
                c += 1;
            }
            _ => {}
        }
    }
    grid.into_iter().map(|l| l.into_iter().collect()).collect()
}

struct Client {
    w: UnixStream,
    r: BufReader<UnixStream>,
}

impl Client {
    fn connect(path: &Path) -> Client {
        let s = UnixStream::connect(path).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        Client { w: s.try_clone().unwrap(), r: BufReader::new(s) }
    }
    fn line(&mut self) -> Value {
        let mut l = String::new();
        self.r.read_line(&mut l).unwrap();
        serde_json::from_str(&l).unwrap_or_else(|e| panic!("{e}: {l:?}"))
    }
    /// Sends a request and returns its response, skipping events.
    fn ask(&mut self, req: Value) -> Value {
        writeln!(self.w, "{req}").unwrap();
        loop {
            let v = self.line();
            if v.get("event").is_none() {
                return v;
            }
        }
    }
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_caretline"))
}

#[test]
fn a_running_editor_takes_pushed_state_and_messages() {
    let dir = PathBuf::from("/tmp").join(format!("cll-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let doc = dir.join("doc.md");
    std::fs::write(&doc, "first line\nsecond line\n").unwrap();
    let sock = dir.join("ed.sock");
    let trace = dir.join("t.jsonl");

    let mut c = bin();
    c.arg(&doc)
        .arg("--listen")
        .arg(&sock)
        .arg("--trace")
        .arg(&trace)
        .arg("--no-mouse")
        .env("TMPDIR", &dir);
    let mut pty = Pty::spawn(c);
    pty.wait("the editor", |s| s.contains("first line") && s.contains("listening on"));

    // Discovery: the pid file names the socket, and send --pid / --latest find it.
    let pid = pty.child.id();
    let info: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("caretline").join(format!("{pid}.json"))).unwrap()).unwrap();
    assert_eq!(info["socket"], sock.to_str().unwrap());
    let latest = bin().args(["send", "--latest", "hello"]).env("TMPDIR", &dir).output().unwrap();
    assert!(String::from_utf8_lossy(&latest.stdout).contains("\"proto\":1"), "{latest:?}");
    let by_pid = bin().args(["send", "--pid", &pid.to_string(), "state.get", "--raw"]).env("TMPDIR", &dir).output().unwrap();
    assert!(String::from_utf8_lossy(&by_pid.stdout).contains("first line"));

    let mut client = Client::connect(&sock);
    let mut watcher = Client::connect(&sock);
    watcher.ask(json!({"op": "subscribe", "frame": {"format": "text"}}));

    // Pushed messages redraw the terminal at once.
    let r = client.ask(json!({"id": 1, "op": "msgs", "msgs": [{"msg": "insert_text", "text": "PUSHED "}]}));
    let rev = r["result"]["rev"].as_u64().unwrap();
    pty.wait("the pushed text", |s| s.contains("PUSHED first line"));
    let ev = watcher.line();
    assert_eq!((ev["rev"].as_u64(), ev["source"].as_str()), (Some(rev), Some("client")));

    // The local user's keys interleave with pushed ones in one order. The client wrote
    // through its own view, so the person's caret stayed before what it put in.
    assert_eq!(r["result"]["view"], 1, "{r}");
    pty.send(b"k");
    pty.wait("the typed key", |s| s.contains("kPUSHED first line"));
    let ev = watcher.line();
    assert_eq!(ev["source"], "terminal");
    assert!(ev["rev"].as_u64().unwrap() > rev);
    let r = client.ask(json!({"op": "keys", "keys": "<down><end>!"}));
    assert!(r["result"]["rev"].as_u64().unwrap() > ev["rev"].as_u64().unwrap());
    pty.wait("the pushed keys", |s| s.contains("second line!"));

    // A pushed save comes back as an effect and doesn't touch the file.
    let r = client.ask(json!({"op": "msgs", "msgs": [{"msg": "save"}]}));
    assert_eq!(r["result"]["effects"][0]["effect"], "write_file");
    assert_eq!(std::fs::read_to_string(&doc).unwrap(), "first line\nsecond line\n");

    // A whole state pushed in replaces the screen, keeping the terminal's size.
    let mut st: Value = serde_json::from_str(&std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/wrapped-paragraph.state.json"),
    ).unwrap()).unwrap();
    st["path"] = json!(doc.to_str().unwrap());
    client.ask(json!({"op": "state.set", "state": st}));
    let screen = pty.wait("the pushed state", |s| !s.contains("PUSHED") && s.contains("doc.md"));
    let got = client.ask(json!({"op": "state.get"}));
    let state = &got["result"]["state"];
    assert_eq!(state["text"], st["text"]);
    assert_eq!(state["viewport"], json!({"width": COLS, "height": ROWS}));
    let first = st["text"].as_str().unwrap().lines().next().unwrap();
    assert!(screen.contains(first), "{screen}");

    // The rendered frame is the screen.
    let r = client.ask(json!({"op": "render"}));
    let frame = r["result"]["frame"].as_str().unwrap();
    let screen = pty.screen();
    for (want, got) in frame.lines().zip(screen.lines()).take(ROWS as usize - 1) {
        assert_eq!(want.trim_end(), got.trim_end());
    }

    // The trace replays to the state the editor holds.
    let replayed = bin().arg("--replay").arg(&trace).args(["--dump-state", "-"]).output().unwrap();
    assert!(replayed.status.success(), "{}", String::from_utf8_lossy(&replayed.stderr));
    let replayed: Value = serde_json::from_slice(&replayed.stdout).unwrap();
    assert_eq!(&replayed, state);
    let r = client.ask(json!({"op": "trace.get", "all": true}));
    let lines: Vec<String> = r["result"]["trace"].as_array().unwrap().iter().map(|l| l.to_string()).collect();
    assert_eq!(lines.len(), std::fs::read_to_string(&trace).unwrap().lines().count());

    // Pushed effects run only when asked: two quits (the first is armed by unsaved changes).
    let r = client.ask(json!({"op": "msgs", "apply_effects": true, "msgs": [{"msg": "quit"}, {"msg": "quit"}]}));
    assert_eq!(r["result"]["executed"], true);
    let deadline = Instant::now() + Duration::from_secs(10);
    while pty.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the editor didn't quit");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!sock.exists(), "the socket is removed on exit");
    assert!(!dir.join("caretline").join(format!("{pid}.json")).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The view list's carets, by view id.
fn carets(c: &mut Client) -> Vec<(u64, u64)> {
    let r = c.ask(json!({"op": "view.list"}));
    r["result"]["views"].as_array().unwrap().iter().map(|v| (v["view"].as_u64().unwrap(), v["caret"].as_u64().unwrap())).collect()
}

#[test]
fn a_person_keeps_typing_while_a_client_pushes_text() {
    let dir = PathBuf::from("/tmp").join(format!("clc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let doc = dir.join("doc.md");
    std::fs::write(&doc, "Hey there, \nnext\n").unwrap();
    let sock = dir.join("ed.sock");
    let trace = dir.join("t.jsonl");
    let mut c = bin();
    c.arg(&doc).arg("--listen").arg(&sock).arg("--trace").arg(&trace).arg("--no-mouse").env("TMPDIR", &dir);
    let mut pty = Pty::spawn(c);
    pty.wait("the editor", |s| s.contains("Hey there,") && s.contains("listening on"));
    let mut client = Client::connect(&sock);
    pty.send(b"\x05"); // ctrl-e: the end of the line
    let deadline = Instant::now() + Duration::from_secs(10);
    while carets(&mut client)[0].1 != 11 {
        assert!(Instant::now() < deadline, "the caret never reached the line's end");
        std::thread::sleep(Duration::from_millis(10));
    }

    // The person types a letter, and the client pushes text at and around their caret, as
    // fast as it can: text.set and external messages, each against the rev it read.
    let mut typed = String::new();
    let mut pushes = 0;
    for i in 0..150 {
        let ch = (b'a' + (i % 26) as u8) as char;
        if i % 3 == 0 {
            pty.send(ch.to_string().as_bytes());
            typed.push(ch);
        }
        for _attempt in 0..50 {
            let got = client.ask(json!({"op": "state.get", "history": false}));
            let rev = got["result"]["rev"].as_u64().unwrap();
            let text = got["result"]["state"]["text"].as_str().unwrap().to_string();
            let at = carets(&mut client)[0].1 as usize;
            let chars: Vec<char> = text.chars().collect();
            let at = match i % 4 {
                0 | 1 => at,
                2 => chars.len(),
                _ => chars[at..].iter().position(|&c| c == '\n').map_or(chars.len(), |p| at + p + 1),
            };
            let note = format!("\n- note {i}");
            let r = if i % 2 == 0 {
                let mut new = chars.clone();
                new.splice(at..at, note.chars());
                client.ask(json!({"op": "text.set", "text": new.into_iter().collect::<String>(), "if_rev": rev}))
            } else {
                let change = json!({"change": "replace", "from": at, "to": at, "text": note});
                client.ask(json!({"op": "msgs", "msgs": [{"msg": "external", "changes": [change]}], "if_rev": rev}))
            };
            if r.get("result").is_some() {
                pushes += 1;
                break;
            }
            assert_eq!(r["error"]["kind"], "stale", "{r}");
        }
    }
    assert!(pushes >= 100, "{pushes} pushes");

    // Every key landed, in one run on the person's line, with their caret at its end.
    let mine = format!("Hey there, {typed}");
    let deadline = Instant::now() + Duration::from_secs(10);
    let text = loop {
        let got = client.ask(json!({"op": "state.get", "history": false}));
        let text = got["result"]["state"]["text"].as_str().unwrap().to_string();
        if text.contains(&mine) {
            break text;
        }
        assert!(Instant::now() < deadline, "the person's typing isn't contiguous: {mine:?} in {text:?}");
        std::thread::sleep(Duration::from_millis(20));
    };
    let start = text.find(&mine).unwrap();
    assert_eq!(carets(&mut client)[0].1 as usize, text[..start + mine.len()].chars().count(), "the person's caret is at the end of their typing");
    assert_eq!(text.matches("- note").count(), pushes, "every push landed once");

    // The person's undo takes back their typing and none of the pushes.
    let want = text.replacen(&mine, "Hey there, ", 1);
    for _ in 0..typed.len() + 5 {
        pty.send(b"\x1a"); // ctrl-z
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = client.ask(json!({"op": "state.get", "history": false}));
        if got["result"]["state"]["text"] == json!(want) {
            break;
        }
        assert!(Instant::now() < deadline, "undo: {} != {want:?}", got["result"]["state"]["text"]);
        std::thread::sleep(Duration::from_millis(20));
    }

    // The trace replays to the state the editor holds.
    let got = client.ask(json!({"op": "state.get"}));
    let replayed = bin().arg("--replay").arg(&trace).args(["--dump-state", "-"]).output().unwrap();
    assert!(replayed.status.success(), "{}", String::from_utf8_lossy(&replayed.stderr));
    let replayed: Value = serde_json::from_slice(&replayed.stdout).unwrap();
    assert_eq!(replayed, got["result"]["state"]);

    // caretline send set-text: one connection, its own view, the person's caret stays.
    let before = carets(&mut client)[0].1;
    let cur = client.ask(json!({"op": "state.get", "history": false}))["result"]["state"]["text"].as_str().unwrap().to_string();
    let file = dir.join("new.md");
    std::fs::write(&file, format!("{cur}tail from send\n")).unwrap();
    let out = bin().args(["send", "--socket"]).arg(&sock).arg("set-text").arg(&file).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let now = client.ask(json!({"op": "state.get", "history": false}));
    assert_eq!(now["result"]["state"]["text"], json!(format!("{cur}tail from send\n")));
    // Send's view closes with its connection: view 0 and this client's own are left.
    let deadline = Instant::now() + Duration::from_secs(10);
    while carets(&mut client).len() > 2 {
        assert!(Instant::now() < deadline, "send's view stayed open");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(carets(&mut client)[0].1, before);
    let _ = std::fs::remove_dir_all(&dir);
}
