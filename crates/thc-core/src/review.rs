//! The agent review queue (docs/design/agents.md §1): which agent transactions still wait for
//! a person's verdict, what each one changed, and a safe revert that keeps later edits.
//!
//! Verdicts live in the `reviews` table (replayed from `tx.review` / `tx.unreview`). A
//! transaction is in the queue when its actor is an agent, it has no verdict, and it isn't
//! itself a review or a revert (those carry review ops).

use crate::alerts::plain_title;
use crate::event::{Event, Op};
use crate::model::Node;
use crate::store::Store;
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::HashSet;

/// What happened to one node in a transaction. `fields` entries are `{from?, to?}` for values
/// and `{add?, remove?}` for sets (agents.md §1.5).
#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub node: String,
    pub short: String,
    /// `create` | `change` | `move` | `complete` | `delete` | `restore` | `alert`
    pub change: &'static str,
    pub text: String,
    pub fields: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<usize>,
}

/// A field someone changed after the transaction, so a revert keeps theirs.
#[derive(Clone, Debug, Serialize)]
pub struct Later {
    pub node: String,
    pub field: String,
    pub ms: i64,
    pub actor: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub n: usize,
    pub tx: String,
    pub short: String,
    pub actor: String,
    pub via: String,
    pub dev: String,
    pub ms: i64,
    pub later_changed_by_human: bool,
    pub conflict: bool,
    pub changes: Vec<Change>,
    pub later: Vec<Later>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
}

/// A batch in one line's worth: `[("create", 3), ("change", 1), ("complete", 1), ("move", 1)]`.
pub fn counts(changes: &[Change]) -> Vec<(&'static str, usize)> {
    ["create", "change", "complete", "move", "delete", "restore", "alert"]
        .into_iter()
        .map(|k| (k, changes.iter().filter(|c| c.change == k).count()))
        .filter(|(_, n)| *n > 0)
        .collect()
}

pub fn count_word(kind: &str) -> &'static str {
    match kind {
        "create" => "created",
        "complete" => "done",
        "move" => "moved",
        "delete" => "deleted",
        "restore" => "restored",
        "alert" => "alerts",
        _ => "changed",
    }
}

/// Links a batch added (`blocks`, `relates`, …), which the counts don't show as changes.
pub fn link_count(changes: &[Change]) -> usize {
    changes.iter().filter_map(|c| c.fields.get("links")).filter_map(|f| f.get("add")).filter_map(|a| a.as_array()).map(|a| a.len()).sum()
}

/// The display suffix of a transaction id (`R64K4H`).
pub fn tx_short(tx: &str) -> String {
    tx[tx.len().saturating_sub(6)..].to_string()
}

// `actor >= 'agent' AND actor < 'agenu'` is `LIKE 'agent%'` as a range the events_actor index
// serves (a LIKE scans every event: 27 ms at 20k events).
const QUEUE_SQL: &str = "SELECT tx FROM events WHERE actor >= 'agent' AND actor < 'agenu' AND (?1 IS NULL OR actor = 'agent:' || ?1)
  GROUP BY tx HAVING min(ms) >= ?2 AND sum(op IN ('tx.review','tx.unreview')) = 0
  AND tx NOT IN (SELECT tx FROM reviews) ORDER BY min(okey)";

