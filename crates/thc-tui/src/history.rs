//! Navigation history: ⌘[ back, ⌘] forward (navigation.md §7).
//!
//! A place is a vault, a view, the page or day open in it, the list's selection and the caret
//! (an anchor: note id + byte) and scroll. Steps aren't recorded route by route: after every
//! input the recorder compares where you are with the current entry. The same place updates
//! the entry (so back returns to where you were, not where you arrived); a different one is a
//! new step. Tab cycling coalesces, big in-document jumps (⌘↑ ⌘↓) are marked explicitly.
//! One history across vaults, per device, beside the vault caches.

use crate::app::{App, View, VIEWS};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The cap (§7.3): the oldest entries drop.
pub(crate) const CAP: usize = 100;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub vault: PathBuf,
    pub view: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caret: Option<(String, usize)>,
    #[serde(default)]
    pub scroll: usize,
    /// What the list shows: `¶ Lisbon flat`, `§ Tue 06 Oct`, `Today`.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub ms: i64,
    /// An agent changed the sidebar's stack here (sidebar.md §10.4): ⌘[ takes it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<crate::sidebar::AgentChange>,
}

impl Place {
    /// Same place: same vault, view and document. The caret only updates the entry.
    fn same(&self, o: &Place) -> bool {
        self.vault == o.vault && self.view == o.view && self.page == o.page && self.day == o.day
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct History {
    pub entries: Vec<Place>,
    pub pos: usize,
    /// The last step came from Tab / ⇧Tab cycling: the next one replaces it (§7.1).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tab_run: bool,
}

impl History {
    fn file(cache: &std::path::Path) -> PathBuf {
        cache.parent().unwrap_or(cache).join("history.json")
    }

    pub fn load(cache: &std::path::Path) -> History {
        let mut h: History = std::fs::read(Self::file(cache)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        h.pos = h.pos.min(h.entries.len().saturating_sub(1));
        h
    }

    pub fn save(&self, cache: &std::path::Path) {
        if let Ok(b) = serde_json::to_vec(self) {
            let _ = std::fs::write(Self::file(cache), b);
        }
    }

    /// A new step: what was ahead is dropped (§7.3), and the oldest past the cap.
    fn push(&mut self, p: Place) {
        if !self.entries.is_empty() {
            self.entries.truncate(self.pos + 1);
        }
        self.entries.push(p);
        if self.entries.len() > CAP {
            let over = self.entries.len() - CAP;
            self.entries.drain(..over);
        }
        self.pos = self.entries.len() - 1;
    }
}

impl App {
    /// The real vault this session is in (a snapshot's scratch copy names its origin).
    fn real_vault(&self) -> PathBuf {
        self.vault.origin.as_ref().map_or(self.vault.paths.vault.clone(), |o| o.vault.clone())
    }

    /// Where you are now.
    pub fn place(&self) -> Place {
        let doc_open = self.doc.is_some();
        let page = if self.view == View::Pages { self.page_open.clone() } else { None };
        let day = (self.view == View::Journal).then(|| self.journal_date.format("%Y-%m-%d").to_string());
        let caret = self.doc.as_ref().map(|d| {
            // A new, empty line isn't a place: the line above it is (as caret memory).
            let a = d.place_anchor().unwrap_or_else(|| d.caret_anchor());
            (a.id, a.byte)
        });
        let g = self.theme.glyphs();
        let label = match (&page, &day) {
            (Some(id), _) => format!("{} {}", g.page, self.vault.store.node(id).ok().flatten().map(|n| self.vault.store.render_text(&n.label())).unwrap_or_default()),
            (None, Some(_)) => format!("{} {}", g.journal, self.journal_date.format("%a %d %b")),
            _ => format!("{:?}", self.view),
        };
        Place {
            vault: self.real_vault(),
            view: format!("{:?}", self.view),
            page,
            day,
            selected: if doc_open { None } else { self.selected.clone() },
            caret,
            scroll: self.doc.as_ref().map_or(self.scroll, |d| d.scroll()),
            label,
            ms: self.ui.now_ms as i64,
            agent: None,
        }
    }

    /// An agent's change to the stack is its own history step (sidebar.md §10.4).
    pub fn history_agent_step(&mut self, change: crate::sidebar::AgentChange) {
        let mut p = self.place();
        p.agent = Some(change);
        self.history.push(p);
        self.history.tab_run = false;
        self.history.save(&self.vault.paths.cache);
    }

    /// After every input: the same place updates its entry, another place is a step.
    /// `via_tab`: the input was Tab / ⇧Tab cycling views (consecutive ones coalesce).
    pub fn history_tick(&mut self, via_tab: bool) {
        let p = self.place();
        let h = &mut self.history;
        match h.entries.get_mut(h.pos) {
            Some(cur) if cur.same(&p) => {
                let agent = cur.agent.take();
                *cur = p;
                cur.agent = agent;
                return;
            }
            Some(_) if via_tab && h.tab_run => {
                // Tab Tab Tab is one step, at the view you stop on.
                let pos = h.pos;
                h.entries[pos] = p;
            }
            _ => h.push(p),
        }
        h.tab_run = via_tab;
        self.history.save(&self.vault.paths.cache);
    }

    /// A big jump inside the document (⌘↑ ⌘↓, a search hit): `before` was where the caret
    /// was. When it moved to another note, `before` stays an entry and here is a new one.
    pub fn history_jumped(&mut self, before: Place) {
        let now = self.place();
        if before.caret.as_ref().map(|c| &c.0) == now.caret.as_ref().map(|c| &c.0) {
            return;
        }
        let h = &mut self.history;
        match h.entries.get_mut(h.pos) {
            Some(cur) if cur.same(&before) => *cur = before,
            _ => h.push(before),
        }
        h.push(now);
        h.tab_run = false;
        self.history.save(&self.vault.paths.cache);
    }

    /// ⌘[ (-1) and ⌘] (+1). A place that's gone is skipped, said once in the bar.
    pub fn history_go(&mut self, delta: isize) {
        self.history_tick(false);
        // Back over an agent's change to the sidebar: it's taken back, and nothing else.
        if delta < 0 {
            let pos = self.history.pos;
            if let Some(change) = self.history.entries.get_mut(pos).and_then(|e| e.agent.take()) {
                self.undo_agent_change(change);
            }
        }
        let mut skipped = false;
        loop {
            let target = self.history.pos as isize + delta;
            if target < 0 || target as usize >= self.history.entries.len() {
                self.info(if delta < 0 { "nothing back" } else { "nothing forward" }.to_string());
                return;
            }
            let t = target as usize;
            let p = self.history.entries[t].clone();
            if !self.place_exists(&p) {
                self.history.entries.remove(t);
                if t < self.history.pos {
                    self.history.pos -= 1;
                }
                skipped = true;
                continue;
            }
            self.history.pos = t;
            self.history.tab_run = false;
            self.history.save(&self.vault.paths.cache);
            self.restore_place(p);
            if skipped {
                self.info("skipped a deleted page".to_string());
            }
            return;
        }
    }

    /// The history list's Enter / click: go to entry `i`, keeping every entry (§7.5).
    pub fn history_jump_to(&mut self, i: usize) {
        self.history_tick(false);
        let Some(p) = self.history.entries.get(i).cloned() else { return };
        if !self.place_exists(&p) {
            self.history.entries.remove(i);
            self.history.pos = self.history.pos.min(self.history.entries.len().saturating_sub(1));
            self.info("skipped a deleted page".to_string());
            return;
        }
        self.history.pos = i;
        self.history.save(&self.vault.paths.cache);
        self.restore_place(p);
    }

    fn place_exists(&self, p: &Place) -> bool {
        if !p.vault.join(thc_core::vault::VAULT_MARKER).exists() {
            return false;
        }
        match &p.page {
            Some(id) if p.vault == self.real_vault() => self.vault.store.node(id).ok().flatten().is_some_and(|n| !n.deleted),
            _ => true,
        }
    }

    /// Go there: vault, view, document, caret, scroll (§7.2). Another vault reopens the TUI on
    /// it first, and the place is restored when it has.
    pub fn restore_place(&mut self, p: Place) {
        if p.vault != self.real_vault() {
            self.hist_pending = Some(p.clone());
            self.switch_to = Some(p.vault);
            return;
        }
        self.overlay = None;
        if let Some(d) = p.day.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()) {
            self.journal_date = d;
        }
        let view = VIEWS.iter().copied().find(|v| format!("{v:?}") == p.view).unwrap_or(View::Today);
        self.save_doc(true);
        self.doc_origin = None;
        self.page_open = p.page.clone();
        self.set_view(view);
        if let Some(id) = &p.selected {
            self.selected = Some(id.clone());
            self.restore_cursor_public();
        }
        if let (Some((id, byte)), Some(d)) = (&p.caret, self.doc.as_mut()) {
            if d.set_caret_anchor(&crate::editor::Anchor { id: id.clone(), byte: *byte }) {
                d.clear_selection();
                d.set_scroll(p.scroll, false);
            }
        } else if self.doc.is_none() {
            self.scroll = p.scroll;
        }
        // The entry is where we are now: the next tick only updates it.
        let here = self.place();
        let pos = self.history.pos;
        if let Some(e) = self.history.entries.get_mut(pos) {
            if e.same(&here) {
                *e = Place { label: here.label, ..p };
            }
        }
    }
}
