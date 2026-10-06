//! The status board, data half (docs/design/status.md): times derived from the
//! event log (§1), counts and medians that leave out untracked and batch work, bottlenecks first,
//! and the token collector (§2). Event times are pinned with THC_FIXTURE_IDS + THC_NOW.

mod common;

use serde_json::Value;
use std::path::PathBuf;

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = std::env::temp_dir().join(format!("thc-status-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap();
        let v = V { root };
        assert!(v.cmd("human", "2026-10-06T08:00").args(["init", "vault"]).output().unwrap().status.success());
        v
    }

    fn cmd(&self, actor: &str, now: &str) -> std::process::Command {
        let mut c = common::thc();
        c.current_dir(&self.root)
            .env("HOME", self.root.join("home"))
            .env("THC_VAULT", self.root.join("vault"))
            .env("THC_CACHE_DIR", self.root.join("cache"))
            .env("THC_BOARD", self.root.join("vault"))
            .env("THC_ACTOR", actor)
            .env("THC_NOW", now)
            .env("THC_FIXTURE_IDS", "1")
            .env("THC_DEVICE", "status-test");
        c
    }

    fn json(&self, actor: &str, now: &str, args: &[&str]) -> Value {
        let o = self.cmd(actor, now).arg("--json").args(args).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }

    fn id(&self, now: &str, args: &[&str]) -> String {
        self.json("human", now, args)["nodes"][0]["id"].as_str().unwrap().to_string()
    }
}

fn task<'a>(r: &'a Value, id: &str) -> &'a Value {
    r["tasks"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap_or_else(|| panic!("{id} not in {r}"))
}

#[test]
fn times_counts_and_bottlenecks_come_from_the_log() {
    let v = V::new("times");
    let page = v.id("2026-10-06T08:00", &["page", "new", "Issues"]);
    std::fs::write(v.root.join("vault/settings.toml"), "[capture]\ntarget = \"¶ Issues\"\n").unwrap();
    // A: filed 09:00, claimed 10:00, done 10:38: worked 38m, waited 1h.
    let a = v.id("2026-10-06T09:00", &["todo", "tracked work", "--under", &page]);
    v.json("claude", "2026-10-06T10:00", &["set", &a, "status=doing", "owner=claude", "--expect", "status=todo"]);
    v.json("claude", "2026-10-06T10:38", &["done", &a]);
    // B: done without a claim: untracked.
    let b = v.id("2026-10-06T09:00", &["todo", "never claimed", "--under", &page]);
    v.json("human", "2026-10-06T11:00", &["done", &b]);
    // C: claimed and done in the same minute: a batch close.
    let c = v.id("2026-10-06T09:30", &["todo", "batch closed", "--under", &page]);
    v.json("claude", "2026-10-06T11:10", &["set", &c, "status=doing", "owner=claude"]);
    v.json("claude", "2026-10-06T11:10", &["done", &c]);
    // D (in flight) blocks E, E blocks F: D holds up two.
    let d = v.id("2026-10-06T09:00", &["todo", "the bottleneck", "--under", &page]);
    let e = v.id("2026-10-06T09:00", &["todo", "waits on d", "--under", &page]);
    let f = v.id("2026-10-06T09:00", &["todo", "waits on e", "--under", &page]);
    v.json("codex", "2026-10-06T09:30", &["set", &d, "status=doing", "owner=codex"]);
    v.json("human", "2026-10-06T09:00", &["link", &d, &e, "--rel", "blocks"]);
    v.json("human", "2026-10-06T09:00", &["link", &e, &f, "--rel", "blocks"]);

    let r = v.json("human", "2026-10-06T12:00", &["status", "--since", "today"]);
    // S2: tracked times.
    let ta = task(&r, &a);
    assert_eq!((ta["worked"].as_i64(), ta["waited"].as_i64(), ta["lead"].as_i64()), (Some(38), Some(60), Some(98)), "{ta}");
    assert!(ta["started"].as_str().unwrap().contains("T10:00:00"), "{ta}");
    // S3: untracked has no started, worked or waited (absent, not null or 0).
    let tb = task(&r, &b);
    assert_eq!(tb["untracked"], true);
    for k in ["started", "worked", "waited"] {
        assert!(tb.get(k).is_none(), "{k} on an untracked task: {tb}");
    }
    assert_eq!(task(&r, &c)["batch"], true);
    // Counts: three done; medians leave out the untracked and the batch one.
    assert_eq!(r["counts"]["done"], 3);
    assert_eq!((r["counts"]["untracked"].as_u64(), r["counts"]["batch"].as_u64()), (Some(1), Some(1)));
    assert_eq!((r["timing"]["sample"].as_u64(), r["timing"]["worked_median"].as_i64()), (Some(1), Some(38)));
    assert_eq!(r["counts"]["doing"], 1);
    assert_eq!(r["counts"]["blocked"], 2);
    // S4: the bottleneck first, holding up both, directly e.
    let first = &r["blocked"][0];
    assert_eq!(first["id"], d.as_str());
    assert_eq!(first["holds_up"].as_array().unwrap().len(), 2);
    assert_eq!(first["direct"], serde_json::json!([e]));
    // Momentum: today, three done (10h, 11h).
    let today = &r["momentum"]["today"];
    assert_eq!(today.as_array().unwrap().iter().map(|h| h["done"].as_u64().unwrap()).sum::<u64>(), 3);
    // Tokens: none known, and it says so.
    assert_eq!((r["tokens"]["known"].as_u64(), r["tokens"]["of"].as_u64()), (Some(0), Some(4)));
    // Text: the same, readable; untracked says so.
    let o = v.cmd("human", "2026-10-06T12:00").args(["status", "--since", "today"]).output().unwrap();
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(text.contains("worked 38m") && text.contains("untracked") && text.contains("holds up 2 tasks"), "{text}");
    assert!(text.contains("tokens known for 0 of 4"), "{text}");
    // thc show: the times line (§3.4).
    let o = v.cmd("human", "2026-10-06T12:00").args(["show", &a, "--depth", "0"]).output().unwrap();
    let show = String::from_utf8_lossy(&o.stdout);
    assert!(show.contains("started 10:00 · done 10:38 · worked 38m · waited 1h 0m"), "{show}");
    // --by: one actor.
    let mine = v.json("human", "2026-10-06T12:00", &["status", "--since", "today", "--by", "claude"]);
    assert_eq!(mine["counts"]["done"], 2);
}

