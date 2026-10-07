//! Documents as blocks (docs/design/mac-editor-arch.md §5): a page or journal day is its subtree
//! in order, each node one block (a paragraph note, a bullet or a task) with its depth. The Mac
//! editor reads `render` and writes `plan` (block ops, one transaction per save); `$EDITOR`'s
//! round trip lands in the same ops.
//!
//! **Concurrent edits (FORMAT.md §"Block ops", editor.md §8.1):** an `edit` carries `base`, the
//! text revision (`text_rev`) the editor started from. It is always written, with that base on its
//! `node.text` event, so a line someone else changed meanwhile becomes an ordinary text conflict
//! (both versions kept, as when two devices sync) and the editor's text wins locally. An edit to a
//! node deleted elsewhere restores it first (same id and history). Other ops may carry `rev`: a
//! stale one is left out and comes back with the server's block. A create whose id already exists
//! comes back `exists`, never a second node. Every passing op goes into the one transaction.

use crate::builder::TxBuilder;
use crate::capture;
use crate::edit::{PARA, STYLE, has_para_style};
use crate::event::{Actor, Event, Op};
use crate::hlc::Hlc;
use crate::model::Node;
use crate::store::Store;
use anyhow::Result;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// A block's kind: a paragraph note (`style = "para"`, no status), a plain note shown as a
/// bullet, or a task (has a status). Lowercase on the wire (`blocks.apply`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A paragraph.
    Para,
    /// A list item.
    Bullet,
    /// A task: a list item with a status.
    Task,
}

/// One block: a node as the editor shows it. `text` is the clean text (fields live in the
/// gutter, editor.md §4), `rev` guards the next save.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    pub parent: Option<String>,
    pub depth: usize,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// The revision of the text (the event that last set it): an edit's `base`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_rev: Option<String>,
    /// An open text conflict on this node (the editor's ≠).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub conflict: bool,
    /// When a task was completed (`done 09:14` in the gutter).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done_at: Option<String>,
    /// A blank line before it (`gap` prop, writing.md §1): None means the default for its kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<bool>,
}

fn kind_of(store: &Store, n: &Node) -> Kind {
    if n.status.is_some() {
        Kind::Task
    } else if has_para_style(store, &n.id) {
        Kind::Para
    } else {
        Kind::Bullet
    }
}

pub fn block(store: &Store, n: &Node, depth: usize) -> Block {
    Block {
        id: n.id.clone(),
        parent: n.parent.clone(),
        depth,
        kind: kind_of(store, n),
        status: n.status.clone(),
        text: store.render_text(&n.text),
        scheduled: n.scheduled.clone(),
        due: n.due.clone(),
        priority: n.priority.clone(),
        repeat: n.repeat.as_ref().and_then(|r| r.get("text")).and_then(|t| t.as_str()).map(str::to_string),
        tags: store.tags_of(&n.id).unwrap_or_default(),
        rev: store.rev(&n.id),
        text_rev: store.conn.query_row("SELECT text_eid FROM nodes WHERE id=?1", [&n.id], |r| r.get(0)).ok().flatten(),
        conflict: store
            .conn
            .query_row("SELECT count(*) FROM conflicts WHERE node=?1 AND field='text' AND resolved=0", [&n.id], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false),
        done_at: n.done_at.clone(),
        gap: crate::edit::gap_of(store, &n.id),
    }
}

/// The subtree under `?1` (not deleted), as an SQL subquery of ids.
/// The recursive step walks `sub` first and looks children up in `by_parent` (CROSS JOIN fixes
/// the order, INDEXED BY the index): statistics gathered while one page held every
/// node make the index look unselective, and the planner then scans per step (O(n²): 560 ms
/// per query at 5,000 lines instead of 2).
/// Re-homed nodes (Store::rehome) count as children of where they show.
const SUBTREE: &str = "WITH RECURSIVE sub(id) AS (SELECT id FROM nodes WHERE parent=?1 AND deleted=0 \
    UNION ALL SELECT node FROM rehomed WHERE under=?1 \
    UNION ALL SELECT n2.id FROM sub CROSS JOIN nodes n2 INDEXED BY by_parent ON n2.parent=sub.id WHERE n2.deleted=0 \
    UNION ALL SELECT r.node FROM sub CROSS JOIN rehomed r ON r.under=sub.id) SELECT id FROM sub";

