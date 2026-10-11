//! `$EDITOR` round-trip: render a subtree into a canonical outline with trailing `^ids`,
//! parse the edited file back, and diff it into ops. Only our own format is parsed.
//!
//! **One nesting rule (owner decision, 2026-10-11):** an indented line is a child of the note
//! above it, whatever either kind (a paragraph, a list item, a task). A note's own later lines
//! are joined by a trailing `\` (a line break); a paragraph's wrapped lines may also continue at
//! the paragraph's own indent. **Lists** (list.rs): a run of sibling items is one list; an
//! item's marker comes from the list's style (the `list` prop on its first item, else the
//! default) and its place in the run; numbers are never stored. `Writer` is the one writer of
//! notes as Markdown lines, for this text and for the blocks' copy (outline::markdown).

use crate::builder::TxBuilder;
use crate::capture;
use crate::error::invalid;
use crate::list::{self, LIST, Level, ListStyle};
use crate::model::Node;
use crate::ord::key_between;
use crate::store::Store;
use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

const INDENT: usize = 2;
const HELP: &str = "<!-- thc edit: write paragraphs, or list items (- * + bullets; 1. a. A. i. I. numbered by their order).\n     \
    A paragraph (lines separated by a blank line) is one note. End a line with \\ to keep a line break inside a note.\n     \
    Keep the ^id at the end of a note's last line to keep identity. Indent 2 spaces to nest: an indented line\n     \
    is a child of the note above it, a paragraph's too. Lines without an id become new nodes; removed lines are deleted.\n     \
    Fields: [ ] [x] [/] [w] [-]  due:fri  sched:mon  !high  repeat:\"every week\"  #tag  [[Page]] -->";

#[derive(Clone, Debug)]
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
    let mut w = Writer::default();
    let mut ids = HashMap::new();
    let short = store.short(&root.id);
    ids.insert(short.clone(), root.id.clone());
    let depth = if root.title.is_some() || root.journal.is_some() {
        let t = root.title.clone().unwrap_or_else(|| format!("{} (journal)", root.journal.clone().unwrap_or_default()));
        w.out.push_str(&format!("# {t} ^{short}\n\n"));
        0
    } else {
        // One note and its subtree: the note is shown alone, so not numbered as an item of its
        // list (its list isn't in the text).
        let para = is_paragraph(store, &root);
        render_note(store, &root, 0, (!para).then(|| ListStyle::BULLET.marker(1)).as_deref(), &mut w, &mut ids);
        1
    };
    render_children(store, &root.id, list::enclosing(store, &root.id), depth, &mut w, &mut ids)?;
    let mut out = w.out;
    out.push('\n');
    out.push_str(HELP);
    out.push('\n');
    Ok(Rendered { text: out, ids, root: root.id })
}

/// A note's children at `depth`, then each one's own children: every kind nests the same way.
/// A list item's marker is its list's style at its place in its run of items (list.rs).
fn render_children(store: &Store, parent: &str, enclosing: Level, depth: usize, w: &mut Writer, ids: &mut HashMap<String, String>) -> Result<()> {
    for (c, place) in list::places_under(store, parent, enclosing)? {
        render_note(store, &c, depth, place.marker().as_deref(), w, ids);
        render_children(store, &c.id, place.inner, depth + 1, w, ids)?;
    }
    Ok(())
}

fn render_note(store: &Store, n: &Node, depth: usize, marker: Option<&str>, w: &mut Writer, ids: &mut HashMap<String, String>) {
    let short = store.short(&n.id);
    ids.insert(short.clone(), n.id.clone());
    let text = store.render_text(&n.text);
    let fields = fields(n);
    w.note(&Note { depth, marker, status: n.status.as_deref(), text: &text, fields: &fields, gap: gap_of(store, &n.id), id: Some(&short) });
}

/// One note as Markdown lines.
pub struct Note<'a> {
    /// Its nesting (2 spaces a level).
    pub depth: usize,
    /// Its list marker (`-`, `3.`); None for a paragraph.
    pub marker: Option<&'a str>,
    pub status: Option<&'a str>,
    pub text: &'a str,
    /// Its fields as capture tokens (` due:… !high`), after the first line.
    pub fields: &'a str,
    /// Its `gap` prop, if it has one.
    pub gap: Option<bool>,
    /// The `^id` that ends its last line (`thc edit`), or none (a copy).
    pub id: Option<&'a str>,
}

/// The one writer of notes as Markdown lines, in document order: `thc edit`'s text and the
/// blocks' copy (outline::markdown) are both written here.
#[derive(Default)]
pub struct Writer {
    pub out: String,
    /// The note before: whether it was a paragraph.
    prev_para: Option<bool>,
}

