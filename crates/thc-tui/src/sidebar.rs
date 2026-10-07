//! The sidebar (docs/design/sidebar.md): a stack of panels beside the main view, each a page,
//! a journal day or a list, each live. A page or day panel is an editor: a caretline `View` on
//! the same `Doc` as every other view of that page (one undo history, one save).
//!
//! This file is the model: the stack as presentation state (it lives in `UiState::sidebar`,
//! serializable and patchable), its rules (dedupe, move to top, the pinned group, the limit,
//! closing and reopening) as pure functions, and the owner's open choices in one place
//! ([`policy`]). The runtime half (which `Doc` holds which panel's view, saving, keys) is in
//! `sidebar_app.rs`; drawing is in `sidebar_ui.rs`.

use serde::{Deserialize, Serialize};

/// The owner's open questions (sidebar.md §17), answered by default here and nowhere else.
pub mod policy {
    /// The most panels the stack holds (§2 rule 4, owner Q6).
    pub const MAX_PANELS: usize = 8;
    /// Closed panels kept for ⌥⇧T (§3.4).
    pub const MAX_CLOSED: usize = 8;
    /// When the stack doesn't fit, the panels other than the active one show this many body
    /// rows (§6.3, owner Q6).
    pub const UNFOCUSED_ROWS: u16 = 6;
    /// The active panel never shrinks below this many body rows (§6.3).
    pub const MIN_ACTIVE_ROWS: u16 = 3;
    /// A plain click on a link inside a panel follows it in the main view, as Logseq does
    /// (§12, owner Q2). False: it moves that panel to the page.
    pub const PANEL_CLICK_FOLLOWS_IN_MAIN: bool = true;
    /// Agents never move keyboard focus (§10.4, owner Q3).
    pub const AGENTS_MOVE_FOCUS: bool = false;
    /// One stack per vault (§11, owner Q5): the stack is kept in each vault's cache.
    pub const STACK_PER_VAULT: bool = true;
    /// The detail pane and the Journal calendar give way to the sidebar column (§6.2, owner
    /// Q1); `⌥\` (hide) brings them back.
    pub const DETAIL_YIELDS: bool = true;
    /// The column from this screen width (§6.1); the drawer below it, replace below
    /// [`DRAWER_AT`].
    pub const COLUMN_AT: u16 = 120;
    pub const DRAWER_AT: u16 = 90;
    /// The drawer's width (§6.4).
    pub const DRAWER_W: u16 = 60;
    /// The column's automatic width: a third of the screen, within these (§6.1, owner Q6).
    pub const AUTO_MIN_W: u16 = 40;
    pub const AUTO_MAX_W: u16 = 64;
    /// A set width (⌥= ⌥-, a divider drag) stays within these, leaving the main view at
    /// least [`MAIN_MIN_W`].
    pub const SET_MIN_W: u16 = 32;
    pub const MAIN_MIN_W: u16 = 80;
    /// Columns ⌥= and ⌥- step by.
    pub const WIDTH_STEP: u16 = 4;
}

/// What a panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelKind {
    /// A page, by `id`.
    Page,
    /// A journal day, by `date` (ISO, or `today`: it turns over at midnight).
    Day,
    /// A list view by name: `today`, `inbox`, `tasks`, `log` or `@saved`.
    View,
    /// A list of what a query finds (a tag panel is the query `#health`).
    Query,
}

impl PanelKind {
    /// A page or a day: the body is an editor.
    pub fn is_doc(self) -> bool {
        matches!(self, PanelKind::Page | PanelKind::Day)
    }
}

/// A panel's identity: its kind, vault and target. Unique in the stack (§10.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelKey {
    pub kind: PanelKind,
    pub vault: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

impl PanelKey {
    pub fn page(vault: &str, id: &str) -> PanelKey {
        PanelKey { kind: PanelKind::Page, vault: vault.into(), id: Some(id.into()), date: None, view: None, query: None }
    }

    /// A day: `2026-10-07`, or `today` (it turns over at midnight).
    pub fn day(vault: &str, date: &str) -> PanelKey {
        PanelKey { kind: PanelKind::Day, vault: vault.into(), id: None, date: Some(date.into()), view: None, query: None }
    }

    pub fn view(vault: &str, name: &str) -> PanelKey {
        PanelKey { kind: PanelKind::View, vault: vault.into(), id: None, date: None, view: Some(name.into()), query: None }
    }

