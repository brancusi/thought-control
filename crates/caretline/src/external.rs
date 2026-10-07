//! Changes from outside the editor: another device, a daemon, an agent. [`Msg::External`]
//! carries [`ExtChange`]s, which the engine turns into transactions and applies to the
//! document **outside the undo history**:
//!
//! - every view's selection is mapped through them (as an edit from another view is);
//! - marks follow the plain rules, and each change binds or drops its block's mark;
//! - the history is transformed over them ([`History::rebase_over`]), so a later undo takes
//!   back only local edits and never the change from elsewhere. Redo past it is gone.
//!
//! With [`ExternalUndo::Barrier`] (a fallback) the history is trimmed instead: undo stops at
//! the change.
//!
//! [`History::rebase_over`]: crate::helix::history::History::rebase_over

use serde::{Deserialize, Serialize};

use crate::helix::history::History;
use crate::helix::{ChangeSet, Rope, Tendril, Transaction};
use crate::marks::{MarkAttrs, Fixup, Mark, MarkDelta, MarkId};
use crate::msg::{Effect, Msg};
use crate::outline::{Hang, Kind, NewBlock};
use crate::state::{Document, ExternalUndo, View};

/// One change from elsewhere. Block changes name blocks by mark and apply to outline
/// documents; `replace` works on any document. Changes in one message apply in order, each
/// to the document the previous one left.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum ExtChange {
    /// New content for a block: its lines after the marker (`\n` for soft breaks). The
    /// marker and indentation stay.
    ReplaceContent { id: MarkId, text: String },
    /// A block's depth, kind and tag: its indentation and list marker are rewritten (a
    /// number, heading or quote marker stays, as content).
    SetShape {
        id: MarkId,
        #[serde(default)]
        depth: u16,
        kind: Kind,
        /// A bullet's tag (`- [c] `).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<char>,
    },
    /// A block's blank row before it (`null`: the default for its kind).
    SetGap {
        id: MarkId,
        #[serde(default)]
        gap: Option<bool>,
    },
    /// A mark's payload (`null`: none). Any document with marks; the text is untouched.
    SetData {
        id: MarkId,
        #[serde(default)]
        data: Option<serde_json::Value>,
    },
    /// A new block after a block (its own lines, not its children), or first when `after` is
    /// absent. `block.mark` binds the new block to that id when it is free.
    InsertBlock {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<MarkId>,
        block: NewBlock,
    },
    /// A block's lines go (its continuation lines too, not its children).
    RemoveBlock { id: MarkId },
    /// Chars `[from, to)` of the current text become `text` (any document).
    Replace {
        from: usize,
        to: usize,
        #[serde(default)]
        text: String,
    },
}

/// Applies `Msg::External` to the document and every view.
pub(crate) fn apply(doc: &mut Document, views: &mut [View], msg: Msg) -> Vec<Effect> {
    let Msg::External { changes } = msg else { return Vec::new() };
    let mut effects = Vec::new();
    let tops = crate::views::tops(doc, views);
    let mut applied: Vec<ChangeSet> = Vec::new();
    let mut marks_changed = false;
    for change in &changes {
        match one(doc, change) {
            Ok(Some(cs)) => applied.push(cs),
            Ok(None) => marks_changed = true,
            Err(why) => effects.push(Effect::Notice { text: format!("a change from elsewhere was skipped: {why}") }),
        }
    }
    if applied.is_empty() && !marks_changed {
        return effects;
    }
    doc.edits.0 = doc.edits.0.wrapping_add(1);
    doc.rev += 1;
    if let Some(remote) = applied.iter().cloned().reduce(|a, b| a.compose(b)) {
        match doc.config.external_undo {
            ExternalUndo::Transform => transform_history(doc, &remote),
            ExternalUndo::Barrier => barrier(doc),
        }
    }
    for (i, v) in views.iter_mut().enumerate() {
        crate::views::rebase(doc, v, &applied, tops[i]);
    }
    doc.dirty = doc.compute_dirty();
    effects
}

