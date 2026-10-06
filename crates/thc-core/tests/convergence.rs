//! Several devices edit concurrently, logs sync at random points (like Dropbox), and every
//! device must converge to an identical materialized state.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::fs;
use std::path::{Path, PathBuf};
use thc_core::builder::TxBuilder;
use thc_core::capture;
use thc_core::dates;
use thc_core::event::{Actor, Op};
use thc_core::review;
use thc_core::vault::{self, Paths, Vault};

struct Dev {
    vault: Vault,
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("thc-conv-{}-{}", name, thc_core::id::new_id()));
    fs::create_dir_all(&d).unwrap();
    d
}

fn open(root: &Path, i: usize) -> Dev {
    let vdir = root.join(format!("v{i}"));
    vault::init(&vdir, None, None).unwrap();
    let paths = Paths { vault: vdir.clone(), cache: root.join(format!("cache{i}")) };
    let actor = Actor { kind: "agent".into(), name: Some(format!("dev{i}")) };
    Dev { vault: Vault::open(paths, actor, "test").unwrap() }
}

/// Copy every log file to every vault (files are per-device, so this is a union).
fn sync(root: &Path, n: usize) {
    for src in 0..n {
        let src_log = root.join(format!("v{src}/log"));
        for dev in fs::read_dir(&src_log).unwrap() {
            let dev = dev.unwrap();
            for f in fs::read_dir(dev.path()).unwrap() {
                let f = f.unwrap();
                for dst in 0..n {
                    if dst == src {
                        continue;
                    }
                    let target = root.join(format!("v{dst}/log")).join(dev.file_name()).join(f.file_name());
                    let src_len = f.metadata().unwrap().len();
                    let dst_len = fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
                    if src_len > dst_len {
                        fs::create_dir_all(target.parent().unwrap()).unwrap();
                        fs::copy(f.path(), &target).unwrap();
                    }
                }
            }
        }
    }
}

fn dump(v: &Vault) -> String {
    let c = &v.store.conn;
    let mut out = String::new();
    let mut st = c
        .prepare(
            "SELECT id, coalesce(parent,''), ord, coalesce(title,''), text, coalesce(status,''), coalesce(scheduled,''), \
             coalesce(due,''), coalesce(priority,''), coalesce(repeat,''), coalesce(journal,''), is_tag, deleted FROM nodes ORDER BY id",
        )
        .unwrap();
    let rows = st
        .query_map([], |r| {
            let mut s = String::new();
            for i in 0..13 {
                let v: rusqlite::types::Value = r.get(i)?;
                s.push_str(&format!("{v:?}|"));
            }
            Ok(s)
        })
        .unwrap();
    for r in rows {
        out.push_str(&r.unwrap());
        out.push('\n');
    }
    let mut st = c.prepare("SELECT src||' '||rel||' '||dst FROM edges ORDER BY 1").unwrap();
    for r in st.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        out.push_str(&r.unwrap());
        out.push('\n');
    }
    let mut st = c.prepare("SELECT node||' '||key||' '||value FROM props ORDER BY 1").unwrap();
    for r in st.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        out.push_str(&r.unwrap());
        out.push('\n');
    }
    let mut st = c.prepare("SELECT tx||' '||verdict||' '||actor FROM reviews ORDER BY 1").unwrap();
    for r in st.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        out.push_str(&format!("review {}\n", r.unwrap()));
    }
    // Re-homing is derived: it must agree too (rows and their flags).
    let mut st = c.prepare("SELECT node||' '||coalesce(under,'-')||' '||deleted_parent FROM rehomed ORDER BY 1").unwrap();
    for r in st.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        out.push_str(&format!("rehomed {}\n", r.unwrap()));
    }
    let mut st = c.prepare("SELECT node||' '||coalesce(loser_value,'')||' '||loser_eid FROM conflicts WHERE field='rehomed' ORDER BY 1").unwrap();
    for r in st.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        out.push_str(&format!("rehomed-flag {}\n", r.unwrap()));
    }
    out.push_str(&format!("queue {:?}\n", review::queue_txs(&v.store, None, None).unwrap()));
    out
}

