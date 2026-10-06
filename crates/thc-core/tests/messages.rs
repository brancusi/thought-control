//! Per-actor receipt fields survive independent devices and a full replay.
use thc_core::{
    builder::TxBuilder,
    event::Actor,
    messages::{self, Recipient},
    vault::{self, Paths, Vault},
};

#[test]
fn receipt_writes_on_two_devices_converge_without_hiding_other_actors() {
    let root = std::env::temp_dir().join(format!("thc-messages-{}", thc_core::id::new_id()));
    let open = |name: &str| {
        let path = root.join(name);
        vault::init(&path, None, None).unwrap();
        Vault::open(Paths { vault: path, cache: root.join(format!("cache-{name}")) }, Actor { kind: "agent".into(), name: Some(name.into()) }, "test").unwrap()
    };
    let mut a = open("codex-engineer-2");
    let mut b = open("claude-engineer");
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
    let (mut union, id) = a
        .transact(|s| {
            let mut tx = TxBuilder::new(s, today);
            let id = messages::send(&mut tx, "engineer", "claude-pm", "build", None, None)?;
            Ok((tx.finish(), id))
        })
        .unwrap();
    for e in &union {
        b.store.apply(e).unwrap();
    }
    let r_a = Recipient::new(Some("engineer"), Some("codex-engineer-2"));
    let r_b = Recipient::new(Some("engineer"), Some("claude-engineer"));
    for (v, recipient) in [(&mut a, &r_a), (&mut b, &r_b)] {
        let (events, ()) = v
            .transact(|s| {
                let mut tx = TxBuilder::new(s, today);
                messages::mark_read(&mut tx, std::slice::from_ref(&id), recipient)?;
                Ok((tx.finish(), ()))
            })
            .unwrap();
        union.extend(events);
    }
    union.sort_by_key(|e| e.order_key());
    union.dedup_by(|a, b| a.eid == b.eid);
    let replay = thc_core::store::Store::open_memory().unwrap();
    for e in &union {
        replay.apply(e).unwrap();
    }
    assert!(messages::list(&replay, &r_a, true, 10).unwrap().is_empty());
    assert!(messages::list(&replay, &r_b, true, 10).unwrap().is_empty());
    let third = Recipient::new(Some("engineer"), Some("grok-engineer"));
    assert_eq!(messages::list(&replay, &third, true, 10).unwrap().len(), 1);
    assert_eq!(replay.props_of(&id).unwrap()["read_codex-engineer-2"], true);
    assert_eq!(replay.props_of(&id).unwrap()["read_claude-engineer"], true);
}
