//! Mechanical session usage collection. Session files and local snapshots are writer-side
//! inputs; replay never reads them. Minute samples live in SQLite, not the event log.
use crate::{
    builder::TxBuilder,
    event::Actor,
    status::{self, Tokens},
    store::Store,
    vault::Vault,
};
use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io::BufRead,
    path::{Path, PathBuf},
};

pub const CHECKPOINT_MS: i64 = 3_600_000;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Input {
    pub id: String,
    pub started: i64,
    pub done: Option<i64>,
    pub sessions: Vec<String>,
}
impl Input {
    pub fn signature(&self) -> String {
        serde_json::to_string(self).expect("usage input serializes")
    }
    pub fn props(&self, tokens: &Tokens, now: i64) -> Vec<(String, String)> {
        vec![
            ("tokens_in".into(), tokens.input.to_string()),
            ("tokens_out".into(), tokens.out.to_string()),
            ("tokens_cache".into(), tokens.cache.to_string()),
            ("tokens_source".into(), "transcript".into()),
            ("tokens_collected_at".into(), now.to_string()),
            ("tokens_window".into(), self.signature()),
        ]
    }
}

/// Session identities on the task and its direct claim notes. Unsafe file names are ignored.
pub fn input(store: &Store, id: &str) -> Result<Option<Input>> {
    let Some(n) = store.node(id)? else { return Ok(None) };
    if n.deleted || !matches!(n.status.as_deref(), Some("doing" | "done")) {
        return Ok(None);
    }
    let t = status::times(store, &[id.to_string()])?.remove(id).unwrap_or_default();
    let Some(started) = t.started else { return Ok(None) };
    let mut sessions = Vec::new();
    let mut add = |props: serde_json::Map<String, Value>| {
        if let Some(s) = props.get("session_id").and_then(Value::as_str).filter(|s| valid_session(s)) {
            if !sessions.iter().any(|v| v == s) {
                sessions.push(s.to_string());
            }
        }
    };
    add(store.props_of(id)?);
    for child in store.children(id)? {
        add(store.props_of(&child.id)?);
    }
    sessions.sort();
    if sessions.is_empty() {
        return Ok(None);
    }
    Ok(Some(Input { id: id.to_string(), started, done: t.done, sessions }))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub tokens: Tokens,
    pub collected_ms: i64,
}

