//! Feeds: pure functions from the store (and an explicit `now`) to the JSON each view binds to.
//! A feed decides what a row says, already in words (`due tomorrow`, `◆ claude`, `¶ Home`) and
//! as theme-token markup; the screen (screens.rs) decides where it goes. The words are
//! thc_core::present's, shared with the TUI.
//!
//! Row records:
//! - a node: `kind` (`node`, `done`, `cancelled`), `id`, `short`, `status` (`todo` `doing` `waiting`
//!   `done` `cancelled` `note`, or `page` / `tag` in search), `text` (plain), `text_m` (markup),
//!   `meta_m` (the fullest meta) and `metas` (§4.2's variants, fullest first), and `children` in a
//!   tree;
//! - a section: `kind: "section"`, `title_m`, `count`, `note_m`, and its rows as `children`;
//! - `blank`, and `empty` (`m`): the §8.1 empty states.

use crate::markup::{self, styled, tok};
use anyhow::Result;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use thc_core::dates::DateVal;
use thc_core::glyphs::Glyphs;
use thc_core::model::{HistoryEntry, Node};
use thc_core::palette::Token;
use thc_core::present;
use thc_core::store::Store;

/// What every feed reads: the store, the glyph set and the time it's drawn at.
pub struct Cx<'a> {
    pub store: &'a Store,
    pub g: &'a Glyphs,
    pub now: NaiveDateTime,
}

impl Cx<'_> {
    pub fn today(&self) -> NaiveDate {
        self.now.date()
    }
}

/// How a row's meta reads in a view (§4.1).
#[derive(Clone, Copy, Default)]
struct Ctx<'s> {
    /// Name the page or day it lives in (`¶ Home`), unless that's `subject`.
    context: bool,
    /// Name today's journal too (Search).
    include_today: bool,
    subject: Option<&'s str>,
    /// Done rows show their done time (Today, Journal).
    done_time: bool,
    /// Rows with no other meta show when they were captured (Inbox).
    captured: bool,
    /// Words to highlight (Search).
    terms: &'s str,
    /// Under an agenda day heading: that day's date part goes from the meta (§5.1).
    under_day: Option<NaiveDate>,
}

// ---- rows ---------------------------------------------------------------------------------------

fn status_key(n: &Node) -> &'static str {
    match n.status.as_deref() {
        Some("doing") => "doing",
        Some("waiting") => "waiting",
        Some("done") => "done",
        Some("cancelled") => "cancelled",
        Some(_) => "todo",
        None => "note",
    }
}

fn closed(n: &Node) -> bool {
    matches!(n.status.as_deref(), Some("done" | "cancelled"))
}

/// A node's one-line text: a page's title, else the first line (` …` when there are more).
fn one_line(cx: &Cx, n: &Node) -> String {
    if n.title.is_some() || n.journal.is_some() {
        return n.label();
    }
    let rendered = cx.store.render_text(&n.text);
    let mut first = rendered.lines().next().unwrap_or("").to_string();
    if rendered.lines().count() > 1 {
        first.push_str(&format!(" {}", cx.g.ellipsis));
    }
    first
}

/// The row's text as markup: spans for open rows; done rows one muted run, cancelled struck.
fn text_m(n: &Node, text: &str, terms: &str) -> String {
    match n.status.as_deref() {
        Some("done") => styled("muted", text),
        Some("cancelled") => styled("muted+s", text),
        _ => markup::text(text, false, terms),
    }
}

/// The page or journal day a node lives in.
fn root_of(store: &Store, n: &Node) -> Option<Node> {
    let mut cur = store.node(n.parent.as_deref()?).ok().flatten()?;
    for _ in 0..256 {
        match cur.parent.clone() {
            Some(p) => cur = store.node(&p).ok().flatten()?,
            None => return Some(cur),
        }
    }
    None
}

fn has_alert(store: &Store, id: &str) -> bool {
    store.alerts_where("node=?1 AND deleted=0 AND state IN ('pending','snoozed')", &[&id]).is_ok_and(|a| !a.is_empty())
}

fn local(ms: i64) -> Option<NaiveDateTime> {
    chrono::Local.timestamp_millis_opt(ms).single().map(|d| d.naive_local())
}

fn agent_name(actor: &str) -> Option<&str> {
    actor.starts_with("agent").then(|| actor.trim_start_matches("agent:").trim_start_matches("agent"))
}

/// Meta variants (§4.2), fullest first: full, without context, repeat shortened to `↻`, and
/// without the alert `◎`. Each is a markup string.
fn metas(cx: &Cx, n: &Node, c: Ctx) -> Vec<String> {
    let g = cx.g;
    let today = cx.today();
    if n.status.as_deref() == Some("done") && c.done_time {
        let t = n.done_at.as_deref().and_then(|d| d.split('T').nth(1)).unwrap_or("");
        return vec![tok(Token::Muted, t)];
    }
    let open = n.is_open() || n.status.is_none();
    let alert = has_alert(cx.store, &n.id);
    let ctx_label = if c.context {
        root_of(cx.store, n).filter(|r| Some(r.id.as_str()) != c.subject).and_then(|r| present::root_label(g, &r, today, c.include_today))
    } else {
        None
    };
    let build = |ctx: bool, short_repeat: bool, alert: bool| -> String {
        let mut items: Vec<Vec<(String, String)>> = Vec::new();
        let spec = |parts: Vec<(String, Token)>| parts.into_iter().map(|(t, k)| (t, k.name().to_string())).collect::<Vec<_>>();
        let timed = n.scheduled.as_deref().is_some_and(|s| s.contains('T'));
        let both = n.scheduled.is_some() && n.due.is_some();
        if let Some(s) = &n.scheduled {
            items.extend(present::date_item(g, today, s, false, open, alert && (timed || n.due.is_none()), c.under_day, both).map(spec));
        }
        if let Some(d) = &n.due {
            items.extend(present::date_item(g, today, d, true, n.is_open(), alert && !timed && n.scheduled.is_none(), c.under_day, both).map(spec));
        }
        if alert && n.scheduled.is_none() && n.due.is_none() {
            items.push(vec![(g.alert.to_string(), "muted".into())]);
        }
        if let Some(p) = &n.priority {
            items.push(vec![(format!("!{p}"), if p == "high" { "text+b".into() } else { "muted".into() })]);
        }
        if let Some(r) = n.repeat.as_ref().and_then(|r| r.get("text")).and_then(|t| t.as_str()) {
            let t = if short_repeat { g.repeat.to_string() } else { format!("{} {}", g.repeat, present::short_repeat(r)) };
            items.push(vec![(t, "muted".into())]);
        }
        if let Some(a) = agent_name(&n.created_by) {
            items.push(vec![(format!("{}{}{a}", g.agent, g.agent_sep), "agent".into())]);
        }
        if items.is_empty() && c.captured {
            if let Some(t) = local(n.created_ms) {
                items.push(vec![(t.format("%b %-d %H:%M").to_string(), "muted".into())]);
            }
        }
        if ctx && let Some(l) = &ctx_label {
            items.push(vec![(l.clone(), "muted".into())]);
        }
        markup::join(&items)
    };
    let mut v = vec![build(true, false, alert), build(false, false, alert), build(false, true, alert), build(false, true, false)];
    v.dedup();
    v
}

fn node_row(cx: &Cx, n: &Node, c: Ctx) -> Value {
    let text = one_line(cx, n);
    let metas = metas(cx, n, c);
    let kind = match n.status.as_deref() {
        Some("done") => "done",
        Some("cancelled") => "cancelled",
        _ => "node",
    };
    json!({
        "kind": kind, "id": n.id, "short": cx.store.short(&n.id), "status": status_key(n),
        "text": text, "text_m": text_m(n, &text, c.terms), "meta_m": metas[0], "metas": metas,
    })
}

/// A section of a tree: its rows are its children.
fn section(title: &str, tone: Token, count: usize, children: Vec<Value>) -> Value {
    json!({"kind": "section", "id": format!("section:{title}"), "title": title, "title_m": styled(&format!("{}+b", tone.name()), title), "count": count, "children": children})
}

