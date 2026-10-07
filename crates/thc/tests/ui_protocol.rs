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
        Pty::spawn_with(root, tmp, &[])
    }

    /// `thc tui ARGS…` on a 100×30 pseudo-terminal.
    fn spawn_with(root: &Path, tmp: &Path, args: &[&std::ffi::OsStr]) -> Pty {
        use std::io::{Read, Write};
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::os::unix::process::CommandExt;
        let (mut m, mut s) = (0, 0);
        let mut ws = libc::winsize { ws_row: 30, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
        assert_eq!(unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) }, 0);
        let slave = unsafe { OwnedFd::from_raw_fd(s) };
        let mut c = thc(root);
        // Unpinned: the bar's pinned-clock warning would cover the toast.
        c.arg("tui").args(args).env("TERM", "xterm-256color").env("TMPDIR", tmp).env_remove("THC_NOW").env_remove("THC_FIXTURE_IDS");
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
    // An agent writes to the vault: the TUI's poll finds it, and the state change it makes (the
    // toast, the flash) reaches subscribers as an `external` message.
    ok(thc(&r.0).args(["add", "from an agent"]).env("THC_ACTOR", "claude").env_remove("THC_NOW").env_remove("THC_FIXTURE_IDS"));
    loop {
        let l = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("an external event for the agent's write");
        if l.contains("\"msg\":\"external\"") {
            break;
        }
    }
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

impl Pty {
    /// The terminal changes size (the TUI gets SIGWINCH and a resize message).
    fn resize(&self, w: u16, h: u16) {
        use std::os::fd::AsRawFd;
        let ws = libc::winsize { ws_row: h, ws_col: w, ws_xpixel: 0, ws_ypixel: 0 };
        assert_eq!(unsafe { libc::ioctl(self._master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) }, 0);
    }
}

