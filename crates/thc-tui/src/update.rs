//! Pure presentation updates. Fields exposes only presentation values and the open document
//! (whose model reads no clock and mints no ids), so this reducer cannot reach a Vault,
//! terminal, channel, clock, or filesystem. Editing commands run here; saving, minting node ids
//! and re-reading the vault come back as effects.
//! This is the first P3 migration; legacy input/persistence paths remain outside it.
use crate::{
    app::{Toast, ToastKind},
    theme::Token,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Viewport {
    List { cursor: usize, scroll: usize, height: usize, previous_section: bool, heights: Vec<usize> },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Msg {
    ViewportPrepared(Viewport),
    TogglePageIds { at: u64 },
    PageIdsPersisted { result: Result<(), String> },
    /// `space t D` / the palette: document mode on or off (writing.md §1).
    ToggleDocumentMode { at: u64 },
    DocumentModePersisted { result: Result<(), String> },
    Copy { text: String, notice: String },
    ClipboardResult { result: Result<(), String>, notice: String, at: u64 },
    RemapKeys,
    KeysEdited { result: Result<String, String>, at: u64 },
    /// The runtime's clock for the open document, before it hands it input. The document
    /// asks for the node ids its next edits need (`Effect::MintIds`).
    DocClock { now_ms: u64 },
    /// Node ids the runtime minted for the open document.
    IdsMinted { ids: Vec<String> },
    /// An editing command on the open document: a caretline command id (`move.left`,
    /// `history.undo`) or thc's host command (`thc.task_cycle`).
    Editor { command: String, at: u64 },
    /// Text typed at the caret of the open document.
    Type { text: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    WritePageIds { visible: bool },
    /// This device's document mode, for every vault (`App::load_document_mode`).
    WriteDocumentMode { on: bool },
    WriteClipboard { text: String, notice: String },
    EditKeys,
    /// Rebuild the rows (and open, switch or close the document) from the store.
    Reload,
    /// Save the open document's typing.
    SaveDoc,
    /// Mint `n` node ids for the open document (`Msg::IdsMinted` brings them back).
    MintIds { n: usize },
    /// Re-read the open document from the vault (an undo or redo restored lines that may have
    /// changed elsewhere meanwhile).
    Reopen,
    Quit,
    SetMouse { on: bool },
    /// `$EDITOR` on a note.
    SpawnEditor { target: String },
    /// A panel joined the stack (or came back): its document or list is the runtime's to load.
    SidebarLoad { key: crate::sidebar::PanelKey },
    /// A panel left the stack: its view (and its document, when no other view holds it) goes.
    SidebarDrop { key: crate::sidebar::PanelKey },
    /// The stack changed: keep it in the vault's cache (sidebar.md §11).
    SidebarPersist,
    /// A panel closed to make room for another (the bar names both, by their titles).
    SidebarEvicted { gone: crate::sidebar::PanelKey, opened: crate::sidebar::PanelKey, by: Option<String> },
}

/// A change to the sidebar's stack, focus or width (sidebar.md §2–§6). The runtime commits a
/// panel's typing before it asks to close or leave it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SidebarOp {
    /// Open a panel (§2): `focus` moves the keyboard to it (the finder, the palette, the
    /// drawer); `by` names an agent that opened it.
    Open { panel: crate::sidebar::Panel, focus: bool, by: Option<String> },
    Close { key: crate::sidebar::PanelKey },
    CloseUnpinned,
    Reopen,
    Pin { key: crate::sidebar::PanelKey },
    Fold { key: crate::sidebar::PanelKey },
    Move { key: crate::sidebar::PanelKey, delta: isize },
    MoveTo { key: crate::sidebar::PanelKey, at: usize },
    /// The keyboard to a panel (None: back to the main view).
    Focus { key: Option<crate::sidebar::PanelKey> },
    /// The active panel moves to the next (1) or previous (-1) one.
    Step { delta: isize },
    /// ⌥\: hide or show.
    ToggleShown,
    /// A day panel goes to another day in place (⌃P ⌃N, §8.1): no history step.
    Retarget { key: crate::sidebar::PanelKey, to: crate::sidebar::PanelKey },
    /// ⌥= ⌥- ⌥0 and the divider: a width, from the current one at screen width `screen`.
    Width { change: WidthChange, screen: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WidthChange {
    Wider,
    Narrower,
    Auto,
    Set(u16),
}

/// The sidebar's pure update: the stack's rules (sidebar.rs) on the state, what to load,
/// drop and keep as effects, and what the bar says.
pub(crate) fn sidebar(ui: &mut crate::ui_state::UiState, op: SidebarOp) -> Vec<Effect> {
    use crate::app::Focus;
    use crate::sidebar::{Opened, policy};
    let now = ui.now_ms;
    let mut fx = Vec::new();
    match op {
        SidebarOp::Open { mut panel, focus, by } => {
            let key = panel.key();
            panel.opened_by = by.clone();
            let was_focus = ui.focus;
            match ui.sidebar.open(panel, now) {
                Opened::Full => {
                    ui.info(format!("{} panels pinned · unpin one to open another", policy::MAX_PANELS));
                    return fx;
                }
                Opened::New { evicted } => {
                    fx.push(Effect::SidebarLoad { key: key.clone() });
                    if let Some(gone) = evicted {
                        fx.push(Effect::SidebarDrop { key: gone.key() });
                        fx.push(Effect::SidebarEvicted { gone: gone.key(), opened: key.clone(), by: by.clone() });
                    }
                }
                Opened::Moved => fx.push(Effect::SidebarLoad { key: key.clone() }),
            }
            ui.sidebar.shown = true;
            if focus && (by.is_none() || policy::AGENTS_MOVE_FOCUS) {
                ui.focus = Focus::Sidebar;
            } else {
                // Keyboard focus stays where it was (§2 rule 1, §10.4).
                ui.focus = if was_focus == Focus::Sidebar && !ui.sidebar.open.is_empty() { Focus::Sidebar } else if was_focus == Focus::Sidebar { Focus::List } else { was_focus };
            }
        }
        SidebarOp::Close { key } => {
            if ui.sidebar.close(&key).is_some() {
                fx.push(Effect::SidebarDrop { key });
            }
        }
        SidebarOp::CloseUnpinned => {
            for key in ui.sidebar.close_unpinned() {
                fx.push(Effect::SidebarDrop { key });
            }
        }
        SidebarOp::Reopen => match ui.sidebar.reopen(now) {
            Some((_, Opened::Full)) => ui.info(format!("{} panels pinned · unpin one to open another", policy::MAX_PANELS)),
            Some((key, opened)) => {
                fx.push(Effect::SidebarLoad { key });
                if let Opened::New { evicted: Some(gone) } = opened {
                    fx.push(Effect::SidebarDrop { key: gone.key() });
                }
                ui.sidebar.shown = true;
            }
            None => ui.info("nothing closed to reopen"),
        },
        SidebarOp::Pin { key } => {
            if let Some(p) = ui.sidebar.get_mut(&key) {
                p.opened_by = None;
            }
            if let Some(on) = ui.sidebar.toggle_pin(&key) {
                ui.info(if on { "pinned · ⌥P unpins" } else { "unpinned" });
            }
        }
        SidebarOp::Fold { key } => {
            if let Some(p) = ui.sidebar.get_mut(&key) {
                p.opened_by = None;
            }
            ui.sidebar.toggle_fold(&key);
            ui.sidebar.focused = Some(key);
        }
        SidebarOp::Move { key, delta } => {
            ui.sidebar.move_within(&key, delta);
        }
        SidebarOp::MoveTo { key, at } => {
            ui.sidebar.move_to(&key, at);
        }
        SidebarOp::Focus { key: Some(key) } => {
            if ui.sidebar.get(&key).is_some() {
                if let Some(p) = ui.sidebar.get_mut(&key) {
                    p.opened_by = None;
                    p.folded = false;
                }
                ui.sidebar.focused = Some(key);
                ui.sidebar.shown = true;
                ui.focus = Focus::Sidebar;
            }
        }
        SidebarOp::Focus { key: None } => {
            if ui.focus == Focus::Sidebar {
                ui.focus = Focus::List;
            }
        }
        SidebarOp::Step { delta } => {
            ui.sidebar.step_focus(delta);
        }
        SidebarOp::Retarget { key, to } => {
            if ui.sidebar.get(&to).is_some() {
                ui.sidebar.focused = Some(to);
            } else if let Some(p) = ui.sidebar.get_mut(&key) {
                let (pinned, folded) = (p.pinned, p.folded);
                let mut np = crate::sidebar::Panel::new(to.clone());
                np.pinned = pinned;
                np.folded = folded;
                *p = np;
                if ui.sidebar.focused.as_ref() == Some(&key) {
                    ui.sidebar.focused = Some(to.clone());
                }
                fx.push(Effect::SidebarDrop { key });
                fx.push(Effect::SidebarLoad { key: to });
            }
        }
        SidebarOp::ToggleShown => {
            if ui.sidebar.open.is_empty() {
                ui.info("nothing beside you · ⇧-click a link or ⌥O");
                return fx;
            }
            ui.sidebar.shown = !ui.sidebar.shown;
            if !ui.sidebar.shown && ui.focus == Focus::Sidebar {
                ui.focus = Focus::List;
            }
        }
        SidebarOp::Width { change, screen } => {
            let cur = crate::sidebar::column_width(ui.sidebar.width, screen);
            ui.sidebar.width = match change {
                WidthChange::Wider => Some(cur + policy::WIDTH_STEP),
                WidthChange::Narrower => Some(cur.saturating_sub(policy::WIDTH_STEP)),
                WidthChange::Auto => None,
                WidthChange::Set(w) => Some(w),
            }
            .map(|w| w.clamp(policy::SET_MIN_W, screen.saturating_sub(policy::MAIN_MIN_W).max(policy::SET_MIN_W)));
        }
    }
    if ui.sidebar.open.is_empty() && ui.focus == Focus::Sidebar {
        ui.focus = Focus::List;
    }
    fx.push(Effect::SidebarPersist);
    fx
}


/// What a navigation action needs to know besides the state: derived rows and facts the
/// runtime holds. Never the store.
pub(crate) struct Facts<'a> {
    pub rows: &'a [crate::app::Row],
    pub doc_open: bool,
    pub mouse: bool,
    /// Rows a half page moves.
    pub half_page: isize,
}

impl Facts<'_> {
    fn selected_id(&self, ui: &crate::ui_state::UiState) -> Option<String> {
        self.rows.get(ui.cursor).and_then(|r| r.node()).map(|n| n.id.clone())
    }
}

/// A keymap action that only changes presentation, as a pure update: the state changes here,
/// and what needs the store or the terminal comes back as effects. None: not one of these
/// (keymap.rs runs it the older way, on App).
pub(crate) fn action(ui: &mut crate::ui_state::UiState, facts: &Facts<'_>, action: &str) -> Option<Vec<Effect>> {
    use crate::app::{Focus, Overlay, PromptKind, VIEWS, View};
    use crate::input::LineInput;
    let reload = || Some(vec![Effect::Reload]);
    let go = |ui: &mut crate::ui_state::UiState, v: View| {
        ui.enter_view(v);
        Some(vec![Effect::Reload])
    };
    match action {
        "go.today" => go(ui, View::Today),
        "go.inbox" => go(ui, View::Inbox),
        "go.tasks" => go(ui, View::Tasks),
        "go.log" => go(ui, View::Log),
        "go.journal" => {
            // The journal is a destination, not a detour: Esc from it goes to Today.
            ui.doc_origin = None;
            go(ui, View::Journal)
        }
        // Arriving never takes the cursor; `/` is a find, so it does.
        "go.search" => {
            if ui.view != View::Search {
                return go(ui, View::Search);
            }
            Some(vec![])
        }
        "search.find" => {
            let effects = if ui.view != View::Search { go(ui, View::Search) } else { Some(vec![]) };
            ui.prompt = Some((PromptKind::Search, LineInput::with(&ui.search_terms)));
            effects
        }
        "tasks.filter" => {
            let effects = if ui.view != View::Tasks { go(ui, View::Tasks) } else { Some(vec![]) };
            ui.prompt = Some((PromptKind::Filter, LineInput::with(&ui.tasks_filter)));
            ui.input_untouched = true;
            effects
        }
        "view.next" | "view.prev" => {
            let i = VIEWS.iter().position(|v| *v == ui.view).unwrap_or(0);
            let n = VIEWS.len();
            match VIEWS[if action == "view.next" { (i + 1) % n } else { (i + n - 1) % n }] {
                // Pages rests on the page last opened: that needs its rows (App::show_pages).
                View::Pages => None,
                v => go(ui, v),
            }
        }
        "pages.filter" => {
            ui.prompt = Some((PromptKind::PagesFilter, LineInput::with(&ui.pages_filter)));
            Some(vec![])
        }
        // `?` on a view: how it's built first; `?` there shows the keys.
        "help.context" => {
            ui.overlay = Some(match ui.recipe_name().filter(|_| !facts.doc_open) {
                Some(name) => Overlay::Recipe { name },
                None => Overlay::Help { all: false, scroll: 0 },
            });
            Some(vec![])
        }
        "view.explain" => {
            match ui.recipe_name() {
                Some(name) => ui.overlay = Some(Overlay::Recipe { name }),
                None => ui.info("no view here to explain · Today, Inbox, Tasks"),
            }
            Some(vec![])
        }
        "help.all" => {
            ui.overlay = Some(Overlay::Help { all: true, scroll: 0 });
            Some(vec![])
        }
        "focus.toggle" => {
            let on = !ui.focus_mode;
            ui.set_focus_mode(on);
            Some(vec![])
        }
        "view.save" => {
            ui.overlay = Some(Overlay::Palette { input: LineInput::with("view add "), sel: 0 });
            Some(vec![])
        }
        "page.new" => {
            ui.prompt = Some((PromptKind::NewPage, LineInput::default()));
            Some(vec![])
        }
        "finder.open" => {
            ui.overlay = Some(Overlay::Finder { input: LineInput::default(), sel: 0 });
            Some(vec![])
        }
        "nav.history" => {
            ui.overlay = Some(Overlay::History { sel: 0 });
            Some(vec![])
        }
        "go.date" => {
            ui.prompt = Some((PromptKind::GoDate, LineInput::default()));
            Some(vec![])
        }
        "pane.detail_toggle" => {
            ui.show_detail = !ui.show_detail;
            Some(vec![])
        }
        "pane.next" => {
            ui.focus = if ui.focus == Focus::List { Focus::Detail } else { Focus::List };
            Some(vec![])
        }
        "mouse.toggle" => Some(vec![Effect::SetMouse { on: !facts.mouse }]),
        "quit" => Some(vec![Effect::SaveDoc, Effect::Quit]),
        // ⌃L: redraw, and lists re-sort (your own changes move to their place).
        "redraw" => reload(),
        "cursor.down" => cursor(ui, facts, 1),
        "cursor.up" => cursor(ui, facts, -1),
        "cursor.half_down" => cursor(ui, facts, facts.half_page),
        "cursor.half_up" => cursor(ui, facts, -facts.half_page),
        "cursor.page_down" => cursor(ui, facts, facts.half_page * 2),
        "cursor.page_up" => cursor(ui, facts, -facts.half_page * 2),
        "cursor.top" | "cursor.bottom" => {
            let rows = facts.rows;
            let i = if action == "cursor.top" { rows.iter().position(|r| r.selectable()) } else { rows.iter().rposition(|r| r.selectable()) };
            if let Some(i) = i {
                ui.cursor = i;
                ui.selected = rows[i].key();
            }
            Some(vec![])
        }
        "node.move" => {
            if let Some(id) = facts.selected_id(ui) {
                ui.overlay = Some(Overlay::Move { node: id, input: LineInput::default(), sel: 0 });
            }
            Some(vec![])
        }
        "node.edit_external" => Some(facts.selected_id(ui).map(|target| vec![Effect::SpawnEditor { target }]).unwrap_or_default()),
        "node.history" => {
            ui.log_node = facts.selected_id(ui);
            ui.view = View::Log;
            ui.selected = None;
            ui.cursor = 0;
            reload()
        }
        "today.by_vault" => {
            // Sections per vault instead of merged.
            ui.today_by_vault = !ui.today_by_vault;
            ui.info(if ui.today_by_vault { "by vault · space t v merges them" } else { "merged · space t v for a section per vault" });
            reload()
        }
        "today.agenda_toggle" => {
            ui.agenda_mode = !ui.agenda_mode;
            ui.selected = None;
            reload()
        }
        "today.show_done" => {
            ui.show_all_done = !ui.show_all_done;
            reload()
        }
        "day.prev" | "day.next" | "week.prev" | "week.next" => {
            let delta = match action {
                "day.prev" => -1,
                "day.next" => 1,
                "week.prev" => -7,
                _ => 7,
            };
            if ui.view == View::Today {
                ui.journal_date = ui.today;
            }
            ui.journal_date += chrono::Duration::days(delta);
            ui.selected = None;
            go(ui, View::Journal)
        }
        _ => None,
    }
}

/// Move the list cursor by `delta` selectable rows.
fn cursor(ui: &mut crate::ui_state::UiState, facts: &Facts<'_>, delta: isize) -> Option<Vec<Effect>> {
    let rows = facts.rows;
    if rows.is_empty() {
        return Some(vec![]);
    }
    let mut i = ui.cursor as isize;
    let step = delta.signum();
    for _ in 0..delta.abs() {
        let mut j = i + step;
        while j >= 0 && (j as usize) < rows.len() && !rows[j as usize].selectable() {
            j += step;
        }
        if j < 0 || j as usize >= rows.len() {
            break;
        }
        i = j;
    }
    ui.cursor = i as usize;
    ui.selected = rows.get(ui.cursor).and_then(|r| r.key());
    Some(vec![])
}

pub(crate) struct Fields<'a> {
    pub page_ids: Option<&'a mut bool>,
    pub document_mode: Option<&'a mut bool>,
    pub cursor: usize,
    pub scroll: &'a mut usize,
    pub toast: &'a mut Option<Toast>,
    /// The open document, if any.
    pub doc: Option<&'a mut crate::editor::Doc>,
}
/// The initial list position also tells derivation which row heights are needed.
/// The complete follow policy runs in update; preparation never assigns scroll.
pub(crate) fn list_start(mut scroll: usize, cursor: usize, height: usize, previous_section: bool) -> usize {
    if cursor < scroll {
        scroll = cursor;
    }
    if cursor >= scroll.saturating_add(height) {
        scroll = cursor.saturating_add(1).saturating_sub(height);
    }
    if scroll > 0 && scroll == cursor && previous_section {
        scroll -= 1;
    }
    scroll
}
pub(crate) fn update(state: Fields<'_>, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::ViewportPrepared(Viewport::List { cursor, scroll, height, previous_section, heights }) => {
            if state.cursor != cursor || *state.scroll != scroll {
                return vec![];
            }
            let mut start = list_start(scroll, cursor, height, previous_section);
            let mut total: usize = heights.iter().sum();
            for h in heights {
                if start >= cursor || total <= height {
                    break;
                }
                total = total.saturating_sub(h);
                start += 1;
            }
            *state.scroll = start;
        }
        Msg::TogglePageIds { at } => {
            let Some(page_ids) = state.page_ids else { return vec![] };
            *page_ids = !*page_ids;
            let text = if *page_ids { "IDs shown · . to hide" } else { "IDs hidden · . to show" };
            *state.toast = Some(Toast { kind: ToastKind::Info, parts: vec![(text.into(), Token::Muted)], at });
            return vec![Effect::WritePageIds { visible: *page_ids }];
        }
        // This preference historically ignored cache failures: the session choice
        // still takes effect. The explicit result can be recorded/replayed.
        Msg::PageIdsPersisted { result: _ } => {}
        Msg::ToggleDocumentMode { at } => {
            let Some(on) = state.document_mode else { return vec![] };
            *on = !*on;
            let text = if *on { "document mode · Enter breaks the line, ⇧Enter starts a note" } else { "outline mode · Enter starts a note, ⇧Enter breaks the line" };
            *state.toast = Some(Toast { kind: ToastKind::Info, parts: vec![(text.into(), Token::Muted)], at });
            return vec![Effect::WriteDocumentMode { on: *on }];
        }
        // As page_ids: the session's choice stands if the device file can't be written.
        Msg::DocumentModePersisted { result: _ } => {}
        Msg::Copy { text, notice } => return vec![Effect::WriteClipboard { text, notice }],
        Msg::RemapKeys => return vec![Effect::EditKeys],
        Msg::DocClock { now_ms } => {
            let Some(d) = state.doc else { return vec![] };
            d.tick(now_ms);
            let n = d.ids_wanted();
            if n > 0 {
                return vec![Effect::MintIds { n }];
            }
        }
        Msg::IdsMinted { ids } => {
            if let Some(d) = state.doc {
                d.fill_ids(ids);
            }
        }
        Msg::Type { text } => {
            if let Some(d) = state.doc {
                d.insert(&text);
            }
        }
        Msg::Editor { command, at } => {
            use crate::editor::Outcome;
            let Some(d) = state.doc else { return vec![] };
            return match d.run_command(&command) {
                Outcome::Done => vec![],
                Outcome::Nothing(why) => {
                    *state.toast = Some(Toast { kind: ToastKind::Info, parts: vec![(why, Token::Muted)], at });
                    vec![]
                }
                // A task completed: save at once.
                Outcome::Completed => vec![Effect::SaveDoc],
                Outcome::Restored => vec![Effect::Reopen],
            };
        }
        Msg::KeysEdited { result, at } => {
            let (kind, token, text) = match result {
                Ok(text) => (ToastKind::Info, Token::Muted, text),
                Err(error) => (ToastKind::Error, Token::Overdue, error),
            };
            *state.toast = Some(Toast { kind, parts: vec![(text, token)], at });
        }
        Msg::ClipboardResult { result, notice, at } => {
            let (kind, token, text) = match result {
                Ok(()) => (ToastKind::Info, Token::Muted, notice),
                Err(error) => (ToastKind::Error, Token::Overdue, error),
            };
            *state.toast = Some(Toast { kind, parts: vec![(text, token)], at });
        }
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(scroll: &mut usize, toast: &mut Option<Toast>, msg: Msg) -> Vec<Effect> {
        update(Fields { page_ids: None, document_mode: None, cursor: 4, scroll, toast, doc: None }, msg)
    }

    #[test]
    fn wrapped_list_rows_follow_and_stale_metrics_do_not_move_scroll() {
        let mut scroll = 0;
        let mut toast = None;
        let message = Msg::ViewportPrepared(Viewport::List {
            cursor: 4,
            scroll: 0,
            height: 5,
            previous_section: false,
            heights: vec![1, 1, 4, 1, 2],
        });
        update(Fields { page_ids: None, document_mode: None, cursor: 4, scroll: &mut scroll, toast: &mut toast, doc: None }, message.clone());
        assert_eq!(scroll, 3);
        update(Fields { page_ids: None, document_mode: None, cursor: 4, scroll: &mut scroll, toast: &mut toast, doc: None }, message);
        assert_eq!(scroll, 3);
    }

    #[test]
    fn page_ids_change_before_persistence_and_a_cache_failure_keeps_the_choice() {
        let mut page_ids = false;
        let mut scroll = 0;
        let mut toast = None;
        let at = 1_000;
        let effects = update(
            Fields { page_ids: Some(&mut page_ids), document_mode: None, cursor: 0, scroll: &mut scroll, toast: &mut toast, doc: None },
            Msg::TogglePageIds { at },
        );
        assert!(page_ids);
        assert_eq!(effects, vec![Effect::WritePageIds { visible: true }]);
        update(
            Fields { page_ids: Some(&mut page_ids), document_mode: None, cursor: 0, scroll: &mut scroll, toast: &mut toast, doc: None },
            Msg::PageIdsPersisted { result: Err("cache unavailable".into()) },
        );
        assert!(page_ids);
        assert_eq!(toast.unwrap().parts, vec![("IDs shown · . to hide".into(), Token::Muted)]);
    }

    #[test]
    fn document_mode_toggles_in_the_state_and_persisting_is_its_effect() {
        let (mut on, mut scroll, mut toast) = (false, 0, None);
        let effects = update(Fields { page_ids: None, document_mode: Some(&mut on), cursor: 0, scroll: &mut scroll, toast: &mut toast, doc: None }, Msg::ToggleDocumentMode { at: 1 });
        assert!(on);
        assert_eq!(effects, vec![Effect::WriteDocumentMode { on: true }]);
        update(Fields { page_ids: None, document_mode: Some(&mut on), cursor: 0, scroll: &mut scroll, toast: &mut toast, doc: None }, Msg::DocumentModePersisted { result: Err("no cache".into()) });
        assert!(on, "a failed write keeps the session's choice");
        let effects = update(Fields { page_ids: None, document_mode: Some(&mut on), cursor: 0, scroll: &mut scroll, toast: &mut toast, doc: None }, Msg::ToggleDocumentMode { at: 2 });
        assert!(!on);
        assert_eq!(effects, vec![Effect::WriteDocumentMode { on: false }]);
    }

    #[test]
    fn remap_requests_only_emit_an_effect_until_the_editor_result_arrives() {
        let mut scroll = 0;
                let mut toast = None;
        assert_eq!(apply(&mut scroll, &mut toast, Msg::RemapKeys), vec![Effect::EditKeys]);
        assert!(toast.is_none());
        let at = 1_000;
        apply(&mut scroll, &mut toast, Msg::KeysEdited { result: Ok("keys ok · 1 remapped".into()), at });
        let success = toast.take().unwrap();
        assert_eq!((success.kind, success.parts, success.at), (ToastKind::Info, vec![("keys ok · 1 remapped".into(), Token::Muted)], at));
        apply(&mut scroll, &mut toast, Msg::KeysEdited { result: Err("line 2: invalid key".into()), at });
        assert_eq!(toast.unwrap().kind, ToastKind::Error);
    }

    #[test]
    fn clipboard_emits_one_value_effect_and_waits_for_an_explicit_result() {
        let mut scroll = 0;
                let mut toast = None;
        let effects = apply(&mut scroll, &mut toast, Msg::Copy { text: "bé🙂".into(), notice: "copied 3 chars".into() });
        assert_eq!(effects, vec![Effect::WriteClipboard { text: "bé🙂".into(), notice: "copied 3 chars".into() }]);
        assert!(toast.is_none());
        let at = 1_000; // supplied replay input; update itself never samples time
        apply(&mut scroll, &mut toast, Msg::ClipboardResult { result: Ok(()), notice: "copied 3 chars".into(), at });
        let success = toast.take().unwrap();
        assert_eq!((success.kind, success.parts, success.at), (ToastKind::Info, vec![("copied 3 chars".into(), Token::Muted)], at));
        apply(
            &mut scroll,
            &mut toast,
            Msg::ClipboardResult { result: Err("clipboard refused".into()), notice: "unused".into(), at },
        );
        assert_eq!(toast.unwrap().kind, ToastKind::Error);
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;
    use crate::editor::{Doc, Target};

    fn doc(texts: &[&str]) -> Doc {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
        let blocks: Vec<thc_core::outline::Block> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| serde_json::from_value(serde_json::json!({"id": format!("n{i}"), "parent": null, "depth": 0, "kind": "para", "text": t, "text_rev": "r"})).unwrap())
            .collect();
        Doc::new(Target::Journal { date: today }, Some("root".into()), &blocks, today)
    }

    fn send(d: &mut Doc, toast: &mut Option<Toast>, msg: Msg) -> Vec<Effect> {
        let mut scroll = 0;
        update(Fields { page_ids: None, document_mode: None, cursor: 0, scroll: &mut scroll, toast, doc: Some(d) }, msg)
    }

    /// The model mints no ids: the clock asks for them, the runtime mints, the ids come back.
    #[test]
    fn the_clock_asks_for_ids_and_minted_ids_fill_the_pool() {
        let mut d = doc(&["alpha"]);
        let mut toast = None;
        let n = match send(&mut d, &mut toast, Msg::DocClock { now_ms: 1_000 }).as_slice() {
            [Effect::MintIds { n }] => *n,
            other => panic!("{other:?}"),
        };
        send(&mut d, &mut toast, Msg::IdsMinted { ids: (0..n).map(|i| format!("id{i:010}")).collect() });
        assert!(send(&mut d, &mut toast, Msg::DocClock { now_ms: 2_000 }).is_empty(), "the pool is full");
    }

    /// Commands run on the document; a completed task saves, an undo re-reads the vault, and a
    /// command that does nothing says why.
    #[test]
    fn editor_commands_come_back_as_effects() {
        let mut d = doc(&["call the bank"]);
        let mut toast = None;
        let cmd = |c: &str| Msg::Editor { command: c.into(), at: 5 };
        assert_eq!(send(&mut d, &mut toast, cmd("history.undo")), vec![]);
        assert_eq!(toast.take().map(|t| (t.parts, t.at)), Some((vec![("nothing to undo".into(), Token::Muted)], 5)));
        assert_eq!(send(&mut d, &mut toast, cmd(crate::editor::TASK_CYCLE)), vec![]);
        assert_eq!(send(&mut d, &mut toast, cmd(crate::editor::TASK_CYCLE)), vec![Effect::SaveDoc], "done saves at once");
        assert_eq!(send(&mut d, &mut toast, cmd("history.undo")), vec![Effect::Reopen]);
        assert_eq!(send(&mut d, &mut toast, cmd("move.right")), vec![]);
        assert!(toast.is_none());
        send(&mut d, &mut toast, Msg::Type { text: "!".into() });
        assert_eq!(d.blocks()[0].text, "c!all the bank");
    }
}

