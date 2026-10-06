//! `thc apply` (agents.md §4): a JSONL batch of CLI-level operations, validated as a whole and
//! written as one transaction or not at all. `thc import -`: an outline into one transaction.
//!
//! Each line is `{"cmd": …, …}` with the command's arguments as fields:
//!
//! | cmd | fields |
//! |---|---|
//! | `add` | `text`, `under?`, `journal?`, `inbox?`, `key?`, `as?` |
//! | `todo` | `text`, `due?`, `sched?`, `priority?`, `tags?[]`, `repeat?`, `under?`, `journal?`, `inbox?`, `key?`, `as?` |
//! | `remind` | `text`, `at`, `repeat?`, `under?`, `inbox?`, `key?`, `as?` |
//! | `set` | `id`, `props{key: value}` |
//! | `text` | `id`, `text` |
//! | `tag` | `id`, `add?[]`, `remove?[]` |
//! | `done` `reopen` `skip` `rm` `restore` | `id` |
//! | `mv` | `id`, one of `under` / `journal` / `root:true` / `after` / `before` |
//! | `link` `unlink` | `a`, `b`, `rel?` |
//! | `alert` | `id`, `before` or `at` |
//!
//! Any id field takes an id prefix, or `$name` for a node an earlier line created with `"as":"name"`.
//! Lines are planned in order against a provisional copy of the store (a rolled-back savepoint),
//! so later lines see earlier ones.

use crate::Ctx;
use crate::review_cmd;
use anyhow::{Result, anyhow};
use chrono::NaiveDate;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{IsTerminal, Read};
use thc_core::builder::TxBuilder;
use thc_core::error::{ThcError, invalid, usage};
use thc_core::event::{Actor, Event, Op, Trigger};
use thc_core::hlc::Hlc;
use thc_core::store::Store;
use thc_core::{capture, dates, review};

/// More ops than this asks first in a terminal.
const CONFIRM_OVER: usize = 20;

pub struct Planned {
    pub line: usize,
    pub cmd: String,
    pub ops: Vec<Op>,
    /// Nodes this line created or changed.
    pub nodes: Vec<String>,
    /// The preview row (marker, cmd, what), filled while the line's effect is visible.
    pub preview: String,
    pub error: Option<anyhow::Error>,
}

fn read_input(file: &str) -> Result<String> {
    let mut s = String::new();
    if file == "-" {
        std::io::stdin().read_to_string(&mut s)?;
    } else {
        s = std::fs::read_to_string(file).map_err(|e| usage(format!("can't read {file}: {e}")))?;
    }
    Ok(s)
}

fn str_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

fn need<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    str_field(v, k).ok_or_else(|| invalid(format!("needs \"{k}\"")))
}

fn list(v: &Value, k: &str) -> Vec<String> {
    match v.get(k) {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(Value::String(s)) => s.split([',', ' ']).filter(|t| !t.is_empty()).map(str::to_string).collect(),
        _ => vec![],
    }
}

/// Raised for a line that names a node an invalid earlier line would have created.
const SKIPPED: &str = "skipped: depends on line ";

struct Refs<'a> {
    store: &'a Store,
    /// `as` name -> created id, or the number of the invalid line that would have created it.
    names: &'a HashMap<String, Result<String, usize>>,
}

impl Refs<'_> {
    fn id(&self, s: &str) -> Result<String> {
        match s.strip_prefix('$') {
            Some(name) => match self.names.get(name) {
                Some(Ok(id)) => Ok(id.clone()),
                Some(Err(line)) => Err(invalid(format!("{SKIPPED}{line}"))),
                None => Err(invalid(format!("no earlier line created \"${name}\" (add \"as\":\"{name}\" to it)"))),
            },
            None => self.store.resolve(s),
        }
    }
    fn opt(&self, v: &Value, k: &str) -> Result<Option<String>> {
        str_field(v, k).map(|s| self.id(s)).transpose()
    }
}

/// Where a new node goes: `inbox`, `under`, `journal <date>`, else today's journal.
fn parent_of(b: &mut TxBuilder, refs: &Refs, v: &Value) -> Result<Option<String>> {
    if v.get("inbox").and_then(|x| x.as_bool()) == Some(true) {
        return Ok(None);
    }
    if let Some(u) = refs.opt(v, "under")? {
        return Ok(Some(u));
    }
    let d = match str_field(v, "journal") {
        Some(j) => dates::parse(j, b.today)?.date(),
        None => b.today,
    };
    Ok(Some(b.journal(d)?))
}

