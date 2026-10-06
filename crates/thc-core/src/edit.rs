//! `$EDITOR` round-trip: render a subtree into a canonical outline with trailing `^ids`,
//! parse the edited file back, and diff it into ops. Only our own format is parsed.

use crate::builder::TxBuilder;
use crate::capture;
use crate::error::invalid;
use crate::model::Node;
use crate::ord::key_between;
use crate::store::Store;
use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

const INDENT: usize = 2;
const HELP: &str = "<!-- thc edit: write paragraphs, or \"- \" bullets for items. A paragraph (lines separated by a blank line) is one note;\n     \
    end a line with \\ to keep a line break inside it. Keep the ^id at the end (of a paragraph's last line) to keep identity.\n     \
    Indent bullets 4 spaces to nest. Lines without an id become new nodes; removed lines are deleted.\n     \
    Fields: [ ] [x] [/] [w] [-]  due:fri  sched:mon  !high  repeat:\"every week\"  #tag  [[Page]] -->";

pub struct Rendered {
    pub text: String,
    /// short id -> full id, for every rendered node (root included)
    pub ids: HashMap<String, String>,
    pub root: String,
}

fn checkbox(status: Option<&str>) -> &'static str {
    crate::outline::checkbox(status)
}

fn fields(n: &Node) -> String {
    let repeat = n.repeat.as_ref().and_then(|r| r.get("text")).and_then(|v| v.as_str());
    crate::outline::fields(n.scheduled.as_deref(), n.due.as_deref(), n.priority.as_deref(), repeat)
}

pub fn render(store: &Store, root_id: &str) -> Result<Rendered> {
    let root = store.must_node(root_id)?;
    let mut out = String::new();
    let mut ids = HashMap::new();
    let short = store.short(&root.id);
    ids.insert(short.clone(), root.id.clone());
    let header = root.title.is_some() || root.journal.is_some();
    if header {
        let t = root.title.clone().unwrap_or_else(|| format!("{} (journal)", root.journal.clone().unwrap_or_default()));
        out.push_str(&format!("# {t} ^{short}\n\n"));
        // Top-level plain notes are prose (tui-handoff §10.8): paragraphs, separated by blank
        // lines. Bullets stay tight; a blank line sits between a paragraph and its neighbours.
        let mut prev_para: Option<bool> = None;
        for c in store.children(&root.id)? {
            let para = is_paragraph(store, &c)?;
            // Its `gap` when it has one (writing.md §1), else the default for its kind.
            if prev_para.is_some_and(|p| gap_of(store, &c.id).unwrap_or(p || para)) {
                out.push('\n');
            }
            if para {
                render_paragraph(store, &c, &mut out, &mut ids);
            } else {
                render_node(store, &c, 0, &mut out, &mut ids)?;
            }
            prev_para = Some(para);
        }
    } else {
        render_node(store, &root, 0, &mut out, &mut ids)?;
    }
    out.push('\n');
    out.push_str(HELP);
    out.push('\n');
    Ok(Rendered { text: out, ids, root: root.id })
}

/// The property that marks a note written as prose (tui-handoff §10.8, "Bullets stay bullets"):
/// `style = "para"`, set only when a note is written as a paragraph. Everything else is a bullet,
/// so lists stay lists through a round trip. A plain property: no format change.
pub const STYLE: &str = "style";
pub const PARA: &str = "para";

/// Set `style = "para"` on a node created in this same transaction (set_props would look for
/// it in the store, where it doesn't exist yet).
pub fn mark_para(b: &mut TxBuilder, id: &str) -> Result<()> {
    let v = b.prop_value(STYLE, PARA)?;
    let mut props = serde_json::Map::new();
    props.insert(STYLE.into(), v);
    b.ops.push(crate::event::Op::NodeSet { id: id.into(), props });
    Ok(())
}

/// Whether a blank line comes before a note (writing.md §1): `gap = "1"` or `"0"`;
/// absent means the default for its kind. A plain property: no format change, and readers that
/// don't know it render the default.
pub const GAP: &str = "gap";