    pub fn query(vault: &str, q: &str) -> PanelKey {
        PanelKey { kind: PanelKind::Query, vault: vault.into(), id: None, date: None, view: None, query: Some(q.into()) }
    }

    /// The day this panel shows, `today` resolved against `today`.
    pub fn day_date(&self, today: chrono::NaiveDate) -> Option<chrono::NaiveDate> {
        match self.date.as_deref()? {
            "today" => Some(today),
            d => chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok(),
        }
    }

    /// The document key (`page:<id>`, `day:<date>`), as the caret memory names documents:
    /// a doc panel and the main view on the same document share it.
    pub fn doc_key(&self, today: chrono::NaiveDate) -> Option<String> {
        match self.kind {
            PanelKind::Page => Some(format!("page:{}", self.id.as_deref()?)),
            PanelKind::Day => Some(format!("day:{}", self.day_date(today)?.format("%Y-%m-%d"))),
            _ => None,
        }
    }
}

/// A doc panel's caret (by note id and byte, so it survives edits elsewhere).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Caret {
    pub node: String,
    pub byte: usize,
}

/// Where a view is scrolled: the first row on screen.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScrollAnchor {
    pub anchor: Option<String>,
    pub row: usize,
}

/// A doc panel's view, as UiState keeps it (the same adapter as `document`): caret, scroll,
/// folds. The engine's own view is runtime state rebuilt from this.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DocView {
    pub caret: Option<Caret>,
    pub scroll: ScrollAnchor,
    pub folds: Vec<String>,
    /// Scrolled away from the caret (the wheel, a scrollbar): the view stays where it was put
    /// instead of following the caret back into sight (zszv1), as `doc_scroll_free` does for
    /// the main view.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub scroll_free: bool,
}

/// A list panel's state (§8.2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ListState {
    pub selected: Option<String>,
    pub scroll: ListScroll,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ListScroll {
    pub anchor: Option<String>,
    pub offset: usize,
}

/// `view` names a list view (`"today"`) on a view panel, and holds the doc adapter on a page
/// or day panel (sidebar.md §10.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ViewField {
    Name(String),
    Doc(DocView),
}

/// One panel of the stack.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Panel {
    pub kind: PanelKind,
    pub vault: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewField>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub folded: bool,
    /// Set by the TUI when an agent opened it; cleared once the person interacts with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<ListState>,
}

impl Panel {
    pub fn new(key: PanelKey) -> Panel {
        let view = match key.kind {
            PanelKind::View => key.view.clone().map(ViewField::Name),
            _ => None,
        };
        Panel { kind: key.kind, vault: key.vault, id: key.id, date: key.date, query: key.query, view, pinned: false, folded: false, opened_by: None, list: None }
    }

    pub fn key(&self) -> PanelKey {
        let view = match (&self.view, self.kind) {
            (Some(ViewField::Name(n)), PanelKind::View) => Some(n.clone()),
            _ => None,
        };
        PanelKey { kind: self.kind, vault: self.vault.clone(), id: self.id.clone(), date: self.date.clone(), view, query: self.query.clone() }
    }

    /// The doc adapter (page and day panels).
    pub fn doc_view(&self) -> Option<&DocView> {
        match &self.view {
            Some(ViewField::Doc(d)) => Some(d),
            _ => None,
        }
    }

    pub fn set_doc_view(&mut self, v: DocView) {
        if self.kind.is_doc() {
            self.view = Some(ViewField::Doc(v));
        }
    }
}

/// The sidebar's presentation state (sidebar.md §10.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SidebarState {
    /// The column is shown (≥ 120 columns), or the drawer or replace is open (narrower).
    /// False: hidden by ⌥\, Esc from the drawer, or Focus.
    pub shown: bool,
    /// A set width in columns; None is automatic (a third of the screen, 40 to 64).
    pub width: Option<u16>,
    /// The active panel: the one with keyboard focus, or the one that had it last.
    pub focused: Option<PanelKey>,
    /// The stack, top to bottom: pinned panels first.
    pub open: Vec<Panel>,
    /// Closed panels for ⌥⇧T, newest first.
    pub closed: Vec<Panel>,
    /// The image item's fold (§7.3).
    pub image_folded: bool,
    /// A panel's header flashing (already open and pinned, §2 rule 2): since when (logical ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flash: Option<(PanelKey, u64)>,
}

