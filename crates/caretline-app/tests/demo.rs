//! `caretline demo`: each demo starts, draws its first frame headless, and runs live on a
//! pseudo-terminal; the agent's writes are guarded and the person's undo spares them.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ROWS: u16 = 30;
const COLS: u16 = 90;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_caretline"))
}

fn run(args: &[&str]) -> String {
    let out = bin().args(args).output().expect("caretline runs");
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// A scratch directory for one test (TMPDIR for sockets, and the demo's files).
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cl-demo-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn demo_help_lists_the_demos() {
    let help = run(&["demo", "--help"]);
    for d in ["tour", "scenes", "agent"] {
        assert!(help.contains(&format!("  {d} ")), "{d} missing:\n{help}");
    }
}

#[test]
fn each_demo_draws_its_first_frame_headless() {
    let tour = run(&["demo", "--snapshot", "80x24"]);
    assert!(tour.contains("Welcome to caretline"), "{tour}");
    assert!(tour.lines().last().unwrap().contains("1/6 · "), "the hint names step 1:\n{tour}");
    assert_eq!(tour, run(&["demo", "tour", "--snapshot", "80x24"]));

    let agent = run(&["demo", "agent", "--snapshot", "80x30"]);
    assert_eq!(agent.lines().count(), 30);
    assert!(agent.contains("## Yours") || agent.contains("Yours"), "{agent}");
    assert!(agent.contains("agent · connecting"), "the pane's status bar:\n{agent}");

    let scenes = run(&["demo", "scenes", "--snapshot", "80x24"]);
    assert!(scenes.contains("C A R E T L I N E"), "{scenes}");
    assert!(scenes.contains("warp · 1/6"), "{scenes}");
    let donut = run(&["demo", "scenes", "--scene", "donut", "--snapshot", "60x20"]);
    assert!(donut.contains("donut · 1/1"), "{donut}");
    assert!(run(&["demo", "--snapshot", "80x24", "--format", "ansi"]).contains("\x1b["));
}

#[test]
fn the_agent_writes_with_if_rev_and_undo_spares_it() {
    let out = bin().args(["demo", "agent", "--headless"]).env("TMPDIR", scratch("headless")).output().unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    assert!(out.status.success(), "{report:#}");
    assert_eq!(report["ok"], true);
    assert!(report["agent"]["stale_retried"].as_u64().unwrap() > 0, "the person's keys forced stale writes");
    assert_eq!(report["agent"]["writes"], report["agent"]["chars"]);
    assert_eq!(report["undo_kept_only_the_agents_text"], true);
    assert!(report["text"].as_str().unwrap().contains("Draft the release notes"));
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
        let deadline = Instant::now() + Duration::from_secs(20);
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


fn spawn_demo(args: &[&str], tmp: &PathBuf) -> Pty {
    let mut c = bin();
    c.args(args).env("TMPDIR", tmp);
    Pty::spawn(c)
}

fn exited(pty: &mut Pty) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(s)) = pty.child.try_wait() {
            return s.success();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn the_tour_runs_live_dumps_and_replays() {
    let tmp = scratch("tour");
    let dir = tmp.join("files");
    let mut pty = spawn_demo(&["demo", "--dir", dir.to_str().unwrap()], &tmp);
    pty.wait("the first hint", |s| s.contains("1/6 · "));
    pty.send(b" and more");
    pty.wait("typing", |s| s.contains("and more"));
    pty.send(b"\x04"); // Ctrl-D
    pty.wait("the dump", |s| s.contains("state.json: "));
    let dumped = std::fs::read_to_string(dir.join("state.json")).unwrap();
    assert!(dumped.contains("and more"));
    pty.send(b"\x10"); // Ctrl-P
    pty.wait("the replay to finish", |s| s.contains("replayed") && s.contains("identical state"));
    pty.send(b"\x11\x11"); // Ctrl-Q twice: leave without saving
    assert!(exited(&mut pty), "quit");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn the_agent_demo_types_beside_the_person() {
    let tmp = scratch("agent");
    let dir = tmp.join("files");
    let mut pty = spawn_demo(&["demo", "agent", "--dir", dir.to_str().unwrap()], &tmp);
    pty.wait("the editor", |s| s.contains("Yours"));
    pty.send(b"mine");
    pty.wait("the agent's typing in its pane", |s| s.contains("Hello! You're in"));
    let s = pty.wait("the person's text", |s| s.contains("mine"));
    assert!(s.contains("agent · "), "the pane's status names the agent:\n{s}");
    pty.send(b"\x11\x11");
    assert!(exited(&mut pty), "quit");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn the_scenes_play_in_the_editor() {
    let tmp = scratch("scenes");
    let mut pty = spawn_demo(&["demo", "scenes"], &tmp);
    pty.wait("the warp field", |s| s.contains("C A R E T L I N E") && s.contains("warp · 1/6"));
    pty.send(b"\x1b[C"); // right: the next scene
    pty.wait("the donut", |s| s.contains("donut · 2/6"));
    pty.send(b"q");
    assert!(exited(&mut pty), "q quits");
    let _ = std::fs::remove_dir_all(&tmp);
}
