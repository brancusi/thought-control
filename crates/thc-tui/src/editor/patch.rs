//! Changes made elsewhere (another device, an agent, the CLI), taken into the open document.

use super::doc::{Doc, Line};
use std::collections::{HashMap, HashSet};
use thc_core::outline::Block;

/// What a refresh did, for the app to follow up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patched {
    /// A kind, parent or place changed elsewhere on a line you aren't on: reopen the document
    /// (keeping what's typed) rather than patch it.
    pub reopen: bool,
    /// A text changed elsewhere on the caret's line or an edited one, held until it's left:
    /// `Some(edited)` to announce.
    pub announce: Option<bool>,
    /// A `≠` line whose other side isn't named yet.
    pub unnamed_conflicts: bool,
}

/// Each block's previous sibling as the vault has it, the root's children counting as parentless.
fn prev_siblings<'a>(all: &'a [Block], root: &str) -> HashMap<&'a str, Option<&'a str>> {
    let mut prev_sib: HashMap<&str, Option<&str>> = HashMap::new();
    let mut last_under: HashMap<Option<&str>, &str> = HashMap::new();
    for b in all {
        let par = b.parent.as_deref().filter(|p| *p != root);
        prev_sib.insert(b.id.as_str(), last_under.get(&par).copied());
        last_under.insert(par, b.id.as_str());
    }
    prev_sib
}

impl Doc {
    /// Take in what the vault has now. `blocks` and `gone` are the vault's view of the saved
    /// lines here (`outline::render_ids`); `all` is the whole document as the vault has it
    /// (`outline::render_for_editor`), when it could be read. Lines you aren't on and haven't
    /// edited update in place; lines gone from the vault leave; notes new to it come in at their
    /// place. The caret never moves off its line (tui-editor.md §9).
    pub fn patch(&mut self, blocks: Vec<Block>, gone: Vec<String>, all: Option<&[Block]>, root: &str, today: chrono::NaiveDate) -> Patched {
        let mut out = Patched::default();
        let caret_id = self.line().id.clone();
        for b in blocks {
            let Some(l) = self.lines_mut().iter_mut().find(|l| l.id == b.id) else { continue };
            l.take_fields(&b, today);
            l.status = b.status.clone();
            l.saved_status = b.status.clone();
            // A gap set elsewhere (another device, thc edit) is taken as it is.
            if l.gap == l.saved_gap {
                l.gap = b.gap;
            }
            l.saved_gap = b.gap;
            l.conflict = b.conflict;
            if l.id == caret_id || l.edited() {
                // Never rewritten under the caret: announced now, applied when it's left.
                if b.text != l.saved.clone().unwrap_or_default() && l.remote_text.as_deref() != Some(b.text.as_str()) {
                    l.remote_text = Some(b.text.clone());
                    out.announce = Some(l.edited());
                }
                continue;
            }
            if l.text != b.text {
                l.text = b.text.clone();
            }
            l.base = b.text_rev.clone();
            l.saved = Some(b.text.clone());
        }
        for id in gone {
            if id == caret_id {
                // Deleted elsewhere while you're on it: it goes when you leave it.
                if let Some(l) = self.lines_mut().iter_mut().find(|l| l.id == id) {
                    l.remote_shape = true;
                }
                continue;
            }
            if let Some(i) = self.lines().iter().position(|l| l.id == id && !l.edited()) {
                self.lines_mut().remove(i);
                if self.view.caret.line > i {
                    self.view.caret.line -= 1;
                }
            }
        }
        // Notes new to this document (made by another device or an agent): in at their place
        // in the vault's order, after the note they follow here. The caret's line keeps its
        // place in the text (two-device soak: they never appeared until the day was reopened).
        if let Some(all) = all {
            // The vault's shape for lines saved here and untouched: a kind, parent or place
            // changed elsewhere means reopening (keeping what's typed). Patching only text let
            // the next save move a line back where it was here, undoing the other device's move.
            {
                let prev_sib = prev_siblings(all, root);
                let caret_id = self.line().id.clone();
                let mut reshaped = false;
                for l in self.lines_mut().iter_mut().filter(|l| !l.is_new) {
                    let Some(b) = all.iter().find(|b| b.id == l.id) else { continue };
                    let par = b.parent.clone().filter(|p| p != root);
                    let saved_par = l.saved_parent.clone().filter(|p| p != root);
                    if Some(b.kind) != l.saved_kind || par != saved_par || prev_sib.get(b.id.as_str()).copied().flatten().map(str::to_string) != l.saved_after {
                        // On it or typed on it: held until it's left, like a remote text.
                        if l.edited() || l.id == caret_id {
                            l.remote_shape = true;
                        } else {
                            reshaped = true;
                        }
                    }
                }
                if reshaped {
                    return Patched { reopen: true, ..Patched::default() };
                }
            }
            // (A line deleted here and not saved yet is still in the vault: it stays out.)
            let mut have: HashSet<String> = self.lines().iter().map(|l| l.id.clone()).chain(self.engine.deleted().iter().cloned()).collect();
            for (i, b) in all.iter().enumerate() {
                if have.contains(&b.id) {
                    continue;
                }
                let prev = all[..i].iter().rev().find(|p| have.contains(&p.id)).map(|p| p.id.clone());
                let at = prev.and_then(|p| self.lines().iter().position(|l| l.id == p)).map_or(0, |x| x + 1);
                self.lines_mut().insert(at, Line::from_block(b, today));
                if at <= self.view.caret.line {
                    self.view.caret.line += 1;
                }
                have.insert(b.id.clone());
            }
            // Each saved line's neighbour as the vault has it now (or the next save moves lines
            // that didn't move).
            let prev_sib = prev_siblings(all, root);
            for l in self.lines_mut().iter_mut().filter(|l| !l.is_new) {
                if let Some(p) = prev_sib.get(l.id.as_str()) {
                    l.saved_after = p.map(str::to_string);
                }
            }
        }
        out.unnamed_conflicts = self.lines().iter().any(|l| l.conflict && l.conflict_with.is_none());
        out
    }
}
