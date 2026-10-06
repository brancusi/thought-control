//! Where a board stands (docs/design/status.md): every task's times derived from
//! the event log (§1), the counts, momentum, medians, tokens (from props, §2), per actor, and
//! the blocked tasks with their bottlenecks. Read-only; no format change.

use crate::store::Store;
use anyhow::Result;
use chrono::{Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Doing → done faster than this is bookkeeping (a batch close), not work: counted as done,
/// left out of medians.
pub const BATCH_MS: i64 = 60_000;

/// One task's times (§1), from its events. Milliseconds since the epoch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Times {
    pub filed: i64,
    /// The first event that set status=doing, by anyone. None: untracked.
    pub started: Option<i64>,
    /// The latest completion.
    pub done: Option<i64>,
    /// The latest completion's transaction (for review state).
    pub done_tx: Option<String>,
    /// Times it went from done back to open.
    pub reopened: u32,
}

impl Times {
    pub fn worked(&self, now: i64) -> Option<i64> {
        self.started.map(|s| self.done.unwrap_or(now) - s)
    }
    pub fn waited(&self) -> Option<i64> {
        self.started.map(|s| s - self.filed)
    }
    pub fn lead(&self) -> Option<i64> {
        self.done.map(|d| d - self.filed)
    }
    pub fn batch(&self) -> bool {
        matches!((self.started, self.done), (Some(s), Some(d)) if d - s < BATCH_MS)
    }
}

/// Every task's times, from the event log in one pass over their events.
pub fn times(store: &Store, ids: &[String]) -> Result<HashMap<String, Times>> {
    let mut out: HashMap<String, Times> = HashMap::new();
    let mut st = store.conn.prepare("SELECT created_ms FROM nodes WHERE id = ?1")?;
    let mut ev = store.conn.prepare("SELECT ms, op, tx, body FROM events WHERE entity = ?1 AND op IN ('node.set', 'node.complete') ORDER BY okey")?;
    for id in ids {
        let filed: i64 = st.query_row([id], |r| r.get(0)).unwrap_or(0);
        let mut t = Times { filed, ..Default::default() };
        let rows = ev.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))?;
        for row in rows {
            let (ms, op, tx, body) = row?;
            if op == "node.complete" {
                t.done = Some(ms);
                t.done_tx = Some(tx);
                continue;
            }
            let status = serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|v| v["props"]["status"].as_str().map(str::to_string));
            match status.as_deref() {
                Some("doing") => {
                    if t.started.is_none() {
                        t.started = Some(ms);
                    }
                    if t.done.take().is_some() {
                        t.reopened += 1;
                        t.done_tx = None;
                    }
                }
                Some("todo" | "waiting") if t.done.is_some() => {
                    t.done = None;
                    t.done_tx = None;
                    t.reopened += 1;
                }
                Some("done") => {
                    t.done = Some(ms);
                    t.done_tx = Some(tx);
                }
                _ => {}
            }
        }
        out.insert(id.clone(), t);
    }
    Ok(out)
}

/// The range a report covers (§4): today, the last N days (3d, 7d), or everything.
#[derive(Clone, Debug, Serialize)]
pub struct Range {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub to: String,
    #[serde(skip)]
    pub from_ms: Option<i64>,
    #[serde(skip)]
    pub to_ms: i64,
}

impl Range {
    /// `today`, `3d`, `7d`, `Nd`, `all`, or a date (from that day on).
    pub fn parse(label: &str, now: NaiveDateTime) -> Result<Range> {
        let today = now.date();
        let from_day = match label {
            "all" => None,
            "today" => Some(today),
            l if l.ends_with('d') && l[..l.len() - 1].parse::<i64>().is_ok() => {
                let n: i64 = l[..l.len() - 1].parse().unwrap_or(3);
                Some(today - Duration::days(n.max(1) - 1))
            }
            l => Some(crate::dates::parse(l, today).map_err(|_| crate::error::invalid(format!("unknown range {l:?} · today, 3d, 7d, all, or a date")))?.date()),
        };
        let from_ms = from_day.map(|d| local_ms(d.and_hms_opt(0, 0, 0).unwrap()));
        let to_ms = local_ms(now);
        Ok(Range { label: label.to_string(), from: from_ms.map(rfc3339), to: rfc3339(to_ms), from_ms, to_ms })
    }

    pub fn contains(&self, ms: i64) -> bool {
        self.from_ms.is_none_or(|f| ms >= f) && ms <= self.to_ms
    }
}

