//! Daemon integration tests: pushes, single writer, two devices through a sync shim, alerts.

use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use thc_core::builder::TxBuilder;
use thc_core::event::{Actor, Op, Trigger};
use thc_core::proto::{Client, Incoming};
use thc_core::vault::{self, Paths, Vault};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("thc-d-{name}-{}", thc_core::id::new_id()));
    fs::create_dir_all(&d).unwrap();
    d
}

fn paths(root: &Path, i: usize) -> Paths {
    let v = root.join(format!("v{i}"));
    vault::init(&v, None, None).unwrap();
    Paths { vault: v, cache: root.join(format!("c{i}")) }
}

fn human() -> Actor {
    Actor { kind: "human".into(), name: None }
}

fn start(p: &Paths) -> JoinHandle<()> {
    let p2 = p.clone();
    let h = std::thread::spawn(move || {
        thc_daemon::run(p2, thc_daemon::RunOpts { json: true, channel: Some(thc_daemon::Channel::Log), label: None, host: false }).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while Client::connect(p).is_none() {
        assert!(Instant::now() < deadline, "daemon didn't start");
        std::thread::sleep(Duration::from_millis(20));
    }
    // It answers before its file watcher is up; these tests time pushes, so wait for it.
    // (A slow fseventsd takes ~20 s per watcher, one at a time: six in parallel need minutes.)
    let deadline = Instant::now() + Duration::from_secs(240);
    while !connect(p).call("status", Value::Null).ok().is_some_and(|s| s["watching"] == true) {
        assert!(Instant::now() < deadline, "the watcher never started");
        std::thread::sleep(Duration::from_millis(50));
    }
    h
}

/// A client with a generous timeout: these tests run beside every other test binary (and,
/// once, beside a second `cargo test` in the same tree, where 10 s wasn't enough), so the
/// daemon can take far longer than the app's 2 s to answer under that load.
fn connect(p: &Paths) -> Client {
    Client::connect_path(&thc_core::proto::socket_path(p), Duration::from_secs(30)).unwrap()
}

fn stop(p: &Paths, h: JoinHandle<()>) {
    let mut c = connect(p);
    c.call("shutdown", Value::Null).unwrap();
    h.join().unwrap();
}

fn subscribe(p: &Paths, topics: &[&str], deliver: bool) -> Client {
    let mut c = connect(p);
    c.hello("test", topics, deliver).unwrap();
    c.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    c
}

/// Wait for an event matching `pred`.
fn wait_event(c: &mut Client, within: Duration, pred: impl Fn(&str, &Value) -> bool) -> Option<Value> {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if let Some(i) = c.pending_events.iter().position(|e| pred(&e.event, &e.data)) {
            return Some(c.pending_events.remove(i).data);
        }
        if let Ok(Some(Incoming::Event(e))) = c.read() {
            if pred(&e.event, &e.data) {
                return Some(e.data);
            }
        }
    }
    None
}

fn add(v: &mut Vault, text: &str) -> String {
    let today = thc_core::dates::today();
    let cap = thc_core::capture::parse(text, today).unwrap();
    let (_, id) = v
        .transact(|s| {
            let mut b = TxBuilder::new(s, today);
            let j = b.journal(today)?;
            let id = b.create_from_capture(Some(j), &cap, None)?;
            Ok((b.finish(), id))
        })
        .unwrap();
    id
}

fn add_alert_now(v: &mut Vault, text: &str) -> (String, String) {
    let today = thc_core::dates::today();
    let at = thc_core::dates::now_local().format("%Y-%m-%dT%H:%M").to_string();
    let cap = thc_core::capture::parse(&format!("[ ] {text}"), today).unwrap();
    let (_, ids) = v
        .transact(|s| {
            let mut b = TxBuilder::new(s, today);
            let j = b.journal(today)?;
            let id = b.create_from_capture(Some(j), &cap, None)?;
            let a = b.add_alert(&id, Trigger { at: Some(at.clone()), offset: None, anchor: None })?;
            Ok((b.finish(), (id, a)))
        })
        .unwrap();
    ids
}

#[test]
fn pushes_changes_made_by_the_cli_on_this_device() {
    let root = tmp("push");
    let p = paths(&root, 0);
    let h = start(&p);
    let mut c = subscribe(&p, &["changed"], false);
    let mut cli = Vault::open(p.clone(), Actor { kind: "agent".into(), name: Some("claude".into()) }, "cli").unwrap();
    let started = Instant::now();
    let id = add(&mut cli, "pushed note");
    let ev = wait_event(&mut c, Duration::from_secs(2), |e, d| e == "changed" && d["ids"].as_array().is_some_and(|a| a.iter().any(|x| x == id.as_str())));
    let ev = ev.expect("no changed event");
    assert_eq!(ev["actor"], "agent:claude");
    eprintln!("push latency: {:?}", started.elapsed());
    // SPEC §8: 250 ms. Shared CI runners get 750 ms, so runner jitter doesn't block a release while
    // a real regression (seconds, a missed push) still fails there.
    let budget = if std::env::var_os("CI").is_some() { 750 } else { 250 };
    assert!(started.elapsed() < Duration::from_millis(budget), "push took {:?} (budget {budget} ms)", started.elapsed());
    stop(&p, h);
}

#[test]
fn concurrent_writers_never_interleave() {
    let root = tmp("writers");
    let p = paths(&root, 0);
    let h = start(&p);
    let mut handles = Vec::new();
    for t in 0..4 {
        let p = p.clone();
        handles.push(std::thread::spawn(move || {
            let mut v = Vault::open(p, human(), "cli").unwrap();
            for i in 0..20 {
                add(&mut v, &format!("w{t}-{i}"));
            }
        }));
    }
    {
        let p = p.clone();
        handles.push(std::thread::spawn(move || {
            let mut c = connect(&p);
            for i in 0..20 {
                c.call("capture", json!({ "text": format!("rpc-{i}") })).unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    // Every log line parses, and replaying from scratch gives every node exactly once.
    let mut lines = 0;
    for dev in fs::read_dir(p.vault.join("log")).unwrap() {
        for f in fs::read_dir(dev.unwrap().path()).unwrap() {
            for line in fs::read_to_string(f.unwrap().path()).unwrap().lines() {
                serde_json::from_str::<thc_core::event::Event>(line).expect("torn or interleaved log line");
                lines += 1;
            }
        }
    }
    assert!(lines >= 100);
    let mut v = Vault::open(p.clone(), human(), "cli").unwrap();
    v.rebuild().unwrap();
    let n = v.store.query("text:w", thc_core::dates::today(), 1000).unwrap().len() + v.store.query("text:rpc-", thc_core::dates::today(), 1000).unwrap().len();
    assert_eq!(n, 100);
    stop(&p, h);
}

/// Copies new log bytes between vaults like a sync tool, in two chunks (partial lines in between).
fn shim(a: PathBuf, b: PathBuf, stop: Arc<AtomicBool>, paused: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            if !paused.load(Ordering::SeqCst) {
                for (src, dst) in [(&a, &b), (&b, &a)] {
                    sync_dir(&src.join("log"), &dst.join("log"));
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    })
}

fn sync_dir(src: &Path, dst: &Path) {
    let Ok(devs) = fs::read_dir(src) else { return };
    for dev in devs.flatten() {
        let Ok(files) = fs::read_dir(dev.path()) else { continue };
        for f in files.flatten() {
            let target = dst.join(dev.file_name()).join(f.file_name());
            let have = fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
            let total = f.metadata().map(|m| m.len()).unwrap_or(0);
            if total <= have {
                continue;
            }
            let mut buf = Vec::new();
            let mut file = fs::File::open(f.path()).unwrap();
            file.seek(SeekFrom::Start(have)).unwrap();
            file.read_to_end(&mut buf).unwrap();
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            let mut out = fs::OpenOptions::new().create(true).append(true).open(&target).unwrap();
            let half = buf.len() / 2;
            out.write_all(&buf[..half]).unwrap();
            out.flush().unwrap();
            std::thread::sleep(Duration::from_millis(15));
            out.write_all(&buf[half..]).unwrap();
        }
    }
}

fn dump(v: &Vault) -> Vec<String> {
    v.store
        .nodes_where("1=1 ORDER BY n.id", &[])
        .unwrap()
        .into_iter()
        .map(|n| format!("{}|{:?}|{}|{}|{:?}|{:?}|{}", n.id, n.parent, n.ord, n.text, n.status, n.due, n.deleted))
        .collect()
}

#[test]
fn two_devices_sync_and_alerts_fire_once_per_device() {
    let root = tmp("two");
    let (pa, pb) = (paths(&root, 0), paths(&root, 1));
    let stop_shim = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    let sh = shim(pa.vault.clone(), pb.vault.clone(), stop_shim.clone(), paused.clone());
    let (ha, hb) = (start(&pa), start(&pb));
    let mut sub_a = subscribe(&pa, &["changed", "alerts"], false);
    let mut sub_b = subscribe(&pb, &["changed", "alerts"], false);
    let mut va = Vault::open(pa.clone(), human(), "cli").unwrap();
    let mut vb = Vault::open(pb.clone(), human(), "cli").unwrap();

    // A write on A reaches B's subscribers through the shim.
    let id = add(&mut va, "from device A");
    let got = wait_event(&mut sub_b, Duration::from_secs(3), |e, d| e == "changed" && d["ids"].as_array().is_some_and(|a| a.iter().any(|x| x == id.as_str())));
    assert!(got.is_some(), "B never heard about A's write");

    // Concurrent text edits become the same conflict on both devices.
    paused.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    vb.catch_up().unwrap();
    for (v, text) in [(&mut va, "edited on A"), (&mut vb, "edited on B")] {
        v.transact(|s| {
            let mut b = TxBuilder::new(s, thc_core::dates::today());
            b.set_text(&id, text)?;
            Ok((b.finish(), ()))
        })
        .unwrap();
    }
    // Alert X is acked on A before B ever sees it; alert Y is not.
    let (_, alert_x) = add_alert_now(&mut va, "ack me early");
    va.transact(|_| Ok((vec![Op::AlertAck { id: alert_x.clone() }], ()))).unwrap();
    let (_, alert_y) = add_alert_now(&mut va, "fire everywhere");
    paused.store(false, Ordering::SeqCst);

    let fired_b = wait_event(&mut sub_b, Duration::from_secs(4), |e, d| e == "alert.fire" && d.to_string().contains(&alert_y));
    assert!(fired_b.is_some(), "B didn't fire Y");
    let fired_a = wait_event(&mut sub_a, Duration::from_secs(4), |e, d| e == "alert.fire" && d.to_string().contains(&alert_y));
    assert!(fired_a.is_some(), "A didn't fire Y");
    // X never fires anywhere; Y doesn't fire twice.
    std::thread::sleep(Duration::from_millis(600));
    for sub in [&mut sub_a, &mut sub_b] {
        assert!(wait_event(sub, Duration::from_millis(300), |e, d| e == "alert.fire" && d.to_string().contains(&alert_x)).is_none(), "acked alert fired");
        assert!(wait_event(sub, Duration::from_millis(300), |e, d| e == "alert.fire" && d.to_string().contains(&alert_y)).is_none(), "fired twice");
    }
    // Acting on Y on A withdraws B's notification once it syncs.
    va.transact(|_| Ok((vec![Op::AlertAck { id: alert_y.clone() }], ()))).unwrap();
    let withdrawn = wait_event(&mut sub_b, Duration::from_secs(3), |e, d| e == "alert.withdraw" && d.to_string().contains(&alert_y));
    assert!(withdrawn.is_some(), "B didn't withdraw Y after the ack synced");

    // Convergence: both devices end up identical, with the conflict on both.
    std::thread::sleep(Duration::from_millis(500));
    va.catch_up().unwrap();
    vb.catch_up().unwrap();
    assert_eq!(dump(&va), dump(&vb));
    assert_eq!(va.store.open_conflicts().unwrap().len(), 1);
    assert_eq!(vb.store.open_conflicts().unwrap().len(), 1);

    stop(&pa, ha);
    stop(&pb, hb);
    stop_shim.store(true, Ordering::SeqCst);
    sh.join().unwrap();
}

#[test]
fn a_delivering_client_suppresses_the_os_fallback() {
    let root = tmp("claim");
    let p = paths(&root, 0);
    let h = start(&p);
    let mut bar = subscribe(&p, &["alerts"], true);
    let mut v = Vault::open(p.clone(), human(), "cli").unwrap();
    // Claimed: the bar answers alert.delivered within the grace period.
    let (_, claimed) = add_alert_now(&mut v, "claimed by the bar");
    let ev = wait_event(&mut bar, Duration::from_secs(3), |e, d| e == "alert.fire" && d.to_string().contains(&claimed)).expect("no alert.fire");
    let _ = ev;
    bar.call("alert.delivered", json!({ "alerts": [claimed] })).unwrap();
    // Unclaimed: no answer, so the daemon falls back after the grace period.
    let (_, unclaimed) = add_alert_now(&mut v, "nobody claims this");
    wait_event(&mut bar, Duration::from_secs(3), |e, d| e == "alert.fire" && d.to_string().contains(&unclaimed)).expect("no alert.fire");
    std::thread::sleep(thc_daemon::DELIVERY_GRACE + Duration::from_millis(600));
    let logged = thc_daemon::LOGGED.lock().unwrap().clone();
    assert!(logged.contains(&unclaimed), "fallback missing for the unclaimed alert");
    assert!(!logged.contains(&claimed), "fallback fired despite the claim");
    stop(&p, h);
}

/// One daemon per vault: a second `run` (a login item racing the app) finds the first and
/// returns Ok, so launchd's KeepAlive (SuccessfulExit=false) doesn't respawn it.
#[test]
fn a_second_daemon_for_the_same_vault_exits_cleanly() {
    let root = tmp("dup");
    let p = paths(&root, 0);
    let h = start(&p);
    let r = thc_daemon::run(p.clone(), thc_daemon::RunOpts { json: true, channel: Some(thc_daemon::Channel::Log), label: None, host: false });
    assert!(r.is_ok(), "duplicate run should exit 0: {r:?}");
    // The first is untouched.
    assert!(connect(&p).call("status", Value::Null).is_ok());
    stop(&p, h);
}

/// Protocol v2 documents (mac-editor-arch.md §4, §6): a journal day is empty until its first save,
/// block ops land in one tx, reads come back in order, and a line changed elsewhere is a conflict.
#[test]
fn documents_as_blocks_over_the_socket() {
    let root = tmp("blocks");
    let p = paths(&root, 0);
    let h = start(&p);
    let mut c = connect(&p);
    let day = c.call("journal", json!({ "date": "2026-10-04" })).unwrap();
    assert!(day["root"].is_null() && day["blocks"].as_array().unwrap().is_empty(), "{day}");
    let (a, b) = (thc_core::id::new_id(), thc_core::id::new_id());
    let r = c
        .call(
            "blocks.apply",
            json!({ "journal": "2026-10-04", "ops": [
                { "op": "create", "id": a, "kind": "para", "text": "A quiet morning." },
                { "op": "create", "id": b, "after": a, "kind": "task", "text": "Call Sam due:2026-10-05" },
            ]}),
        )
        .unwrap();
    assert!(r["tx"].is_string(), "{r}");
    assert!(r["results"].as_array().unwrap().iter().all(|x| x["state"] == "ok"), "{r}");
    let day = c.call("journal", json!({ "date": "2026-10-04" })).unwrap();
    let blocks = day["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["kind"], "para");
    assert_eq!((blocks[1]["text"].as_str(), blocks[1]["due"].as_str()), (Some("Call Sam"), Some("2026-10-05")));
    let root_id = day["root"].as_str().unwrap().to_string();
    // locate: a block on a day lives in that journal.
    let loc = c.call("locate", json!({ "id": b })).unwrap();
    assert_eq!((loc["root"].as_str(), loc["journal"].as_str()), (Some(root_id.as_str()), Some("2026-10-04")), "{loc}");
    // The finder's pages: a journal day isn't a page.
    let pages = c.call("pages", json!({})).unwrap();
    assert!(pages["pages"].as_array().unwrap().iter().all(|p| p["id"] != root_id.as_str()), "{pages}");
    let page = c.call("page", json!({ "id": root_id })).unwrap();
    assert_eq!(page["blocks"].as_array().unwrap().len(), 2);
    // A line changed elsewhere after the editor read it: the editor's save becomes a conflict.
    // The result's text_rev is the committed one: reusing it as the next edit's base is no
    // conflict (it was the plan's provisional id, so a client's second save of a line conflicted).
    let first = c.call("blocks.apply", json!({ "root": root_id, "ops": [{ "op": "edit", "id": b, "text": "Call Sam" }] })).unwrap();
    let rev = first["results"][0]["block"]["text_rev"].as_str().unwrap().to_string();
    assert!(!rev.starts_with("preview-"), "a committed revision: {rev}");
    let again = c.call("blocks.apply", json!({ "root": root_id, "ops": [{ "op": "edit", "id": b, "text": "Call Sam today", "base": rev }] })).unwrap();
    assert_ne!(again["results"][0]["block"]["conflict"], true, "{again}");
    let now = c.call("blocks", json!({ "root": root_id, "ids": [b] })).unwrap();
    let base = now["blocks"][0]["text_rev"].as_str().unwrap().to_string();
    c.call("blocks.apply", json!({ "root": root_id, "ops": [{ "op": "edit", "id": b, "text": "Call Sam back" }] })).unwrap();
    c.call("blocks.apply", json!({ "root": root_id, "ops": [{ "op": "edit", "id": b, "text": "Call Sam about Lisbon", "base": base }] })).unwrap();
    let got = c.call("blocks", json!({ "root": root_id, "ids": [b] })).unwrap();
    assert_eq!(got["blocks"][0]["text"], "Call Sam about Lisbon");
    assert_eq!(got["blocks"][0]["conflict"], true, "{got}");
    // backlinks: a line on the day mentioning a page shows under that page.
    c.call("blocks.apply", json!({ "root": root_id, "ops": [{ "op": "create", "id": thc_core::id::new_id(), "kind": "bullet", "text": "Plan for [[Offsite notes]]" }] })).unwrap();
    let page_id = c.call("query", json!({ "q": "is:page", "limit": 5 })).unwrap()["items"][0]["id"].as_str().unwrap().to_string();
    let bl = c.call("backlinks", json!({ "id": page_id })).unwrap();
    assert_eq!(bl["count"], 1, "{bl}");
    assert!(bl["items"][0]["place"].is_string(), "{bl}");
    let found = c.call("search", json!({ "text": "Lisbon" })).unwrap();
    assert!(found["count"].as_u64().unwrap() >= 1, "{found}");
    let log = c.call("log", json!({ "limit": 5 })).unwrap();
    assert!(!log["txs"].as_array().unwrap().is_empty() && log["txs"][0]["ops"].as_array().is_some(), "{log}");
    // The compare popover's reads and BOTH (editor.md §8.1): the editor's text stays on the line,
    // the other version goes right below it.
    let cs = c.call("conflicts", json!({ "id": b })).unwrap();
    let item = &cs["items"][0];
    assert_eq!(item["kind"], "text", "{cs}");
    let texts = [item["current"]["text"].as_str().unwrap(), item["other"]["text"].as_str().unwrap()];
    assert!(texts.contains(&"Call Sam about Lisbon") && texts.contains(&"Call Sam back"), "{cs}");
    let mine = if texts[0] == "Call Sam about Lisbon" { "current" } else { "other" };
    let r = c.call("conflict.resolve", json!({ "id": b, "keep": "both", "top": mine })).unwrap();
    assert!(r["new_note"].is_string() && r["tx"].is_string(), "{r}");
    let after = c.call("page", json!({ "id": root_id })).unwrap();
    let lines: Vec<&str> = after["blocks"].as_array().unwrap().iter().map(|b| b["text"].as_str().unwrap()).collect();
    let i = lines.iter().position(|t| *t == "Call Sam about Lisbon").unwrap();
    assert_eq!(lines.get(i + 1), Some(&"Call Sam back"), "{lines:?}");
    assert!(c.call("conflicts", json!({ "id": b })).unwrap()["items"].as_array().unwrap().is_empty());
    stop(&p, h);
}