impl Default for SidebarState {
    fn default() -> Self {
        SidebarState { shown: true, width: None, focused: None, open: Vec::new(), closed: Vec::new(), image_folded: false, flash: None }
    }
}

/// What an agent did to the stack (§10.4), so ⌘[ can take back exactly that: close what it
/// opened, bring back what it closed, unfold what it folded.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentChange {
    pub actor: String,
    pub opened: Vec<PanelKey>,
    pub closed: Vec<Panel>,
    pub folded: Vec<PanelKey>,
}

impl AgentChange {
    /// The change from stack `a` to stack `b`.
    pub fn between(actor: &str, a: &SidebarState, b: &SidebarState) -> AgentChange {
        let keys_a: Vec<PanelKey> = a.open.iter().map(Panel::key).collect();
        let keys_b: Vec<PanelKey> = b.open.iter().map(Panel::key).collect();
        AgentChange {
            actor: actor.to_string(),
            opened: keys_b.iter().filter(|k| !keys_a.contains(k)).cloned().collect(),
            closed: a.open.iter().filter(|p| !keys_b.contains(&p.key())).cloned().collect(),
            folded: b.open.iter().filter(|p| p.folded && a.get(&p.key()).is_some_and(|q| !q.folded)).map(Panel::key).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.opened.is_empty() && self.closed.is_empty() && self.folded.is_empty()
    }
}

/// What opening a panel did (§2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    /// A new panel on top of its group; `evicted` was closed to make room.
    New { evicted: Option<Panel> },
    /// It was open: moved to the top of its group (or, pinned, stayed and flashed).
    Moved,
    /// Eight panels pinned: nothing opened.
    Full,
}

impl SidebarState {
    /// Whether there's anything to show.
    pub fn has_panels(&self) -> bool {
        !self.open.is_empty()
    }

    pub fn position(&self, key: &PanelKey) -> Option<usize> {
        self.open.iter().position(|p| p.key() == *key)
    }

    pub fn get(&self, key: &PanelKey) -> Option<&Panel> {
        self.open.iter().find(|p| p.key() == *key)
    }

    pub fn get_mut(&mut self, key: &PanelKey) -> Option<&mut Panel> {
        self.open.iter_mut().find(|p| p.key() == *key)
    }

    /// The active panel: `focused`, or the top one.
    pub fn active(&self) -> Option<&Panel> {
        self.focused.as_ref().and_then(|k| self.get(k)).or(self.open.first())
    }

    pub fn active_key(&self) -> Option<PanelKey> {
        self.active().map(Panel::key)
    }

    /// The newest panel (its title is bold): the top of the unpinned group.
    pub fn newest(&self) -> Option<PanelKey> {
        self.open.iter().find(|p| !p.pinned).map(Panel::key)
    }

    fn pinned_count(&self) -> usize {
        self.open.iter().take_while(|p| p.pinned).count()
    }

    /// Open `p` (§2): on top of its group and expanded, or, already open, moved to the top of
    /// its group (pinned: it stays and flashes) keeping its view. Over the limit, the
    /// bottom-most unpinned panel closes. It becomes the active panel.
    pub fn open(&mut self, p: Panel, now_ms: u64) -> Opened {
        let key = p.key();
        if let Some(i) = self.position(&key) {
            let mut cur = self.open.remove(i);
            cur.folded = false;
            if cur.opened_by.is_some() && p.opened_by.is_none() {
                cur.opened_by = None;
            }
            if cur.pinned {
                self.open.insert(i, cur);
                self.flash = Some((key.clone(), now_ms));
            } else {
                let at = self.pinned_count();
                self.open.insert(at, cur);
            }
            self.focused = Some(key);
            return Opened::Moved;
        }
        let mut evicted = None;
        if self.open.len() >= policy::MAX_PANELS {
            match self.open.iter().rposition(|q| !q.pinned) {
                Some(i) => {
                    let gone = self.open.remove(i);
                    self.remember_closed(gone.clone());
                    evicted = Some(gone);
                }
                None => return Opened::Full,
            }
        }
        let mut p = p;
        p.folded = false;
        let at = self.pinned_count();
        self.open.insert(at, p);
        self.focused = Some(key);
        Opened::New { evicted }
    }

    fn remember_closed(&mut self, p: Panel) {
        let k = p.key();
        self.closed.retain(|c| c.key() != k);
        self.closed.insert(0, p);
        self.closed.truncate(policy::MAX_CLOSED);
    }