/// A line added on one device under a note another device deleted meanwhile: it shows under the
/// nearest live ancestor, flagged, on both devices; restoring the note moves it back (daemon.md
/// §4.0a). Re-homing is derived from the events, so whichever order they arrive in, the devices
/// agree.
#[test]
fn a_child_of_a_concurrently_deleted_note_is_rehomed_and_flagged() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
    let root = tmpdir("rehome");
    let n = 2;
    let mut devs: Vec<Dev> = (0..n).map(|i| open(&root, i)).collect();
    let mk = |d: &mut Dev, parent: Option<String>, text: &str| -> String {
        d.vault
            .transact(|s| {
                let mut b = TxBuilder::new(s, today);
                let cap = capture::parse(text, today)?;
                let id = b.create_from_capture(parent.clone(), &cap, None)?;
                Ok((b.finish(), id))
            })
            .unwrap()
            .1
    };
    let page = devs[0].vault.transact(|s| {
        let mut b = TxBuilder::new(s, today);
        let id = b.create_page("Offsite notes", &[])?;
        Ok((b.finish(), id))
    }).unwrap().1;
    let plan = mk(&mut devs[0], Some(page.clone()), "Plan the offsite");
    let _known = mk(&mut devs[0], Some(plan.clone()), "known child");
    sync(&root, n);
    devs[1].vault.catch_up().unwrap();
    // Offline: dev0 deletes the plan (and the child it knows); dev1 adds a child under it.
    devs[0].vault.transact(|s| {
        let mut b = TxBuilder::new(s, today);
        b.delete(&plan)?;
        Ok((b.finish(), ()))
    }).unwrap();
    let venue = mk(&mut devs[1], Some(plan.clone()), "Book the venue");
    sync(&root, n);
    for d in devs.iter_mut() {
        d.vault.catch_up().unwrap();
    }
    for d in &devs {
        let s = &d.vault.store;
        assert!(!s.node(&venue).unwrap().unwrap().deleted, "the new line is never deleted");
        assert_eq!(s.view_parent(&venue).unwrap().as_deref(), Some(page.as_str()), "it shows under the page");
        assert!(s.children(&page).unwrap().iter().any(|c| c.id == venue));
        let flags: i64 = s.conn.query_row("SELECT count(*) FROM conflicts WHERE node=?1 AND field='rehomed' AND resolved=0", [&venue], |r| r.get(0)).unwrap();
        assert_eq!(flags, 1, "flagged for the person");
        let blocks = thc_core::outline::render(s, &page).unwrap();
        let b = blocks.iter().find(|b| b.id == venue).expect("in the page's outline");
        assert!(b.conflict && b.parent.as_deref() == Some(page.as_str()));
    }
    assert_eq!(dump(&devs[1].vault), dump(&devs[0].vault));
    // Restoring the plan moves the line back and clears the flag, on both.
    devs[0].vault.transact(|s| {
        let mut b = TxBuilder::new(s, today);
        b.restore(&plan)?;
        Ok((b.finish(), ()))
    }).unwrap();
    sync(&root, n);
    for d in devs.iter_mut() {
        d.vault.catch_up().unwrap();
        let s = &d.vault.store;
        assert_eq!(s.view_parent(&venue).unwrap().as_deref(), Some(plan.as_str()));
        let flags: i64 = s.conn.query_row("SELECT count(*) FROM conflicts WHERE field='rehomed'", [], |r| r.get(0)).unwrap();
        assert_eq!(flags, 0);
    }
    assert_eq!(dump(&devs[1].vault), dump(&devs[0].vault));
    // A rebuild derives the same state.
    devs[1].vault.rebuild().unwrap();
    assert_eq!(dump(&devs[1].vault), dump(&devs[0].vault));
    let _ = fs::remove_dir_all(&root);
}

