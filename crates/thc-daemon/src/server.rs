//! Socket server: one thread per connection, newline-delimited JSON (thc_core::proto).

use crate::{ClientConn, Shared, status_json};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use thc_core::builder::TxBuilder;
use thc_core::error::{invalid, usage};
use thc_core::event::Op;
use thc_core::model::Node;
use thc_core::proto::{self, PROTO_VERSION, Request, Response, VERSION};

pub fn accept_loop(shared: Arc<Shared>, listener: UnixListener) {
    for stream in listener.incoming() {
        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        let sh = shared.clone();
        std::thread::spawn(move || {
            let _ = handle(sh, stream);
        });
    }
}

fn handle(shared: Arc<Shared>, stream: UnixStream) -> Result<()> {
    let id = shared.next_client.fetch_add(1, Ordering::SeqCst);
    let mut writer = stream.try_clone()?;
    let reader = BufReader::new(stream.try_clone()?);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response { id: 0, result: None, error: Some(proto::RpcError { kind: "usage".into(), message: format!("bad request: {e}") }) };
                writeln!(writer, "{}", serde_json::to_string(&resp)?)?;
                continue;
            }
        };
        let result = dispatch(&shared, id, &stream, &req);
        let resp = match result {
            Ok(v) => Response { id: req.id, result: Some(v), error: None },
            Err(e) => Response { id: req.id, result: None, error: Some(proto::rpc_error(&e)) },
        };
        // Responses and pushed events share the socket; the clients list holds a clone, so
        // write through a fresh clone under the clients lock to avoid interleaving lines.
        {
            let _guard = shared.clients.lock().unwrap();
            writeln!(writer, "{}", serde_json::to_string(&resp)?)?;
        }
        if req.method == "shutdown" {
            break;
        }
    }
    let mut clients = shared.clients.lock().unwrap();
    if let Some(i) = clients.iter().position(|c| c.id == id) {
        let c = clients.remove(i);
        drop(clients);
        shared.log("client", &format!("{} disconnected", c.kind));
    }
    Ok(())
}

fn s<'a>(p: &'a Value, k: &str) -> Result<&'a str> {
    p.get(k).and_then(|v| v.as_str()).ok_or_else(|| usage(format!("missing \"{k}\"")))
}

