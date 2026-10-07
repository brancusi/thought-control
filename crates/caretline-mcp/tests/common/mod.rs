//! Shared test helpers: the caretline binary, a live editor on a pseudo-terminal (borrowed
//! from caretline-cli's tests/live.rs), and a minimal MCP client over the server's stdio.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub const ROWS: u16 = 12;
pub const COLS: u16 = 60;

pub struct Pty {
    pub master: std::fs::File,
    pub out: Arc<Mutex<Vec<u8>>>,
    pub child: std::process::Child,
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
    pub fn spawn(mut c: Command) -> Pty {
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

    pub fn send(&mut self, b: &[u8]) {
        self.master.write_all(b).unwrap();
    }

    pub fn screen(&self) -> String {
        screen(&self.out.lock().unwrap(), ROWS as usize, COLS as usize).join("\n")
    }

    pub fn wait(&self, what: &str, ok: impl Fn(&str) -> bool) -> String {
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


/// The `caretline` binary (another package's), built once per test run. `CARETLINE_BIN`
/// overrides it.
pub fn caretline() -> PathBuf {
    static BUILD: Once = Once::new();
    if let Ok(p) = std::env::var("CARETLINE_BIN") {
        return PathBuf::from(p);
    }
    let mcp = PathBuf::from(env!("CARGO_BIN_EXE_caretline-mcp"));
    let bin = mcp.with_file_name("caretline");
    BUILD.call_once(|| {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut c = Command::new(cargo);
        c.args(["build", "-q", "-p", "caretline-cli", "--bin", "caretline"]).current_dir(env!("CARGO_MANIFEST_DIR"));
        if mcp.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "release") {
            c.arg("--release");
        }
        let st = c.status().expect("cargo build caretline");
        assert!(st.success(), "building caretline failed");
    });
    assert!(bin.exists(), "{} missing", bin.display());
    bin
}

/// A short scratch directory (socket paths must stay short), used as TMPDIR so the editors
/// a test starts are the only ones its server discovers.
pub fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from("/tmp").join(format!("clm-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A live editor on `file` in a pseudo-terminal, listening on `dir/ed.sock`.
pub fn live_editor(dir: &Path, file: &Path) -> Pty {
    let mut c = Command::new(caretline());
    c.arg(file).arg("--listen").arg(dir.join("ed.sock")).arg("--no-mouse").env("TMPDIR", dir).current_dir(dir);
    let pty = Pty::spawn(c);
    pty.wait("the editor", |s| s.contains("listening on"));
    pty
}

/// An MCP client over a child's stdio: JSON-RPC requests, one per line.
pub struct Mcp {
    pub child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Mcp {
    /// Starts `caretline-mcp ARGS` with TMPDIR = `dir` and runs the initialize handshake.
    pub fn start(dir: &Path, args: &[&str]) -> Mcp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_caretline-mcp"))
            .args(args)
            .env("TMPDIR", dir)
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut m = Mcp { child, stdin, stdout, next: 1 };
        let init = m.request(
            "initialize",
            json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "test-agent", "version": "0"}}),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "caretline-mcp", "{init}");
        m.notify("notifications/initialized", json!({}));
        m
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        writeln!(self.stdin, "{}", json!({"jsonrpc": "2.0", "method": method, "params": params})).unwrap();
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        writeln!(self.stdin, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap();
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "the server closed stdout");
            let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
            if v["id"] == json!(id) {
                return v;
            }
        }
    }

    /// Calls a tool; returns (is_error, structured result).
    pub fn tool(&mut self, name: &str, args: Value) -> (bool, Value) {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        assert!(r.get("error").is_none(), "{name}: protocol error {r}");
        let res = &r["result"];
        (res["isError"] == json!(true), res["structuredContent"].clone())
    }

    /// Calls a tool that must succeed.
    pub fn ok(&mut self, name: &str, args: Value) -> Value {
        let (err, v) = self.tool(name, args.clone());
        assert!(!err, "{name} {args} failed: {v}");
        v
    }
}

/// Waits until `f` holds.
pub fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
