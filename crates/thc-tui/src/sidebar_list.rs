//! List panels (sidebar.md §8.2): Today, Inbox, Tasks, Log, a saved view, a `#tag` or any query
//! beside the main view. Its rows are built by the same code as the main view's
//! ([`App::with_list_panel`] runs it with the panel's view and selection in place), and the list
//! keys act on the panel's row; anything that goes somewhere happens in the main view.

use crate::app::{App, Focus, Row, View};
use crate::sidebar::{ListScroll, ListState, PanelKey, PanelKind};
use crate::update::SidebarOp;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A list panel's runtime: its rows and selection, and its own view choices.
#[derive(Default)]
pub struct ListRt {
    pub rows: Vec<Row>,
    pub cursor: usize,
    pub selected: Option<String>,
    pub scroll: usize,
    row_vault: std::collections::HashMap<usize, usize>,
    agenda: bool,
    show_done: bool,
    /// The view or query couldn't be read: why.
    pub problem: Option<String>,
}

/// What the main list keeps, stashed while a panel's list runs.
struct Stash {
    doc: Option<crate::editor::Doc>,
    view: View,
    cursor: usize,
    scroll: usize,
    selected: Option<String>,
    tasks_filter: String,
    agenda: bool,
    show_done: bool,
    log_node: Option<String>,
    review_lane: bool,
    rows: Vec<Row>,
    row_vault: std::collections::HashMap<usize, usize>,
    tasks_error: Option<(String, String, Option<String>)>,
    tasks_group: Option<String>,
    focus: Focus,
    prompt_view: Option<View>,
}

/// The view and filter a list panel's key reads as.
pub fn view_of(key: &PanelKey) -> (View, Option<String>) {
    match (key.kind, key.view.as_deref(), key.query.as_deref()) {
        (PanelKind::View, Some("today"), _) => (View::Today, None),
        (PanelKind::View, Some("inbox"), _) => (View::Inbox, None),
        (PanelKind::View, Some("tasks"), _) => (View::Tasks, Some(crate::ui_state::DEFAULT_TASKS_FILTER.to_string())),
        (PanelKind::View, Some("log"), _) => (View::Log, None),
        (PanelKind::View, Some(v), _) => (View::Tasks, Some(v.to_string())),
        // A tag panel: its notes, grouped by where they live.
        (PanelKind::Query, _, Some(q)) if q.starts_with('#') && !q.contains(' ') => (View::Tasks, Some(format!("{q} group:parent"))),
        (_, _, Some(q)) => (View::Tasks, Some(q.to_string())),
        _ => (View::Today, None),
    }
}

