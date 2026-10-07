//! `caretline bench`: protocol throughput and latency, in process and over a Unix socket.
//! Build with `--release` for meaningful numbers.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use caretline_next::{By, Dir, Msg, Session, State, Viewport};

use crate::hub;

/// Alternating edits, so the document stays the same size.
const EDITS: [&str; 2] = [
    r#"{"id":1,"op":"msgs","msgs":[{"msg":"insert_text","text":"x"}]}"#,
    r#"{"id":1,"op":"msgs","msgs":[{"msg":"delete_backward"}]}"#,
];

/// The same edits as a key script.
const KEY_EDITS: [&str; 2] = [r#"{"id":1,"op":"keys","keys":"x"}"#, r#"{"id":1,"op":"keys","keys":"<bs>"}"#];

fn doc(lines: usize) -> String {
    let mut s = String::with_capacity(lines * 64);
    for i in 0..lines {
        s.push_str(&format!("{i:06} the quick brown fox jumps over the lazy dog, again and again\n"));
    }
    s
}

fn session(lines: usize) -> Session {
    Session::new(State::new(&doc(lines), None, Viewport { width: 100, height: 40 }))
}

fn fmt(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us < 1000.0 {
        format!("{us:.1} µs")
    } else {
        format!("{:.2} ms", us / 1000.0)
    }
}

/// Median and 99th percentile of `n` runs of `f`.
fn time(n: usize, mut f: impl FnMut()) -> (Duration, Duration) {
    let mut v: Vec<Duration> = (0..n)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .collect();
    v.sort();
    (v[n / 2], v[(n * 99 / 100).min(n - 1)])
}

fn row(label: &str, (p50, p99): (Duration, Duration)) {
    println!("  {label:<44} p50 {:>10}   p99 {:>10}", fmt(p50), fmt(p99));
}

struct Conn {
    w: UnixStream,
    r: BufReader<UnixStream>,
    line: String,
}

impl Conn {
    fn ask(&mut self, req: &str) {
        self.w.write_all(req.as_bytes()).unwrap();
        self.w.write_all(b"\n").unwrap();
        self.line.clear();
        self.r.read_line(&mut self.line).unwrap();
    }
}

fn server(lines: usize, tag: &str) -> (Conn, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("caretline-bench-{}-{tag}.sock", std::process::id()));
    let (tx, rx) = std::sync::mpsc::channel();
    let listening = hub::listen(&path, tx).expect("bind");
    let hub = hub::Hub::new(session(lines), None);
    std::thread::spawn(move || {
        let _keep = listening;
        hub::serve(hub, rx, true);
    });
    let s = UnixStream::connect(&path).expect("connect");
    let conn = Conn { w: s.try_clone().unwrap(), r: BufReader::new(s), line: String::new() };
    (conn, path)
}

