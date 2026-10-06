//! Alert firing (docs/design/daemon.md §2). Pure planning plus per-device local state.
//!
//! - **Replicated state** (`pending` / `snoozed` / `acked`, `fire_at`) comes from the log via
//!   `alert.*` ops, so an ack or snooze on one device stops the others once it syncs.
//! - **Fired state is local to each device** (`alert_local`, never in the log, never touched
//!   by `Store::apply` or `reset`). Firing is a side effect of a device's clock; an alert fires
//!   at most once per device per `fire_at` value, and re-arms when `fire_at` changes.

use crate::dates::{self, DateVal};
use crate::model::{Alert, Node};
use crate::store::Store;
use anyhow::Result;
use chrono::{Duration, NaiveDateTime};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

/// Alerts more than this far past `fire_at` are marked fired without a notification.
pub const STALE_AFTER: Duration = Duration::hours(12);
/// Alerts that came due more than this long ago are "missed" (catch-up summary).
pub const MISSED_AFTER: Duration = Duration::minutes(2);
/// Alerts within one window are one delivery.
pub const GROUP_WINDOW_SECS: i64 = 60;

pub const LOCAL_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS alert_local(alert TEXT PRIMARY KEY, fired_for TEXT NOT NULL, \
    fired_at TEXT NOT NULL, via TEXT NOT NULL) STRICT;";

fn stamp(dt: NaiveDateTime) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S").to_string()
}

fn parse_stamp(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").ok())
}

impl Store {
    /// Nodes in Today only because an alert fires today (views.md §3.3): pending or snoozed
    /// alerts whose `fire_at` falls on `today` and haven't fired here yet, on open tasks or
    /// non-tasks. Skips ids in `shown` (already a Today/Overdue row). Ordered by alert time,
    /// each node once, with its `HH:MM`. Fired alerts surface under Overdue instead.
    pub fn alert_rows(&self, today: chrono::NaiveDate, shown: &std::collections::HashSet<String>) -> Result<Vec<(crate::model::Node, String)>> {
        let t = today.format("%Y-%m-%d").to_string();
        let mut out: Vec<(crate::model::Node, String)> = Vec::new();
        for a in self.alerts_where("deleted=0 AND state IN ('pending','snoozed') AND substr(fire_at,1,10) = ?1 ORDER BY fire_at", &[&t])? {
            if shown.contains(&a.node) || out.iter().any(|(n, _)| n.id == a.node) || self.fired_at(&a).is_some() {
                continue;
            }
            let Some(n) = self.node(&a.node)? else { continue };
            if n.deleted || n.is_tag || matches!(n.status.as_deref(), Some("done" | "cancelled")) {
                continue;
            }
            let time = a.fire_at.as_deref().and_then(|f| f.get(11..16)).unwrap_or("").to_string();
            out.push((n, time));
        }
        Ok(out)
    }