fn random_tx(dev: &Dev, rng: &mut StdRng, sql: &str) -> Option<String> {
    let mut st = dev.vault.store.conn.prepare(sql).unwrap();
    let txs: Vec<String> = st.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    if txs.is_empty() { None } else { Some(txs[rng.random_range(0..txs.len())].clone()) }
}

/// Review actions: a person accepts or withdraws a verdict, an agent reverts (safe revert plus
/// `tx.review reverted`), or an agent tries to accept, which replay must ignore.
fn review_op(dev: &mut Dev, rng: &mut StdRng, roll: u32) {
    let agent = dev.vault.actor.clone();
    let any_tx = "SELECT DISTINCT tx FROM events WHERE op NOT IN ('tx.review','tx.unreview') ORDER BY tx";
    let op = match roll {
        10 | 13 => random_tx(dev, rng, any_tx).map(|t| Op::TxReview { txs: vec![t], verdict: "accepted".into() }),
        11 => random_tx(dev, rng, "SELECT tx FROM reviews ORDER BY tx").map(|t| Op::TxUnreview { txs: vec![t] }),
        _ => None,
    };
    if roll == 12 {
        if let Some(t) = random_tx(dev, rng, any_tx) {
            dev.vault.transact(|s| Ok((review::plan_revert(s, &[t], false)?.ops, ()))).unwrap();
        }
        return;
    }
    let Some(op) = op else { return };
    if roll != 13 {
        dev.vault.actor = Actor { kind: "human".into(), name: None };
    }
    dev.vault.transact(|_| Ok((vec![op], ()))).unwrap();
    dev.vault.actor = agent;
}

fn random_op(dev: &mut Dev, rng: &mut StdRng, step: usize) {
    let roll = rng.random_range(0..14);
    if roll >= 10 {
        return review_op(dev, rng, roll);
    }
    let today = dates::today();
    let nodes: Vec<String> = dev
        .vault
        .store
        .nodes_where("n.deleted=0 AND n.is_tag=0 AND n.journal IS NULL ORDER BY n.id", &[])
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    let pick = |rng: &mut StdRng| -> Option<String> {
        if nodes.is_empty() { None } else { Some(nodes[rng.random_range(0..nodes.len())].clone()) }
    };
    let target = pick(rng);
    let other = pick(rng);
    let due_days = rng.random_range(0..10);
    dev.vault
        .transact(|s| {
            let mut b = TxBuilder::new(s, today);
            match (roll, target) {
                (0..=2, _) | (_, None) => {
                    let text = format!("task {step} #t{} due:+{due_days}d", step % 3);
                    let cap = capture::parse(&text, today)?;
                    let j = b.journal(today)?;
                    b.create_from_capture(Some(j), &cap, None)?;
                }
                (3, Some(t)) => b.set_text(&t, &format!("edited at step {step} [[Page {}]]", step % 2))?,
                (4, Some(t)) => b.set_props(&t, &[("due".into(), format!("+{due_days}d"))])?,
                (5, Some(t)) => b.set_props(&t, &[("priority".into(), "high".into())])?,
                (6, Some(t)) => {
                    let _ = b.complete(&t);
                }
                (7, Some(t)) => {
                    if let Some(o) = other {
                        let _ = b.move_to(&t, Some(o), None, None);
                    }
                }
                (8, Some(t)) => {
                    b.delete(&t)?;
                }
                (_, Some(t)) => b.add_tags(&t, &[format!("x{}", step % 4)], &[])?,
            }
            Ok((b.finish(), ()))
        })
        .unwrap();
}

