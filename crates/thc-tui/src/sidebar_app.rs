//! The sidebar's runtime (sidebar.md §5, §7, §11): which document holds each doc panel's view,
//! running keys and clicks through a panel, saving, live refreshes and the stack's file.
//!
//! **One Document, many Views.** A page shown in the main view and in a panel is one `Doc`
//! with two caretline views (`Doc::add_view`); a page shown only in a panel is a `Doc` of its
//! own, kept in that panel's [`PanelRt`]. When the main view arrives at a page a panel holds,
//! it takes that `Doc` and adds its view; when it leaves one a panel still shows, the `Doc`
//! moves into the panel. A key in a panel runs the document editor exactly as in the main
//! view ([`App::with_panel`]): the panel's document is the open one while it runs, its view
//! current, its editing state (the caret's line, the `[[` popup, …) in place. Anything that
//! would navigate is deferred and runs in the main view afterwards ([`Deferred`]).

use crate::app::{App, Focus, View};
use crate::doc_app::caret_key;
use crate::editor::{Anchor, Doc, MAIN_VIEW, Target, ViewId};
use crate::sidebar::{Caret, DocView, Panel, PanelKey, PanelKind, ScrollAnchor};
use crate::update::{Effect, SidebarOp, WidthChange};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a panel's view carries besides the document: its editor pane's state, and the
/// runtime's bookkeeping for it, swapped in while a panel's key runs.
#[derive(Default)]
pub struct DocSlot {
    /// The panel's own document, when the main view isn't on the same one.
    pub doc: Option<Doc>,
    /// Per-view geometry pin; presentation state lives in sidebar `DocView::editor`.
    pub(crate) caret_pin: Option<crate::doc_ui::CaretPin>,
    /// The panel's remembered scroll, waiting for its first layout (`App::doc_pending_scroll`).
    pub(crate) pending_scroll: Option<(usize, bool)>,
    pub(crate) footer: Option<(String, Vec<crate::doc_app::FooterRow>)>,
    pub(crate) first_ever: bool,
}

/// A doc panel at runtime.
pub struct PanelRt {
    /// Its view's id in its document.
    pub vid: ViewId,
    /// Pages linking to it from elsewhere (`▸ linked from N`), counted on read and refresh.
    pub linked: usize,
    pub slot: DocSlot,
    /// Why it can't show its document (deleted, unreadable).
    pub problem: Option<String>,
}

/// Why a target can't open beside (exit 3, 5 and 6 for `thc ui aside`).
#[derive(Clone, Debug, PartialEq)]
pub enum AsideError {
    NotFound(String),
    Ambiguous(String, Vec<String>),
    Invalid(String),
}

impl AsideError {
    pub fn message(&self) -> String {
        match self {
            AsideError::NotFound(m) | AsideError::Invalid(m) => m.clone(),
            AsideError::Ambiguous(m, c) => format!("{m} · {}", c.join(", ")),
        }
    }
}

/// `:aside <target>` and `:sidebar close | close all | pin | hide | show | width <n|auto>`.
pub fn palette(app: &mut App, buf: &str) -> bool {
    if let Some(t) = buf.strip_prefix("aside") {
        if !(t.is_empty() || t.starts_with(' ')) {
            return false;
        }
        match app.resolve_aside(t) {
            Ok(k) => app.open_aside(k, true),
            Err(e) => app.error(e.message()),
        }
        return true;
    }
    let Some(rest) = buf.strip_prefix("sidebar") else { return false };
    let screen = app.render.size.width.max(app.screen_width);
    match rest.trim() {
        "close" => run(app, "sidebar.close"),
        "close all" => run(app, "sidebar.close_all"),
        "pin" => run(app, "sidebar.pin"),
        "hide" => {
            if app.ui.sidebar.shown {
                run(app, "sidebar.toggle");
            }
            true
        }
        "show" | "" => {
            if !app.ui.sidebar.shown {
                run(app, "sidebar.toggle");
            }
            true
        }
        w if w.starts_with("width") => {
            let v = w["width".len()..].trim();
            let change = if v == "auto" { Some(WidthChange::Auto) } else { v.parse::<u16>().ok().map(WidthChange::Set) };
            match change {
                Some(change) => crate::runtime_effects::sidebar_op(app, SidebarOp::Width { change, screen }),
                None => app.error("sidebar width <columns|auto>"),
            }
            true
        }
        other => {
            app.error(format!("sidebar {other}? · close, close all, pin, hide, show, width <n|auto>"));
            true
        }
    };
    true
}

/// A drag in the sidebar (§3.5, §6.1).
#[derive(Clone, Debug, PartialEq)]
pub enum Drag {
    /// The divider: the column follows the pointer.
    Divider,
    /// A header pressed at row `from`; `to` is where it would drop (a stack index), once moved.
    Header { key: PanelKey, from: u16, to: Option<usize> },
}

/// Compatibility name for host effects from the shared editor component.
pub use crate::editor_pane::PaneEffect as Deferred;

impl App {
    /// The stack's panel keys, top to bottom.
    pub fn panel_keys(&self) -> Vec<PanelKey> {
        self.ui.sidebar.open.iter().map(Panel::key).collect()
    }

    /// The document a doc panel shows, for reading (the main view's when it's the same page).
    pub fn panel_doc(&self, key: &PanelKey) -> Option<&Doc> {
        let rt = self.panels.get(key)?;
        match &rt.slot.doc {
            Some(d) => Some(d),
            None => self.doc.as_ref().filter(|d| d.has_view(rt.vid)),
        }
    }

    /// The document a doc panel shows with its view current (put back with
    /// `use_view(MAIN_VIEW)` when it's the main view's document).
    pub fn panel_doc_mut(&mut self, key: &PanelKey) -> Option<(&mut Doc, ViewId)> {
        let rt = self.panels.get_mut(key)?;
        let vid = rt.vid;
        let d = match rt.slot.doc.as_mut() {
            Some(d) => d,
            None => self.doc.as_mut().filter(|d| d.has_view(vid))?,
        };
        d.use_view(vid);
        Some((d, vid))
    }

    /// Back to the main view in the main document (after `panel_doc_mut`).
    pub fn main_view_current(&mut self) {
        if let Some(d) = self.doc.as_mut() {
            d.use_view(MAIN_VIEW);
        }
    }

    fn swap_slot(&mut self, slot: &mut DocSlot, with_doc: bool) {
        if with_doc {
            std::mem::swap(&mut self.doc, &mut slot.doc);
        }
        std::mem::swap(&mut self.caret_pin, &mut slot.caret_pin);
        std::mem::swap(&mut self.doc_pending_scroll, &mut slot.pending_scroll);
        std::mem::swap(&mut self.doc_footer, &mut slot.footer);
        std::mem::swap(&mut self.doc_first_ever, &mut slot.first_ever);
    }

