//! Why a row is in Today or on an agenda day (views.md §3.3). A list, because a row can be
//! there for several reasons: `["scheduled", "alert"]`.

use crate::model::Node;
use crate::store::Store;
use chrono::NaiveDate;
use std::collections::HashMap;

pub const CODES: &[&str] = &["overdue", "due-today", "scheduled", "alert", "alert-fired", "repeating", "doing", "done-today"];

#[derive(Debug, Clone, PartialEq)]
pub struct Reason {
    pub code: &'static str,
    /// `09:00` for alerts; the scheduled date when it isn't `day`.
    pub detail: Option<String>,
}

fn date10(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

/// Alerts loaded once for a whole listing: node → [(fire_at, fired here)], acked ones left out.
/// A listing of thousands of rows would otherwise run two queries per row.
pub struct Alerts(HashMap<String, Vec<(String, bool)>>);

impl Alerts {
    pub fn load(store: &Store) -> Alerts {
        let fired: HashMap<String, String> = store
            .conn
            .prepare("SELECT alert, fired_for FROM alert_local")
            .and_then(|mut st| st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect())
            .unwrap_or_default();
        let mut m: HashMap<String, Vec<(String, bool)>> = HashMap::new();
        for a in store.alerts_where("deleted=0 AND state != 'acked' AND fire_at IS NOT NULL", &[]).unwrap_or_default() {
            let f = a.fire_at.clone().unwrap_or_default();
            let was_fired = fired.get(&a.id).is_some_and(|x| *x == f);
            m.entry(a.node).or_default().push((f, was_fired));
        }
        Alerts(m)
    }

    fn of(&self, id: &str) -> &[(String, bool)] {
        self.0.get(id).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Reasons for `n` on `day` (`today` for overdue and done-today). One node: loads its alerts.
pub fn reasons(store: &Store, n: &Node, day: NaiveDate, today: NaiveDate) -> Vec<Reason> {
    let mut one = HashMap::new();
    for a in store.alerts_of(&n.id).unwrap_or_default() {
        if a.state != "acked" {
            if let Some(f) = a.fire_at.clone() {
                let fired = store.fired_at(&a).is_some();
                one.entry(n.id.clone()).or_insert_with(Vec::new).push((f, fired));
            }
        }
    }
    reasons_with(&Alerts(one), n, day, today)
}

/// Reasons with alerts preloaded ([`Alerts::load`]), for listings.
pub fn reasons_with(alerts: &Alerts, n: &Node, day: NaiveDate, today: NaiveDate) -> Vec<Reason> {
    let mut out = Vec::new();
    let r = |code, detail: Option<String>| Reason { code, detail };
    let open = n.is_open();
    let due = n.due.as_deref().and_then(date10);
    let sched = n.scheduled.as_deref().and_then(date10);
    if open && day == today && due.is_some_and(|d| d < today) {
        out.push(r("overdue", None));
    }
    if due == Some(day) {
        out.push(r("due-today", None));
    }
    // Scheduled shows from its date on (Today), or on its own day (agenda, events).
    match sched {
        Some(s) if s == day => out.push(r("scheduled", None)),
        Some(s) if s < day && day == today && open => out.push(r("scheduled", Some(s.format("%Y-%m-%d").to_string()))),
        _ => {}
    }
    for (f, fired) in alerts.of(&n.id) {
        let time = f.get(11..16).map(str::to_string);
        if *fired && day == today {
            out.push(r("alert-fired", time));
        } else if date10(f) == Some(day) {
            out.push(r("alert", time));
        }
    }
    if n.repeat.is_some() {
        out.push(r("repeating", None));
    }
    if n.status.as_deref() == Some("doing") {
        out.push(r("doing", None));
    }
    if n.status.as_deref() == Some("done") && n.done_at.as_deref().and_then(date10) == Some(today) && day == today {
        out.push(r("done-today", None));
    }
    out
}

pub fn codes(rs: &[Reason]) -> Vec<&'static str> {
    rs.iter().map(|r| r.code).collect()
}

/// `scheduled today · alert 09:00`: the words read like the row's meta (`due today`,
/// `scheduled yesterday`, `scheduled tue`, `alert fired 09:00`). Used by the TUI's `why
/// here` and `thc today --why`.
pub fn describe(rs: &[Reason], day: NaiveDate, today: NaiveDate) -> String {
    let rel = |d: NaiveDate| crate::dates::relative(d, today);
    rs.iter()
        .map(|r| match (r.code, &r.detail) {
            ("due-today", _) => format!("due {}", rel(day)),
            ("scheduled", Some(d)) => match date10(d) {
                Some(x) => format!("scheduled {}", rel(x)),
                None => "scheduled".into(),
            },
            ("scheduled", None) => format!("scheduled {}", rel(day)),
            ("alert", Some(t)) => format!("alert {t}"),
            ("alert-fired", Some(t)) => format!("alert fired {t}"),
            ("alert-fired", None) => "alert fired".into(),
            ("done-today", _) => "done today".into(),
            (c, _) => c.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}
