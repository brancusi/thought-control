//! `thc ui`: headless render and replay are byte-identical across runs (THC_NOW pinned), and a
//! running TUI answers the JSON-lines protocol (docs/ui-protocol.md).

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

struct Root(PathBuf);

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A vault with a few tasks and notes, for one test.
fn vault(tag: &str) -> Root {
    let root = std::env::temp_dir().join(format!("thc-ui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let r = Root(root);
    let o = thc(&r.0).args(["init", "vault"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    for text in ["[ ] write the plan due:today #work", "[ ] call the bank !high", "[ ] water the plants #home", "an idea about [[Garden]]"] {
        let o = thc(&r.0).args(["add", text]).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    r
}

fn thc(root: &Path) -> Command {
    let mut c = common::thc();
    c.current_dir(root)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .env("THC_NOW", "2026-10-07T10:00")
        .env("THC_FIXTURE_IDS", "1")
        .env("THC_DEVICE", "test")
        .env("THC_ACTOR", "human");
    c
}

fn ok(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into()
}

#[test]
fn render_from_a_state_file_is_the_same_every_time() {
    let r = vault("render");
    let state = r.0.join("state.json");
    std::fs::write(&state, r#"{"view": "tasks", "tasks_filter": "status:open #work"}"#).unwrap();
    let a = ok(thc(&r.0).args(["ui", "render", "100x24", "--state"]).arg(&state));
    let b = ok(thc(&r.0).args(["ui", "render", "100x24", "--state"]).arg(&state));
    assert_eq!(a, b);
    assert!(a.contains("write the plan") && !a.contains("water the plants"), "{a}");
    assert_eq!(a.lines().count(), 24);
    // The default state round-trips: what `state --default` prints renders as Today.
    let d = ok(thc(&r.0).args(["ui", "state", "--default"]));
    std::fs::write(&state, &d).unwrap();
    let today = ok(thc(&r.0).args(["ui", "render", "100x24", "--state"]).arg(&state));
    assert!(today.contains("Today"), "{today}");
    // Cells: JSON rows.
    let cells = ok(thc(&r.0).args(["ui", "render", "60x24", "--format", "cells", "--state"]).arg(&state));
    let v: serde_json::Value = serde_json::from_str(&cells).unwrap();
    assert_eq!(v["rows"].as_array().unwrap().len(), 24);
    // A bad state is a validation error (exit 6) with a suggestion.
    std::fs::write(&state, r#"{"veiw": "tasks"}"#).unwrap();
    let o = thc(&r.0).args(["ui", "render", "--state"]).arg(&state).output().unwrap();
    assert_eq!(o.status.code(), Some(6), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stderr).contains("did you mean `view`"));
}

#[test]
fn a_recorded_trace_replays_byte_for_byte() {
    let r = vault("replay");
    let trace = r.0.join("trace.jsonl");
    let live = ok(thc(&r.0)
        .args(["tui", "--trace"])
        .arg(&trace)
        .env("THC_TUI_SNAPSHOT", "100x24")
        .env("THC_TUI_KEYS", "3jj<tab>k/plan<cr><esc>?<esc>2<s-tab>"));
    let lines = std::fs::read_to_string(&trace).unwrap();
    assert!(lines.lines().next().unwrap().starts_with("{\"state\":"), "{lines}");
    let a = ok(thc(&r.0).args(["ui", "replay"]).arg(&trace));
    let b = ok(thc(&r.0).args(["ui", "replay"]).arg(&trace));
    assert_eq!(a, b);
    assert_eq!(a, live, "the replay's last frame is the session's");
    let every = ok(thc(&r.0).args(["ui", "replay", "--every"]).arg(&trace));
    assert_eq!(every.matches('\u{c}').count(), lines.lines().count() - 1);
    assert_eq!(every, ok(thc(&r.0).args(["ui", "replay", "--every"]).arg(&trace)));
}

/// A TUI on a pseudo-terminal, killed when dropped.
struct Pty {
    child: std::process::Child,
    out: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    _master: std::fs::File,
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
    fn spawn(root: &Path, tmp: &Path) -> Pty {
        use std::io::{Read, Write};
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::os::unix::process::CommandExt;
        let (mut m, mut s) = (0, 0);
        let mut ws = libc::winsize { ws_row: 30, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
        assert_eq!(unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) }, 0);
        let slave = unsafe { OwnedFd::from_raw_fd(s) };
        let mut c = thc(root);
        // Unpinned: the bar's pinned-clock warning would cover the toast.
        c.arg("tui").env("TERM", "xterm-256color").env("TMPDIR", tmp).env_remove("THC_NOW").env_remove("THC_FIXTURE_IDS");
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
        let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
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
                if b[..n].windows(4).any(|x| x == b"\x1b[6n") {
                    let _ = w.write_all(b"\x1b[1;1R");
                }
                o.lock().unwrap().extend_from_slice(&b[..n]);
            }
        });
        Pty { child, out, _master: master }
    }

    fn screen_has(&self, text: &str) -> bool {
        String::from_utf8_lossy(&self.out.lock().unwrap()).contains(text)
    }
}

/// `thc ui …` against the TUIs advertised under `tmp`.
fn ui(root: &Path, tmp: &Path) -> Command {
    let mut c = thc(root);
    c.arg("ui").env("TMPDIR", tmp).env_remove("THC_NOW").env_remove("THC_FIXTURE_IDS");
    c
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !f() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn sessions(root: &Path, tmp: &Path) -> usize {
    let o = ui(root, tmp).args(["--json", "ls"]).output().unwrap();
    serde_json::from_slice::<serde_json::Value>(&o.stdout).map(|v| v["sessions"].as_array().map_or(0, Vec::len)).unwrap_or(0)
}

#[test]
fn a_running_tui_answers_and_a_pushed_state_changes_its_screen() {
    let r = vault("live");
    let tmp = r.0.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let tui = Pty::spawn(&r.0, &tmp);
    wait_for("the TUI to advertise itself", || sessions(&r.0, &tmp) == 1);

    // Read: the state, without history.
    let state: serde_json::Value = serde_json::from_str(&ok(ui(&r.0, &tmp).args(["state", "--no-history"]))).unwrap();
    assert_eq!(state["state"]["view"], "today", "{state}");
    assert!(state["state"].get("history").is_none());
    let rev = state["rev"].as_u64().unwrap();

    // Push: an agent patches the view; the person's screen shows it and says who.
    let t0 = std::time::Instant::now();
    let p: serde_json::Value = serde_json::from_str(&ok(ui(&r.0, &tmp).args(["patch", r#"{"view":"tasks","tasks_filter":"status:open #work"}"#]).env("THC_ACTOR", "claude"))).unwrap();
    assert!(t0.elapsed() < std::time::Duration::from_secs(2), "a patch took {:?}", t0.elapsed());
    assert!(p["rev"].as_u64().unwrap() > rev, "{p}");
    let frame = ok(ui(&r.0, &tmp).args(["render"]));
    assert!(frame.contains("claude changed your view"), "{frame}");
    wait_for("the TUI's screen to show the change", || tui.screen_has("claude changed your view"));
    assert!(frame.contains("write the plan") && !frame.contains("water the plants"), "{frame}");
    assert_eq!(frame.lines().count(), 30, "the TUI's own size");
    let state: serde_json::Value = serde_json::from_str(&ok(ui(&r.0, &tmp).args(["state", "--raw"]))).unwrap();
    assert_eq!(state["view"], "tasks");
    assert_eq!(state["tasks_filter"], "status:open #work");

    // A stale guard changes nothing (exit 4); a bad field is a validation error (exit 6).
    let o = ui(&r.0, &tmp).args(["patch", r#"{"view":"log"}"#, "--if-rev", "0"]).output().unwrap();
    assert_eq!(o.status.code(), Some(4), "{}", String::from_utf8_lossy(&o.stderr));
    let o = ui(&r.0, &tmp).args(["patch", r#"{"veiw":"log"}"#]).output().unwrap();
    assert_eq!(o.status.code(), Some(6), "{}", String::from_utf8_lossy(&o.stderr));

    // Set: a whole state file, the shape `state` prints.
    let file = r.0.join("s.json");
    std::fs::write(&file, r#"{"rev": 1, "state": {"view": "log"}}"#).unwrap();
    ok(ui(&r.0, &tmp).args(["set"]).arg(&file));
    assert!(ok(ui(&r.0, &tmp).args(["render"])).contains("Log"));

    // Keys, and the messages they became.
    let k: serde_json::Value = serde_json::from_str(&ok(ui(&r.0, &tmp).args(["send", "keys", "3"]))).unwrap();
    assert_eq!(k["result"]["msgs"].as_array().unwrap().last().unwrap()["key"], "3", "{k}");
    let state: serde_json::Value = serde_json::from_str(&ok(ui(&r.0, &tmp).args(["state", "--raw"]))).unwrap();
    assert_eq!(state["view"], "tasks");
    // Hello, raw render at another size, the trace.
    assert!(ok(ui(&r.0, &tmp).args(["send", "hello"])).contains("\"proto\":1"));
    assert_eq!(ok(ui(&r.0, &tmp).args(["send", "render", "80x24", "--raw"])).lines().count(), 24);
    let trace = ok(ui(&r.0, &tmp).args(["send", "trace.get", "all", "--raw"]));
    assert!(trace.lines().any(|l| l.contains("\"msg\":\"patch\"")), "{trace}");

    // Subscribe: the person's keys at the terminal reach a subscriber as events.
    let mut sub = common::Guard::spawn(ui(&r.0, &tmp).args(["send", "subscribe"]).stdout(std::process::Stdio::piped()));
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let out = sub.0.stdout.take().unwrap();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for l in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            let _ = tx.send(l);
        }
    });
    let first = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(first.contains("\"subscribed\":true"), "{first}");
    {
        use std::io::Write;
        (&tui._master).write_all(b"2").unwrap();
    }
    let event = loop {
        let l = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("an event for the terminal key");
        if l.contains("\"source\":\"terminal\"") && l.contains("\"key\":\"2\"") {
            break l;
        }
    };
    assert!(event.starts_with("{\"event\":\"state\""), "{event}");
    drop(sub);

    // Read-tier agents can read but not steer.
    let cfg = r.0.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), "[actors.reader]\ntier = \"read\"\n").unwrap();
    let o = ui(&r.0, &tmp).args(["patch", r#"{"view":"inbox"}"#]).env("THC_ACTOR", "reader").env("THC_CONFIG_DIR", &cfg).output().unwrap();
    assert_eq!(o.status.code(), Some(6), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(ui(&r.0, &tmp).args(["state"]).env("THC_ACTOR", "reader").env("THC_CONFIG_DIR", &cfg).output().unwrap().status.success());

    // Two TUIs on the vault: which one must be said (exit 5), then --session picks.
    let second = Pty::spawn(&r.0, &tmp);
    wait_for("a second TUI", || sessions(&r.0, &tmp) == 2);
    let o = ui(&r.0, &tmp).args(["state"]).output().unwrap();
    assert_eq!(o.status.code(), Some(5), "{}", String::from_utf8_lossy(&o.stderr));
    let pid = second.child.id().to_string();
    let s: serde_json::Value = serde_json::from_str(&ok(ui(&r.0, &tmp).args(["state", "--raw", "--session", &pid]))).unwrap();
    assert_eq!(s["view"], "today", "the second TUI has its own state");
    drop(second);
    drop(tui);
}