    /// Run `f` as if panel `key`'s view were the open document (see the module docs). None:
    /// not a loaded doc panel, or already inside one.
    pub fn with_panel<R>(&mut self, key: &PanelKey, f: impl FnOnce(&mut App) -> R) -> Option<R> {
        if self.in_panel.is_some() || !self.panels.contains_key(key) {
            return None;
        }
        let mut rt = self.panels.remove(key)?;
        let own = rt.slot.doc.is_some();
        if !own {
            if !self.doc.as_ref().is_some_and(|d| d.has_view(rt.vid)) {
                self.panels.insert(key.clone(), rt);
                return None;
            }
            self.doc.as_mut().unwrap().use_view(rt.vid);
        }
        let mut editor = self.panel_editor(key).cloned().unwrap_or_else(panel_editor_default);
        std::mem::swap(&mut self.ui.main, &mut editor);
        self.swap_slot(&mut rt.slot, own);
        self.in_panel = Some(key.clone());
        let selected = self.selected.clone();
        let log_sizes = self.log_sizes.clone();
        let saved_view = self.render.doc_view;
        let saved_hits = std::mem::take(&mut self.render.doc_hits);
        let saved_bar = self.render.doc_scrollbar;
        self.render.doc_scrollbar = self.render.panel_scrollbars.iter().find(|(k, _)| k == key).map(|(_, b)| *b);
        self.render.doc_view = self.render.panel_views.iter().find(|(k, _)| k == key).map(|(_, r)| (r.x, r.y, r.width, r.height));
        self.render.doc_hits = self.render.panel_hits.iter().find(|(k, _)| k == key).map(|(_, h)| h.clone()).unwrap_or_default();
        self.clock_tick();
        let r = f(self);
        self.render.doc_view = saved_view;
        self.render.doc_hits = saved_hits;
        self.render.doc_scrollbar = saved_bar;
        self.in_panel = None;
        self.selected = selected;
        std::mem::swap(&mut self.ui.main, &mut editor);
        if let Some(p) = self.ui.sidebar.get_mut(key) {
            let mut view = p.doc_view().cloned().unwrap_or_default();
            view.editor = Some(editor);
            p.set_doc_view(view);
        }
        // The panel's document may have been read again meanwhile: it's whatever is open now.
        self.swap_slot(&mut rt.slot, own);
        if !own {
            if let Some(d) = self.doc.as_mut() {
                d.use_view(MAIN_VIEW);
            }
        }
        self.panels.insert(key.clone(), rt);
        if log_sizes != self.log_sizes {
            // A commit in another document also changes the main's also-today/backlink rows.
            self.load_footer();
            self.build_rail();
        }
        Some(r)
    }

    /// A page or day for a panel, read from the vault: at its top, parked (§9).
    fn read_panel_doc(&self, key: &PanelKey) -> Result<Doc, String> {
        let s = &self.vault.store;
        let (target, root) = match key.kind {
            PanelKind::Page => {
                let id = key.id.clone().unwrap_or_default();
                match s.node(&id).ok().flatten() {
                    Some(n) if n.deleted => return Err(format!("¶ {} was deleted", s.render_text(&n.label()))),
                    Some(n) => (Target::Page { id: id.clone(), title: s.render_text(&n.label()) }, Some(id)),
                    None => return Err("no such page".into()),
                }
            }
            PanelKind::Day => {
                let date = key.day_date(self.today).ok_or("not a day")?;
                (Target::Journal { date }, s.journal_node(&date.format("%Y-%m-%d").to_string()).ok().flatten())
            }
            _ => return Err("not a document".into()),
        };
        let blocks = root.as_ref().and_then(|r| thc_core::outline::render_for_editor(s, r).ok()).unwrap_or_default();
        let mut d = Doc::new(target, root, &blocks, self.today);
        d.tick(self.ui.now_ms);
        d.caret_to_start();
        Ok(d)
    }

    /// A panel's runtime, made when it joins the stack: its view on the main view's document
    /// when that's the same page, else on a document of its own. Its remembered view comes back.
    pub(crate) fn load_panel(&mut self, key: &PanelKey) {
        if !key.kind.is_doc() || self.panels.contains_key(key) {
            return;
        }
        let Some(dk) = key.doc_key(self.today) else { return };
        let vid = self.next_vid;
        self.next_vid += 1;
        let remembered = self.ui.sidebar.get(key).and_then(|p| p.doc_view().cloned());
        // The remembered scroll waits for the panel's first layout (zszv1): its row counts rows
        // as the panel's width wraps them.
        let pending_scroll = remembered.as_ref().filter(|v| v.caret.is_some()).map(|v| (v.scroll.row, v.scroll_free));
        let mut rt = PanelRt { vid, linked: 0, slot: DocSlot { pending_scroll, ..Default::default() }, problem: None };
        if self.doc.as_ref().is_some_and(|d| caret_key(&d.target) == dk) {
            let d = self.doc.as_mut().unwrap();
            d.add_view(vid);
            if let Some(v) = &remembered {
                d.with_view(vid, |d| apply_doc_view(d, v));
            }
        } else {
            match self.read_panel_doc(key) {
                Ok(mut d) => {
                    d.rename_view(vid);
                    if let Some(v) = &remembered {
                        apply_doc_view(&mut d, v);
                    }
                    rt.slot.doc = Some(d);
                }
                Err(why) => rt.problem = Some(why),
            }
        }
        self.panels.insert(key.clone(), rt);
        if remembered.as_ref().and_then(|v| v.editor.as_ref()).is_none() {
            let mut editor = panel_editor_default();
            editor.line_id = self.panel_doc_mut(key).map(|(d, _)| d.caret_block().id.clone());
            self.main_view_current();
            if let Some(p) = self.ui.sidebar.get_mut(key) {
                let mut v = p.doc_view().cloned().unwrap_or_default();
                v.editor = Some(editor);
                p.set_doc_view(v);
            }
        }
        self.count_backlinks(key);
        self.sync_panel_view(key);
    }

    /// `▸ linked from N`: notes elsewhere that mention this page.
    fn count_backlinks(&mut self, key: &PanelKey) {
        let (PanelKind::Page, Some(id)) = (key.kind, key.id.clone()) else { return };
        let here: std::collections::HashSet<String> = self.panel_doc(key).map(|d| d.blocks().iter().map(|l| l.id.clone()).collect()).unwrap_or_default();
        let n = self
            .vault
            .store
            .nodes_where("n.deleted=0 AND n.id IN (SELECT src FROM edges WHERE rel='mention' AND dst=?1) ORDER BY n.updated_ms DESC LIMIT 50", &[&id])
            .unwrap_or_default()
            .iter()
            .filter(|n| !here.contains(&n.id))
            .count();
        if let Some(rt) = self.panels.get_mut(key) {
            rt.linked = n;
        }
    }

    /// A panel left the stack: what's typed in it is saved, then its view goes (and its
    /// document, unless the main view shows it).
    pub(crate) fn drop_panel(&mut self, key: &PanelKey) {
        self.with_panel(key, |a| a.save_doc(true));
        let Some(rt) = self.panels.remove(key) else { return };
        if rt.slot.doc.is_none() {
            if let Some(d) = self.doc.as_mut() {
                d.remove_view(rt.vid);
                d.use_view(MAIN_VIEW);
            }
        }
    }

    /// Every doc panel in the stack has its runtime; a "today" panel follows the date; a panel
    /// whose page was deleted says so. At launch, such a panel is dropped (§11).
    pub(crate) fn ensure_panels(&mut self) {
        let keys = self.panel_keys();
        let launch = !self.sidebar_checked;
        self.sidebar_checked = true;
        let mut gone: Vec<(PanelKey, String)> = Vec::new();
        for k in &keys {
            if !k.kind.is_doc() {
                continue;
            }
            // A day panel on "today" after midnight: the new day.
            if let (Some(rt), Some(dk)) = (self.panels.get(k), k.doc_key(self.today)) {
                let shows = match &rt.slot.doc {
                    Some(d) => caret_key(&d.target) == dk,
                    None => self.doc.as_ref().is_some_and(|d| caret_key(&d.target) == dk && d.has_view(rt.vid)),
                };
                if !shows {
                    self.drop_panel(k);
                }
            }
            if !self.panels.contains_key(k) {
                self.load_panel(k);
            }
            if launch {
                if let Some(why) = self.panels.get(k).and_then(|r| r.problem.clone()) {
                    gone.push((k.clone(), why));
                }
            }
        }
        // Runtimes for panels no longer in the stack (a patched state, a vault switch).
        let stale: Vec<PanelKey> = self.panels.keys().filter(|k| !keys.contains(k)).cloned().collect();
        for k in stale {
            self.drop_panel(&k);
        }
        if let Some((_, why)) = gone.first().cloned() {
            for (k, _) in &gone {
                crate::runtime_effects::sidebar_op(self, SidebarOp::Close { key: k.clone() });
                self.ui.sidebar.closed.retain(|p| p.key() != *k);
            }
            self.info(format!("{why} · removed from the sidebar"));
        }
    }