impl App {
    /// Run `f` with list panel `key`'s list in the main list's place: its view, filter, rows
    /// and selection (the main document set aside, so nothing in `f` can close it). None: not a
    /// list panel, or inside a panel already.
    pub fn with_list_panel<R>(&mut self, key: &PanelKey, f: impl FnOnce(&mut App) -> R) -> Option<R> {
        if key.kind.is_doc() || self.in_list.is_some() || self.in_panel.is_some() {
            return None;
        }
        let mut rt = self.lists.remove(key).unwrap_or_default();
        let (view, filter) = view_of(key);
        let ui = &mut self.ui;
        let stash = Stash {
            doc: self.doc.take(),
            view: std::mem::replace(&mut ui.view, view),
            cursor: std::mem::replace(&mut ui.cursor, rt.cursor),
            scroll: std::mem::replace(&mut ui.scroll, rt.scroll),
            selected: std::mem::replace(&mut ui.selected, rt.selected.clone()),
            tasks_filter: match filter {
                Some(f) => std::mem::replace(&mut ui.tasks_filter, f),
                None => ui.tasks_filter.clone(),
            },
            agenda: std::mem::replace(&mut ui.agenda_mode, rt.agenda),
            show_done: std::mem::replace(&mut ui.show_all_done, rt.show_done),
            log_node: ui.log_node.take(),
            review_lane: std::mem::replace(&mut ui.review_lane, false),
            rows: std::mem::replace(&mut self.rows, std::mem::take(&mut rt.rows)),
            row_vault: std::mem::replace(&mut self.row_vault, std::mem::take(&mut rt.row_vault)),
            tasks_error: self.tasks_error.take(),
            tasks_group: self.tasks_group.take(),
            focus: std::mem::replace(&mut ui.focus, Focus::List),
            prompt_view: None,
        };
        self.in_list = Some(key.clone());
        let r = f(self);
        self.in_list = None;
        let ui = &mut self.ui;
        rt.cursor = std::mem::replace(&mut ui.cursor, stash.cursor);
        rt.scroll = std::mem::replace(&mut ui.scroll, stash.scroll);
        rt.selected = std::mem::replace(&mut ui.selected, stash.selected);
        rt.agenda = std::mem::replace(&mut ui.agenda_mode, stash.agenda);
        rt.show_done = std::mem::replace(&mut ui.show_all_done, stash.show_done);
        ui.view = stash.view;
        ui.tasks_filter = stash.tasks_filter;
        ui.log_node = stash.log_node;
        ui.review_lane = stash.review_lane;
        ui.focus = stash.focus;
        let _ = stash.prompt_view;
        rt.rows = std::mem::replace(&mut self.rows, stash.rows);
        rt.row_vault = std::mem::replace(&mut self.row_vault, stash.row_vault);
        rt.problem = self.tasks_error.take().map(|e| e.0);
        self.tasks_error = stash.tasks_error;
        self.tasks_group = stash.tasks_group;
        self.doc = stash.doc;
        self.lists.insert(key.clone(), rt);
        Some(r)
    }

    /// A list panel's rows, read again (it opened, the vault changed).
    pub(crate) fn refresh_list(&mut self, key: &PanelKey) {
        let remembered = self.ui.sidebar.get(key).and_then(|p| p.list.as_ref()).and_then(|l| l.selected.clone());
        if !self.lists.contains_key(key) {
            self.lists.insert(key.clone(), ListRt { selected: remembered, ..Default::default() });
        }
        self.with_list_panel(key, |a| {
            let _ = a.reload();
        });
        self.sync_list_state(key);
    }

    /// Every list panel, read again.
    pub(crate) fn refresh_lists(&mut self) {
        let keys: Vec<PanelKey> = self.panel_keys().into_iter().filter(|k| !k.kind.is_doc()).collect();
        self.lists.retain(|k, _| keys.contains(k));
        for k in keys {
            self.refresh_list(&k);
        }
    }

    /// The panel's selection into its UiState (`list.selected`, `list.scroll`).
    pub(crate) fn sync_list_state(&mut self, key: &PanelKey) {
        let Some(rt) = self.lists.get(key) else { return };
        let st = ListState { selected: rt.selected.clone(), scroll: ListScroll { anchor: None, offset: rt.scroll } };
        if let Some(p) = self.ui.sidebar.get_mut(key) {
            p.list = Some(st);
        }
    }
}

/// The row a list panel row opens beside (`o`, ⇧Enter, ⌥O): its page or day.
fn row_target(app: &mut App, key: &PanelKey) -> Option<PanelKey> {
    app.with_list_panel(key, |a| a.aside_target()).flatten()
}