/// The §8.1 empty state: two lines, the second's keys in `text` and words in `muted`.
fn empty(l1: &str, l2: &[(&str, &str)]) -> Vec<Value> {
    vec![json!({"kind": "empty", "m": tok(Token::Text, l1)}), json!({"kind": "empty", "m": hints(l2, "  ·  ")})]
}

/// Key hints: `key` in `text`, its word in `muted` (§3.3).
pub fn hints(pairs: &[(&str, &str)], sep: &str) -> String {
    pairs
        .iter()
        .map(|(k, w)| if k.is_empty() { tok(Token::Muted, w) } else { format!("{} {}", tok(Token::Text, k), tok(Token::Muted, w)) })
        .collect::<Vec<_>>()
        .join(sep)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The bar's left side (§3.3): the view's name in `text`, counts in `muted`.
fn bar(name: &str, counts: &[(String, Token)]) -> String {
    let mut s = tok(Token::Text, name);
    for (c, t) in counts {
        s.push_str(&tok(Token::Muted, " · "));
        s.push_str(&tok(*t, c));
    }
    s
}

fn inbox_nodes(store: &Store) -> Result<Vec<Node>> {
    store.nodes_where(
        "n.parent IS NULL AND n.title IS NULL AND n.journal IS NULL AND n.is_tag=0 AND n.deleted=0 ORDER BY n.created_ms",
        &[],
    )
}

// ---- the frame ----------------------------------------------------------------------------------

/// The header's data (§3.1): the inbox count for its tab title, and the date; and the open
/// conflicts (§7.4): how many, and the banner's line.
pub fn frame(cx: &Cx) -> Result<Value> {
    let inbox = inbox_nodes(cx.store)?.len();
    let conflicts = cx.store.open_conflicts()?;
    let mut v = json!({"inbox": if inbox > 0 { inbox.to_string() } else { String::new() }, "date": cx.now.format("%a %b %-d").to_string(), "conflicts": conflicts.len()});
    if let Some((_, node, field, _)) = conflicts.iter().find(|c| c.2 != "rehomed").or(conflicts.first()) {
        let short = cx.store.short(node);
        let sentence = match field.as_str() {
            "parent" => "a move was skipped (it would loop)",
            "rehomed" => "its parent was deleted elsewhere",
            _ => "text was edited on two devices",
        };
        let n = plural(conflicts.len(), "conflict", "conflicts");
        v["banner_m"] = format!("{}{}", styled("conflict+b", &format!(" {} {n}", cx.g.conflict)), tok(Token::Conflict, &format!("  {short} {sentence}"))).into();
    }
    Ok(v)
}

/// What the host adds to a feed's rows from outside the store's rows: each node row's `gutter`
/// (§4: `conflict` on an open conflict, else `live` while an agent's change is new), and the
/// open conflicts at the end of the bar's counts (`· 1 conflict`, §7.4).
pub fn decorate(v: &mut Value, live: &HashSet<String>, conflicted: &HashSet<String>) {
    fn walk(v: &mut Value, live: &HashSet<String>, conflicted: &HashSet<String>) {
        match v {
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, live, conflicted)),
            Value::Object(m) => {
                if let (Some(id), true) = (m.get("id").and_then(Value::as_str).map(str::to_string), m.contains_key("short")) {
                    if conflicted.contains(&id) {
                        m.insert("gutter".into(), "conflict".into());
                    } else if live.contains(&id) {
                        m.insert("gutter".into(), "live".into());
                    }
                }
                for (_, x) in m.iter_mut() {
                    walk(x, live, conflicted);
                }
            }
            _ => {}
        }
    }
    walk(v, live, conflicted);
    let n = conflicted.len();
    if n > 0
        && let Some(bar) = v.get_mut("bar_m")
        && let Some(s) = bar.as_str()
    {
        *bar = format!("{s}{}{}", tok(Token::Muted, " · "), tok(Token::Conflict, &plural(n, "conflict", "conflicts"))).into();
    }
}

// ---- Today --------------------------------------------------------------------------------------

/// Today (§5.1): sections Overdue, Today, Doing, Next 7 days and Done today, each a tree row with
/// its rows under it. Done today folds when it holds more than three (`open: false`).
pub fn today(cx: &Cx) -> Result<Value> {
    let s = thc_core::today::sections(cx.store, cx.today())?;
    let c = Ctx { context: true, ..Ctx::default() };
    let rows_of = |ns: &[Node], c: Ctx| ns.iter().map(|n| node_row(cx, n, c)).collect::<Vec<_>>();
    let mut today_rows: Vec<Node> = s.today.clone();
    today_rows.extend(s.alerted.iter().cloned());
    let mut rows = Vec::new();
    for (title, tone, ns) in [("Overdue", Token::Overdue, &s.overdue), ("Today", Token::Text, &today_rows), ("Doing", Token::Text, &s.doing), ("Next 7 days", Token::Text, &s.next)] {
        if !ns.is_empty() {
            rows.push(section(title, tone, ns.len(), rows_of(ns, c)));
        }
    }
    if !s.done.is_empty() {
        let sec = section("Done today", Token::Text, s.done.len(), rows_of(&s.done, Ctx { done_time: true, ..c }));
        rows.push(sec);
    }
    let open = s.overdue.len() + today_rows.len() + s.doing.len() + s.next.len();
    if rows.is_empty() {
        rows = if s.done.is_empty() {
            empty("Nothing due today.", &[("a", "capture"), ("5", "journal")])
        } else {
            empty(&format!("All clear. {} done today.", s.done.len()), &[("a", "capture")])
        };
    }
    let inbox = inbox_nodes(cx.store)?.len();
    let mut counts = vec![(format!("{open} open"), Token::Muted)];
    if !s.overdue.is_empty() {
        counts.push((format!("{} overdue", s.overdue.len()), Token::Overdue));
    }
    if inbox > 0 {
        counts.push((format!("{inbox} inbox"), Token::Muted));
    }
    Ok(json!({"rows": rows, "bar_m": bar("today", &counts)}))
}

/// Agenda (§5.1, `w` on Today): a heading, Overdue, then a bold line per day for seven days
/// with its rows under it (their meta without that day's date), runs of empty days as one muted
/// line, then Later (the seven days after). The TUI's agenda (thc-tui app.rs `rows_agenda`).
pub fn agenda(cx: &Cx) -> Result<Value> {
    let s = cx.store;
    let today = cx.today();
    let ymd = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
    let c = Ctx { context: true, ..Ctx::default() };
    let mut rows = vec![json!({"kind": "section", "id": "section:Agenda", "title_m": styled("text+b", "Agenda"), "note_m": format!("{}   {}", tok(Token::Muted, "7 days"), hints(&[("w", "sections")], ""))})];
    let mut items = 0;
    let overdue = s.nodes_where("n.status IN ('todo','doing','waiting') AND n.deleted=0 AND substr(n.due,1,10) < ?1 ORDER BY n.due", &[&ymd(today)])?;
    if !overdue.is_empty() {
        items += overdue.len();
        rows.push(section("Overdue", Token::Overdue, overdue.len(), overdue.iter().map(|n| node_row(cx, n, c)).collect()));
    }
    let day_sql = "n.deleted=0 AND n.is_tag=0 AND (n.status IS NULL OR n.status IN ('todo','doing','waiting')) AND \
                   (substr(n.scheduled,1,10) = ?1 OR substr(n.due,1,10) = ?1) ORDER BY coalesce(n.scheduled, n.due)";
    let mut empty_run: Vec<NaiveDate> = Vec::new();
    let flush = |run: &mut Vec<NaiveDate>, rows: &mut Vec<Value>| {
        if let (Some(first), false) = (run.first(), run.is_empty()) {
            let label = run.iter().map(|d| d.format("%a %b %-d").to_string()).collect::<Vec<_>>().join(&format!(" {} ", cx.g.sep));
            rows.push(json!({"kind": "gap", "id": format!("gap:{first}"), "m": tok(Token::Muted, &label), "note_m": tok(Token::Muted, "nothing scheduled")}));
            run.clear();
        }
    };
    for i in 0..7 {
        let d = today + Duration::days(i);
        let ns = s.nodes_where(day_sql, &[&ymd(d)])?;
        if ns.is_empty() && i > 0 {
            empty_run.push(d);
            continue;
        }
        flush(&mut empty_run, &mut rows);
        let rel = thc_core::dates::relative(d, today);
        let title = match i {
            0 => format!("{}{}", styled("text+b", &d.format("%a %b %-d").to_string()), tok(Token::Today, &format!(" {} {rel}", cx.g.sep))),
            1 => format!("{}{}", styled("text+b", &d.format("%a %b %-d").to_string()), tok(Token::Muted, &format!(" {} {rel}", cx.g.sep))),
            _ => styled("text+b", &d.format("%a %b %-d").to_string()),
        };
        items += ns.len();
        let kids: Vec<Value> = ns.iter().map(|n| node_row(cx, n, Ctx { under_day: Some(d), ..c })).collect();
        rows.push(json!({"kind": "section", "id": format!("day:{}", ymd(d)), "title_m": title, "children": kids}));
    }
    flush(&mut empty_run, &mut rows);
    let later = s.nodes_where(
        "n.deleted=0 AND n.is_tag=0 AND (n.status IS NULL OR n.status IN ('todo','doing','waiting')) AND \
         min(coalesce(substr(n.scheduled,1,10),'9'), coalesce(substr(n.due,1,10),'9')) BETWEEN ?1 AND ?2 \
         ORDER BY min(coalesce(n.scheduled,'9'), coalesce(n.due,'9'))",
        &[&ymd(today + Duration::days(7)), &ymd(today + Duration::days(13))],
    )?;
    if !later.is_empty() {
        items += later.len();
        rows.push(json!({"kind": "section", "id": "section:Later", "title_m": styled("text+b", "Later"), "children": later.iter().map(|n| node_row(cx, n, c)).collect::<Vec<_>>()}));
    }
    Ok(json!({"rows": rows, "bar_m": bar("agenda", &[("7 days".into(), Token::Muted), (plural(items, "item", "items"), Token::Muted)])}))
}

