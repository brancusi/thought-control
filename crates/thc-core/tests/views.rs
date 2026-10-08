//! Saved views (views.md §1): seeded once, hidden from listings, and two devices seeding at the
//! same time end up with one set (deterministic ids make the creates idempotent).

use std::fs;
use std::path::Path;
use thc_core::builder::TxBuilder;
use thc_core::dates;
use thc_core::event::Actor;
use thc_core::vault::{self, Paths, Vault};
use thc_core::views;

fn open(root: &Path, i: usize) -> Vault {
    let v = root.join(format!("v{i}"));
    vault::init(&v, None, None).unwrap();
    Vault::open(Paths { vault: v, cache: root.join(format!("c{i}")) }, Actor { kind: "human".into(), name: None }, "test").unwrap()
}

fn write(v: &mut Vault, f: impl FnOnce(&mut TxBuilder) -> anyhow::Result<()>) {
    v.transact(|s| {
        let mut b = TxBuilder::new(s, dates::today());
        f(&mut b)?;
        Ok((b.finish(), ()))
    })
    .unwrap();
}

/// Copy every device's log files into every vault (per-device files, so this is a union).
fn sync(root: &Path, n: usize) {
    for src in 0..n {
        for dev in fs::read_dir(root.join(format!("v{src}/log"))).unwrap() {
            let dev = dev.unwrap();
            for f in fs::read_dir(dev.path()).unwrap() {
                let f = f.unwrap();
                for dst in (0..n).filter(|d| *d != src) {
                    let t = root.join(format!("v{dst}/log")).join(dev.file_name()).join(f.file_name());
                    fs::create_dir_all(t.parent().unwrap()).unwrap();
                    fs::copy(f.path(), &t).unwrap();
                }
            }
        }
    }
}

#[test]
fn seeding_is_once_and_converges_across_devices() {
    let root = thc_core::scratch::dir(&format!("thc-views-{}", thc_core::id::new_id()));
    let (mut a, mut b) = (open(&root, 0), open(&root, 1));
    // Both devices seed before they've synced.
    write(&mut a, |t| views::ensure_seeded(t).map(|_| ()));
    write(&mut b, |t| views::ensure_seeded(t).map(|_| ()));
    write(&mut a, |t| views::add(t, "work", "status:open #work", None, Some(6), true, None).map(|_| ()));
    sync(&root, 2);
    a.catch_up().unwrap();
    b.catch_up().unwrap();
    let names = |v: &Vault| views::list(&v.store).unwrap().into_iter().map(|v| v.name).collect::<Vec<_>>();
    assert_eq!(names(&a), vec!["open", "week", "waiting", "claude", "ready", "work"]);
    assert_eq!(names(&a), names(&b), "devices agree");

    // Seeding again does nothing; a removed built-in stays removed.
    write(&mut a, |t| views::remove(t, "week").map(|_| ()));
    write(&mut a, |t| {
        assert!(!views::ensure_seeded(t)?);
        Ok(())
    });
    assert!(!names(&a).contains(&"week".to_string()));

    // Hidden: the page and the views are in no listing, search or query.
    let today = dates::today();
    assert!(a.store.query("text:work", today, 100).unwrap().iter().all(|n| n.id != views::find(&a.store, "work").unwrap().unwrap().id));
    assert!(a.store.search("Views", 100).unwrap().iter().all(|n| n.id != views::page_id()));
    assert!(a.store.search("work", 100).unwrap().iter().all(|n| !n.text.starts_with('@')));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn views_expand_inside_queries() {
    let root = thc_core::scratch::dir(&format!("thc-views-q-{}", thc_core::id::new_id()));
    let mut a = open(&root, 0);
    write(&mut a, |t| {
        let j = t.journal(dates::today())?;
        for text in ["[ ] alpha #work due:+1d", "[ ] beta #work due:+20d", "[ ] gamma #home"] {
            let cap = thc_core::capture::parse(text, dates::today())?;
            t.create_from_capture(Some(j.clone()), &cap, None)?;
        }
        views::add(t, "work", "status:open #work", None, None, false, None)?;
        views::add(t, "soon", "@work due<=+7d", None, None, false, None).map(|_| ())
    });
    let today = dates::today();
    let texts = |q: &str| a.store.query(q, today, 100).unwrap().into_iter().map(|n| n.text).collect::<Vec<_>>();
    assert_eq!(texts("@work").len(), 2);
    assert_eq!(texts("@work due<=+7d"), vec!["alpha #work"], "composes with terms");
    assert_eq!(texts("@soon"), vec!["alpha #work"], "a view can use a view");
    assert_eq!(texts("@work or #home").len(), 3, "or works");
    assert_eq!(texts("-@work status:open"), vec!["gamma #home"], "negation works");
    let err = a.store.query("@wrk", today, 10).unwrap_err().to_string();
    assert!(err.contains("no view @wrk · did you mean @work?"), "{err}");
    assert_eq!(texts("text:me@work.com"), Vec::<String>::new(), "@ inside a token isn't a view");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cycles_are_rejected_and_builtins_have_titles() {
    let root = thc_core::scratch::dir(&format!("thc-views-c-{}", thc_core::id::new_id()));
    let mut a = open(&root, 0);
    write(&mut a, |t| {
        views::add(t, "a", "status:open", None, None, false, None)?;
        views::add(t, "b", "@a #x", None, None, false, None).map(|_| ())
    });
    let r = a.transact(|s| {
        let mut t = TxBuilder::new(s, dates::today());
        views::set(&mut t, "a", views::Update { query: Some("@b"), ..Default::default() })?;
        Ok((t.finish(), ()))
    });
    let err = r.unwrap_err().to_string();
    assert!(err.contains("@a would refer to itself through @b"), "{err}");
    let titles: Vec<Option<String>> = views::list(&a.store).unwrap().into_iter().filter(|v| ["week", "claude"].contains(&v.name.as_str())).map(|v| v.title).collect();
    assert_eq!(titles, vec![Some("this week".into()), Some("by claude".into())]);
    let _ = fs::remove_dir_all(root);
}
