//! Today's sections (tui-handoff.md §5.1, views.md §3.3) for one vault: Overdue, Today, the rows
//! only an alert puts in Today, Doing, Next 7 days and Done today, each in Today's own order.
//! Merging several vaults, contexts and the `@today` override are the caller's.

use crate::model::Node;
use crate::store::Store;
use anyhow::Result;
use chrono::{Duration, NaiveDate};

/// One vault's Today, by section.
#[derive(Clone, Debug, Default)]
pub struct Sections {
    pub overdue: Vec<Node>,
    pub today: Vec<Node>,
    /// In Today only because an alert fires today: after the Today rows, by alert time.
    pub alerted: Vec<Node>,
    pub doing: Vec<Node>,
    pub next: Vec<Node>,
    pub done: Vec<Node>,
}

fn ymd(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub fn sections(s: &Store, day: NaiveDate) -> Result<Sections> {
    let t = ymd(day);
    let horizon = ymd(day + Duration::days(7));
    let open = "n.status IN ('todo','doing','waiting') AND n.deleted=0";
    let overdue = s.nodes_where(&format!("{open} AND substr(n.due,1,10) < ?1 ORDER BY n.due"), &[&t])?;
    // ISO ranges so each branch uses an index (as in the CLI and daemon).
    let t1 = ymd(day + Duration::days(1));
    let today = s.nodes_where(
        "n.deleted=0 AND n.is_tag=0 AND (n.status IS NULL OR n.status IN ('todo','waiting')) \
         AND NOT (n.due IS NOT NULL AND n.due < ?1) AND n.id IN ( \
           SELECT id FROM nodes WHERE status IN ('todo','doing','waiting') AND deleted=0 AND scheduled < ?2 \
           UNION SELECT id FROM nodes WHERE due >= ?1 AND due < ?2 \
           UNION SELECT id FROM nodes WHERE status IS NULL AND scheduled >= ?1 AND scheduled < ?2) \
         ORDER BY n.scheduled IS NULL, n.scheduled, CASE n.priority WHEN 'high' THEN 0 WHEN 'med' THEN 1 ELSE 2 END",
        &[&t, &t1],
    )?;
    let doing = s.nodes_where(
        "n.status='doing' AND n.deleted=0 AND NOT (n.due IS NOT NULL AND substr(n.due,1,10) < ?1) ORDER BY n.updated_ms DESC",
        &[&t],
    )?;
    // Dates as ranges (`> today` is `>= tomorrow`, `<= horizon` is `< the day after`) so the
    // due and scheduled indexes serve it: substr() forced a scan of every note.
    let h1 = ymd(day + Duration::days(8));
    let next = s.nodes_where(
        "n.deleted=0 AND n.is_tag=0 AND (n.status IS NULL OR n.status IN ('todo','waiting')) AND n.id IN ( \
           SELECT id FROM nodes WHERE scheduled >= ?1 AND scheduled < ?2 \
           UNION SELECT id FROM nodes WHERE due >= ?1 AND due < ?2) \
         AND NOT (n.status IS NOT NULL AND coalesce(n.scheduled,'9999') < ?1) \
         ORDER BY min(coalesce(n.scheduled,'9'), coalesce(n.due,'9')), n.rowid",
        &[&t1, &h1],
    )?;
    let _ = &horizon;
    // In Today only because an alert fires today: Today rows, by alert time, and no longer
    // under Next 7 days (views.md §3.3).
    let shown: std::collections::HashSet<String> = overdue.iter().chain(today.iter()).chain(doing.iter()).map(|n| n.id.clone()).collect();
    let alerted: Vec<Node> = s.alert_rows(day, &shown)?.into_iter().map(|(n, _)| n).collect();
    let next: Vec<Node> = next.into_iter().filter(|n| !today.iter().chain(alerted.iter()).any(|t| t.id == n.id)).collect();
    let done = s.nodes_where("n.deleted=0 AND n.status='done' AND n.done_at >= ?1 AND n.done_at < ?2 ORDER BY n.done_at", &[&t, &t1])?;
    Ok(Sections { overdue, today, alerted, doing, next, done })
}