/// Applies one change. Returns its text changes (`None` for a change of marks only), or why
/// it was skipped.
fn one(doc: &mut Document, change: &ExtChange) -> Result<Option<ChangeSet>, String> {
    let le = doc.config.line_ending.as_str().to_string();
    let lines = |t: &str| crate::update::normalize_line_endings(t, &le);
    let rope = doc.text.clone();
    let text = rope.slice(..);
    let len = text.len_chars();
    if let ExtChange::Replace { from, to, text: ins } = change {
        let from = (*from).min(len);
        let to = (*to).clamp(from, len);
        // A change inside one block's lines is that block's: it keeps its mark even where the
        // deleted text, read alone, is a whole line (an empty line's break: "a\n\n" → "a\n").
        let keep = doc.blocks().and_then(|o| {
            let b = o.block_at(text, from);
            (b.start <= from && to <= b.end && b.id != MarkId(u64::MAX)).then(|| (b.id, b.start, b.attrs.clone()))
        });
        return Ok(Some(edit(doc, vec![least(text, from, to, lines(ins))], move |m, _| keep_mark(m, keep))));
    }
    if let ExtChange::SetData { id, data } = change {
        doc.marks.set_data(*id, data.clone()).ok_or(format!("no mark {}", id.0))?;
        doc.derived.marks_changed();
        return Ok(None);
    }
    let o = doc.blocks().ok_or("block changes need an outline document")?;
    let cfg = doc.outline.clone().expect("outline");
    let block = |id: MarkId| o.get(id).cloned().ok_or(format!("no block {}", id.0));
    match change {
        ExtChange::Replace { .. } | ExtChange::SetData { .. } => unreachable!(),
        ExtChange::ReplaceContent { id, text: content } => {
            let b = block(*id)?;
            let keep = Some((b.id, b.start, b.attrs.clone()));
            Ok(Some(edit(doc, vec![least(text, b.content_start(), b.end, lines(content))], move |m, _| keep_mark(m, keep))))
        }
        ExtChange::SetShape { id, depth, kind, tag } => {
            let b = block(*id)?;
            // The indentation and list marker; a number, heading or quote marker is content.
            let list = match (b.kind, b.hang) {
                (Kind::Bullet, Hang::Bullet) => b.prefix_len,
                _ => b.indent,
            };
            let line_end = crate::helix::line_ending::line_end_char_index(&text, b.first_line);
            let rest = text.slice(b.start + list..line_end).to_string();
            let nb = NewBlock { depth: *depth, kind: *kind, tag: *tag, text: rest.clone(), gap: None, mark: None };
            let full = nb.to_lines(&cfg);
            let prefix = full[..full.len() - rest.len()].to_string();
            Ok(Some(edit(doc, vec![(b.start, b.start + list, prefix)], |_, _| {})))
        }
        ExtChange::SetGap { id, gap } => {
            block(*id)?;
            doc.marks.set_gap(*id, *gap);
            doc.derived.marks_changed();
            Ok(None)
        }
        ExtChange::InsertBlock { after, block: nb } => {
            let (at, ins, line) = match after {
                Some(id) => {
                    let b = block(*id)?;
                    (b.end, format!("{le}{}", lines(&nb.to_lines(&cfg))), b.last_line() + 1)
                }
                None => (0, format!("{}{le}", lines(&nb.to_lines(&cfg))), 0),
            };
            let (mark, gap) = (nb.mark, nb.gap);
            Ok(Some(edit(doc, vec![(at, at, ins)], move |marks, new| {
                let pos = new.line_to_char(line);
                let attrs = MarkAttrs::gap(gap);
                let placed = mark.is_some_and(|id| marks.insert(Mark { pos, id, attrs: attrs.clone() }).is_ok());
                if !placed {
                    match marks.at(pos) {
                        Some(id) => {
                            marks.set_gap(id, attrs.gap);
                        }
                        None => {
                            marks.mint_with(pos, attrs);
                        }
                    }
                }
            })))
        }
        ExtChange::RemoveBlock { id } => {
            let b = block(*id)?;
            let lines_total = text.len_lines();
            let range = if b.last_line() + 1 < lines_total {
                (b.start, text.line_to_char(b.last_line() + 1))
            } else if b.first_line > 0 {
                (crate::helix::line_ending::line_end_char_index(&text, b.first_line - 1), b.end)
            } else {
                (0, b.end)
            };
            let id = *id;
            Ok(Some(edit(doc, vec![(range.0, range.1, String::new())], move |marks, _| {
                marks.remove(id);
            })))
        }
    }
}