impl Writer {
    /// A note: after the blank line its `gap` says (or the default: a paragraph keeps a blank line
    /// on either side), a list item behind its marker or a paragraph (escaped where its first line
    /// would read as an item). Fields follow the first line. A line break is a trailing `\` (a
    /// Markdown hard break), the next line at the note's text column. An id ends the last line.
    pub fn note(&mut self, n: &Note) {
        let para = n.marker.is_none();
        if self.prev_para.is_some_and(|p| n.gap.unwrap_or(p || para)) {
            self.out.push('\n');
        }
        self.prev_para = Some(para);
        let pad = " ".repeat(n.depth * INDENT);
        let mut lines: Vec<&str> = n.text.lines().collect();
        if lines.is_empty() {
            lines.push("");
        }
        let (head, column) = match n.marker {
            Some(m) => (format!("{pad}{m} {}", checkbox(n.status)), " ".repeat(pad.len() + m.chars().count() + 1)),
            None => (pad.clone(), pad.clone()),
        };
        let last = lines.len() - 1;
        for (i, l) in lines.iter().enumerate() {
            let mut line = if i == 0 {
                let first = if para { list::escape(l) } else { l.to_string() };
                format!("{head}{first}{}", n.fields)
            } else {
                format!("{column}{l}")
            };
            if i < last {
                line.push_str(" \\");
            } else if let Some(id) = n.id {
                if !line.trim().is_empty() {
                    line.push(' ');
                }
                line.push('^');
                line.push_str(id);
            }
            self.out.push_str(line.trim_end_matches(' '));
            self.out.push('\n');
        }
    }
}

/// The property that marks a note written as prose (tui-handoff §10.8, "Bullets stay bullets"):
/// `style = "para"`, set only when a note is written as a paragraph. Everything else is a bullet,
/// so lists stay lists through a round trip. A plain property: no format change.
pub const STYLE: &str = "style";
pub const PARA: &str = "para";

/// Set a property on a node, also one created in this same transaction (set_props would look
/// for it in the store, where it doesn't exist yet).
fn put_prop(b: &mut TxBuilder, id: &str, key: &str, raw: &str) -> Result<()> {
    let v = b.prop_value(key, raw)?;
    let mut props = serde_json::Map::new();
    props.insert(key.into(), v);
    b.ops.push(crate::event::Op::NodeSet { id: id.into(), props });
    Ok(())
}

/// Set `style = "para"` on a node, also one created in this same transaction.
pub fn mark_para(b: &mut TxBuilder, id: &str) -> Result<()> {
    put_prop(b, id, STYLE, PARA)
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

/// A note shown as prose: written as a paragraph and not a task (outline.rs `Kind::Para`, the
/// same rule). Its fields are written after its first line, as an item's are; it may have
/// children, like any note.
pub fn is_paragraph(store: &Store, n: &Node) -> bool {
    n.status.is_none() && has_para_style(store, &n.id)
}

/// One note as written in an edit text.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    /// The lines it covers (indices into the text's lines).
    pub lines: std::ops::Range<usize>,
    /// The column it starts at: its marker's, or a paragraph's first word's.
    pub indent: usize,
    /// Its list marker's style; None for a paragraph.
    pub marker: Option<ListStyle>,
    /// Its lines joined (a trailing `\` keeps a break, else a space), the `^id` still at the end.
    pub text: String,
    /// A blank line came before it: its `gap` (writing.md §1).
    pub blank: bool,
}

/// An edit text read into notes: the `# title` (a page or day) and each note in order.
#[derive(Debug, Default)]
pub struct Parsed {
    pub title: Option<String>,
    pub spans: Vec<Span>,
}

