//! `thc review`, bulk `thc undo --by`, `thc diff` and `thc rewind` (docs/design/agents.md §1, §6).

use crate::Ctx;
use crate::cli::ReviewCmd;
use anyhow::Result;
use chrono::{Local, NaiveDate, TimeZone};
use serde_json::{Value, json};
use std::io::{IsTerminal, Write};
use thc_core::dates::{self, DateVal};
use thc_core::error::{ThcError, invalid, not_found, usage};
use thc_core::event::Op;
use thc_core::review::{self, Change, Cut, FieldDiff, Item, NodeDiff};

/// Numbers in a listing stop working after this long.
const LISTING_TTL_MS: i64 = 10 * 60 * 1000;

fn ascii() -> bool {
    std::env::var("THC_GLYPHS").is_ok_and(|g| g == "ascii")
}

fn arrow() -> &'static str {
    if ascii() { "->" } else { "→" }
}

pub(crate) fn marker(change: &str) -> &'static str {
    let a = ascii();
    match change {
        "create" | "restore" => "+",
        "move" => if a { ">" } else { "→" },
        "complete" => if a { "x" } else { "✓" },
        "delete" => if a { "-" } else { "−" },
        "alert" => if a { "(o)" } else { "◎" },
        _ => "~",
    }
}

/// `15:41` today, `Fri 15:41` this week, else `Oct 2 15:41`.
pub fn when(ms: i64, today: NaiveDate) -> String {
    let Some(t) = Local.timestamp_millis_opt(ms).single() else { return String::new() };
    let days = (today - t.date_naive()).num_days();
    match days {
        0 => t.format("%H:%M").to_string(),
        1..=6 => t.format("%a %H:%M").to_string(),
        _ => t.format("%b %-d %H:%M").to_string(),
    }
}

fn actor_label(ctx: &Ctx, actor: &str) -> String {
    match actor.strip_prefix("agent:") {
        Some(name) => ctx.out.agent(&format!("{} {name}", if ascii() { "*" } else { "◆" })),
        None => actor.strip_prefix("human:").unwrap_or(actor).to_string(),
    }
}

pub(crate) fn date_words(s: &str, today: NaiveDate) -> String {
    let Some(dv) = DateVal::from_stored(s) else { return s.to_string() };
    let d = dv.date();
    let days = (d - today).num_days();
    let day = match days {
        -1..=1 => dates::relative(d, today),
        2..=6 => d.format("%a").to_string(),
        _ => d.format("%b %-d").to_string(),
    };
    match dv.time() {
        Some(t) => format!("{day} {}", t.format("%H:%M")),
        None => day,
    }
}

/// A place label with a journal date made friendly (`§ today`).
pub(crate) fn place_words(p: &str, today: NaiveDate) -> String {
    match p.strip_prefix("§ ") {
        Some(d) => format!("§ {}", date_words(d, today)),
        None => p.to_string(),
    }
}

/// A field value for display. `raw` is true for diff values (parent is an id, repeat an object).
fn value_words(ctx: &Ctx, key: &str, v: &Value, raw: bool) -> String {
    let today = ctx.out.today;
    match (key, v) {
        (_, Value::Null) => "—".into(),
        ("parent", Value::String(p)) if raw => place_words(&review::place(ctx.store(), Some(p)), today),
        ("parent", Value::String(p)) => place_words(p, today),
        ("repeat", Value::Object(o)) => format!("↻ {}", o.get("text").and_then(|t| t.as_str()).unwrap_or("")),
        ("repeat", Value::String(t)) => format!("↻ {t}"),
        ("priority", Value::String(p)) => format!("!{p}"),
        ("scheduled" | "due" | "done_at", Value::String(s)) => date_words(s, today),
        ("alert", Value::String(s)) => match DateVal::from_stored(s) {
            Some(DateVal::DateTime(dt)) if dt.date() == today => format!("alert at {}", dt.format("%H:%M")),
            Some(_) => format!("alert at {}", date_words(s, today)),
            None => format!("alert {s}"),
        },
        ("text", Value::String(s)) => ctx.store().render_text(s),
        (_, Value::String(s)) => s.clone(),
        (_, other) => other.to_string(),
    }
}

fn first_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or("");
    if l.chars().count() > 70 { format!("{}…", l.chars().take(69).collect::<String>()) } else { l.to_string() }
}

/// `[ ] text` for tasks, the text otherwise.
fn node_title(ctx: &Ctx, id: &str, fallback: &str) -> String {
    match ctx.store().node(id).ok().flatten() {
        Some(n) => {
            let text = first_line(&if n.title.is_some() { n.label() } else { ctx.store().render_text(&n.text) });
            match n.status.as_deref() {
                Some("done") => format!("[x] {text}"),
                Some("cancelled") => format!("[-] {text}"),
                Some(_) => format!("[ ] {text}"),
                None => text,
            }
        }
        None => first_line(fallback),
    }
}

fn set_words(f: &Value, prefix: &str) -> String {
    let minus = if ascii() { "-" } else { "−" };
    let mut parts = Vec::new();
    for (side, sign) in [("add", "+"), ("remove", minus)] {
        for v in f.get(side).and_then(|a| a.as_array()).into_iter().flatten() {
            parts.push(format!("{sign}{prefix}{}", v.as_str().unwrap_or("")));
        }
    }
    parts.join(" ")
}

