//! A bare `m` is rejected in user input, but stored data keeps its meaning (docs/FORMAT.md):
//! alert offsets are re-read at replay with `m` = minutes, and repeats are stored as RRULEs,
//! so logs written before the change replay exactly as they did.

use serde_json::json;
use thc_core::builder::TxBuilder;
use thc_core::capture;
use thc_core::event::{Actor, Op, Trigger};
use thc_core::vault::{self, Paths, Vault};

#[test]
fn old_minutes_and_repeats_replay_unchanged() {
    let root = std::env::temp_dir().join(format!("thc-minutes-{}", thc_core::id::new_id()));
    vault::init(&root.join("v"), None, None).unwrap();
    let paths = Paths { vault: root.join("v"), cache: root.join("cache") };
    let mut v = Vault::open(paths, Actor { kind: "human".into(), name: None }, "test").unwrap();
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
    let (_, id) = v
        .transact(|s| {
            let mut b = TxBuilder::new(s, today);
            let cap = capture::parse("[ ] Pay rent due:2026-10-05T12:00", today)?;
            let id = b.create_from_capture(None, &cap, None)?;
            // As an older thc wrote them: an offset in bare minutes, and `every 1m` (monthly).
            b.ops.push(Op::NodeSet {
                id: id.clone(),
                props: json!({ "repeat": { "rule": "FREQ=MONTHLY;INTERVAL=1", "mode": "fixed", "text": "every 1m" } }).as_object().unwrap().clone(),
            });
            b.add_alert(&id, Trigger { at: None, offset: Some("-15m".into()), anchor: Some("due".into()) })?;
            Ok((b.finish(), id))
        })
        .unwrap();
    let check = |v: &Vault| {
        let a = v.store.alerts_of(&id).unwrap();
        assert_eq!(a[0].fire_at.as_deref(), Some("2026-10-05T11:45"), "-15m is fifteen minutes");
        let n = v.store.node(&id).unwrap().unwrap();
        assert_eq!(n.repeat.as_ref().unwrap()["rule"], "FREQ=MONTHLY;INTERVAL=1");
    };
    check(&v);
    v.rebuild().unwrap();
    check(&v);
    // Completing advances by a month, as `every 1m` always meant.
    v.transact(|s| {
        let mut b = TxBuilder::new(s, today);
        b.complete(&id)?;
        Ok((b.finish(), ()))
    })
    .unwrap();
    v.rebuild().unwrap();
    assert_eq!(v.store.node(&id).unwrap().unwrap().due.as_deref(), Some("2026-11-05T12:00"));
    let _ = std::fs::remove_dir_all(root);
}