/// Build one line's ops. Returns the node ids it created or changed.
fn build(b: &mut TxBuilder, refs: &Refs, v: &Value) -> Result<Vec<String>> {
    let cmd = need(v, "cmd")?;
    let today = b.today;
    let key_id = |v: &Value| str_field(v, "key").map(thc_core::id::from_key).or_else(|| str_field(v, "id").filter(|_| cmd_creates(cmd)).map(thc_core::id::normalize));
    match cmd {
        "add" | "todo" | "remind" => {
            let mut cap = capture::parse(need(v, "text")?, today)?;
            if cmd == "todo" || cmd == "remind" {
                cap.status = Some(cap.status.unwrap_or_else(|| "todo".into()));
            }
            if let Some(d) = str_field(v, "due") {
                cap.due = Some(dates::parse(d, today)?);
            }
            if let Some(d) = str_field(v, "sched").or(str_field(v, "scheduled")) {
                cap.scheduled = Some(dates::parse(d, today)?);
            }
            if let Some(p) = str_field(v, "priority") {
                cap.priority = Some(capture::normalize_priority(p).ok_or_else(|| invalid("priority must be high, med or low"))?.into());
            }
            if let Some(r) = str_field(v, "repeat") {
                cap.repeat = Some(thc_core::recur::Repeat::parse(r, None)?);
            }
            for t in list(v, "tags") {
                let t = t.trim_start_matches('#').to_lowercase();
                if !cap.tags.contains(&t) {
                    cap.tags.push(t);
                }
            }
            if cmd == "remind" {
                let at = match dates::parse(need(v, "at")?, today)? {
                    dates::DateVal::Date(d) => dates::DateVal::DateTime(d.and_hms_opt(9, 0, 0).unwrap()),
                    dt => dt,
                };
                cap.scheduled = Some(at);
            }
            let id = key_id(v);
            if let Some(i) = &id {
                if b.store.node_exists(i)? {
                    return Ok(vec![i.clone()]); // keyed: already there, a no-op
                }
            }
            let parent = parent_of(b, refs, v)?;
            let id = b.create_from_capture(parent, &cap, id)?;
            if cmd == "remind" {
                b.add_alert(&id, Trigger { at: None, offset: Some("0m".into()), anchor: Some("scheduled".into()) })?;
            }
            Ok(vec![id])
        }
        "set" => {
            let id = refs.id(need(v, "id")?)?;
            // `"props": {"priority": "med"}`, or CLI-style `"set": "priority=med due=+3d"`.
            let kv: Vec<(String, String)> = match (v.get("props").and_then(|p| p.as_object()), str_field(v, "set")) {
                (Some(props), _) => props
                    .iter()
                    .map(|(k, x)| (k.to_lowercase(), match x {
                        Value::Null => String::new(),
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    }))
                    .collect(),
                (None, Some(pairs)) => pairs
                    .split_whitespace()
                    .map(|p| p.split_once('=').map(|(k, x)| (k.to_lowercase(), x.to_string())).ok_or_else(|| invalid(format!("expected key=value, got {p:?}"))))
                    .collect::<Result<_>>()?,
                _ => return Err(invalid("needs \"props\": {\"key\": \"value\"} or \"set\": \"key=value …\"")),
            };
            b.set_props(&id, &kv)?;
            Ok(vec![id])
        }
        "text" => {
            let id = refs.id(need(v, "id")?)?;
            if b.store.conflict_details(Some(&id))?.iter().any(|c| c.kind == "text") {
                return Err(ThcError::Conflict(format!("{} has an open sync conflict · thc conflict ls", b.store.short(&id))).into());
            }
            b.set_text(&id, need(v, "text")?)?;
            Ok(vec![id])
        }
        "tag" => {
            let id = refs.id(need(v, "id")?)?;
            b.add_tags(&id, &list(v, "add"), &list(v, "remove"))?;
            Ok(vec![id])
        }
        "done" | "reopen" | "skip" | "rm" | "restore" => {
            let id = refs.id(need(v, "id")?)?;
            match cmd {
                "done" => {
                    b.complete(&id)?;
                }
                "reopen" => {
                    b.set_props(&id, &[("status".into(), "todo".into()), ("done_at".into(), String::new())])?;
                }
                "skip" => {
                    b.skip(&id)?;
                }
                "rm" => {
                    b.delete(&id)?;
                }
                _ => {
                    b.restore(&id)?;
                }
            }
            Ok(vec![id])
        }
        "mv" => {
            let id = refs.id(need(v, "id")?)?;
            let (after, before) = (refs.opt(v, "after")?, refs.opt(v, "before")?);
            let parent = if v.get("root").and_then(|x| x.as_bool()) == Some(true) {
                None
            } else if let Some(j) = str_field(v, "journal") {
                Some(b.journal(dates::parse(j, today)?.date())?)
            } else if let Some(u) = refs.opt(v, "under")? {
                Some(u)
            } else if after.is_some() || before.is_some() {
                b.store.must_node(&id)?.parent
            } else {
                return Err(invalid("say where: under, journal, root, after or before"));
            };
            b.move_to(&id, parent, after.as_deref(), before.as_deref())?;
            Ok(vec![id])
        }
        "link" | "unlink" => {
            let (a, bb) = (refs.id(need(v, "a")?)?, refs.id(need(v, "b")?)?);
            if cmd == "link" {
                b.link(&a, &bb, str_field(v, "rel").unwrap_or("relates"))?;
            } else {
                b.unlink(&a, &bb, str_field(v, "rel"))?;
            }
            Ok(vec![a, bb])
        }
        "alert" => {
            let id = refs.id(need(v, "id")?)?;
            let trigger = match (str_field(v, "before"), str_field(v, "at")) {
                (Some(off), None) => {
                    dates::user_duration(off)?;
                    Trigger { at: None, offset: Some(format!("-{}", off.trim_start_matches(['-', '+']))), anchor: Some(str_field(v, "anchor").unwrap_or("due").into()) }
                }
                (None, Some(at)) => Trigger { at: Some(dates::parse(at, today)?.fmt()), offset: None, anchor: None },
                _ => return Err(invalid("needs \"before\" (e.g. 1d) or \"at\"")),
            };
            b.add_alert(&id, trigger)?;
            Ok(vec![id])
        }
        other => {
            const CMDS: [&str; 15] = ["add", "todo", "remind", "set", "text", "tag", "done", "reopen", "skip", "rm", "restore", "mv", "link", "unlink", "alert"];
            let hint = CMDS.iter().min_by_key(|c| distance(c, other)).filter(|c| distance(c, other) <= 2).map(|c| format!(" · did you mean {c}?")).unwrap_or_default();
            Err(invalid(format!("unknown cmd \"{other}\"{hint}")))
        }
    }
}

