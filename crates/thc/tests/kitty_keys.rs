//! Real terminal input, end to end: `thc j` runs in a pty that answers as a kitty-protocol
//! terminal (as WezTerm with enable_kitty_keyboard, kitty, Ghostty), and keys arrive encoded
//! for the flags thc pushed. Snapshot tests inject decoded keys and can't see encoding bugs:
//! under "report all keys" without "alternate keys", ⇧h arrived as `h` and capitals were lost.
#![cfg(unix)]

mod common;

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Pty {
    master: std::fs::File,
    out: Arc<Mutex<Vec<u8>>>,
    child: std::process::Child,
}

/// A failing assert must never leave the TUI running (one sat on a pty for 19 hours).
impl Drop for Pty {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Pty {
    fn spawn(root: &Path, args: &[&str]) -> Pty {
        Self::spawn_env(root, args, &[])
    }

    fn spawn_env(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Pty {
        Self::spawn_with(common::thc(), root, args, env)
    }

    fn spawn_with(mut c: std::process::Command, root: &Path, args: &[&str], env: &[(&str, &str)]) -> Pty {
        let (mut m, mut s) = (0, 0);
        let mut ws = libc::winsize { ws_row: 30, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
        assert_eq!(unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) }, 0);
        let slave = unsafe { OwnedFd::from_raw_fd(s) };
        c.args(args).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env("TERM", "xterm-256color").env_remove("THC_ACTOR");
        for (k, v) in env {
            c.env(k, v);
        }
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
                // crossterm asks for the kitty flags (CSI ? u) and then DA1: answer as a kitty terminal.
                if b[..n].windows(4).any(|x| x == b"\x1b[?u") {
                    let _ = w.write_all(b"\x1b[?0u\x1b[?62;22c");
                }
                o.lock().unwrap().extend_from_slice(&b[..n]);
            }
        });
        Pty { master, out, child }
    }

    /// The flags thc pushed (`CSI > n u`), once it has.
    fn flags(&self) -> Option<u32> {
        let o = self.out.lock().unwrap();
        let s = String::from_utf8_lossy(&o);
        s.rfind("\x1b[>").and_then(|i| s[i + 3..].split('u').next()?.parse().ok())
    }

    fn send(&mut self, b: &[u8]) {
        self.master.write_all(b).unwrap();
        std::thread::sleep(Duration::from_millis(80));
    }
}

/// A key as WezTerm 20240203 encodes it under the kitty `flags` (a port of its
/// `KeyEvent::encode_kitty` for letters): plain text when no modifiers and no "report all keys"
/// (8); the legacy text form while "alternate keys" (4) is off; otherwise CSI u with the
/// unshifted key, the shifted one as the alternate (flag 4), and modifiers taken from the *raw*
/// event: `raw_shift` false is what turned ⇧a into `a` in WezTerm.
fn wezterm(flags: u32, ch: char, shift: bool, ctrl: bool, raw_shift: bool) -> Vec<u8> {
    let shifted = if shift { ch.to_ascii_uppercase() } else { ch };
    if !shift && !ctrl && flags & 8 == 0 {
        return shifted.to_string().into_bytes();
    }
    let mods = 1 + (shift && raw_shift) as u32 + 4 * ctrl as u32;
    let legacy = flags & 4 == 0 && !(flags & 1 != 0 && ctrl);
    if legacy {
        return shifted.to_string().into_bytes();
    }
    let base = ch.to_ascii_lowercase();
    let alt = if flags & 4 != 0 && base != shifted { format!(":{}", shifted as u32) } else { String::new() };
    format!("\x1b[{}{alt};{mods}u", base as u32).into_bytes()
}

/// The encoder port reproduces the 0.8.9 bug: under flags 1|4|8 WezTerm sent ⇧h as `h`'s code
/// with no Shift, which crossterm reads as `h`.
#[test]
fn the_wezterm_port_reproduces_the_lost_capital() {
    assert_eq!(wezterm(13, 'h', true, false, false), b"\x1b[104:72;1u".to_vec());
    assert_eq!(wezterm(1, 'h', true, false, false), b"H".to_vec());
}