#[test]
fn devices_converge_after_random_concurrent_edits() {
    for seed in 0..5u64 {
        let root = tmpdir(&format!("s{seed}"));
        let n = 3;
        let mut devs: Vec<Dev> = (0..n).map(|i| open(&root, i)).collect();
        let mut rng = StdRng::seed_from_u64(seed);
        for step in 0..120 {
            let i = rng.random_range(0..n);
            random_op(&mut devs[i], &mut rng, step);
            if rng.random_range(0..6) == 0 {
                sync(&root, n);
            }
        }
        sync(&root, n);
        for d in devs.iter_mut() {
            d.vault.catch_up().unwrap();
        }
        let first = dump(&devs[0].vault);
        assert!(first.lines().count() > 10, "test produced too little data");
        assert!(first.contains("review "), "no review verdicts were exercised (seed {seed})");
        assert!(!first.contains("accepted agent:"), "an agent's accept was applied (seed {seed})");
        for (i, d) in devs.iter().enumerate().skip(1) {
            assert_eq!(dump(&d.vault), first, "device {i} diverged (seed {seed})");
        }
        // A from-scratch rebuild must agree with incremental application.
        let before = dump(&devs[1].vault);
        devs[1].vault.rebuild().unwrap();
        assert_eq!(dump(&devs[1].vault), before, "rebuild differs from incremental (seed {seed})");
        let _ = fs::remove_dir_all(&root);
    }
}

/// The Mac editor makes node ids on the client (mac-editor-arch.md §2). Two devices typing into the
/// same page offline, each saving block ops with their own ids, converge; and even the same id
/// created on both (a clash the alphabet makes practically impossible) leaves one node everywhere.
#[test]
fn client_made_ids_from_editors_converge() {
    use thc_core::outline::{self, BlockOp, Kind};
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
    for seed in 0..5u64 {
        let root = tmpdir(&format!("ids{seed}"));
        let n = 2;
        let mut devs: Vec<Dev> = (0..n).map(|i| open(&root, i)).collect();
        let (_, page) = devs[0]
            .vault
            .transact(|s| {
                let mut b = TxBuilder::new(s, today);
                let p = b.create_page("Shared", &[])?;
                Ok((b.finish(), p))
            })
            .unwrap();
        sync(&root, n);
        devs[1].vault.catch_up().unwrap();
        let clash = thc_core::id::new_id();
        let mut rng = StdRng::seed_from_u64(seed);
        for step in 0..40 {
            let i = rng.random_range(0..n);
            let blocks = outline::render(&devs[i].vault.store, &page).unwrap();
            let pick = |rng: &mut StdRng| (!blocks.is_empty()).then(|| blocks[rng.random_range(0..blocks.len())].clone());
            let op = match (step, rng.random_range(0..4)) {
                (7, _) | (8, _) => BlockOp::Create { id: clash.clone(), parent: None, after: None, kind: Kind::Bullet, text: format!("clash from dev{i}") },
                (_, 0) | (_, 1) => BlockOp::Create {
                    id: thc_core::id::new_id(),
                    parent: None,
                    after: pick(&mut rng).filter(|b| b.depth == 0).map(|b| b.id),
                    kind: if rng.random_range(0..2) == 0 { Kind::Para } else { Kind::Task },
                    text: format!("line {step} from dev{i}"),
                },
                (_, 2) => match pick(&mut rng) {
                    Some(b) => BlockOp::Edit { id: b.id, text: format!("edited {step} by dev{i}"), base: b.text_rev, rev: None, raw: false },
                    None => continue,
                },
                _ => match pick(&mut rng) {
                    Some(b) => BlockOp::Delete { id: b.id, rev: b.rev },
                    None => continue,
                },
            };
            let p = page.clone();
            devs[i].vault.transact(move |s| outline::plan(s, &p, &[op], today)).unwrap();
            if rng.random_range(0..5) == 0 {
                sync(&root, n);
                for d in devs.iter_mut() {
                    d.vault.catch_up().unwrap();
                }
            }
        }
        sync(&root, n);
        for d in devs.iter_mut() {
            d.vault.catch_up().unwrap();
        }
        let first = dump(&devs[0].vault);
        assert_eq!(dump(&devs[1].vault), first, "editors diverged (seed {seed})");
        let clashes = devs[0].vault.store.conn.query_row("SELECT count(*) FROM nodes WHERE id=?1", [&clash], |r| r.get::<_, i64>(0)).unwrap();
        assert!(clashes <= 1, "one node per id (seed {seed})");
        let before = dump(&devs[1].vault);
        devs[1].vault.rebuild().unwrap();
        assert_eq!(dump(&devs[1].vault), before, "rebuild differs from incremental (seed {seed})");
        let _ = fs::remove_dir_all(&root);
    }
}