    /// The main view leaves document `d`: a panel showing the same page keeps it (with its
    /// view current); otherwise it goes.
    pub(crate) fn park_main_doc(&mut self, mut d: Doc) {
        let key = caret_key(&d.target);
        let today = self.today;
        let holder = self.panels.iter().find(|(k, rt)| rt.slot.doc.is_none() && k.doc_key(today).as_deref() == Some(key.as_str()) && d.has_view(rt.vid)).map(|(k, _)| k.clone());
        if let Some(k) = holder {
            let rt = self.panels.get_mut(&k).unwrap();
            d.use_view(rt.vid);
            d.remove_view(MAIN_VIEW);
            rt.slot.doc = Some(d);
        }
    }

    /// The main view arrives at `target`: a panel's document of it, with a main view added.
    pub(crate) fn adopt_panel_doc(&mut self, target: &Target) -> Option<Doc> {
        let key = caret_key(target);
        let today = self.today;
        let k = self.panels.iter().find(|(k, rt)| rt.slot.doc.is_some() && k.doc_key(today).as_deref() == Some(key.as_str())).map(|(k, _)| k.clone())?;
        let rt = self.panels.get_mut(&k)?;
        let mut d = rt.slot.doc.take()?;
        d.add_view(MAIN_VIEW);
        d.use_view(MAIN_VIEW);
        Some(d)
    }

    /// Changes from elsewhere reach every panel's document (the main view's is patched by
    /// `sync_doc`): lines update in place, the caret stays on its text (§4.3).
    pub(crate) fn patch_panels(&mut self) {
        let keys: Vec<PanelKey> = self.panels.iter().filter(|(_, rt)| rt.slot.doc.is_some()).map(|(k, _)| k.clone()).collect();
        for k in self.panels.keys().cloned().collect::<Vec<_>>() {
            self.count_backlinks(&k);
        }
        let typing = (self.ui.focus == Focus::Sidebar).then(|| self.ui.sidebar.active_key()).flatten();
        for k in keys {
            // A panel without the keyboard takes changes at once, its caret mapped through them.
            let hold = typing.as_ref() == Some(&k);
            self.with_panel(&k, |a| {
                if let Some(d) = a.doc.as_mut() {
                    d.hold_caret_line = hold;
                }
                a.patch_doc();
                if let Some(d) = a.doc.as_mut() {
                    d.hold_caret_line = true;
                }
            });
        }
    }

    /// Save what's typed in every panel's own document (`all`: the caret's line too).
    pub(crate) fn save_panels(&mut self, all: bool) {
        let keys: Vec<PanelKey> = self.panels.iter().filter(|(_, rt)| rt.slot.doc.is_some()).map(|(k, _)| k.clone()).collect();
        for k in keys {
            self.with_panel(&k, |a| a.save_doc(all));
        }
    }

    /// Save everything, the main view's document and every panel's (quit, a vault switch, the
    /// terminal losing focus).
    pub fn save_everything(&mut self) {
        self.save_doc(true);
        self.save_panels(true);
        self.sync_panel_views();
    }

    /// The idle point and the deferred line-leave save, for panels' own documents.
    pub(crate) fn panels_idle(&mut self) -> bool {
        let keys: Vec<PanelKey> = self.panels.iter().filter(|(_, rt)| rt.slot.doc.is_some()).map(|(k, _)| k.clone()).collect();
        let mut any = false;
        for k in keys {
            any |= self.with_panel(&k, |a| a.idle_step()).unwrap_or(false);
        }
        any
    }

    pub(crate) fn panels_after_frame(&mut self) -> bool {
        let keys: Vec<PanelKey> = self.panel_keys().into_iter().filter(|k| self.panel_editor(k).is_some_and(|e| e.save_after_frame)).collect();
        let mut any = false;
        for k in keys {
            any |= self.with_panel(&k, |a| a.after_frame()).unwrap_or(false);
        }
        any
    }

    /// A panel's document is waiting on time (unsaved typing, a save in flight).
    pub fn panels_want_clock(&self) -> bool {
        self.panels.values().filter_map(|rt| rt.slot.doc.as_ref()).any(|d| d.blocks().iter().any(|l| l.edited() || l.saving_since.is_some() || l.flash_until.is_some()))
    }

    /// Each doc panel's view (caret, scroll) into its UiState adapter.
    pub(crate) fn sync_panel_views(&mut self) {
        for k in self.panel_keys() {
            self.sync_panel_view(&k);
        }
    }

    fn sync_panel_view(&mut self, key: &PanelKey) {
        // Not laid out yet: the remembered view stands until it is.
        if self.panels.get(key).is_some_and(|rt| rt.slot.pending_scroll.is_some()) {
            return;
        }
        let editor = self.panel_editor(key).cloned();
        let Some((d, _)) = self.panel_doc_mut(key) else { return };
        let a = d.place_anchor().unwrap_or_else(|| d.caret_anchor());
        let v = DocView {
            caret: Some(Caret { node: if d.caret_block().is_new { String::new() } else { a.id }, byte: a.byte }),
            scroll: ScrollAnchor { anchor: None, row: d.scroll() }, folds: Vec::new(), scroll_free: d.scroll_free(),
            editor, fresh_end: d.has_fresh_end(), caret_new: d.new_caret_line(), caret_tail: d.caret_tail(),
        };
        self.main_view_current();
        if let Some(p) = self.ui.sidebar.get_mut(key) {
            p.set_doc_view(v);
        }
    }

    // ---- what a panel shows -----------------------------------------------------------------

    /// A panel's header title (§4.1): the page title, `Wed 07 Oct` (· today), the view, the
    /// query.
    pub fn panel_title(&self, key: &PanelKey) -> String {
        match key.kind {
            PanelKind::Page => {
                let id = key.id.as_deref().unwrap_or("");
                match self.panel_doc(key).map(|d| &d.target) {
                    Some(Target::Page { title, .. }) if !title.is_empty() => title.clone(),
                    _ => self.node_label(id),
                }
            }
            PanelKind::Day => match key.day_date(self.today) {
                Some(d) => d.format("%a %d %b").to_string(),
                None => key.date.clone().unwrap_or_default(),
            },
            PanelKind::View => match key.view.as_deref().unwrap_or("") {
                v if v.starts_with('@') => v.to_string(),
                v => {
                    let mut c = v.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                }
            },
            PanelKind::Query => key.query.clone().unwrap_or_default(),
        }
    }

    /// `¶ Health`, `§ Tue 06 Oct`, `≡ Today`: how the bar names a panel.
    pub fn panel_name(&self, key: &PanelKey) -> String {
        let g = self.theme.glyphs();
        let sym = match key.kind {
            PanelKind::Page => g.page,
            PanelKind::Day => g.journal,
            PanelKind::View => g.query,
            PanelKind::Query if key.query.as_deref().is_some_and(|q| q.starts_with('#')) => "#",
            PanelKind::Query => g.query,
        };
        format!("{sym} {}", self.panel_title(key))
    }