fn cmd_creates(cmd: &str) -> bool {
    matches!(cmd, "add" | "todo" | "remind")
}

/// Levenshtein distance, for did-you-mean.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Apply ops to the store provisionally (inside the caller's savepoint) so later lines see them.
fn provisional(store: &Store, ops: &[Op], tx: &str) -> Result<()> {
    let mut hlc = store.max_hlc()?;
    for op in ops {
        hlc = Hlc::tick(hlc);
        let e = Event {
            v: op.min_version(),
            eid: format!("preview-{}", thc_core::id::new_id()),
            hlc,
            dev: "preview".into(),
            actor: Actor { kind: "human".into(), name: None },
            via: "apply".into(),
            tx: tx.to_string(),
            op: op.clone(),
        };
        store.apply(&e)?;
    }
    Ok(())
}

/// Plan every line against a provisional store. The savepoint is always rolled back.
/// `preview`: render preview rows (dry runs; the store must be `ctx`'s).
pub fn plan(preview: Option<&Ctx>, store: &Store, lines: &[(usize, Value)], today: NaiveDate) -> Result<Vec<Planned>> {
    store.conn.execute_batch("SAVEPOINT thc_apply")?;
    let r = plan_inner(preview, store, lines, today);
    store.conn.execute_batch("ROLLBACK TO thc_apply; RELEASE thc_apply")?;
    r
}

