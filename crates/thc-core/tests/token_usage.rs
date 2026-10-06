//! Local usage snapshots never become replay inputs; only coarse/final props are logged.
use std::{fs, path::PathBuf};
use thc_core::{
    builder::TxBuilder,
    capture, dates,
    event::Actor,
    status::Tokens,
    token_usage as u,
    vault::{self, Paths, Vault},
};

fn root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("thc-usage-{name}-{}", thc_core::id::new_id()));
    fs::create_dir_all(&p).unwrap();
    p
}
fn event_count(v: &Vault) -> i64 {
    v.store.conn.query_row("SELECT count(*) FROM events", [], |r| r.get(0)).unwrap()
}
fn props(v: &mut Vault, id: &str, kv: &[(&str, &str)]) {
    v.transact(|s| {
        let mut b = TxBuilder::new(s, dates::today());
        b.set_props(id, &kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>())?;
        Ok((b.finish(), ()))
    })
    .unwrap();
}

#[test]
fn minute_cache_hourly_checkpoint_final_reconciliation_and_stale_claim() {
    let root = root("cache");
    vault::init(&root.join("vault"), None, None).unwrap();
    let mut v =
        Vault::open(Paths { vault: root.join("vault"), cache: root.join("cache") }, Actor { kind: "agent".into(), name: Some("test".into()) }, "test").unwrap();
    let id = v
        .transact(|s| {
            let mut b = TxBuilder::new(s, dates::today());
            let id = b.create_from_capture(None, &capture::parse("todo: tracked work", dates::today())?, None)?;
            Ok((b.finish(), id))
        })
        .unwrap()
        .1;
    props(&mut v, &id, &[("status", "doing"), ("session_id", "session-one"), ("owner", "test")]);
    let i = u::input(&v.store, &id).unwrap().unwrap();
    let before = event_count(&v);
    let mut tokens = Tokens { input: 100, out: 20, cache: 50, source: Some("transcript".into()), sessions: i.sessions.clone() };
    for minute in 1..60 {
        tokens.input += 1;
        u::apply(&mut v, &i, &tokens, i.started + minute * 60_000).unwrap();
    }
    assert_eq!(event_count(&v), before, "minute snapshots must not grow the log");
    assert_eq!(u::cached(&v.store, &id).unwrap().unwrap().tokens, tokens);
    assert!(Tokens::of(&v.store.props_of(&id).unwrap()).is_none());
    let now = dates::now_local();
    let report = thc_core::status::report(&v.store, None, thc_core::status::Range::parse("all", now).unwrap(), None, now).unwrap();
    assert_eq!((report.tokens.input, report.tokens.known, report.tokens.of), (159, 1, 1));
    assert_eq!(report.actors[0].tokens.input, 159);
    u::apply(&mut v, &i, &tokens, i.started + u::CHECKPOINT_MS).unwrap();
    let checkpoint = event_count(&v);
    assert!(checkpoint > before);
    assert_eq!(Tokens::of(&v.store.props_of(&id).unwrap()).unwrap().input, 159);
    tokens.input += 1;
    u::apply(&mut v, &i, &tokens, i.started + u::CHECKPOINT_MS + 60_000).unwrap();
    assert_eq!(event_count(&v), checkpoint);
    // Completion changes the signature: an old sample cannot overwrite final usage.
    props(&mut v, &id, &[("status", "done")]);
    let before_final = event_count(&v);
    assert!(!u::apply(&mut v, &i, &tokens, i.started + u::CHECKPOINT_MS + 120_000).unwrap());
    assert_eq!(event_count(&v), before_final);
    let final_i = u::input(&v.store, &id).unwrap().unwrap();
    assert!(u::cached(&v.store, &id).unwrap().is_none());
    u::apply(&mut v, &final_i, &tokens, final_i.done.unwrap() + 100).unwrap();
    assert!(event_count(&v) > before_final);
    let final_count = event_count(&v);
    u::apply(&mut v, &final_i, &tokens, final_i.done.unwrap() + 60_000).unwrap();
    assert_eq!(event_count(&v), final_count, "unchanged finals must be idempotent");
    assert!(u::pending(&v.store).unwrap().is_empty());
    assert_eq!(v.actor.name.as_deref(), Some("test"));
    assert!(v.store.history_where("actor='agent:collector'", &[]).unwrap().len() > 0);
    props(&mut v, &id, &[("status", "doing"), ("session_id", "session-two")]);
    assert!(u::cached(&v.store, &id).unwrap().is_none());
    assert!(!u::apply(&mut v, &final_i, &tokens, final_i.done.unwrap() + 120_000).unwrap());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn codex_without_claude_directory_counts_cumulative_deltas_once_inside_window() {
    let root = root("codex");
    let dir = root.join(".codex/sessions/2026/10/06");
    fs::create_dir_all(&dir).unwrap();
    let line = |seconds: i64, input: u64, output: u64, cache: u64| {
        serde_json::json!({"timestamp":chrono::DateTime::from_timestamp(seconds,0).unwrap().to_rfc3339(),"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":input,"output_tokens":output,"cached_input_tokens":cache},"last_token_usage":{"input_tokens":100,"output_tokens":10,"cached_input_tokens":20}}}}).to_string()+"\n"
    };
    let file = dir.join("rollout-any-sess-one.jsonl");
    fs::write(&file, [line(1, 100, 10, 20), line(3, 300, 30, 70), line(4, 300, 30, 70), line(6, 400, 40, 90), line(10, 1400, 140, 290)].concat()).unwrap();
    assert_eq!(u::usage(&root, "sess-one", 2000, 8000), Some((230, 30, 70)));
    assert_eq!(u::usage(&root, "../sess-one", 0, 8000), None);
    fs::write(&file, "{\"timestamp\":\"1970-01-01T00:00:03Z\",\"payload\":{\"type\":\"token_count\",\"info\":null}}\npartial").unwrap();
    assert_eq!(u::usage(&root, "sess-one", 2000, 8000), None);
    let last = serde_json::json!({"timestamp":"1970-01-01T00:00:03Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"output_tokens":10,"cached_input_tokens":20}}}}).to_string()+"\n";
    fs::write(&file, last.repeat(2)).unwrap();
    assert_eq!(u::usage(&root, "sess-one", 2000, 8000), Some((80, 10, 20)), "identical last-usage records count once");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn claude_streamed_messages_use_latest_usage_and_missing_window_is_unknown() {
    let root = root("claude");
    let dir = root.join(".claude/projects/scratch");
    fs::create_dir_all(&dir).unwrap();
    let line = |output: u64| {
        serde_json::json!({"timestamp":"1970-01-01T00:00:03Z","message":{"id":"same-message","usage":{"input_tokens":100,"output_tokens":output,"cache_creation_input_tokens":30,"cache_read_input_tokens":40}}}).to_string()+"\n"
    };
    fs::write(dir.join("sess-two.jsonl"), line(5) + &line(15) + "partial").unwrap();
    assert_eq!(u::usage(&root, "sess-two", 2000, 8000), Some((130, 15, 40)));
    assert_eq!(u::usage(&root, "sess-two", 5000, 8000), None);
    fs::remove_dir_all(root).unwrap();
}