fn local_ms(t: NaiveDateTime) -> i64 {
    Local.from_local_datetime(&t).earliest().map(|d| d.timestamp_millis()).unwrap_or(0)
}

fn local(ms: i64) -> NaiveDateTime {
    Local.timestamp_millis_opt(ms).single().map(|d| d.naive_local()).unwrap_or_default()
}

/// RFC 3339 in the device's offset, to the second.
pub fn rfc3339(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().map(|d| d.format("%Y-%m-%dT%H:%M:%S%:z").to_string()).unwrap_or_default()
}

/// Whole minutes, to the nearest.
pub fn minutes(ms: i64) -> i64 {
    (ms + 30_000) / 60_000
}

/// `38m`, `1h 46m`, `2d 3h`; under a minute `<1m`.
pub fn duration(ms: i64) -> String {
    if ms < 60_000 {
        return "<1m".into();
    }
    let m = minutes(ms);
    if m < 1 {
        return "<1m".into();
    }
    let (d, h, mm) = (m / 1440, (m % 1440) / 60, m % 60);
    match (d, h) {
        (0, 0) => format!("{mm}m"),
        (0, _) => format!("{h}h {mm}m"),
        _ => format!("{d}d {h}h"),
    }
}

/// `412k`, `2.3M`.
pub fn tokens_short(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Tokens {
    #[serde(rename = "in")]
    pub input: u64,
    pub out: u64,
    pub cache: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<String>,
}

impl Tokens {
    /// From a task's props; None when it has none.
    pub fn of(props: &serde_json::Map<String, serde_json::Value>) -> Option<Tokens> {
        // Props infer numbers as floats (`6000.0`); a string or an integer reads too.
        let n = |k: &str| props.get(k).and_then(|v| v.as_u64().or_else(|| v.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)).or_else(|| v.as_str().and_then(|s| s.parse().ok())));
        let (input, out) = (n("tokens_in"), n("tokens_out"));
        if input.is_none() && out.is_none() {
            return None;
        }
        Some(Tokens {
            input: input.unwrap_or(0),
            out: out.unwrap_or(0),
            cache: n("tokens_cache").unwrap_or(0),
            source: props.get("tokens_source").and_then(|v| v.as_str()).map(str::to_string),
            sessions: props.get("session_id").and_then(|v| v.as_str()).map(|s| vec![s.to_string()]).unwrap_or_default(),
        })
    }
    pub fn shown(&self) -> u64 {
        self.input + self.out
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Task {
    pub id: String,
    pub short: String,
    pub title: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    pub filed: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worked: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waited: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead: Option<i64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub untracked: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub batch: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub long: bool,
    pub reopened: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Tokens>,
    #[serde(skip)]
    pub times: Times,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Counts {
    pub done: usize,
    pub done_today: usize,
    pub doing: usize,
    pub ready: usize,
    pub todo: usize,
    pub waiting: usize,
    pub blocked: usize,
    pub to_review: usize,
    pub untracked: usize,
    pub batch: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Timing {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worked_median: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worked_p90: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waited_median: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead_median: Option<i64>,
    /// How many tasks the medians use (done in range, tracked, not batch).
    pub sample: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Day {
    pub date: String,
    pub done: usize,
    pub started: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Hour {
    pub hour: u32,
    pub done: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Momentum {
    pub days: Vec<Day>,
    pub today: Vec<Hour>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TokenTotals {
    #[serde(rename = "in")]
    pub input: u64,
    pub out: u64,
    pub cache: u64,
    pub known: usize,
    pub of: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Actor {
    pub actor: String,
    pub done: usize,
    pub doing: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worked_median: Option<i64>,
    pub tokens: TokenTotals,
}

#[derive(Clone, Debug, Serialize)]
pub struct Blocked {
    pub id: String,
    pub short: String,
    pub title: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// A bottleneck: every open task it holds up, through any chain.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub holds_up: Vec<String>,
    /// The first step of that chain.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub direct: Vec<String>,
    /// A blocked task that isn't a bottleneck: its open blockers.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub range: Range,
    pub counts: Counts,
    pub timing: Timing,
    pub momentum: Momentum,
    pub tokens: TokenTotals,
    pub actors: Vec<Actor>,
    pub tasks: Vec<Task>,
    pub blocked: Vec<Blocked>,
}

fn median(v: &mut [i64]) -> Option<i64> {
    pct(v, 50)
}

fn pct(v: &mut [i64], p: usize) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let i = ((v.len() - 1) * p + 50) / 100;
    Some(v[i.min(v.len() - 1)])
}

const OPEN: &[&str] = &["todo", "doing", "waiting"];

/// The report for the tasks under `root` (the board's page; None: the whole vault), in `range`,
/// optionally only `by` one actor (the task's owner). `now` is the device's local time.
pub fn report(store: &Store, root: Option<&str>, range: Range, by: Option<&str>, now: NaiveDateTime) -> Result<Report> {
    let today = now.date();
    let now_ms = range.to_ms;
    let under = root.map(|r| format!(" under:{r}")).unwrap_or_default();
    let all = store.query(&format!("is:task status:any{under}"), today, 100_000)?;
    let ids: Vec<String> = all.iter().map(|n| n.id.clone()).collect();
    let times = times(store, &ids)?;
    let ready: HashSet<String> = store.query(&format!("is:task is:ready{under}"), today, 100_000)?.into_iter().map(|n| n.id).collect();
    let pending: HashSet<String> = crate::review::queue_txs(store, None, None)?.into_iter().collect();
    let mut reviewed_at: HashMap<String, i64> = HashMap::new();
    {
        let mut st = store.conn.prepare("SELECT tx, ms FROM reviews WHERE verdict = 'accepted'")?;
        for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (tx, ms) = r?;
            reviewed_at.insert(tx, ms);
        }
    }
    // Blocks edges between open tasks.
    let status_of: HashMap<&str, &str> = all.iter().map(|n| (n.id.as_str(), n.status.as_deref().unwrap_or(""))).collect();
    let open = |id: &str| status_of.get(id).is_some_and(|s| OPEN.contains(s));
    let mut blocks: HashMap<String, Vec<String>> = HashMap::new();
    let mut blockers: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut st = store.conn.prepare("SELECT src, dst FROM edges WHERE rel = 'blocks' ORDER BY src, dst")?;
        for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (a, b) = r?;
            if open(&a) && open(&b) {
                blocks.entry(a.clone()).or_default().push(b.clone());
                blockers.entry(b).or_default().push(a);
            }
        }
    }

    // The tasks: done in range, or open now; for one actor if asked.
    let mut tasks: Vec<Task> = Vec::new();
    let mut p90_pool: Vec<i64> = Vec::new();
    for n in &all {
        let t = times.get(&n.id).cloned().unwrap_or_default();
        let status = n.status.clone().unwrap_or_default();
        let is_open = OPEN.contains(&status.as_str());
        let done_in = t.done.is_some_and(|d| range.contains(d)) && !is_open;
        if !is_open && !done_in {
            continue;
        }
        let props = store.props_of(&n.id)?;
        let s = |k: &str| props.get(k).and_then(|v| v.as_str()).filter(|v| !v.is_empty()).map(str::to_string);
        let owner = s("owner");
        if by.is_some_and(|b| owner.as_deref() != Some(b)) {
            continue;
        }
        let untracked = t.started.is_none() && !is_open;
        if done_in && !untracked && !t.batch() {
            if let Some(w) = t.worked(now_ms) {
                p90_pool.push(minutes(w));
            }
        }
        tasks.push(Task {
            id: n.id.clone(),
            short: store.short(&n.id),
            title: n.title.clone().unwrap_or_else(|| n.text.lines().next().unwrap_or("").to_string()),
            status,
            owner,
            role: s("role"),
            priority: n.priority.clone(),
            filed: rfc3339(t.filed),
            started: t.started.map(rfc3339),
            done: if is_open { None } else { t.done.map(rfc3339) },
            reviewed: t.done_tx.as_ref().and_then(|tx| reviewed_at.get(tx)).copied().map(rfc3339),
            worked: t.worked(now_ms).map(minutes),
            waited: t.waited().map(minutes),
            lead: if is_open { None } else { t.lead().map(minutes) },
            untracked,
            batch: !is_open && t.batch(),
            long: false,
            reopened: t.reopened,
            tokens: Tokens::of(&props),
            times: t,
        });
    }
    // Long: in flight past the p90 of worked times in range, and at least 2 h.
    let long_min = pct(&mut p90_pool.clone(), 90).unwrap_or(0).max(120);
    for t in tasks.iter_mut().filter(|t| t.status == "doing") {
        t.long = t.worked.is_some_and(|w| w > long_min);
    }
    // Newest first: in flight by start, landed by done.
    tasks.sort_by_key(|t| std::cmp::Reverse(t.times.done.or(t.times.started).unwrap_or(t.times.filed)));

    let done: Vec<&Task> = tasks.iter().filter(|t| t.done.is_some()).collect();
    let today_from = local_ms(today.and_hms_opt(0, 0, 0).unwrap());
    let mut counts = Counts {
        done: done.len(),
        done_today: done.iter().filter(|t| t.times.done.is_some_and(|d| d >= today_from)).count(),
        untracked: done.iter().filter(|t| t.untracked).count(),
        batch: done.iter().filter(|t| t.batch).count(),
        to_review: done.iter().filter(|t| t.times.done_tx.as_ref().is_some_and(|tx| pending.contains(tx))).count(),
        ..Default::default()
    };
    for t in &tasks {
        match t.status.as_str() {
            "doing" => counts.doing += 1,
            "todo" => counts.todo += 1,
            "waiting" => counts.waiting += 1,
            _ => {}
        }
        if ready.contains(&t.id) {
            counts.ready += 1;
        }
        if blockers.contains_key(&t.id) {
            counts.blocked += 1;
        }
    }
    let sample: Vec<&&Task> = done.iter().filter(|t| !t.untracked && !t.batch).collect();
    let timing = Timing {
        worked_median: median(&mut sample.iter().filter_map(|t| t.worked).collect::<Vec<_>>()),
        worked_p90: pct(&mut sample.iter().filter_map(|t| t.worked).collect::<Vec<_>>(), 90),
        waited_median: median(&mut sample.iter().filter_map(|t| t.waited).collect::<Vec<_>>()),
        lead_median: median(&mut sample.iter().filter_map(|t| t.lead).collect::<Vec<_>>()),
        sample: sample.len(),
    };

    // Momentum: done and started per local day in range, and done per hour today.
    let first_day: NaiveDate = range.from_ms.map(|f| local(f).date()).unwrap_or_else(|| tasks.iter().filter_map(|t| t.times.done.or(t.times.started)).min().map(|m| local(m).date()).unwrap_or(today));
    let mut days: BTreeMap<NaiveDate, (usize, usize)> = BTreeMap::new();
    let mut d = first_day;
    while d <= today {
        days.insert(d, (0, 0));
        d += Duration::days(1);
    }
    let mut hours: BTreeMap<u32, usize> = BTreeMap::new();
    for t in &tasks {
        if let Some(ms) = t.times.done.filter(|_| t.done.is_some()) {
            let at = local(ms);
            if let Some(e) = days.get_mut(&at.date()) {
                e.0 += 1;
            }
            if at.date() == today {
                *hours.entry(at.hour()).or_default() += 1;
            }
        }
        if let Some(ms) = t.times.started.filter(|&s| range.contains(s)) {
            if let Some(e) = days.get_mut(&local(ms).date()) {
                e.1 += 1;
            }
        }
    }
    let momentum = Momentum {
        days: days.into_iter().map(|(d, (done, started))| Day { date: d.format("%Y-%m-%d").to_string(), done, started }).collect(),
        today: hours.into_iter().map(|(hour, done)| Hour { hour, done }).collect(),
    };

    // Tokens over the done tasks; coverage stated.
    let totals = |ts: &[&Task]| {
        let mut t = TokenTotals { of: ts.len(), ..Default::default() };
        for x in ts {
            if let Some(k) = &x.tokens {
                t.input += k.input;
                t.out += k.out;
                t.cache += k.cache;
                t.known += 1;
            }
        }
        t
    };
    let tokens = totals(&done);

    // Per actor (a task's owner; untracked work under "(no owner)").
    let mut by_actor: BTreeMap<String, Vec<&Task>> = BTreeMap::new();
    for t in &tasks {
        by_actor.entry(t.owner.clone().unwrap_or_else(|| "(no owner)".into())).or_default().push(t);
    }
    let mut actors: Vec<Actor> = by_actor
        .into_iter()
        .map(|(actor, ts)| {
            let d: Vec<&Task> = ts.iter().copied().filter(|t| t.done.is_some()).collect();
            Actor {
                actor,
                done: d.len(),
                doing: ts.iter().filter(|t| t.status == "doing").count(),
                worked_median: median(&mut d.iter().filter(|t| !t.untracked && !t.batch).filter_map(|t| t.worked).collect::<Vec<_>>()),
                tokens: totals(&d),
            }
        })
        .collect();
    actors.sort_by(|a, b| b.done.cmp(&a.done).then(a.actor.cmp(&b.actor)));

    // Blocked: bottlenecks first (open tasks that block others and aren't blocked themselves),
    // by how many they hold up through any chain; then every other blocked task.
    let node = |id: &str| all.iter().find(|n| n.id == id);
    let holds_up = |root: &str| {
        let mut seen: Vec<String> = Vec::new();
        let mut stack = vec![root.to_string()];
        while let Some(x) = stack.pop() {
            for y in blocks.get(&x).into_iter().flatten() {
                if !seen.contains(y) && y != root {
                    seen.push(y.clone());
                    stack.push(y.clone());
                }
            }
        }
        seen
    };
    let entry = |id: &str| -> Option<Blocked> {
        let n = node(id)?;
        let props = store.props_of(id).ok()?;
        Some(Blocked {
            id: id.to_string(),
            short: store.short(id),
            title: n.title.clone().unwrap_or_else(|| n.text.lines().next().unwrap_or("").to_string()),
            status: n.status.clone().unwrap_or_default(),
            owner: props.get("owner").and_then(|v| v.as_str()).filter(|v| !v.is_empty()).map(str::to_string),
            holds_up: vec![],
            direct: vec![],
            blocked_by: vec![],
        })
    };
    let mut bottlenecks: Vec<Blocked> = blocks
        .keys()
        .filter(|id| !blockers.contains_key(*id))
        .filter_map(|id| {
            let mut b = entry(id)?;
            b.holds_up = holds_up(id);
            b.direct = blocks.get(id).cloned().unwrap_or_default();
            Some(b)
        })
        .collect();
    bottlenecks.sort_by(|a, b| b.holds_up.len().cmp(&a.holds_up.len()).then(a.short.cmp(&b.short)));
    let mut rest: Vec<Blocked> = blockers
        .iter()
        .filter_map(|(id, by)| {
            let mut b = entry(id)?;
            b.blocked_by = by.clone();
            Some(b)
        })
        .collect();
    rest.sort_by(|a, b| a.short.cmp(&b.short));
    bottlenecks.extend(rest);

    Ok(Report { range, counts, timing, momentum, tokens, actors, tasks, blocked: bottlenecks })
}

/// One task's times as a line (§3.4): `filed Tue 11:40 · started 13:26 · done 14:04 · worked 38m
/// · waited 1h 46m · 412k tokens (1 session)`, or `… · no start recorded`.
pub fn times_line(t: &Times, tokens: Option<&Tokens>, open: bool, now_ms: i64) -> String {
    let at = |ms: i64| {
        let d = local(ms);
        if d.date() == local(now_ms).date() { d.format("%H:%M").to_string() } else { d.format("%a %H:%M").to_string() }
    };
    let mut parts = vec![format!("filed {}", at(t.filed))];
    match t.started {
        Some(s) => parts.push(format!("started {}", at(s))),
        None if !open => {
            if let Some(d) = t.done {
                parts.push(format!("done {}", at(d)));
            }
            parts.push("no start recorded".into());
            return parts.join(" · ");
        }
        None => {}
    }
    if let Some(d) = t.done.filter(|_| !open) {
        parts.push(format!("done {}", at(d)));
    }
    if let Some(w) = t.worked(now_ms) {
        parts.push(format!("worked {}", duration(w)));
    }
    if let Some(w) = t.waited() {
        parts.push(format!("waited {}", duration(w)));
    }
    if let Some(k) = tokens {
        let s = k.sessions.len();
        let approx = if k.source.as_deref() == Some("self") { "~" } else { "" };
        parts.push(format!("{approx}{} tokens{}", tokens_short(k.shown()), if s > 0 { format!(" ({s} session{})", if s == 1 { "" } else { "s" }) } else { String::new() }));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_tokens_read_like_the_spec() {
        assert_eq!(duration(38 * 60_000), "38m");
        assert_eq!(duration(106 * 60_000), "1h 46m");
        assert_eq!(duration((2 * 1440 + 3 * 60) * 60_000), "2d 3h");
        assert_eq!(duration(30_000), "<1m");
        assert_eq!(tokens_short(412_000), "412k");
        assert_eq!(tokens_short(2_300_000), "2.3M");
        let mut v = vec![5, 1, 3];
        assert_eq!(median(&mut v), Some(3));
        assert_eq!(median(&mut []), None);
    }

    #[test]
    fn times_mark_batch_and_untracked() {
        let t = Times { filed: 0, started: Some(1_000), done: Some(30_000), ..Default::default() };
        assert!(t.batch());
        let u = Times { filed: 0, started: None, done: Some(10), ..Default::default() };
        assert_eq!((u.worked(99), u.waited()), (None, None));
        assert!(times_line(&u, None, false, 99).ends_with("no start recorded"));
    }
}