fn dispatch(shared: &Arc<Shared>, client_id: u64, stream: &UnixStream, req: &Request) -> Result<Value> {
    let p = &req.params;
    match req.method.as_str() {
        "hello" => {
            let kind = p.get("client").and_then(|v| v.as_str()).unwrap_or("client").to_string();
            let topics: HashSet<String> = p.get("topics").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect()).unwrap_or_default();
            let deliver = p.get("deliver").and_then(|v| v.as_bool()).unwrap_or(false);
            let their_version = p.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string();
            {
                let mut clients = shared.clients.lock().unwrap();
                clients.retain(|c| c.id != client_id);
                clients.push(ClientConn { id: client_id, kind: kind.clone(), topics, deliver, writer: stream.try_clone()? });
            }
            shared.log("client", &format!("{kind} connected{}", if deliver { " (delivers alerts)" } else { "" }));
            let device = shared.vault.lock().unwrap().device.clone();
            Ok(json!({ "daemon": VERSION, "proto": PROTO_VERSION, "proto_minor": thc_core::proto::PROTO_MINOR, "device": device, "version_mismatch": !their_version.is_empty() && their_version != VERSION }))
        }
        "status" => status_json(shared),
        "shutdown" => {
            shared.shutdown.store(true, Ordering::SeqCst);
            // Unblock accept() so the server thread can exit.
            let _ = UnixStream::connect(&shared.socket);
            Ok(json!({ "ok": true }))
        }
        "query" => {
            let q = p.get("q").and_then(|v| v.as_str()).unwrap_or("status:open sort:due");
            let limit = p.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            let v = shared.vault.lock().unwrap();
            let nodes = v.store.query(q, thc_core::dates::today(), limit)?;
            Ok(json!({ "count": nodes.len(), "items": nodes.iter().map(|n| node_json(&v.store, n)).collect::<Vec<_>>() }))
        }
        "today" => today_json(shared),
        "alert.delivered" => {
            let ids: Vec<String> = p.get("alerts").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
            shared.delivered.lock().unwrap().extend(ids.iter().cloned());
            Ok(json!({ "ok": true, "claimed": ids.len() }))
        }
        // A capture preview: what `capture` would save, from the same parser (writes nothing).
        "parse" => {
            let text = s(p, "text")?;
            Ok(thc_core::capture::preview(text, thc_core::dates::today())?)
        }
        "capture" => {
            let text = s(p, "text")?.to_string();
            let named = p.get("target").and_then(|v| v.as_str()).map(str::to_string);
            // No target named: the vault's capture target, if it sets one (vaults.md §9).
            let eff = named.is_none().then(|| {
                let path = shared.vault.lock().map(|v| v.paths.vault.clone()).ok();
                thc_core::settings::load(path.as_deref())
            });
            let target = named.unwrap_or_else(|| "journal".into());
            write(shared, |b| {
                let cap = thc_core::capture::parse_lenient(&text, b.today)?.0;
                let set = eff.as_ref().and_then(|e| thc_core::settings::capture_target(e, b.store)).map(|(id, _)| id);
                let parent = match target.as_str() {
                    _ if set.is_some() => set.clone(),
                    "inbox" => None,
                    "journal" | "today" => Some(b.journal(b.today)?),
                    id => Some(b.store.resolve(id)?),
                };
                b.create_from_capture(parent, &cap, None).map(|id| json!({ "id": id }))
            })
        }
        "complete" => {
            let id = resolve(shared, s(p, "id")?)?;
            write(shared, |b| b.complete(&id).map(|next| json!({ "id": id, "next": next })))
        }
        "set" => {
            let id = resolve(shared, s(p, "id")?)?;
            let pairs: Vec<(String, String)> = p
                .get("props")
                .and_then(|v| v.as_object())
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| if v.is_null() { String::new() } else { v.to_string() }))).collect())
                .ok_or_else(|| usage("missing \"props\""))?;
            write(shared, |b| b.set_props(&id, &pairs).map(|_| json!({ "id": id })))
        }
        "snooze" | "ack" => {
            let alert = s(p, "alert")?.to_string();
            let until = if req.method == "snooze" {
                let raw = p.get("until").and_then(|v| v.as_str()).unwrap_or("1h");
                // Protocol v1 compatibility: `until: "15m"` (minutes) was accepted from the start
                // and older ThoughtBars send it, so the socket keeps reading it. People typing
                // durations go through dates::user_duration, which rejects a bare `m`.
                Some(match thc_core::dates::parse_duration(raw) {
                    Ok(d) => (thc_core::dates::now_local() + d).format("%Y-%m-%dT%H:%M").to_string(),
                    Err(_) => thc_core::dates::parse(raw, thc_core::dates::today())?.fmt(),
                })
            } else {
                None
            };
            write(shared, |b| {
                b.ops.push(match &until {
                    Some(u) => Op::AlertSnooze { id: alert.clone(), until: u.clone() },
                    None => Op::AlertAck { id: alert.clone() },
                });
                Ok(json!({ "alert": alert, "until": until }))
            })
        }
        "undo" => {
            let tx = {
                let v = shared.vault.lock().unwrap();
                match p.get("tx").and_then(|v| v.as_str()) {
                    Some(t) => v.store.conn.query_row("SELECT tx FROM events WHERE tx LIKE ?1 ORDER BY okey DESC LIMIT 1", [format!("%{t}")], |r| r.get::<_, String>(0)).map_err(|_| anyhow!("no transaction {t}"))?,
                    None => v.store.conn.query_row("SELECT tx FROM events ORDER BY okey DESC LIMIT 1", [], |r| r.get::<_, String>(0)).map_err(|_| anyhow!("nothing to undo"))?,
                }
            };
            let inverse = thc_core::review::undo_ops(&shared.vault.lock().unwrap().store, &tx)?;
            if inverse.is_empty() {
                return Err(invalid("that transaction has nothing to undo"));
            }
            write(shared, |b| {
                b.ops.extend(inverse);
                Ok(json!({ "undone": tx }))
            })
        }
        // ---- documents as blocks (protocol v2, mac-editor-arch.md §4, §6) ----
        "page" => {
            let id = resolve(shared, s(p, "id")?)?;
            let v = shared.vault.lock().unwrap();
            let root = v.store.must_node(&id)?;
            let blocks = thc_core::outline::render(&v.store, &id)?;
            Ok(json!({ "root": id, "title": root.title, "journal": root.journal, "blocks": blocks }))
        }
        "journal" => {
            // `date` is YYYY-MM-DD or anything the date parser reads (`today`, `-1d`). A day with
            // nothing written yet has no node: `root` is null and the first save creates it.
            let today = thc_core::dates::today();
            let date = match p.get("date").and_then(|v| v.as_str()) {
                Some(d) => thc_core::dates::parse(d, today)?.date(),
                None => today,
            };
            let key = date.format("%Y-%m-%d").to_string();
            let v = shared.vault.lock().unwrap();
            let root = v.store.journal_node(&key)?;
            let blocks = match &root {
                Some(id) => thc_core::outline::render(&v.store, id)?,
                None => vec![],
            };
            Ok(json!({ "root": root, "date": key, "blocks": blocks }))
        }
        // Where a node lives: its page or journal day (the window opens it there).
        "locate" => {
            let id = resolve(shared, s(p, "id")?)?;
            let v = shared.vault.lock().unwrap();
            let mut top = v.store.must_node(&id)?;
            while let Some(pid) = top.parent.clone() {
                top = v.store.must_node(&pid)?;
            }
            Ok(json!({ "id": id, "root": top.id, "journal": top.journal, "title": top.title, "inbox": top.id == id && top.parent.is_none() && top.title.is_none() && top.journal.is_none() }))
        }
        "delete" => {
            let id = resolve(shared, s(p, "id")?)?;
            write(shared, |b| b.delete(&id).map(|n| json!({ "id": id, "deleted": n })))
        }
        // Open conflicts on a node (or all), with both versions (editor.md §8.1 compare).
        "conflicts" => {
            let id = match p.get("id").and_then(|v| v.as_str()) {
                Some(i) => Some(resolve(shared, i)?),
                None => None,
            };
            let v = shared.vault.lock().unwrap();
            let s = &v.store;
            let at = |ms: i64| chrono::TimeZone::timestamp_millis_opt(&chrono::Local, ms).single().map(|t| t.format("%Y-%m-%dT%H:%M").to_string());
            // The CLI's shape (`thc conflict ls --json`): rendered text, local times.
            let items: Vec<Value> = s
                .conflict_details(id.as_deref())?
                .iter()
                .map(|d| {
                    let ver = |v: &Option<thc_core::model::ConflictVersion>| {
                        v.as_ref().map(|v| json!({ "text": if d.kind == "text" { s.render_text(&v.text) } else { v.text.clone() }, "actor": v.actor, "dev": v.dev, "at": at(v.ms), "eid": v.eid }))
                    };
                    let since = [d.current.as_ref(), d.other.as_ref()].iter().flatten().map(|v| v.ms).max().and_then(at);
                    json!({ "node": d.node, "kind": d.kind, "since": since, "current": ver(&d.current), "other": ver(&d.other), "base": d.base.as_deref().map(|b| s.render_text(b)) })
                })
                .collect();
            Ok(json!({ "items": items }))
        }
        // Close a text conflict: keep current | other | both (`top` stays on the node).
        "conflict.resolve" => {
            let id = resolve(shared, s(p, "id")?)?;
            let keep = s(p, "keep")?.to_string();
            let top = p.get("top").and_then(|v| v.as_str()).unwrap_or("current").to_string();
            write(shared, |b| b.resolve_text_conflict(&id, &keep, &top).map(|n| json!({ "id": id, "kept": keep, "new_note": n })))
        }
        // Nodes that mention this one (a page's LINKED FROM, editor.md §6.1), with their place.
        "backlinks" => {
            let id = resolve(shared, s(p, "id")?)?;
            let v = shared.vault.lock().unwrap();
            let nodes = v.store.nodes_where("n.deleted=0 AND n.id IN (SELECT src FROM edges WHERE rel='mention' AND dst=?1) ORDER BY n.updated_ms DESC LIMIT 50", &[&id])?;
            Ok(json!({ "count": nodes.len(), "items": nodes.iter().map(|n| node_json(&v.store, n)).collect::<Vec<_>>() }))
        }
        "search" => {
            let text = s(p, "text")?.to_string();
            let limit = p.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            let v = shared.vault.lock().unwrap();
            let nodes = v.store.search(&text, limit)?;
            Ok(json!({ "count": nodes.len(), "items": nodes.iter().map(|n| node_json(&v.store, n)).collect::<Vec<_>>() }))
        }
        // The log as transactions, newest first (mac-window.md §3 Log): who, how, when, what.
        "log" => {
            let limit = p.get("limit").and_then(|v| v.as_u64()).unwrap_or(60) as i64;
            let v = shared.vault.lock().unwrap();
            let pending: std::collections::HashSet<String> = thc_core::review::queue_txs(&v.store, None, None)?.into_iter().collect();
            let h = v.store.history_where(&format!("tx IN (SELECT tx FROM events GROUP BY tx ORDER BY max(okey) DESC LIMIT {limit}) ORDER BY okey DESC"), &[])?;
            let mut txs: Vec<Value> = Vec::new();
            for e in &h {
                if txs.last().and_then(|t| t["tx"].as_str()) != Some(e.tx.as_str()) {
                    txs.push(json!({ "tx": e.tx, "ms": e.ms, "actor": e.actor, "via": e.via, "dev": e.dev, "review": pending.contains(&e.tx), "ops": [] }));
                }
                let label = v.store.node(&e.entity).ok().flatten().map(|n| if n.title.is_some() { n.title.clone().unwrap_or_default() } else { v.store.render_text(&n.text) }).unwrap_or_default();
                if let Some(ops) = txs.last_mut().and_then(|t| t["ops"].as_array_mut()) {
                    if ops.len() < 6 {
                        ops.push(json!({ "op": e.op, "entity": e.entity, "label": label.chars().take(80).collect::<String>() }));
                    }
                }
            }
            Ok(json!({ "txs": txs }))
        }
        "pages" => {
            let v = shared.vault.lock().unwrap();
            Ok(json!({ "pages": thc_core::outline::pages(&v.store)? }))
        }
        "blocks" => {
            let root = resolve(shared, s(p, "root")?)?;
            let ids: Vec<String> = p.get("ids").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
            let v = shared.vault.lock().unwrap();
            let (blocks, gone) = thc_core::outline::render_ids(&v.store, &root, &ids)?;
            Ok(json!({ "root": root, "blocks": blocks, "gone": gone }))
        }
        // Blocks as Markdown (editor.md §3.2 copy): the whole document, or the given ids in
        // document order. The same lines `thc edit` writes.
        "markdown" => {
            let root = resolve(shared, s(p, "root")?)?;
            let ids: Option<std::collections::HashSet<String>> = p.get("ids").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect());
            let v = shared.vault.lock().unwrap();
            let mut blocks = thc_core::outline::render(&v.store, &root)?;
            if let Some(ids) = ids {
                blocks.retain(|b| ids.contains(&b.id));
            }
            Ok(json!({ "markdown": thc_core::outline::markdown(&blocks) }))
        }
        "blocks.apply" => {
            let ops: Vec<thc_core::outline::BlockOp> = serde_json::from_value(p.get("ops").cloned().ok_or_else(|| usage("missing \"ops\""))?)
                .map_err(|e| invalid(format!("bad block op: {e}")))?;
            let today = thc_core::dates::today();
            let mut v = shared.vault.lock().unwrap();
            // A journal day is created by its first save.
            let root = match (p.get("root").and_then(|x| x.as_str()), p.get("journal").and_then(|x| x.as_str())) {
                (Some(r), _) => v.store.resolve(r)?,
                (None, Some(d)) => {
                    let date = thc_core::dates::parse(d, today)?.date();
                    match v.store.journal_node(&date.format("%Y-%m-%d").to_string())? {
                        Some(id) => id,
                        None => v.transact(|st| {
                            let mut b = TxBuilder::new(st, today);
                            let id = b.journal(date)?;
                            Ok((b.finish(), id))
                        })?.1,
                    }
                }
                _ => return Err(usage("blocks.apply needs \"root\" or \"journal\"")),
            };
            let r = root.clone();
            let (events, mut results) = v.transact(move |st| thc_core::outline::plan(st, &r, &ops, today))?;
            thc_core::outline::refresh_revs(&v.store, &mut results);
            drop(v);
            shared.dirty.store(true, Ordering::SeqCst);
            Ok(json!({ "root": root, "tx": events.first().map(|e| e.tx.clone()), "events": events.len(), "results": results }))
        }
        m => Err(usage(format!("unknown method {m}"))),
    }
}