#[test]
fn collect_reads_usage_from_a_session_log_and_can_be_undone() {
    let v = V::new("collect");
    let page = v.id("2026-10-06T08:00", &["page", "new", "Issues"]);
    std::fs::write(v.root.join("vault/settings.toml"), "[capture]\ntarget = \"¶ Issues\"\n").unwrap();
    let a = v.id("2026-10-06T09:00", &["todo", "tracked work", "--under", &page]);
    v.json("claude", "2026-10-06T10:00", &["set", &a, "status=doing", "owner=claude", "session_id=sess-1"]);
    v.json("claude", "2026-10-06T10:38", &["done", &a]);
    // A Claude Code session log: two messages inside the window (one repeated per content
    // block), one before the claim, one after the done.
    use chrono::TimeZone;
    let at = |h: u32, m: u32| chrono::Local.with_ymd_and_hms(2026, 10, 6, h, m, 0).unwrap().to_rfc3339();
    let line = |ts: String, id: &str, i: u64, o: u64, cr: u64| format!("{{\"type\":\"assistant\",\"timestamp\":\"{ts}\",\"sessionId\":\"sess-1\",\"message\":{{\"id\":\"{id}\",\"content\":[],\"usage\":{{\"input_tokens\":{i},\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":{cr},\"output_tokens\":{o}}}}}}}\n");
    let dir = v.root.join("home/.claude/projects/-tmp-x");
    std::fs::create_dir_all(&dir).unwrap();
    let log = [line(at(9, 50), "m0", 999, 999, 999), line(at(10, 5), "m1", 100, 20, 1000), line(at(10, 5), "m1", 100, 20, 1000), line(at(10, 30), "m2", 300, 40, 5000), line(at(11, 0), "m3", 999, 999, 999)].concat();
    std::fs::write(dir.join("sess-1.jsonl"), log).unwrap();
    let r = v.json("human", "2026-10-06T12:00", &["status", "--since", "today", "--collect"]);
    let t = task(&r, &a);
    assert_eq!(t["tokens"]["in"], 400, "{t}");
    assert_eq!(t["tokens"]["out"], 60);
    assert_eq!(t["tokens"]["cache"], 6000);
    assert_eq!(t["tokens"]["source"], "transcript");
    assert_eq!((r["tokens"]["known"].as_u64(), r["tokens"]["of"].as_u64()), (Some(1), Some(1)));
    // Written by `collector`, one transaction: undo takes it back.
    let h = v.json("human", "2026-10-06T12:00", &["history", &a]);
    assert!(h.to_string().contains("collector"), "{h}");
    v.json("human", "2026-10-06T12:01", &["undo"]);
    let r = v.json("human", "2026-10-06T12:02", &["status", "--since", "today"]);
    assert!(task(&r, &a).get("tokens").is_none());
    // A session id that isn't a plain name is never read as a path.
    let b = v.id("2026-10-06T12:05", &["todo", "odd session", "--under", &page]);
    v.json("claude", "2026-10-06T12:06", &["set", &b, "status=doing", "owner=claude", "session_id=../-tmp-x/sess-1"]);
    v.json("claude", "2026-10-06T12:10", &["done", &b]);
    let r2 = v.json("human", "2026-10-06T12:30", &["status", "--since", "today", "--collect"]);
    assert!(task(&r2, &b).get("tokens").is_none(), "{}", task(&r2, &b));
}

