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
            items.extend(present::date_item(g, today, s, false, open, alert && (timed || n.due.is_none()), None, both).map(spec));
        }
        if let Some(d) = &n.due {
            items.extend(present::date_item(g, today, d, true, n.is_open(), alert && !timed && n.scheduled.is_none(), None, both).map(spec));
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

/// The header's data (§3.1): the inbox count for its tab title, and the date.
pub fn frame(cx: &Cx) -> Result<Value> {
    let inbox = inbox_nodes(cx.store)?.len();
    Ok(json!({"inbox": if inbox > 0 { inbox.to_string() } else { String::new() }, "date": cx.now.format("%a %b %-d").to_string()}))
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
pub fn tasks(cx: &Cx, query: &str) -> Result<Value> {
    let today = cx.today();
    let (ns, error) = match cx.store.query(query, today, 500) {
        Ok(ns) => (ns, None),
        Err(e) => (vec![], Some(format!("{e:#}"))),
    };
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

/// The bar of a page or day being written in: what it is and how much is open in it.
pub fn doc(cx: &Cx, key: &str) -> Result<Value> {
    let g = cx.g;
    let (name, root) = if let Some(id) = key.strip_prefix("page/") {
        let n = cx.store.node(&cx.store.resolve(id)?)?;
        (format!("{} {}", g.page, n.as_ref().map(|n| n.label()).unwrap_or_default()), n.map(|n| n.id))
    } else {
        let d = key.strip_prefix("day/").unwrap_or("");
        let date = DateVal::from_stored(d).map(|v| v.date()).unwrap_or(cx.today());
        (format!("{} {}", g.journal, date.format("%a %b %-d")), cx.store.journal_node(d)?)
    };
    let open = match &root {
        Some(r) => open_under(cx.store, r)?,
        None => 0,
    };
    Ok(json!({"bar_m": bar(&name, &[(format!("{open} open"), Token::Muted), ("writing".into(), Token::Muted)])}))
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
    // The strip: seven days centred on the shown one, `•` on days with entries.
    let strip: Vec<String> = (-3..=3)
        .map(|k| {
            let d = date + Duration::days(k);
            let label = d.format("%a %-d").to_string();
            let dot = if written.contains(&d) { format!(" {}", if d == date { tok(Token::Accent, g.live) } else { tok(Token::Muted, g.live) }) } else { String::new() };
            let l = if d == date { styled("text+b", &label) } else { tok(Token::Muted, &label) };
            format!("{l}{dot}")
        })
        .collect();
    let rel = if date == today { format!(" {}", tok(Token::Today, "· today")) } else { tok(Token::Muted, &format!(" · {}", thc_core::dates::relative(date, today))) };
    Ok(json!({
        "rows": rows,
        "title_m": format!("{}{rel}", styled("text+b", &format!("{} {}", g.journal, date.format("%A, %B %-d %Y")))),
        "strip_m": strip.join("   "),
        "calendar": calendar(g, date, today, &written),
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

/// The month around `date` (§5.5): its name, the weekday row, and a line per week, each day two
/// wide with `•` after days with entries and today as `[3•]` in `today`.
fn calendar(g: &Glyphs, date: NaiveDate, today: NaiveDate, written: &HashSet<NaiveDate>) -> Vec<Value> {
    let first = date.with_day(1).unwrap_or(date);
    let mut lines = vec![json!({"m": styled("text+b", &first.format("%B %Y").to_string())}), json!({"m": tok(Token::Muted, "Mo Tu We Th Fr Sa Su")})];
    let mut line = "   ".repeat(first.weekday().num_days_from_monday() as usize);
    let mut d = first;
    while d.month() == first.month() {
        let mark = if written.contains(&d) { g.live } else { " " };
        let cell = format!("{:>2}{mark}", d.day());
        line.push_str(&if d == today { tok(Token::Today, &cell) } else if d == date { styled("text+b", &cell) } else { tok(Token::Text, &cell) });
        if d.weekday() == chrono::Weekday::Sun {
            lines.push(json!({"m": std::mem::take(&mut line)}));
        }
        d += Duration::days(1);
    }
    if !line.is_empty() {
        lines.push(json!({"m": line}));
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

// ---- Log ----------------------------------------------------------------------------------------

/// The Log (§5.7): the last day's transactions, newest first. Each is a row (`time`, `who_m`,
/// `tx`, `summary_m`) followed by a `detail` row (what it did, in words), and carries its
/// detail-pane lines (`lines`).
pub fn log(cx: &Cx) -> Result<Value> {
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
    if txs.is_empty() {
        rows.extend(empty("No changes in the last 24h.", &[("", "thc log --since 7d")]));
    }
    let by = agents.iter().map(|(a, n)| format!("{n} by {a}")).collect::<Vec<_>>();
    let mut counts = vec![(plural(txs.len(), "tx", "tx"), Token::Muted)];
    counts.extend(by.into_iter().map(|b| (b, Token::Muted)));
    Ok(json!({"rows": rows, "bar_m": bar("log", &counts), "title_m": format!("{}{}", styled("text+b", "Log"), tok(Token::Muted, &counts.iter().map(|(c, _)| format!(" · {c}")).collect::<String>()))}))
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