    // ---- opening ----------------------------------------------------------------------------

    /// What ⌥O (or `o`, ⇧Enter) opens beside here (§2): the link the caret is in, or the
    /// selected row's page or day.
    pub fn aside_target(&self) -> Option<PanelKey> {
        let vault = self.ui.vault_name.clone();
        if let Some(d) = self.doc.as_ref() {
            let l = d.caret_block();
            let title = crate::doc_app::link_at(&l.text, d.caret().byte)?;
            let id = self.vault.store.find_root_by_title(&title, false).ok().flatten()?;
            return Some(PanelKey::page(&vault, &id));
        }
        let row = self.rows.get(self.cursor)?;
        if self.row_vault.contains_key(&self.cursor) {
            return None;
        }
        let n = row.node()?;
        let s = &self.vault.store;
        let mut root = n.clone();
        while let Some(p) = root.parent.clone().and_then(|p| s.node(&p).ok().flatten()) {
            root = p;
        }
        if let Some(j) = &root.journal {
            return thc_core::dates::DateVal::from_stored(j).map(|dv| PanelKey::day(&vault, &dv.date().format("%Y-%m-%d").to_string()));
        }
        root.title.is_some().then(|| PanelKey::page(&vault, &root.id))
    }

    /// What `:aside <target>` and `thc ui aside <target>` name (§2, §10.3): a page by title or
    /// id, a day (`today`, `fri`, `2026-10-06`), `@view`, `#tag`, or any query.
    pub fn resolve_aside(&self, target: &str) -> Result<PanelKey, AsideError> {
        let t = target.trim();
        let vault = self.ui.vault_name.clone();
        let s = &self.vault.store;
        if t.is_empty() {
            return Err(AsideError::Invalid("name a page, a day, @view, #tag or a query".into()));
        }
        if t.eq_ignore_ascii_case("today") {
            return Ok(PanelKey::day(&vault, "today"));
        }
        if let Some(name) = t.strip_prefix('@') {
            if matches!(name, "today" | "inbox" | "tasks" | "log") || self.saved_views.iter().any(|v| v.name == name) {
                return Ok(PanelKey::view(&vault, if matches!(name, "today" | "inbox" | "tasks" | "log") { name.to_string() } else { format!("@{name}") }.as_str()));
            }
            return Err(AsideError::Invalid(format!("unknown view @{name} · :views edit")));
        }
        if t.starts_with('#') && !t.contains(' ') {
            return Ok(PanelKey::query(&vault, t));
        }
        if let Ok(Some(id)) = s.find_root_by_title(t, false) {
            return Ok(PanelKey::page(&vault, &id));
        }
        if let Ok(d) = thc_core::dates::parse(t, self.today) {
            return Ok(PanelKey::day(&vault, &d.date().format("%Y-%m-%d").to_string()));
        }
        match s.resolve(t) {
            Ok(id) => {
                return match s.node(&id).ok().flatten() {
                    Some(n) if n.journal.is_some() => Ok(PanelKey::day(&vault, n.journal.as_deref().unwrap_or(""))),
                    Some(n) if n.parent.is_none() && n.title.is_some() => Ok(PanelKey::page(&vault, &id)),
                    Some(_) => Err(AsideError::Invalid(format!("{t} isn't a page or a day"))),
                    None => Err(AsideError::NotFound(format!("no page or day {t:?}"))),
                };
            }
            Err(e) => {
                if let Some(thc_core::error::ThcError::Ambiguous { candidates, .. }) = e.downcast_ref::<thc_core::error::ThcError>() {
                    return Err(AsideError::Ambiguous(format!("{t:?} is ambiguous"), candidates.clone()));
                }
            }
        }
        // A query (anything with a field, a status, a sort).
        if t.contains(':') || t.starts_with('-') {
            return match thc_core::query::compile(t, s, self.today) {
                Ok(_) => Ok(PanelKey::query(&vault, t)),
                Err(e) => Err(AsideError::Invalid(format!("{e:#}"))),
            };
        }
        Err(AsideError::NotFound(format!("no page ¶ {t}")))
    }

    /// Open `key` beside (§2). `focus`: the keyboard goes to it (the finder, the palette).
    pub fn open_aside(&mut self, key: PanelKey, focus: bool) {
        if self.in_panel.is_some() {
            self.panel_defer.push(Deferred::Aside(key));
            return;
        }
        if focus && self.ui.focus != Focus::Sidebar {
            self.save_doc(true);
        }
        crate::runtime_effects::sidebar_op(self, SidebarOp::Open { panel: Panel::new(key.clone()), focus, by: None });
        if !self.sidebar_fits() && !focus {
            // Narrower than the column: the drawer or replace opens, and takes the keyboard.
            if self.ui.sidebar.get(&key).is_some() {
                crate::runtime_effects::sidebar_op(self, SidebarOp::Focus { key: Some(key) });
            }
        }
    }

    /// A link's page, beside (a ⇧-click, ⌥O on a link).
    pub fn open_aside_link(&mut self, title: &str) {
        match self.vault.store.find_root_by_title(title, false).ok().flatten() {
            Some(id) => {
                let key = PanelKey::page(&self.ui.vault_name, &id);
                self.open_aside(key, false);
            }
            None => self.info(format!("no page ¶ {title} yet · it's made when the line saves")),
        }
    }

    /// Whether the sidebar is a column at this width (else the drawer or replace).
    pub fn sidebar_fits(&self) -> bool {
        let w = if self.render.size.width > 0 { self.render.size.width } else { self.screen_width.max(crate::sidebar::policy::COLUMN_AT) };
        w >= crate::sidebar::policy::COLUMN_AT
    }

    // ---- focus --------------------------------------------------------------------------------

    /// The keyboard to panel `key`: the view it leaves commits first (§5.1).
    pub fn focus_panel(&mut self, key: PanelKey) {
        if self.ui.focus == Focus::Sidebar {
            if let Some(cur) = self.ui.sidebar.focused.clone().filter(|c| *c != key) {
                self.commit_panel(&cur);
            }
        } else {
            self.save_doc(true);
        }
        crate::runtime_effects::sidebar_op(self, SidebarOp::Focus { key: Some(key) });
    }

    /// The keyboard back to the main view: the panel commits first.
    pub fn focus_main(&mut self) {
        if self.ui.focus != Focus::Sidebar {
            return;
        }
        if let Some(cur) = self.ui.sidebar.focused.clone() {
            self.commit_panel(&cur);
        }
        crate::runtime_effects::sidebar_op(self, SidebarOp::Focus { key: None });
    }

    /// A panel's commit point: its typing saved (one transaction), its view remembered, parked
    /// again for its next focus.
    pub fn commit_panel(&mut self, key: &PanelKey) {
        self.with_panel(key, |a| {
            if let Some(d) = a.doc.as_mut() {
                d.clear_selection();
            }
            a.save_doc(true);
            a.main.parked = true;
        });
        self.sync_panel_view(key);
        self.persist_sidebar();
    }