/// A key in a list panel (§5.2, §8.2): the `sidebar` chords came first; then the list keys on
/// the panel's own selection. `Enter` opens the row in the main view; `o`, ⇧Enter, ⌥O beside.
pub fn key(app: &mut App, pk: &PanelKey, k: KeyEvent) -> bool {
    let plain = !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    // Open beside.
    if (k.code == KeyCode::Char('o') && plain) || (k.code == KeyCode::Enter && shift) || (k.code == KeyCode::Char('o') && k.modifiers.contains(KeyModifiers::ALT)) {
        match row_target(app, pk) {
            Some(t) => app.open_aside(t, false),
            None => app.info("nothing to open beside on this row"),
        }
        return true;
    }
    // Enter: the row in the main view (§12), the keyboard with it.
    if k.code == KeyCode::Enter && k.modifiers.is_empty() {
        let id = app.lists.get(pk).and_then(|rt| rt.rows.get(rt.cursor)).and_then(|r| r.node()).map(|n| n.id.clone());
        if let Some(id) = id {
            app.focus_main();
            app.save_doc(true);
            app.remember_origin();
            app.focus_node(&id);
        }
        return true;
    }
    // The list's own keys, on the panel's row. What goes elsewhere (views, the palette) is the
    // main view's; anything that writes reads the vault again everywhere after.
    let mut wrote = false;
    app.with_list_panel(pk, |a| {
        let before = a.vault.log.files().ok();
        crate::keymap::dispatch(a, &k);
        wrote = a.vault.log.files().ok() != before;
    });
    app.sync_list_state(pk);
    let deferred = std::mem::take(&mut app.panel_defer);
    for d in deferred {
        match d {
            crate::sidebar_app::Deferred::Action(a) => {
                app.focus_main();
                crate::keymap::run(app, &a);
            }
            crate::sidebar_app::Deferred::Aside(k) => app.open_aside(k, false),
            crate::sidebar_app::Deferred::Follow(t) => app.follow_in_main(&t),
        }
    }
    if wrote {
        let _ = app.reload();
    }
    true
}

/// Actions a list panel runs itself; the rest are the main view's (deferred).
pub fn runs_in_list(action: &str) -> bool {
    action.starts_with("node.") || action.starts_with("cursor.") || action.starts_with("fold.") || matches!(action, "today.agenda_toggle" | "today.show_done" | "leader" | "undo" | "redraw" | "toast.done")
}

/// A click on a list panel's row: select it; a double-click opens it in the main view.
pub fn click_row(app: &mut App, pk: &PanelKey, row: usize, clicks: u8) {
    if app.ui.focus != Focus::Sidebar || app.ui.sidebar.focused.as_ref() != Some(pk) {
        app.focus_panel(pk.clone());
    }
    if let Some(rt) = app.lists.get_mut(pk) {
        if rt.rows.get(row).is_some_and(|r| r.selectable()) {
            rt.cursor = row;
            rt.selected = rt.rows[row].key();
        }
    }
    app.sync_list_state(pk);
    if clicks >= 2 {
        key(app, pk, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }
}

/// `nothing here`, the rows as the panel draws them: (text, meta, status, is a heading).
pub fn rows_of(app: &App, pk: &PanelKey) -> Vec<(String, String, Option<String>, bool)> {
    let Some(rt) = app.lists.get(pk) else { return vec![] };
    let s = &app.vault.store;
    rt.rows
        .iter()
        .map(|r| match r {
            Row::Section { title, count, .. } => (format!("{title}{}", count.map(|c| format!("  {c}")).unwrap_or_default()), String::new(), None, true),
            Row::Node { node, .. } => {
                let block = thc_core::outline::Block {
                    id: node.id.clone(),
                    parent: node.parent.clone(),
                    depth: 0,
                    kind: if node.status.is_some() { thc_core::outline::Kind::Task } else { thc_core::outline::Kind::Bullet },
                    status: node.status.clone(),
                    text: node.text.clone(),
                    scheduled: node.scheduled.clone(),
                    due: node.due.clone(),
                    priority: node.priority.clone(),
                    repeat: node.repeat.as_ref().and_then(|r| r.get("text")).and_then(|t| t.as_str()).map(str::to_string),
                    tags: vec![],
                    rev: None,
                    text_rev: None,
                    conflict: false,
                    done_at: node.done_at.clone(),
                    gap: None,
                };
                (s.render_text(&node.label()), crate::editor::meta_text(&block, app.today), node.status.clone(), false)
            }
            Row::Empty { l1, .. } => (l1.clone(), String::new(), None, true),
            Row::Muted(t) => (t.clone(), String::new(), None, true),
            Row::Tag { name, count, .. } => (format!("#{name}  {count}"), String::new(), None, false),
            _ => (String::new(), String::new(), None, true),
        })
        .collect()
}