pub fn main(argv: &[String]) -> Result<(), String> {
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        println!("caretline bench: protocol throughput and latency (build with --release)");
        return Ok(());
    }
    if cfg!(debug_assertions) {
        println!("(debug build: numbers are much slower than release)");
    }

    println!("In-process Session (1,000-line document)");
    let mut s = session(1000);
    let cycle = [
        Msg::InsertText { text: "a".into() },
        Msg::Move { dir: Dir::Forward, by: By::Grapheme, extend: false },
        Msg::DeleteBackward,
        Msg::Move { dir: Dir::Forward, by: By::VisualLine, extend: false },
    ];
    let n = 200_000;
    let t = Instant::now();
    for i in 0..n {
        s.apply(cycle[i % cycle.len()].clone());
    }
    let el = t.elapsed();
    println!("  Session::apply                               {:>10.0} msgs/s  ({} per msg)", n as f64 / el.as_secs_f64(), fmt(el / n as u32));
    for msg in [
        Msg::Tick { now_ms: 1 },
        Msg::Move { dir: Dir::Forward, by: By::Grapheme, extend: false },
        Msg::Move { dir: Dir::Forward, by: By::VisualLine, extend: false },
    ] {
        let mut s = session(1000);
        let label = format!("  apply {}", serde_json::to_string(&msg).unwrap());
        row(&label, time(5000, || drop(s.apply(msg.clone()))));
    }
    let lines: Vec<String> = cycle
        .iter()
        .map(|m| format!("{{\"op\":\"msgs\",\"msgs\":[{}]}}", serde_json::to_string(m).unwrap()))
        .collect();
    let mut s = session(1000);
    let n = 100_000;
    let t = Instant::now();
    for i in 0..n {
        s.handle(&lines[i % lines.len()], None);
    }
    let el = t.elapsed();
    println!("  Session::handle (JSON request and response)  {:>10.0} req/s   ({} per request)", n as f64 / el.as_secs_f64(), fmt(el / n as u32));

    println!("One typing run, one insert_text request per char (no clock), into an empty document");
    for pattern in ["x", "The quick brown fox jumps over the lazy dog. "] {
        let chars: Vec<char> = pattern.chars().collect();
        let label = if pattern == "x" { "one long line" } else { "prose" };
        for n in [1_000usize, 4_000, 16_000, 64_000] {
            let mut s = Session::new(State::new("", None, Viewport { width: 100, height: 40 }));
            let reqs: Vec<String> = (0..n)
                .map(|i| {
                    let msg = Msg::InsertText { text: chars[i % chars.len()].to_string() };
                    format!("{{\"op\":\"msgs\",\"msgs\":[{}]}}", serde_json::to_string(&msg).unwrap())
                })
                .collect();
            let t = Instant::now();
            for r in &reqs {
                s.handle(r, None);
            }
            let el = t.elapsed();
            println!("  {label:<14} {n:>6} chars: {:>9}  ({} per char)", fmt(el), fmt(el / n as u32));
        }
    }

    for lines in [1_000usize, 100_000] {
        let mut s = session(lines);
        println!("In-process, {lines}-line document");
        row("state.get", time(200, || drop(s.handle(r#"{"op":"state.get"}"#, None))));
        row("render 100x40 text", time(500, || drop(s.handle(r#"{"op":"render"}"#, None))));
        row("render 100x40 cells", time(500, || drop(s.handle(r#"{"op":"render","format":"cells"}"#, None))));
        let mut i = 0;
        row("msgs, one edit (type or backspace)", time(2000, || {
            i += 1;
            drop(s.handle(EDITS[i % 2], None))
        }));
        let mut i = 0;
        row("keys, the same edit (x or <bs>)", time(2000, || {
            i += 1;
            drop(s.handle(KEY_EDITS[i % 2], None))
        }));
    }

    for lines in [1_000usize, 100_000] {
        let (mut c, path) = server(lines, &lines.to_string());
        println!("Unix socket round trip, {lines}-line document");
        row("hello", time(5000, || c.ask(r#"{"id":1,"op":"hello"}"#)));
        let mut i = 0;
        row("msgs, one edit (type or backspace)", time(5000, || {
            i += 1;
            c.ask(EDITS[i % 2])
        }));
        row("keys \"<down>\"", time(5000, || c.ask(r#"{"id":1,"op":"keys","keys":"<down>"}"#)));
        row("msgs, move down (what <down> becomes)", time(5000, || c.ask(r#"{"id":1,"op":"msgs","msgs":[{"msg":"move","dir":"forward","by":"visual_line"}]}"#)));
        let mut i = 0;
        row("keys, one edit (x or <bs>)", time(5000, || {
            i += 1;
            c.ask(KEY_EDITS[i % 2])
        }));
        row("render 100x40 text", time(2000, || c.ask(r#"{"id":1,"op":"render"}"#)));
        row("render 100x40 cells", time(2000, || c.ask(r#"{"id":1,"op":"render","format":"cells"}"#)));
        row("state.get", time(100, || c.ask(r#"{"id":1,"op":"state.get"}"#)));
        println!("  (state.get response: {:.1} KB)", c.line.len() as f64 / 1024.0);
        drop(c);
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}