/// A task's Markdown cell (`[ ] `, `[x] `, …), or nothing for a plain line.
pub fn checkbox(status: Option<&str>) -> &'static str {
    match status {
        Some("todo") => "[ ] ",
        Some("doing") => "[/] ",
        Some("waiting") => "[w] ",
        Some("done") => "[x] ",
        Some("cancelled") => "[-] ",
        _ => "",
    }
}

/// Fields re-attached as capture tokens (` sched:… due:… !high repeat:"…"`). Dates stay absolute
/// so a copy pasted next week, or an `$EDITOR` buffer saved tomorrow, means the same day.
pub fn fields(scheduled: Option<&str>, due: Option<&str>, priority: Option<&str>, repeat: Option<&str>) -> String {
    let mut s = String::new();
    if let Some(d) = scheduled {
        s.push_str(&format!(" sched:{d}"));
    }
    if let Some(d) = due {
        s.push_str(&format!(" due:{d}"));
    }
    if let Some(p) = priority {
        s.push_str(&format!(" !{p}"));
    }
    if let Some(t) = repeat {
        s.push_str(&format!(" repeat:\"{t}\""));
    }
    s
}

/// Blocks as Markdown (editor.md §3.2 copy, the same lines `thc edit` writes without its ids):
/// paragraphs separated by blank lines, bullets as `- ` nested by 2 spaces, tasks as `- [ ] `,
/// fields re-attached. Depth is relative to the shallowest block given.
pub fn markdown(blocks: &[Block]) -> String {
    let base = blocks.iter().map(|b| b.depth).min().unwrap_or(0);
    let mut out = String::new();
    let mut prev_para: Option<bool> = None;
    for b in blocks {
        let para = b.kind == Kind::Para;
        let f = fields(b.scheduled.as_deref(), b.due.as_deref(), b.priority.as_deref(), b.repeat.as_deref());
        // A blank line before it: its `gap` when it has one, else the default for its kind.
        if prev_para.is_some() && b.gap.unwrap_or(para || prev_para == Some(true)) {
            out.push('\n');
        }
        if para {
            out.push_str(&b.text);
            out.push_str(&f);
        } else {
            let pad = "  ".repeat(b.depth.saturating_sub(base));
            let mut lines = b.text.split('\n');
            out.push_str(&format!("{pad}- {}{}{f}", checkbox(b.status.as_deref()), lines.next().unwrap_or("")));
            for l in lines {
                out.push_str(&format!("\n{pad}  {l}"));
            }
        }
        out.push('\n');
        prev_para = Some(para);
    }
    out
}


/// The document under `root`, in order: depth 0 for the root's children. A fixed number of
/// queries however long the document is (a 5,000-block page must open in well under 50 ms).
pub fn render(store: &Store, root: &str) -> Result<Vec<Block>> {
    render_with(store, root, true)
}

/// `render` for the TUI's editor, which never sends a block's `rev` (its saves carry text
/// bases): the per-block latest-event lookup is skipped, a third of a 5,000-line open.
pub fn render_for_editor(store: &Store, root: &str) -> Result<Vec<Block>> {
    render_with(store, root, false)
}

