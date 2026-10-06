//! `thc doctor --fix` (data-model-review.md §1, §5): repairs to the data that ordinary ops can
//! make, planned from the store and written as one transaction (so one undo reverses them).
//! - A journal day with more than one root (two devices made it offline): the newer roots'
//!   lines move under the oldest, and the newer roots are deleted (soft: history keeps them).
//! - A tag that exists twice: its notes are re-tagged to the oldest node; the others go.
//! - Empty-named tags (a heading marker read as a tag before 0.6.2): their edges and nodes go.
//! - Pages that share a title are only reported: which one is right is the person's call.

use crate::event::Op;
use crate::ord::key_between;
use crate::store::Store;
use anyhow::Result;
use rusqlite::params;

/// Several nodes that are one thing: `keep` (the oldest) and the rest, merged into it.
#[derive(Clone, Debug)]
pub struct Merge {
    /// `day` or `tag`.
    pub kind: &'static str,
    /// The day (`2026-10-04`) or the tag's name.
    pub name: String,
    pub keep: String,
    pub merged: Vec<String>,
    /// For a tag, each spelling its nodes have (`lisbon`, `Lisbon`): how a person knows it.
    pub spellings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub merges: Vec<Merge>,
    pub empty_tags: Vec<String>,
    /// Lower-cased titles more than one page has (reported, not changed).
    pub shared_titles: Vec<String>,
}

impl Plan {
    /// Nothing to write (shared titles are only reported).
    pub fn is_empty(&self) -> bool {
        self.merges.is_empty() && self.empty_tags.is_empty()
    }

    pub fn count(&self, kind: &str) -> usize {
        self.merges.iter().filter(|m| m.kind == kind).count()
    }
}

fn strings(store: &Store, sql: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Vec<String>> {
    let mut st = store.conn.prepare(sql)?;
    let rows = st.query_map(p, |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// What needs fixing. The oldest node (creation time, then id) is the one kept, so the plan is
/// the same on every device.
pub fn plan(store: &Store) -> Result<Plan> {
    let mut p = Plan::default();
    for day in strings(store, "SELECT journal FROM nodes WHERE journal IS NOT NULL AND deleted=0 GROUP BY journal HAVING count(*) > 1 ORDER BY journal", &[])? {
        let ids = strings(store, "SELECT id FROM nodes WHERE journal=?1 AND deleted=0 ORDER BY created_ms, id", &[&day])?;
        p.merges.push(Merge { kind: "day", name: day, keep: ids[0].clone(), merged: ids[1..].to_vec(), spellings: vec![] });
    }
    let tag_names = strings(
        store,
        "SELECT lower(title) FROM nodes WHERE is_tag=1 AND deleted=0 AND trim(coalesce(title,'')) <> '' GROUP BY lower(title) HAVING count(*) > 1 ORDER BY 1",
        &[],
    )?;
    for name in tag_names {
        let ids = strings(store, "SELECT id FROM nodes WHERE is_tag=1 AND deleted=0 AND lower(title)=?1 ORDER BY created_ms, id", &[&name])?;
        let mut spellings = strings(store, "SELECT title FROM nodes WHERE is_tag=1 AND deleted=0 AND lower(title)=?1 ORDER BY created_ms, id", &[&name])?;
        spellings.dedup();
        p.merges.push(Merge { kind: "tag", name, keep: ids[0].clone(), merged: ids[1..].to_vec(), spellings });
    }
    p.empty_tags = strings(store, "SELECT id FROM nodes WHERE is_tag=1 AND deleted=0 AND trim(coalesce(title,'')) = '' ORDER BY id", &[])?;
    p.shared_titles = strings(
        store,
        "SELECT lower(title) FROM nodes WHERE parent IS NULL AND is_tag=0 AND journal IS NULL AND trim(coalesce(title,'')) <> '' AND deleted=0 \
         GROUP BY lower(title) HAVING count(*) > 1 ORDER BY 1",
        &[],
    )?;
    Ok(p)
}

/// The ops that carry out `plan`: moves, edge changes and deletes of the merged nodes only
/// (their lines have moved; nothing under them is deleted).
pub fn ops(store: &Store, plan: &Plan) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    for m in &plan.merges {
        // Lines (a day's entries, a tag page's notes) go to the end of the kept node, in order.
        let mut last = store.children(&m.keep)?.last().map(|n| n.ord.clone());
        for dup in &m.merged {
            for child in store.children(dup)? {
                let order = key_between(last.as_deref(), None);
                ops.push(Op::NodeMove { id: child.id.clone(), parent: Some(m.keep.clone()), order: order.clone() });
                last = Some(order);
            }
            if m.kind == "tag" {
                let tagged = strings(store, "SELECT src FROM edges WHERE rel='tag' AND dst=?1", &[dup])?;
                for src in tagged {
                    ops.push(Op::EdgeRemove { src: src.clone(), rel: "tag".into(), dst: dup.clone() });
                    let has: bool = store.conn.query_row(
                        "SELECT count(*) FROM edges WHERE rel='tag' AND src=?1 AND dst=?2",
                        params![src, m.keep],
                        |r| r.get::<_, i64>(0),
                    )? > 0;
                    let pending = ops.iter().any(|o| matches!(o, Op::EdgeAdd { src: s, rel, dst } if *s == src && rel == "tag" && *dst == m.keep));
                    if !has && !pending {
                        ops.push(Op::EdgeAdd { src, rel: "tag".into(), dst: m.keep.clone() });
                    }
                }
            }
            ops.push(Op::NodeDelete { id: dup.clone() });
        }
    }
    for t in &plan.empty_tags {
        for src in strings(store, "SELECT src FROM edges WHERE rel='tag' AND dst=?1", &[t])? {
            ops.push(Op::EdgeRemove { src, rel: "tag".into(), dst: t.clone() });
        }
        ops.push(Op::NodeDelete { id: t.clone() });
    }
    Ok(ops)
}