    /// When this device fired the alert for its current `fire_at`, if it has.
    pub fn fired_at(&self, alert: &Alert) -> Option<String> {
        let row: Option<(String, String)> = self
            .conn
            .query_row("SELECT fired_for, fired_at FROM alert_local WHERE alert=?1", [&alert.id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .ok()
            .flatten();
        match row {
            Some((fired_for, at)) if Some(fired_for.as_str()) == alert.fire_at.as_deref() => Some(at),
            _ => None,
        }
    }

    /// Effective state for display: `acked` / `snoozed` (replicated) win, then `fired` (local).
    pub fn alert_state(&self, alert: &Alert) -> String {
        match alert.state.as_str() {
            "acked" => "acked".into(),
            s if self.fired_at(alert).is_some() => {
                let _ = s;
                "fired".into()
            }
            s => s.to_string(),
        }
    }

    pub fn mark_fired(&self, alert: &Alert, via: &str, now: NaiveDateTime) -> Result<()> {
        let Some(fire_at) = &alert.fire_at else { return Ok(()) };
        self.conn.execute(
            "INSERT INTO alert_local(alert,fired_for,fired_at,via) VALUES(?1,?2,?3,?4) \
             ON CONFLICT(alert) DO UPDATE SET fired_for=excluded.fired_for, fired_at=excluded.fired_at, via=excluded.via",
            params![alert.id, fire_at, stamp(now), via],
        )?;
        Ok(())
    }

    /// Pending or snoozed alerts due at or before `now` that this device hasn't fired yet,
    /// on nodes that are still live and open (or not tasks).
    /// Pending alerts due by `now`, including ones this device already fired (for previews).
    pub fn due_alerts_all(&self, now: NaiveDateTime) -> Result<Vec<(Alert, Node)>> {
        let cutoff = now.format("%Y-%m-%dT%H:%M").to_string();
        let alerts = self.alerts_where("deleted=0 AND state IN ('pending','snoozed') AND fire_at IS NOT NULL AND fire_at <= ?1 ORDER BY fire_at", &[&cutoff])?;
        let mut out = Vec::new();
        for a in alerts {
            if let Some(n) = self.node(&a.node)?.filter(|n| !n.deleted) {
                out.push((a, n));
            }
        }
        Ok(out)
    }

    pub fn due_alerts(&self, now: NaiveDateTime) -> Result<Vec<(Alert, Node)>> {
        let cutoff = now.format("%Y-%m-%dT%H:%M").to_string();
        let alerts = self.alerts_where(
            "deleted=0 AND state IN ('pending','snoozed') AND fire_at IS NOT NULL AND fire_at <= ?1 ORDER BY fire_at",
            &[&cutoff],
        )?;
        let mut out = Vec::new();
        for a in alerts {
            if self.fired_at(&a).is_some() {
                continue;
            }
            let Some(n) = self.node(&a.node)? else { continue };
            if n.deleted || matches!(n.status.as_deref(), Some("done" | "cancelled")) {
                continue;
            }
            out.push((a, n));
        }
        Ok(out)
    }

    /// The next `fire_at` this device will act on (for the daemon's timer).
    pub fn next_fire_at(&self) -> Result<Option<NaiveDateTime>> {
        let alerts = self.alerts_where("deleted=0 AND state IN ('pending','snoozed') AND fire_at IS NOT NULL ORDER BY fire_at", &[])?;
        for a in alerts {
            if self.fired_at(&a).is_none() {
                if let Some(t) = a.fire_at.as_deref().and_then(parse_stamp) {
                    return Ok(Some(t));
                }
            }
        }
        Ok(None)
    }

    pub fn pending_alerts_within(&self, now: NaiveDateTime, within: Duration) -> Result<usize> {
        let hi = (now + within).format("%Y-%m-%dT%H:%M").to_string();
        let alerts = self.alerts_where("deleted=0 AND state IN ('pending','snoozed') AND fire_at IS NOT NULL AND fire_at <= ?1", &[&hi])?;
        Ok(alerts.iter().filter(|a| self.fired_at(a).is_none()).count())
    }
}

/// What a notification says (daemon.md §2.2). Shared by the daemon, Linux and ThoughtBar.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Notification {
    pub alert: String,
    pub node: String,
    pub title: String,
    pub line1: String,
    pub line2: String,
    /// Same as line2 but without glyphs, for plain-text channels.
    pub line2_plain: String,
    /// `thc.alerts.YYYY-MM-DD`
    pub thread: String,
    pub time_sensitive: bool,
    pub is_task: bool,
    pub fire_at: String,
}

/// Node text without markup: `[[Health]]` -> `Health`, tags dropped, cut at 60 chars.
pub fn plain_title(store: &Store, n: &Node) -> String {
    let text = if n.title.is_some() { n.label() } else { store.render_text(&n.text) };
    let first = text.lines().next().unwrap_or("").replace("[[", "").replace("]]", "");
    let words: Vec<&str> = first.split_whitespace().filter(|w| !(w.starts_with('#') && w.len() > 1)).collect();
    let t = words.join(" ");
    if t.chars().count() > 60 { format!("{}…", t.chars().take(59).collect::<String>()) } else { t }
}

fn when_words(dv: DateVal, today: chrono::NaiveDate) -> String {
    let d = dv.date();
    let days = (d - today).num_days();
    let day = match days {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        n if (2..7).contains(&n) => d.format("%a").to_string(),
        _ => d.format("%a %b %-d").to_string(),
    };
    match dv.time() {
        Some(t) if days == 0 => t.format("%H:%M").to_string(),
        Some(t) => format!("{day} {}", t.format("%H:%M")),
        None => day,
    }
}

fn human_offset(offset: &str) -> String {
    let Ok(d) = dates::parse_duration(offset) else { return offset.to_string() };
    let mins = d.num_minutes().abs();
    match mins {
        0 => "at the time".into(),
        m if m % 1440 == 0 => format!("{} day{} before", m / 1440, if m == 1440 { "" } else { "s" }),
        m if m % 60 == 0 => format!("{} hour{} before", m / 60, if m == 60 { "" } else { "s" }),
        m => format!("{m} min before"),
    }
}

pub fn build(store: &Store, alert: &Alert, n: &Node, today: chrono::NaiveDate) -> Notification {
    let title = plain_title(store, n);
    let repeat = n.repeat.as_ref().and_then(|r| r.get("text")).and_then(|t| t.as_str()).map(|t| format!(" · ↻ {t}")).unwrap_or_default();
    let fire = alert.fire_at.clone().unwrap_or_default();
    let line1 = if alert.state == "snoozed" {
        let orig = alert.at.clone().or_else(|| {
            let anchor = if alert.anchor.as_deref() == Some("scheduled") { n.scheduled.clone() } else { n.due.clone() };
            anchor
        });
        format!("snoozed from {}", orig.as_deref().and_then(DateVal::from_stored).map(|d| when_words(d, today)).unwrap_or_default())
    } else if let Some(at) = alert.at.as_deref().and_then(DateVal::from_stored) {
        format!("{}{repeat}", when_words(at, today))
    } else {
        let offset = alert.offset.clone().unwrap_or_else(|| "0m".into());
        let zero = dates::parse_duration(&offset).map(|d| d.num_minutes() == 0).unwrap_or(true);
        let anchor_date = if alert.anchor.as_deref() == Some("scheduled") { &n.scheduled } else { &n.due };
        let timed = anchor_date.as_deref().and_then(DateVal::from_stored).filter(|d| d.time().is_some());
        if let (true, Some(at)) = (zero, timed) {
            // It arrives at that moment, so the time is the whole story (daemon.md §2.2).
            format!("{}{repeat}", when_words(at, today))
        } else if alert.anchor.as_deref() == Some("scheduled") {
            let start = n.scheduled.as_deref().and_then(DateVal::from_stored);
            let when = start.map(|d| match d.time() {
                Some(t) if d.date() == today => format!("starts at {}", t.format("%H:%M")),
                _ => format!("starts {}", when_words(d, today)),
            });
            match (when, zero) {
                (Some(w), true) => format!("{w}{repeat}"),
                (Some(w), false) => format!("{w} · reminder {}", human_offset(&offset)),
                (None, _) => "reminder".into(),
            }
        } else {
            let due = n.due.as_deref().and_then(DateVal::from_stored).map(|d| format!("due {}", when_words(d, today)));
            match (due, zero) {
                (Some(d), true) => format!("{d}{repeat}"),
                (Some(d), false) => format!("{d} · reminder {}", human_offset(&offset)),
                (None, _) => "reminder".into(),
            }
        }
    };
    // Where it lives, and who set it if not a human.
    let mut place = String::new();
    let mut root = n.parent.as_deref().and_then(|p| store.node(p).ok().flatten());
    while let Some(r) = root.clone() {
        match r.parent.as_deref().and_then(|p| store.node(p).ok().flatten()) {
            Some(p) => root = Some(p),
            None => break,
        }
    }
    if let Some(r) = root {
        place = match r.journal.as_deref().and_then(DateVal::from_stored) {
            Some(d) if d.date() == today => "in today's journal".into(),
            Some(d) if Some(d.date()) == today.pred_opt() => "in yesterday's journal".into(),
            Some(d) => format!("in § {}", d.date().format("%b %-d")),
            None if r.title.is_some() => format!("in ¶ {}", r.label()),
            None => String::new(),
        };
    } else if n.parent.is_none() && n.title.is_none() {
        place = "in inbox".into();
    }
    let agent = n.created_by.strip_prefix("agent:").map(str::to_string);
    let (line2, line2_plain) = match agent {
        Some(a) if place.is_empty() => (format!("set by ◆ {a}"), format!("set by {a}")),
        Some(a) => (format!("{place} · set by ◆ {a}"), format!("{} · set by {a}", place.replace("¶ ", "").replace("§ ", ""))),
        None => (place.clone(), place.replace("¶ ", "").replace("§ ", "")),
    };
    let thread_day = fire.get(..10).unwrap_or("").to_string();
    Notification {
        alert: alert.id.clone(),
        node: n.id.clone(),
        title,
        line1,
        line2,
        line2_plain,
        thread: format!("thc.alerts.{thread_day}"),
        // An explicit time: an absolute `at`, or an offset from a date that has a time
        // (`thc remind --at 9am` anchors to a timed `scheduled`). Date-only reminders aren't.
        time_sensitive: alert.at.is_some() || {
            let anchor = if alert.anchor.as_deref() == Some("scheduled") { &n.scheduled } else { &n.due };
            anchor.as_deref().is_some_and(|d| d.contains('T'))
        },
        is_task: n.status.is_some(),
        fire_at: fire,
    }
}

/// One delivery decision.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Delivery {
    /// A normal notification per alert (one or two in a window).
    Single { notification: Notification },
    /// Three or more in a window, or missed while away.
    Summary { title: String, body: String, alerts: Vec<String>, missed: bool, thread: String },
    /// More than 12 h late: mark fired, no notification.
    Silent { alerts: Vec<String> },
}