fn plan_inner(ctx: Option<&Ctx>, store: &Store, lines: &[(usize, Value)], today: NaiveDate) -> Result<Vec<Planned>> {
    let mut names: HashMap<String, Result<String, usize>> = HashMap::new();
    let mut out = Vec::new();
    for (n, v) in lines {
        let cmd = str_field(v, "cmd").unwrap_or("?").to_string();
        let mut b = TxBuilder::new(store, today);
        let refs = Refs { store, names: &names };
        let built = if v.is_object() { build(&mut b, &refs, v) } else { Err(invalid("each line must be a JSON object")) };
        let ops = b.finish();
        match built {
            Ok(nodes) => {
                let tx = format!("preview-line-{n}");
                provisional(store, &ops, &tx)?;
                if let (Some(name), Some(id)) = (str_field(v, "as"), nodes.first()) {
                    names.insert(name.to_string(), Ok(id.clone()));
                }
                let preview = ctx.map(|c| preview_row(c, store, &cmd, &tx, &nodes, v)).unwrap_or_default();
                out.push(Planned { line: *n, cmd, ops, nodes, preview, error: None });
            }
            Err(e) => {
                if let Some(name) = str_field(v, "as") {
                    names.insert(name.to_string(), Err(*n));
                }
                out.push(Planned { line: *n, cmd, ops: vec![], nodes: vec![], preview: String::new(), error: Some(e) })
            }
        }
    }
    Ok(out)
}

/// `+ todo   [ ] Book the venue  due fri  → under ¶ Offsite`, `~ set    pab27  priority  !high → !med`
fn preview_row(ctx: &Ctx, store: &Store, cmd: &str, tx: &str, nodes: &[String], v: &Value) -> String {
    let title = |id: &str| store.node(id).ok().flatten().map(|n| if n.title.is_some() { n.label() } else { store.render_text(&n.text) }).unwrap_or_default();
    if let ("link" | "unlink", [a, b, ..]) = (cmd, nodes) {
        let rel = str_field(v, "rel").unwrap_or(if cmd == "link" { "relates" } else { "any" });
        return format!("  ~ {cmd:<6}   {}  {rel}  {}", title(a), title(b));
    }
    let Ok(it) = review::item(store, tx, 0) else {
        return format!("  · {cmd:<6} {}", nodes.first().map(|n| store.short(n)).unwrap_or_default());
    };
    let Some(c) = nodes.first().and_then(|n| it.changes.iter().find(|c| &c.node == n)).or(it.changes.first()) else {
        let short = nodes.first().map(|n| store.short(n)).unwrap_or_default();
        return format!("  · {cmd:<6} {short}  no change");
    };
    let (title, lines) = review_cmd::change_lines(ctx, c);
    let marker = review_cmd::marker(c.change);
    if c.change == "create" {
        let place = c.fields.get("parent").and_then(|p| p.get("to")).and_then(|p| p.as_str()).map(|p| review_cmd::place_words(p, ctx.out.today)).unwrap_or_default();
        let mut bits: Vec<String> = Vec::new();
        for k in ["scheduled", "due"] {
            if let Some(d) = c.fields.get(k).and_then(|f| f.get("to")).and_then(|d| d.as_str()) {
                bits.push(format!("{} {}", if k == "due" { "due" } else { "sched" }, review_cmd::date_words(d, ctx.out.today)));
            }
        }
        let extra = if bits.is_empty() { String::new() } else { format!("  {}", bits.join(" · ")) };
        let to = if place == "inbox" || place.starts_with('§') { format!("→ {place}") } else { format!("→ under {place}") };
        return format!("  {marker} {cmd:<6}   {title}{extra}  {to}");
    }
    let what = if title.is_empty() { lines.join(" · ") } else { format!("{title} · {}", lines.join(" · ")) };
    format!("  {marker} {cmd:<6} {:<6} {what}", c.short)
}

fn short_error(e: &anyhow::Error) -> String {
    let s = format!("{e:#}");
    s.trim_start_matches("not found: ").trim_start_matches("invalid: ").trim_start_matches("conflict: ").to_string()
}

fn error_json(e: &anyhow::Error) -> Value {
    let kind = e.downcast_ref::<ThcError>().map(|t| t.kind()).unwrap_or("error");
    let kind = if short_error(e).starts_with(SKIPPED) { "skipped" } else { kind };
    json!({ "kind": kind, "message": short_error(e) })
}

fn parse_lines(text: &str) -> Vec<(usize, Result<Value, String>)> {
    // A single JSON array works too (models emit arrays reliably); items count as lines 1..n.
    if text.trim_start().starts_with('[') {
        return match serde_json::from_str::<Vec<Value>>(text) {
            Ok(items) => items.into_iter().enumerate().map(|(i, v)| (i + 1, Ok(v))).collect(),
            Err(e) => vec![(1, Err(format!("not a JSON array: {e}")))],
        };
    }
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#') && !l.trim_start().starts_with("//"))
        .map(|(i, l)| (i + 1, serde_json::from_str::<Value>(l).map_err(|e| format!("not JSON: {e}"))))
        .collect()
}