    /// Run what a panel's key deferred, in the main view.
    fn run_deferred(&mut self, from: &PanelKey) {
        let todo = std::mem::take(&mut self.panel_defer);
        for d in todo {
            match d {
                Deferred::Aside(k) => self.open_aside(k, false),
                Deferred::Compare(id) => {
                    self.selected = Some(id);
                    self.open_compare();
                }
                Deferred::Leave => self.focus_main(),
                Deferred::OpenIssue { target, line } => {
                    self.focus_main();
                    self.save_doc(true);
                    self.doc_back = matches!(target, Target::Page { .. }).then(|| (target, line.clone(), line.clone()));
                    self.page_open = Some(line);
                    self.set_view(View::Pages);
                }
                Deferred::Follow(title) => {
                    self.focus_main();
                    let _ = from;
                    self.follow_in_main(&title);
                }
                Deferred::Action(a) => {
                    // ⌥S, Esc and the rest of the sidebar's own: as from the sidebar.
                    if !a.starts_with("sidebar.") {
                        self.focus_main();
                    }
                    crate::keymap::run(self, &a);
                }
            }
        }
    }

    /// A link followed in the main view (§12): the page by its title.
    pub fn follow_in_main(&mut self, title: &str) {
        match self.vault.store.find_root_by_title(title, false).ok().flatten() {
            Some(id) => {
                self.save_doc(true);
                self.remember_origin();
                self.page_open = Some(id);
                self.set_view(View::Pages);
            }
            None => self.info(format!("no page ¶ {title} yet · it's made when the line saves")),
        }
    }

    /// ⌥M (§3.4): the main view goes to this panel's page at its caret and scroll, and the
    /// panel closes. The page left is a history step.
    pub fn panel_to_main(&mut self, key: &PanelKey) {
        if !key.kind.is_doc() {
            return self.info("to main is for a page or a day");
        }
        self.commit_panel(key);
        let place = self.ui.sidebar.get(key).and_then(|p| p.doc_view().cloned());
        let doc_key = key.doc_key(self.today);
        crate::runtime_effects::sidebar_op(self, SidebarOp::Close { key: key.clone() });
        crate::runtime_effects::sidebar_op(self, SidebarOp::Focus { key: None });
        self.ui.focus = Focus::List;
        if let (Some(dk), Some(v)) = (doc_key, place) {
            if let Some(c) = v.caret {
                self.carets.insert(dk, (c.node, c.byte, v.scroll.row));
            }
        }
        self.save_doc(true);
        self.remember_origin();
        match key.kind {
            PanelKind::Page => {
                self.page_open = key.id.clone();
                self.set_view(View::Pages);
            }
            _ => {
                if let Some(d) = key.day_date(self.today) {
                    self.journal_date = d;
                    self.selected = None;
                    self.set_view(View::Journal);
                }
            }
        }
        // Arrive where the panel was (the caret memory just set it), even if the document was
        // already open.
        let caret = self.doc.as_ref().map(|d| caret_key(&d.target)).and_then(|k| self.carets.get(&k).cloned());
        if let (Some(d), Some((id, byte, scroll))) = (self.doc.as_mut(), caret) {
            if d.set_caret_anchor(&Anchor { id, byte }) {
                d.set_scroll(scroll, false);
            }
            self.main.line_id = Some(d.caret_block().id.clone());
        }
    }

    // ---- agents (§10) -------------------------------------------------------------------------

    /// `thc ui aside`: an agent puts `target` beside the person (§10.3). The stack's rules run
    /// here (dedupe, move to top, the limit); the keyboard stays where it is; the panel says who
    /// opened it; the bar says so; ⌘[ takes it back. With `close`, it closes a panel it opened.
    pub fn agent_aside(&mut self, target: &str, pin: bool, fold: bool, close: bool, actor: Option<&str>) -> Result<PanelKey, AsideError> {
        let key = match self.resolve_aside(target) {
            Ok(k) => k,
            // `--close` also takes the panel by its id as the stack holds it.
            Err(e) if close => self.panel_keys().into_iter().find(|k| k.id.as_deref() == Some(target) || k.query.as_deref() == Some(target) || k.view.as_deref() == Some(target)).ok_or(e)?,
            Err(e) => return Err(e),
        };
        let before = self.ui.sidebar.clone();
        let who = actor.unwrap_or("").to_string();
        if close {
            let Some(p) = self.ui.sidebar.get(&key).cloned() else { return Err(AsideError::NotFound(format!("{} isn't beside you", self.panel_name(&key)))) };
            if p.pinned && actor.is_some() {
                return Err(AsideError::Invalid("a pinned panel is the person's · an agent can't close it".into()));
            }
            if actor.is_some() && p.opened_by.as_deref() != actor {
                return Err(AsideError::Invalid(format!("{} wasn't opened by {who} · an agent closes only what it opened", self.panel_name(&key))));
            }
            self.commit_panel(&key);
            crate::runtime_effects::sidebar_op(self, SidebarOp::Close { key: key.clone() });
        } else {
            let focus_before = self.ui.focus;
            let active_before = self.ui.sidebar.focused.clone();
            crate::runtime_effects::sidebar_op(self, SidebarOp::Open { panel: Panel::new(key.clone()), focus: false, by: actor.map(str::to_string) });
            if self.ui.sidebar.get(&key).is_none() {
                return Err(AsideError::Invalid(format!("{} panels pinned · the person unpins one first", crate::sidebar::policy::MAX_PANELS)));
            }
            if pin && !self.ui.sidebar.get(&key).is_some_and(|p| p.pinned) {
                crate::runtime_effects::sidebar_op(self, SidebarOp::Pin { key: key.clone() });
                if let Some(p) = self.ui.sidebar.get_mut(&key) {
                    p.opened_by = actor.map(str::to_string);
                }
            }
            if fold && !self.ui.sidebar.get(&key).is_some_and(|p| p.folded) {
                if let Some(p) = self.ui.sidebar.get_mut(&key) {
                    p.folded = true;
                }
            }
            // Agents never move the keyboard (§10.4): the person's active panel stays theirs.
            if actor.is_some() && !crate::sidebar::policy::AGENTS_MOVE_FOCUS {
                self.ui.focus = focus_before;
                if focus_before == Focus::Sidebar {
                    self.ui.sidebar.focused = active_before.filter(|k| self.ui.sidebar.get(k).is_some()).or(self.ui.sidebar.focused.clone());
                }
            }
        }
        if let Some(a) = actor {
            let change = crate::sidebar::AgentChange::between(a, &before, &self.ui.sidebar);
            if !change.is_empty() {
                self.history_agent_step(change);
            }
            self.agent_toast(a, &key, close, &before);
        }
        self.persist_sidebar();
        Ok(key)
    }

    /// `◆ claude opened ¶ Reading List beside you   ⌘[ back`, coalesced: `◆ claude opened 3
    /// panels beside you (3)`; an eviction names the panel it closed.
    fn agent_toast(&mut self, actor: &str, key: &PanelKey, close: bool, before: &crate::sidebar::SidebarState) {
        let g = self.theme.glyphs();
        let back = if self.cmd_seen { "⌘[" } else { "⌃⌥←" };
        let name = self.panel_name(key);
        let evicted = before.open.iter().find(|p| !p.pinned && p.key() != *key && self.ui.sidebar.get(&p.key()).is_none()).map(|p| self.panel_name(&p.key()));
        let now = self.ui.now_ms;
        let n = match &self.agent_opens {
            Some((a, n, at)) if a == actor && !close && now.saturating_sub(*at) < 5000 => n + 1,
            _ => 1,
        };
        if !close {
            self.agent_opens = Some((actor.to_string(), n, now));
        }
        let text = if close {
            format!("{} {actor} closed {name}   {back} back", g.agent)
        } else if let Some(gone) = evicted {
            format!("{} {actor} opened {name} · closed {gone} to make room   {back} back", g.agent)
        } else if n > 1 {
            format!("{} {actor} opened {n} panels beside you ({n})   {back} back", g.agent)
        } else {
            format!("{} {actor} opened {name} beside you   {back} back", g.agent)
        };
        self.toast_parts(crate::app::ToastKind::Agent, vec![(text, crate::theme::Token::Agent)]);
    }

