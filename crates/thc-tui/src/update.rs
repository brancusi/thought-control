//! Pure presentation updates. Fields exposes only presentation values, so this
//! reducer cannot reach a Vault, terminal, channel, clock, or filesystem.
//! This is the first P3 migration; legacy input/persistence paths remain outside it.
use crate::{
    app::{Toast, ToastKind},
    editor::{BlockPos, Target},
    theme::Token,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DocumentIdentity {
    pub vault: std::path::PathBuf,
    pub target: Target,
    pub revision: u64,
    pub caret: BlockPos,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Viewport {
    Document { identity: DocumentIdentity, rows: usize, caret: Option<usize>, height: usize, free: bool, typewriter: bool },
    List { cursor: usize, scroll: usize, height: usize, previous_section: bool, heights: Vec<usize> },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Msg {
    ViewportPrepared(Viewport),
    TogglePageIds { at: u64 },
    PageIdsPersisted { result: Result<(), String> },
    Copy { text: String, notice: String },
    ClipboardResult { result: Result<(), String>, notice: String, at: u64 },
    RemapKeys,
    KeysEdited { result: Result<String, String>, at: u64 },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    WritePageIds { visible: bool },
    WriteClipboard { text: String, notice: String },
    EditKeys,
    /// Rebuild the rows (and open, switch or close the document) from the store.
    Reload,
    /// Save the open document's typing.
    SaveDoc,
    Quit,
    SetMouse { on: bool },
    /// `$EDITOR` on a note.
    SpawnEditor { target: String },
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

pub(crate) struct DocumentFields<'a> {
    pub identity: DocumentIdentity,
    pub scroll: &'a mut usize,
}
pub(crate) struct Fields<'a> {
    pub page_ids: Option<&'a mut bool>,
    pub cursor: usize,
    pub scroll: &'a mut usize,
    pub document: Option<DocumentFields<'a>>,
    pub toast: &'a mut Option<Toast>,
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
        Msg::ViewportPrepared(Viewport::Document { identity, rows, caret, height, free, typewriter }) => {
            let Some(document) = state.document.filter(|d| d.identity == identity) else { return vec![] };
            let scroll = document.scroll;
            if free {
                *scroll = (*scroll).min(rows.saturating_sub(1));
            } else if let Some(caret) = caret {
                if typewriter {
                    *scroll = caret.saturating_sub(height * 45 / 100);
                } else {
                    let context = 2.min(height.saturating_sub(1) / 2);
                    if caret < scroll.saturating_add(context) {
                        *scroll = caret.saturating_sub(context);
                    } else if caret.saturating_add(context) >= scroll.saturating_add(height) {
                        *scroll = caret
                            .saturating_add(context + 1)
                            .saturating_sub(height)
                            .min(rows.saturating_sub(height))
                            .max(caret.saturating_add(1).saturating_sub(height));
                    }
                }
            }
        }
        Msg::ViewportPrepared(Viewport::List { cursor, scroll, height, previous_section, heights }) => {
            if state.document.is_some() || state.cursor != cursor || *state.scroll != scroll {
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
        Msg::Copy { text, notice } => return vec![Effect::WriteClipboard { text, notice }],
        Msg::RemapKeys => return vec![Effect::EditKeys],
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

    fn identity() -> DocumentIdentity {
        DocumentIdentity {
            vault: "/scratch/one".into(),
            target: Target::Journal { date: chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap() },
            revision: 7,
            caret: BlockPos { line: 110, byte: 0 },
        }
    }
    fn apply(scroll: &mut usize, doc_scroll: &mut usize, toast: &mut Option<Toast>, msg: Msg) -> Vec<Effect> {
        update(
            Fields {
                page_ids: None,
                cursor: 4,
                scroll,
                document: Some(DocumentFields { identity: identity(), scroll: doc_scroll }),
                toast,
            },
            msg,
        )
    }

    #[test]
    fn document_follow_replays_and_rejects_another_vault_or_revision() {
        let mut left = (0, 0, None);
        let mut right = (0, 0, None);
        let message = Msg::ViewportPrepared(Viewport::Document {
            identity: identity(),
            rows: 200,
            caret: Some(110),
            height: 40,
            free: false,
            typewriter: true,
        });
        assert!(apply(&mut left.0, &mut left.1, &mut left.2, message.clone()).is_empty());
        assert!(apply(&mut right.0, &mut right.1, &mut right.2, message).is_empty());
        assert_eq!((left.0, left.1), (right.0, right.1));
        assert_eq!(left.1, 92);
        for other in [DocumentIdentity { vault: "/scratch/two".into(), ..identity() }, DocumentIdentity { revision: 8, ..identity() }] {
            let message = Msg::ViewportPrepared(Viewport::Document {
                identity: other,
                rows: 200,
                caret: Some(190),
                height: 40,
                free: false,
                typewriter: false,
            });
            apply(&mut left.0, &mut left.1, &mut left.2, message);
            assert_eq!(left.1, 92);
        }
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
        update(Fields { page_ids: None, cursor: 4, scroll: &mut scroll, document: None, toast: &mut toast }, message.clone());
        assert_eq!(scroll, 3);
        update(Fields { page_ids: None, cursor: 4, scroll: &mut scroll, document: None, toast: &mut toast }, message);
        assert_eq!(scroll, 3);
    }

    #[test]
    fn page_ids_change_before_persistence_and_a_cache_failure_keeps_the_choice() {
        let mut page_ids = false;
        let mut scroll = 0;
        let mut toast = None;
        let at = 1_000;
        let effects = update(
            Fields { page_ids: Some(&mut page_ids), cursor: 0, scroll: &mut scroll, document: None, toast: &mut toast },
            Msg::TogglePageIds { at },
        );
        assert!(page_ids);
        assert_eq!(effects, vec![Effect::WritePageIds { visible: true }]);
        update(
            Fields { page_ids: Some(&mut page_ids), cursor: 0, scroll: &mut scroll, document: None, toast: &mut toast },
            Msg::PageIdsPersisted { result: Err("cache unavailable".into()) },
        );
        assert!(page_ids);
        assert_eq!(toast.unwrap().parts, vec![("IDs shown · . to hide".into(), Token::Muted)]);
    }

    #[test]
    fn remap_requests_only_emit_an_effect_until_the_editor_result_arrives() {
        let mut scroll = 0;
        let mut doc_scroll = 0;
        let mut toast = None;
        assert_eq!(apply(&mut scroll, &mut doc_scroll, &mut toast, Msg::RemapKeys), vec![Effect::EditKeys]);
        assert!(toast.is_none());
        let at = 1_000;
        apply(&mut scroll, &mut doc_scroll, &mut toast, Msg::KeysEdited { result: Ok("keys ok · 1 remapped".into()), at });
        let success = toast.take().unwrap();
        assert_eq!((success.kind, success.parts, success.at), (ToastKind::Info, vec![("keys ok · 1 remapped".into(), Token::Muted)], at));
        apply(&mut scroll, &mut doc_scroll, &mut toast, Msg::KeysEdited { result: Err("line 2: invalid key".into()), at });
        assert_eq!(toast.unwrap().kind, ToastKind::Error);
    }

    #[test]
    fn clipboard_emits_one_value_effect_and_waits_for_an_explicit_result() {
        let mut scroll = 0;
        let mut doc_scroll = 0;
        let mut toast = None;
        let effects = apply(&mut scroll, &mut doc_scroll, &mut toast, Msg::Copy { text: "bé🙂".into(), notice: "copied 3 chars".into() });
        assert_eq!(effects, vec![Effect::WriteClipboard { text: "bé🙂".into(), notice: "copied 3 chars".into() }]);
        assert!(toast.is_none());
        let at = 1_000; // supplied replay input; update itself never samples time
        apply(&mut scroll, &mut doc_scroll, &mut toast, Msg::ClipboardResult { result: Ok(()), notice: "copied 3 chars".into(), at });
        let success = toast.take().unwrap();
        assert_eq!((success.kind, success.parts, success.at), (ToastKind::Info, vec![("copied 3 chars".into(), Token::Muted)], at));
        apply(
            &mut scroll,
            &mut doc_scroll,
            &mut toast,
            Msg::ClipboardResult { result: Err("clipboard refused".into()), notice: "unused".into(), at },
        );
        assert_eq!(toast.unwrap().kind, ToastKind::Error);
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
