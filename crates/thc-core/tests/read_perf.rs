//! Read correctness and bounded SQL work even when planner statistics lag behind writes.
use thc_core::{builder::TxBuilder, capture, dates, event::Actor, query, store::Store, vault::{self, Paths, Vault}};

fn insert(store: &Store, id: &str, status: &str) {
    store.conn.execute("INSERT INTO nodes(id,ord,text,status,created_ms,created_by,updated_ms) VALUES(?1,'b',?1,?2,0,'human',0)", [id, status]).unwrap();
}

#[test]
fn ready_checks_edges_in_bounded_work_with_stale_statistics() {
    let store = Store::open_memory().unwrap();
    for i in 0..1000 { insert(&store, &format!("old{i:09}"), "done"); }
    insert(&store, "task00000000", "waiting");
    store.analyze(); // Just one open task when the statistics were collected.
    for i in 1..500 { insert(&store, &format!("task{i:08}"), "todo"); }
    store.conn.execute_batch("INSERT INTO edges VALUES('task00000000','blocks','task00000010');
        INSERT INTO edges VALUES('old000000000','blocks','task00000001');
        UPDATE nodes SET scheduled='2099-01-01' WHERE id='task00000003';
        UPDATE nodes SET status='done' WHERE id='task00000004';").unwrap();
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
    let ready = store.query("is:ready", today, 1000).unwrap();
    assert_eq!(ready.len(), 496);
    assert!(ready.iter().any(|n| n.id == "task00000001"), "a closed blocker doesn't block");
    assert!(!ready.iter().any(|n| n.id == "task00000010"), "a waiting blocker blocks");
    assert_eq!(store.query("is:blocked", today, 1000).unwrap().iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["task00000010"]);
    store.conn.execute("UPDATE nodes SET deleted=1 WHERE id='task00000000'", []).unwrap();
    assert!(store.query("is:ready", today, 1000).unwrap().iter().any(|n| n.id == "task00000010"));
    assert!(store.query("is:blocked", today, 1000).unwrap().is_empty());
    let c = query::compile("is:ready", &store, today).unwrap();
    let sql = format!("SELECT n.id FROM nodes n WHERE {} {} LIMIT 100", c.where_sql, c.order_sql);
    let p: Vec<&dyn rusqlite::ToSql> = c.params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
    let mut st = store.conn.prepare(&sql).unwrap();
    let ids = st.query_map(p.as_slice(), |r| r.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(ids.len(), 100);
    let steps = st.get_status(rusqlite::StatementStatus::VmStep);
    assert!(steps < 100_000, "ready did {steps} VM steps: blocker lookup must not loop through all open tasks per candidate");
}

#[test]
fn cached_metadata_statements_observe_later_writes() {
    let s = Store::open_memory().unwrap();
    insert(&s, "abcde0000000", "todo");
    assert_eq!(s.short("abcde0000000"), "abcde");
    assert!(s.tags_of("abcde0000000").unwrap().is_empty());
    assert!(s.props_of("abcde0000000").unwrap().is_empty());
    assert_eq!(s.rev("abcde0000000"), None);
    insert(&s, "abcde1000000", "todo");
    insert(&s, "tag000000000", "done");
    s.conn.execute_batch("UPDATE nodes SET title='work' WHERE id='tag000000000';
        INSERT INTO edges VALUES('abcde0000000','tag','tag000000000');
        INSERT INTO props VALUES('abcde0000000','client','\"acme\"');
        INSERT INTO events VALUES('event1','1',1,'d','human','test','tx','test','abcde0000000','{}','[]');").unwrap();
    assert_eq!(s.short("abcde0000000"), "abcde0");
    assert_eq!(s.tags_of("abcde0000000").unwrap(), ["work"]);
    assert_eq!(s.props_of("abcde0000000").unwrap()["client"], "acme");
    assert_eq!(s.rev("abcde0000000").as_deref(), Some("event1"));
    s.conn.execute_batch("DELETE FROM edges; DELETE FROM props;
        INSERT INTO events VALUES('event2','2',2,'d','human','test','tx2','test','abcde0000000','{}','[]');").unwrap();
    assert!(s.tags_of("abcde0000000").unwrap().is_empty());
    assert!(s.props_of("abcde0000000").unwrap().is_empty());
    assert_eq!(s.rev("abcde0000000").as_deref(), Some("event2"));
}

#[test]
fn large_local_batch_refreshes_statistics_without_prior_queries() {
    let root = thc_core::scratch::dir(&format!("thc-read-stats-{}", thc_core::id::new_id()));
    vault::init(&root.join("vault"), None, None).unwrap();
    let mut v = Vault::open(Paths { vault: root.join("vault"), cache: root.join("cache") }, Actor { kind: "human".into(), name: None }, "test").unwrap();
    let today = dates::today();
    let add = |v: &mut Vault, count| {
        v.transact(|s| {
            let mut b = TxBuilder::new(s, today);
            for _ in 0..count { b.create_from_capture(None, &capture::parse("[ ] sample", today)?, None)?; }
            Ok((b.finish(), ()))
        }).unwrap();
    };
    add(&mut v, 500);
    let count = || v.store.conn.query_row("SELECT stat FROM sqlite_stat1 WHERE idx='sqlite_autoindex_nodes_1'", [], |r| r.get::<_, String>(0)).unwrap().split_whitespace().next().unwrap().parse::<usize>().unwrap();
    let before = count();
    assert!(before >= 500);
    add(&mut v, 6000); // More than SQLite's 10-fold growth trigger, on the same writer connection.
    let after: String = v.store.conn.query_row("SELECT stat FROM sqlite_stat1 WHERE idx='sqlite_autoindex_nodes_1'", [], |r| r.get(0)).unwrap();
    let after = after.split_whitespace().next().unwrap().parse::<usize>().unwrap();
    assert!(after > before * 10, "large local batches must refresh stale stats: {before}→{after}");
    drop(v);
    std::fs::remove_dir_all(root).unwrap();
}