    /// ⌘[ over an agent's change (§10.4): close what it opened (unless the person pinned it
    /// since), bring back what it closed, unfold what it folded. Nothing else.
    pub fn undo_agent_change(&mut self, c: crate::sidebar::AgentChange) {
        for k in &c.opened {
            if self.ui.sidebar.get(k).is_some_and(|p| !p.pinned) {
                self.commit_panel(k);
                crate::runtime_effects::sidebar_op(self, SidebarOp::Close { key: k.clone() });
                self.ui.sidebar.closed.retain(|p| p.key() != *k);
            }
        }
        for p in c.closed.iter().rev() {
            if self.ui.sidebar.get(&p.key()).is_none() {
                let focus = self.ui.focus;
                crate::runtime_effects::sidebar_op(self, SidebarOp::Open { panel: p.clone(), focus: false, by: None });
                self.ui.focus = focus;
                self.ui.sidebar.closed.retain(|q| q.key() != p.key());
            }
        }
        for k in &c.folded {
            if let Some(p) = self.ui.sidebar.get_mut(k) {
                p.folded = false;
            }
        }
        if self.ui.focus == Focus::Sidebar && !self.ui.sidebar.has_panels() {
            self.ui.focus = Focus::List;
        }
        self.persist_sidebar();
        self.info(format!("took back {}'s change beside you", c.actor));
    }

    /// The stack as `thc ui aside --ls` reports it: each panel with its title and counts.
    pub fn aside_ls(&self) -> serde_json::Value {
        let open: Vec<serde_json::Value> = self
            .ui
            .sidebar
            .open
            .iter()
            .map(|p| {
                let k = p.key();
                let mut v = serde_json::to_value(p).unwrap_or_default();
                v["title"] = serde_json::json!(self.panel_title(&k));
                if let Some(d) = self.panel_doc(&k) {
                    let open = d.blocks().iter().filter(|l| l.kind() == thc_core::outline::Kind::Task && matches!(l.status.as_deref(), Some("todo" | "doing" | "waiting"))).count();
                    v["notes"] = serde_json::json!(d.blocks().iter().filter(|l| !l.text.trim().is_empty()).count());
                    v["open"] = serde_json::json!(open);
                }
                if let Some(rt) = self.lists.get(&k) {
                    v["rows"] = serde_json::json!(rt.rows.iter().filter(|r| r.node().is_some()).count());
                }
                v
            })
            .collect();
        serde_json::json!({"focus": self.ui.focus, "focused": self.ui.sidebar.focused, "shown": self.ui.sidebar.shown, "width": self.ui.sidebar.width, "open": open, "closed": self.ui.sidebar.closed.len()})
    }

    // ---- the stack's file ---------------------------------------------------------------------

    /// The stack in the vault's cache (§11: per vault, per device).
    pub(crate) fn persist_sidebar(&mut self) {
        if crate::SNAPSHOT.with(|s| s.get()) && std::env::var_os("THC_TUI_SNAPSHOT_SIDEBAR").is_none() {
            return;
        }
        let mut s = self.ui.sidebar.clone();
        s.flash = None;
        if let Ok(j) = serde_json::to_vec_pretty(&s) {
            let dir = &self.vault.paths.cache;
            let _ = std::fs::create_dir_all(dir);
            let tmp = dir.join("sidebar.json.tmp");
            if std::fs::write(&tmp, j).is_ok() {
                let _ = std::fs::rename(&tmp, dir.join("sidebar.json"));
            }
        }
    }
}

/// The stack this vault's cache kept (or none).
pub(crate) fn load_sidebar(cache: &std::path::Path, vault: &str) -> crate::sidebar::SidebarState {
    if crate::SNAPSHOT.with(|s| s.get()) && std::env::var_os("THC_TUI_SNAPSHOT_SIDEBAR").is_none() {
        return Default::default();
    }
    // One stack per vault (policy::STACK_PER_VAULT): each vault's cache keeps its own.
    let _ = crate::sidebar::policy::STACK_PER_VAULT;
    let Some(mut s) = std::fs::read(cache.join("sidebar.json")).ok().and_then(|b| serde_json::from_slice::<crate::sidebar::SidebarState>(&b).ok()) else {
        return Default::default();
    };
    // A vault renamed since: its panels follow.
    for p in s.open.iter_mut().chain(s.closed.iter_mut()) {
        p.vault = vault.to_string();
    }
    if s.validate().is_err() {
        return Default::default();
    }
    s
}

fn panel_editor_default() -> crate::editor_pane::EditorState {
    crate::editor_pane::EditorState { parked: true, write: true, ..Default::default() }
}

impl App {
    pub(crate) fn panel_editor(&self, key: &PanelKey) -> Option<&crate::editor_pane::EditorState> {
        self.ui.sidebar.get(key)?.doc_view()?.editor.as_ref()
    }

    pub(crate) fn pointer_editor(&self) -> &crate::editor_pane::EditorState {
        self.panel_pointer.as_ref().and_then(|k| self.panel_editor(k)).unwrap_or(&self.main)
    }

    pub(crate) fn restore_panel_views(&mut self) {
        for k in self.panel_keys() {
            let view = self.ui.sidebar.get(&k).and_then(|p| p.doc_view().cloned());
            if let (Some(v), Some((d, _))) = (view.as_ref(), self.panel_doc_mut(&k)) {
                apply_doc_view(d, v);
            }
            self.main_view_current();
        }
    }
}

fn apply_doc_view(d: &mut Doc, v: &DocView) {
    if v.fresh_end && !d.has_fresh_end() {
        d.caret_to_end(true);
    }
    if let Some(n) = &v.caret_new {
        d.caret_to_new_line(n.after.as_deref(), n.depth, n.kind, v.fresh_end);
    } else if let Some(c) = &v.caret {
        if c.node.is_empty() && v.fresh_end {
            d.caret_to_end(true);
        } else if d.set_caret_anchor(&Anchor { id: c.node.clone(), byte: c.byte }) {
            if let Some(tail) = &v.caret_tail {
                d.restore_caret_tail(&c.node, tail, c.byte);
            }
        }
    }
    d.set_scroll(v.scroll.row, v.scroll_free);
}

// ---- keys -----------------------------------------------------------------------------------

/// A key while the keyboard is in the sidebar (§5.2): the `sidebar` context's chords first, then
/// the panel's own keys (a doc panel writes). False: not the sidebar's.
pub fn key(app: &mut App, k: KeyEvent) -> bool {
    if app.ui.focus != Focus::Sidebar || app.overlay.is_some() || app.prompt.is_some() || app.edit.is_some() || !app.pending_keys.is_empty() {
        return false;
    }
    let Some(pk) = app.ui.sidebar.active_key() else {
        app.ui.focus = Focus::List;
        return false;
    };
    // In a narrow terminal the drawer may have been closed: focus is back in main.
    if !app.ui.sidebar.shown {
        app.ui.focus = Focus::List;
        return false;
    }
    let popup = app.panel_editor(&pk).is_some_and(|e| e.link_open);
    let selected = if pk.kind.is_doc() && k.code == KeyCode::Esc {
        let selected = app.panel_doc_mut(&pk).is_some_and(|(d, _)| d.selection().is_some());
        app.main_view_current();
        selected
    } else { false };
    let key = crate::keymap::Key::of(&k);
    let nav = matches!(k.code, KeyCode::Esc | KeyCode::Up | KeyCode::Down | KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab) && k.modifiers.difference(KeyModifiers::SHIFT).is_empty();
    if !(popup && nav || selected) {
        if let Some(Some(b)) = crate::keymap::lookup(app, &[crate::keymap::Ctx::Sidebar], &[key]) {
            crate::keymap::run(app, b.action);
            return true;
        }
    }
    let panel = app.ui.sidebar.get(&pk).cloned();
    if panel.as_ref().is_some_and(|p| p.folded) {
        return true;
    }
    if !pk.kind.is_doc() {
        return crate::sidebar_list::key(app, &pk, k);
    }
    // Tab in a parked panel goes to the main view's tabs (§5.1).
    let parked = app.panel_editor(&pk).is_none_or(|e| e.parked);
    if parked && matches!(k.code, KeyCode::Tab | KeyCode::BackTab) && !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        app.focus_main();
        crate::keymap::run(app, if k.code == KeyCode::Tab { "view.next" } else { "view.prev" });
        return true;
    }
    if let Some(p) = app.ui.sidebar.get_mut(&pk) {
        p.opened_by = None;
    }
    app.with_panel(&pk, |a| crate::doc_keys::handle(a, k));
    app.sync_panel_view(&pk);
    app.run_deferred(&pk);
    true
}