/// Two devices appending offline give siblings the same order key; moving a node between those
/// twins must not panic, and every device must agree on the order (key, then id).
#[test]
fn tied_sibling_keys_converge_and_never_panic() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
    let root = tmpdir("ties");
    let n = 2;
    let mut devs: Vec<Dev> = (0..n).map(|i| open(&root, i)).collect();
    let (_, page) = devs[0]
        .vault
        .transact(|s| {
            let mut b = TxBuilder::new(s, today);
            let p = b.create_page("Ties", &[])?;
            Ok((b.finish(), p))
        })
        .unwrap();
    sync(&root, n);
    devs[1].vault.catch_up().unwrap();
    // Offline, each device appends one child: both pick the key after the (empty) last sibling.
    let mut kids = vec![];
    for (i, d) in devs.iter_mut().enumerate() {
        let p = page.clone();
        let (_, id) = d
            .vault
            .transact(move |s| {
                let mut b = TxBuilder::new(s, today);
                let cap = capture::parse(&format!("twin {i}"), today)?;
                let id = b.create_from_capture(Some(p), &cap, None)?;
                Ok((b.finish(), id))
            })
            .unwrap();
        kids.push(id);
    }
    sync(&root, n);
    for d in devs.iter_mut() {
        d.vault.catch_up().unwrap();
    }
    let ords: Vec<String> = devs[0].vault.store.children(&page).unwrap().iter().map(|c| c.ord.clone()).collect();
    assert_eq!(ords[0], ords[1], "the twins share a key");
    // A third node moved between the twins: no panic, and both devices agree afterwards.
    let first = devs[0].vault.store.children(&page).unwrap()[0].id.clone();
    let p = page.clone();
    devs[0]
        .vault
        .transact(move |s| {
            let mut b = TxBuilder::new(s, today);
            let cap = capture::parse("between", today)?;
            let id = b.create_from_capture(Some(p.clone()), &cap, None)?;
            Ok((b.finish(), id))
        })
        .map(|(_, id)| {
            let p = page.clone();
            devs[0].vault.transact(move |s| {
                let mut b = TxBuilder::new(s, today);
                b.move_to(&id, Some(p), Some(&first), None)?;
                Ok((b.finish(), ()))
            })
        })
        .unwrap()
        .unwrap();
    sync(&root, n);
    for d in devs.iter_mut() {
        d.vault.catch_up().unwrap();
    }
    assert_eq!(dump(&devs[0].vault), dump(&devs[1].vault), "devices agree on tied order");
    let _ = fs::remove_dir_all(&root);
}

/// Two devices offline both write today's journal, tag #lisbon and link [[Offsite]]: after
/// sync there's one day, one tag and one page (data-model-review.md §1), with both lines.
#[test]
fn offline_days_tags_and_pages_are_one_node_after_sync() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
    let root = tmpdir("keyed");
    let n = 2;
    let mut devs: Vec<Dev> = (0..n).map(|i| open(&root, i)).collect();
    for (i, d) in devs.iter_mut().enumerate() {
        d.vault
            .transact(|s| {
                let mut b = TxBuilder::new(s, today);
                let day = b.journal(today)?;
                let cap = capture::parse(&format!("from dev{i} #lisbon [[Offsite]]"), today)?;
                b.create_from_capture(Some(day), &cap, None)?;
                Ok((b.finish(), ()))
            })
            .unwrap();
    }
    sync(&root, n);
    for d in devs.iter_mut() {
        d.vault.catch_up().unwrap();
    }
    let count = |sql: &str| devs[0].vault.store.conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(count("SELECT count(*) FROM nodes WHERE journal='2026-10-04' AND deleted=0"), 1, "one day");
    assert_eq!(count("SELECT count(*) FROM nodes WHERE is_tag=1 AND lower(title)='lisbon' AND deleted=0"), 1, "one tag");
    assert_eq!(count("SELECT count(*) FROM nodes WHERE lower(title)='offsite' AND is_tag=0 AND deleted=0"), 1, "one page");
    let day = devs[0].vault.store.journal_node("2026-10-04").unwrap().unwrap();
    assert_eq!(devs[0].vault.store.children(&day).unwrap().len(), 2, "both devices' lines are under the one day");
    assert_eq!(dump(&devs[1].vault), dump(&devs[0].vault));
    let _ = fs::remove_dir_all(&root);
}