/// The lines under a change's marker row (agents.md §1.3), unindented.
pub(crate) fn change_lines(ctx: &Ctx, c: &Change) -> (String, Vec<String>) {
    let title = node_title(ctx, &c.node, &c.text);
    let f = &c.fields;
    let get = |k: &str| f.get(k).and_then(|x| x.get("to")).cloned().unwrap_or(Value::Null);
    let from = |k: &str| f.get(k).and_then(|x| x.get("from")).cloned().unwrap_or(Value::Null);
    let a = arrow();
    match c.change {
        "create" => {
            let mut bits = vec![format!("in {}", value_words(ctx, "parent", &get("parent"), false))];
            for k in ["scheduled", "due"] {
                if !get(k).is_null() {
                    let w = value_words(ctx, k, &get(k), false);
                    bits.push(if k == "due" { format!("due {w}") } else { w });
                }
            }
            for k in ["priority", "repeat"] {
                if !get(k).is_null() {
                    bits.push(value_words(ctx, k, &get(k), false));
                }
            }
            if let Some(t) = f.get("tags") {
                bits.push(set_words(t, "#").replace("+#", "#"));
            }
            if !get("alert").is_null() {
                bits.push(format!("{} {}", marker("alert"), value_words(ctx, "alert", &get("alert"), false)));
            }
            (title, vec![bits.join(" · ")])
        }
        "move" => {
            let p_from = value_words(ctx, "parent", &from("parent"), false);
            let p_to = value_words(ctx, "parent", &get("parent"), false);
            (title, vec![format!("moved from {p_from} to under {p_to}")])
        }
        "complete" => {
            // A repeating completion moves its dates (status stays todo).
            let next = ["scheduled", "due"].iter().find_map(|k| get(k).as_str().and_then(DateVal::from_stored)).map(|d| d.date().format("%a %b %-d").to_string());
            let l = match next {
                Some(n) => format!("done · ↻ next {n}"),
                None => "done".to_string(),
            };
            (title, vec![l])
        }
        "delete" => {
            let kids = c.children.unwrap_or(0);
            (title, vec![if kids > 0 { format!("deleted with {kids} child{}", if kids == 1 { "" } else { "ren" }) } else { "deleted".into() }])
        }
        "restore" => (title, vec!["restored".into()]),
        "alert" => (title, vec![value_words(ctx, "alert", &get("alert"), false)]),
        _ => {
            // One line per field: `due  tomorrow → fri`; long text gets two lines.
            let mut keys: Vec<&String> = f.keys().collect();
            keys.sort_by_key(|k| review::field_rank(k));
            let mut lines = Vec::new();
            for k in keys {
                let label = if k == "parent" { "place" } else { k.as_str() };
                if k == "tags" || k == "links" {
                    lines.push(format!("{label:<9} {}", set_words(&f[k], if k == "tags" { "#" } else { "" })));
                    continue;
                }
                let old = value_words(ctx, k, &from(k), false);
                let new = value_words(ctx, k, &get(k), false);
                if k == "text" && old.chars().count() + new.chars().count() > 50 {
                    lines.push(format!("{label:<9} {}", ctx.out.dim(&first_line(&old))));
                    lines.push(format!("{}{a} {}", " ".repeat(10), first_line(&new)));
                } else {
                    lines.push(format!("{label:<9} {} {a} {new}", ctx.out.dim(&old)));
                }
            }
            (String::new(), lines)
        }
    }
}

fn print_item(ctx: &mut Ctx, it: &Item) {
    let today = ctx.out.today;
    let dev = if it.dev == ctx.vault.device { "this device".to_string() } else { it.dev.clone() };
    let head = format!("{:<2} {}  {} · {} · {dev}", it.n, it.short, actor_label(ctx, &it.actor), when(it.ms, today));
    ctx.out.line(head);
    // A batch (thc apply) is one item: counts here, the list in `thc review show`.
    if it.changes.len() > 3 && it.n > 0 {
        let mut parts: Vec<String> = review::counts(&it.changes).iter().map(|(k, n)| format!("{} {n} {}", marker(k), review::count_word(k))).collect();
        let links = review::link_count(&it.changes);
        if links > 0 {
            parts.push(format!("{links} link{}", if links == 1 { "" } else { "s" }));
        }
        ctx.out.line(format!("   {}", parts.join(" · ")));
        let more = ctx.out.dim(&format!("thc review show {} lists them", it.n));
        ctx.out.line(format!("   {more}"));
        return;
    }
    for c in &it.changes {
        let (title, lines) = change_lines(ctx, c);
        let lead = format!("   {} {:<5}  ", marker(c.change), c.short);
        let pad = " ".repeat(lead.chars().count());
        if title.is_empty() {
            let mut it_lines = lines.iter();
            if let Some(l) = it_lines.next() {
                ctx.out.line(format!("{lead}{l}"));
            }
            for l in it_lines {
                ctx.out.line(format!("{pad}{l}"));
            }
        } else {
            ctx.out.line(format!("{lead}{title}"));
            for l in &lines {
                ctx.out.line(format!("{pad}{}", ctx.out.dim(l)));
            }
        }
        // A concurrent edit is a conflict (below), not a change made afterwards.
        let in_conflict = |f: &str| it.conflict && f == "text";
        for l in it.later.iter().filter(|l| l.node == c.node && !in_conflict(&l.field) && (l.actor == "human" || l.actor.starts_with("human:"))) {
            let note = format!("you changed {} afterwards ({})", if l.field == "parent" { "its place" } else { l.field.as_str() }, when(l.ms, today));
            ctx.out.line(format!("{pad}{}", ctx.out.yellow(&note)));
        }
    }
    if it.conflict {
        let ne = if ascii() { "!=" } else { "≠" };
        let pad = " ".repeat(12);
        ctx.out.line(format!("{pad}{}", ctx.out.red(&format!("{ne} in conflict with your edit · thc conflict ls"))));
    }
}