/// Read an edit text into notes. A note starts at a list marker or a paragraph's first line;
/// it goes on after a line ending in `\`, and a paragraph also over lines at its own indent. A
/// blank line ends it. Nesting is the indent alone (the caller's: the nearest note above with a
/// smaller one). `header`: the text starts with a page's or day's `# title`.
pub fn parse(edited: &str, header: bool) -> Parsed {
    let mut parsed = Parsed::default();
    let mut open: Option<(Span, Vec<String>)> = None;
    let mut blank = false;
    let mut in_comment = false;
    let close = |open: &mut Option<(Span, Vec<String>)>, spans: &mut Vec<Span>| {
        if let Some((mut span, lines)) = open.take() {
            span.text = join(&lines);
            spans.push(span);
        }
    };
    for (k, raw) in edited.lines().enumerate() {
        let line = raw.replace('\t', "    ");
        let trimmed = line.trim();
        if in_comment {
            in_comment = !trimmed.contains("-->");
            continue;
        }
        if trimmed.starts_with("<!--") {
            in_comment = !trimmed.contains("-->");
            continue;
        }
        if trimmed.is_empty() {
            close(&mut open, &mut parsed.spans);
            // (After the title, or before any note, a blank line is just layout.)
            blank |= !parsed.spans.is_empty();
            continue;
        }
        if header && parsed.title.is_none() && parsed.spans.is_empty() && open.is_none() && trimmed.starts_with("# ") {
            parsed.title = Some(split_id(trimmed.trim_start_matches("# ")).0.trim().to_string());
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let item = list::marker(trimmed);
        if let Some((span, lines)) = open.as_mut() {
            let broken = lines.last().is_some_and(|l| l.ends_with('\\'));
            let wrapped = item.is_none() && span.marker.is_none() && span.indent == indent;
            if broken || wrapped {
                lines.push(trimmed.to_string());
                span.lines.end = k + 1;
                continue;
            }
        }
        close(&mut open, &mut parsed.spans);
        let (marker, body) = match item {
            Some((style, body)) => (Some(style), body.to_string()),
            None => (None, list::unescape(trimmed)),
        };
        let span = Span { lines: k..k + 1, indent, marker, text: String::new(), blank: std::mem::take(&mut blank) };
        open = Some((span, vec![body]));
    }
    close(&mut open, &mut parsed.spans);
    parsed
}

/// A note's lines as its text: Markdown's rule (lines join with a space), except that a trailing
/// `\` keeps a line break.
fn join(lines: &[String]) -> String {
    let mut text = String::new();
    for (i, l) in lines.iter().enumerate() {
        let more = i + 1 < lines.len();
        match l.trim().strip_suffix('\\') {
            Some(body) if more => {
                text.push_str(body.trim_end());
                text.push('\n');
            }
            _ => {
                text.push_str(l.trim());
                if more {
                    text.push(' ');
                }
            }
        }
    }
    text
}

#[derive(Debug, Clone)]
struct Item {
    id: Option<String>,
    indent: usize,
    parent: Option<usize>,
    first_line: String,
    more: Vec<String>,
    /// Its list marker's style; None for a paragraph (the note gets `style = "para"`).
    marker: Option<ListStyle>,
    /// A blank line came before it: its `gap` (writing.md §1).
    blank: bool,
}

impl Item {
    fn para(&self) -> bool {
        self.marker.is_none()
    }
}

/// A line and its trailing `^id`, if it has one (the id normalized). A line that is only an id
/// (an empty note) has an empty body.
pub fn split_id(line: &str) -> (String, Option<String>) {
    let t = line.trim_end();
    let id = |cand: &str| (!cand.is_empty() && crate::id::looks_like_id(cand) && !cand.contains(' ')).then(|| crate::id::normalize(cand));
    if let Some(i) = t.rfind(" ^") {
        if let Some(id) = id(&t[i + 2..]) {
            return (t[..i].trim_end().to_string(), Some(id));
        }
    }
    if let Some(id) = t.trim_start().strip_prefix('^').and_then(id) {
        return (String::new(), Some(id));
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

/// A list as written, against the `list` its first item has: the level it ends up at, and the
/// value to write (empty clears it) when the text shows a style other than the one it reads as.
/// `baseline`: what its first item reads as with no prop (the list before it, which it would
/// continue, else the default). Only a style that differs from that is stored.
fn decide(enclosing: Level, baseline: ListStyle, have: Option<ListStyle>, written: ListStyle) -> (Level, Option<String>) {
    let now = have.unwrap_or(baseline);
    let write = (written != now).then(|| if written == baseline { String::new() } else { written.as_str() });
    (enclosing.inside(written), write)
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

    let parsed = parse(edited, header);
    let mut items: Vec<Item> = Vec::with_capacity(parsed.spans.len());
    for span in parsed.spans {
        let (body, id) = split_id(&span.text);
        let id = match id {
            Some(s) => Some(resolve(&s).ok_or_else(|| invalid(format!("unknown id ^{s} (ids cannot be invented; remove it to create a new node)")))?),
            None => None,
        };
        // Parent: the nearest note above with a smaller indent.
        let parent = items.iter().rposition(|it| it.indent < span.indent);
        let mut lines = body.split('\n');
        let first_line = lines.next().unwrap_or("").to_string();
        let more = lines.map(str::to_string).collect();
        items.push(Item { id, indent: span.indent, parent, first_line, more, marker: span.marker, blank: span.blank });
    }

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
    if let (Some(t), Some(old)) = (&parsed.title, &root.title) {
        if t != old && !t.is_empty() {
            b.set_props(&root.id, &[("title".into(), t.clone())])?;
            summary.updated += 1;
        }
    }

    // Each note's `gap` (writing.md §1, E80): what the text shows, written only where it differs
    // from what the node already renders. None: nothing to write.
    let mut gap_want: Vec<Option<Option<bool>>> = vec![None; items.len()];
    for i in 1..items.len() {
        let it = &items[i];
        let default = items[i - 1].para() || it.para();
        let have = it.id.as_deref().and_then(|id| gap_of(store, id));
        if have.unwrap_or(default) != it.blank || (have.is_some() && it.blank == default) {
            gap_want[i] = Some(if it.blank == default { None } else { Some(it.blank) });
        }
    }

    // Lists (list.rs): a run of sibling items is one list, its style its first item's marker,
    // stored on that first item when it isn't the default. It's re-derived from the text, so
    // when a list's first item moves or goes, the new first one takes the prop and an item that
    // no longer starts a list loses its own. By parent item: the level its children's lists sit
    // in, and the list now open among them. None: the root's children (editing one note: that
    // note and its siblings, whose list isn't in the text, so never written).
    let root_inner = list::enclosing(store, &root.id);
    let mut enclosing: HashMap<Option<usize>, Level> = HashMap::from([(None, root_inner)]);
    let mut open_list: HashMap<Option<usize>, Level> = HashMap::new();

    // Resolve final ids for new items as we go (parents precede children).
    let mut final_ids: Vec<String> = Vec::with_capacity(items.len());
    let mut parent_of: Vec<Option<String>> = Vec::with_capacity(items.len());
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
        // Its place in a list: the first item writes the list's style; any other note clears one.
        let have_list = it.id.as_deref().and_then(|id| list::explicit(store, id));
        let (inner, list_want) = if !header && it.parent.is_none() {
            (if it.para() { Level::NONE } else { root_inner }, None)
        } else if it.para() {
            open_list.remove(&it.parent);
            (Level::NONE, have_list.map(|_| String::new()))
        } else if let Some(&level) = open_list.get(&it.parent).filter(|l| it.marker.is_some_and(|m| l.style.admits(m))) {
            (level, have_list.map(|_| String::new()))
        } else {
            // A list starts here: first, after a paragraph, or at a change of list type.
            let e = enclosing[&it.parent];
            let baseline = open_list.get(&it.parent).map_or(e.default_inside(), |l| l.style);
            let (level, write) = decide(e, baseline, have_list, it.marker.expect("an item has a marker"));
            open_list.insert(it.parent, level);
            (level, write)
        };
        enclosing.insert(Some(i), inner);
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
                // The form it was written in: a paragraph gets `style = "para"`, an item loses it.
                if it.para() != has_para_style(store, id) {
                    pairs.push((STYLE.into(), if it.para() { PARA.into() } else { String::new() }));
                }
                if let Some(g) = gap_want[i] {
                    pairs.push((GAP.into(), g.map_or(String::new(), |g| if g { "1".into() } else { "0".into() })));
                }
                if let Some(l) = list_want {
                    pairs.push((LIST.into(), l));
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
                if it.para() {
                    mark_para(b, &id)?;
                }
                if let Some(Some(g)) = gap_want[i] {
                    put_prop(b, &id, GAP, if g { "1" } else { "0" })?;
                }
                if let Some(l) = list_want.filter(|l| !l.is_empty()) {
                    put_prop(b, &id, LIST, &l)?;
                }
                summary.created += 1;
                final_ids.push(id);
            }
        }
        parent_of.push(parent_id);
    }

    // The text states every list under these notes (written above): nothing to carry over.
    b.lists_stated.extend(final_ids.iter().cloned().chain(header.then(|| root.id.clone())));

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Actor;
    use crate::vault::{self, Paths, Vault};

    fn today() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap()
    }

    fn setup() -> (Vault, String) {
        let root = crate::scratch::dir(&format!("thc-edit-{}", crate::id::new_id()));
        vault::init(&root.join("v"), None, None).unwrap();
        let paths = Paths { vault: root.join("v"), cache: root.join("cache") };
        let mut v = Vault::open(paths, Actor { kind: "human".into(), name: None }, "test").unwrap();
        let (_, page) = v
            .transact(|s| {
                let mut b = TxBuilder::new(s, today());
                let p = b.create_page("Notes", &[])?;
                Ok((b.finish()?, p))
            })
            .unwrap();
        (v, page)
    }

    /// Apply `edited` against `rendered`.
    fn apply_text(v: &mut Vault, rendered: &Rendered, edited: &str) -> EditSummary {
        v.transact(|s| {
            let mut b = TxBuilder::new(s, today());
            let sum = apply(&mut b, rendered, edited)?;
            Ok((b.finish()?, sum))
        })
        .unwrap()
        .1
    }

    /// Save `text` as the page's edit text (the heading is added).
    fn save(v: &mut Vault, page: &str, text: &str) {
        let rendered = render(&v.store, page).unwrap();
        apply_text(v, &rendered, &format!("# Notes\n\n{text}"));
    }

    /// The page's edit text without its heading, help or ids: what a person sees.
    fn body(v: &Vault, page: &str) -> String {
        let text = render(&v.store, page).unwrap().text;
        let text = &text[..text.find("<!--").unwrap()];
        let lines: Vec<String> = text.lines().skip(2).map(|l| split_id(l).0).collect();
        lines.join("\n").trim_end().to_string() + "\n"
    }

    /// Render → apply → render: nothing written, the same text.
    fn round_trips(v: &mut Vault, page: &str) {
        let before = render(&v.store, page).unwrap();
        let sum = apply_text(v, &before, &before.text);
        assert_eq!((sum.created, sum.updated, sum.moved, sum.deleted), (0, 0, 0, 0), "{}", before.text);
        assert_eq!(render(&v.store, page).unwrap().text, before.text);
    }

    fn kid(v: &Vault, parent: &str, i: usize) -> Node {
        v.store.children(parent).unwrap().remove(i)
    }

    fn list_of(v: &Vault, id: &str) -> Option<String> {
        list::explicit(&v.store, id).map(|s| s.as_str())
    }

    #[test]
    fn an_indented_line_is_a_child_of_a_paragraph() {
        let (mut v, page) = setup();
        let text = "Para one\n  child para\n\n  - child item\n    - grandchild\n\nPara two \\\nits second line\n  - under the second\n";
        save(&mut v, &page, text);
        let p1 = kid(&v, &page, 0);
        assert!(is_paragraph(&v.store, &p1) && p1.text == "Para one");
        let (c0, c1) = (kid(&v, &p1.id, 0), kid(&v, &p1.id, 1));
        assert!(is_paragraph(&v.store, &c0) && c0.text == "child para", "a child paragraph");
        assert!(!is_paragraph(&v.store, &c1) && c1.text == "child item");
        assert_eq!(kid(&v, &c1.id, 0).text, "grandchild");
        let p2 = kid(&v, &page, 1);
        assert_eq!(p2.text, "Para two\nits second line", "a line break, not a child");
        assert_eq!(kid(&v, &p2.id, 0).text, "under the second");
        assert_eq!(v.store.children(&page).unwrap().len(), 2);
        assert_eq!(body(&v, &page), text);
        // The id ends a paragraph's last line; its children follow.
        let r = render(&v.store, &page).unwrap().text;
        let (s1, s2) = (v.store.short(&p1.id), v.store.short(&p2.id));
        assert!(r.contains(&format!("Para one ^{s1}\n  child para ^")), "{r}");
        assert!(r.contains(&format!("Para two \\\nits second line ^{s2}\n  - under the second ^")), "{r}");
        round_trips(&mut v, &page);
    }

    #[test]
    fn an_item_s_line_break_and_an_indented_line_differ() {
        let (mut v, page) = setup();
        save(&mut v, &page, "- [ ] first \\\n      second\n  a child paragraph\n- \n");
        let t = kid(&v, &page, 0);
        assert_eq!((t.text.as_str(), t.status.as_deref()), ("first\nsecond", Some("todo")));
        assert!(is_paragraph(&v.store, &kid(&v, &t.id, 0)));
        assert_eq!(body(&v, &page), "- [ ] first \\\n  second\n  a child paragraph\n- (empty)\n");
        round_trips(&mut v, &page);
        assert_eq!(v.store.children(&page).unwrap().len(), 2, "an empty item keeps its id (shown as capture writes it)");
    }

    #[test]
    fn indenting_a_paragraph_line_makes_it_a_child() {
        // editing.md E22: Tab on a paragraph's later line makes that line a child paragraph.
        let (mut v, page) = setup();
        save(&mut v, &page, "Para line\nfirst subtask\n");
        let p = kid(&v, &page, 0);
        assert_eq!(p.text, "Para line first subtask", "a wrapped line at the paragraph's indent");
        let r = render(&v.store, &page).unwrap();
        let short = v.store.short(&p.id);
        let edited = r.text.replace(&format!("Para line first subtask ^{short}"), &format!("Para line ^{short}\n  first subtask"));
        apply_text(&mut v, &r, &edited);
        let p = kid(&v, &page, 0);
        assert_eq!(p.text, "Para line");
        let c = kid(&v, &p.id, 0);
        assert!(is_paragraph(&v.store, &c) && c.text == "first subtask");
        round_trips(&mut v, &page);
    }

    #[test]
    fn every_list_style_at_three_depths() {
        for s in ["1.", "1)", "a.", "a)", "A.", "i.", "I.", "I)", "-", "*", "+"] {
            let (mut v, page) = setup();
            let st = ListStyle::parse(s).unwrap();
            let m = |n| st.marker(n);
            let text = format!("{} one\n  {} sub one\n    {} deep one\n    {} deep two\n  {} sub two\n{} two\n", m(1), m(1), m(1), m(2), m(2), m(2));
            save(&mut v, &page, &text);
            assert_eq!(body(&v, &page), text, "{s}");
            round_trips(&mut v, &page);
            // Each list's style is on its first item, written only where it isn't the default.
            let one = kid(&v, &page, 0);
            let sub = kid(&v, &one.id, 0);
            let deep = kid(&v, &sub.id, 0);
            let want = |default: &str| (s != default).then(|| s.to_string());
            let (d2, d3) = if st.is_ordered() { ("a.", "i.") } else { ("-", "-") };
            assert_eq!(list_of(&v, &one.id), want("-"), "{s} at depth 1");
            assert_eq!(list_of(&v, &sub.id), want(d2), "{s} at depth 2");
            assert_eq!(list_of(&v, &deep.id), want(d3), "{s} at depth 3");
            // No other item stores anything, and none a number.
            for other in [kid(&v, &sub.id, 1), kid(&v, &one.id, 1), kid(&v, &page, 1)] {
                assert!(v.store.props_of(&other.id).unwrap().is_empty(), "{s}");
            }
        }
    }

    #[test]
    fn the_depth_sequence_needs_no_props_below_the_first_list() {
        let (mut v, page) = setup();
        let text = "1. one\n  a. sub\n    i. deep\n      1. deeper\n";
        save(&mut v, &page, text);
        assert_eq!(body(&v, &page), text);
        let one = kid(&v, &page, 0);
        let sub = kid(&v, &one.id, 0);
        let deep = kid(&v, &sub.id, 0);
        let deeper = kid(&v, &deep.id, 0);
        assert_eq!(list_of(&v, &one.id).as_deref(), Some("1."));
        assert_eq!([list_of(&v, &sub.id), list_of(&v, &deep.id), list_of(&v, &deeper.id)], [None, None, None]);
        round_trips(&mut v, &page);
    }

    #[test]
    fn a_reorder_renumbers_and_the_style_stays_with_the_list() {
        let (mut v, page) = setup();
        save(&mut v, &page, "a. alpha\nb. beta\nc. gamma\n");
        let r = render(&v.store, &page).unwrap();
        let line = |t: &str| r.text.lines().find(|l| l.contains(t)).unwrap().to_string();
        // The first item moved to the end, its old letter kept as typed.
        let edited = format!("# Notes\n\n{}\n{}\n{}\n", line("beta"), line("gamma"), line("alpha"));
        let sum = apply_text(&mut v, &r, &edited);
        assert_eq!(sum.moved, 1);
        assert_eq!(body(&v, &page), "a. beta\nb. gamma\nc. alpha\n");
        let ids: Vec<String> = v.store.children(&page).unwrap().into_iter().map(|n| n.id).collect();
        assert_eq!(list_of(&v, &ids[0]).as_deref(), Some("a."), "the new first item takes the style");
        assert_eq!(list_of(&v, &ids[2]), None, "the old one no longer starts a list");
        round_trips(&mut v, &page);
    }

    #[test]
    fn deleting_a_list_s_first_item_keeps_its_style() {
        let (mut v, page) = setup();
        save(&mut v, &page, "I. alpha\nII. beta\nIII. gamma\n");
        let r = render(&v.store, &page).unwrap();
        let edited: String = r.text.lines().filter(|l| !l.contains("alpha")).map(|l| format!("{l}\n")).collect();
        let sum = apply_text(&mut v, &r, &edited);
        assert_eq!(sum.deleted, 1);
        assert_eq!(body(&v, &page), "I. beta\nII. gamma\n");
        assert_eq!(list_of(&v, &kid(&v, &page, 0).id).as_deref(), Some("I."));
        round_trips(&mut v, &page);
    }

    #[test]
    fn lists_of_different_styles_on_one_level() {
        let (mut v, page) = setup();
        let text = "- milk\n- eggs\n\nThen, in order:\n\n1. preheat\n2. whisk\n\nAnd finally:\n\ni) serve\nii) eat\n";
        save(&mut v, &page, text);
        assert_eq!(body(&v, &page), text);
        let kids = v.store.children(&page).unwrap();
        let styles: Vec<Option<String>> = kids.iter().map(|k| list_of(&v, &k.id)).collect();
        assert_eq!(styles, [None, None, None, Some("1.".into()), None, None, Some("i)".into()), None]);
        round_trips(&mut v, &page);
    }

    #[test]
    fn a_change_of_list_type_starts_a_new_list() {
        // CommonMark and the editor: bullet <-> ordered, another bullet character, another count
        // or delimiter each start a list. Each new list's first item carries its style.
        for (text, want) in [
            ("- a\n- b\n1. c\n2. d\n", "- a\n- b\n1. c\n2. d\n"),
            ("* a\n- b\n", "* a\n- b\n"),
            ("b) one\n1. two\n- three\nvii. four\n", "a) one\n1. two\n- three\ni. four\n"),
            ("1. one\n1) two\n", "1. one\n1) two\n"),
        ] {
            let (mut v, page) = setup();
            save(&mut v, &page, text);
            assert_eq!(body(&v, &page), want, "{text:?}");
            round_trips(&mut v, &page);
        }
        let styles = |v: &Vault, page: &str| -> Vec<Option<String>> { v.store.children(page).unwrap().iter().map(|k| list_of(v, &k.id)).collect() };
        let (mut v, page) = setup();
        save(&mut v, &page, "- a\n- b\n1. c\n2. d\n");
        assert_eq!(styles(&v, &page), [None, None, Some("1.".into()), None]);
        // `* a` / `- b`: the second list's first item needs `-` written, though it's the default.
        let (mut v, page) = setup();
        save(&mut v, &page, "* a\n- b\n");
        assert_eq!(styles(&v, &page), [Some("*".into()), Some("-".into())]);
    }

    #[test]
    fn letters_take_roman_looking_counts_and_a_blank_line_doesn_t_split() {
        // `i.` after `h.` is the ninth letter; a blank line between items of one kind is a loose
        // list (CommonMark), kept as the item's gap.
        let (mut v, page) = setup();
        let text = "a. a\nb. b\nc. c\nd. d\ne. e\nf. f\ng. g\nh. h\ni. i\n\nj. j\n";
        save(&mut v, &page, text);
        assert_eq!(body(&v, &page), text);
        let kids = v.store.children(&page).unwrap();
        assert!(kids[1..].iter().all(|k| list_of(&v, &k.id).is_none()), "one list");
        round_trips(&mut v, &page);
    }

    #[test]
    fn a_numbered_list_right_after_bullets_keeps_its_style() {
        // The live repro (Q4 Planning): a numbered list typed under a bullet, with a roman list
        // under its second item.
        let (mut v, page) = setup();
        let text = "- Theme for the quarter: fewer, deeper bets\n1. Pack the tent\n2. Buy fuel\n  i. Check the forecast\n";
        save(&mut v, &page, text);
        assert_eq!(body(&v, &page), text);
        round_trips(&mut v, &page);
    }

    #[test]
    fn a_bullet_list_under_a_numbered_one_round_trips() {
        let (mut v, page) = setup();
        let text = "1. one\n  - sub\n  - sub two\n2. two\n  * starred\n";
        save(&mut v, &page, text);
        assert_eq!(body(&v, &page), text);
        let one = kid(&v, &page, 0);
        assert_eq!(list_of(&v, &kid(&v, &one.id, 0).id).as_deref(), Some("-"), "differs from the default a.");
        round_trips(&mut v, &page);
        // Cycled back to the default: the prop goes.
        let r = render(&v.store, &page).unwrap();
        let edited = r.text.replacen("  - sub ", "  a. sub ", 1).replacen("  - sub two ", "  b. sub two ", 1);
        apply_text(&mut v, &r, &edited);
        assert_eq!(list_of(&v, &kid(&v, &one.id, 0).id), None);
        assert_eq!(body(&v, &page), "1. one\n  a. sub\n  b. sub two\n2. two\n  * starred\n");
    }

    #[test]
    fn a_list_after_a_paragraph_starts_at_one() {
        let (mut v, page) = setup();
        save(&mut v, &page, "1. a\n2. b\n\nBetween.\n\n3. c\n4. d\n");
        assert_eq!(body(&v, &page), "1. a\n2. b\n\nBetween.\n\n1. c\n2. d\n");
        round_trips(&mut v, &page);
    }

    #[test]
    fn a_paragraph_that_starts_like_an_item_stays_a_paragraph() {
        let (mut v, page) = setup();
        save(&mut v, &page, "1\\. not a list\n\n\\+ nor a bullet\n");
        let (a, b) = (kid(&v, &page, 0), kid(&v, &page, 1));
        assert!(is_paragraph(&v.store, &a) && a.text == "1. not a list");
        assert!(is_paragraph(&v.store, &b) && b.text == "+ nor a bullet");
        assert_eq!(body(&v, &page), "1\\. not a list\n\n\\+ nor a bullet\n");
        assert_eq!(list_of(&v, &a.id), None);
        round_trips(&mut v, &page);
    }

    #[test]
    fn one_note_s_edit_reads_the_same_way() {
        let (mut v, page) = setup();
        save(&mut v, &page, "1. one\n2. two\n");
        let two = kid(&v, &page, 1);
        let r = render(&v.store, &two.id).unwrap();
        let short = v.store.short(&two.id);
        assert!(r.text.starts_with(&format!("- two ^{short}\n")), "shown alone: {}", r.text);
        let edited = r.text.replace(&format!("- two ^{short}"), &format!("- two ^{short}\n  i. first\n  ii. second\n  A paragraph under it"));
        apply_text(&mut v, &r, &edited);
        assert_eq!(list_of(&v, &kid(&v, &page, 0).id).as_deref(), Some("1."), "its own list is untouched");
        assert_eq!(list_of(&v, &two.id), None);
        assert_eq!(list_of(&v, &kid(&v, &two.id, 0).id).as_deref(), Some("i."));
        assert!(is_paragraph(&v.store, &kid(&v, &two.id, 2)));
        assert_eq!(body(&v, &page), "1. one\n2. two\n  i. first\n  ii. second\n  A paragraph under it\n");
    }

    #[test]
    fn builder_moves_and_deletes_keep_a_list_s_style() {
        // Not only the editor's save: any builder write keeps the style on the list's first item.
        let (mut v, page) = setup();
        save(&mut v, &page, "i) alpha\nii) beta\n\nprose\n\n1. one\n2. two\n");
        let kids: Vec<String> = v.store.children(&page).unwrap().into_iter().map(|n| n.id).collect();
        let write = |v: &mut Vault, f: &dyn Fn(&mut TxBuilder) -> Result<()>| {
            v.transact(|s| {
                let mut b = TxBuilder::new(s, today());
                f(&mut b)?;
                Ok((b.finish()?, ()))
            })
            .unwrap();
        };
        // beta to the top of its list, then the second list's first item deleted.
        write(&mut v, &|b| b.move_to(&kids[1], Some(page.clone()), None, Some(&kids[0])));
        write(&mut v, &|b| b.delete(&kids[3]).map(|_| ()));
        assert_eq!(body(&v, &page), "i) beta\nii) alpha\n\nprose\n\n1. two\n");
        assert_eq!([list_of(&v, &kids[1]), list_of(&v, &kids[0]), list_of(&v, &kids[4])], [Some("i)".into()), None, Some("1.".into())]);
        round_trips(&mut v, &page);
    }

    #[test]
    fn a_list_fix_up_that_fails_fails_the_write() {
        // The normaliser's store error comes back from finish (here: a store that refuses the
        // provisional apply), so the move isn't committed without its list fix-up.
        let (mut v, page) = setup();
        save(&mut v, &page, "1. alpha\n2. beta\n");
        let kids: Vec<String> = v.store.children(&page).unwrap().into_iter().map(|n| n.id).collect();
        v.store.conn.execute_batch("PRAGMA query_only = ON").unwrap();
        let mut b = TxBuilder::new(&v.store, today());
        b.move_to(&kids[1], Some(page.clone()), None, Some(&kids[0])).unwrap();
        assert!(b.finish().is_err(), "the write fails, and says why");
        v.store.conn.execute_batch("PRAGMA query_only = OFF").unwrap();
        assert_eq!(body(&v, &page), "1. alpha\n2. beta\n", "nothing written");
    }

    #[test]
    fn the_blocks_copy_is_the_edit_text_without_ids() {
        let (mut v, page) = setup();
        let text = "Para \\\nline two\n  1. child\n  2. child two\n\n- [ ] task due:2026-10-20 \\\n  more\n\n1\\. escaped\n";
        save(&mut v, &page, text);
        let copy = crate::outline::markdown(&crate::outline::render(&v.store, &page).unwrap());
        assert_eq!(copy, body(&v, &page));
        assert_eq!(copy, text);
    }
}