/// A page keeps its id through a rename; making a page with the old title then gets a new id
/// (the keyed one is taken), never the renamed page.
#[test]
fn a_renamed_page_keeps_its_id() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
    let root = tmpdir("rename");
    let mut d = open(&root, 0);
    let (_, p) = d.vault.transact(|s| { let mut b = TxBuilder::new(s, today); let p = b.create_page("Q4 Planning", &[])?; Ok((b.finish(), p)) }).unwrap();
    assert_eq!(p, thc_core::id::from_key("page:q4 planning"), "a page made by title has the keyed id");
    let p2 = p.clone();
    d.vault.transact(move |s| { let mut b = TxBuilder::new(s, today); b.set_props(&p2, &[("title".into(), "Q1 Planning".into())])?; Ok((b.finish(), ())) }).unwrap();
    let (_, again) = d.vault.transact(|s| { let mut b = TxBuilder::new(s, today); let p = b.create_page("Q4 Planning", &[])?; Ok((b.finish(), p)) }).unwrap();
    assert_ne!(again, p, "the old title makes a new page");
    let renamed = d.vault.store.node(&p).unwrap().unwrap();
    assert_eq!(renamed.title.as_deref(), Some("Q1 Planning"), "the renamed page is untouched");
    let _ = fs::remove_dir_all(&root);
}

/// Two vaults side by side (vaults.md): each store holds only its own vault's events, and a
/// federated query reads each in its own store, merged in the query's order.
#[test]
fn two_vaults_never_cross_and_merge_in_order() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
    let root = tmpdir("twovaults");
    let mut a = open(&root, 0);
    let mut b = open(&root, 1);
    for (d, text) in [(&mut a, "alpha due:2026-10-06"), (&mut b, "beta due:2026-10-05")] {
        d.vault
            .transact(|s| {
                let mut bl = TxBuilder::new(s, today);
                let cap = capture::parse(&format!("[ ] {text}"), today)?;
                bl.create_from_capture(None, &cap, None)?;
                Ok((bl.finish(), ()))
            })
            .unwrap();
    }
    let texts = |v: &Vault| -> Vec<String> { v.store.query("status:open", today, 100).unwrap().into_iter().map(|n| n.text).collect() };
    assert_eq!(texts(&a.vault), ["alpha"]);
    assert_eq!(texts(&b.vault), ["beta"]);
    let entries: Vec<thc_core::registry::Entry> = [("a", &a), ("b", &b)].iter().map(|(n, d)| thc_core::registry::Entry { name: n.to_string(), path: d.vault.paths.vault.clone(), id: None }).collect();
    let pa = a.vault.paths.clone();
    let pb = b.vault.paths.clone();
    let paths_for = move |e: &thc_core::registry::Entry| if e.name == "a" { pa.clone() } else { pb.clone() };
    let actor = Actor { kind: "human".into(), name: None };
    let parts = thc_core::federated::run(&entries, &paths_for, &actor, "status:open sort:due", today, 100).unwrap();
    let order = thc_core::query::compile("status:open sort:due", &parts[0].vault.store, today).unwrap().order_sql;
    let merged: Vec<String> = thc_core::federated::merge(&parts, &order, 100).unwrap().into_iter().map(|(p, r)| parts[p].nodes[r].text.clone()).collect();
    assert_eq!(merged, ["beta", "alpha"], "merged by due across vaults");
    let _ = fs::remove_dir_all(&root);
}
