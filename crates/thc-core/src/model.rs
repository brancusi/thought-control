//! Read-side models and queries over the materialized store.

use crate::error::{ThcError, not_found};
use crate::id::{self, ID_LEN, MIN_PREFIX, MIN_SHORT};
use crate::store::Store;
use anyhow::Result;
use rusqlite::{OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::{Map, Value};

pub const NODE_COLS: &str = "n.id, n.parent, n.ord, n.title, n.text, n.status, n.scheduled, n.due, n.priority, \
    n.repeat, n.done_at, n.journal, n.is_tag, n.created_ms, n.created_by, n.updated_ms, n.deleted";

/// Where a node goes among its siblings (`Store::neighbour_ords`).
#[derive(Clone, Copy, Debug)]
pub enum Place<'a> {
    After(&'a str),
    Before(&'a str),
    First,
    Last,
}

#[derive(Clone, Debug, Serialize)]
pub struct Node {
    pub id: String,
    pub parent: Option<String>,
    #[serde(skip)]
    pub ord: String,
    pub title: Option<String>,
    pub text: String,
    pub status: Option<String>,
    pub scheduled: Option<String>,
    pub due: Option<String>,
    pub priority: Option<String>,
    pub repeat: Option<Value>,
    pub done_at: Option<String>,
    pub journal: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_tag: bool,
    pub created_ms: i64,
    pub created_by: String,
    pub updated_ms: i64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

impl Node {
    pub fn from_row(r: &Row) -> rusqlite::Result<Node> {
        let repeat: Option<String> = r.get(9)?;
        Ok(Node {
            id: r.get(0)?,
            parent: r.get(1)?,
            ord: r.get(2)?,
            title: r.get(3)?,
            text: r.get(4)?,
            status: r.get(5)?,
            scheduled: r.get(6)?,
            due: r.get(7)?,
            priority: r.get(8)?,
            repeat: repeat.and_then(|s| serde_json::from_str(&s).ok()),
            done_at: r.get(10)?,
            journal: r.get(11)?,
            is_tag: r.get::<_, i64>(12)? == 1,
            created_ms: r.get(13)?,
            created_by: r.get(14)?,
            updated_ms: r.get(15)?,
            deleted: r.get::<_, i64>(16)? == 1,
        })
    }

    pub fn is_open(&self) -> bool {
        matches!(self.status.as_deref(), Some("todo" | "doing" | "waiting"))
    }

    /// Label for listings: title for pages, journal date, else text.
    pub fn label(&self) -> String {
        if let Some(t) = &self.title {
            return t.clone();
        }
        if let Some(j) = &self.journal {
            return j.clone();
        }
        self.text.lines().next().unwrap_or("").to_string()
    }

    pub fn kind(&self) -> &'static str {
        if self.is_tag {
            "tag"
        } else if self.journal.is_some() {
            "journal"
        } else if self.status.is_some() {
            "task"
        } else if self.parent.is_none() && self.title.is_some() {
            "page"
        } else if self.parent.is_none() {
            "inbox"
        } else {
            "block"
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Alert {
    pub id: String,
    pub node: String,
    pub at: Option<String>,
    pub offset: Option<String>,
    pub anchor: Option<String>,
    pub state: String,
    pub fire_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct ConflictVersion {
    pub text: String,
    pub dev: String,
    pub actor: String,
    pub ms: i64,
    pub eid: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct ConflictDetail {
    pub id: i64,
    pub node: String,
    /// `text`, `move` or `rehomed`
    pub kind: String,
    pub current: Option<ConflictVersion>,
    pub other: Option<ConflictVersion>,
    pub base: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HistoryEntry {
    pub eid: String,
    pub ms: i64,
    pub dev: String,
    pub actor: String,
    pub via: String,
    pub tx: String,
    pub op: String,
    pub entity: String,
    pub body: Value,
}

impl Store {
    pub fn node(&self, id: &str) -> Result<Option<Node>> {
        // Cached: a page's links each look their target up (vw384).
        Ok(self.conn.prepare_cached(&format!("SELECT {NODE_COLS} FROM nodes n WHERE n.id=?1"))?.query_row([id], Node::from_row).optional()?)
    }

    pub fn must_node(&self, id: &str) -> Result<Node> {
        self.node(id)?.ok_or_else(|| not_found(format!("node {id}")))
    }

    pub fn nodes_where(&self, where_sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Vec<Node>> {
        let sql = format!("SELECT {NODE_COLS} FROM nodes n WHERE {where_sql}");
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(params, Node::from_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The ords either side of a place among `parent`'s children (None: the top level),
    /// leaving `exclude` (the node being placed) out: after a sibling, before one, first or last.
    /// Two indexed lookups instead of loading every sibling: a 1,000-line paste into a
    /// 5,000-line page took 4.3 s that way.
    pub fn neighbour_ords(&self, parent: Option<&str>, exclude: &str, at: Place<'_>) -> Result<(Option<String>, Option<String>)> {
        let scope = if parent.is_some() { "parent = ?1" } else { "parent IS NULL AND ?1 IS NULL" };
        let one = |sql: &str, extra: &[&dyn rusqlite::ToSql]| -> Result<Option<String>> {
            let mut p: Vec<&dyn rusqlite::ToSql> = vec![&parent, &exclude];
            p.extend_from_slice(extra);
            Ok(self.conn.query_row(&format!("SELECT ord FROM nodes WHERE {scope} AND deleted = 0 AND id <> ?2 {sql} LIMIT 1"), p.as_slice(), |r| r.get(0)).optional()?)
        };
        let anchor = |id: &str| -> Result<(String, String)> {
            let row: Option<(String, String)> = self
                .conn
                .query_row(&format!("SELECT ord, id FROM nodes WHERE id = ?3 AND {scope} AND deleted = 0 AND id <> ?2"), rusqlite::params![parent, exclude, id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            row.ok_or_else(|| crate::error::invalid("after / before must be a sibling under the parent"))
        };
        Ok(match at {
            Place::After(a) => {
                let (ord, aid) = anchor(a)?;
                let next = one("AND (ord > ?3 OR (ord = ?3 AND id > ?4)) ORDER BY ord, id", &[&ord, &aid])?;
                (Some(ord), next)
            }
            Place::Before(b) => {
                let (ord, bid) = anchor(b)?;
                let prev = one("AND (ord < ?3 OR (ord = ?3 AND id < ?4)) ORDER BY ord DESC, id DESC", &[&ord, &bid])?;
                (prev, Some(ord))
            }
            Place::First => (None, one("ORDER BY ord, id", &[])?),
            Place::Last => (one("ORDER BY ord DESC, id DESC", &[])?, None),
        })
    }

    pub fn children(&self, id: &str) -> Result<Vec<Node>> {
        // With the nodes re-homed here (their parent was deleted elsewhere).
        self.nodes_where("(n.parent=?1 OR n.id IN (SELECT node FROM rehomed WHERE under=?1)) AND n.deleted=0 ORDER BY n.ord, n.id", &[&id])
    }

    pub fn descendants(&self, id: &str) -> Result<Vec<String>> {
        let mut st = self.conn.prepare(
            "WITH RECURSIVE d(id) AS (SELECT id FROM nodes WHERE parent=?1 UNION SELECT node FROM rehomed WHERE under=?1 \
             UNION SELECT n.id FROM d CROSS JOIN nodes n INDEXED BY by_parent ON n.parent=d.id UNION SELECT r.node FROM d CROSS JOIN rehomed r ON r.under=d.id) SELECT id FROM d",
        )?;
        let ids = st.query_map([id], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
        Ok(ids)
    }

    pub fn last_child_ord(&self, parent: Option<&str>) -> Result<Option<String>> {
        Ok(match parent {
            Some(p) => self.conn.query_row("SELECT max(ord) FROM nodes WHERE parent=?1", [p], |r| r.get(0))?,
            None => self.conn.query_row("SELECT max(ord) FROM nodes WHERE parent IS NULL", [], |r| r.get(0))?,
        })
    }

    pub fn tags_of(&self, id: &str) -> Result<Vec<String>> {
        let mut st = self.conn.prepare_cached(
            "SELECT t.title FROM edges e JOIN nodes t ON t.id=e.dst WHERE e.src=?1 AND e.rel='tag' AND t.title IS NOT NULL ORDER BY t.title",
        )?;
        let v = st.query_map([id], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
        Ok(v)
    }

    pub fn edges_from(&self, id: &str) -> Result<Vec<(String, String)>> {
        let mut st = self.conn.prepare("SELECT rel, dst FROM edges WHERE src=?1 ORDER BY rel, dst")?;
        let v = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    pub fn backlinks(&self, id: &str) -> Result<Vec<Node>> {
        self.nodes_where(
            "n.deleted=0 AND n.id IN (SELECT src FROM edges WHERE dst=?1 AND rel IN ('mention','relates','blocks')) ORDER BY n.updated_ms DESC",
            &[&id],
        )
    }

    /// The notes that show an attachment (its `embed` sources): "used in".
    pub fn embedders(&self, id: &str) -> Result<Vec<Node>> {
        self.nodes_where("n.deleted=0 AND n.id IN (SELECT src FROM edges WHERE dst=?1 AND rel='embed') ORDER BY n.updated_ms DESC", &[&id])
    }

    pub fn props_of(&self, id: &str) -> Result<Map<String, Value>> {
        let mut st = self.conn.prepare_cached("SELECT key, value FROM props WHERE node=?1 ORDER BY key")?;
        let mut m = Map::new();
        for row in st.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (k, v) = row?;
            m.insert(k, serde_json::from_str(&v).unwrap_or(Value::String(v)));
        }
        Ok(m)
    }

    pub fn prop_type(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT type FROM prop_defs WHERE key=?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn alerts_of(&self, id: &str) -> Result<Vec<Alert>> {
        self.alerts_where("node=?1 AND deleted=0 ORDER BY fire_at", &[&id])
    }

    pub fn alerts_where(&self, where_sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Vec<Alert>> {
        let mut st =
            self.conn.prepare(&format!("SELECT id, node, at, offset, anchor, state, fire_at FROM alerts WHERE {where_sql}"))?;
        let v = st
            .query_map(params, |r| {
                Ok(Alert {
                    id: r.get(0)?,
                    node: r.get(1)?,
                    at: r.get(2)?,
                    offset: r.get(3)?,
                    anchor: r.get(4)?,
                    state: r.get(5)?,
                    fire_at: r.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    pub fn journal_node(&self, date: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT id FROM nodes WHERE journal=?1 AND deleted=0 ORDER BY id LIMIT 1", [date], |r| r.get(0))
            .optional()?)
    }

    /// Children of every journal node for a date (duplicates can exist after concurrent creation).
    pub fn journal_entries(&self, date: &str) -> Result<Vec<Node>> {
        self.nodes_where(
            "n.deleted=0 AND n.parent IN (SELECT id FROM nodes WHERE journal=?1 AND deleted=0) ORDER BY n.ord, n.id",
            &[&date],
        )
    }

    pub fn find_root_by_title(&self, title: &str, tag: bool) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM nodes WHERE parent IS NULL AND deleted=0 AND title=?1 COLLATE NOCASE AND is_tag=?2 ORDER BY id LIMIT 1",
                params![title, tag as i64],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Resolve a user-supplied id or unique prefix (>= 4 chars). Never guesses.
    pub fn resolve(&self, input: &str) -> Result<String> {
        let p = id::normalize(input);
        if p.len() < MIN_PREFIX || !id::looks_like_id(&p) {
            return Err(ThcError::Usage(format!("{input:?} is not an id (need at least {MIN_PREFIX} chars)")).into());
        }
        let hi = format!("{p}~");
        let mut st = self.conn.prepare("SELECT id FROM nodes WHERE id >= ?1 AND id < ?2 ORDER BY id LIMIT 6")?;
        let ids = st.query_map([&p, &hi], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        match ids.len() {
            0 => {
                // Alerts are addressable too.
                let mut st = self.conn.prepare("SELECT id FROM alerts WHERE id >= ?1 AND id < ?2 LIMIT 6")?;
                let a = st.query_map([&p, &hi], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                match a.len() {
                    1 => Ok(a[0].clone()),
                    0 => Err(not_found(format!("no node starts with {input:?}"))),
                    _ => Err(ThcError::Ambiguous { prefix: p, candidates: a }.into()),
                }
            }
            1 => Ok(ids[0].clone()),
            _ => Err(ThcError::Ambiguous { prefix: p, candidates: ids.iter().map(|i| self.short(i)).collect() }.into()),
        }
    }

    /// Shortest unique prefix, at least MIN_SHORT chars.
    pub fn short(&self, id: &str) -> String {
        let Ok(mut st) = self.conn.prepare_cached("SELECT count(*) FROM nodes WHERE id >= ?1 AND id < ?2") else { return id.to_string() };
        for len in MIN_SHORT..ID_LEN {
            let p = &id[..len];
            let hi = format!("{p}~");
            let n: i64 = st
                .query_row([p, hi.as_str()], |r| r.get(0))
                .unwrap_or(2);
            if n <= 1 {
                return p.to_string();
            }
        }
        id.to_string()
    }

    pub fn history(&self, entity: &str, limit: usize) -> Result<Vec<HistoryEntry>> {
        self.history_where(
            "entity=?1 OR body LIKE ?2 ORDER BY okey DESC LIMIT ?3",
            &[&entity, &format!("%\"dst\":\"{entity}\"%"), &(limit as i64)],
        )
    }

    pub fn history_where(&self, where_sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Vec<HistoryEntry>> {
        let mut st =
            self.conn.prepare(&format!("SELECT eid, ms, dev, actor, via, tx, op, entity, body FROM events WHERE {where_sql}"))?;
        let v = st
            .query_map(params, |r| {
                let body: String = r.get(8)?;
                Ok(HistoryEntry {
                    eid: r.get(0)?,
                    ms: r.get(1)?,
                    dev: r.get(2)?,
                    actor: r.get(3)?,
                    via: r.get(4)?,
                    tx: r.get(5)?,
                    op: r.get(6)?,
                    entity: r.get(7)?,
                    body: serde_json::from_str(&body).unwrap_or(Value::Null),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    /// Open tasks that block this node (`blocks` edges from tasks that aren't done).
    pub fn open_blockers(&self, id: &str) -> Result<Vec<Node>> {
        self.nodes_where(
            "n.deleted=0 AND n.status IN ('todo','doing','waiting') AND n.id IN (SELECT src FROM edges WHERE dst=?1 AND rel='blocks') ORDER BY n.id",
            &[&id],
        )
    }

    /// The eid of the node's latest event: its revision, for `--if-match`.
    pub fn rev(&self, id: &str) -> Option<String> {
        self.conn
            .prepare_cached("SELECT eid FROM events WHERE entity=?1 ORDER BY okey DESC LIMIT 1")
            .ok()?
            .query_row([id], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn tx_inverse(&self, tx: &str) -> Result<Vec<crate::event::Op>> {
        let mut st = self.conn.prepare("SELECT inverse FROM events WHERE tx=?1 ORDER BY okey DESC")?;
        let mut ops = Vec::new();
        for inv in st.query_map([tx], |r| r.get::<_, String>(0))? {
            let v: Vec<crate::event::Op> = serde_json::from_str(&inv?)?;
            ops.extend(v);
        }
        Ok(ops)
    }

    pub fn open_conflicts(&self) -> Result<Vec<(i64, String, String, Option<String>)>> {
        let mut st = self.conn.prepare("SELECT id, node, field, loser_value FROM conflicts WHERE resolved=0 ORDER BY id")?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    /// Full details of open conflicts (daemon.md §4.2): current/other versions with device,
    /// actor and time, and the base text for text conflicts.
    pub fn conflict_details(&self, node: Option<&str>) -> Result<Vec<ConflictDetail>> {
        let mut st = self.conn.prepare(
            "SELECT id, node, field, winner_eid, loser_eid, loser_value FROM conflicts WHERE resolved=0 AND (?1 IS NULL OR node=?1) ORDER BY id",
        )?;
        let rows: Vec<(i64, String, String, Option<String>, String, Option<String>)> =
            st.query_map([node], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?.collect::<Result<_, _>>()?;
        let mut out = Vec::new();
        for (cid, node, field, winner, loser, loser_value) in rows {
            let ev = |eid: &str| -> Option<(String, String, i64, Value)> {
                self.conn
                    .query_row("SELECT dev, actor, ms, body FROM events WHERE eid=?1", [eid], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?))
                    })
                    .optional()
                    .ok()
                    .flatten()
                    .map(|(d, a, m, b)| (d, a, m, serde_json::from_str(&b).unwrap_or(Value::Null)))
            };
            if field == "rehomed" {
                // current: where it shows now (`text` = that node's id, "" = the inbox);
                // other: the deletion of its parent (`text` = the deleted parent's id).
                let under: Option<String> = self.conn.query_row("SELECT under FROM rehomed WHERE node=?1", [&node], |r| r.get(0)).optional()?.flatten();
                let del = ev(&loser);
                out.push(ConflictDetail {
                    id: cid,
                    node: node.clone(),
                    kind: "rehomed".into(),
                    current: Some(ConflictVersion { text: under.unwrap_or_default(), dev: String::new(), actor: String::new(), ms: 0, eid: None }),
                    other: Some(match del {
                        Some((dev, actor, ms, _)) => ConflictVersion { text: loser_value.clone().unwrap_or_default(), dev, actor, ms, eid: Some(loser.clone()) },
                        None => ConflictVersion { text: loser_value.clone().unwrap_or_default(), dev: String::new(), actor: String::new(), ms: 0, eid: None },
                    }),
                    base: None,
                });
                continue;
            }
            if field == "parent" {
                let rejected = ev(&loser);
                let target = loser_value.clone().unwrap_or_default();
                let n = self.node(&node)?;
                out.push(ConflictDetail {
                    id: cid,
                    node: node.clone(),
                    kind: "move".into(),
                    current: n.as_ref().map(|n| ConflictVersion { text: n.parent.clone().unwrap_or_default(), dev: String::new(), actor: String::new(), ms: 0, eid: None }),
                    other: rejected.map(|(dev, actor, ms, _)| ConflictVersion { text: target.clone(), dev, actor, ms, eid: Some(loser.clone()) }),
                    base: None,
                });
                continue;
            }
            // Text: the current version is what the node shows now.
            let n = self.node(&node)?;
            let cur_eid: Option<String> = self.conn.query_row("SELECT text_eid FROM nodes WHERE id=?1", [&node], |r| r.get(0)).optional()?.flatten();
            let current = n.as_ref().map(|n| {
                let meta = cur_eid.as_deref().or(winner.as_deref()).and_then(&ev);
                ConflictVersion {
                    text: n.text.clone(),
                    dev: meta.as_ref().map(|m| m.0.clone()).unwrap_or_default(),
                    actor: meta.as_ref().map(|m| m.1.clone()).unwrap_or_default(),
                    ms: meta.as_ref().map(|m| m.2).unwrap_or(0),
                    eid: cur_eid.clone(),
                }
            });
            let lmeta = ev(&loser);
            let other_text = loser_value.clone().unwrap_or_default();
            // Base: the text the losing edit started from (or, if the loser was the earlier
            // current text, the base of the winning edit).
            let base_eid = lmeta.as_ref().and_then(|m| m.3.get("base").and_then(|b| b.as_str()).map(str::to_string)).or_else(|| {
                winner.as_deref().and_then(&ev).and_then(|m| m.3.get("base").and_then(|b| b.as_str()).map(str::to_string))
            });
            let base = base_eid.and_then(|b| ev(&b)).and_then(|m| m.3.get("text").and_then(|t| t.as_str()).map(str::to_string));
            out.push(ConflictDetail {
                id: cid,
                node: node.clone(),
                kind: "text".into(),
                current,
                other: lmeta.map(|(dev, actor, ms, _)| ConflictVersion { text: other_text.clone(), dev, actor, ms, eid: Some(loser.clone()) }),
                base,
            });
        }
        Ok(out)
    }

    /// Replace `[[id]]` references with `[[Title]]` for display.
    pub fn render_text(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("[[") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("]]") else {
                out.push_str(&rest[start..]);
                return out;
            };
            let inner = &after[..end];
            let (target, label) = match inner.split_once('|') {
                Some((t, l)) => (t, Some(l)),
                None => (inner, None),
            };
            let shown = match label {
                Some(l) => l.to_string(),
                None if id::looks_like_id(target) && target.len() == ID_LEN => {
                    self.node(target).ok().flatten().map(|n| n.label()).unwrap_or_else(|| target.to_string())
                }
                None => target.to_string(),
            };
            out.push_str("[[");
            out.push_str(&shown);
            out.push_str("]]");
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        out
    }
}