/// Replacing `[from, to)` with `new`, as the least change: the chars both share at the start
/// and the end stay, so carets and undo steps there are untouched.
fn least(text: crate::helix::RopeSlice, from: usize, to: usize, new: String) -> (usize, usize, String) {
    let old: Vec<char> = text.slice(from..to).chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let pre = old.iter().zip(&new_chars).take_while(|(a, b)| a == b).count();
    let max_suf = old.len().min(new_chars.len()) - pre;
    let suf = old.iter().rev().zip(new_chars.iter().rev()).take(max_suf).take_while(|(a, b)| a == b).count();
    let ins: String = new_chars[pre..new_chars.len() - suf].iter().collect();
    (from + pre, to - suf, ins)
}

/// Applies changes (sorted, in current positions) to the text and the marks, outside the
/// history; `fix` adjusts the marks after they are mapped. Returns the change set.
/// Puts a block's mark back at its start if a change inside the block took it (its start is
/// before the change, so it's still a line start where it was).
fn keep_mark(marks: &mut crate::marks::Marks, keep: Option<(MarkId, usize, crate::marks::MarkAttrs)>) {
    let Some((id, pos, attrs)) = keep else { return };
    if !marks.contains(id) && marks.at(pos).is_none() {
        let _ = marks.insert(crate::marks::Mark { pos, id, attrs });
    }
}

fn edit(
    doc: &mut Document,
    changes: Vec<(usize, usize, String)>,
    fix: impl FnOnce(&mut crate::marks::Marks, crate::helix::RopeSlice),
) -> ChangeSet {
    let txn = Transaction::change(
        &doc.text,
        changes.into_iter().map(|(a, b, t)| (a, b, (!t.is_empty()).then(|| Tendril::from(t.as_str())))),
    );
    let cs = txn.changes().clone();
    let old = doc.text.clone();
    cs.apply(&mut doc.text);
    doc.touched.record(&cs);
    doc.derived.edited(&cs);
    doc.marks.map(old.slice(..), doc.text.slice(..), &cs);
    fix(&mut doc.marks, doc.text.slice(..));
    doc.derived.marks_changed();
    if doc.outline.is_some() {
        crate::outline::mint_missing(doc);
    }
    cs
}

/// Transforms the undo history over `remote` (see the module docs), keeping the save point,
/// the open edit run and the mark log on the revisions they belong to.
fn transform_history(doc: &mut Document, remote: &ChangeSet) {
    doc.fit_mark_log();
    let old_len = doc.history.len();
    let kept = doc.history.rebase_over(remote, &doc.text);
    let mut new_index: Vec<Option<usize>> = vec![None; old_len];
    for (i, k) in kept.iter().enumerate() {
        new_index[k.old] = Some(i);
    }
    let remap = |r: Option<usize>| r.and_then(|r| new_index.get(r).copied().flatten());
    doc.saved_revision = remap(doc.saved_revision);
    doc.saving = remap(doc.saving);
    doc.run = doc.run.and_then(|mut run| {
        run.revision = remap(Some(run.revision))?;
        Some(run)
    });
    // A revision's undo fix-up describes its parent's document; its redo fix-up its own.
    let old_log = std::mem::take(&mut doc.mark_log);
    let mut log = Vec::with_capacity(kept.len());
    for (i, k) in kept.iter().enumerate() {
        let d = old_log.get(k.old).cloned().unwrap_or_default();
        if i == 0 {
            log.push(MarkDelta::default());
        } else if d.is_empty() {
            log.push(d);
        } else {
            let parent = &kept[i - 1];
            log.push(MarkDelta {
                undo: map_fixup(&d.undo, &parent.remote, &parent.text),
                redo: map_fixup(&d.redo, &k.remote, &k.text),
            });
        }
    }
    doc.mark_log = log;
}

/// Moves a fix-up's marks through `changes` onto `text` (the document after them), each to
/// the start of the line its position lands on.
fn map_fixup(f: &Fixup, changes: &ChangeSet, text: &Rope) -> Fixup {
    if f.set.is_empty() {
        return f.clone();
    }
    let t = text.slice(..);
    let set = f
        .set
        .iter()
        .map(|m| {
            let p = changes.map_pos(m.pos.min(changes.len()), crate::helix::Assoc::After);
            Mark { pos: crate::marks::line_start_at(t, p), ..m.clone() }
        })
        .collect();
    Fixup { set, drop: f.drop.clone() }
}

/// The fallback: the history starts again at the change, so undo stops there.
fn barrier(doc: &mut Document) {
    let clean = !doc.compute_dirty();
    doc.history = History::default();
    doc.mark_log = vec![MarkDelta::default()];
    doc.saved_revision = clean.then_some(0);
    doc.saving = None;
    doc.run = None;
    doc.undo_floor = true;
}
