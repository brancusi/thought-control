//! A running TUI and daemon follow the vault registry live: an agent's
//! `thc vault new`, `rename` and `rm` show in an open picker without reopening it, and a new
//! vault is served by the daemon without a restart.
#![cfg(unix)]

mod common;

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The live-update budget: half a second's poll plus a frame, with slack for a loaded CI box.
const BUDGET: Duration = Duration::from_secs(3);

struct H {
    home: PathBuf,
}

impl H {
    fn new(name: &str) -> H {
        let home = std::env::temp_dir().join(format!("thc-livereg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let h = H { home };
        h.ok(&["init", "thought", "--global"]);
        h.ok(&["vault", "new", "acme"]);
        h
    }

    fn command(&self) -> std::process::Command {
        let mut c = common::thc();
        c.current_dir(&self.home).env("HOME", &self.home).env_remove("THC_VAULT").env_remove("THC_CACHE_DIR").env_remove("THC_CONFIG_DIR").env_remove("THC_ACTOR");
        c
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.command().args(args).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn live(&self, vault: &str) -> bool {
        let o = self.command().args(["--json", "--vault", vault, "daemon", "status"]).output().unwrap();
        serde_json::from_slice::<serde_json::Value>(&o.stdout).is_ok_and(|s| s["state"] == "live")
    }
}

impl Drop for H {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

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
    fn spawn(mut c: std::process::Command, rows: u16, cols: u16) -> Pty {
        let (mut m, mut s) = (0, 0);
        let mut ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        assert_eq!(unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) }, 0);
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
                if b[..n].windows(4).any(|x| x == b"\x1b[?u") {
                    let _ = w.write_all(b"\x1b[?0u\x1b[?62;22c");
                }
                o.lock().unwrap().extend_from_slice(&b[..n]);
            }
        });
        Pty { master, out, child }
    }

    fn send(&mut self, b: &[u8]) {
        self.master.write_all(b).unwrap();
        std::thread::sleep(Duration::from_millis(80));
    }

    fn screen(&self) -> String {
        screen(&self.out.lock().unwrap(), 30, 100).join("\n")
    }

    /// Wait until the screen satisfies `ok`, within `within`; the screen either way.
    fn wait(&self, within: Duration, ok: impl Fn(&str) -> bool) -> Result<String, String> {
        let deadline = Instant::now() + within;
        loop {
            let s = self.screen();
            if ok(&s) {
                return Ok(s);
            }
            if Instant::now() >= deadline {
                return Err(s);
            }
            std::thread::sleep(Duration::from_millis(50));
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

/// The picker's rows (each shows its sync state).
fn picker_rows(s: &str) -> Vec<String> {
    s.lines().filter(|l| l.contains("local only")).map(|l| l.trim().to_string()).collect()
}

fn has_row(s: &str, name: &str) -> bool {
    picker_rows(s).iter().any(|r| r.split_whitespace().any(|w| w == name))
}

fn picker_follows_the_registry(h: &H) {
    let mut c = h.command();
    c.arg("tui");
    let mut p = Pty::spawn(c, 30, 100);
    p.wait(Duration::from_secs(10), |s| s.contains("Today")).expect("the TUI started");
    p.send(b"V");
    let s = p.wait(Duration::from_secs(5), |s| has_row(s, "acme")).unwrap_or_else(|s| panic!("the picker opened: {s}"));
    assert!(!has_row(&s, "agentvault"));
    // An agent makes a vault while the picker is open: it appears, and the counts with it.
    h.ok(&["vault", "new", "agentvault"]);
    p.wait(BUDGET, |s| has_row(s, "agentvault")).unwrap_or_else(|s| panic!("the new vault never appeared: {s}"));
    // Renamed: the row follows.
    h.ok(&["vault", "rename", "agentvault", "agentvault2"]);
    p.wait(BUDGET, |s| has_row(s, "agentvault2") && !has_row(s, "agentvault")).unwrap_or_else(|s| panic!("the rename never showed: {s}"));
    // Removed: the row goes; the picker stays open on the others.
    h.ok(&["vault", "rm", "agentvault2"]);
    p.wait(BUDGET, |s| !has_row(s, "agentvault2") && has_row(s, "acme")).unwrap_or_else(|s| panic!("the removal never showed: {s}"));
}

#[test]
fn an_open_picker_follows_new_renamed_and_removed_vaults() {
    let h = H::new("tui");
    picker_follows_the_registry(&h);
}

#[test]
fn the_daemon_serves_a_new_vault_at_once_and_the_picker_still_follows() {
    let h = H::new("daemon");
    let mut c = h.command();
    c.args(["daemon", "run"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let _daemon = common::Guard::spawn(&mut c);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !(h.live("personal") && h.live("acme")) {
        assert!(Instant::now() < deadline, "the daemon never went live");
        std::thread::sleep(Duration::from_millis(200));
    }
    // A vault made now is served without a restart, well inside the old 30 s rescan.
    h.ok(&["vault", "new", "fresh"]);
    let t = Instant::now();
    while !h.live("fresh") {
        assert!(t.elapsed() < BUDGET, "the daemon didn't serve the new vault within {BUDGET:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
    picker_follows_the_registry(&h);
    let _ = h.command().args(["daemon", "stop"]).output();
}