/// A node's `gap`, if it carries one.
pub fn gap_of(store: &Store, id: &str) -> Option<bool> {
    store.props_of(id).ok().and_then(|p| p.get(GAP).and_then(gap_value))
}

/// A stored `gap`: "1" / "0" (also a number or a bool, if one was ever written that way).
pub fn gap_value(v: &serde_json::Value) -> Option<bool> {
    match v {
        serde_json::Value::String(s) => match s.as_str() {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        },
        serde_json::Value::Number(n) => n.as_f64().map(|f| f != 0.0),
        serde_json::Value::Bool(b) => Some(*b),
        _ => None,
    }
}

/// Whether a node carries `style = "para"`.
pub fn has_para_style(store: &Store, id: &str) -> bool {
    store.props_of(id).ok().and_then(|p| p.get(STYLE).and_then(|v| v.as_str()).map(|s| s == PARA)).unwrap_or(false)
}

/// Every node written as a paragraph (one query, for listings).
pub fn para_ids(store: &Store) -> HashSet<String> {
    store
        .conn
        .prepare("SELECT node FROM props WHERE key = ?1 AND value = ?2")
        .and_then(|mut st| st.query_map([STYLE, &format!("\"{PARA}\"")], |r| r.get::<_, String>(0))?.collect())
        .unwrap_or_default()
}

/// A note shown as prose: written as a paragraph, and still plain (no status, dates, priority
/// or repeat, no children, no title).
pub fn is_paragraph(store: &Store, n: &Node) -> Result<bool> {
    Ok(has_para_style(store, &n.id)
        && n.status.is_none()
        && n.scheduled.is_none()
        && n.due.is_none()
        && n.priority.is_none()
        && n.repeat.is_none()
        && n.title.is_none()
        && store.children(&n.id)?.is_empty())
}

/// `First line \` / `second line ^id`: a ⌃J line break is a trailing backslash (a Markdown
/// hard break); the id ends the last line.
fn render_paragraph(store: &Store, n: &Node, out: &mut String, ids: &mut HashMap<String, String>) {
    let short = store.short(&n.id);
    ids.insert(short.clone(), n.id.clone());
    let text = store.render_text(&n.text);
    let lines: Vec<&str> = text.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if i + 1 < lines.len() {
            out.push_str(&format!("{l} \\\n"));
        } else {
            out.push_str(&format!("{l} ^{short}\n"));
        }
    }
    if lines.is_empty() {
        out.push_str(&format!("^{short}\n"));
    }
}