fn item_json(ctx: &Ctx, it: &Item) -> Value {
    let mut v = serde_json::to_value(it).unwrap_or(Value::Null);
    v["at"] = json!(crate::out::ms_to_local(it.ms));
    v["later_changed"] = json!(it.later.iter().map(|l| json!({ "node": l.node, "field": l.field, "actor": l.actor, "at": crate::out::ms_to_local(l.ms) })).collect::<Vec<_>>());
    if let Some(m) = v.as_object_mut() {
        m.remove("later");
        m.remove("ms");
    }
    let _ = ctx;
    v
}

/// In `--since`, `--as-of` and `--to`, a bare `m` is ambiguous (minutes or months).
pub fn reject_bare_m(s: &str) -> Result<()> {
    dates::reject_bare_m(s)
}

fn since_ms(since: Option<&str>) -> Result<Option<i64>> {
    if let Some(s) = since {
        reject_bare_m(s)?;
    }
    since
        .map(|s| -> Result<i64> {
            let d = dates::parse_duration(s)?;
            Ok(chrono::Utc::now().timestamp_millis() - d.num_milliseconds().abs())
        })
        .transpose()
}

fn listing_path(ctx: &Ctx) -> std::path::PathBuf {
    ctx.vault.paths.cache.join("review-listing.json")
}

fn save_listing(ctx: &Ctx, txs: &[String]) {
    let v = json!({ "ms": chrono::Utc::now().timestamp_millis(), "txs": txs });
    let _ = std::fs::write(listing_path(ctx), v.to_string());
}