    /// Close panel `key`: it goes to the closed list (for ⌥⇧T, with its view). The active
    /// panel moves to the one below it (or above, at the bottom).
    pub fn close(&mut self, key: &PanelKey) -> Option<Panel> {
        let i = self.position(key)?;
        let p = self.open.remove(i);
        self.remember_closed(p.clone());
        if self.focused.as_ref() == Some(key) {
            self.focused = self.open.get(i).or_else(|| i.checked_sub(1).and_then(|j| self.open.get(j))).map(Panel::key);
        }
        if self.flash.as_ref().is_some_and(|(k, _)| k == key) {
            self.flash = None;
        }
        Some(p)
    }

    /// Close every unpinned panel. Their keys.
    pub fn close_unpinned(&mut self) -> Vec<PanelKey> {
        let gone: Vec<PanelKey> = self.open.iter().filter(|p| !p.pinned).map(Panel::key).collect();
        for k in gone.iter().rev() {
            self.close(k);
        }
        gone
    }

    /// The last closed panel back (with its view), as an open (§3.4).
    pub fn reopen(&mut self, now_ms: u64) -> Option<(PanelKey, Opened)> {
        let p = self.closed.first()?.clone();
        let key = p.key();
        let opened = self.open(p, now_ms);
        if opened != Opened::Full {
            self.closed.remove(0);
        }
        Some((key, opened))
    }

    /// Pin or unpin (§3.2): pinned, it goes to the bottom of the pinned group; unpinned, to
    /// the top of the unpinned group. The new state.
    pub fn toggle_pin(&mut self, key: &PanelKey) -> Option<bool> {
        let i = self.position(key)?;
        let mut p = self.open.remove(i);
        p.pinned = !p.pinned;
        let at = self.pinned_count();
        let pinned = p.pinned;
        self.open.insert(at, p);
        Some(pinned)
    }

    /// Fold or unfold. The new state.
    pub fn toggle_fold(&mut self, key: &PanelKey) -> Option<bool> {
        let p = self.get_mut(key)?;
        p.folded = !p.folded;
        Some(p.folded)
    }

    /// Move panel `key` one place up (`-1`) or down (`1`) within its group (§3.5). False:
    /// it's at the edge of its group.
    pub fn move_within(&mut self, key: &PanelKey, delta: isize) -> bool {
        let Some(i) = self.position(key) else { return false };
        let j = i as isize + delta;
        if j < 0 || j as usize >= self.open.len() || self.open[j as usize].pinned != self.open[i].pinned {
            return false;
        }
        self.open.swap(i, j as usize);
        true
    }

    /// Move panel `key` to `at` (a header drag, §3.5): dropped among the pinned panels it pins,
    /// below them it unpins.
    pub fn move_to(&mut self, key: &PanelKey, at: usize) -> bool {
        let Some(i) = self.position(key) else { return false };
        let mut p = self.open.remove(i);
        let at = at.min(self.open.len());
        let pinned_n = self.pinned_count();
        // Into the pinned group pins; below it unpins; right at its edge keeps the state.
        if at < pinned_n {
            p.pinned = true;
        } else if at > pinned_n {
            p.pinned = false;
        }
        self.open.insert(at, p);
        // Keep the pinned group first.
        self.open.sort_by_key(|q| !q.pinned);
        true
    }

    /// The next (`1`) or previous (`-1`) expanded panel after the active one, wrapping.
    pub fn step_focus(&mut self, delta: isize) -> Option<PanelKey> {
        let n = self.open.len();
        if n == 0 {
            return None;
        }
        let cur = self.active_key().and_then(|k| self.position(&k)).unwrap_or(0);
        for k in 1..=n {
            let j = (cur as isize + delta * k as isize).rem_euclid(n as isize) as usize;
            if !self.open[j].folded || k == n {
                let key = self.open[j].key();
                self.focused = Some(key.clone());
                return Some(key);
            }
        }
        None
    }