/// Group due alerts into deliveries (daemon.md §2.4).
/// A day's reminders stack together: `thc.alerts.2026-10-04` (summaries and singles alike).
fn thread_for(a: &Alert) -> String {
    format!("thc.alerts.{}", a.fire_at.as_deref().and_then(|f| f.get(..10)).unwrap_or(""))
}

pub fn plan(store: &Store, due: &[(Alert, Node)], now: NaiveDateTime) -> Vec<Delivery> {
    let today = now.date();
    let mut out = Vec::new();
    let mut silent = Vec::new();
    let mut missed: Vec<(Alert, Node)> = Vec::new();
    let mut fresh: Vec<(NaiveDateTime, Alert, Node)> = Vec::new();
    for (a, n) in due {
        let Some(at) = a.fire_at.as_deref().and_then(parse_stamp) else { continue };
        let late = now - at;
        if late > STALE_AFTER {
            silent.push(a.id.clone());
        } else if late > MISSED_AFTER {
            missed.push((a.clone(), n.clone()));
        } else {
            fresh.push((at, a.clone(), n.clone()));
        }
    }
    if !silent.is_empty() {
        out.push(Delivery::Silent { alerts: silent });
    }
    match missed.len() {
        0 => {}
        1 => out.push(Delivery::Single { notification: build(store, &missed[0].0, &missed[0].1, today) }),
        k => {
            let body = missed
                .iter()
                .map(|(a, n)| {
                    let t = a.fire_at.as_deref().and_then(parse_stamp).map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
                    format!("{} {t}", plain_title(store, n))
                })
                .collect::<Vec<_>>()
                .join(" · ");
            out.push(Delivery::Summary {
                title: format!("Missed while away: {k} reminders"),
                body,
                alerts: missed.iter().map(|(a, _)| a.id.clone()).collect(),
                missed: true,
                thread: thread_for(&missed[0].0),
            });
        }
    }
    // Windows of GROUP_WINDOW_SECS starting at the earliest alert in each.
    fresh.sort_by_key(|(t, a, n)| (*t, n.created_ms, a.id.clone()));
    let mut i = 0;
    while i < fresh.len() {
        let start = fresh[i].0;
        let mut j = i;
        while j < fresh.len() && (fresh[j].0 - start).num_seconds() < GROUP_WINDOW_SECS {
            j += 1;
        }
        let group = &fresh[i..j];
        if group.len() >= 3 {
            out.push(Delivery::Summary {
                title: format!("{} reminders", group.len()),
                body: group.iter().map(|(_, _, n)| plain_title(store, n)).collect::<Vec<_>>().join(" · "),
                alerts: group.iter().map(|(_, a, _)| a.id.clone()).collect(),
                missed: false,
                thread: thread_for(&group[0].1),
            });
        } else {
            for (_, a, n) in group {
                out.push(Delivery::Single { notification: build(store, a, n, today) });
            }
        }
        i = j;
    }
    out
}