#[test]
fn daemon_polls_live_cache_without_log_events_then_reconciles_rpc_completion() {
    use chrono::{Duration as CD, TimeZone};
    use std::{time::{Duration, Instant}, process::Stdio};
    use thc_core::{proto::Client, vault::Paths};
    let v = V::new("live");
    let now = chrono::Local::now().naive_local();
    let stamp = |t: chrono::NaiveDateTime| t.format("%Y-%m-%dT%H:%M").to_string();
    let at = |t: chrono::NaiveDateTime| chrono::Local.from_local_datetime(&t).unwrap().to_rfc3339();
    let current = stamp(now);
    let id = v.id(&stamp(now-CD::hours(1)), &["todo", "live usage"]);
    v.json("codex", &stamp(now-CD::minutes(5)), &["set", &id, "status=doing", "owner=codex", "session_id=live-codex"]);
    let dir = v.root.join("home/.codex/sessions/2026/10/06");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("rollout-scratch-live-codex.jsonl");
    let line = |t: chrono::NaiveDateTime, input: u64, output: u64| serde_json::json!({"timestamp":at(t),"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":input,"output_tokens":output,"cached_input_tokens":0},"last_token_usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":0}}}}).to_string()+"\n";
    let first = line(now-CD::minutes(3),100,20);
    std::fs::write(&file, &first).unwrap();
    let count = || v.json("human", &current, &["history", &id])["events"].as_array().unwrap().len();
    let before = count();
    let paths = Paths {vault:v.root.join("vault"),cache:v.root.join("cache")};
    let start = || common::Guard::spawn(v.cmd("human", &current).args(["daemon","run"]).env("THC_TEST_TOKEN_POLL_MS","200").stdout(Stdio::from(std::fs::File::create(v.root.join("daemon.log")).unwrap())).stderr(Stdio::from(std::fs::File::create(v.root.join("daemon.err")).unwrap())));
    let mut daemon = start();
    let deadline=Instant::now()+Duration::from_secs(15);
    let mut client = loop {
        if let Some(c)=Client::connect(&paths) { break c }
        assert!(Instant::now()<deadline,"daemon did not start: {} / {}",std::fs::read_to_string(v.root.join("daemon.log")).unwrap_or_default(),std::fs::read_to_string(v.root.join("daemon.err")).unwrap_or_default()); std::thread::sleep(Duration::from_millis(50));
    };
    let wait_usage = |client: &mut Client, expected: u64| {
        let deadline=Instant::now()+Duration::from_secs(10);
        loop {
            let r=client.call("tokens",serde_json::json!({})).unwrap();
            if r["items"].as_array().unwrap().iter().any(|x| x["id"]==id && x["tokens"]["in"]==expected) { break }
            assert!(Instant::now()<deadline,"usage {expected} never arrived: {r}"); std::thread::sleep(Duration::from_millis(50));
        }
    };
    wait_usage(&mut client,100);
    assert_eq!(count(),before,"startup collection before one hour is cache-only");
    client.hello("test", &["tokens"], false).unwrap();
    std::fs::write(&file, first.clone()+&line(now-CD::minutes(1),300,60)).unwrap();
    wait_usage(&mut client,300);
    assert_eq!(count(),before,"minute polling must not append props");
    assert!(client.pending_events.iter().any(|e| e.event=="tokens"));
    let r=v.json("human",&current,&["status","--since","all"]);
    assert_eq!(task(&r,&id)["tokens"]["in"],300);
    assert_eq!((r["tokens"]["in"].as_u64(),r["tokens"]["known"].as_u64(),r["tokens"]["of"].as_u64()),(Some(300),Some(1),Some(1)));
    // The cache is local and survives a daemon restart without checkpoint churn.
    client.call("shutdown",serde_json::json!({})).unwrap();
    let deadline=Instant::now()+Duration::from_secs(10);
    while daemon.0.try_wait().unwrap().is_none() { assert!(Instant::now()<deadline); std::thread::sleep(Duration::from_millis(50)); }
    daemon=start();
    let deadline=Instant::now()+Duration::from_secs(10);
    let mut client=loop {
        if let Some(c)=Client::connect(&paths) { break c }
        assert!(Instant::now()<deadline); std::thread::sleep(Duration::from_millis(50));
    };
    wait_usage(&mut client,300);
    assert_eq!(count(),before);
    // This completion goes through RPC (not CLI's after_done hook).
    client.call("complete",serde_json::json!({"id":id})).unwrap();
    std::fs::write(&file,first+&line(now-CD::minutes(1),300,60)+&line(now+CD::minutes(2),9999,9999)).unwrap();
    let deadline=Instant::now()+Duration::from_secs(10);
    loop {
        let r=v.json("human",&current,&["show",&id]);
        if r["props"]["tokens_source"]=="transcript" {
            assert_eq!(r["props"]["tokens_in"].as_f64(),Some(300.0)); break
        }
        assert!(Instant::now()<deadline,"final reconciliation never arrived: {r}"); std::thread::sleep(Duration::from_millis(50));
    }
    assert!(count()>before);
    client.call("shutdown",serde_json::json!({})).unwrap();
    let _ = daemon.0.wait();
}

