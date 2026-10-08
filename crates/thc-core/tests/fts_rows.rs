//! The search index follows every change by each node's own row (fts_rows), and stays right
//! when the map is stale (another version of thc wrote the store meanwhile).
use thc_core::{
    event::{Actor, Event, Op},
    hlc::Hlc,
    store::Store,
};

fn apply(s: &Store, seq: u64, op: Op) {
    s.apply(&Event { v: 1, eid: format!("e{seq:011}"), hlc: Hlc(seq, 0), dev: "test".into(), actor: Actor { kind: "human".into(), name: None }, via: "test".into(), tx: format!("t{seq}"), op }).unwrap();
}

fn create(id: &str, text: &str) -> Op {
    Op::NodeCreate { id: id.into(), parent: None, order: "a".into(), title: None, text: text.into(), props: Default::default() }
}

fn found(s: &Store, words: &str) -> Vec<String> {
    let mut v: Vec<String> = s.search(words, 100).unwrap().into_iter().map(|n| n.id).collect();
    v.sort();
    v
}

fn rows(s: &Store) -> i64 {
    s.conn.query_row("SELECT count(*) FROM nodes_fts", [], |r| r.get(0)).unwrap()
}

#[test]
fn edits_and_deletes_keep_one_row_per_node() {
    let s = Store::open_memory().unwrap();
    for i in 0..50u64 {
        apply(&s, i + 1, create(&format!("node{i:08}"), &format!("alpha {i}")));
    }
    apply(&s, 100, Op::NodeText { id: "node00000007".into(), text: "bravo".into(), base: None });
    apply(&s, 101, Op::NodeDelete { id: "node00000009".into() });
    assert_eq!(found(&s, "bravo"), ["node00000007"]);
    assert_eq!(found(&s, "alpha").len(), 48);
    assert_eq!(rows(&s), 49);
    // A stale map (an older thc rewrote the rows): the next change still leaves one row each.
    s.conn.execute_batch("DELETE FROM nodes_fts WHERE id='node00000003'; INSERT INTO nodes_fts(id,title,text) VALUES('node00000003','','alpha 3');").unwrap();
    apply(&s, 102, Op::NodeText { id: "node00000003".into(), text: "charlie".into(), base: None });
    assert_eq!(found(&s, "charlie"), ["node00000003"]);
    assert_eq!(rows(&s), 49);
    assert_eq!(found(&s, "alpha").len(), 47);
}
