//! The state protocol through the binary: `caretline serve` on stdio and on a socket, and
//! `caretline send`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_caretline"))
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn scratch(name: &str) -> PathBuf {
    // Short: socket paths are limited to about 100 bytes.
    let dir = PathBuf::from("/tmp").join(format!("clp-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs `caretline serve ARGS` with these request lines on stdin; the output lines.
fn serve_stdio(args: &[&str], requests: &[Value]) -> Vec<Value> {
    let mut child = bin()
        .arg("serve")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect()
}

#[test]
fn serve_on_stdio_answers_every_request_in_order() {
    let out = serve_stdio(
        &["--size", "30x4"],
        &[
            json!({"id": 1, "op": "hello"}),
            json!({"id": 2, "op": "keys", "keys": "abc"}),
            json!({"id": 3, "op": "msgs", "msgs": [{"msg": "select_all"}, {"msg": "copy"}]}),
            json!({"id": 4, "op": "render"}),
            json!({"id": 5, "op": "bogus"}),
            json!({"id": 6, "op": "state.get"}),
            json!({"id": 7, "op": "trace.get"}),
        ],
    );
    let ids: Vec<i64> = out.iter().map(|v| v["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, [1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(out[0]["result"]["proto"], 1);
    assert_eq!(out[1]["result"]["rev"], 3);
    assert_eq!(out[2]["result"]["effects"][0], json!({"effect": "clipboard_set", "text": "abc"}));
    assert!(out[3]["result"]["frame"].as_str().unwrap().starts_with("abc\n"));
    assert_eq!(out[4]["error"]["kind"], "unknown_op");
    assert_eq!(out[5]["result"]["state"]["text"], "abc");
    assert_eq!(out[6]["result"]["trace"].as_array().unwrap().len(), 6);
}

#[test]
fn serve_loads_a_file_and_never_writes_it() {
    let dir = scratch("file");
    let doc = dir.join("doc.md");
    std::fs::write(&doc, "on disk\n").unwrap();
    let out = serve_stdio(
        &[doc.to_str().unwrap()],
        &[json!({"op": "keys", "keys": "X<c-s>"}), json!({"op": "state.get"})],
    );
    assert_eq!(out[0]["result"]["effects"][0]["effect"], "write_file");
    assert_eq!(out[1]["result"]["state"]["text"], "Xon disk\n");
    assert_eq!(std::fs::read_to_string(&doc).unwrap(), "on disk\n");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn state_set_then_render_matches_snapshot_byte_for_byte() {
    let mut seen = 0;
    for entry in std::fs::read_dir(fixtures()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".state.json") {
            continue;
        }
        let state: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let (w, h) = (state["viewport"]["width"].as_u64().unwrap(), state["viewport"]["height"].as_u64().unwrap());
        for (format, size) in [("text", (w, h)), ("ansi", (w, h)), ("text", (w - 3, h + 2))] {
            let snap = bin()
                .arg("--state")
                .arg(&path)
                .args(["--snapshot", &format!("{}x{}", size.0, size.1), "--format", format])
                .output()
                .unwrap();
            let want = String::from_utf8(snap.stdout).unwrap();
            let out = serve_stdio(
                &[],
                &[
                    json!({"op": "state.set", "state": state}),
                    json!({"op": "render", "w": size.0, "h": size.1, "format": format}),
                ],
            );
            assert_eq!(out[1]["result"]["frame"].as_str().unwrap(), want, "{name} {format} {size:?}");
        }
        seen += 1;
    }
    assert!(seen >= 4);
}

struct Server {
    child: Child,
    socket: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(dir) = self.socket.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn wait_for_socket(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while UnixStream::connect(path).is_err() {
        assert!(Instant::now() < deadline, "no server at {}", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn socket_server(name: &str, args: &[&str]) -> Server {
    let socket = scratch(name).join("s.sock");
    let child = bin().arg("serve").args(args).arg("--socket").arg(&socket).spawn().unwrap();
    wait_for_socket(&socket);
    Server { child, socket }
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
    fn send(&mut self, req: Value) {
        writeln!(self.w, "{req}").unwrap();
    }
    fn line(&mut self) -> Value {
        let mut l = String::new();
        self.r.read_line(&mut l).unwrap();
        serde_json::from_str(&l).unwrap_or_else(|e| panic!("{e}: {l:?}"))
    }
    fn ask(&mut self, req: Value) -> Value {
        self.send(req);
        self.line()
    }
}

#[test]
fn two_clients_share_one_session_and_see_each_others_changes() {
    let server = socket_server("two", &["--size", "40x5"]);
    let mut a = Client::connect(&server.socket);
    let mut b = Client::connect(&server.socket);
    let r = a.ask(json!({"id": "sub", "op": "subscribe", "frame": {"format": "text"}}));
    assert_eq!(r["result"]["subscribed"], true);

    let r = b.ask(json!({"id": 1, "op": "msgs", "msgs": [{"msg": "insert_text", "text": "from b"}]}));
    assert_eq!(r["result"]["rev"], 1);
    let ev = a.line();
    assert_eq!((ev["event"].as_str(), ev["rev"].as_u64(), ev["source"].as_str()), (Some("state"), Some(1), Some("client")));
    assert_eq!(ev["msgs"][0]["text"], "from b");
    assert!(ev["frame"]["frame"].as_str().unwrap().starts_with("from b\n"));

    // a's own change: its response comes first, then its event.
    let r = a.ask(json!({"id": 2, "op": "keys", "keys": "!"}));
    assert_eq!(r["id"], 2);
    assert_eq!(r["result"]["rev"], 2);
    let ev = a.line();
    assert_eq!(ev["rev"], 2);

    let r = b.ask(json!({"op": "state.get"}));
    assert_eq!(r["result"]["state"]["text"], "from b!");
    assert_eq!(r["result"]["rev"], 2);

    // Revs are strictly increasing across interleaved writers.
    let mut revs = Vec::new();
    for i in 0..20 {
        let c = if i % 2 == 0 { &mut a } else { &mut b };
        c.send(json!({"id": i, "op": "msgs", "msgs": [{"msg": "insert_text", "text": "."}]}));
        loop {
            let v = c.line();
            if v.get("event").is_none() {
                revs.push(v["result"]["rev"].as_u64().unwrap());
                break;
            }
        }
    }
    assert!(revs.windows(2).all(|w| w[0] < w[1]), "{revs:?}");

    // b's disconnect doesn't disturb a.
    drop(b);
    let mut r = a.ask(json!({"id": "end", "op": "hello"}));
    // Skip the events for the writes above.
    while r.get("event").is_some() {
        r = a.line();
    }
    assert_eq!(r["result"]["rev"], 22);
}

#[test]
fn send_talks_to_a_socket() {
    let server = socket_server("send", &["--size", "20x3"]);
    let sock = server.socket.to_str().unwrap();
    let send = |args: &[&str]| {
        let out = bin().args(["send", "--socket", sock]).args(args).output().unwrap();
        (out.status.success(), String::from_utf8(out.stdout).unwrap())
    };
    let (ok, out) = send(&["keys", "hi<cr>there"]);
    assert!(ok, "{out}");
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["result"]["rev"], 8);
    let (_, frame) = send(&["render", "20x3", "--raw"]);
    assert_eq!(frame.lines().take(2).collect::<Vec<_>>(), ["hi", "there"]);
    let (_, state) = send(&["state.get", "--raw"]);
    let st: Value = serde_json::from_str(&state).unwrap();
    assert_eq!(st["text"], "hi\nthere");

    // set-state from a file, and raw JSON requests from stdin.
    let file = server.socket.with_file_name("s.json");
    std::fs::write(&file, std::fs::read_to_string(fixtures().join("emoji-line.state.json")).unwrap()).unwrap();
    let (ok, _) = send(&["set-state", file.to_str().unwrap()]);
    assert!(ok);
    let mut child = bin()
        .args(["send", "--socket", sock])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.as_mut().unwrap(), "{}", json!({"id": 1, "op": "hello"})).unwrap();
    writeln!(child.stdin.as_mut().unwrap(), "{}", json!({"id": 2, "op": "state.get"})).unwrap();
    let out = child.wait_with_output().unwrap();
    let lines: Vec<Value> = String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["result"]["state"]["path"], "emoji.md");

    // A failing request exits non-zero.
    let (ok, out) = send(&["{\"op\":\"nope\"}"]);
    assert!(!ok);
    assert!(out.contains("unknown_op"));
}