fn render_with(store: &Store, root: &str, with_rev: bool) -> Result<Vec<Block>> {
    use std::collections::{HashMap, HashSet};
    store.must_node(root)?;
    // The subtree once, into a temp table the other lookups join (each recomputed it: 5 ms a
    // time at 5,000 blocks, half the open).
    store.conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS render_sub(id TEXT PRIMARY KEY) WITHOUT ROWID; DELETE FROM temp.render_sub;")?;
    store.conn.execute(&format!("INSERT INTO temp.render_sub {SUBTREE}"), [root])?;
    const IN_SUB: &str = "SELECT id FROM temp.render_sub";
    let nodes = store.nodes_where(&format!("n.id IN ({IN_SUB}) ORDER BY n.ord, n.id"), &[])?;
    let pairs = |sql: &str| -> Result<Vec<(String, String)>> {
        let mut st = store.conn.prepare(sql)?;
        let v = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))?.collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    };
    let para: HashSet<String> = pairs(&format!("SELECT node, value FROM props WHERE key='{STYLE}' AND node IN ({IN_SUB})"))?
        .into_iter()
        .filter(|(_, v)| v.trim_matches('"') == PARA)
        .map(|(n, _)| n)
        .collect();
    let gaps: HashMap<String, bool> = pairs(&format!("SELECT node, value FROM props WHERE key='{}' AND node IN ({IN_SUB})", crate::edit::GAP))?
        .into_iter()
        .filter_map(|(n, v)| serde_json::from_str::<serde_json::Value>(&v).ok().and_then(|v| crate::edit::gap_value(&v)).map(|g| (n, g)))
        .collect();
    let mut tags: HashMap<String, Vec<String>> = HashMap::new();
    for (n, t) in pairs(&format!("SELECT e.src, t.title FROM edges e JOIN nodes t ON t.id=e.dst WHERE e.rel='tag' AND t.title IS NOT NULL AND e.src IN ({IN_SUB}) ORDER BY t.title"))? {
        tags.entry(n).or_default().push(t);
    }
    let text_rev: HashMap<String, String> = pairs(&format!("SELECT id, text_eid FROM nodes WHERE id IN ({IN_SUB})"))?.into_iter().collect();
    // SQLite returns the row of the max() for bare columns: each entity's latest event.
    let rev: HashMap<String, String> = if with_rev {
        pairs(&format!("SELECT entity, eid FROM (SELECT entity, eid, max(okey) FROM events WHERE entity IN ({IN_SUB}) GROUP BY entity)"))?.into_iter().collect()
    } else {
        HashMap::new()
    };
    let conflicted: HashSet<String> = pairs(&format!("SELECT node, '' FROM conflicts WHERE field IN ('text', 'rehomed') AND resolved=0 AND node IN ({IN_SUB})"))?.into_iter().map(|(n, _)| n).collect();
    // Where each re-homed node shows (its parent was deleted elsewhere).
    let moved: HashMap<String, String> = pairs(&format!("SELECT node, under FROM rehomed WHERE node IN ({IN_SUB})"))?.into_iter().collect();
    let parent_of = |n: &Node| -> Option<String> { moved.get(&n.id).cloned().or_else(|| n.parent.clone()) };
    let mut children: HashMap<String, Vec<&Node>> = HashMap::new();
    for n in &nodes {
        children.entry(parent_of(n).unwrap_or_default()).or_default().push(n);
    }
    let mut out = Vec::with_capacity(nodes.len());
    let mut stack: Vec<(&Node, usize)> = children.get(root).map(|c| c.iter().rev().map(|n| (*n, 0)).collect()).unwrap_or_default();
    while let Some((n, depth)) = stack.pop() {
        out.push(Block {
            id: n.id.clone(),
            parent: parent_of(n),
            depth,
            kind: if n.status.is_some() { Kind::Task } else if para.contains(&n.id) { Kind::Para } else { Kind::Bullet },
            status: n.status.clone(),
            text: if n.text.contains("[[") { store.render_text(&n.text) } else { n.text.clone() },
            scheduled: n.scheduled.clone(),
            due: n.due.clone(),
            priority: n.priority.clone(),
            repeat: n.repeat.as_ref().and_then(|r| r.get("text")).and_then(|t| t.as_str()).map(str::to_string),
            tags: tags.remove(&n.id).unwrap_or_default(),
            rev: rev.get(&n.id).filter(|s| !s.is_empty()).cloned(),
            text_rev: text_rev.get(&n.id).filter(|s| !s.is_empty()).cloned(),
            conflict: conflicted.contains(&n.id),
            done_at: n.done_at.clone(),
            gap: gaps.get(&n.id).copied(),
        });
        if let Some(kids) = children.get(&n.id) {
            for k in kids.iter().rev() {
                stack.push((*k, depth + 1));
            }
        }
    }
    Ok(out)
}