/// A paste while a doc panel has the keyboard.
pub fn paste(app: &mut App, text: &str) -> bool {
    if app.ui.focus != Focus::Sidebar || app.overlay.is_some() || app.prompt.is_some() {
        return false;
    }
    let Some(pk) = app.ui.sidebar.active_key().filter(|k| k.kind.is_doc()) else { return false };
    app.with_panel(&pk, |a| crate::doc_keys::paste(a, text));
    app.sync_panel_view(&pk);
    app.run_deferred(&pk);
    true
}

/// A writing footer/palette action targets the human's focused document, not main.
pub(crate) fn write_action(app: &mut App, action: &str) {
    let key = app.ui.sidebar.active_key().filter(|k| k.kind.is_doc());
    let consumed = key.as_ref().and_then(|k| app.with_panel(k, |a| crate::doc_keys::run_write(a, action))).unwrap_or(false);
    if let Some(k) = key { app.sync_panel_view(&k); app.run_deferred(&k); }
    if !consumed { crate::keymap::run(app, action); }
}

/// The sidebar's actions (`sidebar.*`, sidebar.md §5.3).
pub fn run(app: &mut App, action: &str) -> bool {
    use crate::sidebar::policy;
    let active = app.ui.sidebar.active_key();
    let screen = app.render.size.width.max(app.screen_width);
    match action {
        "sidebar.open_aside" => match app.aside_target() {
            Some(k) => app.open_aside(k, false),
            None => app.info("nothing here to open beside you · a [[link]], a page or a day"),
        },
        "sidebar.focus" => {
            if app.ui.focus == Focus::Sidebar {
                app.focus_main();
            } else {
                match active {
                    Some(k) => app.focus_panel(k),
                    None => app.info("nothing beside you · ⇧-click a link or ⌥O"),
                }
            }
        }
        "sidebar.back" => app.focus_main(),
        "sidebar.next" | "sidebar.prev" => {
            if let Some(k) = active.clone() {
                app.commit_panel(&k);
            }
            crate::runtime_effects::sidebar_op(app, SidebarOp::Step { delta: if action == "sidebar.next" { 1 } else { -1 } });
            if let Some(k) = app.ui.sidebar.focused.clone() {
                if app.ui.focus == Focus::Sidebar {
                    crate::runtime_effects::sidebar_op(app, SidebarOp::Focus { key: Some(k) });
                }
            }
        }
        "sidebar.fold" => {
            if let Some(k) = active {
                app.commit_panel(&k);
                crate::runtime_effects::sidebar_op(app, SidebarOp::Fold { key: k });
            }
        }
        "sidebar.close" => {
            if let Some(k) = active {
                app.commit_panel(&k);
                crate::runtime_effects::sidebar_op(app, SidebarOp::Close { key: k });
            }
        }
        "sidebar.close_all" => crate::runtime_effects::sidebar_op(app, SidebarOp::CloseUnpinned),
        "sidebar.to_main" => {
            if let Some(k) = active {
                app.panel_to_main(&k);
            }
        }
        "sidebar.pin" => {
            if let Some(k) = active {
                crate::runtime_effects::sidebar_op(app, SidebarOp::Pin { key: k });
            }
        }
        "sidebar.move_up" | "sidebar.move_down" => {
            if let Some(k) = active {
                crate::runtime_effects::sidebar_op(app, SidebarOp::Move { key: k, delta: if action == "sidebar.move_up" { -1 } else { 1 } });
            }
        }
        "sidebar.reopen" => crate::runtime_effects::sidebar_op(app, SidebarOp::Reopen),
        "sidebar.day_prev" | "sidebar.day_next" => {
            let Some(k) = active.filter(|k| k.kind == PanelKind::Day) else {
                app.info("days are in the journal · ⌃O to go");
                return true;
            };
            let Some(d) = k.day_date(app.today) else { return true };
            let to = d + chrono::Duration::days(if action == "sidebar.day_prev" { -1 } else { 1 });
            app.commit_panel(&k);
            let key = PanelKey::day(&k.vault, &to.format("%Y-%m-%d").to_string());
            crate::runtime_effects::sidebar_op(app, SidebarOp::Retarget { key: k, to: key.clone() });
            if app.ui.focus == Focus::Sidebar {
                crate::runtime_effects::sidebar_op(app, SidebarOp::Focus { key: Some(key) });
            }
        }
        "sidebar.toggle" => {
            if app.ui.focus == Focus::Sidebar {
                app.focus_main();
            }
            crate::runtime_effects::sidebar_op(app, SidebarOp::ToggleShown);
        }
        "sidebar.wider" => crate::runtime_effects::sidebar_op(app, SidebarOp::Width { change: WidthChange::Wider, screen }),
        "sidebar.narrower" => crate::runtime_effects::sidebar_op(app, SidebarOp::Width { change: WidthChange::Narrower, screen }),
        "sidebar.width_auto" => crate::runtime_effects::sidebar_op(app, SidebarOp::Width { change: WidthChange::Auto, screen }),
        _ => return false,
    }
    let _ = policy::MAX_PANELS;
    true
}

// ---- the mouse (§12) ------------------------------------------------------------------------

/// A click on a panel's header (by its place in the stack).
pub fn header_click(app: &mut App, i: usize, part: crate::sidebar_ui::Part, clicks: u8, middle: bool) {
    use crate::sidebar_ui::Part;
    let Some(key) = app.ui.sidebar.open.get(i).map(Panel::key) else { return };
    if middle {
        app.commit_panel(&key);
        crate::runtime_effects::sidebar_op(app, SidebarOp::Close { key });
        return;
    }
    match part {
        Part::Close => {
            app.commit_panel(&key);
            crate::runtime_effects::sidebar_op(app, SidebarOp::Close { key });
        }
        Part::OpenMain => app.panel_to_main(&key),
        Part::Pinned => crate::runtime_effects::sidebar_op(app, SidebarOp::Pin { key }),
        Part::More => app.focus_panel(key),
        Part::Title if clicks >= 2 => {
            // The first click folded it: unfold, then open in main.
            if app.ui.sidebar.get(&key).is_some_and(|p| p.folded) {
                crate::runtime_effects::sidebar_op(app, SidebarOp::Fold { key: key.clone() });
            }
            app.panel_to_main(&key);
        }
        Part::Title => {
            // A press: a drag reorders it (§3.5); released where it was, it folds.
            let y = app.derived.sidebar.as_ref().and_then(|p| p.panels.iter().find(|pp| pp.key == key)).map_or(0, |pp| pp.header_y);
            app.sidebar_drag = Some(Drag::Header { key, from: y, to: None });
        }
    }
}