pub fn apply(ctx: &mut Ctx, file: &str, yes: bool) -> Result<()> {
    let text = read_input(file)?;
    let parsed = parse_lines(&text);
    if parsed.is_empty() {
        return Err(usage("no operations (one JSON object per line)"));
    }
    let total = parsed.len();
    let mut bad: Vec<(usize, String, Value)> = Vec::new();
    let mut lines: Vec<(usize, Value)> = Vec::new();
    for (n, r) in parsed {
        match r {
            Ok(v) => lines.push((n, v)),
            Err(e) => bad.push((n, "?".into(), json!({ "kind": "validation", "message": e }))),
        }
    }
    let today = ctx.out.today;
    let planned = plan(Some(ctx), ctx.store(), &lines, today)?;
    for p in &planned {
        if let Some(e) = &p.error {
            bad.push((p.line, p.cmd.clone(), error_json(e)));
        }
    }
    let dry = ctx.dry_run;
    let suffix = if dry { " (dry run)" } else { "" };
    if !bad.is_empty() {
        bad.sort_by_key(|b| b.0);
        if ctx.out.json {
            let errors: Vec<Value> = bad.iter().map(|(n, cmd, e)| json!({ "line": n, "cmd": cmd, "error": e })).collect();
            let skipped = bad.iter().filter(|b| b.2["kind"] == "skipped").count();
            ctx.out.json(&json!({ "ok": false, "ops": total, "invalid": bad.len() - skipped, "skipped": skipped, "errors": errors }));
        } else {
            ctx.out.line(format!("apply · {} op{}{suffix}", total, if total == 1 { "" } else { "s" }));
            for (n, cmd, e) in &bad {
                let id = lines.iter().find(|(l, _)| l == n).and_then(|(_, v)| str_field(v, "id").or(str_field(v, "a"))).unwrap_or("");
                let id = if id.starts_with('$') { "" } else { id };
                ctx.out.line(format!("  line {n:<3} {cmd:<6} {id:<6} {}", e["message"].as_str().unwrap_or("")));
            }
            let skipped = bad.iter().filter(|b| b.2["kind"] == "skipped").count();
            let invalid_n = bad.len() - skipped;
            let skip = if skipped > 0 { format!(", {skipped} skipped") } else { String::new() };
            ctx.out.line(format!("{invalid_n} of {total} invalid{skip} · nothing written · fix them and run again"));
        }
        ctx.out.flush();
        return Err(anyhow::Error::from(ThcError::Validation(format!("{} invalid line(s)", bad.len()))).context(crate::daemon_cmd::Quiet));
    }
    let ops: Vec<Op> = planned.iter().flat_map(|p| p.ops.clone()).collect();
    if dry {
        if ctx.out.json {
            let items: Vec<Value> = planned.iter().map(|p| json!({ "line": p.line, "cmd": p.cmd, "nodes": p.nodes })).collect();
            ctx.out.json(&json!({ "dry_run": true, "ok": true, "ops": total, "lines": items, "events": ops }));
        } else {
            ctx.out.line(format!("apply · {total} op{} → 1 transaction (dry run)", if total == 1 { "" } else { "s" }));
            for p in &planned {
                ctx.out.line(&p.preview);
            }
            ctx.out.line(format!("{total} ok · nothing written"));
        }
        return Ok(());
    }
    if total > CONFIRM_OVER && !yes && !ctx.out.json && std::io::stdin().is_terminal() && file != "-" {
        eprint!("apply {total} ops as one transaction? [y/N] ");
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        if !matches!(s.trim(), "y" | "Y" | "yes") {
            ctx.out.line("nothing written");
            return Ok(());
        }
    }
    // Plan again under the write lock (the store may have moved), then commit as one tx.
    let pre = ctx.pre.clone();
    let mut result_nodes: Vec<(usize, String, Vec<String>)> = Vec::new();
    let guard = ctx.guard()?;
    let (events, ()) = ctx.vault.transact(|s| {
        let planned = plan(None, s, &lines, today)?;
        if let Some(p) = planned.iter().find(|p| p.error.is_some()) {
            return Err(anyhow!("line {} changed meanwhile: {}", p.line, short_error(p.error.as_ref().unwrap())));
        }
        let ops: Vec<Op> = planned.iter().flat_map(|p| p.ops.clone()).collect();
        crate::pre::check(s, &ops, &pre)?;
        // Per line first, so a refusal names it (`claude can't delete (apply line 3, …)`).
        for p in &planned {
            guard.ops_at(&p.ops, Some(&format!("apply line {}", p.line)))?;
        }
        guard.ops(&ops)?;
        result_nodes = planned.iter().map(|p| (p.line, p.cmd.clone(), p.nodes.clone())).collect();
        Ok((ops, ()))
    })?;
    let tx = events.first().map(|e| e.tx.clone());
    if ctx.out.json {
        let items: Vec<Value> = result_nodes.iter().map(|(n, cmd, nodes)| json!({ "line": n, "cmd": cmd, "nodes": nodes })).collect();
        let mut ids: Vec<String> = Vec::new();
        for (_, _, ns) in &result_nodes {
            for n in ns {
                if !ids.contains(n) {
                    ids.push(n.clone());
                }
            }
        }
        let nodes: Vec<Value> = ids.iter().filter_map(|i| ctx.store().node(i).ok().flatten()).map(|n| crate::out::node_json(ctx.store(), &n)).collect();
        ctx.out.json(&json!({ "ok": true, "tx": tx, "ops": total, "events": events.len(), "lines": items, "nodes": nodes }));
    } else {
        match &tx {
            Some(t) => {
                let s = review::tx_short(t);
                ctx.out.line(format!("applied {total} op{} · tx {s} · thc undo --tx {s} reverts all of it", if total == 1 { "" } else { "s" }));
            }
            None => ctx.out.line(format!("applied {total} op{} · nothing changed", if total == 1 { "" } else { "s" })),
        }
    }
    Ok(())
}