pub fn cached(store: &Store, id: &str) -> Result<Option<Snapshot>> {
    let row: Option<(String, String, i64)> = store
        .conn
        .query_row("SELECT signature, tokens, collected_ms FROM local_tokens WHERE node=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let Some((signature, tokens, collected_ms)) = row else { return Ok(None) };
    if input(store, id)?.is_none_or(|i| i.signature() != signature) {
        return Ok(None);
    }
    Ok(serde_json::from_str(&tokens).ok().map(|tokens| Snapshot { id: id.into(), tokens, collected_ms }))
}

/// Completed work uses its signed final props. Live work uses the freshest available sample.
pub fn reported(store: &Store, id: &str, props: &serde_json::Map<String, Value>) -> Result<Option<Tokens>> {
    if store.node(id)?.is_some_and(|n| n.status.as_deref() == Some("doing")) {
        if let Some(s) = cached(store, id)? {
            if s.collected_ms >= number(props.get("tokens_collected_at")).unwrap_or(0) as i64 {
                return Ok(Some(s.tokens));
            }
        }
    }
    if props.get("tokens_source").and_then(Value::as_str) == Some("transcript") {
        if let Some(window) = props.get("tokens_window").and_then(Value::as_str) {
            if input(store, id)?.is_none_or(|i| i.signature() != window) {
                return Ok(None);
            }
        }
    }
    Ok(Tokens::of(props))
}

/// Completion must remain successful even when logs are absent or the collector is refused.
pub fn after_done(vault: &mut Vault, ids: &[String], home: &Path, now: i64) -> Result<()> {
    for id in ids {
        let Some(i) = input(&vault.store, id)?.filter(|i| i.done.is_some()) else { continue };
        if let Some(tokens) = collect(home, &i, now) {
            apply(vault, &i, &tokens, now)?;
        }
    }
    Ok(())
}

pub fn snapshots(store: &Store) -> Result<Vec<Snapshot>> {
    let mut st = store.conn.prepare("SELECT node FROM local_tokens ORDER BY node")?;
    let ids = st.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
    ids.iter().filter_map(|id| cached(store, id).transpose()).collect()
}

/// Active tasks plus final reconciliation. Allow a minute for the last log flush after done.
pub fn pending(store: &Store) -> Result<Vec<Input>> {
    let nodes = store.query("is:task (status:doing or status:done)", crate::dates::today(), 100_000)?;
    let mut out = Vec::new();
    for n in nodes {
        let Some(i) = input(store, &n.id)? else { continue };
        if i.done.is_some_and(|d| cached(store, &i.id).ok().flatten().is_some_and(|s| s.collected_ms >= d + 60_000)) {
            continue;
        }
        out.push(i);
    }
    Ok(out)
}

/// Save a local snapshot and, only at done or an hourly checkpoint, signed token props.
/// Recheck the work window under the vault's writer lock: a changed claim invalidates a sample.
pub fn apply(vault: &mut Vault, i: &Input, tokens: &Tokens, now: i64) -> Result<bool> {
    let actor = Actor { kind: "agent".into(), name: Some("collector".into()) };
    let allowed = crate::policy::Policy::load(&actor, false)?.check(&["set".into()], false).is_ok();
    let me = std::mem::replace(&mut vault.actor, actor);
    let result = vault.transact(|store| {
        if input(store, &i.id)?.as_ref() != Some(i) { return Ok((vec![], false)) }
        let props = store.props_of(&i.id)?;
        let last = number(props.get("tokens_collected_at")).map(|n| n as i64).unwrap_or(i.started);
        let changed = Tokens::of(&props).is_none_or(|t| (t.input, t.out, t.cache, t.source.as_deref()) != (tokens.input, tokens.out, tokens.cache, Some("transcript")));
        let final_window = props.get("tokens_window").and_then(Value::as_str) == Some(i.signature().as_str());
        let checkpoint = if i.done.is_some() { changed || !final_window } else { now.saturating_sub(last) >= CHECKPOINT_MS && changed };
        let mut b = TxBuilder::new(store, crate::dates::today());
        if allowed && checkpoint { b.set_props(&i.id, &i.props(tokens, now))?; }
        let previous = cached(store, &i.id)?;
        let changed = previous.is_none_or(|s| s.tokens != *tokens);
        store.conn.execute("INSERT INTO local_tokens(node,signature,tokens,collected_ms) VALUES(?1,?2,?3,?4) ON CONFLICT(node) DO UPDATE SET signature=excluded.signature,tokens=excluded.tokens,collected_ms=excluded.collected_ms",
            rusqlite::params![i.id, i.signature(), serde_json::to_string(tokens)?, now])?;
        Ok((b.finish(), changed))
    });
    vault.actor = me;
    result.map(|(_, changed)| changed)
}

fn number(v: Option<&Value>) -> Option<u64> {
    v.and_then(|v| v.as_u64().or_else(|| v.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)).or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}
fn valid_session(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn collect(home: &Path, i: &Input, now: i64) -> Option<Tokens> {
    let mut total = Tokens { source: Some("transcript".into()), ..Default::default() };
    for s in &i.sessions {
        if let Some((input, out, cache)) = usage(home, s, i.started, i.done.unwrap_or(now)) {
            total.input += input;
            total.out += out;
            total.cache += cache;
            total.sessions.push(s.clone());
        }
    }
    (!total.sessions.is_empty()).then_some(total)
}

/// Only usage fields are inspected; no agent calls or transcript-content extraction.
pub fn usage(home: &Path, session: &str, from: i64, to: i64) -> Option<(u64, u64, u64)> {
    if !valid_session(session) {
        return None;
    }
    let dir = home.join(".claude/projects");
    crate::sandbox::check(&dir);
    let claude = std::fs::read_dir(dir).ok().into_iter().flatten().flatten().map(|d| d.path().join(format!("{session}.jsonl"))).find(|p| p.is_file());
    if let Some(p) = claude {
        return claude_usage(&p, from, to);
    }
    let dir = home.join(".codex/sessions");
    crate::sandbox::check(&dir);
    codex_usage(&find_rollout(&dir, session)?, from, to)
}
fn timestamp(v: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(v["timestamp"].as_str()?).ok().map(|t| t.timestamp_millis())
}
fn reader(p: &Path) -> Option<std::io::BufReader<std::fs::File>> {
    crate::sandbox::check(p);
    Some(std::io::BufReader::new(std::fs::File::open(p).ok()?))
}
fn claude_usage(p: &Path, from: i64, to: i64) -> Option<(u64, u64, u64)> {
    let mut messages = HashMap::new();
    for (line_no, line) in reader(p)?.lines().map_while(std::result::Result::ok).enumerate() {
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if !timestamp(&v).is_some_and(|t| (from..=to).contains(&t)) {
            continue;
        }
        let u = &v["message"]["usage"];
        if !["input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"].iter().any(|k| number(u.get(*k)).is_some()) {
            continue;
        }
        let n = |k| number(u.get(k)).unwrap_or(0);
        let id = v["message"]["id"].as_str().map(str::to_string).unwrap_or_else(|| format!("line-{line_no}"));
        // Streaming blocks repeat a message, sometimes with later output totals.
        messages.insert(id, (n("input_tokens") + n("cache_creation_input_tokens"), n("output_tokens"), n("cache_read_input_tokens")));
    }
    if messages.is_empty() {
        return None;
    }
    Some(messages.values().fold((0, 0, 0), |(i, o, c), (x, y, z)| (i + x, o + y, c + z)))
}
fn find_rollout(dir: &Path, session: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        // Never follow directory symlinks (cycles or escape from the session tree).
        let kind = e.file_type().ok()?;
        if kind.is_dir() {
            if let Some(f) = find_rollout(&p, session) {
                return Some(f);
            }
        } else if kind.is_file() && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(&format!("-{session}.jsonl"))) {
            return Some(p);
        }
    }
    None
}
fn codex_usage(p: &Path, from: i64, to: i64) -> Option<(u64, u64, u64)> {
    let mut previous: Option<(u64, u64, u64)> = None;
    let mut seen = HashSet::new();
    let (mut total, mut found) = ((0, 0, 0), false);
    for line in reader(p)?.lines().map_while(std::result::Result::ok) {
        if !line.contains("\"token_count\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(at) = timestamp(&v) else { continue };
        if at > to {
            continue;
        }
        let info = &v["payload"]["info"];
        let tuple = |u: &Value| {
            let n = |k| number(u.get(k)).unwrap_or(0);
            (n("input_tokens").saturating_sub(n("cached_input_tokens")), n("output_tokens"), n("cached_input_tokens"))
        };
        let has_usage = |u: &Value| ["input_tokens", "output_tokens", "cached_input_tokens"].iter().any(|k| number(u.get(*k)).is_some());
        let cumulative = has_usage(&info["total_token_usage"]).then(|| tuple(&info["total_token_usage"]));
        let last = has_usage(&info["last_token_usage"]).then(|| tuple(&info["last_token_usage"]));
        if cumulative.is_none() && !seen.insert((at, last)) {
            continue;
        }
        let delta = match (previous, cumulative) {
            (Some(a), Some(b)) if b.0 >= a.0 && b.1 >= a.1 && b.2 >= a.2 => Some((b.0 - a.0, b.1 - a.1, b.2 - a.2)),
            _ => last,
        };
        if let Some(c) = cumulative {
            previous = Some(c);
        }
        if at < from {
            continue;
        }
        if let Some((i, o, c)) = delta {
            total.0 += i;
            total.1 += o;
            total.2 += c;
            found = true;
        }
    }
    found.then_some(total)
}