/// JSON requests to the running TUI in one connection (`thc ui send` with stdin): the responses.
fn requests(root: &Path, tmp: &Path, reqs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    use std::io::Write;
    let mut c = ui(root, tmp);
    c.arg("send").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
    let mut child = c.spawn().unwrap();
    let input: String = reqs.iter().map(|r| format!("{r}\n")).collect();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
    String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// The lines of a trace up to `rev`: the trace as it stood when the TUI was at that rev.
fn upto(lines: impl Iterator<Item = serde_json::Value>, rev: u64) -> String {
    lines.filter(|v| v.get("_rev").or(v.get("rev")).and_then(serde_json::Value::as_u64).is_some_and(|n| n <= rev)).map(|l| format!("{l}\n")).collect()
}

/// The live TUI's frame at `w`×`h`, and its trace (`trace`, a request) up to that frame's rev
/// (the clock keeps ticking in between). Also that rev.
fn frame_and_trace(root: &Path, tmp: &Path, w: u16, h: u16, trace: serde_json::Value) -> (String, String, u64) {
    let r = requests(root, tmp, &[serde_json::json!({"op": "render", "w": w, "h": h}), trace]);
    let rev = r[0]["result"]["rev"].as_u64().unwrap();
    let lines = upto(r[1]["result"]["trace"].as_array().unwrap().iter().cloned(), rev);
    (r[0]["result"]["frame"].as_str().unwrap().to_string(), lines, rev)
}

/// `thc ui replay` from a process whose terminal is nothing like the TUI's (no TERM, no
/// COLORTERM: CI's): the frames still come out as the TUI drew them.
fn replay(root: &Path, trace: &Path, size: &str) -> String {
    ok(thc(root).args(["ui", "replay"]).arg(trace).args(["--size", size]).env_remove("TERM").env_remove("COLORTERM").env("LANG", "C"))
}

/// A real session's trace replays to its screen. A TUI on a pty is typed into through the
/// protocol (a journal line, Tab, ⌥← and ⇧⌥→, ⌃Z, ⌥↑ ⌥↓), saves when it idles, takes in an
/// agent's `thc add`, has its view switched by `thc ui patch` and its terminal resized. Its
/// trace, from `trace.get`, from `thc tui --trace` and after `trace.checkpoint`, replays (with
/// THC_NOW pinned) to exactly the frame the TUI draws, at a size given with `--size`: every
/// write lands once, on the vault as it was when the trace began.
#[test]
fn a_live_sessions_trace_replays_to_its_screen() {
    let r = vault("live-replay");
    let tmp = r.0.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let file = r.0.join("session.jsonl");
    let tui = Pty::spawn_with(&r.0, &tmp, &["--trace".as_ref(), file.as_os_str()]);
    wait_for("the TUI to advertise itself", || sessions(&r.0, &tmp) == 1);
    let trace_has = |what: &str| ok(ui(&r.0, &tmp).args(["send", "trace.get", "all", "--raw"])).contains(what);
    for keys in ["5", "coffee notes", "<cr><tab>nested line", "<cr>Word motion<m-left><s-m-right>", "<c-z>", "<m-up><m-down>"] {
        ok(ui(&r.0, &tmp).args(["send", "keys", keys]));
    }
    wait_for("the idle save", || trace_has("{\"msg\":\"idle\""));
    ok(thc(&r.0).args(["add", "from the agent"]).env("THC_ACTOR", "claude").env_remove("THC_NOW").env_remove("THC_FIXTURE_IDS"));
    wait_for("the poll to take the agent's line in", || trace_has("\"_log\""));
    ok(ui(&r.0, &tmp).args(["patch", r#"{"view":"tasks"}"#]).env("THC_ACTOR", "claude"));
    ok(ui(&r.0, &tmp).args(["patch", r#"{"view":"journal"}"#]).env("THC_ACTOR", "claude"));
    tui.resize(150, 40);
    wait_for("the resize", || trace_has("{\"msg\":\"resize\",\"w\":150,\"h\":40"));

    let (live, trace, rev) = frame_and_trace(&r.0, &tmp, 77, 24, serde_json::json!({"op": "trace.get", "all": true}));
    assert!(live.contains("coffee notes") && live.contains("nested line") && live.contains("from the agent"), "{live}");
    let t = r.0.join("t.jsonl");
    std::fs::write(&t, &trace).unwrap();
    let replayed = replay(&r.0, &t, "77x24");
    assert_eq!(replayed, live, "the replay draws the live screen");
    assert_eq!(replayed.matches("coffee notes").count(), 1, "{replayed}");
    assert_eq!(replay(&r.0, &t, "77x24"), replayed, "the same every run");
    // Without --size: the trace's own size, the terminal's after the resize.
    assert_eq!(ok(thc(&r.0).args(["ui", "replay"]).arg(&t)).lines().count(), 40);

    // `thc tui --trace FILE`: the same session, every line, up to the same rev.
    let recorded = std::fs::read_to_string(&file).unwrap();
    let f = r.0.join("f.jsonl");
    std::fs::write(&f, upto(recorded.lines().map(|l| serde_json::from_str(l).unwrap()), rev)).unwrap();
    assert_eq!(replay(&r.0, &f, "77x24"), live, "the --trace file replays the same");

    // A checkpoint: the segment from it replays on its own, on the vault as of then.
    ok(ui(&r.0, &tmp).args(["send", "trace.checkpoint"]));
    ok(ui(&r.0, &tmp).args(["send", "keys", "<cr>after the checkpoint"]));
    ok(thc(&r.0).args(["add", "another from the agent"]).env("THC_ACTOR", "claude").env_remove("THC_NOW").env_remove("THC_FIXTURE_IDS"));
    wait_for("the second agent line", || ok(ui(&r.0, &tmp).args(["render"])).contains("another from the agent"));
    let (live, segment, _) = frame_and_trace(&r.0, &tmp, 90, 30, serde_json::json!({"op": "trace.get"}));
    assert!(segment.starts_with("{\"state\"") && segment.lines().next().unwrap().contains("\"log\""), "{segment}");
    std::fs::write(&t, &segment).unwrap();
    assert_eq!(replay(&r.0, &t, "90x30"), live, "the segment replays");
    let (live, all, _) = frame_and_trace(&r.0, &tmp, 90, 30, serde_json::json!({"op": "trace.get", "all": true}));
    std::fs::write(&t, &all).unwrap();
    assert_eq!(replay(&r.0, &t, "90x30"), live, "both segments replay");
    drop(tui);
}

/// The sidebar's agent verb (sidebar.md §10, S21–S23): `thc ui aside` opens beside the person
/// without moving their keyboard, says so, and ⌘[ takes it back; an agent's patch can't move
/// focus or close a pinned panel; the stack round-trips through a headless render.
#[test]
fn an_agent_opens_a_page_beside_the_person() {
    let r = vault("aside");
    let page: serde_json::Value = serde_json::from_str(&ok(thc(&r.0).args(["page", "new", "Reading List", "--json"]))).unwrap();
    let id = page["nodes"][0]["id"].as_str().unwrap().to_string();
    ok(thc(&r.0).args(["add", "--under", &id, "Atomic Habits: finished"]));
    let tmp = r.0.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let _tui = Pty::spawn(&r.0, &tmp);
    wait_for("the TUI to advertise itself", || sessions(&r.0, &tmp) == 1);
    let claude = |args: &[&str]| -> std::process::Output { ui(&r.0, &tmp).args(args).env("THC_ACTOR", "claude").output().unwrap() };

    // S21: on top, marked, a toast; the keyboard doesn't move; ⌘[ closes it.
    let o = claude(&["aside", "Reading List"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let frame = ok(ui(&r.0, &tmp).args(["render", "140x40"]));
    assert!(frame.contains("▾ ¶ Reading List") && frame.contains("◆ claude"), "{frame}");
    assert!(frame.contains("claude opened ¶ Reading List beside you"), "{frame}");
    let state: serde_json::Value = serde_json::from_str(&ok(ui(&r.0, &tmp).args(["state", "--raw"]))).unwrap();
    assert_eq!(state["focus"], "list", "{state}");
    assert_eq!(state["sidebar"]["open"][0]["opened_by"], "claude");
    let ls: serde_json::Value = serde_json::from_str(&ok(ui(&r.0, &tmp).args(["--json", "aside", "--ls"]))).unwrap();
    assert_eq!(ls["open"][0]["title"], "Reading List", "{ls}");
    ok(ui(&r.0, &tmp).args(["send", "keys", "<c-m-left>"]));
    let frame = ok(ui(&r.0, &tmp).args(["render", "140x40"]));
    assert!(!frame.contains("¶ Reading List  "), "⌘[ closed it: {frame}");
    let state: serde_json::Value = serde_json::from_str(&ok(ui(&r.0, &tmp).args(["state", "--raw"]))).unwrap();
    assert!(state["sidebar"]["open"].as_array().unwrap().is_empty(), "{state}");

    // Exit codes: no such page 3; a bad query 6.
    assert_eq!(claude(&["aside", "No such page"]).status.code(), Some(3));
    assert_eq!(claude(&["aside", "status:opn"]).status.code(), Some(6));

    // S22: the person pins it; an agent's focus patch makes it active, focus stays; closing a
    // pinned panel by patch or aside is refused (exit 6).
    assert!(claude(&["aside", "Reading List"]).status.success());
    ok(ui(&r.0, &tmp).args(["send", "keys", "<m-s><m-p><esc>"]));
    let o = claude(&["patch", r#"{"focus":"sidebar"}"#]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let state: serde_json::Value = serde_json::from_str(&ok(ui(&r.0, &tmp).args(["state", "--raw"]))).unwrap();
    assert_eq!(state["focus"], "list", "{state}");
    assert_eq!(state["sidebar"]["open"][0]["pinned"], true, "{state}");
    let o = claude(&["patch", r#"{"sidebar":{"open":[]}}"#]);
    assert_eq!(o.status.code(), Some(6), "{}", String::from_utf8_lossy(&o.stderr));
    let o = claude(&["aside", "--close", "Reading List"]);
    assert_eq!(o.status.code(), Some(6), "{}", String::from_utf8_lossy(&o.stderr));

    // S23: the stack, read and rendered headlessly, draws the same sidebar.
    let st = ok(ui(&r.0, &tmp).args(["state", "--raw"]));
    let file = r.0.join("stack.json");
    std::fs::write(&file, &st).unwrap();
    let live = ok(ui(&r.0, &tmp).args(["render", "140x40"]));
    let headless = ok(thc(&r.0).args(["ui", "render", "140x40", "--state"]).arg(&file).env_remove("THC_NOW"));
    let side = |f: &str| f.lines().skip(2).take(10).map(|l| l.chars().skip(93).collect::<String>()).collect::<Vec<_>>();
    assert_eq!(side(&live), side(&headless), "live:\n{live}\nheadless:\n{headless}");
}
