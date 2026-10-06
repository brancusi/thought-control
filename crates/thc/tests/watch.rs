//! Streaming contract, including real scratch daemon delivery, outage recovery and signals.
mod common;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

struct V {
    root: std::path::PathBuf,
}
impl V {
    fn new(name: &str) -> Self {
        let root = common::root().join(format!("watch-{name}"));
        std::fs::create_dir_all(&root).unwrap();
        let v = Self { root };
        v.json(&["init", "vault"]);
        let p = v.root.join(".thc.toml");
        let text = std::fs::read_to_string(&p).unwrap();
        std::fs::write(p, format!("{text}\nboard = \"vault\"\n")).unwrap();
        v
    }
    fn cmd(&self) -> Command {
        let mut c = common::thc();
        c.current_dir(&self.root)
            .env("THC_VAULT", self.root.join("vault"))
            .env("THC_CACHE_DIR", self.root.join("cache"))
            .env("THC_ACTOR", "codex-engineer-3")
            .env("THC_CONFIG_DIR", self.root.join("config"))
            .env("THC_NOW", "2026-10-06T09:00")
            .env_remove("THC_SOCKET");
        c
    }
    fn json(&self, args: &[&str]) -> Value {
        let o = self.cmd().args(["--json"]).args(args).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn id(&self, args: &[&str]) -> String {
        self.json(args)["nodes"][0]["id"].as_str().unwrap().into()
    }
    fn daemon(&self) -> common::Guard {
        let d = common::Guard::spawn(self.cmd().args(["daemon", "run"]).stdout(Stdio::null()).stderr(Stdio::null()));
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            if self.cmd().args(["daemon", "status"]).output().unwrap().status.success() {
                return d;
            }
            assert!(Instant::now() < end, "scratch daemon failed to start");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    fn watch(&self, target: &str, extra: &[&str]) -> W {
        let mut c = self.cmd();
        c.args(["--json", "watch", "--for", target]).args(extra).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = common::Guard::spawn(&mut c);
        let out = lines(child.0.stdout.take().unwrap());
        let err = lines(child.0.stderr.take().unwrap());
        W { child, out, err }
    }
}
fn lines(r: impl std::io::Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    rx
}
struct W {
    child: common::Guard,
    out: Receiver<String>,
    err: Receiver<String>,
}
impl W {
    fn item(&self, event: &str, id: &str) -> Value {
        let s = self.out.recv_timeout(Duration::from_secs(10)).expect("watch did not flush an item");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["event"], event, "{s}");
        assert_eq!(v["node"]["id"], id, "{s}");
        v
    }
    fn notice(&self, term: &str) {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            let s = self.err.recv_timeout(end.saturating_duration_since(Instant::now())).expect("missing watch notice");
            if s.contains(term) {
                return;
            }
        }
    }
    fn quiet(&self) {
        assert!(self.out.recv_timeout(Duration::from_millis(500)).is_err(), "unexpected duplicate/unaddressed item");
    }
    fn exited(&mut self) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.0.try_wait().unwrap() {
                assert!(status.success(), "watch failed: {status}");
                return;
            }
            assert!(Instant::now() < end, "watch did not exit cleanly");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[test]
fn startup_once_receipts_and_board_scope_are_read_only() {
    let v = V::new("startup");
    let page = v.id(&["page", "new", "Issues"]);
    let task = v.id(&["todo", "Build it", "--under", &page]);
    v.json(&["set", &task, "role=engineer"]);
    let old = v.id(&["msg", "engineer", "already read", "--on", &task]);
    v.json(&["msg", "read", &old]);
    let other = v.id(&["msg", "designer", "outside queue"]);
    let outside_task = v.id(&["todo", "Outside queue"]);
    v.json(&["set", &outside_task, "role=engineer"]);
    v.id(&["msg", "designer", "other role", "--on", &task]);
    let direct = v.id(&["msg", "codex-engineer-2", "direct to teammate", "--on", &task]);
    let board = format!("{}:¶ Issues", v.root.join("vault").display());
    let mut w = v.watch("engineer", &["--once", "--board", &board, "--limit", "1"]);
    w.item("message", &direct);
    w.exited();
    let mut w = v.watch("codex-engineer-3", &["--once", "--board", &board]);
    w.item("task", &task);
    w.exited();
    assert_eq!(v.json(&["msgs", "--unread"])["count"], 0); // this actor excludes teammate directs
    assert!(v.json(&["show", &direct])["props"].get("read_codex-engineer-3").is_none());
    assert!(v.json(&["show", &task])["props"].get("owner").is_none());
    assert_eq!(v.json(&["show", &other])["props"]["to"], "designer");
}

#[test]
fn loose_messages_are_included_in_the_board_vault() {
    let v = V::new("loose");
    v.id(&["page", "new", "Issues"]);
    let another = V::new("other-vault");
    another.id(&["msg", "engineer", "different vault"]);
    let msg = v.id(&["msg", "engineer", "loose board message"]);
    let board = format!("{}:¶ Issues", v.root.join("vault").display());
    let mut w = v.watch("engineer", &["--once", "--board", &board]);
    let item = w.item("message", &msg);
    assert_eq!(item["board"]["page"]["title"], "Issues");
    assert_eq!(item["node"]["props"]["to"], "engineer");
    w.exited();
}

#[test]
fn live_push_filters_and_rechecks_blocked_tasks_without_duplicates() {
    let v = V::new("live");
    let _daemon = v.daemon();
    let mut w = v.watch("codex-engineer-3", &[]);
    w.notice("connected");
    v.id(&["msg", "designer", "wrong role"]);
    v.id(&["msg", "codex-engineer-2", "wrong actor"]);
    w.quiet();
    let msg = v.id(&["msg", "engineer", "live message"]);
    let item = w.item("message", &msg);
    assert_eq!(item["source"], "daemon");
    assert!(item["change"]["tx"].is_string());
    v.json(&["set", &msg, "extra=metadata"]);
    w.quiet();
    let blocker = v.id(&["todo", "Blocker"]);
    let task = v.id(&["todo", "Waiting"]);
    v.json(&["link", &blocker, &task, "--rel", "blocks"]);
    v.json(&["set", &task, "role=engineer"]);
    w.quiet();
    v.json(&["done", &blocker]);
    w.item("task", &task);
    v.json(&["set", &task, "status=doing", "owner=codex-engineer-2"]);
    w.quiet();
    v.json(&["set", &task, "owner=", "status=todo"]);
    w.item("task", &task);
    v.json(&["set", &task, "extra=metadata"]);
    w.quiet();
    unsafe {
        libc::kill(w.child.0.id() as i32, libc::SIGINT);
    }
    w.exited();
}

#[test]
fn offline_polling_once_and_sigterm() {
    let v = V::new("offline");
    let mut w = v.watch("engineer", &["--once"]);
    w.notice("polling");
    v.id(&["todo", "Unrouted"]);
    let task = v.id(&["todo", "New routed task"]);
    v.json(&["set", &task, "role=engineer"]);
    assert_eq!(w.item("task", &task)["source"], "poll");
    w.exited();
    let mut w = v.watch("designer", &[]);
    w.notice("polling");
    unsafe {
        libc::kill(w.child.0.id() as i32, libc::SIGTERM);
    }
    w.exited();
}

#[test]
fn daemon_reconnect_reconciles_and_resumes_push() {
    let v = V::new("reconnect");
    let mut daemon = v.daemon();
    let w = v.watch("engineer", &[]);
    w.notice("connected");
    let a = v.id(&["msg", "engineer", "first"]);
    w.item("message", &a);
    v.json(&["daemon", "stop"]);
    daemon.0.wait().unwrap();
    w.notice("polling");
    let b = v.id(&["msg", "engineer", "during outage"]);
    w.item("message", &b);
    let _daemon = v.daemon();
    w.notice("connected");
    w.quiet();
    let c = v.id(&["msg", "codex-engineer-2", "after reconnect"]);
    assert_eq!(w.item("message", &c)["source"], "daemon");
    w.quiet();
}

#[test]
fn partial_socket_lines_survive_timeouts_and_reconnect_snapshot() {
    use std::{io::Write, os::unix::net::UnixListener};
    let v = V::new("partial");
    let socket = v.root.join("fake.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let mut c = v.cmd();
    c.env("THC_SOCKET", &socket).args(["--json", "watch", "--for", "engineer"]).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = common::Guard::spawn(&mut c);
    let w = W { out: lines(child.0.stdout.take().unwrap()), err: lines(child.0.stderr.take().unwrap()), child };
    let (mut peer, _) = listener.accept().unwrap();
    let mut hello = String::new();
    BufReader::new(peer.try_clone().unwrap()).read_line(&mut hello).unwrap();
    let hello: Value = serde_json::from_str(&hello).unwrap();
    assert_eq!(hello["params"]["topics"], serde_json::json!(["changed"]));
    assert_eq!(hello["params"]["deliver"], false);
    peer.write_all(b"{\"id\":1,\"result\":{\"proto\":1}}\n").unwrap();
    w.notice("connected");
    let a = v.id(&["msg", "engineer", "partial"]);
    peer.write_all(b"{\"event\":\"changed\",").unwrap();
    std::thread::sleep(Duration::from_millis(450));
    w.quiet();
    peer.write_all(format!("\"data\":{{\"ids\":[\"{a}\"],\"tx\":\"test\"}}}}\n").as_bytes()).unwrap();
    assert_eq!(w.item("message", &a)["source"], "daemon");
    drop(peer);
    let (mut peer, _) = listener.accept().unwrap();
    let b = v.id(&["msg", "engineer", "created before hello reply"]);
    peer.write_all(b"{\"id\":1,\"result\":{\"proto\":1}}\n").unwrap();
    w.notice("connected");
    assert_eq!(w.item("message", &b)["source"], "reconcile");
    w.quiet();
}