    /// The checks types alone can't make (§10.1): unique keys, the limit, pinned first,
    /// `focused` in the stack, a target for each kind.
    pub fn validate(&self) -> Result<(), String> {
        if self.open.len() > policy::MAX_PANELS {
            return Err(format!("sidebar.open: at most {} panels", policy::MAX_PANELS));
        }
        if self.closed.len() > policy::MAX_CLOSED {
            return Err(format!("sidebar.closed: at most {} panels", policy::MAX_CLOSED));
        }
        let mut seen = std::collections::HashSet::new();
        let mut unpinned = false;
        for (i, p) in self.open.iter().enumerate() {
            let at = format!("sidebar.open[{i}]");
            let ok = match p.kind {
                PanelKind::Page => p.id.as_deref().is_some_and(|s| !s.is_empty()),
                PanelKind::Day => p.date.as_deref().is_some_and(|d| d == "today" || chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok()),
                PanelKind::View => matches!(&p.view, Some(ViewField::Name(n)) if !n.is_empty()),
                PanelKind::Query => p.query.as_deref().is_some_and(|q| !q.trim().is_empty()),
            };
            if !ok {
                let want = match p.kind {
                    PanelKind::Page => "a page needs `id`",
                    PanelKind::Day => "a day needs `date` (YYYY-MM-DD or today)",
                    PanelKind::View => "a view needs `view` (today, inbox, tasks, log or @name)",
                    PanelKind::Query => "a query needs `query`",
                };
                return Err(format!("{at}: {want}"));
            }
            if p.kind.is_doc() && matches!(p.view, Some(ViewField::Name(_))) {
                return Err(format!("{at}.view: a page or day panel's view is {{caret, scroll, folds}}"));
            }
            if p.vault.is_empty() {
                return Err(format!("{at}.vault: name the vault"));
            }
            if !seen.insert(p.key()) {
                return Err(format!("{at}: already in the stack (a panel opens once)"));
            }
            if p.pinned && unpinned {
                return Err(format!("{at}: pinned panels come first"));
            }
            unpinned |= !p.pinned;
        }
        if let Some(f) = &self.focused {
            if !seen.contains(f) {
                return Err("sidebar.focused: not a panel in sidebar.open".into());
            }
        }
        if self.width.is_some_and(|w| w < policy::SET_MIN_W) {
            return Err(format!("sidebar.width: at least {}", policy::SET_MIN_W));
        }
        Ok(())
    }
}

/// How the sidebar is presented at a screen width (§6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// A column `width` wide, beside the main area.
    Column { width: u16 },
    /// An overlay on the right, `width` wide (90–119 columns).
    Drawer { width: u16 },
    /// The whole body (under 90 columns).
    Replace,
}

/// The layout at screen width `w`: the set width (clamped at each resize) or a third.
pub fn layout_at(state: &SidebarState, w: u16) -> Layout {
    if w >= policy::COLUMN_AT {
        Layout::Column { width: column_width(state.width, w) }
    } else if w >= policy::DRAWER_AT {
        Layout::Drawer { width: policy::DRAWER_W.min(w) }
    } else {
        Layout::Replace
    }
}