// ---- Inbox --------------------------------------------------------------------------------------

/// Inbox (§5.2): root notes with no page or day, oldest first, with when they were captured.
pub fn inbox(cx: &Cx) -> Result<Value> {
    let ns = inbox_nodes(cx.store)?;
    let c = Ctx { captured: true, ..Ctx::default() };
    let mut rows = Vec::new();
    if ns.is_empty() {
        rows.extend(empty("Inbox empty.", &[("A", "captures here")]));
    } else {
        rows.push(json!({"kind": "section", "title_m": styled("text+b", "Inbox"), "count": ns.len(), "note_m": tok(Token::Muted, "oldest first")}));
        rows.extend(ns.iter().map(|n| node_row(cx, n, c)));
    }
    Ok(json!({"rows": rows, "bar_m": bar("inbox", &[(plural(ns.len(), "note", "notes"), Token::Muted)])}))
}

// ---- Tasks --------------------------------------------------------------------------------------

/// Tasks (§5.3): a `thc q` query's results as table rows (`id`, `task`, `due`, `sched`, `pri`,
/// `where`, `by`, each a markup cell, `—` when empty), and the filter row's markup.
pub fn tasks(cx: &Cx, query: &str, keep: &[String]) -> Result<Value> {
    let today = cx.today();
    let (mut ns, error) = match cx.store.query(query, today, 500) {
        Ok(ns) => (ns, None),
        Err(e) => (vec![], Some(format!("{e:#}"))),
    };
    // Rows done here stay where they were until you leave (navigation.md §8): the same query
    // with any status gives every row its place; the query's own rows and the kept ones show.
    let missing = keep.iter().any(|k| !ns.iter().any(|n| &n.id == k));
    if missing && error.is_none() && query.contains("status:open") {
        let shown: HashSet<String> = ns.iter().map(|n| n.id.clone()).chain(keep.iter().cloned()).collect();
        ns = cx.store.query(&query.replace("status:open", "status:any"), today, 500)?.into_iter().filter(|n| shown.contains(&n.id)).collect();
    }
    let dash = || tok(Token::Muted, "—");
    let date = |raw: Option<&String>, due: bool, open: bool| match raw {
        Some(r) => {
            let (t, k) = present::table_date(today, r, due, open);
            tok(k, &t)
        }
        None => dash(),
    };
    let rows: Vec<Value> = ns
        .iter()
        .map(|n| {
            let text = one_line(cx, n);
            let mut task = text_m(n, &text, "");
            if n.repeat.is_some() {
                task.push_str(&tok(Token::Muted, &format!(" {}", cx.g.repeat)));
            }
            let place = n.parent.as_deref().and_then(|p| cx.store.node(p).ok().flatten()).map(|p| present::place_label(cx.store, cx.g, &p, today));
            json!({
                "id": n.id, "short": cx.store.short(&n.id), "status": status_key(n), "text": text, "task_m": task,
                "due_m": date(n.due.as_ref(), true, n.is_open()), "sched_m": date(n.scheduled.as_ref(), false, n.is_open()),
                "pri_m": match n.priority.as_deref() { Some("high") => styled("text+b", "!high"), Some(p) => tok(Token::Muted, &format!("!{p}")), None => dash() },
                "where_m": place.map(|p| tok(Token::Muted, &p)).unwrap_or_else(dash),
                "by_m": agent_name(&n.created_by).map(|a| tok(Token::Agent, &format!("{}{}{a}", cx.g.agent, cx.g.agent_sep))).unwrap_or_else(dash),
            })
        })
        .collect();
    // `status:` muted, `open` in text: the filter as the query reads (§5.3).
    let filter_m: String = query
        .split(' ')
        .map(|w| match w.split_once(':') {
            Some((k, v)) => format!("{}{}", tok(Token::Muted, &format!("{k}:")), tok(Token::Text, v)),
            None => tok(Token::Text, w),
        })
        .collect::<Vec<_>>()
        .join(" ");
    let count = plural(rows.len(), "task", "tasks");
    Ok(json!({
        "rows": rows, "query": query, "filter_m": filter_m, "count": count,
        "error_m": error.map(|e| tok(Token::Overdue, &e)).unwrap_or_default(),
        "bar_m": bar("tasks", &[(count.clone(), Token::Muted)]),
    }))
}

// ---- The sidebar ----------------------------------------------------------------------------------

