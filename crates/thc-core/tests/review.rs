//! Review semantics (docs/design/agents.md §1): the queue, verdicts and the safe revert.

use std::fs;
use thc_core::builder::TxBuilder;
use thc_core::capture;
use thc_core::dates;
use thc_core::event::{Actor, Op};
use thc_core::review;
use thc_core::vault::{self, Paths, Vault};

fn open() -> (Vault, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("thc-review-{}", thc_core::id::new_id()));
    vault::init(&root.join("v"), None, None).unwrap();
    let paths = Paths { vault: root.join("v"), cache: root.join("cache") };
    (Vault::open(paths, human(), "test").unwrap(), root)
}

fn human() -> Actor {
    Actor { kind: "human".into(), name: None }
}

fn claude() -> Actor {
    Actor { kind: "agent".into(), name: Some("claude".into()) }
}

fn write(v: &mut Vault, actor: Actor, f: impl FnOnce(&mut TxBuilder) -> anyhow::Result<()>) -> String {
    v.actor = actor;
    let (ev, ()) = v
        .transact(|s| {
            let mut b = TxBuilder::new(s, dates::today());
            f(&mut b)?;
            Ok((b.finish(), ()))
        })
        .unwrap();
    ev[0].tx.clone()
}

fn ops(v: &mut Vault, actor: Actor, ops: Vec<Op>) {
    v.actor = actor;
    v.transact(|_| Ok((ops, ()))).unwrap();
}

fn create(v: &mut Vault, actor: Actor, text: &str) -> (String, String) {
    let mut id = String::new();
    let tx = write(v, actor, |b| {
        let cap = capture::parse(text, dates::today())?;
        id = b.create_from_capture(None, &cap, None)?;
        Ok(())
    });
    (id, tx)
}

#[test]
fn queue_holds_agent_txs_until_a_person_decides() {
    let (mut v, root) = open();
    create(&mut v, human(), "mine");
    let (_, t1) = create(&mut v, claude(), "[ ] agent task due:tomorrow");
    let (_, t2) = create(&mut v, claude(), "agent note #reading");
    assert_eq!(review::queue_txs(&v.store, None, None).unwrap(), vec![t1.clone(), t2.clone()]);

    // An agent can't accept: replay ignores it.
    ops(&mut v, claude(), vec![Op::TxReview { txs: vec![t1.clone()], verdict: "accepted".into() }]);
    assert_eq!(review::pending_count(&v.store).unwrap(), 2);

    ops(&mut v, human(), vec![Op::TxReview { txs: vec![t1.clone()], verdict: "accepted".into() }]);
    assert_eq!(review::queue_txs(&v.store, None, None).unwrap(), vec![t2.clone()]);

    // Undoing the accept (its stored inverse) puts it back.
    let accept_tx: String = v.store.conn.query_row("SELECT review_tx FROM reviews WHERE tx=?1", [&t1], |r| r.get(0)).unwrap();
    let inv = v.store.tx_inverse(&accept_tx).unwrap();
    assert_eq!(inv, vec![Op::TxUnreview { txs: vec![t1.clone()] }]);
    ops(&mut v, human(), inv);
    assert_eq!(review::pending_count(&v.store).unwrap(), 2);

    let item = review::item(&v.store, &t1, 1).unwrap();
    assert_eq!(item.changes[0].change, "create");
    assert_eq!(item.changes[0].fields["due"]["to"], dates::today().succ_opt().unwrap().format("%Y-%m-%d").to_string());
    assert_eq!(review::queue(&v.store, Some("someone-else"), None).unwrap().len(), 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn revert_keeps_fields_a_person_changed_afterwards() {
    let (mut v, root) = open();
    let (id, _) = create(&mut v, human(), "[ ] Dentist");
    let agent_tx = write(&mut v, claude(), |b| {
        b.set_text(&id, "Dentist, moved to 15:00")?;
        b.set_props(&id, &[("priority".into(), "high".into()), ("due".into(), "tomorrow".into())])
    });
    write(&mut v, human(), |b| b.set_props(&id, &[("priority".into(), "low".into())]));

    let item = review::item(&v.store, &agent_tx, 1).unwrap();
    assert!(item.later_changed_by_human);
    assert_eq!(item.changes[0].fields["text"]["from"], "[ ] Dentist".replace("[ ] ", ""));

    let plan = review::plan_revert(&v.store, std::slice::from_ref(&agent_tx), false).unwrap();
    assert_eq!(plan.skipped.len(), 1);
    assert_eq!(plan.skipped[0].field, "priority");
    ops(&mut v, human(), plan.ops);
    let n = v.store.must_node(&id).unwrap();
    assert_eq!(n.text, "Dentist");
    assert_eq!(n.due, None);
    assert_eq!(n.priority.as_deref(), Some("low"), "the later edit is kept");
    assert_eq!(v.store.verdict(&agent_tx).unwrap().as_deref(), Some("reverted"));
    assert_eq!(review::pending_count(&v.store).unwrap(), 0);

    // --force reverts it anyway.
    let plan = review::plan_revert(&v.store, std::slice::from_ref(&agent_tx), true).unwrap();
    assert!(plan.skipped.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn reverting_several_agent_txs_together_ignores_their_own_overlap() {
    let (mut v, root) = open();
    let (id, _) = create(&mut v, human(), "[ ] Report due:2026-10-10");
    let a = write(&mut v, claude(), |b| b.set_props(&id, &[("due".into(), "2026-10-12".into())]));
    let b2 = write(&mut v, claude(), |b| b.set_props(&id, &[("due".into(), "2026-10-14".into())]));
    let plan = review::plan_revert(&v.store, &[a.clone(), b2.clone()], false).unwrap();
    assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
    ops(&mut v, human(), plan.ops);
    assert_eq!(v.store.must_node(&id).unwrap().due.as_deref(), Some("2026-10-10"));
    assert_eq!(review::revertible_txs(&v.store, "claude", 0).unwrap().len(), 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn reverting_a_create_skips_nodes_a_person_edited() {
    let (mut v, root) = open();
    let (id, tx) = create(&mut v, claude(), "agent note");
    write(&mut v, human(), |b| b.set_text(&id, "agent note, fixed by me"));
    let plan = review::plan_revert(&v.store, std::slice::from_ref(&tx), false).unwrap();
    assert_eq!(plan.skipped.len(), 1);
    ops(&mut v, human(), plan.ops);
    assert!(!v.store.must_node(&id).unwrap().deleted);
    let _ = fs::remove_dir_all(root);
}