pub fn delivery_alerts(d: &Delivery) -> Vec<String> {
    match d {
        Delivery::Single { notification } => vec![notification.alert.clone()],
        Delivery::Summary { alerts, .. } | Delivery::Silent { alerts } => alerts.clone(),
    }
}

pub fn now() -> NaiveDateTime {
    dates::now_local()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::TxBuilder;
    use crate::event::{Actor, Event, Op, Trigger};
    use crate::hlc::Hlc;
    use chrono::NaiveDate;

    fn store_with(ops: Vec<Op>) -> Store {
        let s = Store::open_memory().unwrap();
        let mut hlc = Hlc::default();
        for (i, op) in ops.into_iter().enumerate() {
            hlc = Hlc(hlc.0 + 1, 0);
            let e = Event {
                v: 1,
                eid: format!("e{i:04}"),
                hlc,
                dev: "d".into(),
                actor: Actor { kind: "human".into(), name: None },
                via: "test".into(),
                tx: format!("t{i}"),
                op,
            };
            s.apply(&e).unwrap();
        }
        s
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 3).unwrap()
    }

    fn at(h: u32, m: u32) -> NaiveDateTime {
        today().and_hms_opt(h, m, 0).unwrap()
    }

    fn seeded() -> Store {
        let s = Store::open_memory().unwrap();
        let ops = {
            let mut b = TxBuilder::new(&s, today());
            let j = b.journal(today()).unwrap();
            for (i, (text, when)) in [("Pay rent", "2026-10-03T09:00"), ("Call the dentist", "2026-10-03T09:00"), ("Water plants", "2026-10-03T09:00")].iter().enumerate() {
                let cap = crate::capture::parse(&format!("[ ] {text}"), today()).unwrap();
                let id = b.create_from_capture(Some(j.clone()), &cap, Some(format!("n{i}aaaaaaaaaa"))).unwrap();
                b.add_alert(&id, Trigger { at: Some(when.to_string()), offset: None, anchor: None }).unwrap();
            }
            b.finish()
        };
        let st = store_with(ops);
        drop(s);
        st
    }

    #[test]
    fn three_in_a_window_is_one_summary() {
        let s = seeded();
        let due = s.due_alerts(at(9, 0)).unwrap();
        assert_eq!(due.len(), 3);
        let plan = plan(&s, &due, at(9, 0));
        assert_eq!(plan.len(), 1, "{plan:?}");
        match &plan[0] {
            Delivery::Summary { title, body, missed, .. } => {
                assert_eq!(title, "3 reminders");
                assert_eq!(body, "Pay rent · Call the dentist · Water plants");
                assert!(!missed);
            }
            d => panic!("{d:?}"),
        }
    }

    #[test]
    fn fired_once_per_fire_at_and_rearms_on_snooze() {
        let s = seeded();
        let due = s.due_alerts(at(9, 0)).unwrap();
        for (a, _) in &due {
            s.mark_fired(a, "test", at(9, 0)).unwrap();
        }
        assert!(s.due_alerts(at(9, 1)).unwrap().is_empty());
        let a = &due[0].0;
        assert_eq!(s.alert_state(&s.alerts_of(&a.node).unwrap()[0]), "fired");
        // Snooze moves fire_at, so it fires again later.
        let snooze = Op::AlertSnooze { id: a.id.clone(), until: "2026-10-03T10:00".into() };
        s.apply(&Event { v: 1, eid: "snz".into(), hlc: Hlc(999, 0), dev: "d".into(), actor: Actor { kind: "human".into(), name: None }, via: "t".into(), tx: "snz".into(), op: snooze }).unwrap();
        assert!(s.due_alerts(at(9, 30)).unwrap().is_empty());
        assert_eq!(s.due_alerts(at(10, 0)).unwrap().len(), 1);
    }

    #[test]
    fn ack_from_another_device_prevents_firing() {
        let s = seeded();
        let a = s.due_alerts(at(9, 0)).unwrap()[0].0.clone();
        s.apply(&Event { v: 1, eid: "ack".into(), hlc: Hlc(999, 0), dev: "other".into(), actor: Actor { kind: "human".into(), name: None }, via: "t".into(), tx: "ack".into(), op: Op::AlertAck { id: a.id.clone() } }).unwrap();
        assert!(s.due_alerts(at(9, 0)).unwrap().iter().all(|(x, _)| x.id != a.id));
    }

    #[test]
    fn missed_and_stale() {
        let s = seeded();
        let due = s.due_alerts(at(13, 0)).unwrap();
        match &plan(&s, &due, at(13, 0))[0] {
            Delivery::Summary { title, missed: true, .. } => assert_eq!(title, "Missed while away: 3 reminders"),
            d => panic!("{d:?}"),
        }
        let tomorrow = today().succ_opt().unwrap().and_hms_opt(22, 0, 0).unwrap();
        let due = s.due_alerts(tomorrow).unwrap();
        assert!(matches!(&plan(&s, &due, tomorrow)[0], Delivery::Silent { alerts } if alerts.len() == 3));
    }

    #[test]
    fn notification_text() {
        let s = seeded();
        let (a, n) = s.due_alerts(at(9, 0)).unwrap().into_iter().next().unwrap();
        let note = build(&s, &a, &n, today());
        assert_eq!(note.title, "Pay rent");
        assert_eq!(note.line1, "09:00");
        assert_eq!(note.line2, "in today's journal");
        assert_eq!(note.thread, "thc.alerts.2026-10-03");
        assert!(note.time_sensitive && note.is_task);
    }
}
