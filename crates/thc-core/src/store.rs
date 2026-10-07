//! SQLite materialized view of the event log. Disposable: rebuilt by replaying the log.
//!
//! Merge rules (identical on every device because events apply in total order):
//! - every field (position, text, title, each property, deleted) is last-writer-wins by order key
//! - concurrent text edits from the same base keep the winner and record the loser in `conflicts`
//! - a move that would create a cycle is rejected and recorded as a conflict

use crate::dates::{DateVal, parse_duration};
use crate::event::{Event, NextDates, Op, Trigger};
use crate::hlc::Hlc;
use anyhow::{Context, Result};
use chrono::NaiveTime;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};
use std::path::Path;

/// Properties stored as indexed columns on `nodes`. Everything else lives in `props`.
pub const BUILTIN: [&str; 8] = ["title", "status", "scheduled", "due", "priority", "repeat", "done_at", "journal"];

const SCHEMA: &str = r#"
-- Local usage samples: never replayed or synced (signed props are coarse checkpoints).
CREATE TABLE IF NOT EXISTS local_tokens(node TEXT PRIMARY KEY, signature TEXT NOT NULL, tokens TEXT NOT NULL, collected_ms INTEGER NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS nodes(
  id TEXT PRIMARY KEY, parent TEXT, ord TEXT NOT NULL, title TEXT, text TEXT NOT NULL DEFAULT '',
  status TEXT, scheduled TEXT, due TEXT, priority TEXT, repeat TEXT, done_at TEXT, journal TEXT,
  is_tag INTEGER NOT NULL DEFAULT 0, text_eid TEXT,
  created_ms INTEGER NOT NULL, created_by TEXT NOT NULL, updated_ms INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0) STRICT;
CREATE TABLE IF NOT EXISTS props(node TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
  PRIMARY KEY(node, key)) STRICT;
CREATE TABLE IF NOT EXISTS prop_defs(key TEXT PRIMARY KEY, type TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS clocks(entity TEXT NOT NULL, field TEXT NOT NULL, okey TEXT NOT NULL,
  PRIMARY KEY(entity, field)) STRICT;
CREATE TABLE IF NOT EXISTS edges(src TEXT NOT NULL, rel TEXT NOT NULL, dst TEXT NOT NULL,
  PRIMARY KEY(src, rel, dst)) STRICT;
CREATE TABLE IF NOT EXISTS alerts(id TEXT PRIMARY KEY, node TEXT NOT NULL, at TEXT, offset TEXT, anchor TEXT,
  state TEXT NOT NULL DEFAULT 'pending', snooze_until TEXT, fire_at TEXT,
  deleted INTEGER NOT NULL DEFAULT 0) STRICT;
CREATE TABLE IF NOT EXISTS events(eid TEXT PRIMARY KEY, okey TEXT NOT NULL UNIQUE, ms INTEGER NOT NULL,
  dev TEXT NOT NULL, actor TEXT NOT NULL, via TEXT NOT NULL, tx TEXT NOT NULL, op TEXT NOT NULL,
  entity TEXT NOT NULL, body TEXT NOT NULL, inverse TEXT NOT NULL DEFAULT '[]') STRICT;
CREATE TABLE IF NOT EXISTS conflicts(id INTEGER PRIMARY KEY, node TEXT NOT NULL, field TEXT NOT NULL,
  winner_eid TEXT, loser_eid TEXT NOT NULL, loser_value TEXT, resolved INTEGER NOT NULL DEFAULT 0) STRICT;
CREATE TABLE IF NOT EXISTS reviews(tx TEXT PRIMARY KEY, verdict TEXT NOT NULL, actor TEXT NOT NULL,
  ms INTEGER NOT NULL, review_tx TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS cursors(file TEXT PRIMARY KEY, offset INTEGER NOT NULL) STRICT;
-- Derived, never from an op: a live node whose parent was deleted (on another device, while it
-- was added here) shows under its nearest live ancestor (`under`; NULL = the inbox) and is
-- flagged until the person decides (daemon.md §4.0a). See Store::rehome.
CREATE TABLE IF NOT EXISTS rehomed(node TEXT PRIMARY KEY, under TEXT, deleted_parent TEXT NOT NULL) STRICT;
CREATE INDEX IF NOT EXISTS rehomed_under ON rehomed(under);
CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(id UNINDEXED, title, text,
  tokenize='unicode61 remove_diacritics 2');
CREATE INDEX IF NOT EXISTS by_parent ON nodes(parent, ord);
CREATE INDEX IF NOT EXISTS open_by_sched ON nodes(scheduled)
  WHERE status IN ('todo','doing','waiting') AND deleted = 0;
CREATE INDEX IF NOT EXISTS open_by_due ON nodes(due)
  WHERE status IN ('todo','doing','waiting') AND deleted = 0;
CREATE INDEX IF NOT EXISTS by_journal ON nodes(journal) WHERE journal IS NOT NULL;
CREATE INDEX IF NOT EXISTS by_title ON nodes(title COLLATE NOCASE) WHERE title IS NOT NULL;
-- Any-status date lookups (today's rows, done today, date terms in queries) as ISO ranges.
CREATE INDEX IF NOT EXISTS by_due ON nodes(due) WHERE due IS NOT NULL;
CREATE INDEX IF NOT EXISTS by_sched ON nodes(scheduled) WHERE scheduled IS NOT NULL;
CREATE INDEX IF NOT EXISTS by_done ON nodes(done_at) WHERE done_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS edges_dst ON edges(dst, rel);
CREATE INDEX IF NOT EXISTS props_key ON props(key, node);
CREATE INDEX IF NOT EXISTS alerts_fire ON alerts(fire_at) WHERE deleted = 0;
CREATE INDEX IF NOT EXISTS events_entity ON events(entity, okey);
CREATE INDEX IF NOT EXISTS events_tx ON events(tx);
-- The review queue reads agents' transactions only (review.rs): a range on actor.
CREATE INDEX IF NOT EXISTS events_actor ON events(actor, tx);
-- Open tasks counted and filtered from the index alone (the TUI's counts, Today's Doing):
-- visiting each open note's page cost ~35 ms at 600 open tasks on a cold start.
CREATE INDEX IF NOT EXISTS open_by_status ON nodes(status, deleted, due, scheduled, updated_ms) WHERE status IN ('todo','doing','waiting');
"#;

const MATERIALIZED: [&str; 10] = ["nodes", "props", "prop_defs", "clocks", "edges", "alerts", "conflicts", "reviews", "nodes_fts", "rehomed"];

pub struct Store {
    pub conn: Connection,
}

/// A fingerprint of the schema text (FNV-1a folded to a positive i32), kept in
/// `PRAGMA user_version`: any change to SCHEMA re-runs it on the next open.
fn schema_fingerprint() -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in SCHEMA.bytes().chain(crate::alerts::LOCAL_SCHEMA.bytes()) {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    (h & 0x7fff_ffff).max(1) as i32
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let conn = Connection::open(path).with_context(|| format!("opening store {}", path.display()))?;
        // Per-connection settings only; WAL is persistent and set with the schema below.
        conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        let s = Store { conn };
        // Every read opens the store, so skip the schema batch (dozens of CREATE … IF NOT
        // EXISTS) when `user_version` already carries this schema's fingerprint (SPEC §8).
        let want = schema_fingerprint();
        let have: i32 = s.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if have != want {
            s.conn.execute_batch("PRAGMA journal_mode=WAL;")?;
            s.conn.execute_batch(SCHEMA)?;
            s.conn.execute_batch(crate::alerts::LOCAL_SCHEMA)?;
            s.conn.execute_batch(&format!("PRAGMA user_version={want};"))?;
        }
        Ok(s)
    }

    /// Planner statistics (sqlite_stat1), local to this cache like everything else here.
    /// Without them SQLite guesses, and with several partial indexes on `due` it can pick the
    /// all-dates one for an open-tasks query: `status:open due<=+3d` 5.4 ms with stats, 10.6
    /// without, at 50k nodes. Best effort: a failure here only costs speed.
    pub fn analyze(&self) {
        let _ = self.conn.execute_batch("ANALYZE;");
    }

    /// Re-analyze tables whose size changed a lot since the last ANALYZE (cheap otherwise).
    pub fn refresh_stats(&self) {
        // Short-lived CLI writers may never have queried the tables they grew. Include
        // those tables (0x10000), retaining SQLite's bounded analysis work (0x10).
        let _ = self.conn.execute_batch("PRAGMA optimize=0x10012;");
    }

    /// First stats for a store that grew past toy size without a rebuild. Two cheap lookups
    /// on every open; the ANALYZE itself runs once.
    pub fn ensure_stats(&self) {
        let has: bool = self.conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='sqlite_stat1')", [], |r| r.get(0)).unwrap_or(true);
        if has {
            return;
        }
        let big: bool = self.conn.query_row("SELECT coalesce(max(rowid), 0) >= 1000 FROM nodes", [], |r| r.get(0)).unwrap_or(false);
        if big {
            self.analyze();
        }
    }

    pub fn open_memory() -> Result<Store> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(crate::alerts::LOCAL_SCHEMA)?;
        Ok(Store { conn })
    }

    /// Drop all materialized state (events and cursors included) before a full replay.
    /// The nodes a batch of events may have re-homed or un-re-homed (Store::rehome): created or
    /// moved nodes, and the children of deleted or restored ones. Call after applying them.
    pub fn rehome_candidates(&self, events: &[crate::event::Event]) -> Vec<String> {
        use crate::event::Op;
        let mut out: Vec<String> = Vec::new();
        for e in events {
            match &e.op {
                Op::NodeCreate { id, .. } | Op::NodeMove { id, .. } => out.push(id.clone()),
                Op::NodeDelete { id } | Op::NodeRestore { id } => {
                    out.push(id.clone());
                    if let Ok(mut st) = self.conn.prepare_cached("SELECT id FROM nodes WHERE parent = ?1") {
                        if let Ok(rows) = st.query_map([id], |r| r.get::<_, String>(0)) {
                            out.extend(rows.flatten());
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Every live node whose parent is deleted (a full pass: after a rebuild, or once on a store
    /// that predates re-homing).
    pub fn rehome_all(&self) -> Result<()> {
        let ids: Vec<String> = {
            let mut st = self.conn.prepare("SELECT c.id FROM nodes c JOIN nodes p ON p.id = c.parent WHERE c.deleted = 0 AND p.deleted = 1")?;
            st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        self.rehome(&ids)?;
        self.set_meta("rehome", "1")
    }

    /// Re-home these nodes, and re-check every node already re-homed. A pure function of the
    /// store (which is a pure function of the events), so devices converge whatever order events
    /// arrived in: a live node whose parent exists but is deleted shows under its nearest live
    /// ancestor, with a `rehomed` conflict; anything else carries neither. A parent that hasn't
    /// arrived yet isn't a deletion.
    pub fn rehome(&self, candidates: &[String]) -> Result<()> {
        let mut ids: std::collections::BTreeSet<String> = candidates.iter().cloned().collect();
        {
            let mut st = self.conn.prepare_cached("SELECT node FROM rehomed")?;
            for r in st.query_map([], |r| r.get::<_, String>(0))? {
                ids.insert(r?);
            }
        }
        if ids.is_empty() {
            return Ok(());
        }
        let pos = |id: &str| -> Result<Option<(Option<String>, bool)>> {
            Ok(self.conn.query_row("SELECT parent, deleted FROM nodes WHERE id = ?1", [id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)? != 0))).optional()?)
        };
        for id in ids {
            let orphan = match pos(&id)? {
                Some((Some(p), false)) => match pos(&p)? {
                    Some((pp, true)) => {
                        // The nearest live ancestor (None: everything above is deleted).
                        let mut under = pp;
                        let mut guard = 0;
                        while let Some(a) = under.clone() {
                            guard += 1;
                            match pos(&a)? {
                                Some((_, false)) => break,
                                Some((up, true)) if guard < 10_000 => under = up,
                                _ => {
                                    under = None;
                                    break;
                                }
                            }
                        }
                        Some((p, under))
                    }
                    _ => None,
                },
                _ => None,
            };
            match orphan {
                Some((p, under)) => {
                    self.conn.execute(
                        "INSERT INTO rehomed(node, under, deleted_parent) VALUES(?1, ?2, ?3) ON CONFLICT(node) DO UPDATE SET under = excluded.under, deleted_parent = excluded.deleted_parent",
                        params![id, under, p],
                    )?;
                    let del: String = self
                        .conn
                        .query_row("SELECT eid FROM events WHERE entity = ?1 AND op = 'node.delete' ORDER BY okey DESC LIMIT 1", [&p], |r| r.get(0))
                        .optional()?
                        .unwrap_or_default();
                    let have: Option<(i64, Option<String>, String)> = self
                        .conn
                        .query_row("SELECT id, loser_value, loser_eid FROM conflicts WHERE node = ?1 AND field = 'rehomed'", [&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                        .optional()?;
                    match have {
                        Some((cid, v, e)) if v.as_deref() != Some(p.as_str()) || e != del => {
                            self.conn.execute("UPDATE conflicts SET loser_value = ?2, loser_eid = ?3, resolved = 0 WHERE id = ?1", params![cid, p, del])?;
                        }
                        Some(_) => {}
                        None => {
                            self.conn.execute("INSERT INTO conflicts(node, field, winner_eid, loser_eid, loser_value) VALUES(?1, 'rehomed', NULL, ?2, ?3)", params![id, del, p])?;
                        }
                    }
                }
                None => {
                    self.conn.execute("DELETE FROM rehomed WHERE node = ?1", [&id])?;
                    self.conn.execute("DELETE FROM conflicts WHERE node = ?1 AND field = 'rehomed'", [&id])?;
                }
            }
        }
        Ok(())
    }

    /// Where a node shows: its parent, or for a re-homed one, its nearest live ancestor.
    pub fn view_parent(&self, id: &str) -> Result<Option<String>> {
        if let Some(u) = self.conn.query_row("SELECT under FROM rehomed WHERE node = ?1", [id], |r| r.get::<_, Option<String>>(0)).optional()? {
            return Ok(u);
        }
        Ok(self.conn.query_row("SELECT parent FROM nodes WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten())
    }

    pub fn reset(&self) -> Result<()> {
        for t in MATERIALIZED {
            self.conn.execute(&format!("DELETE FROM {t}"), [])?;
        }
        self.conn.execute_batch("DELETE FROM events; DELETE FROM cursors; DELETE FROM meta;")?;
        Ok(())
    }

    pub fn meta(&self, k: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT v FROM meta WHERE k=?1", [k], |r| r.get(0)).optional()?)
    }

    /// Rewrite the store compactly (VACUUM) at most every `every_ms`, for the daemon when idle.
    /// A store that grew row by row interleaves its tables' pages, and a cold start then reads
    /// many of them: the TUI's first frame took 43 ms instead of 4 ms on a 5k-note store.
    /// Maintenance, not replay: the store is a cache, and nothing here is in the log.
    pub fn vacuum_if_due(&self, now_ms: i64, every_ms: i64) -> Result<bool> {
        let last = self.meta("vacuumed_ms")?.and_then(|v| v.parse::<i64>().ok());
        if last.is_some_and(|l| now_ms - l < every_ms) {
            return Ok(false);
        }
        self.conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
        self.set_meta("vacuumed_ms", &now_ms.to_string())?;
        Ok(true)
    }

    pub fn set_meta(&self, k: &str, v: &str) -> Result<()> {
        self.conn.execute("INSERT INTO meta(k,v) VALUES(?1,?2) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [k, v])?;
        Ok(())
    }

    pub fn max_okey(&self) -> Result<Option<String>> {
        self.meta("max_okey")
    }

    pub fn max_hlc(&self) -> Result<Hlc> {
        Ok(match self.meta("max_hlc")? {
            Some(s) => serde_json::from_str(&s)?,
            None => Hlc::default(),
        })
    }

    pub fn cursor(&self, file: &str) -> Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT offset FROM cursors WHERE file=?1", [file], |r| r.get::<_, i64>(0))
            .optional()?
            .unwrap_or(0) as u64)
    }

    /// Every log file's cursor: how far into it the store has read (`Vault::frontier`).
    pub fn cursors(&self) -> Result<std::collections::BTreeMap<String, u64>> {
        let mut st = self.conn.prepare_cached("SELECT file, offset FROM cursors WHERE offset > 0")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_cursor(&self, file: &str, offset: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO cursors(file,offset) VALUES(?1,?2) ON CONFLICT(file) DO UPDATE SET offset=excluded.offset",
            params![file, offset as i64],
        )?;
        Ok(())
    }

    pub fn has_event(&self, eid: &str) -> Result<bool> {
        Ok(self.conn.query_row("SELECT 1 FROM events WHERE eid=?1", [eid], |_| Ok(())).optional()?.is_some())
    }

    // ---- field clocks -------------------------------------------------------------------

    /// The order key of the event that last won `field` on `entity` (node fields, `pos`, `del`,
    /// `edge:<rel>:<dst>`, alert `state`, `review` on `tx:<id>`).
    pub fn clock(&self, entity: &str, field: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT okey FROM clocks WHERE entity=?1 AND field=?2", [entity, field], |r| r.get(0))
            .optional()?)
    }

    /// True (and records the clock) if an event with `okey` wins this field.
    fn claim(&self, entity: &str, field: &str, okey: &str) -> Result<bool> {
        if let Some(cur) = self.clock(entity, field)? {
            if cur.as_str() >= okey {
                return Ok(false);
            }
        }
        self.conn.execute(
            "INSERT INTO clocks(entity,field,okey) VALUES(?1,?2,?3) ON CONFLICT(entity,field) DO UPDATE SET okey=excluded.okey",
            [entity, field, okey],
        )?;
        Ok(true)
    }

    // ---- reads used during apply ----------------------------------------------------------

    /// The current verdict on a transaction (`accepted` | `reverted`), if any.
    pub fn verdict(&self, tx: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT verdict FROM reviews WHERE tx=?1", [tx], |r| r.get(0)).optional()?)
    }

    pub fn node_exists(&self, id: &str) -> Result<bool> {
        Ok(self.conn.query_row("SELECT 1 FROM nodes WHERE id=?1", [id], |_| Ok(())).optional()?.is_some())
    }

    pub fn get_field(&self, id: &str, key: &str) -> Result<Value> {
        if BUILTIN.contains(&key) {
            let v: Option<String> =
                self.conn.query_row(&format!("SELECT {key} FROM nodes WHERE id=?1"), [id], |r| r.get(0)).optional()?.flatten();
            return Ok(match (key, v) {
                (_, None) => Value::Null,
                ("repeat", Some(s)) => serde_json::from_str(&s).unwrap_or(Value::Null),
                (_, Some(s)) => Value::String(s),
            });
        }
        if key == "tag" {
            let t: Option<i64> = self.conn.query_row("SELECT is_tag FROM nodes WHERE id=?1", [id], |r| r.get(0)).optional()?;
            return Ok(if t == Some(1) { Value::Bool(true) } else { Value::Null });
        }
        let v: Option<String> =
            self.conn.query_row("SELECT value FROM props WHERE node=?1 AND key=?2", [id, key], |r| r.get(0)).optional()?;
        Ok(v.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null))
    }

    fn position(&self, id: &str) -> Result<Option<(Option<String>, String)>> {
        Ok(self.conn.query_row("SELECT parent, ord FROM nodes WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
    }

    fn is_ancestor_or_self(&self, maybe_ancestor: &str, id: &str) -> Result<bool> {
        let mut cur = Some(id.to_string());
        let mut guard = 0;
        while let Some(c) = cur {
            if c == maybe_ancestor {
                return Ok(true);
            }
            cur = self.conn.query_row("SELECT parent FROM nodes WHERE id=?1", [&c], |r| r.get(0)).optional()?.flatten();
            guard += 1;
            if guard > 10_000 {
                break;
            }
        }
        Ok(false)
    }

    fn write_field(&self, id: &str, key: &str, value: &Value) -> Result<()> {
        if BUILTIN.contains(&key) {
            let stored: Option<String> = match value {
                Value::Null => None,
                Value::String(s) => Some(s.clone()),
                other => Some(other.to_string()),
            };
            self.conn.execute(&format!("UPDATE nodes SET {key}=?2 WHERE id=?1"), params![id, stored])?;
        } else if key == "tag" {
            let flag = matches!(value, Value::Bool(true));
            self.conn.execute("UPDATE nodes SET is_tag=?2 WHERE id=?1", params![id, flag as i64])?;
        } else if value.is_null() {
            self.conn.execute("DELETE FROM props WHERE node=?1 AND key=?2", [id, key])?;
        } else {
            self.conn.execute(
                "INSERT INTO props(node,key,value) VALUES(?1,?2,?3) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value",
                params![id, key, value.to_string()],
            )?;
        }
        Ok(())
    }

    fn touch(&self, id: &str, ms: u64) -> Result<()> {
        self.conn.execute("UPDATE nodes SET updated_ms=max(updated_ms, ?2) WHERE id=?1", params![id, ms as i64])?;
        Ok(())
    }

    fn reindex_embedders(&self, id: &str) -> Result<()> {
        let mut st = self.conn.prepare_cached("SELECT src FROM edges WHERE dst=?1 AND rel='embed'")?;
        let ids = st.query_map([id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        for id in ids { self.reindex_fts(&id)?; }
        Ok(())
    }

    fn reindex_fts(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM nodes_fts WHERE id=?1", [id])?;
        let row: Option<(Option<String>, String)> = self
            .conn
            .query_row("SELECT title, text FROM nodes WHERE id=?1 AND deleted=0", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        if let Some((title, text)) = row {
            // Index link targets by their titles so `[[Atomic Habits]]` is findable as "habits".
            let mut rendered = self.render_text(&text);
            let mut st = self.conn.prepare_cached("SELECT json_extract(p.value,'$.text') FROM props p JOIN nodes image ON image.id=p.node AND image.deleted=0 WHERE p.key='ocr' AND json_type(p.value,'$.text')='text' AND (p.node=?1 OR p.node IN (SELECT dst FROM edges WHERE src=?1 AND rel='embed')) ORDER BY p.node")?;
            for text in st.query_map([id], |r| r.get::<_, String>(0))? {
                rendered.push('\n');
                rendered.push_str(&text?);
            }
            self.conn.execute(
                "INSERT INTO nodes_fts(id,title,text) VALUES(?1,?2,?3)",
                params![id, title.unwrap_or_default(), rendered],
            )?;
        }
        Ok(())
    }

    /// Recompute `fire_at` for a node's relative alerts (and re-arm them if the anchor moved).
    fn recompute_alerts(&self, node: &str) -> Result<()> {
        let rows: Vec<(String, Option<String>, Option<String>, Option<String>, Option<String>)> = {
            let mut st = self.conn.prepare("SELECT id, at, offset, anchor, fire_at FROM alerts WHERE node=?1 AND deleted=0")?;
            st.query_map([node], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<Result<_, _>>()?
        };
        for (id, at, offset, anchor, old_fire) in rows {
            let fire = compute_fire_at(self, node, at.as_deref(), offset.as_deref(), anchor.as_deref())?;
            if fire != old_fire {
                self.conn.execute(
                    "UPDATE alerts SET fire_at=?2, state=CASE WHEN at IS NULL THEN 'pending' ELSE state END, snooze_until=NULL WHERE id=?1",
                    params![id, fire],
                )?;
            }
        }
        Ok(())
    }

    // ---- apply ------------------------------------------------------------------------------

    /// Apply one event. Caller wraps batches in a transaction. Idempotent by `eid`.
    pub fn apply(&self, e: &Event) -> Result<()> {
        if self.has_event(&e.eid)? {
            return Ok(());
        }
        let okey = e.order_key();
        let ms = e.hlc.ms();
        let inverse = self.apply_op(e, &okey, ms).with_context(|| format!("applying {} {}", e.op.name(), e.eid))?;
        let body = serde_json::to_string(e)?;
        self.conn.execute(
            "INSERT INTO events(eid,okey,ms,dev,actor,via,tx,op,entity,body,inverse) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                e.eid,
                okey,
                ms as i64,
                e.dev,
                e.actor.label(),
                e.via,
                e.tx,
                e.op.name(),
                e.op.entity(),
                body,
                serde_json::to_string(&inverse)?
            ],
        )?;
        if self.max_okey()?.is_none_or(|m| okey > m) {
            self.set_meta("max_okey", &okey)?;
        }
        if e.hlc > self.max_hlc()? {
            self.set_meta("max_hlc", &serde_json::to_string(&e.hlc)?)?;
        }
        Ok(())
    }

    fn apply_op(&self, e: &Event, okey: &str, ms: u64) -> Result<Vec<Op>> {
        let mut inverse = Vec::new();
        match &e.op {
            Op::NodeCreate { id, parent, order, text, title, props } => {
                if self.node_exists(id)? {
                    return Ok(inverse);
                }
                self.conn.execute(
                    "INSERT INTO nodes(id,parent,ord,title,text,text_eid,created_ms,created_by,updated_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?7)",
                    params![id, parent, order, title, text, e.eid, ms as i64, e.actor.label()],
                )?;
                for f in ["pos", "text", "title", "del"] {
                    self.claim(id, f, okey)?;
                }
                for (k, v) in props {
                    if self.claim(id, k, okey)? {
                        self.write_field(id, k, v)?;
                    }
                }
                self.reindex_fts(id)?;
                self.reindex_embedders(id)?;
                self.recompute_alerts(id)?;
                inverse.push(Op::NodeDelete { id: id.clone() });
            }
            Op::NodeText { id, text, base } => {
                let Some((cur_text, cur_eid)) = self
                    .conn
                    .query_row("SELECT text, text_eid FROM nodes WHERE id=?1", [id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
                    })
                    .optional()?
                else {
                    return Ok(inverse);
                };
                let concurrent = base.is_some() && cur_eid.is_some() && base != &cur_eid;
                if self.claim(id, "text", okey)? {
                    if concurrent {
                        self.conn.execute(
                            "INSERT INTO conflicts(node,field,winner_eid,loser_eid,loser_value) VALUES(?1,'text',?2,?3,?4)",
                            params![id, e.eid, cur_eid, cur_text],
                        )?;
                    } else if base.is_some() {
                        // An edit made with full knowledge of the current text resolves open conflicts.
                        self.conn.execute("UPDATE conflicts SET resolved=1 WHERE node=?1 AND field='text'", [id])?;
                    }
                    self.conn.execute("UPDATE nodes SET text=?2, text_eid=?3 WHERE id=?1", params![id, text, e.eid])?;
                    self.touch(id, ms)?;
                    self.reindex_fts(id)?;
                    inverse.push(Op::NodeText { id: id.clone(), text: cur_text, base: None });
                } else if concurrent {
                    self.conn.execute(
                        "INSERT INTO conflicts(node,field,winner_eid,loser_eid,loser_value) VALUES(?1,'text',?2,?3,?4)",
                        params![id, cur_eid, e.eid, text],
                    )?;
                }
            }
            Op::NodeSet { id, props } => {
                if !self.node_exists(id)? {
                    return Ok(inverse);
                }
                let mut old = Map::new();
                let mut dates_changed = false;
                for (k, v) in props {
                    if self.claim(id, k, okey)? {
                        old.insert(k.clone(), self.get_field(id, k)?);
                        self.write_field(id, k, v)?;
                        dates_changed |= k == "scheduled" || k == "due";
                    }
                }
                if !old.is_empty() {
                    self.touch(id, ms)?;
                    if old.contains_key("title") || old.contains_key("ocr") {
                        self.reindex_fts(id)?;
                    }
                    if old.contains_key("ocr") { self.reindex_embedders(id)?; }
                    if dates_changed {
                        self.recompute_alerts(id)?;
                    }
                    inverse.push(Op::NodeSet { id: id.clone(), props: old });
                }
            }
            Op::NodeMove { id, parent, order } => {
                let Some((old_parent, old_ord)) = self.position(id)? else { return Ok(inverse) };
                if let Some(p) = parent {
                    if self.is_ancestor_or_self(id, p)? {
                        self.conn.execute(
                            "INSERT INTO conflicts(node,field,winner_eid,loser_eid,loser_value) VALUES(?1,'parent',NULL,?2,?3)",
                            params![id, e.eid, p],
                        )?;
                        return Ok(inverse);
                    }
                }
                if self.claim(id, "pos", okey)? {
                    // A move made after a rejected one (even to the same place) dismisses it.
                    self.conn.execute("UPDATE conflicts SET resolved=1 WHERE node=?1 AND field='parent'", [id])?;
                    self.conn.execute("UPDATE nodes SET parent=?2, ord=?3 WHERE id=?1", params![id, parent, order])?;
                    self.touch(id, ms)?;
                    inverse.push(Op::NodeMove { id: id.clone(), parent: old_parent, order: old_ord });
                }
            }
            Op::NodeComplete { id, at, next, .. } => {
                if !self.node_exists(id)? {
                    return Ok(inverse);
                }
                let mut set = Map::new();
                set.insert("done_at".into(), json!(at));
                match next {
                    Some(NextDates { scheduled, due }) => {
                        if let Some(s) = scheduled {
                            set.insert("scheduled".into(), json!(s));
                        }
                        if let Some(d) = due {
                            set.insert("due".into(), json!(d));
                        }
                        set.insert("status".into(), json!("todo"));
                    }
                    None => {
                        set.insert("status".into(), json!("done"));
                    }
                }
                inverse = self.apply_op(&Event { op: Op::NodeSet { id: id.clone(), props: set }, ..e.clone() }, okey, ms)?;
            }
            Op::NodeSkip { id, next, .. } => {
                let mut set = Map::new();
                if let Some(s) = &next.scheduled {
                    set.insert("scheduled".into(), json!(s));
                }
                if let Some(d) = &next.due {
                    set.insert("due".into(), json!(d));
                }
                inverse = self.apply_op(&Event { op: Op::NodeSet { id: id.clone(), props: set }, ..e.clone() }, okey, ms)?;
            }
            Op::NodeDelete { id } | Op::NodeRestore { id } => {
                let del = matches!(e.op, Op::NodeDelete { .. });
                let cur: Option<i64> = self.conn.query_row("SELECT deleted FROM nodes WHERE id=?1", [id], |r| r.get(0)).optional()?;
                let Some(cur) = cur else { return Ok(inverse) };
                if self.claim(id, "del", okey)? && (cur == 1) != del {
                    self.conn.execute("UPDATE nodes SET deleted=?2 WHERE id=?1", params![id, del as i64])?;
                    self.touch(id, ms)?;
                    self.reindex_fts(id)?;
                    self.reindex_embedders(id)?;
                    inverse.push(if del { Op::NodeRestore { id: id.clone() } } else { Op::NodeDelete { id: id.clone() } });
                }
            }
            Op::EdgeAdd { src, rel, dst } | Op::EdgeRemove { src, rel, dst } => {
                let add = matches!(e.op, Op::EdgeAdd { .. });
                let field = format!("edge:{rel}:{dst}");
                let exists = self
                    .conn
                    .query_row("SELECT 1 FROM edges WHERE src=?1 AND rel=?2 AND dst=?3", [src, rel, dst], |_| Ok(()))
                    .optional()?
                    .is_some();
                if self.claim(src, &field, okey)? && exists != add {
                    if add {
                        self.conn.execute("INSERT INTO edges(src,rel,dst) VALUES(?1,?2,?3)", [src, rel, dst])?;
                        inverse.push(Op::EdgeRemove { src: src.clone(), rel: rel.clone(), dst: dst.clone() });
                    } else {
                        self.conn.execute("DELETE FROM edges WHERE src=?1 AND rel=?2 AND dst=?3", [src, rel, dst])?;
                        inverse.push(Op::EdgeAdd { src: src.clone(), rel: rel.clone(), dst: dst.clone() });
                    }
                    if rel == "embed" { self.reindex_fts(src)?; }
                }
            }
            Op::AlertAdd { id, node, trigger } => {
                let exists = self.conn.query_row("SELECT 1 FROM alerts WHERE id=?1", [id], |_| Ok(())).optional()?.is_some();
                if exists {
                    return Ok(inverse);
                }
                let fire = compute_fire_at(self, node, trigger.at.as_deref(), trigger.offset.as_deref(), trigger.anchor.as_deref())?;
                self.conn.execute(
                    "INSERT INTO alerts(id,node,at,offset,anchor,fire_at) VALUES(?1,?2,?3,?4,?5,?6)",
                    params![id, node, trigger.at, trigger.offset, trigger.anchor, fire],
                )?;
                self.claim(id, "state", okey)?;
                inverse.push(Op::AlertRemove { id: id.clone() });
            }
            Op::AlertAck { id } | Op::AlertSnooze { id, .. } | Op::AlertRemove { id } => {
                let row: Option<(String, Option<String>, Option<String>, Option<String>, String, Option<String>)> = self
                    .conn
                    .query_row("SELECT node, at, offset, anchor, state, snooze_until FROM alerts WHERE id=?1", [id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
                    })
                    .optional()?;
                let Some((node, at, offset, anchor, _state, _snooze)) = row else { return Ok(inverse) };
                if !self.claim(id, "state", okey)? {
                    return Ok(inverse);
                }
                match &e.op {
                    Op::AlertAck { .. } => {
                        self.conn.execute("UPDATE alerts SET state='acked' WHERE id=?1", [id])?;
                    }
                    Op::AlertSnooze { until, .. } => {
                        self.conn.execute(
                            "UPDATE alerts SET state='snoozed', snooze_until=?2, fire_at=?2 WHERE id=?1",
                            params![id, until],
                        )?;
                    }
                    _ => {
                        self.conn.execute("UPDATE alerts SET deleted=1 WHERE id=?1", [id])?;
                        // Re-adding needs a fresh id; undo re-creates the alert with the same trigger.
                        inverse.push(Op::AlertAdd {
                            id: format!("{id}r"),
                            node,
                            trigger: Trigger { at, offset, anchor },
                        });
                    }
                }
            }
            Op::PropDefine { key, ty } => {
                self.conn.execute("INSERT OR IGNORE INTO prop_defs(key,type) VALUES(?1,?2)", [key, ty])?;
            }
            // Verdicts are last-writer-wins per reviewed tx. Only a person can accept; an agent's
            // `accepted` is ignored on replay as well as refused by the writer.
            Op::TxReview { txs, verdict } => {
                let valid = match verdict.as_str() {
                    "accepted" => e.actor.kind == "human",
                    "reverted" => true,
                    _ => false,
                };
                if !valid {
                    return Ok(inverse);
                }
                let mut unreview = Vec::new();
                for t in txs {
                    let prev = self.verdict(t)?;
                    if self.claim(&format!("tx:{t}"), "review", okey)? {
                        self.conn.execute(
                            "INSERT INTO reviews(tx,verdict,actor,ms,review_tx) VALUES(?1,?2,?3,?4,?5)
                             ON CONFLICT(tx) DO UPDATE SET verdict=excluded.verdict, actor=excluded.actor, ms=excluded.ms, review_tx=excluded.review_tx",
                            params![t, verdict, e.actor.label(), ms as i64, e.tx],
                        )?;
                        match prev {
                            None => unreview.push(t.clone()),
                            Some(p) if &p != verdict => inverse.push(Op::TxReview { txs: vec![t.clone()], verdict: p }),
                            Some(_) => {}
                        }
                    }
                }
                if !unreview.is_empty() {
                    inverse.push(Op::TxUnreview { txs: unreview });
                }
            }
            Op::TxUnreview { txs } => {
                for t in txs {
                    let prev = self.verdict(t)?;
                    if self.claim(&format!("tx:{t}"), "review", okey)? {
                        if let Some(p) = prev {
                            self.conn.execute("DELETE FROM reviews WHERE tx=?1", [t])?;
                            inverse.push(Op::TxReview { txs: vec![t.clone()], verdict: p });
                        }
                    }
                }
            }
        }
        Ok(inverse)
    }
}

/// Alert fire time: absolute `at`, or anchor date + offset (date-only anchors use 09:00).
pub fn compute_fire_at(
    store: &Store,
    node: &str,
    at: Option<&str>,
    offset: Option<&str>,
    anchor: Option<&str>,
) -> Result<Option<String>> {
    if let Some(at) = at {
        return Ok(Some(at.to_string()));
    }
    let anchor = anchor.unwrap_or("due");
    let field = if anchor == "scheduled" { "scheduled" } else { "due" };
    let Value::String(s) = store.get_field(node, field)? else { return Ok(None) };
    let Some(dv) = DateVal::from_stored(&s) else { return Ok(None) };
    let base = match dv {
        DateVal::DateTime(dt) => dt,
        DateVal::Date(d) => d.and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap()),
    };
    let off = match offset {
        Some(o) => parse_duration(o)?,
        None => chrono::Duration::zero(),
    };
    Ok(Some((base + off).format("%Y-%m-%dT%H:%M").to_string()))
}

#[cfg(test)]
mod vacuum_tests {
    use super::*;

    #[test]
    fn compaction_runs_at_most_once_per_interval() {
        let s = Store::open_memory().unwrap();
        assert!(s.vacuum_if_due(1_000, 100).unwrap(), "never compacted: runs");
        assert!(!s.vacuum_if_due(1_050, 100).unwrap(), "within the interval: skips");
        assert!(s.vacuum_if_due(1_200, 100).unwrap(), "past it: runs again");
    }
}