/// Transactions waiting for review, oldest first.
pub fn queue_txs(store: &Store, by: Option<&str>, since_ms: Option<i64>) -> Result<Vec<String>> {
    let mut st = store.conn.prepare_cached(QUEUE_SQL)?;
    let v = st.query_map(params![by, since_ms.unwrap_or(0)], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(v)
}

pub fn pending_count(store: &Store) -> Result<usize> {
    let n: i64 = store.conn.query_row(&format!("SELECT count(*) FROM ({QUEUE_SQL})"), params![None::<String>, 0], |r| r.get(0))?;
    Ok(n as usize)
}

pub fn queue(store: &Store, by: Option<&str>, since_ms: Option<i64>) -> Result<Vec<Item>> {
    queue_txs(store, by, since_ms)?.iter().enumerate().map(|(i, tx)| item(store, tx, i + 1)).collect()
}

/// The ops that undo one transaction exactly. Undoing changes (not just verdicts) also marks
/// the transaction reverted, so it leaves the review queue; undoing that undo withdraws it.
pub fn undo_ops(store: &Store, tx: &str) -> Result<Vec<Op>> {
    let mut ops = store.tx_inverse(tx)?;
    if ops.iter().any(|o| !o.is_review()) {
        ops.push(Op::TxReview { txs: vec![tx.to_string()], verdict: "reverted".into() });
    }
    Ok(ops)
}

/// Agent data transactions by `by` since `since_ms` that haven't been reverted yet (bulk undo).
pub fn revertible_txs(store: &Store, by: &str, since_ms: i64) -> Result<Vec<String>> {
    let mut st = store.conn.prepare(
        "SELECT tx FROM events WHERE actor = 'agent:' || ?1 GROUP BY tx
         HAVING min(ms) >= ?2 AND sum(op IN ('tx.review','tx.unreview')) = 0
         AND tx NOT IN (SELECT tx FROM reviews WHERE verdict='reverted') ORDER BY min(okey)",
    )?;
    let v = st.query_map(params![by, since_ms], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(v)
}

struct Row {
    okey: String,
    event: Event,
    inverse: Vec<Op>,
}

fn rows(store: &Store, tx: &str) -> Result<Vec<Row>> {
    let mut st = store.conn.prepare_cached("SELECT okey, body, inverse FROM events WHERE tx=?1 ORDER BY okey")?;
    let raw: Vec<(String, String, String)> = st.query_map([tx], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    raw.into_iter()
        .map(|(okey, body, inv)| Ok(Row { okey, event: serde_json::from_str(&body)?, inverse: serde_json::from_str(&inv)? }))
        .collect()
}

/// Where a node lives, as a person would say it: `§ 2026-10-03`, `¶ Health`, a parent's text, `inbox`.
pub fn place(store: &Store, parent: Option<&str>) -> String {
    let Some(p) = parent else { return "inbox".into() };
    match store.node(p).ok().flatten() {
        Some(n) if n.journal.is_some() => format!("§ {}", n.journal.unwrap_or_default()),
        Some(n) if n.title.is_some() => format!("¶ {}", n.label()),
        Some(n) => plain_title(store, &n),
        None => "a deleted node".into(),
    }
}

fn rank(c: &str) -> u8 {
    match c {
        "change" => 0,
        "alert" => 1,
        "move" => 2,
        "complete" => 3,
        "delete" | "restore" => 4,
        _ => 5, // create
    }
}

fn alert_node(store: &Store, alert: &str) -> Option<String> {
    store.conn.query_row("SELECT node FROM alerts WHERE id=?1", [alert], |r| r.get(0)).optional().ok().flatten()
}

fn display_value(store: &Store, key: &str, v: &Value) -> Value {
    match (key, v) {
        ("repeat", Value::Object(o)) => o.get("text").cloned().unwrap_or(Value::Null),
        ("parent", Value::String(p)) => json!(place(store, Some(p))),
        _ => v.clone(),
    }
}

fn set_add(fields: &mut Map<String, Value>, key: &str, side: &str, val: String) {
    let e = fields.entry(key.to_string()).or_insert_with(|| json!({}));
    let arr = e.as_object_mut().unwrap().entry(side).or_insert_with(|| json!([]));
    arr.as_array_mut().unwrap().push(json!(val));
}

fn from_to(fields: &mut Map<String, Value>, key: &str, from: Option<Value>, to: Value) {
    let e = fields.entry(key.to_string()).or_insert_with(|| json!({}));
    let o = e.as_object_mut().unwrap();
    // Keep the first `from` (the value before the transaction) and the last `to`.
    if let Some(f) = from.filter(|f| !f.is_null()) {
        o.entry("from").or_insert(f);
    }
    if to.is_null() {
        o.remove("to");
    } else {
        o.insert("to".into(), to);
    }
}

/// Build one review item: per-node changes, later edits and conflict state.
pub fn item(store: &Store, tx: &str, n: usize) -> Result<Item> {
    let rows = rows(store, tx)?;
    let first = rows.first().map(|r| r.event.clone());
    let mut changes: Vec<Change> = Vec::new();
    let mut created: HashSet<String> = HashSet::new();
    let node_change = |changes: &mut Vec<Change>, id: &str, kind: &'static str| -> usize {
        if let Some(i) = changes.iter().position(|c| c.node == id) {
            if rank(kind) > rank(changes[i].change) {
                changes[i].change = kind;
            }
            return i;
        }
        let node = store.node(id).ok().flatten();
        changes.push(Change {
            node: id.to_string(),
            short: store.short(id),
            change: kind,
            text: node.as_ref().map(|n| n.label()).unwrap_or_default(),
            fields: Map::new(),
            children: None,
        });
        changes.len() - 1
    };
    for r in &rows {
        // An event that changed nothing (lost every field to a later write) has no inverse.
        let effective = !r.inverse.is_empty() || matches!(r.event.op, Op::NodeCreate { .. });
        if !effective {
            continue;
        }
        let inv_props = |id: &str| -> Map<String, Value> {
            r.inverse
                .iter()
                .find_map(|o| match o {
                    Op::NodeSet { id: i, props } if i == id => Some(props.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        };
        match &r.event.op {
            // A tag created as a side effect shows up as `tags +#q4` on the tagged node.
            Op::NodeCreate { props, .. } if props.get("tag") == Some(&Value::Bool(true)) => {}
            Op::NodeCreate { id, parent, props, .. } => {
                created.insert(id.clone());
                let i = node_change(&mut changes, id, "create");
                let f = &mut changes[i].fields;
                from_to(f, "parent", None, json!(place(store, parent.as_deref())));
                for (k, v) in props {
                    if k != "journal" {
                        from_to(f, k, None, display_value(store, k, v));
                    }
                }
            }
            Op::NodeText { id, text, .. } => {
                let from = r.inverse.iter().find_map(|o| match o {
                    Op::NodeText { text, .. } => Some(json!(text)),
                    _ => None,
                });
                let i = node_change(&mut changes, id, "change");
                from_to(&mut changes[i].fields, "text", if created.contains(id) { None } else { from }, json!(text));
            }
            Op::NodeSet { id, .. } | Op::NodeComplete { id, .. } | Op::NodeSkip { id, .. } => {
                let kind = if matches!(r.event.op, Op::NodeComplete { .. }) { "complete" } else { "change" };
                let i = node_change(&mut changes, id, kind);
                let old = inv_props(id);
                let new: Map<String, Value> = match &r.event.op {
                    Op::NodeSet { props, .. } => props.clone(),
                    _ => old.keys().map(|k| (k.clone(), store.get_field(id, k).unwrap_or(Value::Null))).collect(),
                };
                for (k, v) in new {
                    if !old.contains_key(&k) && !created.contains(id) {
                        continue; // lost to a later write
                    }
                    if kind == "complete" && k == "done_at" {
                        continue;
                    }
                    let from = old.get(&k).map(|f| display_value(store, &k, f));
                    from_to(&mut changes[i].fields, &k, if created.contains(id) { None } else { from }, display_value(store, &k, &v));
                }
            }
            Op::NodeMove { id, parent, .. } => {
                let from = r.inverse.iter().find_map(|o| match o {
                    Op::NodeMove { parent, .. } => Some(json!(place(store, parent.as_deref()))),
                    _ => None,
                });
                let kind = if created.contains(id) { "create" } else { "move" };
                let i = node_change(&mut changes, id, kind);
                from_to(&mut changes[i].fields, "parent", if created.contains(id) { None } else { from }, json!(place(store, parent.as_deref())));
            }
            Op::NodeDelete { id } => {
                let i = node_change(&mut changes, id, "delete");
                let kids: i64 = store.conn.query_row("SELECT count(*) FROM nodes WHERE parent=?1", [id], |r| r.get(0))?;
                changes[i].children = Some(kids as usize);
            }
            Op::NodeRestore { id } => {
                node_change(&mut changes, id, "restore");
            }
            Op::EdgeAdd { src, rel, dst } | Op::EdgeRemove { src, rel, dst } => {
                // Mentions follow the text, which the diff already shows.
                if rel == "mention" {
                    continue;
                }
                let side = if matches!(r.event.op, Op::EdgeAdd { .. }) { "add" } else { "remove" };
                let i = node_change(&mut changes, src, "change");
                if rel == "tag" {
                    let name = store.node(dst)?.map(|n| n.label()).unwrap_or_default();
                    set_add(&mut changes[i].fields, "tags", side, name);
                } else {
                    set_add(&mut changes[i].fields, "links", side, format!("{rel} {}", store.short(dst)));
                }
            }
            Op::AlertAdd { node, trigger, id } => {
                let fire: Option<String> = store.conn.query_row("SELECT fire_at FROM alerts WHERE id=?1", [id], |r| r.get(0)).optional()?.flatten();
                let i = node_change(&mut changes, node, "alert");
                let what = fire.or(trigger.at.clone()).or(trigger.offset.clone()).unwrap_or_default();
                from_to(&mut changes[i].fields, "alert", None, json!(what));
            }
            Op::AlertAck { id } | Op::AlertSnooze { id, .. } | Op::AlertRemove { id } => {
                let Some(node) = alert_node(store, id) else { continue };
                let i = node_change(&mut changes, &node, "alert");
                let what = match &r.event.op {
                    Op::AlertAck { .. } => "acknowledged".to_string(),
                    Op::AlertSnooze { until, .. } => format!("snoozed until {until}"),
                    _ => "removed".to_string(),
                };
                from_to(&mut changes[i].fields, "alert", None, json!(what));
            }
            Op::PropDefine { .. } | Op::TxReview { .. } | Op::TxUnreview { .. } => {}
        }
    }
    // A set to the value it already had changes nothing.
    for c in changes.iter_mut() {
        c.fields.retain(|_, f| f.get("from").is_none() || f.get("from") != f.get("to"));
    }
    changes.retain(|c| c.change != "change" || !c.fields.is_empty());
    let exclude: HashSet<String> = [tx.to_string()].into();
    let mut later = Vec::new();
    for r in &rows {
        later.extend(later_for(store, r, &exclude)?);
    }
    later.sort_by(|a, b| (&a.node, &a.field).cmp(&(&b.node, &b.field)));
    later.dedup_by(|a, b| a.node == b.node && a.field == b.field);
    let eids: Vec<String> = rows.iter().map(|r| r.event.eid.clone()).collect();
    let conflict = eids.iter().any(|eid| {
        store
            .conn
            .query_row("SELECT 1 FROM conflicts WHERE resolved=0 AND (winner_eid=?1 OR loser_eid=?1)", [eid], |_| Ok(()))
            .optional()
            .ok()
            .flatten()
            .is_some()
    });
    let f = first.unwrap_or_else(|| panic!("transaction {tx} has no events"));
    Ok(Item {
        n,
        tx: tx.to_string(),
        short: tx_short(tx),
        actor: f.actor.label(),
        via: f.via.clone(),
        dev: f.dev.clone(),
        ms: f.hlc.ms() as i64,
        later_changed_by_human: later.iter().any(|l| l.actor == "human" || l.actor.starts_with("human:")),
        conflict,
        changes,
        later,
        verdict: store.verdict(tx)?,
    })
}

/// The (entity, field) clocks an op writes. `None` field = every field of the entity.
fn touched(op: &Op) -> Vec<(String, Option<String>)> {
    match op {
        Op::NodeCreate { id, .. } => vec![(id.clone(), None)],
        Op::NodeText { id, .. } => vec![(id.clone(), Some("text".into()))],
        Op::NodeSet { id, props } => props.keys().map(|k| (id.clone(), Some(k.clone()))).collect(),
        Op::NodeMove { id, .. } => vec![(id.clone(), Some("pos".into()))],
        Op::NodeComplete { id, .. } | Op::NodeSkip { id, .. } => {
            ["status", "done_at", "scheduled", "due"].iter().map(|k| (id.clone(), Some(k.to_string()))).collect()
        }
        Op::NodeDelete { id } | Op::NodeRestore { id } => vec![(id.clone(), Some("del".into()))],
        Op::EdgeAdd { src, rel, dst } | Op::EdgeRemove { src, rel, dst } => vec![(src.clone(), Some(format!("edge:{rel}:{dst}")))],
        Op::AlertAdd { id, .. } | Op::AlertAck { id } | Op::AlertSnooze { id, .. } | Op::AlertRemove { id } => {
            vec![(id.clone(), Some("state".into()))]
        }
        Op::PropDefine { .. } | Op::TxReview { .. } | Op::TxUnreview { .. } => vec![],
    }
}

/// Clocks on `entity` (one field, or all) won by an event after `okey` outside `exclude`.
fn later_clocks(store: &Store, entity: &str, field: Option<&str>, okey: &str, exclude: &HashSet<String>) -> Result<Vec<Later>> {
    let mut st = store.conn.prepare_cached(
        "SELECT c.field, e.tx, e.actor, e.ms FROM clocks c JOIN events e ON e.okey = c.okey
         WHERE c.entity=?1 AND (?2 IS NULL OR c.field=?2) AND c.okey > ?3",
    )?;
    let rows: Vec<(String, String, String, i64)> =
        st.query_map(params![entity, field, okey], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
    let node = if store.node_exists(entity)? { entity.to_string() } else { alert_node(store, entity).unwrap_or_else(|| entity.to_string()) };
    Ok(rows
        .into_iter()
        .filter(|(_, tx, _, _)| !exclude.contains(tx))
        .map(|(field, _, actor, ms)| Later { node: node.clone(), field: if field == "pos" { "parent".into() } else { field }, ms, actor })
        .collect())
}

fn later_for(store: &Store, r: &Row, exclude: &HashSet<String>) -> Result<Vec<Later>> {
    let mut out = Vec::new();
    for (entity, field) in touched(&r.event.op) {
        out.extend(later_clocks(store, &entity, field.as_deref(), &r.okey, exclude)?);
    }
    Ok(out)
}

/// A part of a revert that was left alone.
#[derive(Clone, Debug, Serialize)]
pub struct Skip {
    pub tx: String,
    pub node: String,
    pub field: String,
    pub ms: i64,
    pub actor: String,
    /// `changed_later` (reported) | `deleted` (the node was deleted afterwards; silent)
    pub reason: &'static str,
}

pub struct RevertPlan {
    /// Inverse ops (newest first) followed by one `tx.review … reverted`.
    pub ops: Vec<Op>,
    pub skipped: Vec<Skip>,
}

fn deleted_later(store: &Store, id: &str, okey: &str, exclude: &HashSet<String>) -> Result<bool> {
    let del = store.node(id)?.is_some_and(|n: Node| n.deleted);
    Ok(del && !later_clocks(store, id, Some("del"), okey, exclude)?.is_empty())
}

/// Revert `txs` together, skipping fields someone changed afterwards (agents.md §1.4).
/// Changes made by the transactions being reverted don't count as "afterwards".
pub fn plan_revert(store: &Store, txs: &[String], force: bool) -> Result<RevertPlan> {
    let exclude: HashSet<String> = txs.iter().cloned().collect();
    let mut all: Vec<(String, Row)> = Vec::new();
    for tx in txs {
        for r in rows(store, tx)? {
            all.push((tx.clone(), r));
        }
    }
    all.sort_by(|a, b| b.1.okey.cmp(&a.1.okey));
    let mut ops = Vec::new();
    let mut skipped = Vec::new();
    for (tx, r) in &all {
        for inv in r.inverse.iter().rev() {
            if force || inv.is_review() {
                ops.push(inv.clone());
                continue;
            }
            let target = match inv {
                Op::AlertAdd { node, .. } => node.clone(),
                other => other.entity(),
            };
            let is_del_op = matches!(inv, Op::NodeDelete { .. } | Op::NodeRestore { .. });
            if !is_del_op && store.node_exists(&target)? && deleted_later(store, &target, &r.okey, &exclude)? {
                skipped.push(Skip { tx: tx.clone(), node: target, field: "deleted".into(), ms: 0, actor: String::new(), reason: "deleted" });
                continue;
            }
            // Reverting a create deletes the node: only if nobody touched it since.
            let check: Vec<(String, Option<String>)> = match (&r.event.op, inv) {
                (Op::NodeCreate { id, .. }, Op::NodeDelete { .. }) => vec![(id.clone(), None)],
                _ => touched(inv),
            };
            let mut later = Vec::new();
            for (entity, field) in &check {
                later.extend(later_clocks(store, entity, field.as_deref(), &r.okey, &exclude)?);
            }
            if later.is_empty() {
                ops.push(inv.clone());
                continue;
            }
            match inv {
                Op::NodeSet { id, props } => {
                    let blocked: HashSet<&str> = later.iter().map(|l| l.field.as_str()).collect();
                    let keep: Map<String, Value> = props.iter().filter(|(k, _)| !blocked.contains(k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
                    if !keep.is_empty() {
                        ops.push(Op::NodeSet { id: id.clone(), props: keep });
                    }
                }
                _ => {}
            }
            for l in later {
                skipped.push(Skip { tx: tx.clone(), node: l.node, field: l.field, ms: l.ms, actor: l.actor, reason: "changed_later" });
            }
        }
    }
    if !ops.is_empty() || !txs.is_empty() {
        ops.push(Op::TxReview { txs: txs.to_vec(), verdict: "reverted".into() });
    }
    skipped.sort_by(|a, b| (&a.node, &a.field).cmp(&(&b.node, &b.field)));
    skipped.dedup_by(|a, b| a.node == b.node && a.field == b.field);
    Ok(RevertPlan { ops, skipped })
}


// ---- node diff and rewind (agents.md §6) ------------------------------------------------------

/// Where a diff starts: events with `okey > key` (or `>=` when `inclusive`) count as changes.
#[derive(Clone, Debug)]
pub struct Cut {
    pub key: String,
    pub inclusive: bool,
    /// The moment the cut stands for, for display.
    pub ms: i64,
}

impl Cut {
    /// Everything after local wall time `ms` (inclusive of that millisecond).
    pub fn at_ms(ms: i64) -> Cut {
        Cut { key: format!("{ms:013}.~"), inclusive: false, ms }
    }

    /// Right after transaction `tx` (`before`: right before it).
    pub fn tx(store: &Store, tx: &str, before: bool) -> Result<Option<Cut>> {
        let agg = if before { "min" } else { "max" };
        let row: Option<(Option<String>, Option<i64>)> =
            store.conn.query_row(&format!("SELECT {agg}(okey), {agg}(ms) FROM events WHERE tx=?1"), [tx], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        Ok(match row {
            Some((Some(key), Some(ms))) => Some(Cut { key, inclusive: before, ms }),
            _ => None,
        })
    }
}

/// One field's net change since the cut. Values are raw (as stored); `parent` is a node id.
#[derive(Clone, Debug, Serialize)]
pub struct FieldDiff {
    pub key: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub from: Value,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub to: Value,
    /// For tags and links: `rel dst` pairs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<(String, String)>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<(String, String)>,
    pub edits: usize,
    pub actor: String,
    pub ms: i64,
    pub tx: String,
    /// The order key needed to put a node back in place (`parent` only).
    #[serde(skip)]
    pub order: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Stamp {
    pub actor: String,
    pub ms: i64,
    pub tx: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct NodeDiff {
    pub node: String,
    pub fields: Vec<FieldDiff>,
    /// The node was created after the cut.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<Stamp>,
    /// Deleted (or restored, `false`) since the cut, with who did it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<(bool, Stamp)>,
    pub children_added: usize,
    pub children_removed: usize,
    pub txs: Vec<String>,
}

/// Display order (agents.md §6): text/title, status, dates, priority, repeat, place, tags, links, props.
pub fn field_rank(k: &str) -> (u8, String) {
    let r = match k {
        "text" => 0,
        "title" => 1,
        "status" => 2,
        "scheduled" => 3,
        "due" => 4,
        "priority" => 5,
        "repeat" => 6,
        "parent" => 7,
        "tags" => 8,
        "links" => 9,
        _ => 10,
    };
    (r, k.to_string())
}

pub fn node_diff(store: &Store, id: &str, cut: &Cut) -> Result<NodeDiff> {
    let mut st = store.conn.prepare(
        "SELECT okey, body, inverse FROM events WHERE entity=?1 AND (okey > ?2 OR (?3 AND okey = ?2)) ORDER BY okey",
    )?;
    let raw: Vec<(String, String, String)> =
        st.query_map(params![id, cut.key, cut.inclusive], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    let mut fields: Vec<FieldDiff> = Vec::new();
    let mut edges: Vec<((String, String), bool, Stamp, usize)> = Vec::new(); // (rel,dst), existed before, last, edits
    let mut created = None;
    let mut deleted: Option<(bool, bool, Stamp)> = None; // (before, after, stamp)
    let mut txs: Vec<String> = Vec::new();
    for (_okey, body, inv) in raw {
        let e: Event = serde_json::from_str(&body)?;
        let inverse: Vec<Op> = serde_json::from_str(&inv)?;
        if inverse.is_empty() && !matches!(e.op, Op::NodeCreate { .. }) {
            continue;
        }
        let stamp = Stamp { actor: e.actor.label(), ms: e.hlc.ms() as i64, tx: e.tx.clone() };
        if !txs.contains(&e.tx) {
            txs.push(e.tx.clone());
        }
        let mut touch = |key: &str, from: Value, order: Option<String>| {
            match fields.iter_mut().find(|f| f.key == key) {
                Some(f) => {
                    f.edits += 1;
                    f.actor = stamp.actor.clone();
                    f.ms = stamp.ms;
                    f.tx = stamp.tx.clone();
                }
                None => fields.push(FieldDiff {
                    key: key.to_string(),
                    from,
                    to: Value::Null,
                    add: vec![],
                    remove: vec![],
                    edits: 1,
                    actor: stamp.actor.clone(),
                    ms: stamp.ms,
                    tx: stamp.tx.clone(),
                    order,
                }),
            }
        };
        for op in &inverse {
            match op {
                Op::NodeText { text, .. } => touch("text", json!(text), None),
                Op::NodeSet { props, .. } => {
                    for (k, v) in props {
                        touch(k, v.clone(), None);
                    }
                }
                Op::NodeMove { parent, order, .. } => touch("parent", json!(parent), Some(order.clone())),
                Op::NodeDelete { .. } if matches!(e.op, Op::NodeCreate { .. }) => {}
                Op::NodeDelete { .. } | Op::NodeRestore { .. } => {
                    let before = matches!(op, Op::NodeDelete { .. });
                    let b = deleted.as_ref().map(|d| d.0).unwrap_or(before);
                    deleted = Some((b, !before, stamp.clone()));
                }
                Op::EdgeAdd { rel, dst, .. } | Op::EdgeRemove { rel, dst, .. } => {
                    let existed = matches!(op, Op::EdgeAdd { .. });
                    match edges.iter_mut().find(|x| x.0 == (rel.clone(), dst.clone())) {
                        Some(x) => {
                            x.2 = stamp.clone();
                            x.3 += 1;
                        }
                        None => edges.push(((rel.clone(), dst.clone()), existed, stamp.clone(), 1)),
                    }
                }
                _ => {}
            }
        }
        if matches!(e.op, Op::NodeCreate { .. }) {
            created = Some(stamp.clone());
        }
    }
    // Net values: compare the value before the cut with now.
    let node = store.node(id)?;
    let current = |k: &str| -> Result<Value> {
        Ok(match k {
            "text" => json!(node.as_ref().map(|n| n.text.clone())),
            "parent" => json!(node.as_ref().and_then(|n| n.parent.clone())),
            _ => store.get_field(id, k)?,
        })
    };
    for f in fields.iter_mut() {
        f.to = current(&f.key)?;
    }
    fields.retain(|f| f.from != f.to);
    let now: HashSet<(String, String)> = store.edges_from(id)?.into_iter().collect();
    for kind in ["tags", "links"] {
        let mut fd: Option<FieldDiff> = None;
        for ((rel, dst), before, stamp, edits) in &edges {
            if (rel == "tag") != (kind == "tags") {
                continue;
            }
            let after = now.contains(&(rel.clone(), dst.clone()));
            if *before == after {
                continue;
            }
            let f = fd.get_or_insert_with(|| FieldDiff {
                key: kind.into(),
                from: Value::Null,
                to: Value::Null,
                add: vec![],
                remove: vec![],
                edits: 0,
                actor: stamp.actor.clone(),
                ms: 0,
                tx: String::new(),
                order: None,
            });
            f.edits += edits;
            if stamp.ms >= f.ms {
                f.actor = stamp.actor.clone();
                f.ms = stamp.ms;
                f.tx = stamp.tx.clone();
            }
            if after { f.add.push((rel.clone(), dst.clone())) } else { f.remove.push((rel.clone(), dst.clone())) }
        }
        fields.extend(fd);
    }
    fields.sort_by_key(|f| field_rank(&f.key));
    let kids = |sql: &str| -> Result<usize> {
        let n: i64 = store.conn.query_row(sql, params![id, cut.key, cut.inclusive], |r| r.get(0))?;
        Ok(n as usize)
    };
    let after = "(e.okey > ?2 OR (?3 AND e.okey = ?2))";
    let children_added = kids(&format!(
        "SELECT count(*) FROM events e JOIN nodes n ON n.id = e.entity WHERE e.op='node.create' AND json_extract(e.body,'$.parent') = ?1 AND n.parent = ?1 AND n.deleted = 0 AND {after}"
    ))?;
    let children_removed = kids(&format!(
        "SELECT count(DISTINCT e.entity) FROM events e JOIN nodes n ON n.id = e.entity WHERE e.op='node.delete' AND n.parent = ?1 AND n.deleted = 1 AND {after}"
    ))?;
    Ok(NodeDiff {
        node: id.to_string(),
        fields,
        created,
        deleted: deleted.filter(|(b, a, _)| b != a).map(|(_, a, s)| (a, s)),
        children_added,
        children_removed,
        txs,
    })
}

/// Ops that set a node back to how it was at the cut. Never deletes the node.
pub fn rewind_ops(store: &Store, d: &NodeDiff) -> Result<Vec<Op>> {
    let id = &d.node;
    let mut ops = Vec::new();
    if let Some((true, _)) = d.deleted {
        ops.push(Op::NodeRestore { id: id.clone() });
    }
    let mut props = Map::new();
    for f in &d.fields {
        match f.key.as_str() {
            "text" => {
                let base: Option<String> = store.conn.query_row("SELECT text_eid FROM nodes WHERE id=?1", [id], |r| r.get(0)).optional()?.flatten();
                ops.push(Op::NodeText { id: id.clone(), text: f.from.as_str().unwrap_or_default().to_string(), base });
            }
            "parent" => ops.push(Op::NodeMove {
                id: id.clone(),
                parent: f.from.as_str().map(str::to_string),
                order: f.order.clone().unwrap_or_else(|| crate::ord::key_between(None, None)),
            }),
            "tags" | "links" => {
                for (rel, dst) in &f.add {
                    ops.push(Op::EdgeRemove { src: id.clone(), rel: rel.clone(), dst: dst.clone() });
                }
                for (rel, dst) in &f.remove {
                    ops.push(Op::EdgeAdd { src: id.clone(), rel: rel.clone(), dst: dst.clone() });
                }
            }
            k => {
                props.insert(k.to_string(), f.from.clone());
            }
        }
    }
    if !props.is_empty() {
        ops.push(Op::NodeSet { id: id.clone(), props });
    }
    Ok(ops)
}