fn render_node(store: &Store, n: &Node, depth: usize, out: &mut String, ids: &mut HashMap<String, String>) -> Result<()> {
    let short = store.short(&n.id);
    ids.insert(short.clone(), n.id.clone());
    let pad = " ".repeat(depth * INDENT);
    let text = store.render_text(&n.text);
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    out.push_str(&format!("{pad}- {}{first}{} ^{short}\n", checkbox(n.status.as_deref()), fields(n)));
    for l in lines {
        out.push_str(&format!("{pad}  {l}\n"));
    }
    for c in store.children(&n.id)? {
        render_node(store, &c, depth + 1, out, ids)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct Item {
    id: Option<String>,
    indent: usize,
    parent: Option<usize>,
    first_line: String,
    more: Vec<String>,
    /// Written as a paragraph (no `- `): the note gets `style = "para"`.
    para: bool,
    /// A blank line came before it: its `gap` (writing.md §1).
    blank: bool,
}

fn split_id(line: &str) -> (String, Option<String>) {
    let t = line.trim_end();
    if let Some(i) = t.rfind(" ^") {
        let cand = &t[i + 2..];
        if !cand.is_empty() && crate::id::looks_like_id(cand) && !cand.contains(' ') {
            return (t[..i].trim_end().to_string(), Some(crate::id::normalize(cand)));
        }
    }
    (t.to_string(), None)
}

#[derive(Debug, Default, Serialize)]
pub struct EditSummary {
    pub created: usize,
    pub updated: usize,
    pub moved: usize,
    pub deleted: usize,
}

/// Diff the edited text against the store and append ops to `b`.
pub fn apply(b: &mut TxBuilder, rendered: &Rendered, edited: &str) -> Result<EditSummary> {
    let store = b.store;
    let root = store.must_node(&rendered.root)?;
    let header = root.title.is_some() || root.journal.is_some();
    let resolve = |short: &str| -> Option<String> {
        rendered.ids.get(short).cloned().or_else(|| {
            let m: Vec<&String> = rendered.ids.values().filter(|full| full.starts_with(short)).collect();
            if m.len() == 1 { Some(m[0].clone()) } else { None }
        })
    };

    let mut items: Vec<Item> = Vec::new();
    let mut new_title: Option<String> = None;
    let mut in_comment = false;
    // The paragraph being read (top-level prose, header mode): its lines so far.
    let mut para: Vec<String> = Vec::new();
    // A blank line since the last item: the next one's `gap`.
    let mut blank = false;
    let flush = |para: &mut Vec<String>, items: &mut Vec<Item>, blank: &mut bool| -> Result<()> {
        if para.is_empty() {
            return Ok(());
        }
        // Lines join with spaces (Markdown's rule); a trailing `\` keeps a line break.
        let mut text = String::new();
        let n = para.len();
        for (i, l) in para.drain(..).enumerate() {
            let t = l.trim();
            match t.strip_suffix('\\') {
                Some(body) if i + 1 < n => {
                    text.push_str(body.trim_end());
                    text.push('\n');
                }
                _ => {
                    text.push_str(t);
                    if i + 1 < n {
                        text.push(' ');
                    }
                }
            }
        }
        let (body, id) = split_id(&text);
        let id = match id {
            Some(s) => Some(resolve(&s).ok_or_else(|| invalid(format!("unknown id ^{s} (ids cannot be invented; remove it to create a new node)")))?),
            None => None,
        };
        let mut lines = body.split('\n');
        let first = lines.next().unwrap_or("").to_string();
        items.push(Item { id, indent: 0, parent: None, first_line: first, more: lines.map(str::to_string).collect(), para: true, blank: std::mem::take(blank) });
        Ok(())
    };
    for raw in edited.lines() {
        let line = raw.replace('\t', "    ");
        let trimmed = line.trim();
        if in_comment {
            if trimmed.contains("-->") {
                in_comment = false;
            }
            continue;
        }
        if trimmed.starts_with("<!--") {
            in_comment = !trimmed.contains("-->");
            continue;
        }
        if trimmed.is_empty() {
            let had = !para.is_empty();
            flush(&mut para, &mut items, &mut blank)?;
            // (After the title, or before any note, a blank line is just layout.)
            if had || !items.is_empty() {
                blank = true;
            }
            continue;
        }
        if header && items.is_empty() && new_title.is_none() && trimmed.starts_with("# ") {
            let (t, _) = split_id(trimmed.trim_start_matches("# "));
            new_title = Some(t.trim().to_string());
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let is_bullet = trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed == "-";
        // Prose on a page or day: a top-level line without "- " starts or continues a paragraph.
        if header && !is_bullet && (indent == 0 || !para.is_empty()) {
            para.push(line.clone());
            continue;
        }
        flush(&mut para, &mut items, &mut blank)?;
        if !is_bullet {
            if let Some(last) = items.last_mut() {
                if indent > last.indent {
                    last.more.push(line[(last.indent + 2).min(indent)..].to_string());
                    continue;
                }
            }
        }
        let body = if is_bullet { trimmed[1..].trim_start() } else { trimmed };
        let (body, id) = split_id(body);
        let id = match id {
            Some(s) => Some(resolve(&s).ok_or_else(|| invalid(format!("unknown id ^{s} (ids cannot be invented; remove it to create a new node)")))?),
            None => None,
        };
        // Parent: nearest previous item with smaller indent.
        let parent = items.iter().rposition(|it| it.indent < indent);
        items.push(Item { id, indent, parent, first_line: body, more: vec![], para: false, blank: std::mem::take(&mut blank) });
    }
    flush(&mut para, &mut items, &mut blank)?;

    // Duplicate ids are an error (copy-pasted lines).
    let mut seen = HashSet::new();
    for it in &items {
        if let Some(id) = &it.id {
            if !seen.insert(id.clone()) {
                return Err(invalid(format!("id ^{} appears twice", store.short(id))));
            }
        }
    }
    if !header && (items.is_empty() || items[0].id.as_deref() != Some(root.id.as_str())) {
        return Err(invalid("the first line must stay the edited node (keep its ^id)"));
    }

    let mut summary = EditSummary::default();
    if let (Some(t), Some(old)) = (&new_title, &root.title) {
        if t != old && !t.is_empty() {
            b.set_props(&root.id, &[("title".into(), t.clone())])?;
            summary.updated += 1;
        }
    }

    // Resolve final ids for new items as we go (parents precede children).
    let mut final_ids: Vec<String> = Vec::with_capacity(items.len());
    let mut parent_of: Vec<Option<String>> = Vec::with_capacity(items.len());
    // Each top-level note's `gap` (writing.md §1, E80): what the text shows, written only where
    // it differs from what the node already renders. None: nothing to write.
    let mut gap_want: Vec<Option<Option<bool>>> = vec![None; items.len()];
    if header {
        let mut prev: Option<bool> = None;
        for (i, it) in items.iter().enumerate() {
            if it.parent.is_some() || it.indent > 0 {
                continue;
            }
            if let Some(p) = prev {
                let default = p || it.para;
                let have = it.id.as_deref().and_then(|id| gap_of(store, id));
                if have.unwrap_or(default) != it.blank || (have.is_some() && it.blank == default) {
                    gap_want[i] = Some(if it.blank == default { None } else { Some(it.blank) });
                }
            }
            prev = Some(it.para);
        }
    }
    for (i, it) in items.iter().enumerate() {
        let parent_id = match it.parent {
            Some(p) => Some(final_ids[p].clone()),
            None if header => Some(root.id.clone()),
            None => root.parent.clone(),
        };
        let mut cap = capture::parse(&it.first_line, b.today).or_else(|_| capture::parse("(empty)", b.today))?;
        if !it.more.is_empty() {
            cap.text = format!("{}\n{}", cap.text, it.more.join("\n"));
        }
        match &it.id {
            Some(id) => {
                let node = store.must_node(id)?;
                let before = b.ops.len();
                b.set_text(id, &cap.text)?;
                let mut pairs: Vec<(String, String)> = Vec::new();
                let want_status = cap.status.clone();
                let completing = want_status.as_deref() == Some("done") && node.status.as_deref() != Some("done");
                if completing && node.repeat.is_some() {
                    b.complete(id)?;
                } else if want_status != node.status {
                    pairs.push(("status".into(), want_status.unwrap_or_default()));
                }
                let cmp = |new: Option<String>, old: &Option<String>, key: &str, pairs: &mut Vec<(String, String)>| {
                    if &new != old {
                        pairs.push((key.into(), new.unwrap_or_default()));
                    }
                };
                if !(completing && node.repeat.is_some()) {
                    cmp(cap.scheduled.map(|d| d.fmt()), &node.scheduled, "scheduled", &mut pairs);
                    cmp(cap.due.map(|d| d.fmt()), &node.due, "due", &mut pairs);
                }
                cmp(cap.priority.clone(), &node.priority, "priority", &mut pairs);
                let old_rep = node.repeat.as_ref().and_then(|r| r.get("text")).and_then(|t| t.as_str()).map(str::to_string);
                let new_rep = cap.repeat.as_ref().map(|r| r.text.clone());
                if new_rep != old_rep {
                    pairs.push(("repeat".into(), new_rep.unwrap_or_default()));
                }
                // The form it was written in: a paragraph gets `style = "para"`, a bullet loses it.
                let was_para = has_para_style(store, id);
                if header && it.para != was_para {
                    pairs.push((STYLE.into(), if it.para { PARA.into() } else { String::new() }));
                }
                if let Some(g) = gap_want[i] {
                    pairs.push((GAP.into(), g.map_or(String::new(), |g| if g { "1".into() } else { "0".into() })));
                }
                if !pairs.is_empty() {
                    b.set_props(id, &pairs)?;
                }
                if b.ops.len() > before {
                    summary.updated += 1;
                }
                final_ids.push(id.clone());
            }
            None => {
                let id = b.create_from_capture(parent_id.clone(), &cap, None)?;
                if it.para {
                    mark_para(b, &id)?;
                }
                if let Some(Some(g)) = gap_want[i] {
                    let v = b.prop_value(GAP, if g { "1" } else { "0" })?;
                    let mut props = serde_json::Map::new();
                    props.insert(GAP.into(), v);
                    b.ops.push(crate::event::Op::NodeSet { id: id.clone(), props });
                }
                summary.created += 1;
                final_ids.push(id);
            }
        }
        parent_of.push(parent_id);
        let _ = i;
    }

    // Order: per parent group, keep the longest run of already-ordered siblings, re-key the rest.
    let mut groups: Vec<(Option<String>, Vec<usize>)> = Vec::new();
    for (i, p) in parent_of.iter().enumerate() {
        if !header && i == 0 {
            continue; // the edited root keeps its position
        }
        match groups.iter_mut().find(|(gp, _)| gp == p) {
            Some((_, v)) => v.push(i),
            None => groups.push((p.clone(), vec![i])),
        }
    }
    for (parent, idxs) in groups {
        let current: Vec<Option<String>> = idxs
            .iter()
            .map(|&i| {
                items[i].id.as_ref().and_then(|id| {
                    store.node(id).ok().flatten().filter(|n| n.parent == parent).map(|n| n.ord)
                })
            })
            .collect();
        let keep = lis(&current);
        let mut prev: Option<String> = None;
        for (k, &i) in idxs.iter().enumerate() {
            if keep.contains(&k) {
                prev = current[k].clone();
                continue;
            }
            let next_kept = (k + 1..idxs.len()).find(|j| keep.contains(j)).and_then(|j| current[j].clone());
            let ord = key_between(prev.as_deref(), next_kept.as_deref());
            let id = final_ids[i].clone();
            if items[i].id.is_some() {
                b.ops.push(crate::event::Op::NodeMove { id, parent: parent.clone(), order: ord.clone() });
                summary.moved += 1;
            } else {
                b.set_created_order(&id, ord.clone());
            }
            prev = Some(ord);
        }
    }

    // Deletions: rendered nodes that no longer appear.
    let present: HashSet<&String> = items.iter().filter_map(|i| i.id.as_ref()).collect();
    for full in rendered.ids.values() {
        if full != &root.id && !present.contains(full) {
            b.ops.push(crate::event::Op::NodeDelete { id: full.clone() });
            summary.deleted += 1;
        }
    }
    Ok(summary)
}

/// Indices of a longest strictly increasing subsequence over the `Some` entries.
fn lis(keys: &[Option<String>]) -> HashSet<usize> {
    let idx: Vec<usize> = (0..keys.len()).filter(|&i| keys[i].is_some()).collect();
    let n = idx.len();
    let mut len = vec![1usize; n];
    let mut prev = vec![usize::MAX; n];
    for a in 0..n {
        for b in 0..a {
            if keys[idx[b]] < keys[idx[a]] && len[b] + 1 > len[a] {
                len[a] = len[b] + 1;
                prev[a] = b;
            }
        }
    }
    let mut out = HashSet::new();
    if let Some(mut best) = (0..n).max_by_key(|&i| len[i]) {
        loop {
            out.insert(idx[best]);
            if prev[best] == usize::MAX {
                break;
            }
            best = prev[best];
        }
    }
    out
}