/// The column's width at screen width `w` (§6.1): `clamp(40, w/3, 64)`, or a set width within
/// `[32, w − 80]`.
pub fn column_width(set: Option<u16>, w: u16) -> u16 {
    match set {
        Some(s) => s.clamp(policy::SET_MIN_W, w.saturating_sub(policy::MAIN_MIN_W).max(policy::SET_MIN_W)),
        None => (w / 3).clamp(policy::AUTO_MIN_W, policy::AUTO_MAX_W),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(id: &str) -> Panel {
        Panel::new(PanelKey::page("v", id))
    }

    fn ids(s: &SidebarState) -> Vec<String> {
        s.open.iter().map(|p| format!("{}{}", p.id.clone().or(p.view.clone().and_then(|v| if let ViewField::Name(n) = v { Some(n) } else { None })).unwrap_or_default(), if p.pinned { "*" } else { "" })).collect()
    }

    #[test]
    fn new_panels_go_on_top_and_reopening_moves_without_duplicating() {
        let mut s = SidebarState::default();
        s.open(page("health"), 0);
        s.open(page("reading"), 0);
        assert_eq!(ids(&s), ["reading", "health"]);
        assert_eq!(s.open(page("health"), 0), Opened::Moved);
        assert_eq!(ids(&s), ["health", "reading"]);
        assert_eq!(s.focused, Some(PanelKey::page("v", "health")));
    }

    #[test]
    fn the_ninth_closes_the_bottom_unpinned_and_eight_pinned_refuse() {
        let mut s = SidebarState::default();
        for i in 0..8 {
            s.open(page(&format!("p{i}")), 0);
        }
        match s.open(page("p8"), 0) {
            Opened::New { evicted: Some(p) } => assert_eq!(p.id.as_deref(), Some("p0")),
            o => panic!("{o:?}"),
        }
        assert_eq!(s.open.len(), 8);
        assert_eq!(s.closed[0].id.as_deref(), Some("p0"));
        for p in s.open.clone() {
            s.toggle_pin(&p.key());
        }
        assert_eq!(s.open(page("p9"), 0), Opened::Full);
        assert_eq!(s.open.len(), 8);
        s.validate().unwrap();
    }

    #[test]
    fn pinning_keeps_the_pinned_group_first_and_close_all_keeps_it() {
        let mut s = SidebarState::default();
        s.open(Panel::new(PanelKey::view("v", "today")), 0);
        s.open(page("a"), 0);
        s.open(page("b"), 0);
        assert_eq!(s.toggle_pin(&PanelKey::view("v", "today")), Some(true));
        assert_eq!(ids(&s), ["today*", "b", "a"]);
        s.open(page("c"), 0);
        assert_eq!(ids(&s), ["today*", "c", "b", "a"], "new panels go under the pinned group");
        s.open(Panel::new(PanelKey::view("v", "today")), 5);
        assert_eq!(ids(&s), ["today*", "c", "b", "a"], "a pinned panel doesn't move");
        assert!(s.flash.is_some());
        s.close_unpinned();
        assert_eq!(ids(&s), ["today*"]);
        s.validate().unwrap();
    }

    #[test]
    fn moves_stay_within_the_group_and_a_drag_across_pins() {
        let mut s = SidebarState::default();
        for id in ["a", "b", "c"] {
            s.open(page(id), 0);
        }
        s.toggle_pin(&PanelKey::page("v", "a"));
        assert_eq!(ids(&s), ["a*", "c", "b"]);
        assert!(!s.move_within(&PanelKey::page("v", "c"), -1), "not across the pin boundary");
        assert!(s.move_within(&PanelKey::page("v", "c"), 1));
        assert_eq!(ids(&s), ["a*", "b", "c"]);
        s.move_to(&PanelKey::page("v", "c"), 0);
        assert_eq!(ids(&s), ["c*", "a*", "b"]);
        s.validate().unwrap();
    }

    #[test]
    fn close_and_reopen_keep_the_view() {
        let mut s = SidebarState::default();
        let mut p = page("a");
        p.set_doc_view(DocView { caret: Some(Caret { node: "n1".into(), byte: 3 }), ..Default::default() });
        s.open(p.clone(), 0);
        s.open(page("b"), 0);
        s.close(&PanelKey::page("v", "a"));
        assert_eq!(s.focused, Some(PanelKey::page("v", "b")));
        let (k, _) = s.reopen(0).unwrap();
        assert_eq!(k, PanelKey::page("v", "a"));
        assert_eq!(s.get(&k).unwrap().doc_view(), p.doc_view());
        assert!(s.closed.is_empty());
    }

    #[test]
    fn the_shape_round_trips_and_validation_names_the_field() {
        let mut s = SidebarState::default();
        s.open(Panel::new(PanelKey::view("personal", "today")), 0);
        s.open(page("jx"), 0);
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["open"][1]["view"], "today");
        assert_eq!(serde_json::from_value::<SidebarState>(j).unwrap(), s);
        s.open[0].pinned = false;
        s.open[1].pinned = true;
        assert!(s.validate().unwrap_err().contains("pinned panels come first"));
        let bad: SidebarState = serde_json::from_value(serde_json::json!({"open": [{"kind": "page", "vault": "v"}]})).unwrap();
        assert!(bad.validate().unwrap_err().contains("sidebar.open[0]"));
    }

    #[test]
    fn the_column_is_a_third_within_40_and_64() {
        assert_eq!(column_width(None, 120), 40);
        assert_eq!(column_width(None, 140), 46);
        assert_eq!(column_width(None, 160), 53);
        assert_eq!(column_width(None, 192), 64);
        assert_eq!(column_width(Some(48), 140), 48);
        assert_eq!(column_width(Some(100), 140), 60, "the main view keeps 80");
        assert_eq!(layout_at(&SidebarState::default(), 100), Layout::Drawer { width: 60 });
        assert_eq!(layout_at(&SidebarState::default(), 80), Layout::Replace);
    }
}