/// The sidebar's panel headers (sidebar.md §4.1), top to bottom, from the stack in the screen's
/// store: fold glyph, kind glyph (`¶` `§` `≡`, `#` in `tag`), the title (bold for the newest),
/// then on the right `◆ claude` for an agent's open, the folded count (`3 open`, `12 notes`, a
/// list's row count), `pinned` and `×`. A row's `id` is the panel's key.
pub fn sidebar(cx: &Cx, stack: &crate::sidebar::Stack, active: &str, height: usize, width: usize) -> Result<Value> {
    // The text's width in a panel: the column's (a third, 40–64) or the drawer's (60), less the
    // body's padding, so a long line counts the rows it wraps to.
    let text_w = if width >= 120 { (width / 3).clamp(40, 64) } else { 60 }.saturating_sub(5).max(10);
    use crate::sidebar::Kind;
    let g = cx.g;
    let today = cx.today();
    let newest = stack.open.iter().position(|p| !p.pinned);
    let mut rows = Vec::new();
    let (mut show, mut lists, mut naturals) = (serde_json::Map::new(), serde_json::Map::new(), Vec::new());
    for (i, p) in stack.open.iter().enumerate() {
        let (glyph, title, count) = match p.kind {
            Kind::Page => match cx.store.node(&p.target)? {
                Some(n) => {
                    let open = open_under(cx.store, &n.id)?;
                    let notes = cx.store.children(&n.id)?.len();
                    (tok(Token::Muted, g.page), n.label(), if open > 0 { format!("{open} open") } else { plural(notes, "note", "notes") })
                }
                None => (tok(Token::Muted, g.page), format!("{} (deleted)", p.target), String::new()),
            },
            Kind::Day => {
                let d = if p.target == "today" { Some(today) } else { NaiveDate::parse_from_str(&p.target, "%Y-%m-%d").ok() };
                let d = d.unwrap_or(today);
                let entries = cx.store.journal_entries(&d.format("%Y-%m-%d").to_string())?;
                let open = entries.iter().filter(|n| n.is_open()).count();
                let title = d.format("%a %d %b").to_string();
                (tok(Token::Muted, g.journal), title, if open > 0 { format!("{open} open") } else { plural(entries.len(), "note", "notes") })
            }
            Kind::View => {
                let n = match p.target.as_str() {
                    "inbox" => inbox_nodes(cx.store)?.len(),
                    "today" => {
                        let s = thc_core::today::sections(cx.store, today)?;
                        s.overdue.len() + s.today.len() + s.doing.len() + s.next.len()
                    }
                    "tasks" => tasks(cx, "status:open", &[])?["rows"].as_array().map_or(0, Vec::len),
                    _ => 0,
                };
                let mut name = p.target.clone();
                if let Some(c) = name.get_mut(0..1) {
                    c.make_ascii_uppercase();
                }
                (tok(Token::Muted, g.query), name, n.to_string())
            }
            Kind::Query => {
                let n = tasks(cx, &p.target, &[])?["rows"].as_array().map_or(0, Vec::len);
                match p.target.strip_prefix('#') {
                    Some(tag) => (tok(Token::Tag, "#"), tag.to_string(), n.to_string()),
                    None => (tok(Token::Muted, g.query), p.target.clone(), n.to_string()),
                }
            }
        };
        let today_m = match p.kind {
            Kind::Day if p.target == "today" || p.target == today.format("%Y-%m-%d").to_string() => tok(Token::Today, &format!(" {} today", g.sep)),
            _ => String::new(),
        };
        let mut right = Vec::new();
        if let Some(by) = &p.opened_by {
            right.push(tok(Token::Agent, &format!("{}{}{}", g.agent, g.agent_sep, by.trim_start_matches("agent:"))));
        }
        if p.folded && !count.is_empty() {
            right.push(tok(Token::Muted, &count));
        }
        if p.pinned {
            right.push(tok(Token::Muted, "pinned"));
        }
        right.push(tok(Token::Muted, g.close));
        let title_m = if Some(i) == newest { styled("text+b", &title) } else { tok(Token::Text, &title) };
        let fold_m = tok(Token::Muted, if p.folded { g.fold_closed } else { g.fold_open });
        // The body (§4.2): a page or a day as its document's text, a list panel as its rows.
        let (doc, natural) = match p.kind {
            Kind::Page | Kind::Day if !p.folded => {
                let key = match p.kind {
                    Kind::Page => format!("page/{}", p.target),
                    _ => format!("day/{}", if p.target == "today" { today.format("%Y-%m-%d").to_string() } else { p.target.clone() }),
                };
                let lines = doc_lines(cx, &key, text_w);
                (key, lines)
            }
            Kind::View | Kind::Query if !p.folded => {
                let rows_of = list_panel_rows(cx, p)?;
                let n = count_rows(&rows_of);
                lists.insert(p.id(), Value::Array(rows_of));
                (String::new(), n.max(1))
            }
            _ => (String::new(), 0),
        };
        show.insert(p.id(), json!({"doc": !doc.is_empty(), "list": lists.contains_key(&p.id())}));
        naturals.push((p.id(), natural));
        rows.push(json!({
            "id": p.id(), "at": i, "key": p.key(), "kind": "panel", "panel": serde_json::to_value(p.kind)?, "ref": p.target, "folded": p.folded,
            "fold_m": fold_m, "glyph_m": glyph, "title_m": format!("{title_m}{today_m}"), "right_m": right.join("  "),
            "header_m": format!("{fold_m} {glyph} {title_m}{today_m}\t{} ", right.join("  ")),
            "doc": doc,
        }));
    }
    // Sharing the height (§6.3): each panel a header and its body, a blank row between them.
    let sizes = share_height(&naturals, active, height);
    for (row, (body, more)) in rows.iter_mut().zip(sizes) {
        row["h"] = body.into();
        row["total"] = (body + 1).into();
        row["more_m"] = if more > 0 { tok(Token::Muted, &format!("{} {more} more", g.more)) } else { String::new() }.into();
    }
    Ok(json!({"rows": rows, "count": stack.open.len(), "show": show, "lists": lists}))
}

/// How many rows a page or day takes (its text as an editor has it, each line wrapped at
/// `width` cells).
fn doc_lines(cx: &Cx, key: &str, width: usize) -> usize {
    let root = match crate::docs::Doc::parse(key) {
        Some(crate::docs::Doc::Page(id)) => Some(id),
        Some(crate::docs::Doc::Day(d)) => cx.store.journal_node(&d.format("%Y-%m-%d").to_string()).ok().flatten(),
        None => None,
    };
    let rows = |text: &str| -> usize { text.lines().map(|l| unicode_width::UnicodeWidthStr::width(l).div_ceil(width.max(1)).max(1)).sum() };
    root.and_then(|r| thc_core::edit::render(cx.store, &r).ok()).map_or(1, |r| rows(&crate::docs::shown(&r.text)).max(1))
}

/// A list panel's rows (§8.2): the view's sections and rows as in the main view, or a query's
/// rows, all as node rows (`text_m`, `meta_m`).
fn list_panel_rows(cx: &Cx, p: &crate::sidebar::Panel) -> Result<Vec<Value>> {
    let v = match (p.kind, p.target.as_str()) {
        (crate::sidebar::Kind::View, "today") => today(cx)?,
        (crate::sidebar::Kind::View, "inbox") => inbox(cx)?,
        (crate::sidebar::Kind::View, "log") => json!({"rows": []}),
        (crate::sidebar::Kind::View, _) => tasks(cx, "status:open", &[])?,
        (_, q) => tasks(cx, q, &[])?,
    };
    let mut rows = v["rows"].as_array().cloned().unwrap_or_default();
    // Tasks rows are table rows: as node rows.
    for r in rows.iter_mut() {
        if r.get("text_m").is_none() && r.get("task_m").is_some() {
            r["text_m"] = r["task_m"].clone();
            r["meta_m"] = r["due_m"].clone();
        }
    }
    if rows.is_empty() {
        rows.push(json!({"kind": "empty", "m": tok(Token::Muted, "nothing here")}));
    }
    Ok(rows)
}

/// Rows in a tree, children included.
fn count_rows(rows: &[Value]) -> usize {
    rows.iter().map(|r| 1 + r["children"].as_array().map_or(0, |c| count_rows(c))).sum()
}

/// §6.3: each panel 1 header row, its body, 1 blank row between panels, in `height - 3` rows
/// (under the tab rule, above the bar). If everything fits, each body is its natural height.
/// Else the bodies other than the active one are capped at 6 rows, and the active one takes
/// what's left, at least 3. What's still too tall folds for room from the bottom (body 0). Each
/// body's rows, and how many of its rows don't show (`↓ N more`).
pub fn share_height(naturals: &[(String, usize)], active: &str, height: usize) -> Vec<(usize, usize)> {
    let avail = height.saturating_sub(3);
    let n = naturals.len();
    if n == 0 {
        return vec![];
    }
    let frame = n + n.saturating_sub(1);
    let total: usize = naturals.iter().map(|(_, h)| h).sum::<usize>() + frame;
    if total <= avail {
        return naturals.iter().map(|(_, h)| (*h, 0)).collect();
    }
    let act = naturals.iter().position(|(id, _)| id == active).unwrap_or(0);
    let mut body: Vec<usize> = naturals.iter().enumerate().map(|(i, (_, h))| if i == act { *h } else { (*h).min(6) }).collect();
    let others: usize = body.iter().enumerate().filter(|(i, _)| *i != act).map(|(_, b)| b).sum();
    body[act] = avail.saturating_sub(frame + others).clamp(3.min(naturals[act].1), naturals[act].1);
    // Still too tall: fold for room from the bottom, never the active one.
    let mut i = n;
    while body.iter().sum::<usize>() + frame > avail && i > 0 {
        i -= 1;
        if i != act {
            body[i] = 0;
        }
    }
    body.iter().zip(naturals).map(|(b, (_, h))| (*b, h.saturating_sub(*b))).collect()
}