/// A number from the last listing, or a transaction suffix.
fn resolve_item(ctx: &Ctx, s: &str) -> Result<String> {
    if let Ok(n) = s.parse::<usize>() {
        let v: Value = std::fs::read_to_string(listing_path(ctx)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        let age = chrono::Utc::now().timestamp_millis() - v["ms"].as_i64().unwrap_or(0);
        if v.is_null() || age > LISTING_TTL_MS {
            return Err(invalid("listing is stale · run thc review again"));
        }
        return v["txs"]
            .get(n.wrapping_sub(1))
            .and_then(|t| t.as_str())
            .map(str::to_string)
            .ok_or_else(|| invalid(format!("no item {n} in the last listing")));
    }
    resolve_tx(ctx, s)
}

pub fn resolve_tx(ctx: &Ctx, s: &str) -> Result<String> {
    let suffix = s.trim().to_uppercase();
    let mut st = ctx.store().conn.prepare("SELECT DISTINCT tx FROM events WHERE tx LIKE ?1 LIMIT 6")?;
    let found: Vec<String> = st.query_map([format!("%{suffix}")], |r| r.get(0))?.collect::<Result<_, _>>()?;
    match found.len() {
        0 => Err(not_found(format!("transaction {s}"))),
        1 => Ok(found[0].clone()),
        _ => Err(ThcError::Ambiguous { prefix: s.into(), candidates: found.iter().map(|t| review::tx_short(t)).collect() }.into()),
    }
}

fn by_words(items: &[Item]) -> String {
    let mut names: Vec<String> = Vec::new();
    for it in items {
        let n = it.actor.strip_prefix("agent:").unwrap_or(&it.actor).to_string();
        if !names.contains(&n) {
            names.push(n);
        }
    }
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        _ => format!("{} and {}", names[..names.len() - 1].join(", "), names[names.len() - 1]),
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn confirm(question: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Err(usage("stdin isn't a terminal · add --yes to confirm"));
    }
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(matches!(s.trim(), "y" | "Y" | "yes"))
}

fn write_ops(ctx: &mut Ctx, ops: Vec<Op>) -> Result<Option<String>> {
    if ctx.dry_run {
        ctx.out.json(&json!({ "dry_run": true, "ops": ops }));
        return Ok(None);
    }
    let r = ctx.write(|b| {
        b.ops.extend(ops);
        Ok(())
    })?;
    Ok(r.and_then(|(ev, ())| ev.first().map(|e| e.tx.clone())))
}

fn skipped_lines(ctx: &mut Ctx, plan: &review::RevertPlan) {
    let today = ctx.out.today;
    for s in plan.skipped.iter().filter(|s| s.reason == "changed_later") {
        let who = if s.actor == "human" || s.actor.starts_with("human:") { "you".to_string() } else { actor_label(ctx, &s.actor) };
        let kept = if who == "you" { "kept yours" } else { "kept theirs" };
        let field = if s.field == "parent" { "place" } else { s.field.as_str() };
        let line = format!("  {:<5} {field}  {who} changed it afterwards ({}), {kept} · --force to revert anyway", ctx.store().short(&s.node), when(s.ms, today));
        ctx.out.line(line);
    }
}

fn skipped_json(ctx: &Ctx, plan: &review::RevertPlan) -> Value {
    json!(plan
        .skipped
        .iter()
        .map(|s| json!({ "tx": s.tx, "node": s.node, "short": ctx.store().short(&s.node), "field": s.field, "reason": s.reason, "actor": s.actor, "at": crate::out::ms_to_local(s.ms) }))
        .collect::<Vec<_>>())
}

pub fn review(ctx: &mut Ctx, cmd: Option<ReviewCmd>, by: Option<String>, since: Option<String>) -> Result<()> {
    let since = since_ms(since.as_deref())?;
    match cmd {
        None => {
            let items = review::queue(ctx.store(), by.as_deref(), since)?;
            save_listing(ctx, &items.iter().map(|i| i.tx.clone()).collect::<Vec<_>>());
            if ctx.out.json {
                let v: Vec<Value> = items.iter().take(ctx.limit).map(|i| item_json(ctx, i)).collect();
                ctx.out.json(&json!({ "pending": items.len(), "items": v }));
                return Ok(());
            }
            if items.is_empty() {
                ctx.out.line("Nothing to review. Agents haven't changed anything since you last looked.");
                return Ok(());
            }
            let head = format!("Review · {} by {} · oldest first", plural(items.len(), "change"), by_words(&items));
            ctx.out.heading(&head);
            for it in items.iter().take(ctx.limit) {
                ctx.out.line("");
                print_item(ctx, it);
            }
            ctx.out.line("");
            let hint = if items.len() == 1 {
                "thc review accept 1 · thc review revert 1".to_string()
            } else {
                format!("thc review accept 1 2 … · thc review revert {} · thc review accept --all", items.len())
            };
            let hint = ctx.out.dim(&hint);
            ctx.out.line(hint);
        }
        Some(ReviewCmd::Show { item }) => {
            let tx = resolve_item(ctx, &item)?;
            let n = review::queue_txs(ctx.store(), None, None)?.iter().position(|t| t == &tx).map(|i| i + 1).unwrap_or(0);
            let it = review::item(ctx.store(), &tx, n)?;
            let h = ctx.store().history_where("tx = ?1 ORDER BY okey", &[&tx])?;
            if ctx.out.json {
                let mut v = item_json(ctx, &it);
                v["events"] = json!(h);
                ctx.out.json(&v);
                return Ok(());
            }
            print_item(ctx, &it);
            if let Some(v) = &it.verdict {
                ctx.out.line(format!("   {}", ctx.out.dim(v)));
            }
            ctx.out.line("");
            for e in &h {
                let short = if e.entity.len() >= 5 { ctx.store().short(&e.entity) } else { e.entity.clone() };
                ctx.out.line(format!("    {:<14} {:<6} {}", e.op, short, crate::summarize(&e.body)));
            }
        }
        Some(ReviewCmd::Accept { items, all }) => {
            if ctx.vault.actor.kind != "human" {
                let name = ctx.vault.actor.name.clone().unwrap_or_else(|| ctx.vault.actor.kind.clone());
                return Err(invalid(format!("only a person can review agent changes · this session is {name}")));
            }
            let txs: Vec<String> = if all {
                review::queue_txs(ctx.store(), by.as_deref(), since)?
            } else if items.is_empty() {
                return Err(usage("name the changes to accept (numbers or tx suffixes), or --all"));
            } else {
                items.iter().map(|s| resolve_item(ctx, s)).collect::<Result<_>>()?
            };
            if txs.is_empty() {
                ctx.out.line("Nothing to review.");
                return Ok(());
            }
            let queued: Vec<Item> = txs.iter().map(|t| review::item(ctx.store(), t, 0)).collect::<Result<_>>()?;
            let tx = write_ops(ctx, vec![Op::TxReview { txs: txs.clone(), verdict: "accepted".into() }])?;
            if tx.is_none() {
                return Ok(());
            }
            if ctx.out.json {
                ctx.out.json(&json!({ "ok": true, "tx": tx, "accepted": txs, "pending": review::pending_count(ctx.store())? }));
            } else {
                ctx.out.line(format!("accepted {} by {} · thc undo to undo", plural(txs.len(), "change"), by_words(&queued)));
            }
        }
        Some(ReviewCmd::Revert { items, force }) => {
            let txs: Vec<String> = items.iter().map(|s| resolve_item(ctx, s)).collect::<Result<_>>()?;
            revert(ctx, &txs, force)?;
        }
    }
    Ok(())
}

/// Revert transactions together (one undo transaction), reporting what was kept.
fn revert(ctx: &mut Ctx, txs: &[String], force: bool) -> Result<()> {
    let plan = review::plan_revert(ctx.store(), txs, force)?;
    let shorts: Vec<String> = txs.iter().map(|t| review::tx_short(t)).collect();
    if ctx.dry_run && !ctx.out.json {
        return revert_preview(ctx, txs, &plan);
    }
    let summary = if txs.len() == 1 {
        let it = review::item(ctx.store(), &txs[0], 0)?;
        it.changes.first().map(|c| format!(" · {}", first_line(&node_title(ctx, &c.node, &c.text)))).unwrap_or_default()
    } else {
        String::new()
    };
    let tx = write_ops(ctx, plan.ops.clone())?;
    if tx.is_none() {
        return Ok(());
    }
    if ctx.out.json {
        let skipped = skipped_json(ctx, &plan);
        ctx.out.json(&json!({ "ok": true, "tx": tx, "reverted": txs, "skipped": skipped, "pending": review::pending_count(ctx.store())? }));
        return Ok(());
    }
    if plan.skipped.iter().any(|s| s.reason == "changed_later") {
        ctx.out.line(format!("reverted {}, except:", shorts.join(", ")));
        skipped_lines(ctx, &plan);
    } else {
        ctx.out.line(format!("reverted {}{summary}", shorts.join(", ")));
    }
    Ok(())
}

/// `--dry-run` in human mode: what each transaction's revert would change (now → then), and
/// what it would skip.
fn revert_preview(ctx: &mut Ctx, txs: &[String], plan: &review::RevertPlan) -> Result<()> {
    let today = ctx.out.today;
    let shorts: Vec<String> = txs.iter().map(|t| review::tx_short(t)).collect();
    ctx.out.line(format!("revert {} (dry run)", shorts.join(", ")));
    let a = arrow();
    let mut n = 0;
    for tx in txs {
        let it = review::item(ctx.store(), tx, 0)?;
        for c in &it.changes {
            let skipped: Vec<&review::Skip> = plan.skipped.iter().filter(|s| &s.tx == tx && s.node == c.node && s.reason == "changed_later").collect();
            let head = format!("  {} {:<5}  {}", marker(c.change), c.short, first_line(&node_title(ctx, &c.node, &c.text)));
            ctx.out.line(head);
            match c.change {
                "create" => {
                    if skipped.is_empty() {
                        ctx.out.line(format!("    {:<9} would be deleted", ""));
                        n += 1;
                    }
                }
                _ => {
                    let mut keys: Vec<&String> = c.fields.keys().collect();
                    keys.sort_by_key(|k| review::field_rank(k));
                    for k in keys {
                        let label = if k == "parent" { "place" } else { k.as_str() };
                        if skipped.iter().any(|s| &s.field == k || (k == "parent" && s.field == "parent")) {
                            continue;
                        }
                        let f = &c.fields[k];
                        let line = if k == "tags" || k == "links" {
                            let flip = json!({ "add": f.get("remove"), "remove": f.get("add") });
                            set_words(&flip, if k == "tags" { "#" } else { "" })
                        } else {
                            let now = value_words(ctx, k, f.get("to").unwrap_or(&Value::Null), false);
                            let then = value_words(ctx, k, f.get("from").unwrap_or(&Value::Null), false);
                            format!("{} {a} {then}", ctx.out.dim(&now))
                        };
                        ctx.out.line(format!("    {label:<9} {line}"));
                        n += 1;
                    }
                }
            }
            for s in skipped {
                let who = if s.actor.starts_with("human") { "you".to_string() } else { actor_label(ctx, &s.actor) };
                let field = if s.field == "parent" { "place" } else { s.field.as_str() };
                let ne = if ascii() { "!=" } else { "≠" };
                // A concurrent edit is a conflict, not a change made afterwards.
                let why = if it.conflict && s.field == "text" {
                    format!("skipped: {ne} in conflict with your edit · thc conflict ls")
                } else {
                    format!("skipped: {who} changed it afterwards ({})", when(s.ms, today))
                };
                let l = ctx.out.dim(&why);
                ctx.out.line(format!("    {field:<9} {l}"));
            }
        }
    }
    ctx.out.line(format!("{} · nothing written", plural(n, "change")));
    Ok(())
}

/// `thc undo --by claude --since 2h`: preview, confirm, one revert transaction.
pub fn undo_bulk(ctx: &mut Ctx, by: &str, since: &str, yes: bool, force: bool) -> Result<()> {
    if !ctx.dry_run {
        ctx.guard()?;
    }
    let ms = since_ms(Some(since))?.unwrap_or(0);
    let by = by.strip_prefix("agent:").unwrap_or(by);
    let txs = review::revertible_txs(ctx.store(), by, ms)?;
    if txs.is_empty() {
        if ctx.out.json {
            ctx.out.json(&json!({ "ok": true, "reverted": [], "skipped": [] }));
        } else {
            ctx.out.line(format!("no changes by {by} in the last {since}"));
        }
        return Ok(());
    }
    if ctx.dry_run {
        let plan = review::plan_revert(ctx.store(), &txs, force)?;
        if !ctx.out.json {
            return revert_preview(ctx, &txs, &plan);
        }
        ctx.out.json(&json!({ "dry_run": true, "reverts": txs, "ops": plan.ops, "skipped": skipped_json(ctx, &plan) }));
        return Ok(());
    }
    if !ctx.out.json {
        let items: Vec<Item> = txs.iter().enumerate().map(|(i, t)| review::item(ctx.store(), t, i + 1)).collect::<Result<_>>()?;
        let head = format!("Revert · {} by {by} from the last {since}", plural(items.len(), "change"));
        ctx.out.heading(&head);
        for it in &items {
            ctx.out.line("");
            print_item(ctx, it);
        }
        ctx.out.line("");
        ctx.out.flush();
    }
    if !confirm(&format!("Revert {} by {by} from the last {since}?", plural(txs.len(), "change")), yes)? {
        ctx.out.line("nothing reverted");
        return Ok(());
    }
    revert(ctx, &txs, force)
}

// ---- diff and rewind ---------------------------------------------------------------------------

fn field_label(k: &str) -> &str {
    if k == "parent" { "place" } else { k }
}

/// `+#q4 −#someday` / `+blocks f5xss` for set fields.
fn diff_set_words(ctx: &Ctx, f: &FieldDiff) -> String {
    let minus = if ascii() { "-" } else { "−" };
    let name = |rel: &str, dst: &str| -> String {
        let label = ctx.store().node(dst).ok().flatten().map(|n| n.label()).unwrap_or_else(|| dst.to_string());
        match rel {
            "tag" => format!("#{label}"),
            "mention" => format!("[[{label}]]"),
            r => format!("{r} {}", ctx.store().short(dst)),
        }
    };
    let mut parts: Vec<String> = f.add.iter().map(|(r, d)| format!("+{}", name(r, d))).collect();
    parts.extend(f.remove.iter().map(|(r, d)| format!("{minus}{}", name(r, d))));
    parts.join(" ")
}

/// The rows of a diff: (left text, attribution).
/// The same diff read the other way (now → then), for rewind previews.
fn reversed(d: &NodeDiff) -> NodeDiff {
    let mut r = d.clone();
    for f in r.fields.iter_mut() {
        std::mem::swap(&mut f.from, &mut f.to);
        std::mem::swap(&mut f.add, &mut f.remove);
    }
    r.children_added = 0;
    r.children_removed = 0;
    r.deleted = None;
    r
}

fn diff_rows(ctx: &Ctx, d: &NodeDiff, attribution: bool) -> Vec<(String, String)> {
    let today = ctx.out.today;
    let a = arrow();
    let mut rows = Vec::new();
    for f in d.fields.iter().filter(|f| f.key != "done_at") {
        let attr = if attribution {
            let edits = if f.edits > 1 { format!("({} edits)  ", f.edits) } else { String::new() };
            format!("{edits}{} · {} · {}", actor_label(ctx, &f.actor), when(f.ms, today), review::tx_short(&f.tx))
        } else {
            String::new()
        };
        let key = format!("  {:<11}", field_label(&f.key));
        if f.key == "tags" || f.key == "links" {
            rows.push((format!("{key}{}", diff_set_words(ctx, f)), attr));
            continue;
        }
        let old = value_words(ctx, &f.key, &f.from, true);
        let new = value_words(ctx, &f.key, &f.to, true);
        if f.key == "text" && old.chars().count() + new.chars().count() > 44 {
            rows.push((format!("{key}{}", ctx.out.dim(&first_line(&old))), String::new()));
            rows.push((format!("{}{a} {}", " ".repeat(11), first_line(&new)), attr));
        } else {
            rows.push((format!("{key}{} {a} {new}", ctx.out.dim(&old)), attr));
        }
    }
    if d.children_added + d.children_removed > 0 {
        let minus = if ascii() { "-" } else { "−" };
        let mut parts = Vec::new();
        if d.children_added > 0 {
            parts.push(format!("+{}", d.children_added));
        }
        if d.children_removed > 0 {
            parts.push(format!("{minus}{}", d.children_removed));
        }
        rows.push((format!("  {:<11}{}", "children", parts.join(" · ")), String::new()));
    }
    if let Some((del, s)) = &d.deleted {
        let short = ctx.store().short(&d.node);
        let line = if *del {
            format!("  {} deleted {} by {} · thc restore {short}", marker("delete"), when(s.ms, today), actor_label(ctx, &s.actor))
        } else {
            format!("  + restored {} by {}", when(s.ms, today), actor_label(ctx, &s.actor))
        };
        rows.push((line, String::new()));
    }
    rows
}

fn visible_len(s: &str) -> usize {
    let mut n = 0;
    let mut esc = false;
    for c in s.chars() {
        match (esc, c) {
            (true, 'm') => esc = false,
            (true, _) => {}
            (false, '\x1b') => esc = true,
            _ => n += 1,
        }
    }
    n
}

fn print_rows(ctx: &mut Ctx, rows: &[(String, String)]) {
    let width = rows.iter().map(|(l, _)| visible_len(l)).max().unwrap_or(0).clamp(40, 64);
    for (l, attr) in rows {
        if attr.is_empty() {
            ctx.out.line(l);
        } else {
            let pad = " ".repeat(width.saturating_sub(visible_len(l)) + 2);
            let attr = ctx.out.dim(attr);
            ctx.out.line(format!("{l}{pad}{attr}"));
        }
    }
}

fn diff_json(ctx: &Ctx, d: &NodeDiff, from: Value) -> Value {
    let mut fields = serde_json::Map::new();
    for f in &d.fields {
        let mut o = serde_json::Map::new();
        if f.key == "tags" || f.key == "links" {
            let pairs = |v: &[(String, String)]| -> Vec<Value> {
                v.iter()
                    .map(|(r, dst)| if f.key == "tags" { json!(ctx.store().node(dst).ok().flatten().map(|n| n.label()).unwrap_or_default()) } else { json!({ "rel": r, "node": dst }) })
                    .collect()
            };
            if !f.add.is_empty() {
                o.insert("add".into(), json!(pairs(&f.add)));
            }
            if !f.remove.is_empty() {
                o.insert("remove".into(), json!(pairs(&f.remove)));
            }
        } else {
            if !f.from.is_null() {
                o.insert("from".into(), f.from.clone());
            }
            if !f.to.is_null() {
                o.insert("to".into(), f.to.clone());
            }
        }
        if f.edits > 1 {
            o.insert("edits".into(), json!(f.edits));
        }
        o.insert("by".into(), json!(f.actor));
        o.insert("at".into(), json!(crate::out::ms_to_local(f.ms)));
        o.insert("tx".into(), json!(f.tx));
        fields.insert(field_label(&f.key).to_string(), Value::Object(o));
    }
    let mut v = json!({ "node": d.node, "short": ctx.store().short(&d.node), "from": from, "fields": fields, "txs": d.txs });
    if d.children_added + d.children_removed > 0 {
        v["children"] = json!({ "add": d.children_added, "remove": d.children_removed });
    }
    if let Some((true, s)) = &d.deleted {
        v["deleted"] = json!({ "at": crate::out::ms_to_local(s.ms), "by": s.actor });
    }
    if let Some(s) = &d.created {
        v["created"] = json!({ "at": crate::out::ms_to_local(s.ms), "by": s.actor });
    }
    v
}

pub fn diff(ctx: &mut Ctx, id: &str, since: Option<String>, tx: Option<String>, as_of: Option<String>) -> Result<()> {
    let id = ctx.resolve(id)?;
    let short = ctx.store().short(&id);
    let today = ctx.out.today;
    if let Some(t) = tx {
        // One transaction's changes to this node (the review item, narrowed).
        let tx = resolve_tx(ctx, &t)?;
        let it = review::item(ctx.store(), &tx, 0)?;
        let change = it.changes.iter().find(|c| c.node == id).cloned();
        if ctx.out.json {
            let fields = change.as_ref().map(|c| json!(c.fields)).unwrap_or(json!({}));
            ctx.out.json(&json!({ "node": id, "short": short, "from": { "tx": tx }, "change": change.as_ref().map(|c| c.change), "fields": fields, "txs": [tx] }));
            return Ok(());
        }
        let Some(c) = change else {
            ctx.out.line(format!("tx {} didn't change {short}", review::tx_short(&tx)));
            return Ok(());
        };
        let head = format!("{short}  {} · tx {} · {} · {}", first_line(&node_title(ctx, &id, "")), review::tx_short(&tx), actor_label(ctx, &it.actor), when(it.ms, today));
        ctx.out.line(head);
        ctx.out.line("");
        let (title, lines) = change_lines(ctx, &c);
        if !title.is_empty() {
            ctx.out.line(format!("  {} {title}", marker(c.change)));
        }
        for l in lines {
            ctx.out.line(format!("  {l}"));
        }
        return Ok(());
    }
    let (cut, from_json, since_word) = match (&as_of, &since) {
        (Some(t), _) => {
            reject_bare_m(t)?;
            let ms = crate::when_to_ms(t, today)? as i64;
            (Cut::at_ms(ms), json!({ "at": crate::out::ms_to_local(ms) }), when(ms, today))
        }
        (None, s) => {
            let ms = since_ms(Some(s.as_deref().unwrap_or("1d")))?.unwrap_or(0);
            (Cut::at_ms(ms), json!({ "at": crate::out::ms_to_local(ms) }), when(ms, today))
        }
    };
    let mut d = review::node_diff(ctx.store(), &id, &cut)?;
    // Created in the range: the creation line, then what changed since it was created.
    let created = d.created.clone();
    if let Some(c) = &created {
        if let Some(after) = Cut::tx(ctx.store(), &c.tx, false)? {
            d = review::node_diff(ctx.store(), &id, &after)?;
            d.created = created.clone();
        }
    }
    if ctx.out.json {
        let v = diff_json(ctx, &d, from_json);
        ctx.out.json(&v);
        return Ok(());
    }
    let title = first_line(&node_title(ctx, &id, ""));
    let rows = diff_rows(ctx, &d, true);
    if let Some(c) = &d.created {
        ctx.out.line(format!("{short}  {title} · created {} by {}", when(c.ms, today), actor_label(ctx, &c.actor)));
        ctx.out.line("");
        if rows.is_empty() {
            let l = ctx.out.dim("no changes since it was created");
            ctx.out.line(format!("  {l}"));
        } else {
            print_rows(ctx, &rows);
        }
        return Ok(());
    }
    if rows.is_empty() {
        let msg = if as_of.is_some() { format!("{short} is the same as it was at {since_word}") } else { format!("no changes to {short} since {since_word}") };
        ctx.out.line(msg);
        return Ok(());
    }
    let mut actors: Vec<String> = Vec::new();
    for f in &d.fields {
        let a = actor_label(ctx, &f.actor);
        if !actors.contains(&a) {
            actors.push(a);
        }
    }
    let n = d.fields.iter().filter(|f| f.key != "done_at").count();
    let head = format!("{short}  {title} · since {since_word} · {} by {}", plural(n, "change"), actors.join(" and "));
    ctx.out.line(head);
    ctx.out.line("");
    print_rows(ctx, &rows);
    ctx.out.line("");
    let hint = ctx.out.dim(&format!("thc rewind {short} --to \"{since_word}\" puts it back"));
    ctx.out.line(hint);
    Ok(())
}

pub fn rewind(ctx: &mut Ctx, id: &str, to: &str, yes: bool) -> Result<()> {
    // Policy before any prompt (policy.md §1): a `confirm` verb asks the human, not stdin.
    if !ctx.dry_run {
        ctx.guard()?;
    }
    let id = ctx.resolve(id)?;
    let short = ctx.store().short(&id);
    let today = ctx.out.today;
    // A tx suffix (optionally `^` for "right before"), else a time.
    reject_bare_m(to)?;
    let (before, t) = match to.strip_suffix('^') {
        Some(t) => (true, t),
        None => (false, to),
    };
    let tx_cut = if t.len() >= 4 && t.chars().all(|c| c.is_ascii_alphanumeric()) && t.chars().any(|c| c.is_ascii_digit() || c.is_ascii_uppercase()) {
        resolve_tx(ctx, t).ok().map(|tx| Cut::tx(ctx.store(), &tx, before)).transpose()?.flatten()
    } else {
        None
    };
    let cut = match tx_cut {
        Some(c) => c,
        None if before => return Err(not_found(format!("transaction {t}"))),
        None => Cut::at_ms(crate::when_to_ms(t, today).map_err(|_| invalid(format!("--to wants a time or a tx suffix, got {to:?}")))? as i64),
    };
    let at = when(cut.ms, today);
    let d = review::node_diff(ctx.store(), &id, &cut)?;
    if let Some(c) = &d.created {
        return Err(not_found(format!("{short} didn't exist at {at} · it was created {}", when(c.ms, today))));
    }
    let ops = review::rewind_ops(ctx.store(), &d)?;
    let n = d.fields.iter().filter(|f| f.key != "done_at").count() + usize::from(matches!(d.deleted, Some((true, _))));
    if ops.is_empty() {
        if ctx.out.json {
            ctx.out.json(&json!({ "ok": true, "tx": null, "fields": 0 }));
        } else {
            ctx.out.line(format!("{short} already matches {at} · nothing written"));
        }
        return Ok(());
    }
    // An agent may not write over an open text conflict (same guard as `thc text`).
    if ctx.vault.actor.kind != "human" && d.fields.iter().any(|f| f.key == "text") {
        let open = ctx.store().conflict_details(Some(&id))?.iter().any(|c| c.kind == "text");
        if open {
            return Err(ThcError::Conflict(format!("{short} has an open text conflict · tell the human, don't resolve it")).into());
        }
    }
    let title = first_line(&node_title(ctx, &id, ""));
    if ctx.dry_run {
        if ctx.out.json {
            ctx.out.json(&json!({ "dry_run": true, "ops": ops, "fields": n }));
            return Ok(());
        }
        ctx.out.line(format!("rewind {short} {title} to how it was at {at} (dry run)"));
        let rows = diff_rows(ctx, &reversed(&d), false);
        print_rows(ctx, &rows);
        let changed: Vec<&str> = d.fields.iter().map(|f| field_label(&f.key)).collect();
        let same: Vec<&str> = ["status", "text", "place", "tags"].into_iter().filter(|k| !changed.contains(k)).collect();
        if !same.is_empty() {
            let l = ctx.out.dim(&format!("  {} unchanged since then: left as they are", same.join(", ")));
            ctx.out.line(l);
        }
        ctx.out.line(format!("{} · nothing written", plural(n, "field")));
        return Ok(());
    }
    if !ctx.out.json && !yes && std::io::stdin().is_terminal() {
        for (l, _) in diff_rows(ctx, &reversed(&d), false) {
            ctx.out.line(l);
        }
        ctx.out.flush();
    }
    if !confirm(&format!("rewind {short}? {} change", plural(n, "field")), yes)? {
        ctx.out.line("nothing written");
        return Ok(());
    }
    let tx = write_ops(ctx, ops)?;
    if ctx.out.json {
        ctx.out.json(&json!({ "ok": true, "tx": tx, "fields": n, "to": crate::out::ms_to_local(cut.ms) }));
    } else {
        ctx.out.line(format!("rewound {short} to {at} · {} · thc undo to redo", plural(n, "field")));
    }
    Ok(())
}