fn resolve(shared: &Arc<Shared>, id: &str) -> Result<String> {
    shared.vault.lock().unwrap().store.resolve(id)
}

/// A write on behalf of a socket client, under the device write lock (Vault::transact).
fn write(shared: &Arc<Shared>, f: impl FnOnce(&mut TxBuilder) -> Result<Value>) -> Result<Value> {
    let today = thc_core::dates::today();
    let mut v = shared.vault.lock().unwrap();
    let (events, out) = v.transact(|s| {
        let mut b = TxBuilder::new(s, today);
        let out = f(&mut b)?;
        Ok((b.finish(), out))
    })?;
    drop(v);
    shared.dirty.store(true, Ordering::SeqCst);
    let mut out = out;
    if let Value::Object(m) = &mut out {
        m.insert("tx".into(), json!(events.first().map(|e| e.tx.clone())));
        m.insert("events".into(), json!(events.len()));
    }
    Ok(out)
}

pub fn node_json(store: &thc_core::store::Store, n: &Node) -> Value {
    let mut v = json!({
        "id": n.id, "short": store.short(&n.id), "rev": store.rev(&n.id), "kind": n.kind(), "parent": n.parent, "title": n.title,
        "text": store.render_text(&n.text), "status": n.status, "scheduled": n.scheduled, "due": n.due,
        "priority": n.priority, "repeat": n.repeat, "done_at": n.done_at, "journal": n.journal,
        "tags": store.tags_of(&n.id).unwrap_or_default(), "created_by": n.created_by,
        "created": local(n.created_ms), "updated": local(n.updated_ms),
        "place": thc_core::review::place(store, n.parent.as_deref()),
    });
    if let Value::Object(m) = &mut v {
        m.retain(|_, val| !val.is_null());
    }
    v
}

