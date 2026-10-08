//! TodayPanel.badge (mac-app.md §1): overdue + today (due or scheduled, or an alert firing
//! today: views.md §3.3) + fired unacknowledged alerts, each node once.

use thc_core::builder::TxBuilder;
use thc_core::capture;
use thc_core::dates;
use thc_core::event::{Actor, Trigger};
use thc_core::vault::{self, Paths, Vault};

#[test]
fn badge_counts_each_node_once() {
    let root = thc_core::scratch::dir(&format!("thc-badge-{}", thc_core::id::new_id()));
    vault::init(&root.join("v"), None, None).unwrap();
    let paths = Paths { vault: root.join("v"), cache: root.join("cache") };
    let mut v = Vault::open(paths, Actor { kind: "human".into(), name: None }, "test").unwrap();
    let today = dates::today();
    let add = |v: &mut Vault, text: &str, alert: bool| -> String {
        let (_, id) = v
            .transact(|s| {
                let mut b = TxBuilder::new(s, today);
                let cap = capture::parse(text, today)?;
                let j = b.journal(today)?;
                let id = b.create_from_capture(Some(j), &cap, None)?;
                if alert {
                    b.add_alert(&id, Trigger { at: Some("2020-01-01T09:00".into()), offset: None, anchor: None })?;
                }
                Ok((b.finish(), id))
            })
            .unwrap();
        id
    };
    add(&mut v, "[ ] scheduled today sched:today", false);
    add(&mut v, "[ ] due today due:today", false);
    add(&mut v, "[ ] overdue with a fired alert due:-2d", true);
    add(&mut v, "[ ] no date, fired alert", true);
    add(&mut v, "[ ] later due:+3d", false);
    add(&mut v, "a note today", false);
    let now = dates::now_local();
    for a in v.store.alerts_where("deleted=0", &[]).unwrap() {
        v.store.mark_fired(&a, "test", now).unwrap();
    }
    // Due tomorrow, with an alert later today (`--before 1d`): it joins today[], not upcoming[].
    let tomorrow = add(&mut v, "[ ] due tomorrow, alert today due:tomorrow", false);
    let at = format!("{}T23:59", today.format("%Y-%m-%d"));
    v.transact(|s| {
        let mut b = TxBuilder::new(s, today);
        b.add_alert(&tomorrow, Trigger { at: Some(at.clone()), offset: None, anchor: None })?;
        Ok((b.finish(), ()))
    })
    .unwrap();
    let p = thc_daemon::today_panel(&v.store).unwrap();
    // scheduled today, due today, overdue (its fired alert doesn't count twice), the undated
    // fired alert, the alert-today row. The note has no date, so it isn't in today[].
    assert_eq!(p["badge"], 5, "{p:#}");
    let ids = |k: &str| p[k].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert!(ids("today").contains(&tomorrow), "{p:#}");
    assert!(!ids("upcoming").contains(&tomorrow), "{p:#}");
    let _ = std::fs::remove_dir_all(root);
}