#[cfg(test)]
mod action_tests {
    use super::*;
    use crate::app::{Row, View};
    use crate::ui_state::UiState;

    fn rows() -> Vec<Row> {
        vec![Row::Section { title: "Today".into(), count: None, token: Token::Muted, note: None }, Row::Tag { id: "t1".into(), name: "a".into(), count: 1 }, Row::Blank, Row::Tag { id: "t2".into(), name: "b".into(), count: 2 }]
    }

    #[test]
    fn navigation_changes_only_the_state_and_asks_for_a_reload() {
        let rows = rows();
        let facts = Facts { rows: &rows, doc_open: false, mouse: true, half_page: 5 };
        let mut ui = UiState::default();
        ui.prompt = Some((crate::app::PromptKind::Search, crate::input::LineInput::default()));
        ui.view = View::Search;
        assert_eq!(action(&mut ui, &facts, "go.tasks"), Some(vec![Effect::Reload]));
        assert_eq!((ui.view, ui.prompt.is_none()), (View::Tasks, true), "a finder's prompt is put away on leaving");
        assert_eq!(action(&mut ui, &facts, "cursor.down"), Some(vec![]));
        assert_eq!((ui.cursor, ui.selected.as_deref()), (1, Some("tag:a")));
        action(&mut ui, &facts, "cursor.down");
        assert_eq!((ui.cursor, ui.selected.as_deref()), (3, Some("tag:b")), "skips what can't be selected");
        action(&mut ui, &facts, "cursor.top");
        assert_eq!(ui.cursor, 1);
        assert_eq!(action(&mut ui, &facts, "quit"), Some(vec![Effect::SaveDoc, Effect::Quit]));
        assert_eq!(action(&mut ui, &facts, "mouse.toggle"), Some(vec![Effect::SetMouse { on: false }]));
        assert_eq!(action(&mut ui, &facts, "help.context"), Some(vec![]));
        assert!(matches!(ui.overlay, Some(crate::app::Overlay::Recipe { .. })));
        ui.now_ms = 77;
        ui.view = View::Today;
        action(&mut ui, &facts, "today.by_vault");
        assert_eq!(ui.toast.as_ref().map(|t| t.at), Some(77), "toasts carry the logical clock");
        assert_eq!(action(&mut ui, &facts, "node.done"), None, "writes still run on App");
        // The same state and action give the same result: nothing else is read.
        let (mut a, mut b) = (ui.clone(), ui.clone());
        for act in ["day.next", "week.prev", "view.next", "pane.next", "focus.toggle", "tasks.filter"] {
            assert_eq!(action(&mut a, &facts, act), action(&mut b, &facts, act));
        }
        assert_eq!(a, b);
    }
}