/// `thc import -`: an outline (the `thc edit` format, without ^ids) into one transaction.
pub fn import(ctx: &mut Ctx, file: &str, under: Option<String>, journal: Option<String>, page: Option<String>) -> Result<()> {
    let text = read_input(file)?;
    if text.trim().is_empty() {
        return Err(usage("nothing to import"));
    }
    let today = ctx.out.today;
    let under = match under {
        Some(u) => Some(ctx.resolve(&u)?),
        None => None,
    };
    let jdate = match &journal {
        Some(j) => dates::parse(j, today)?.date(),
        None => today,
    };
    if ctx.dry_run && !ctx.out.json {
        // The apply preview format: target, then the outline with markers and indents.
        let lines = outline(&text);
        let place = match (&under, &page) {
            (Some(u), _) => review_cmd::place_words(&review::place(ctx.store(), Some(u)), today),
            (None, Some(t)) => format!("¶ {t}"),
            _ => format!("§ {}", review_cmd::date_words(&jdate.format("%Y-%m-%d").to_string(), today)),
        };
        let n = lines.len();
        ctx.out.line(format!("import · {n} node{} under {place} → 1 transaction (dry run)", if n == 1 { "" } else { "s" }));
        for (indent, body) in &lines {
            // Parsed, as apply shows it: `+ [ ] Book flights  due Fri`.
            let cap = capture::parse(body, today)?;
            let check = match cap.status.as_deref() {
                Some("done") => "[x] ",
                Some("cancelled") => "[-] ",
                Some(_) => "[ ] ",
                None => "",
            };
            let mut bits = Vec::new();
            if let Some(d) = &cap.scheduled {
                bits.push(format!("sched {}", review_cmd::date_words(&d.fmt(), today)));
            }
            if let Some(d) = &cap.due {
                bits.push(format!("due {}", review_cmd::date_words(&d.fmt(), today)));
            }
            if let Some(p) = &cap.priority {
                bits.push(format!("!{p}"));
            }
            if let Some(r) = &cap.repeat {
                bits.push(format!("↻ {}", r.text));
            }
            let meta = if bits.is_empty() { String::new() } else { format!("  {}", bits.join(" · ")) };
            ctx.out.line(format!("  {}+ {check}{}{meta}", " ".repeat(*indent), cap.text));
        }
        ctx.out.line(format!("{n} ok · nothing written"));
        return Ok(());
    }
    // Strict like the rest of the CLI: a token that doesn't parse (`due:fryday`, `due:+30m`)
    // exits 6 naming its line, before anything is written. (Appending to an existing page goes
    // through the editor round trip, which on its own would keep such a token as text.)
    for (i, (_, body)) in outline(&text).iter().enumerate() {
        if let Err(e) = capture::parse(body, today) {
            let msg = e.to_string();
            return Err(invalid(format!("line {}: {}", i + 1, msg.strip_prefix("invalid: ").unwrap_or(&msg))));
        }
    }
    let r = ctx.write(|b| {
        let root = match (&under, &page) {
            (Some(u), _) => u.clone(),
            (None, Some(title)) => match b.store.find_root_by_title(title, false)? {
                Some(id) => id,
                None => b.create_page(title, &[])?,
            },
            _ => b.journal(jdate)?,
        };
        // A root created in this same tx isn't in the store yet: create children directly.
        let lines = outline(&text);
        let gaps = outline_gaps(&text);
        if !b.store.node_exists(&root)? {
            let mut stack: Vec<(usize, String)> = Vec::new();
            for (k, (indent, body)) in lines.iter().enumerate() {
                while stack.last().is_some_and(|(i, _)| i >= indent) {
                    stack.pop();
                }
                let parent = stack.last().map(|(_, id)| id.clone()).unwrap_or_else(|| root.clone());
                let cap = capture::parse(body, b.today)?;
                let id = b.create_from_capture(Some(parent), &cap, None)?;
                // A blank line before a top-level item (these are list items: the default is none).
                if *indent == 0 && gaps[k].2 {
                    let v = b.prop_value(thc_core::edit::GAP, "1")?;
                    let mut props = serde_json::Map::new();
                    props.insert(thc_core::edit::GAP.into(), v);
                    b.ops.push(thc_core::event::Op::NodeSet { id: id.clone(), props });
                }
                stack.push((*indent, id));
            }
            return Ok((root, lines.len()));
        }
        let rendered = thc_core::edit::render(b.store, &root)?;
        let header = rendered.text.lines().any(|l| l.starts_with("# "));
        let pad = if header { 0 } else { 4 };
        let mut edited = rendered.text.clone();
        if !edited.ends_with('\n') {
            edited.push('\n');
        }
        for (indent, body, gap) in &gaps {
            if *gap && *indent == 0 {
                edited.push('\n');
            }
            edited.push_str(&format!("{}- {body}\n", " ".repeat(pad + indent)));
        }
        let summary = thc_core::edit::apply(b, &rendered, &edited)?;
        Ok((root, summary.created))
    })?;
    if let Some((events, (root, n))) = r {
        let tx = events.first().map(|e| e.tx.clone());
        if ctx.out.json {
            ctx.out.json(&json!({ "ok": true, "tx": tx, "created": n, "root": root, "events": events.len() }));
        } else {
            let s = tx.as_deref().map(review::tx_short).unwrap_or_default();
            let place = review_cmd::place_words(&review::place(ctx.store(), Some(&root)), today);
            ctx.out.line(format!("imported {n} node{} under {place} · tx {s} · thc undo --tx {s} reverts all of it", if n == 1 { "" } else { "s" }));
        }
    }
    Ok(())
}

/// Outline lines as (relative indent, text): `- ` / `* ` bullets or plain lines; tabs = 4.
fn outline(text: &str) -> Vec<(usize, String)> {
    outline_gaps(text).into_iter().map(|(i, b, _)| (i, b)).collect()
}

/// The same, with whether a blank line came before each (its `gap`, writing.md §1, E80).
fn outline_gaps(text: &str) -> Vec<(usize, String, bool)> {
    let mut blank = false;
    let mut raw: Vec<(usize, String, bool)> = Vec::new();
    for l in text.lines() {
        if l.trim().is_empty() {
            blank = !raw.is_empty();
            continue;
        }
        let l = l.replace('\t', "    ");
        let indent = l.len() - l.trim_start().len();
        let t = l.trim();
        let body = t.strip_prefix("- ").or(t.strip_prefix("* ")).unwrap_or(t);
        raw.push((indent, body.to_string(), std::mem::take(&mut blank)));
    }
    let min = raw.iter().map(|(i, _, _)| *i).min().unwrap_or(0);
    raw.into_iter().map(|(i, b, g)| (i - min, b, g)).collect()
}