#[test]
fn manual_live_collection_and_offline_done_respect_collector_policy() {
    use chrono::TimeZone;
    let v=V::new("offline");
    let id=v.id("2026-10-06T09:00",&["todo","offline tokens"]);
    v.json("codex","2026-10-06T10:00",&["set",&id,"status=doing","session_id=offline-codex"]);
    let dir=v.root.join("home/.codex/sessions"); std::fs::create_dir_all(&dir).unwrap();
    let line=serde_json::json!({"timestamp":chrono::Local.with_ymd_and_hms(2026,10,6,10,10,0).unwrap().to_rfc3339(),"payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":30}}}}).to_string()+"\n";
    std::fs::write(dir.join("rollout-scratch-offline-codex.jsonl"),line).unwrap();
    let dry=v.cmd("human","2026-10-06T10:15").args(["--dry-run","status","--collect"]).output().unwrap();
    assert!(dry.status.success());
    assert!(v.json("human","2026-10-06T10:15",&["show",&id])["props"].get("tokens_in").is_none());
    let r=v.json("human","2026-10-06T10:15",&["status","--collect"]);
    assert_eq!(task(&r,&id)["tokens"]["in"],70);
    v.json("codex","2026-10-06T10:20",&["done",&id]);
    let r=v.json("human","2026-10-06T10:20",&["show",&id]);
    assert_eq!(r["props"]["tokens_in"].as_f64(),Some(70.0));
    assert!(r["props"]["tokens_window"].as_str().unwrap().contains("\"done\":"));
    let denied=v.id("2026-10-06T09:00",&["todo","collector refused"]);
    v.json("codex","2026-10-06T10:00",&["set",&denied,"status=doing","session_id=offline-codex"]);
    let config=v.root.join("config");std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.toml"),"[actors.collector]\ndeny=[\"set\"]\n").unwrap();
    let o=v.cmd("codex","2026-10-06T10:20").env("THC_CONFIG_DIR",&config).args(["done",&denied]).output().unwrap();
    assert!(o.status.success(),"{}",String::from_utf8_lossy(&o.stderr));
    let r=v.json("human","2026-10-06T10:20",&["show",&denied]);
    assert_eq!(r["status"],"done");
    assert!(r["props"].get("tokens_in").is_none());
}