/// Where a header dragged to row `y` would drop: the stack index it would take.
fn drop_index(app: &App, key: &PanelKey, y: u16) -> usize {
    let Some(p) = app.derived.sidebar.as_ref() else { return 0 };
    p.panels.iter().filter(|pp| pp.key != *key && pp.header_y < y).count()
}

/// A drag or release while a sidebar drag is on. True: it was the sidebar's.
fn drag(app: &mut App, m: ratatui::crossterm::event::MouseEvent, clicks: u8) -> bool {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind as K};
    let Some(d) = app.sidebar_drag.clone() else { return false };
    let w = app.render.size.width;
    match (m.kind, d) {
        (K::Drag(MouseButton::Left), Drag::Divider) => {
            let width = w.saturating_sub(m.column + 1);
            crate::runtime_effects::sidebar_op(app, SidebarOp::Width { change: WidthChange::Set(width), screen: w });
        }
        (K::Drag(MouseButton::Left), Drag::Header { key, from, .. }) => {
            let to = (m.row != from).then(|| drop_index(app, &key, m.row));
            app.sidebar_drag = Some(Drag::Header { key, from, to });
        }
        (K::Up(_), Drag::Header { key, to, .. }) => {
            app.sidebar_drag = None;
            match to {
                Some(at) => crate::runtime_effects::sidebar_op(app, SidebarOp::MoveTo { key, at }),
                None => {
                    let _ = clicks;
                    app.commit_panel(&key);
                    crate::runtime_effects::sidebar_op(app, SidebarOp::Fold { key });
                }
            }
        }
        (K::Up(_), Drag::Divider) => app.sidebar_drag = None,
        _ => return false,
    }
    true
}

/// The mouse over a panel's body: a press focuses it and places its caret, exactly as in the
/// main view; its drag and release follow; the wheel scrolls it without moving its caret. A
/// press in the main view while the sidebar has the keyboard gives it back to main. True:
/// the sidebar took the event.
pub fn mouse(app: &mut App, m: ratatui::crossterm::event::MouseEvent, clicks: u8) -> bool {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind as K};
    let (x, y) = (m.column, m.row);
    if drag(app, m, clicks) {
        return true;
    }
    // The divider: a drag resizes, a double-click goes back to automatic (§6.1).
    if let (K::Down(MouseButton::Left), Some(s)) = (m.kind, app.sidebar_col) {
        let dx = app.render.size.width.saturating_sub(s + 1);
        let rows = app.derived.sidebar.as_ref().map(|p| (p.area.y, p.area.bottom()));
        if x == dx && rows.is_some_and(|(a, b)| y >= a && y < b) {
            if clicks >= 2 {
                app.sidebar_drag = None;
                crate::runtime_effects::sidebar_op(app, SidebarOp::Width { change: WidthChange::Auto, screen: app.render.size.width });
            } else {
                app.sidebar_drag = Some(Drag::Divider);
            }
            return true;
        }
    }
    let target = app.render.panel_targets.iter().rev().find_map(|(k, ts)| {
        ts.iter().rev().find(|t| t.y == y && x >= t.x0 && x < t.x1).map(|t| (k.clone(), t.what.clone()))
    });
    let over = target.as_ref().map(|(k, _)| k.clone()).or_else(|| app.render.panel_views.iter().find(|(_, r)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom()).map(|(k, _)| k.clone()));
    match m.kind {
        K::ScrollUp | K::ScrollDown => {
            let Some(k) = over else { return false };
            let rows = app.tui_prefs.wheel_rows.max(1) as isize;
            if let Some((d, _)) = app.panel_doc_mut(&k) {
                d.scroll_view(if m.kind == K::ScrollUp { -rows } else { rows });
            }
            app.main_view_current();
            app.sync_panel_view(&k);
            true
        }
        K::Down(MouseButton::Left | MouseButton::Middle) => {
            let Some(k) = over else {
                // A press in the main view takes the keyboard back there (the click goes on).
                let in_sidebar = match app.sidebar_over {
                    Some((_, r)) => x >= r.x && x < r.right() && y >= r.y && y < r.bottom(),
                    None => app.derived.sidebar.as_ref().is_some_and(|p| x >= p.area.x && y >= p.area.y && y < p.area.bottom()),
                };
                if app.ui.focus == Focus::Sidebar && !in_sidebar && y > 1 && y + 1 < app.render.size.height {
                    app.focus_main();
                }
                return in_sidebar;
            };
            if app.ui.focus != Focus::Sidebar || app.ui.sidebar.focused.as_ref() != Some(&k) {
                app.focus_panel(k.clone());
            }
            if let Some(p) = app.ui.sidebar.get_mut(&k) {
                p.opened_by = None;
            }
            app.panel_pointer = Some(k.clone());
            app.with_panel(&k, |a| {
                if !crate::editor_pane::clicks::target(a, target.as_ref().map(|(_, t)| t), m) {
                    crate::doc_keys::mouse(a, m, clicks);
                }
            });
            app.sync_panel_view(&k);
            app.run_deferred(&k);
            true
        }
        K::Drag(MouseButton::Left) | K::Up(_) => {
            let Some(k) = app.panel_pointer.clone() else { return false };
            if matches!(m.kind, K::Up(_)) {
                app.panel_pointer = None;
            }
            app.with_panel(&k, |a| crate::doc_keys::mouse(a, m, clicks));
            app.sync_panel_view(&k);
            app.run_deferred(&k);
            true
        }
        _ => false,
    }
}

/// ⌃W with panels (§5.1): main → each expanded panel → main.
pub fn pane_next(app: &mut App) {
    let expanded: Vec<PanelKey> = app.ui.sidebar.open.iter().filter(|p| !p.folded).map(Panel::key).collect();
    match app.ui.focus {
        Focus::Sidebar => {
            let cur = app.ui.sidebar.active_key();
            let i = cur.and_then(|c| expanded.iter().position(|k| *k == c));
            match i.and_then(|i| expanded.get(i + 1)).cloned() {
                Some(k) => app.focus_panel(k),
                None => app.focus_main(),
            }
        }
        _ => match expanded.first().cloned() {
            Some(k) => app.focus_panel(k),
            None => app.ui.focus = Focus::List,
        },
    }
}

/// The runtime half of the pure update's effects.
pub(crate) fn perform(app: &mut App, effect: Effect) {
    match effect {
        Effect::SidebarLoad { key } if !key.kind.is_doc() => app.refresh_list(&key),
        Effect::SidebarDrop { key } if !key.kind.is_doc() => {
            app.lists.remove(&key);
        }
        Effect::SidebarLoad { key } => app.load_panel(&key),
        Effect::SidebarDrop { key } => app.drop_panel(&key),
        Effect::SidebarPersist => {
            app.sync_panel_views();
            app.persist_sidebar();
        }
        Effect::SidebarEvicted { gone, opened, by } => {
            let gone_name = app.ui.sidebar.closed.first().filter(|p| p.key() == gone).map(|_| app.panel_name(&gone)).unwrap_or_else(|| app.panel_name(&gone));
            match by {
                None => app.info(format!("{gone_name} closed to make room · ⌥⇧T reopen")),
                Some(actor) => {
                    let opened = app.panel_name(&opened);
                    app.info(format!("◆ {actor} opened {opened} · closed {gone_name} to make room"));
                }
            }
        }
        _ => {}
    }
}