/// A page in the finder: its title, how many blocks it holds, and when
/// anything in it was last edited (epoch ms).
#[derive(Clone, Debug, Serialize)]
pub struct PageSummary {
    pub id: String,
    pub title: String,
    pub lines: i64,
    pub updated_ms: i64,
}

/// Every top-level page (not tags, not journal days), with counts, in one query.
pub fn pages(store: &Store) -> Result<Vec<PageSummary>> {
    let sql = format!(
        "WITH RECURSIVE anc(id, root, ms) AS ( \
           SELECT id, id, updated_ms FROM nodes WHERE parent IS NULL AND title IS NOT NULL AND is_tag=0 AND deleted=0 AND journal IS NULL AND {hidden} \
           UNION ALL SELECT n.id, anc.root, n.updated_ms FROM nodes n JOIN anc ON n.parent=anc.id WHERE n.deleted=0) \
         SELECT p.id, p.title, count(*) - 1, max(anc.ms) FROM anc JOIN nodes p ON p.id=anc.root GROUP BY p.id ORDER BY p.title COLLATE NOCASE",
        hidden = crate::views::HIDDEN_SQL.replace("n.", "")
    );
    let mut st = store.conn.prepare(&sql)?;
    let v = st
        .query_map([], |r| Ok(PageSummary { id: r.get(0)?, title: r.get(1)?, lines: r.get(2)?, updated_ms: r.get(3)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

/// Blocks for these ids (live patching), with depth counted from `root`. Ids that are gone or
/// outside `root` come back in `gone`.
pub fn render_ids(store: &Store, root: &str, ids: &[String]) -> Result<(Vec<Block>, Vec<String>)> {
    let mut blocks = Vec::new();
    let mut gone = Vec::new();
    for id in ids {
        match store.node(id)? {
            Some(n) if !n.deleted => {
                let mut depth = 0usize;
                let mut p = store.view_parent(&n.id)?;
                let mut inside = false;
                while let Some(pid) = p {
                    if pid == root {
                        inside = true;
                        break;
                    }
                    depth += 1;
                    p = store.view_parent(&pid)?;
                }
                if inside { blocks.push(block(store, &n, depth)) } else { gone.push(id.clone()) }
            }
            _ => gone.push(id.clone()),
        }
    }
    Ok((blocks, gone))
}

/// One block op (mac-editor-arch.md §3).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum BlockOp {
    /// New text in capture syntax: tokens become fields; fields not mentioned stay as they are.
    /// `base` is the `text_rev` the editor started from (a concurrent change becomes a conflict).
    /// `raw` stores the text as typed, without reading tokens (the idle save, editor.md §8: a
    /// half-typed `due:fr` is never read as a date).
    Edit { id: String, text: String, #[serde(default)] base: Option<String>, #[serde(default)] rev: Option<String>, #[serde(default)] raw: bool },
    /// A new block with a client-made id, under `parent` (None = the root), after a sibling.
    Create { id: String, #[serde(default)] parent: Option<String>, #[serde(default)] after: Option<String>, kind: Kind, text: String },
    Move { id: String, #[serde(default)] parent: Option<String>, #[serde(default)] after: Option<String>, #[serde(default)] rev: Option<String> },
    Delete { id: String, #[serde(default)] rev: Option<String> },
    /// Paragraph, bullet or task (the status cell, `- `, `[] `).
    Kind { id: String, kind: Kind, #[serde(default)] rev: Option<String> },
    /// The status cell: todo, doing, waiting, done, cancelled (done on a repeating task advances it).
    Status { id: String, status: String, #[serde(default)] rev: Option<String> },
    /// Whether a blank line comes before it (`gap`, writing.md §1); None clears it (the default).
    Gap { id: String, #[serde(default)] gap: Option<bool>, #[serde(default)] rev: Option<String> },
}

impl BlockOp {
    pub fn id(&self) -> &str {
        match self {
            BlockOp::Edit { id, .. } | BlockOp::Create { id, .. } | BlockOp::Move { id, .. } | BlockOp::Delete { id, .. } | BlockOp::Kind { id, .. } | BlockOp::Status { id, .. } | BlockOp::Gap { id, .. } => id,
        }
    }

    fn rev(&self) -> Option<&str> {
        match self {
            BlockOp::Edit { rev, .. } | BlockOp::Move { rev, .. } | BlockOp::Delete { rev, .. } | BlockOp::Kind { rev, .. } | BlockOp::Status { rev, .. } | BlockOp::Gap { rev, .. } => rev.as_deref(),
            BlockOp::Create { .. } => None,
        }
    }
}

/// An op result as it comes back over the socket (`blocks.apply`), owned.
#[derive(Clone, Debug, Deserialize)]
pub struct OpResultWire {
    pub index: usize,
    pub id: String,
    pub state: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub block: Option<Block>,
}

impl OpResultWire {
    pub fn into_result(self) -> OpResult {
        let state = match self.state.as_str() {
            "ok" => "ok",
            "stale" => "stale",
            "exists" => "exists",
            _ => "error",
        };
        OpResult { index: self.index, id: self.id, state, error: self.error, block: self.block }
    }
}

/// What happened to one op.
#[derive(Clone, Debug, Serialize)]
pub struct OpResult {
    pub index: usize,
    pub id: String,
    /// ok | stale | exists | error
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The block now (after the save for ok; the server's state for stale and exists).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block: Option<Block>,
}

/// Plan `ops` under `root` against a provisional store (a savepoint, always rolled back), so later
/// ops see earlier ones. Returns the event ops of every op that passed (one transaction) and a
/// result per op. Results for ok ops carry the block as it will be once the ops are written.
pub fn plan(store: &Store, root: &str, ops: &[BlockOp], today: NaiveDate) -> Result<(Vec<Op>, Vec<OpResult>)> {
    store.must_node(root)?;
    // Revisions are checked against the state before this save, once.
    let mut results: Vec<Option<OpResult>> = vec![None; ops.len()];
    for (i, op) in ops.iter().enumerate() {
        if let Some(want) = op.rev() {
            let cur = store.rev(op.id());
            if cur.as_deref() != Some(want) {
                let blk = store.node(op.id())?.filter(|n| !n.deleted).map(|n| block(store, &n, 0));
                results[i] = Some(OpResult { index: i, id: op.id().into(), state: "stale", error: None, block: blk });
            }
        }
        if let BlockOp::Create { id, .. } = op {
            if !crate::id::looks_like_id(id) || id.len() != crate::id::ID_LEN {
                results[i] = Some(OpResult { index: i, id: id.clone(), state: "error", error: Some(format!("{id} isn't a {}-char thc id", crate::id::ID_LEN)), block: None });
            } else if let Some(n) = store.node(id)? {
                // A client-made id that already exists (a clash, or a retried save): never two nodes.
                results[i] = Some(OpResult { index: i, id: id.clone(), state: "exists", error: None, block: (!n.deleted).then(|| block(store, &n, 0)) });
            }
        }
    }
    store.conn.execute_batch("SAVEPOINT thc_outline")?;
    let planned = plan_inner(store, root, ops, today, &mut results);
    store.conn.execute_batch("ROLLBACK TO thc_outline; RELEASE thc_outline")?;
    let all = planned?;
    Ok((all, results.into_iter().map(|r| r.expect("every op has a result")).collect()))
}

fn plan_inner(store: &Store, root: &str, ops: &[BlockOp], today: NaiveDate, results: &mut [Option<OpResult>]) -> Result<Vec<Op>> {
    let mut all = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        if results[i].is_some() {
            continue;
        }
        let mut b = TxBuilder::new(store, today);
        match build(&mut b, store, root, op) {
            Ok(()) => {
                let built = b.finish();
                provisional(store, &built, &format!("outline-{i}"))?;
                let blk = store.node(op.id())?.filter(|n| !n.deleted).map(|n| {
                    let (bl, _) = render_ids(store, root, std::slice::from_ref(&n.id)).unwrap_or_default();
                    bl.into_iter().next().unwrap_or_else(|| block(store, &n, 0))
                });
                all.extend(built);
                results[i] = Some(OpResult { index: i, id: op.id().into(), state: "ok", error: None, block: blk });
            }
            Err(e) => results[i] = Some(OpResult { index: i, id: op.id().into(), state: "error", error: Some(format!("{e:#}")), block: None }),
        }
    }
    Ok(all)
}

fn parent_or_root(root: &str, parent: &Option<String>) -> String {
    parent.clone().unwrap_or_else(|| root.to_string())
}

fn build(b: &mut TxBuilder, store: &Store, root: &str, op: &BlockOp) -> Result<()> {
    match op {
        BlockOp::Edit { id, text, base, raw, .. } => {
            let n = store.node(id)?.ok_or_else(|| crate::error::not_found(format!("node {id}")))?;
            if n.deleted {
                // Deleted elsewhere while edited here: keep the line (same id, same history).
                b.restore(id)?;
                crate::outline::provisional(store, &b.ops, "outline-restore")?;
            }
            let before = b.ops.len();
            // An emptied note is set empty as is: capture refuses empty text ("nothing to
            // capture"), which failed the save and brought the old text back (fuzz).
            if *raw || text.trim().is_empty() {
                b.set_text(id, text)?;
            } else {
                // Lenient: a token thc can't read stays as text (a line is never refused).
                let (cap, _) = capture::parse_lenient_text(text, b.today)?;
                b.edit_from_capture(id, &cap)?;
            }
            if let Some(base) = base {
                for op in b.ops[before..].iter_mut() {
                    if let Op::NodeText { id: oid, base: obase, .. } = op {
                        if oid == id {
                            *obase = Some(base.clone());
                        }
                    }
                }
            }
        }
        BlockOp::Create { id, parent, after, kind, text } => {
            let parent = parent_or_root(root, parent);
            let (mut cap, _) = capture::parse_lenient_text(text, b.today)?;
            match kind {
                Kind::Task if cap.status.is_none() => cap.status = Some("todo".into()),
                Kind::Para | Kind::Bullet => cap.status = cap.status.take().filter(|_| *kind == Kind::Task),
                _ => {}
            }
            let id = b.create_from_capture(Some(parent.clone()), &cap, Some(id.clone()))?;
            if *kind == Kind::Para {
                crate::edit::mark_para(b, &id)?;
            }
            // Place it: the builder can't see its own creates, so the order is set from the
            // provisional store's siblings (the earlier ops of this save are already there).
            // No `after` means first among its siblings.
            let ord = match after {
                Some(a) => {
                    let (lo, hi) = store.neighbour_ords(Some(&parent), &id, crate::model::Place::After(a))?;
                    Some(crate::ord::key_between(lo.as_deref(), hi.as_deref()))
                }
                None => store.neighbour_ords(Some(&parent), &id, crate::model::Place::First)?.1.map(|f| crate::ord::key_between(None, Some(&f))),
            };
            if let Some(ord) = ord {
                b.set_created_order(&id, ord);
            }
        }
        BlockOp::Move { id, parent, after, .. } => {
            // No `after` means first among its siblings (before the first one that isn't it).
            let parent = parent_or_root(root, parent);
            // Never under a parent that isn't there (a create earlier in this save that failed):
            // the move would orphan the note out of its document (fuzz). It stays where it is.
            if store.node(&parent)?.is_none_or(|p| p.deleted) {
                return Err(crate::error::not_found(format!("parent {parent} isn't there")));
            }
            match after {
                Some(a) => b.move_to(id, Some(parent), Some(a), None)?,
                None => {
                    // First among its siblings: before the first that isn't it.
                    use rusqlite::OptionalExtension;
                    let first: Option<String> = store.conn.query_row("SELECT id FROM nodes WHERE parent = ?1 AND deleted = 0 AND id <> ?2 ORDER BY ord, id LIMIT 1", rusqlite::params![parent, id], |r| r.get(0)).optional()?;
                    b.move_to(id, Some(parent), None, first.as_deref())?
                }
            }
        }
        BlockOp::Delete { id, .. } => {
            b.delete(id)?;
        }
        BlockOp::Kind { id, kind, .. } => {
            let n = store.must_node(id)?;
            let mut pairs: Vec<(String, String)> = Vec::new();
            match kind {
                Kind::Task => {
                    if n.status.is_none() {
                        pairs.push(("status".into(), "todo".into()));
                    }
                }
                Kind::Para | Kind::Bullet => {
                    // A task that stops being one (⌃T to text, its marker deleted) takes its
                    // status and dates with it: plain text has none (writing.md §1, §3).
                    if n.status.is_some() {
                        pairs.push(("status".into(), String::new()));
                        for (k, set) in [("due", n.due.is_some()), ("scheduled", n.scheduled.is_some()), ("repeat", n.repeat.is_some()), ("done_at", n.done_at.is_some())] {
                            if set {
                                pairs.push((k.into(), String::new()));
                            }
                        }
                    }
                }
            }
            let para = has_para_style(store, id);
            if (*kind == Kind::Para) != para {
                pairs.push((STYLE.into(), if *kind == Kind::Para { PARA.into() } else { String::new() }));
            }
            if !pairs.is_empty() {
                b.set_props(id, &pairs)?;
            }
        }
        BlockOp::Status { id, status, .. } => {
            let n = store.must_node(id)?;
            if status == "done" && n.repeat.is_some() && n.status.as_deref() != Some("done") {
                b.complete(id)?;
            } else {
                b.set_props(id, &[("status".into(), status.clone())])?;
            }
        }
        BlockOp::Gap { id, gap, .. } => match gap {
            // As mark_para: the node may have been created in this same save.
            Some(g) => {
                let v = b.prop_value(crate::edit::GAP, if *g { "1" } else { "0" })?;
                let mut props = serde_json::Map::new();
                props.insert(crate::edit::GAP.into(), v);
                b.ops.push(crate::event::Op::NodeSet { id: id.clone(), props });
            }
            None => {
                if crate::edit::gap_of(store, id).is_some() {
                    b.set_props(id, &[(crate::edit::GAP.into(), String::new())])?;
                }
            }
        },
    }
    Ok(())
}

/// After the plan's transaction is committed: the result blocks' revisions as they really are.
/// `plan` builds results against a provisional store whose events have `preview-` ids, so its
/// `rev` and `text_rev` name events that never existed: an editor that took them as the next
/// edit's base turned its own second save of a line into a conflict (0.8.x).
pub fn refresh_revs(store: &Store, results: &mut [OpResult]) {
    for r in results.iter_mut() {
        if let Some(b) = r.block.as_mut() {
            b.rev = store.rev(&b.id);
            b.text_rev = store.conn.query_row("SELECT text_eid FROM nodes WHERE id=?1", [&b.id], |row| row.get::<_, Option<String>>(0)).ok().flatten();
        }
    }
}

/// Apply ops to the store provisionally (inside the caller's savepoint) so later ops see them.
pub fn provisional(store: &Store, ops: &[Op], tx: &str) -> Result<()> {
    let mut hlc = store.max_hlc()?;
    for op in ops {
        hlc = Hlc::tick(hlc);
        let e = Event {
            v: op.min_version(),
            eid: format!("preview-{}", crate::id::new_id()),
            hlc,
            dev: "preview".into(),
            actor: Actor { kind: "human".into(), name: None },
            via: "outline".into(),
            tx: tx.to_string(),
            op: op.clone(),
        };
        store.apply(&e)?;
    }
    Ok(())
}