#[test]
fn capitals_survive_the_kitty_keyboard_protocol() {
    let root = std::env::temp_dir().join(format!("thc-kitty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut init = common::thc();
    assert!(init.current_dir(&root).args(["init"]).arg(root.join("v")).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).status().unwrap().success());

    let mut p = Pty::spawn(&root, &["j", "--no-focus"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    let flags = loop {
        if let Some(f) = p.flags() {
            break f;
        }
        assert!(Instant::now() < deadline, "thc never pushed kitty flags");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(flags, 1, "disambiguate only: printable keys stay text (flags {flags})");
    // What WezTerm sends for these flags, with the raw Shift bit missing as on a real Mac.
    for (ch, shift) in [('h', true), ('i', false), ('x', true), ('y', true)] {
        p.send(&wezterm(flags, ch, shift, false, false));
    }
    std::thread::sleep(Duration::from_millis(1800)); // the idle save
    p.send(&wezterm(flags, 'q', false, true, true));
    let deadline = Instant::now() + Duration::from_secs(5);
    while p.child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = p.child.kill();
            panic!("thc didn't quit on ⌃Q");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut q = common::thc();
    let o = q.args(["--json", "q", "text:hixy"]).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let texts: Vec<&str> = v["items"].as_array().unwrap().iter().filter_map(|i| i["text"].as_str()).collect();
    assert_eq!(texts, ["HiXY"], "what was typed is what was saved");
    let _ = std::fs::remove_dir_all(&root);
}

/// An idle screen writes nothing (ratatui re-showed the cursor on every 250 ms redraw, which
/// restarted its blink), a frame goes out as one synchronized update, and writing shows a bar.
#[test]
fn an_idle_screen_writes_nothing() {
    let root = std::env::temp_dir().join(format!("thc-idle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut init = common::thc();
    assert!(init.current_dir(&root).args(["init"]).arg(root.join("v")).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).status().unwrap().success());
    let mut p = Pty::spawn(&root, &["j", "--no-focus"]);
    // Settled: the first frames are out (under load startup can take seconds).
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last = (0, Instant::now());
    loop {
        let n = p.out.lock().unwrap().len();
        if n != last.0 {
            last = (n, Instant::now());
        } else if n > 0 && last.1.elapsed() > Duration::from_millis(1000) {
            break;
        }
        assert!(Instant::now() < deadline, "thc never settled");
        std::thread::sleep(Duration::from_millis(50));
    }
    // The header's clock legitimately repaints once a minute: one quiet window out of three is
    // proof enough (it caught a minute rollover on CI).
    let mut windows = Vec::new();
    for _ in 0..3 {
        let before = p.out.lock().unwrap().len();
        std::thread::sleep(Duration::from_millis(1500));
        windows.push(p.out.lock().unwrap().len() - before);
        if windows.last() == Some(&0) {
            break;
        }
    }
    assert!(windows.contains(&0), "bytes written while idle, per 1.5 s window: {windows:?}");
    let at = p.out.lock().unwrap().len();
    p.send(b"h");
    std::thread::sleep(Duration::from_millis(400));
    let out = p.out.lock().unwrap().clone();
    let frame = &out[at..];
    assert!(frame.starts_with(b"\x1b[?2026h") && frame.windows(8).any(|w| w == b"\x1b[?2026l"), "a synchronized frame: {:?}", String::from_utf8_lossy(frame));
    assert!(out.windows(5).any(|w| w == b"\x1b[6 q"), "a steady bar cursor while writing (keymap.md §3.1)");
    p.send(b"\x11");
    let deadline = Instant::now() + Duration::from_secs(5);
    while p.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = p.child.kill();
    let _ = std::fs::remove_dir_all(&root);
}

/// mouse.md M9/M10: the mouse modes requested (1000, 1002, 1006; 1003 only with hover, never
/// over SSH by default) and released on quit; none at all with `mouse = false`.
#[test]
fn mouse_modes_are_requested_and_released() {
    let root = std::env::temp_dir().join(format!("thc-mousemodes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut init = common::thc();
    assert!(init.current_dir(&root).args(["init"]).arg(root.join("v")).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).status().unwrap().success());
    let run = |env: &[(&str, &str)]| -> String {
        let mut p = Pty::spawn_env(&root, &["j", "--no-focus"], env);
        std::thread::sleep(Duration::from_millis(1500));
        p.send(b"\x11");
        let deadline = Instant::now() + Duration::from_secs(5);
        while p.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = p.child.kill();
        std::thread::sleep(Duration::from_millis(200));
        let out = p.out.lock().unwrap().clone();
        String::from_utf8_lossy(&out).to_string()
    };
    let out = run(&[("SSH_CONNECTION", "1.2.3.4 5 6.7.8.9 22")]);
    assert!(out.contains("\x1b[?1000h\x1b[?1002h\x1b[?1006h"), "requested");
    assert!(!out.contains("\x1b[?1003h"), "no hover over SSH");
    let on = out.find("\x1b[?1000h").unwrap();
    assert!(out[on..].contains("\x1b[?1000l"), "released on quit");
    let out = run(&[("THC_TUI_MOUSE", "0")]);
    assert!(!out.contains("\x1b[?1000h") && !out.contains("\x1b[?1006h"), "mouse = false: nothing requested");
    let _ = std::fs::remove_dir_all(&root);
}

/// Crash safety (hardening): a panic mid-edit leaves the terminal as the shell expects it
/// (alternate screen left, kitty flags popped, mouse and paste reports off, the cursor shape
/// reset) and keeps what was typed but not saved: recovery.json, put back on the next open.
#[test]
fn a_panic_restores_the_terminal_and_keeps_what_was_typed() {
    let root = std::env::temp_dir().join(format!("thc-crash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cli = |args: &[&str], env: &[(&str, &str)]| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env_remove("THC_ACTOR");
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    };
    assert!(cli(&["init", "v"], &[]).status.success());
    let mut p = Pty::spawn_env(&root, &["j", "--no-focus"], &[("THC_TUI_TEST_PANIC", "1")]);
    let start = Instant::now();
    while p.flags().is_none() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    // Typed, and the panic before the idle save (1.5 s) could write it.
    p.send(b"unsaved words");
    p.send(b"\x1b[24~"); // F12: the test panic
    let deadline = Instant::now() + Duration::from_secs(5);
    while p.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(p.child.try_wait().unwrap().is_some(), "thc exited after the panic");
    std::thread::sleep(Duration::from_millis(200));
    let out = String::from_utf8_lossy(&p.out.lock().unwrap()).to_string();
    let tail = &out[out.rfind("test panic").map_or(0, |i| i.saturating_sub(600))..];
    for (seq, what) in [("\x1b[?1049l", "the alternate screen left"), ("\x1b[<1u", "kitty flags popped"), ("\x1b[?1000l", "mouse reports off"), ("\x1b[?2004l", "paste reports off"), ("\x1b[0 q", "cursor shape reset")] {
        assert!(out.contains(seq), "{what}: {seq:?} not in {:?}", &tail[..tail.len().min(400)]);
    }
    let rec = std::fs::read_to_string(root.join("c/recovery.json")).expect("recovery.json written");
    assert!(rec.contains("unsaved words"), "{rec}");
    // The next open puts the line back (and says so), and it saves; the recovery file goes.
    let o = cli(&["j", "--no-focus"], &[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", ""), ("THC_TUI_SNAPSHOT_WRITE", "1")]);
    let f = String::from_utf8_lossy(&o.stdout);
    assert!(f.contains("unsaved words") && f.contains("recovered 1 unsaved line from a crash"), "{f}");
    assert!(!root.join("c/recovery.json").exists(), "taken once");
    let q = String::from_utf8_lossy(&cli(&["--json", "q", "text:unsaved"], &[]).stdout).to_string();
    assert!(q.contains("unsaved words"), "saved after recovery: {q}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Asked to stop (SIGTERM) or the terminal closing (SIGHUP): thc leaves as ⌃Q does, the line
/// being typed saved, instead of dying mid-line.
#[test]
fn a_hangup_saves_the_line_being_typed() {
    let root = std::env::temp_dir().join(format!("thc-hup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cli = |args: &[&str]| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env_remove("THC_ACTOR");
        c.output().unwrap()
    };
    assert!(cli(&["init", "v"]).status.success());
    let mut p = Pty::spawn(&root, &["j", "--no-focus"]);
    let start = Instant::now();
    while p.flags().is_none() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    p.send(b"half a thought");
    unsafe { libc::kill(p.child.id() as i32, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(5);
    while p.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(p.child.try_wait().unwrap().is_some(), "thc left on SIGTERM");
    let q = String::from_utf8_lossy(&cli(&["--json", "q", "text:thought"]).stdout).to_string();
    assert!(q.contains("half a thought"), "saved on the way out: {q}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Fixed in 0.9.31: a page made in the Pages finder (`+ new page`, ↓ Enter) took typing
/// into a row editor hidden under the document, so nothing showed until Esc. In a real terminal
/// the typed text must reach the screen before any Esc.
#[test]
fn typing_in_a_new_page_shows_at_once() {
    let root = std::env::temp_dir().join(format!("thc-newpage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut init = common::thc();
    assert!(init.current_dir(&root).args(["init"]).arg(root.join("v")).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).status().unwrap().success());
    let mut p = Pty::spawn(&root, &["tui"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while p.flags().is_none() {
        assert!(Instant::now() < deadline, "thc never started");
        std::thread::sleep(Duration::from_millis(50));
    }
    p.send(b"4");
    p.send(b"Zebra notes");
    p.send(b"\x1b[B");
    p.send(b"\r");
    let mark = p.out.lock().unwrap().len();
    p.send(b"quokka");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let shown = String::from_utf8_lossy(&p.out.lock().unwrap()[mark..]).contains("quokka");
        if shown {
            break;
        }
        assert!(Instant::now() < deadline, "typing in the new page never reached the screen");
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(p);
    let _ = std::fs::remove_dir_all(&root);
}

/// motion.md, with real arrow-key bytes: from the document's start (⌃Home), ↓ row by row through
/// a wrapped paragraph and into the next note, then type. 0.9.31 got stuck at the start of the
/// paragraph's last row; what's typed must land at the next note's start.
#[test]
fn arrows_walk_down_a_wrapped_paragraph_into_the_next_note() {
    let root = std::env::temp_dir().join(format!("thc-arrows-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cli = |args: &[&str]| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env_remove("THC_ACTOR");
        c.output().unwrap()
    };
    assert!(cli(&["init", "v"]).status.success());
    let j: serde_json::Value = serde_json::from_slice(&cli(&["--json", "page", "new", "Fox"]).stdout).unwrap();
    let id = j["nodes"][0]["id"].as_str().or(j["id"].as_str()).expect("page id").to_string();
    // Three rows or more at any width thc uses (the column is at most 72).
    let long = "The quick brown fox jumps over the lazy dog and keeps running through the long grass until dusk, then rests under an old oak tree while the sun goes down behind the hills and the evening settles in quietly over everything.";
    assert!(cli(&["add", "--plain", "--under", &id, long]).status.success());
    assert!(cli(&["add", "--plain", "--under", &id, "Short one."]).status.success());
    // How many rows the paragraph wraps to at this terminal's size (the same layout draws both).
    let mut snap = common::thc();
    snap.args(["p", "Fox", "--no-focus"]).current_dir(&root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env("THC_TUI_SNAPSHOT", "100x30");
    let frame = String::from_utf8_lossy(&snap.output().unwrap().stdout).to_string();
    let first = frame.lines().position(|l| l.contains("The quick brown")).expect("the paragraph is drawn");
    let rows = frame.lines().skip(first).take_while(|l| !l.trim().is_empty() && !l.contains("Short one.")).count();
    assert!(rows >= 3, "the paragraph wraps: {frame}");
    let mut p = Pty::spawn(&root, &["p", "Fox", "--no-focus"]);
    let start = Instant::now();
    while p.flags().is_none() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    p.send(b"\x1b[1;5H"); // ⌃Home
    // ↓ once per row of the paragraph: the last one goes into the next note.
    for _ in 0..rows {
        p.send(b"\x1b[B");
    }
    p.send(b"Z");
    std::thread::sleep(Duration::from_millis(1800)); // the idle save
    p.send(b"\x1b[113;5u"); // ⌃Q
    let deadline = Instant::now() + Duration::from_secs(5);
    while p.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let q = String::from_utf8_lossy(&cli(&["--json", "q", "text:Short"]).stdout).to_string();
    assert!(q.contains("\"ZShort one.\""), "typed at the next note's start: {q}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A TUI started before `thc update` ran elsewhere keeps the old code. When the binary on disk
/// changes, the bar says so: `thc X installed · :update reloads here` (checked on focus and once
/// a minute).
#[test]
fn a_running_tui_notices_a_newer_thc_installed_under_it() {
    let root = std::env::temp_dir().join(format!("thc-stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let bin = root.join("bin/thc");
    std::fs::copy(env!("CARGO_BIN_EXE_thc"), &bin).unwrap();
    let mut init = common::thc();
    assert!(init.current_dir(&root).args(["init"]).arg(root.join("v")).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).status().unwrap().success());
    let mut c = std::process::Command::new(&bin);
    common::sandbox(&mut c);
    let mut p = Pty::spawn_with(c, &root, &["tui"], &[]);
    let start = Instant::now();
    while p.flags().is_none() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    // `thc update` elsewhere: a new file at the same path (a rename, as the updater does).
    let next = root.join("bin/thc.next");
    std::fs::write(&next, "#!/bin/sh\necho 'thc 9.9.9'\n").unwrap();
    std::os::unix::fs::PermissionsExt::set_mode(&mut std::fs::metadata(&next).unwrap().permissions(), 0o755);
    std::fs::set_permissions(&next, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::rename(&next, &bin).unwrap();
    let mark = p.out.lock().unwrap().len();
    p.send(b"\x1b[I"); // the terminal regains focus
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let s = String::from_utf8_lossy(&p.out.lock().unwrap()[mark..]).to_string();
        if s.contains("9.9.9 installed") {
            break;
        }
        assert!(Instant::now() < deadline, "no notice of the newer thc: {s}");
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(p);
    let _ = std::fs::remove_dir_all(&root);
}

/// The screen a byte stream draws, as rows of text: a minimal terminal (cursor moves `CSI r;c H`
/// and text; other escapes ignored), enough to read what a frame shows.
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
                    // OSC … BEL or ST
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

/// The view a screen shows: the tab with the heavy rule under it.
fn view_on(screen: &[String]) -> String {
    let (tabs, rule): (Vec<char>, Vec<char>) = (screen[0].chars().collect(), screen[1].chars().collect());
    for name in ["Today", "Inbox", "Tasks", "Pages", "Journal", "Search", "Log"] {
        let t: Vec<char> = name.chars().collect();
        if let Some(i) = tabs.windows(t.len()).position(|w| w == t.as_slice()) {
            if rule.get(i..i + t.len()).is_some_and(|r| r.contains(&'━')) {
                return name.to_string();
            }
        }
    }
    String::new()
}

/// Tab breaks out of any field, even a search with text in it. In a real terminal: Tab and ⇧Tab
/// change view from every view (a parked document included), from the Pages finder after
/// typing, and from the Search input with text.
#[test]
fn tab_always_changes_view_in_a_real_terminal() {
    let root = std::env::temp_dir().join(format!("thc-tabs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut init = common::thc();
    assert!(init.current_dir(&root).args(["init"]).arg(root.join("v")).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).status().unwrap().success());
    let mut p = Pty::spawn(&root, &["tui"]);
    let start = Instant::now();
    while p.flags().is_none() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    let wait_view = |p: &Pty, want: &str| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let sc = screen(&p.out.lock().unwrap(), 30, 100);
            if view_on(&sc) == want {
                return;
            }
            assert!(Instant::now() < deadline, "never reached {want}: on {}\n{}", view_on(&sc), sc.join("\n"));
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    // Tab from Today all the way round: Journal (a document) doesn't trap it; ⇧Tab back.
    for want in ["Inbox", "Tasks", "Pages", "Journal", "Search", "Log", "Today"] {
        p.send(b"\t");
        wait_view(&p, want);
    }
    for want in ["Log", "Search", "Journal", "Pages", "Tasks", "Inbox", "Today"] {
        p.send(b"\x1b[Z");
        wait_view(&p, want);
    }
    // Pages with a query typed, then Tab: the next view.
    p.send(b"4");
    wait_view(&p, "Pages");
    p.send(b"lis");
    p.send(b"\t");
    wait_view(&p, "Journal");
    // Search with text in its input, then ⇧Tab and Tab.
    p.send(b"\t"); // the parked Journal: Tab goes on
    wait_view(&p, "Search");
    p.send(b"/abc");
    p.send(b"\x1b[Z");
    wait_view(&p, "Journal");
    // And back with Tab: Search, its text kept.
    p.send(b"\t");
    wait_view(&p, "Search");
    assert!(screen(&p.out.lock().unwrap(), 30, 100)[2].contains("abc"), "the query is kept");
    drop(p);
    let _ = std::fs::remove_dir_all(&root);
}

/// keymap.md §12.5: the macOS editing keys, as the bytes thc's WezTerm snippet makes
/// them send (`thc setup wezterm`): ⌥← word left, ⌘← Home, ⌘→ End, ⌘↑ C-Home, ⌘⇧→ select to
/// the end, ⌘⌫ ⌃U, ⌥⌫ ESC DEL.
#[test]
fn macos_editing_keys_as_wezterm_sends_them() {
    let root = std::env::temp_dir().join(format!("thc-mackeys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cli = |args: &[&str]| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env_remove("THC_ACTOR");
        c.output().unwrap()
    };
    assert!(cli(&["init", "v"]).status.success());
    let mut p = Pty::spawn(&root, &["j", "--no-focus"]);
    let start = Instant::now();
    while p.flags().is_none() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    p.send(b"alpha beta gamma");
    p.send(b"\x1b[1;3D"); // ⌥← : before gamma
    p.send(b"X");
    p.send(b"\x1b[H"); // ⌘← : line start
    p.send(b"Y");
    p.send(b"\x1b[F"); // ⌘→ : line end
    p.send(b"Z");
    p.send(b"\x1b\x7f"); // ⌥⌫ : delete word back (XgammaZ)
    p.send(b"\r");
    p.send(b"second line");
    p.send(b"\x15"); // ⌘⌫ : delete to line start
    p.send(b"kept");
    p.send(b"\x1b[1;5H"); // ⌘↑ : document start
    p.send(b"S");
    std::thread::sleep(Duration::from_millis(1800)); // the idle save
    p.send(b"\x1b[113;5u"); // ⌃Q
    let deadline = Instant::now() + Duration::from_secs(5);
    while p.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let q = String::from_utf8_lossy(&cli(&["--json", "q", "journal=today"]).stdout).to_string();
    // (Enter in a paragraph is a line break in the same note.)
    assert!(q.contains("\"SYalpha beta \\nkept\""), "⌥← ⌘← ⌘→ ⌥⌫ ⌘↑ ⌘⌫: {q}");
    let _ = std::fs::remove_dir_all(&root);
}

/// attachments.md §3, T4: an attached screenshot draws inline under its chip.
/// WezTerm (and iTerm2) get the iTerm2 protocol, kitty and Ghostty the kitty protocol, and
/// `[tui] images = "chips"` draws none.
#[test]
fn an_attached_image_draws_inline_in_wezterm() {
    let root = std::env::temp_dir().join(format!("thc-inline-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cli = |args: &[&str]| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env_remove("THC_ACTOR");
        c.output().unwrap()
    };
    assert!(cli(&["init", "v"]).status.success());
    // A real (tiny) PNG: 8×6.
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend(8u32.to_be_bytes());
    png.extend(6u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    std::fs::write(root.join("shot.png"), &png).unwrap();
    let j: serde_json::Value = serde_json::from_slice(&cli(&["--json", "page", "new", "Shots"]).stdout).unwrap();
    let page = j["nodes"][0]["id"].as_str().or(j["id"].as_str()).unwrap().to_string();
    assert!(cli(&["add", "--plain", "--under", &page, "above the shot"]).status.success());
    assert!(cli(&["attach", &page, root.join("shot.png").to_str().unwrap(), "--caption", "the shot"]).status.success());
    let shows = |env: &[(&str, &str)], needle: &[u8]| -> bool {
        let p = Pty::spawn_env(&root, &["p", "Shots", "--no-focus"], env);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if p.out.lock().unwrap().windows(needle.len()).any(|w| w == needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    };
    assert!(shows(&[("TERM_PROGRAM", "WezTerm")], b"\x1b]1337;File=inline=1"), "WezTerm: the iTerm2 image protocol");
    assert!(shows(&[("TERM_PROGRAM", "ghostty")], b"\x1b_Gf=100,a=T"), "Ghostty: the kitty graphics protocol");
    std::fs::write(root.join("v/settings.toml"), "[tui]\nimages = \"chips\"\n").unwrap();
    assert!(!shows(&[("TERM_PROGRAM", "WezTerm")], b"\x1b]1337;File="), "images = chips: none");
    let _ = std::fs::remove_dir_all(&root);
}

/// Two workflows, with real bytes and a fake pasteboard (never the real
/// clipboard): a dropped file attaches with no prompt; ⌘V (as thc's WezTerm keys send it, the
/// kitty protocol's CSI 118;9u) with a screenshot on the clipboard attaches it, and with text
/// pastes the text; an empty bracketed paste (an image-only clipboard) looks for the image.
#[test]
fn screenshots_by_drag_and_by_cmd_v() {
    let root = std::env::temp_dir().join(format!("thc-cmdv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cli = |args: &[&str]| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env_remove("THC_ACTOR");
        c.output().unwrap()
    };
    assert!(cli(&["init", "v"]).status.success());
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend(8u32.to_be_bytes());
    png.extend(6u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    let shot = root.join("dragged.png");
    std::fs::write(&shot, &png).unwrap();
    let clip = root.join("clipboard.png");
    std::fs::write(&clip, &png).unwrap();
    let session = |env: &[(&str, &str)], input: &[&[u8]]| {
        let mut p = Pty::spawn_env(&root, &["j", "--no-focus"], env);
        let start = Instant::now();
        while p.flags().is_none() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(300));
        for i in input {
            p.send(i);
        }
        std::thread::sleep(Duration::from_millis(800));
        p.send(b"\x1b[113;5u"); // ⌃Q
        let deadline = Instant::now() + Duration::from_secs(5);
        while p.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let texts = || -> String { String::from_utf8_lossy(&cli(&["--json", "q", "journal=today"]).stdout).to_string() };
    // A drop: the terminal pastes the path (bracketed).
    let drop = format!("\x1b[200~{}\x1b[201~", shot.display());
    session(&[], &[drop.as_bytes()]);
    assert!(texts().contains("![dragged](files/"), "dropped: {}", texts());
    // ⌘V with a screenshot on the clipboard.
    let c = clip.to_str().unwrap();
    session(&[("THC_CLIPBOARD_IMAGE", c)], &[b"\x1b[118;9u"]);
    assert_eq!(texts().matches("![screenshot ").count(), 1, "⌘V image: {}", texts());
    // An empty bracketed paste (WezTerm's own ⌘V with an image-only clipboard).
    session(&[("THC_CLIPBOARD_IMAGE", c)], &[b"\x1b[200~\x1b[201~"]);
    assert_eq!(texts().matches("![screenshot ").count(), 2, "empty paste: {}", texts());
    // ⌘V with text: the text, exactly.
    session(&[("THC_CLIPBOARD_TEXT", "pasted words")], &[b"\x1b[118;9u"]);
    assert!(texts().contains("pasted words"), "⌘V text: {}", texts());
    let _ = std::fs::remove_dir_all(&root);
}

/// editing.md §5, §11: ⌘A ⌘C ⌘X ⌘Z as thc's WezTerm keys send them (the kitty
/// protocol's CSI 97/99/120/122;9u) select all, copy, cut and undo in thc. The clipboard is the
/// sandbox's file, never the real one.
#[test]
fn cmd_c_x_a_z_work_in_thc() {
    let root = std::env::temp_dir().join(format!("thc-cmdkeys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cli = |args: &[&str]| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env_remove("THC_ACTOR");
        c.output().unwrap()
    };
    assert!(cli(&["init", "v"]).status.success());
    let mut p = Pty::spawn(&root, &["j", "--no-focus"]);
    let start = Instant::now();
    while p.flags().is_none() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    p.send(b"alpha beta");
    p.send(b"\x1b[97;9u"); // ⌘A
    p.send(b"\x1b[99;9u"); // ⌘C
    let deadline = Instant::now() + Duration::from_secs(5);
    while common::clipboard().trim_end() != "alpha beta" && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let sc = screen(&p.out.lock().unwrap(), 30, 100);
    assert_eq!(common::clipboard().trim_end(), "alpha beta", "⌘C copied the selection:\n{}", sc.join("\n"));
    p.send(b"\x1b[120;9u"); // ⌘X: cut it
    p.send(b"\x1b[122;9u"); // ⌘Z: back
    p.send(b"\x1b[F"); // End
    p.send(b"!");
    std::thread::sleep(Duration::from_millis(1800)); // the idle save
    p.send(b"\x1b[113;5u"); // ⌃Q
    let deadline = Instant::now() + Duration::from_secs(5);
    while p.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let q = String::from_utf8_lossy(&cli(&["--json", "q", "journal=today"]).stdout).to_string();
    assert!(q.contains("\"alpha beta!\""), "⌘X then ⌘Z restored the text: {q}");
    let _ = std::fs::remove_dir_all(&root);
}