// ---- Pages and outlines ---------------------------------------------------------------------------

/// Pages (§5.4): a section of pages (`¶ Title`, `N children · N open · ← N`), then the tags
/// (inline, with counts) and the recent journal days. Each page and day row names its document
/// (`doc`: `page/<id>`, `day/<date>`), the host entry its preview and its editor show.
pub fn pages(cx: &Cx) -> Result<Value> {
    let g = cx.g;
    let pages = thc_core::outline::pages(cx.store)?;
    let mut rows = Vec::new();
    rows.push(json!({"kind": "section", "title_m": styled("text+b", "Pages"), "count": pages.len(), "note_m": tok(Token::Muted, "sort: name")}));
    for p in &pages {
        let kids = cx.store.children(&p.id)?.len();
        let open = open_under(cx.store, &p.id)?;
        let back = cx.store.backlinks(&p.id)?.len();
        let mut meta = Vec::new();
        if kids > 0 {
            meta.push(plural(kids, "child", "children"));
        }
        if open > 0 {
            meta.push(format!("{open} open"));
        }
        if back > 0 {
            meta.push(format!("{} {back}", g.backlink));
        }
        rows.push(json!({
            "kind": "page", "open": "page", "doc": format!("page/{}", p.id), "id": p.id, "short": cx.store.short(&p.id), "text": p.title,
            "title_m": styled("text+b", &format!("{} {}", g.page, p.title)),
            "meta_m": tok(Token::Muted, &meta.join(&format!(" {} ", g.sep))),
        }));
    }
    if pages.is_empty() {
        rows.extend(empty("No pages yet.", &[("", "Link one into existence: [[Page Title]]")]));
    }
    // Tags, inline, by use.
    let mut st = cx.store.conn.prepare(
        "SELECT t.title, count(e.src) FROM nodes t JOIN edges e ON e.dst=t.id AND e.rel='tag' JOIN nodes s ON s.id=e.src AND s.deleted=0 \
         WHERE t.is_tag=1 AND t.deleted=0 GROUP BY t.id ORDER BY count(e.src) DESC, t.title COLLATE NOCASE",
    )?;
    let tags: Vec<(String, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    if !tags.is_empty() {
        rows.push(json!({"kind": "blank"}));
        rows.push(json!({"kind": "section", "title_m": styled("text+b", "Tags"), "count": tags.len()}));
        let line = tags.iter().map(|(t, n)| format!("{} {}", tok(Token::Tag, &format!("#{}", t.to_lowercase())), tok(Token::Muted, &n.to_string()))).collect::<Vec<_>>().join("   ");
        rows.push(json!({"kind": "line", "m": line}));
    }
    // Recent journal days with entries.
    let mut st = cx.store.conn.prepare(
        "SELECT j.journal, count(c.id) FROM nodes j JOIN nodes c ON c.parent=j.id AND c.deleted=0 WHERE j.journal IS NOT NULL AND j.deleted=0 \
         GROUP BY j.id ORDER BY j.journal DESC LIMIT 7",
    )?;
    let days: Vec<(String, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    if !days.is_empty() {
        rows.push(json!({"kind": "blank"}));
        rows.push(json!({"kind": "section", "title_m": styled("text+b", "Journal"), "count": days.len()}));
        for (d, n) in days {
            let Some(date) = DateVal::from_stored(&d).map(|v| v.date()) else { continue };
            let rel = thc_core::dates::relative(date, cx.today());
            rows.push(json!({
                "kind": "day", "open": "day", "doc": format!("day/{d}"), "date": d,
                "title_m": format!("{} {}", tok(Token::Muted, g.journal), tok(Token::Text, &date.format("%a %b %-d").to_string())),
                "meta_m": tok(Token::Muted, &format!("{} {} {rel}", plural(n as usize, "entry", "entries"), g.sep)),
            }));
        }
    }
    Ok(json!({"rows": rows, "bar_m": bar("pages", &[(plural(pages.len(), "page", "pages"), Token::Muted)])}))
}

/// Open tasks anywhere under a node.
fn open_under(store: &Store, id: &str) -> Result<i64> {
    Ok(store.conn.query_row(
        "WITH RECURSIVE sub(id) AS (SELECT id FROM nodes WHERE parent=?1 AND deleted=0 UNION ALL SELECT n.id FROM nodes n JOIN sub ON n.parent=sub.id WHERE n.deleted=0) \
         SELECT count(*) FROM nodes WHERE id IN (SELECT id FROM sub) AND status IN ('todo','doing','waiting')",
        [id],
        |r| r.get(0),
    )?)
}

/// A subtree as tree rows: each node's row with its children under it (`children`).
fn subtree(cx: &Cx, parent: &str, c: Ctx, depth: usize) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for n in cx.store.children(parent)? {
        let mut row = node_row(cx, &n, c);
        if depth < 32 {
            row["children"] = subtree(cx, &n.id, c, depth + 1)?.into();
        }
        out.push(row);
    }
    Ok(out)
}

/// One page's outline (§5.4): its title and counts, its rows as a tree, then `← linked from`.
pub fn outline(cx: &Cx, id: &str) -> Result<Value> {
    let g = cx.g;
    let Some(page) = cx.store.node(id)? else { anyhow::bail!("no node {id}") };
    let c = Ctx { context: true, subject: Some(id), ..Ctx::default() };
    let mut rows = subtree(cx, id, c, 0)?;
    let back = cx.store.backlinks(id)?;
    if !back.is_empty() {
        let linked: Vec<Value> = back.iter().map(|n| node_row(cx, n, Ctx { context: true, ..Ctx::default() })).collect();
        rows.push(json!({"kind": "section", "id": "section:linked", "title": format!("{} linked from", g.backlink), "title_m": styled("muted+b", &format!("{} linked from", g.backlink)), "count": linked.len(), "children": linked}));
    }
    let open = open_under(cx.store, id)?;
    let label = page.label();
    Ok(json!({
        "rows": rows,
        "title_m": styled("text+b", &format!("{} {label}", g.page)),
        "meta_m": tok(Token::Muted, &format!("{} {} {open} open {} {} {}", cx.store.short(id), g.sep, g.sep, g.backlink, back.len())),
        "bar_m": bar(&label, &[(format!("{open} open"), Token::Muted)]),
    }))
}

/// The heading row over a page or a day being written in (the title is the screen's, not the
/// document's text): `¶ Title` or `§ Sunday, October 11 2026`, and how much is open in it. Nothing
/// open (an empty key): an empty heading.
pub fn doc(cx: &Cx, key: &str) -> Result<Value> {
    let g = cx.g;
    let (title, root) = if let Some(id) = key.strip_prefix("page/").filter(|i| !i.is_empty()) {
        let n = cx.store.node(&cx.store.resolve(id)?)?;
        (format!("{} {}", g.page, n.as_ref().map(|n| n.label()).unwrap_or_default()), n.map(|n| n.id))
    } else if let Some(date) = key.strip_prefix("day/").and_then(|d| DateVal::from_stored(d)).map(|v| v.date()) {
        let d = date.format("%Y-%m-%d").to_string();
        (format!("{} {}", g.journal, date.format("%A, %B %-d %Y")), cx.store.journal_node(&d)?)
    } else {
        return Ok(json!({"title_m": "", "meta_m": ""}));
    };
    let open = match &root {
        Some(r) => open_under(cx.store, r)?,
        None => 0,
    };
    Ok(json!({"title_m": styled("text+b", &title), "meta_m": tok(Token::Muted, &format!("{open} open"))}))
}

// ---- Journal ------------------------------------------------------------------------------------

/// A journal day (§5.5): its title, the day strip, its entries as a tree (Completed today on
/// today), and the month's calendar lines.
pub fn journal(cx: &Cx, date: NaiveDate) -> Result<Value> {
    let g = cx.g;
    let today = cx.today();
    let key = date.format("%Y-%m-%d").to_string();
    let c = Ctx { context: true, done_time: true, ..Ctx::default() };
    let jid = cx.store.journal_node(&key)?;
    let mut rows = match &jid {
        Some(j) => subtree(cx, j, Ctx { subject: Some(j), ..c }, 0)?,
        None => vec![],
    };
    if date == today {
        let t1 = (today + Duration::days(1)).format("%Y-%m-%d").to_string();
        let done = cx.store.nodes_where("n.deleted=0 AND n.status='done' AND n.done_at >= ?1 AND n.done_at < ?2 ORDER BY n.done_at", &[&key, &t1])?;
        if !done.is_empty() {
            let kids: Vec<Value> = done.iter().map(|n| node_row(cx, n, Ctx { include_today: true, ..c })).collect();
            rows.push(section("Completed today", Token::Text, kids.len(), kids));
        }
    }
    if rows.is_empty() {
        rows = if date == today {
            empty("Today's page is blank.", &[("a", "write the first line")])
        } else {
            empty(&format!("Nothing written on {}.", date.format("%a %b %-d")), &[("[ ]", "move a day"), ("T", "today")])
        };
    }
    let written = days_written(cx.store)?;
    // The strip (§5.5, row 3): seven days centred on the shown one from column 3, three spaces
    // apart, `•` on days with entries (`accent` on the shown day, which is bold); under it (row
    // 4) a `━` in `accent` under the shown day's label, a cell either side.
    let (mut strip, mut under, mut col) = (Vec::new(), String::new(), STRIP_COL);
    for k in -3..=3 {
        let d = date + Duration::days(k);
        let label = d.format("%a %-d").to_string();
        let w = label.chars().count();
        let dot = if written.contains(&d) { format!(" {}", if d == date { tok(Token::Accent, g.live) } else { tok(Token::Muted, g.live) }) } else { String::new() };
        if d == date {
            under = format!("{}{}", " ".repeat(col - 1), tok(Token::Accent, &"━".repeat(w + 2)));
        }
        strip.push(format!("{}{dot}", if d == date { styled("text+b", &label) } else { tok(Token::Muted, &label) }));
        col += w + if dot.is_empty() { 0 } else { 2 } + 3;
    }
    // The month, then a legend and the shown day in short (§5.5).
    let mut cal = calendar(g, date, today, &written);
    let entries = cx.store.journal_entries(&key)?;
    let open = entries.iter().filter(|n| n.is_open()).count();
    cal.push(json!({"m": ""}));
    cal.push(json!({"m": format!("{} {}   {} {}", tok(Token::Muted, g.live), tok(Token::Muted, "written"), tok(Token::Today, "[ ]"), tok(Token::Muted, "today"))}));
    cal.push(json!({"m": ""}));
    cal.push(json!({"m": format!("{}{}", styled("text+b", &date.format("%a %b %-d").to_string()), tok(Token::Muted, &format!(" {} {} {} {open} open", g.sep, plural(entries.len(), "entry", "entries"), g.sep)))}));
    let rel = if date == today { format!(" {}", tok(Token::Today, "· today")) } else { tok(Token::Muted, &format!(" · {}", thc_core::dates::relative(date, today))) };
    Ok(json!({
        "rows": rows,
        "title_m": format!("{}{rel}", styled("text+b", &format!("{} {}", g.journal, date.format("%A, %B %-d %Y")))),
        "strip_m": format!("{}{}", " ".repeat(STRIP_COL), strip.join("   ")),
        "under_m": under,
        "calendar": cal,
        "date": key,
        "prev": (date - Duration::days(1)).format("%Y-%m-%d").to_string(),
        "next": (date + Duration::days(1)).format("%Y-%m-%d").to_string(),
        "bar_m": bar("journal", &[(plural(cx.store.journal_entries(&key)?.len(), "entry", "entries"), Token::Muted)]),
    }))
}

fn days_written(store: &Store) -> Result<HashSet<NaiveDate>> {
    let mut st = store.conn.prepare("SELECT j.journal FROM nodes j WHERE j.journal IS NOT NULL AND j.deleted=0 AND EXISTS (SELECT 1 FROM nodes c WHERE c.parent=j.id AND c.deleted=0)")?;
    let days: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(days.iter().filter_map(|d| DateVal::from_stored(d).map(|v| v.date())).collect())
}

/// The column the Journal's day strip starts at (§5.5).
const STRIP_COL: usize = 3;

/// The month around `date` (§5.5), as the TUI draws it (thc-tui ui.rs `draw_calendar`): its
/// name, a blank, the weekday row in `muted`, and a 21-cell line per week, each day two wide
/// (right-aligned) and a cell for `•` after days with entries (`accent` on the shown day). Days
/// with entries are `text`, the rest `muted`, the shown day bold and underlined, and today
/// `[3•]` in `today`, its brackets taking the cells beside it.
fn calendar(g: &Glyphs, date: NaiveDate, today: NaiveDate, written: &HashSet<NaiveDate>) -> Vec<Value> {
    let first = date.with_day(1).unwrap_or(date);
    let mut lines = vec![json!({"m": styled("text+b", &first.format("%B %Y").to_string())}), json!({"m": ""}), json!({"m": tok(Token::Muted, "Mo Tu We Th Fr Sa Su")})];
    let blank = || vec![(" ".to_string(), String::new()); 21];
    let mut week = blank();
    let flush = |week: &mut Vec<(String, String)>, lines: &mut Vec<Value>| {
        let m: String = week.iter().map(|(c, st)| if st.is_empty() { c.clone() } else { styled(st, c) }).collect();
        lines.push(json!({"m": m.trim_end()}));
        *week = blank();
    };
    let mut d = first;
    while d.month() == first.month() {
        let col = d.weekday().num_days_from_monday() as usize * 3;
        let has = written.contains(&d);
        let num = if d == today {
            "today+b"
        } else if d == date {
            "text+b+u"
        } else if has {
            "text"
        } else {
            "muted"
        };
        for (i, ch) in format!("{:>2}", d.day()).chars().enumerate() {
            if ch != ' ' {
                week[col + i] = (ch.to_string(), num.into());
            }
        }
        if has {
            week[col + 2] = (g.live.to_string(), if d == date { "accent" } else { "muted" }.into());
        }
        if d == today {
            week[if d.day() < 10 { col } else { col.saturating_sub(1) }] = ("[".into(), "today".into());
            if col + 3 < 21 {
                week[col + 3] = ("]".into(), "today".into());
            }
        }
        if d.weekday() == chrono::Weekday::Sun {
            flush(&mut week, &mut lines);
        }
        d += Duration::days(1);
    }
    if week.iter().any(|(c, _)| c != " ") {
        flush(&mut week, &mut lines);
    }
    lines
}

// ---- Search -------------------------------------------------------------------------------------

/// Search (§5.6): sections Pages, Tags and Nodes, the words found in bold on `accent-tint`.
pub fn search(cx: &Cx, query: &str) -> Result<Value> {
    let q = query.trim();
    let mut rows = Vec::new();
    if q.is_empty() {
        rows.extend(empty("Search every note.", &[("", "type, then Enter")]));
        return Ok(json!({"rows": rows, "count_m": "", "bar_m": bar("search", &[])}));
    }
    let found = cx.store.search(q, 100)?;
    let c = Ctx { context: true, include_today: true, terms: q, ..Ctx::default() };
    let (mut pages, mut tags, mut nodes) = (vec![], vec![], vec![]);
    for n in &found {
        if n.is_tag {
            let mut r = node_row(cx, n, c);
            r["status"] = "tag".into();
            r["text_m"] = markup::text(&format!("#{}", n.label().to_lowercase()), false, q).into();
            tags.push(r);
        } else if n.parent.is_none() && n.title.is_some() && n.journal.is_none() {
            let mut r = node_row(cx, n, c);
            r["status"] = "page".into();
            r["text_m"] = format!("<text+b>{}</>", markup::text(&n.label(), false, q)).into();
            pages.push(r);
        } else {
            nodes.push(node_row(cx, n, c));
        }
    }
    let counts = format!("{} {} {} {} {}", plural(nodes.len(), "node", "nodes"), cx.g.sep, plural(pages.len(), "page", "pages"), cx.g.sep, plural(tags.len(), "tag", "tags"));
    for (title, list) in [("Pages", pages), ("Tags", tags), ("Nodes", nodes)] {
        if !list.is_empty() {
            rows.push(section(title, Token::Text, list.len(), list));
        }
    }
    if rows.is_empty() {
        rows.extend(empty(&format!("No matches for \"{q}\"."), &[("", "add is:deleted to include deleted")]));
    }
    Ok(json!({"rows": rows, "count_m": tok(Token::Muted, &counts), "query": q, "bar_m": bar("search", &[(counts.clone(), Token::Muted)])}))
}

/// Search's preview (§5.6, 120+): the selected match among its siblings, under where it lives,
/// the words found in bold on `accent-tint`, the match itself marked. A page or a tag shows its
/// own children instead.
pub fn context(cx: &Cx, id: &str, query: &str) -> Result<Value> {
    let Some(n) = cx.store.node(id).ok().flatten() else { return Ok(json!({"lines": []})) };
    let g = cx.g;
    let c = Ctx { terms: query, ..Ctx::default() };
    let (head, sibs) = match &n.parent {
        Some(p) if n.title.is_none() => {
            let parent = cx.store.must_node(p)?;
            let head = match parent.journal.as_deref().and_then(DateVal::from_stored) {
                Some(d) => format!("{} {}", g.journal, d.date().format("%a %b %-d")),
                None if parent.title.is_some() => format!("{} {}", g.page, parent.label()),
                None => one_line(cx, &parent),
            };
            (head, cx.store.children(p)?)
        }
        _ => (format!("{} {}", g.page, n.label()), cx.store.children(&n.id)?),
    };
    let mut lines = vec![json!({"k": "line", "m": styled("text+b", &head)}), json!({"k": "blank"})];
    for s in &sibs {
        let row = node_row(cx, s, c);
        let here = s.id == n.id;
        lines.push(json!({
            "k": "sub", "status": row["status"], "meta_m": row["meta_m"],
            "text_m": if here { format!("<accent+b>{}</> {}", g.cursor, row["text_m"].as_str().unwrap_or("")) } else { format!("  {}", row["text_m"].as_str().unwrap_or("")) },
        }));
    }
    if sibs.is_empty() {
        lines.push(json!({"k": "line", "m": tok(Token::Muted, "nothing under it")}));
    }
    Ok(json!({"lines": lines}))
}

/// The move picker's places (§7.2): every page, best match first for what's typed (fuzzy: its
/// letters in order, tighter and earlier is better), the inbox last.
pub fn dest(cx: &Cx, query: &str) -> Result<Value> {
    let g = cx.g;
    let q: Vec<char> = query.trim().to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    // A subsequence match's cost: the span it takes and where it starts; None if it doesn't match.
    let score = |title: &str| -> Option<(usize, usize)> {
        if q.is_empty() {
            return Some((0, 0));
        }
        let t: Vec<char> = title.to_lowercase().chars().collect();
        let mut best: Option<(usize, usize)> = None;
        for start in 0..t.len() {
            if t[start] != q[0] {
                continue;
            }
            let (mut i, mut j) = (start, 0);
            while i < t.len() && j < q.len() {
                if t[i] == q[j] {
                    j += 1;
                }
                i += 1;
            }
            if j == q.len() {
                let cost = (i - start, start);
                best = Some(best.map_or(cost, |b: (usize, usize)| b.min(cost)));
            }
        }
        best
    };
    let mut found: Vec<((usize, usize), Value)> = thc_core::outline::pages(cx.store)?
        .into_iter()
        .filter_map(|p| {
            let s = score(&p.title)?;
            Some((s, json!({"id": p.id, "short": cx.store.short(&p.id), "label_m": format!("{} {}", g.page, markup::text(&p.title, false, query)), "where": "page"})))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    let mut rows: Vec<Value> = found.into_iter().map(|(_, r)| r).collect();
    if score("inbox").is_some() {
        rows.push(json!({"id": "", "short": "", "label_m": tok(Token::Muted, "inbox"), "where": "inbox"}));
    }
    Ok(json!({"rows": rows}))
}

// ---- Log ----------------------------------------------------------------------------------------

/// The Log (§5.7): the last day's transactions, newest first. Each is a row (`time`, `who_m`,
/// `tx`, `summary_m`) followed by a `detail` row (what it did, in words), and carries its
/// detail-pane lines (`lines`). `agents_only` keeps agents' transactions (`L`, review).
pub fn log(cx: &Cx, agents_only: bool) -> Result<Value> {
    let g = cx.g;
    let since = cx.now.and_utc().timestamp_millis() - 24 * 3600 * 1000;
    let h = cx.store.history_where("ms >= ?1 ORDER BY okey DESC LIMIT 400", &[&since])?;
    let mut txs: Vec<(String, Vec<HistoryEntry>)> = Vec::new();
    for e in h {
        match txs.last_mut() {
            Some((tx, v)) if *tx == e.tx => v.push(e),
            _ => txs.push((e.tx.clone(), vec![e])),
        }
    }
    let mut rows = Vec::new();
    let mut agents = BTreeMap::new();
    for (tx, entries) in &txs {
        let e = &entries[0];
        let agent = agent_name(&e.actor);
        if agents_only && agent.is_none() {
            continue;
        }
        if let Some(a) = agent {
            *agents.entry(a.to_string()).or_insert(0) += 1;
        }
        let who = match agent {
            Some(a) => present::truncate_str(&format!("{}{}{a}", g.agent, g.agent_sep), 8, g.ellipsis),
            None => "you".into(),
        };
        let Some((verb, target, n)) = present::tx_main(entries) else { continue };
        let label = cx.store.node(&target.entity).ok().flatten().map(|t| match t.journal.as_deref().and_then(DateVal::from_stored) {
            Some(d) => format!("{} {}", g.journal, d.date().format("%b %-d")),
            None => one_line(cx, &t),
        });
        let mut summary = tok(Token::Text, &format!("{verb} "));
        if target.entity.len() == 12 {
            summary.push_str(&tok(Token::Muted, &format!("{} ", cx.store.short(&target.entity))));
        }
        summary.push_str(&tok(Token::Text, &label.unwrap_or_default()));
        if n > 1 {
            summary.push_str(&tok(Token::Muted, &format!("  +{} more", n - 1)));
        }
        let suffix = tx[tx.len().saturating_sub(6)..].to_string();
        let detail = present::tx_detail(cx.store, entries, g, cx.today());
        let when = local(e.ms).map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
        rows.push(json!({
            "kind": if agent.is_some() { "agent" } else { "you" }, "id": tx, "tx": suffix, "time": when, "who": who,
            "summary_m": summary, "detail": detail, "node": target.entity,
        }));
    }
    let shown = rows.len();
    if rows.is_empty() {
        let first = if agents_only { "No agent has changed anything in the last 24h." } else { "No changes in the last 24h." };
        rows.extend(empty(first, &[("", "thc log --since 7d")]));
    }
    let by = agents.iter().map(|(a, n)| format!("{n} by {a}")).collect::<Vec<_>>();
    let mut counts = vec![(plural(shown, "tx", "tx"), Token::Muted)];
    counts.extend(by.into_iter().map(|b| (b, Token::Muted)));
    let (who, other) = if agents_only { ("agents", "everyone") } else { ("everyone", "agents") };
    Ok(json!({"rows": rows, "who": who, "other": other, "bar_m": bar("log", &counts), "title_m": format!("{}{}", styled("text+b", "Log"), tok(Token::Muted, &counts.iter().map(|(c, _)| format!(" · {c}")).collect::<String>()))}))
}

/// A transaction's detail pane (§5.7): its id, who, how, where and when, one op per line, and
/// how to undo it.
/// One transaction's detail pane (§5.7), for the Log's selected row.
pub fn tx(cx: &Cx, tx: &str) -> Result<Value> {
    let entries = cx.store.history_where("tx = ?1 ORDER BY okey DESC", &[&tx])?;
    if entries.is_empty() {
        return Ok(json!({"lines": []}));
    }
    Ok(json!({"lines": tx_lines(cx, tx, &entries)}))
}

fn tx_lines(cx: &Cx, tx: &str, entries: &[HistoryEntry]) -> Vec<Value> {
    let e = &entries[0];
    let field = |k: &str, v: String| json!({"k": "field", "key": k, "m": tok(Token::Text, &v)});
    let suffix = &tx[tx.len().saturating_sub(6)..];
    let mut lines = vec![
        json!({"k": "line", "m": format!("{} {}", styled("text+b", &format!("tx {suffix}")), tok(Token::Muted, tx))}),
        json!({"k": "blank"}),
        field("actor", agent_name(&e.actor).map(|a| format!("{}{}{a}", cx.g.agent, cx.g.agent_sep)).unwrap_or_else(|| e.actor.clone())),
        field("via", e.via.clone()),
        field("device", e.dev.clone()),
        field("at", local(e.ms).map(|t| t.format("%a %b %-d %H:%M:%S").to_string()).unwrap_or_default()),
        json!({"k": "blank"}),
        json!({"k": "line", "m": styled("text+b", "ops")}),
    ];
    for e in entries.iter().rev() {
        lines.push(json!({"k": "line", "m": format!("{}  {}", tok(Token::Text, &e.op), tok(Token::Muted, &cx.store.short(&e.entity)))}));
    }
    lines.push(json!({"k": "blank"}));
    lines.push(json!({"k": "line", "m": format!("{}  {}", tok(Token::Text, "u"), tok(Token::Muted, &format!("undo this tx {} adds {} inverse events", cx.g.sep, entries.len())))}));
    lines.push(json!({"k": "line", "m": tok(Token::Muted, &format!("thc undo --tx {suffix}"))}));
    lines
}

// ---- Detail -------------------------------------------------------------------------------------

/// The detail of one node (§5.8).
pub fn detail(cx: &Cx, id: &str) -> Result<Value> {
    // A section row or nothing selected: an empty pane.
    let Some(n) = cx.store.node(id).ok().flatten() else { return Ok(json!({"lines": [], "bar_m": bar("detail", &[])})) };
    Ok(json!({"lines": detail_lines(cx, &n)?, "bar_m": bar("detail", &[(cx.store.short(id), Token::Muted)])}))
}

/// The detail pane as lines (`k`: `title`, `muted`, `field`, `section`, `sub`, `hist`, `line`,
/// `blank`): title, where it is, its fields (keys 11 wide), subtasks, history and keys.
fn detail_lines(cx: &Cx, n: &Node) -> Result<Vec<Value>> {
    let g = cx.g;
    let today = cx.today();
    let mut lines = Vec::new();
    let text = one_line(cx, n);
    lines.push(json!({"k": "title", "status": status_key(n), "text_m": styled("text+b", &text)}));
    // Where it is: its ancestors, then its short id.
    let mut crumbs = vec![cx.store.short(&n.id)];
    let mut p = n.parent.clone();
    while let Some(pid) = p {
        let Some(x) = cx.store.node(&pid)? else { break };
        crumbs.push(present::place_label(cx.store, g, &x, today));
        p = x.parent.clone();
        if crumbs.len() > 32 {
            break;
        }
    }
    crumbs.reverse();
    lines.push(json!({"k": "muted", "m": tok(Token::Muted, &crumbs.join(&format!(" {} ", "›")))}));
    lines.push(json!({"k": "blank"}));
    let field = |k: &str, m: String| json!({"k": "field", "key": k, "m": m});
    let long = |raw: &str| -> Option<(String, NaiveDate)> {
        let dv = DateVal::from_stored(raw)?;
        let t = dv.time().map(|t| format!(" {}", t.format("%H:%M"))).unwrap_or_default();
        Some((format!("{}{t}", dv.date().format("%a %b %-d")), dv.date()))
    };
    let rel = |d: NaiveDate| {
        let k = (d - today).num_days();
        if (0..7).contains(&k) { thc_core::dates::relative(d, today) } else if k > 0 { format!("in {k}d") } else { thc_core::dates::relative(d, today) }
    };
    if let Some(s) = &n.status {
        lines.push(field("status", tok(if s == "done" { Token::Done } else { Token::Text }, s)));
    }
    if let Some((t, d)) = n.scheduled.as_deref().and_then(long) {
        lines.push(field("scheduled", format!("{}{}", tok(Token::Text, &t), tok(Token::Muted, &format!(" {} {}", g.sep, rel(d))))));
    }
    if let Some((t, d)) = n.due.as_deref().and_then(long) {
        let tone = if n.is_open() && d < today { Token::Overdue } else { Token::Text };
        lines.push(field("due", format!("{}{}", tok(tone, &t), tok(Token::Muted, &format!(" {} {}", g.sep, rel(d))))));
    }
    if let Some(p) = &n.priority {
        lines.push(field("priority", if p == "high" { styled("text+b", "!high") } else { tok(Token::Muted, &format!("!{p}")) }));
    }
    if let Some(r) = n.repeat.as_ref().and_then(|r| r.get("text")).and_then(|t| t.as_str()) {
        lines.push(field("repeat", tok(Token::Text, &format!("{} {r}", g.repeat))));
    }
    for a in cx.store.alerts_where("node=?1 AND deleted=0 AND state IN ('pending','snoozed')", &[&n.id])? {
        let at = a.fire_at.as_deref().unwrap_or("").replace('T', " ");
        lines.push(field("alert", tok(Token::Text, &format!("{} {at} {} {}", g.alert, g.sep, a.state))));
    }
    let tags = cx.store.tags_of(&n.id)?;
    if !tags.is_empty() {
        lines.push(field("tags", tags.iter().map(|t| tok(Token::Tag, &format!("#{}", t.to_lowercase()))).collect::<Vec<_>>().join(" ")));
    }
    let created = local(n.created_ms).map(|t| t.format("%b %-d %H:%M").to_string()).unwrap_or_default();
    let by = match agent_name(&n.created_by) {
        Some(a) => tok(Token::Agent, &format!("{}{}{a}", g.agent, g.agent_sep)),
        None => tok(Token::Text, "you"),
    };
    lines.push(field("created", format!("{}{}{by}", tok(Token::Text, &created), tok(Token::Muted, &format!(" {} ", g.sep)))));
    // Subtasks: the tasks directly under it.
    let kids: Vec<Node> = cx.store.children(&n.id)?.into_iter().filter(|k| k.status.is_some()).collect();
    if !kids.is_empty() {
        let done = kids.iter().filter(|k| closed(k)).count();
        lines.push(json!({"k": "blank"}));
        lines.push(json!({"k": "section", "m": format!("{}  {}", styled("text+b", "Subtasks"), tok(Token::Muted, &format!("{done}/{}", kids.len())))}));
        for k in &kids {
            let t = one_line(cx, k);
            let m = metas(cx, k, Ctx::default());
            lines.push(json!({"k": "sub", "id": k.id, "status": status_key(k), "text_m": text_m(k, &t, ""), "meta_m": m.last().cloned().unwrap_or_default()}));
        }
    }
    // History: its last few transactions, in words.
    let h = cx.store.history(&n.id, 50)?;
    let mut seen = Vec::new();
    for e in &h {
        if !seen.contains(&e.tx) {
            seen.push(e.tx.clone());
        }
    }
    if !seen.is_empty() {
        lines.push(json!({"k": "blank"}));
        lines.push(json!({"k": "section", "m": styled("text+b", "History")}));
        for tx in seen.iter().take(3) {
            let e = h.iter().find(|e| &e.tx == tx).expect("seen");
            let what = thc_core::review::item(cx.store, tx, 0)
                .ok()
                .and_then(|it| it.changes.iter().find(|c| c.node == n.id).map(|c| present::change_words(cx.store, c, g, today)))
                .unwrap_or_default();
            let who = match agent_name(&e.actor) {
                Some(a) => tok(Token::Agent, &format!("{}{}{a}", g.agent, g.agent_sep)),
                None => tok(Token::Muted, "you"),
            };
            let when = local(e.ms).map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
            lines.push(json!({"k": "hist", "time": when, "who_m": who, "m": tok(Token::Muted, &what)}));
        }
    }
    lines.push(json!({"k": "blank"}));
    lines.push(json!({"k": "line", "m": hints(&[("x", "done"), ("X", "reopen"), ("enter", "open")], "  ")}));
    Ok(lines)
}
