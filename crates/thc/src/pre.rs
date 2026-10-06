//! Write preconditions (docs/design/agents.md §3): `--if-match <rev>` and `--expect field=value`.
//! Checked under the write lock, after catching up, so a passing check and the write are atomic.

use anyhow::Result;
use serde_json::{Value, json};
use std::collections::HashSet;
use thc_core::error::{ThcError, invalid, usage};
use thc_core::event::Op;
use thc_core::review::{self, Cut};
use thc_core::store::Store;

#[derive(Clone, Debug, Default)]
pub struct Pre {
    pub if_match: Option<String>,
    pub expect: Vec<String>,
}

impl Pre {
    pub fn is_empty(&self) -> bool {
        self.if_match.is_none() && self.expect.is_empty()
    }
}

/// Existing nodes the ops change, in order.
fn targets(ops: &[Op]) -> Vec<String> {
    let created: HashSet<&str> = ops
        .iter()
        .filter_map(|o| match o {
            Op::NodeCreate { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let mut out: Vec<String> = Vec::new();
    for o in ops {
        let id = match o {
            Op::NodeText { id, .. }
            | Op::NodeSet { id, .. }
            | Op::NodeMove { id, .. }
            | Op::NodeComplete { id, .. }
            | Op::NodeSkip { id, .. }
            | Op::NodeDelete { id }
            | Op::NodeRestore { id } => id,
            Op::EdgeAdd { src, .. } | Op::EdgeRemove { src, .. } => src,
            Op::AlertAdd { node, .. } => node,
            _ => continue,
        };
        if !created.contains(id.as_str()) && !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

fn words(v: &Value) -> String {
    match v {
        Value::Null => "—".into(),
        Value::String(s) => s.clone(),
        Value::Object(o) => o.get("text").and_then(|t| t.as_str()).map(str::to_string).unwrap_or_else(|| v.to_string()),
        other => other.to_string(),
    }
}

fn local(ms: i64) -> String {
    crate::out::ms_to_local(ms)
}

fn hhmm(ms: i64) -> String {
    local(ms).get(11..).unwrap_or_default().to_string()
}

pub fn check(store: &Store, ops: &[Op], pre: &Pre) -> Result<()> {
    // Nothing to write means nothing to guard.
    if pre.is_empty() || ops.is_empty() {
        return Ok(());
    }
    let targets = targets(ops);
    if targets.is_empty() {
        return Err(usage("--if-match and --expect guard writes that change an existing node"));
    }
    // The rev names its node, so a write that also touches others (subtasks) still works.
    let (node, since) = match &pre.if_match {
        Some(rev) => {
            let row: Option<(String, String, i64)> = store
                .conn
                .query_row("SELECT entity, okey, ms FROM events WHERE eid=?1", [rev.trim()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .ok();
            let Some((entity, okey, ms)) = row else {
                return Err(invalid(format!("unknown rev {rev} · copy it from the node's JSON \"rev\"")));
            };
            if !targets.contains(&entity) {
                return Err(invalid(format!("rev {rev} belongs to {}, which this write doesn't change", store.short(&entity))));
            }
            (entity, Some((rev.trim().to_string(), okey, ms)))
        }
        None => (targets[0].clone(), None),
    };
    let short = store.short(&node);
    let current = store.rev(&node);
    let hint = format!("read it again with thc show {short} --json");
    if let Some((rev, okey, ms)) = since {
        if current.as_deref() != Some(rev.as_str()) {
            let d = review::node_diff(store, &node, &Cut { key: okey, inclusive: false, ms })?;
            let mut changed = Vec::new();
            let mut parts = Vec::new();
            for f in d.fields.iter().filter(|f| f.key != "done_at") {
                let actor = f.actor.strip_prefix("agent:").unwrap_or(&f.actor).to_string();
                let key = if f.key == "parent" { "place" } else { f.key.as_str() };
                let mut c = json!({ "field": key, "actor": f.actor, "at": local(f.ms), "tx": f.tx });
                if f.key == "tags" || f.key == "links" {
                    let names = |v: &[(String, String)]| -> Vec<String> { v.iter().map(|(_, dst)| store.node(dst).ok().flatten().map(|n| n.label()).unwrap_or_default()).collect() };
                    let (add, remove) = (names(&f.add), names(&f.remove));
                    let mut bits: Vec<String> = add.iter().map(|t| format!("+{t}")).collect();
                    bits.extend(remove.iter().map(|t| format!("−{t}")));
                    parts.push(format!("{key} {} ({actor}, {})", bits.join(" "), hhmm(f.ms)));
                    c["add"] = json!(add);
                    c["remove"] = json!(remove);
                } else {
                    parts.push(format!("{key} {} → {} ({actor}, {})", words(&f.from), words(&f.to), hhmm(f.ms)));
                    c["from"] = f.from.clone();
                    c["to"] = f.to.clone();
                }
                changed.push(c);
            }
            if let Some((true, s)) = &d.deleted {
                parts.push(format!("deleted ({}, {})", s.actor.strip_prefix("agent:").unwrap_or(&s.actor), hhmm(s.ms)));
                changed.push(json!({ "field": "deleted", "from": false, "to": true, "actor": s.actor, "at": local(s.ms) }));
            }
            if parts.is_empty() {
                parts.push("edited (now back to what you read)".into());
            }
            return Err(ThcError::Stale {
                message: format!("changed since you read it: {short} {} · nothing written", parts.join(", ")),
                hint,
                node: node.clone(),
                rev: current,
                changed,
            }
            .into());
        }
    }
    for e in &pre.expect {
        let Some((k, want)) = e.split_once('=') else {
            return Err(usage(format!("--expect wants field=value, got {e:?}")));
        };
        let (k, want) = (k.trim(), want.trim());
        let found = match k {
            "text" => store.node(&node)?.map(|n| store.render_text(&n.text)).map(Value::String).unwrap_or(Value::Null),
            "parent" => store.node(&node)?.and_then(|n| n.parent).map(Value::String).unwrap_or(Value::Null),
            _ => store.get_field(&node, k)?,
        };
        let found_s = words(&found);
        let matches = match (&found, want) {
            (Value::Null, "" | "none" | "null" | "—") => true,
            (_, w) if k == "status" && w == "open" => matches!(found.as_str(), Some("todo" | "doing" | "waiting")),
            _ => found_s.eq_ignore_ascii_case(want),
        };
        if !matches {
            return Err(ThcError::Stale {
                message: format!("expected {k}={want}, found {found_s} · nothing written"),
                hint: hint.clone(),
                node: node.clone(),
                rev: current.clone(),
                changed: vec![json!({ "field": k, "expected": want, "found": found })],
            }
            .into());
        }
    }
    Ok(())
}