/// Epoch ms as local `YYYY-MM-DDTHH:MM` (as the CLI's node JSON).
fn local(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%dT%H:%M").to_string()).unwrap_or_default()
}

/// Upcoming rows a panel shows before "+ N more" (mac-app.md §2.3).
pub const UPCOMING_SHOWN: usize = 8;

fn today_json(shared: &Arc<Shared>) -> Result<Value> {
    let v = shared.vault.lock().unwrap();
    let mut p = today_panel(&v.store)?;
    // ThoughtBar ignores contexts but shows this device's one in its footer (views.md §2.3).
    p["context"] = json!(thc_core::context::device(&v.paths.cache));
    Ok(p)
}

/// The `today` method's TodayPanel. `thc today --panel --json` prints the same thing.
pub fn today_panel(s: &thc_core::store::Store) -> Result<Value> {
    let t = thc_core::dates::today().format("%Y-%m-%d").to_string();
    let horizon = (thc_core::dates::today() + chrono::Duration::days(7)).format("%Y-%m-%d").to_string();
    let open = "n.status IN ('todo','doing','waiting') AND n.deleted=0";
    let j = |nodes: Vec<Node>| nodes.iter().map(|n| node_json(s, n)).collect::<Vec<_>>();
    let overdue = s.nodes_where(&format!("{open} AND substr(n.due,1,10) < ?1 ORDER BY n.due"), &[&t])?;
    // ISO ranges (t1 = tomorrow, h1 = the day after the horizon) so each branch uses an index.
    let t1 = (thc_core::dates::today() + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
    let h1 = (thc_core::dates::today() + chrono::Duration::days(8)).format("%Y-%m-%d").to_string();
    let today = s.nodes_where(
        "n.deleted=0 AND n.is_tag=0 AND (n.status IS NULL OR n.status IN ('todo','doing','waiting')) AND NOT (n.due IS NOT NULL AND n.due < ?1) AND n.id IN ( \
           SELECT id FROM nodes WHERE status IN ('todo','doing','waiting') AND deleted=0 AND scheduled < ?2 \
           UNION SELECT id FROM nodes WHERE due >= ?1 AND due < ?2 \
           UNION SELECT id FROM nodes WHERE status IS NULL AND scheduled >= ?1 AND scheduled < ?2) ORDER BY n.scheduled",
        &[&t, &t1],
    )?;
    // In Today only because an alert fires today: they join today[], so the badge counts them
    // (views.md §3.3). Each node once.
    let mut today = today;
    let shown: std::collections::HashSet<String> = overdue.iter().chain(today.iter()).map(|n| n.id.clone()).collect();
    today.extend(s.alert_rows(thc_core::dates::today(), &shown)?.into_iter().map(|(n, _)| n));
    let in_today: std::collections::HashSet<String> = today.iter().map(|n| n.id.clone()).collect();
    let _ = &horizon;
    let upcoming = s.nodes_where(
        "n.deleted=0 AND n.is_tag=0 AND (n.status IS NULL OR n.status IN ('todo','doing','waiting')) AND n.id IN ( \
           SELECT id FROM nodes WHERE scheduled >= ?1 AND scheduled < ?2 \
           UNION SELECT id FROM nodes WHERE due >= ?1 AND due < ?2) ORDER BY coalesce(n.scheduled, n.due)",
        &[&t1, &h1],
    )?;
    // A row is in Today or Upcoming, never both.
    let upcoming: Vec<Node> = upcoming.into_iter().filter(|n| !in_today.contains(&n.id)).collect();
    let inbox = s.nodes_where("n.parent IS NULL AND n.title IS NULL AND n.journal IS NULL AND n.is_tag=0 AND n.deleted=0 ORDER BY n.created_ms DESC", &[])?;
    let pending = s.alerts_where("deleted=0 AND state IN ('pending','snoozed') ORDER BY fire_at", &[])?;
    let alerts: Vec<Value> = pending
        .iter()
        .map(|a| json!({ "id": a.id, "node": a.node, "fire_at": a.fire_at, "state": s.alert_state(a), "fired_at": s.fired_at(a) }))
        .collect();
    // The menu bar count (SPEC §6.5): overdue + today + fired, unacknowledged alerts,
    // each node once. Computed here so clients don't copy the rule.
    // "Today" is everything in today[]: due or scheduled today (mac-app.md §1), or an alert
    // firing today (views.md §3.3).
    let mut badge: std::collections::HashSet<String> = overdue.iter().chain(today.iter()).map(|n| n.id.clone()).collect();
    for a in pending.iter().filter(|a| s.fired_at(a).is_some()) {
        if s.node(&a.node)?.is_some_and(|n| !n.deleted && !matches!(n.status.as_deref(), Some("done" | "cancelled"))) {
            badge.insert(a.node.clone());
        }
    }
    let upcoming_more = upcoming.len().saturating_sub(UPCOMING_SHOWN);
    // ThoughtBar presets: views marked --bar, at most 3 (views.md §1.4).
    let today_date = thc_core::dates::today();
    let presets: Vec<Value> = if thc_core::views::page_exists(s)? {
        thc_core::views::list(s)?
            .into_iter()
            .filter(|v| v.bar)
            .take(3)
            .map(|v| json!({ "name": v.name, "title": v.title, "query": v.query, "count": s.query(&v.query, today_date, 1000).map(|n| n.len()).unwrap_or(0) }))
            .collect()
    } else {
        vec![]
    };
    let mut conflict_ids: Vec<String> = s.open_conflicts()?.into_iter().map(|c| c.1).collect();
    conflict_ids.sort();
    conflict_ids.dedup();
    Ok(json!({
        "date": t,
        "overdue": j(overdue),
        "today": j(today),
        "upcoming": j(upcoming),
        "inbox": { "count": inbox.len(), "latest": j(inbox.into_iter().take(5).collect()) },
        "alerts": alerts,
        "conflicts": s.open_conflicts()?.len(),
        "conflict_ids": conflict_ids,
        "to_review": thc_core::review::pending_count(s)?,
        "badge": badge.len(),
        "presets": presets,
        "upcoming_more": upcoming_more,
    }))
}
