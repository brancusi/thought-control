//! One declarative keymap (docs/design/keymap.md §0, §2, §12): every key outside the overlays is a
//! row of `DEFAULTS`, matched exactly, looked up through the active context stack, and run by
//! action ID. The footer, help, the palette and `thc keys` read the same rows.

use crate::app::{App, Row, View};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key as the table holds it: a code and the ⌃ ⌥ ⌘ (and, for named keys, ⇧) modifiers.
/// Letters carry ⇧ in their case (`X`), so `x` never matches ⇧X and `X` never matches ⌃X.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

const MODS: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::ALT).union(KeyModifiers::SUPER).union(KeyModifiers::SHIFT);

impl Key {
    /// The event as the table compares it (exact matching, §1.3).
    pub fn of(k: &KeyEvent) -> Key {
        let mut mods = k.modifiers & MODS;
        let mut code = k.code;
        if code == KeyCode::BackTab {
            code = KeyCode::Tab;
            mods |= KeyModifiers::SHIFT;
        }
        if let KeyCode::Char(c) = code {
            // ⇧ lives in the character: ⌘⇧z arrives as `z` + ⇧ or as `Z`; both are `Cmd-Z`.
            if mods.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase() {
                code = KeyCode::Char(c.to_ascii_uppercase());
            }
            mods.remove(KeyModifiers::SHIFT);
        }
        Key { code, mods }
    }

    /// One key in the notation of §8.2: `x`, `X`, `space`, `C-t`, `A-up`, `S-tab`, `Cmd-c`.
    pub fn parse(s: &str) -> Option<Key> {
        let mut mods = KeyModifiers::NONE;
        let mut rest = s;
        loop {
            let lower = rest.to_ascii_lowercase();
            let (m, n) = if lower.starts_with("c-") && rest.len() > 2 {
                (KeyModifiers::CONTROL, 2)
            } else if lower.starts_with("ctrl-") && rest.len() > 5 {
                (KeyModifiers::CONTROL, 5)
            } else if lower.starts_with("a-") && rest.len() > 2 {
                (KeyModifiers::ALT, 2)
            } else if lower.starts_with("alt-") && rest.len() > 4 {
                (KeyModifiers::ALT, 4)
            } else if lower.starts_with("s-") && rest.len() > 2 {
                (KeyModifiers::SHIFT, 2)
            } else if lower.starts_with("shift-") && rest.len() > 6 {
                (KeyModifiers::SHIFT, 6)
            } else if lower.starts_with("cmd-") && rest.len() > 4 {
                (KeyModifiers::SUPER, 4)
            } else {
                break;
            };
            mods |= m;
            rest = &rest[n..];
        }
        let code = match rest.to_ascii_lowercase().as_str() {
            "space" => KeyCode::Char(' '),
            "enter" | "ret" => KeyCode::Enter,
            "esc" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            f if f.len() >= 2 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => KeyCode::F(f[1..].parse().ok()?),
            _ if rest.chars().count() == 1 => KeyCode::Char(rest.chars().next()?),
            _ => return None,
        };
        let mut k = Key { code, mods };
        if let KeyCode::Char(c) = k.code {
            if k.mods.contains(KeyModifiers::SHIFT) {
                k.code = KeyCode::Char(c.to_ascii_uppercase());
                k.mods.remove(KeyModifiers::SHIFT);
            }
        }
        Some(k)
    }

    /// How hints show it: `⌃T`, `⌥↑`, `⇧Tab`, `⌘C`, `F1`, `space`, `x`.
    pub fn display(&self) -> String {
        let mut s = String::new();
        let chord = !self.mods.is_empty();
        // A chord on a capital is ⇧ too (`Cmd-Z` is ⌘⇧Z, not ⌘Z).
        let shift = self.mods.contains(KeyModifiers::SHIFT) || (chord && matches!(self.code, KeyCode::Char(c) if c.is_ascii_uppercase()));
        for (on, g) in [(self.mods.contains(KeyModifiers::CONTROL), "⌃"), (self.mods.contains(KeyModifiers::ALT), "⌥"), (shift, "⇧"), (self.mods.contains(KeyModifiers::SUPER), "⌘")] {
            if on {
                s.push_str(g);
            }
        }
        s.push_str(&match self.code {
            KeyCode::Char(' ') => "space".into(),
            KeyCode::Char(c) if chord => c.to_uppercase().to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "Enter".into(),
            KeyCode::Esc => "Esc".into(),
            KeyCode::Tab => "Tab".into(),
            KeyCode::Backspace => "⌫".into(),
            KeyCode::Delete => "Del".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::Home => "Home".into(),
            KeyCode::End => "End".into(),
            KeyCode::PageUp => "PgUp".into(),
            KeyCode::PageDown => "PgDn".into(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        });
        s
    }
}

impl Key {
    /// The key in the notation `parse` reads (`x`, `X`, `space`, `C-t`, `A-up`, `S-tab`,
    /// `Cmd-c`, `f1`): `Key::parse(&k.notation()) == Some(k)`.
    pub fn notation(&self) -> String {
        let mut s = String::new();
        for (m, p) in [(KeyModifiers::CONTROL, "C-"), (KeyModifiers::ALT, "A-"), (KeyModifiers::SHIFT, "S-"), (KeyModifiers::SUPER, "Cmd-")] {
            if self.mods.contains(m) {
                s.push_str(p);
            }
        }
        s.push_str(&match self.code {
            KeyCode::Char(' ') => "space".into(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "enter".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Delete => "delete".into(),
            KeyCode::Up => "up".into(),
            KeyCode::Down => "down".into(),
            KeyCode::Left => "left".into(),
            KeyCode::Right => "right".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pageup".into(),
            KeyCode::PageDown => "pagedown".into(),
            KeyCode::F(n) => format!("f{n}"),
            other => format!("{other:?}").to_lowercase(),
        });
        s
    }
}

/// A key sequence (`g g`, `p h`) or a single key.
pub fn parse_seq(s: &str) -> Option<Vec<Key>> {
    s.split(' ').filter(|t| !t.is_empty()).map(Key::parse).collect()
}

pub fn display_seq(keys: &[Key]) -> String {
    keys.iter().map(Key::display).collect::<Vec<_>>().join(" ")
}

/// A named layer of bindings (§2.1). `Write` and the overlays are sealed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ctx {
    Global,
    List,
    Today,
    Inbox,
    Tasks,
    Pages,
    Journal,
    Search,
    Log,
    ToastAlert,
    ToastAgent,
    ToastConfirm,
    Write,
    /// The keyboard is in the sidebar (sidebar.md §5.2): chords only, on top of the panel's
    /// own keys.
    Sidebar,
    // Overlays, prompts and the link popup: sealed, with their own handlers; their rows here
    // give the footer, help and `thc keys` (§12.6).
    Link,
    Prompt,
    Palette,
    Finder,
    Vaults,
    Capture,
    Move,
    Compare,
    Focus,
    Help,
    Notes,
}

// `name`, `ALL`, `When::name`, `group` and `footer` are read by the generated footer, help and
// `thc keys` (keymap.md §0 steps 2–3).
#[allow(dead_code)]
impl Ctx {
    pub fn name(self) -> &'static str {
        match self {
            Ctx::Global => "global",
            Ctx::List => "list",
            Ctx::Today => "today",
            Ctx::Inbox => "inbox",
            Ctx::Tasks => "tasks",
            Ctx::Pages => "pages",
            Ctx::Journal => "journal",
            Ctx::Search => "search",
            Ctx::Log => "log",
            Ctx::ToastAlert => "toast.alert",
            Ctx::ToastAgent => "toast.agent",
            Ctx::ToastConfirm => "toast.confirm",
            Ctx::Write => "write",
            Ctx::Sidebar => "sidebar",
            Ctx::Link => "link",
            Ctx::Prompt => "prompt",
            Ctx::Palette => "palette",
            Ctx::Finder => "finder",
            Ctx::Vaults => "vaults",
            Ctx::Capture => "capture",
            Ctx::Move => "move",
            Ctx::Compare => "compare",
            Ctx::Focus => "focus",
            Ctx::Help => "help",
            Ctx::Notes => "notes",
        }
    }

    pub const ALL: [Ctx; 25] = [
        Ctx::Global,
        Ctx::List,
        Ctx::Today,
        Ctx::Inbox,
        Ctx::Tasks,
        Ctx::Pages,
        Ctx::Journal,
        Ctx::Search,
        Ctx::Log,
        Ctx::ToastAlert,
        Ctx::ToastAgent,
        Ctx::ToastConfirm,
        Ctx::Write,
        Ctx::Sidebar,
        Ctx::Link,
        Ctx::Prompt,
        Ctx::Palette,
        Ctx::Finder,
        Ctx::Vaults,
        Ctx::Capture,
        Ctx::Move,
        Ctx::Compare,
        Ctx::Focus,
        Ctx::Help,
        Ctx::Notes,
    ];

    fn of_view(v: View) -> Ctx {
        match v {
            View::Today => Ctx::Today,
            View::Inbox => Ctx::Inbox,
            View::Tasks => Ctx::Tasks,
            View::Pages => Ctx::Pages,
            View::Journal => Ctx::Journal,
            View::Search => Ctx::Search,
            View::Log => Ctx::Log,
        }
    }
}

/// When a binding applies (§2): checked against the selected row and the app's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    Always,
    HasNode,
    NodeIsTask,
    NodeHasAlert,
    NodeHasConflict,
    NodeRepeats,
    /// The Log's review lane is on (and it isn't one node's history).
    LaneOn,
    /// The Log as a whole, not one node's history.
    LogAll,
    /// One node's history in the Log.
    NodeLog,
    /// The whole Log with the review lane off.
    LogBrowse,
    /// Something to go back from (an open page, a filter, a node's history); else `q` quits.
    CanBack,
    /// The selected row has children to fold or unfold.
    Foldable,
    /// The Pages index (no page open).
    PagesIndex,
    /// The open document is a journal day (⌃P ⌃N go by day; a page has no days).
    DocIsJournal,
    /// The compare overlay shows a rejected move (Enter ok), not a text conflict.
    CompareMove,
    CompareText,
    CompareRehomed,
    ViewToday,
    ViewTasks,
    /// An open sync conflict somewhere (`space c`).
    AnyConflict,
    /// Agent changes waiting for review (`space r`).
    ToReview,
    /// A newer thc is out (`space u`).
    UpdateAvailable,
    /// Something to open beside (sidebar.md §5.3): the caret in a link, a row with a page or day.
    HasTarget,
    /// `o` opens aside: a row with a page or day, outside Pages and Search (they type `o`).
    RowAside,
    SidebarHasPanels,
    SidebarShown,
    SidebarClosedAny,
    /// The active panel is a page or a day.
    PanelIsDoc,
    /// The sidebar is the drawer or replace (narrower than a column): Esc closes it.
    SidebarOver,
}

impl When {
    #[allow(dead_code)]
    pub fn name(self) -> &'static str {
        match self {
            When::Always => "",
            When::HasNode => "has_node",
            When::NodeIsTask => "node_is_task",
            When::NodeHasAlert => "node_has_alert",
            When::NodeHasConflict => "node_has_conflict",
            When::NodeRepeats => "node_repeats",
            When::LaneOn => "lane_on",
            When::LogAll => "log_all",
            When::NodeLog => "node_log",
            When::LogBrowse => "log_browse",
            When::CanBack => "can_back",
            When::Foldable => "foldable",
            When::PagesIndex => "pages_index",
            When::DocIsJournal => "doc_is_journal",
            When::CompareMove => "compare_move",
            When::CompareText => "compare_text",
            When::CompareRehomed => "compare_rehomed",
            When::ViewToday => "view_today",
            When::ViewTasks => "view_tasks",
            When::AnyConflict => "any_conflict",
            When::ToReview => "to_review",
            When::UpdateAvailable => "update_available",
            When::HasTarget => "has_target",
            When::RowAside => "row_aside",
            When::SidebarHasPanels => "sidebar_has_panels",
            When::SidebarShown => "sidebar_shown",
            When::SidebarClosedAny => "sidebar_closed_any",
            When::PanelIsDoc => "panel_is_doc",
            When::SidebarOver => "sidebar_over",
        }
    }

    /// For help: a row's state (a task, children, an alert) is taken as given, since help
    /// describes keys, not this row; the app's state (Pages, the lane, a filter) still decides.
    pub fn holds_for_help(self, app: &App) -> bool {
        matches!(self, When::HasNode | When::NodeIsTask | When::NodeHasAlert | When::NodeHasConflict | When::NodeRepeats | When::Foldable | When::HasTarget | When::RowAside | When::SidebarHasPanels | When::SidebarShown | When::SidebarClosedAny | When::PanelIsDoc | When::SidebarOver) || self.holds(app)
    }

    pub fn holds(self, app: &App) -> bool {
        let node = || app.selected_node();
        match self {
            When::Always => true,
            When::HasNode => node().is_some(),
            When::NodeIsTask => node().is_some_and(|n| n.status.is_some()),
            When::NodeHasAlert => node().is_some_and(|n| app.vault.store.alerts_of(&n.id).is_ok_and(|a| !a.is_empty())),
            When::NodeHasConflict => node().is_some_and(|n| app.vault.store.conflict_details(Some(&n.id)).is_ok_and(|c| !c.is_empty())),
            When::NodeRepeats => node().is_some_and(|n| n.repeat.is_some()),
            When::LaneOn => app.review_lane && app.log_node.is_none(),
            When::LogAll => app.log_node.is_none(),
            When::NodeLog => app.log_node.is_some(),
            When::LogBrowse => app.log_node.is_none() && !app.review_lane,
            When::CanBack => app.can_back(),
            When::Foldable => app.rows.get(app.cursor).is_some_and(|r| matches!(r, Row::Node { has_children: true, .. })),
            When::PagesIndex => app.view == View::Pages && app.page_open.is_none(),
            When::DocIsJournal => app.doc.as_ref().is_some_and(|d| matches!(d.target, crate::editor::Target::Journal { .. })),
            When::CompareMove => matches!(&app.overlay, Some(crate::app::Overlay::Compare { detail }) if detail.kind == "move"),
            When::CompareText => matches!(&app.overlay, Some(crate::app::Overlay::Compare { detail }) if detail.kind == "text"),
            When::CompareRehomed => matches!(&app.overlay, Some(crate::app::Overlay::Compare { detail }) if detail.kind == "rehomed"),
            When::ViewToday => app.view == View::Today,
            When::ViewTasks => app.view == View::Tasks,
            When::AnyConflict => !app.conflicts.is_empty(),
            When::ToReview => app.to_review > 0,
            When::UpdateAvailable => app.update_available.is_some() || app.installed.is_some(),
            When::HasTarget => app.aside_target().is_some(),
            When::RowAside => app.doc.is_none() && !matches!(app.view, View::Pages | View::Search) && app.aside_target().is_some(),
            When::SidebarHasPanels => app.ui.sidebar.has_panels(),
            When::SidebarShown => app.ui.sidebar.has_panels() && app.ui.sidebar.shown,
            When::SidebarClosedAny => !app.ui.sidebar.closed.is_empty(),
            When::PanelIsDoc => app.ui.sidebar.active_key().is_some_and(|k| k.kind.is_doc()),
            When::SidebarOver => app.sidebar_over.is_some(),
        }
    }
}

/// One row of the keymap (§2).
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Binding {
    pub remapped: bool,
    pub ctx: Ctx,
    pub keys: &'static str,
    pub action: &'static str,
    pub when: When,
    pub label: &'static str,
    pub group: &'static str,
    /// Footer rank, 1 first (0: a toast's keys, ahead of everything), or none.
    pub footer: Option<u8>,
}

macro_rules! b {
    ($ctx:ident, $keys:expr, $action:expr, $when:ident, $label:expr, $group:expr) => {
        Binding { remapped: false, ctx: Ctx::$ctx, keys: $keys, action: $action, when: When::$when, label: $label, group: $group, footer: None }
    };
    ($ctx:ident, $keys:expr, $action:expr, $when:ident, $label:expr, $group:expr, $rank:expr) => {
        Binding { remapped: false, ctx: Ctx::$ctx, keys: $keys, action: $action, when: When::$when, label: $label, group: $group, footer: Some($rank) }
    };
}

/// The default table: keymap.md §12 with §0's changes (documents are modeless and sealed; no
/// Navigate, no line select, no footer cursor; outline-row editing is gone with outline rows).
/// The first row that matches and holds wins, top context first.
pub fn defaults() -> Vec<Binding> {
    let mut t = vec![
        // ---- global (§12.1)
        b!(Global, "*", "view.scope", Always, "scope", "View"),
        b!(Global, "Cmd-[", "nav.back", Always, "back", "Go"),
        b!(Global, "Cmd-]", "nav.forward", Always, "forward", "Go"),
        b!(Global, "C-A-left", "nav.back", Always, "back", "Go"),
        b!(Global, "C-A-right", "nav.forward", Always, "forward", "Go"),
        b!(Global, "1", "go.today", Always, "Today", "View"),
        b!(Global, "2", "go.inbox", Always, "Inbox", "View"),
        b!(Global, "3", "go.tasks", Always, "Tasks", "View"),
        b!(Global, "4", "go.pages", Always, "Pages", "View"),
        b!(Global, "5", "go.journal", Always, "Journal", "View"),
        b!(Global, "6", "go.search", Always, "Search", "View"),
        b!(Global, "7", "go.log", Always, "Log", "View"),
        b!(Global, "tab", "view.next", Always, "next view", "View"),
        b!(Global, "S-tab", "view.prev", Always, "previous view", "View"),
        b!(Global, "?", "help.context", Always, "keys", "View", 9),
        b!(Global, "f1", "help.context", Always, "keys", "View"),
        b!(Global, ":", "palette.open", Always, "commands", "View"),
        b!(Global, "C-o", "finder.open", Always, "open", "Find"),
        b!(Global, "/", "pages.filter", PagesIndex, "filter pages", "Find"),
        b!(Global, "/", "search.find", Always, "search", "Find"),
        b!(Global, "f", "tasks.filter", Always, "filter", "Find"),
        b!(Global, "\\", "pane.detail_toggle", Always, "detail", "View"),
        b!(Global, "C-w", "pane.next", Always, "next pane", "View"),
        b!(Global, "C", "context.toggle", Always, "context", "View"),
        b!(Global, "T", "go.journal_today", Always, "today's journal", "Time"),
        b!(Global, "g d", "go.date", Always, "go to date", "Time"),
        b!(Global, "u", "undo", Always, "undo", "Change"),
        b!(Global, "U", "undo", Always, "undo", "Change"),
        b!(Global, "q", "back", CanBack, "back", "View"),
        b!(Global, "q", "quit", Always, "quit", "View"),
        b!(Global, "C-c", "quit", Always, "quit", "View"),
        b!(Global, "C-q", "quit", Always, "quit", "View"),
        b!(Global, "C-l", "redraw", Always, "", ""),
        // ---- the sidebar from anywhere (sidebar.md §5.3): also in `write`, which is sealed.
        b!(Global, "A-s", "sidebar.focus", SidebarHasPanels, "sidebar", "Sidebar"),
        b!(Global, "A-T", "sidebar.reopen", SidebarClosedAny, "reopen", "Sidebar"),
        b!(Global, "A-\\", "sidebar.toggle", SidebarHasPanels, "hide · show", "Sidebar"),
        b!(Global, "A-=", "sidebar.wider", SidebarShown, "width", "Sidebar"),
        b!(Global, "A--", "sidebar.narrower", SidebarShown, "width", "Sidebar"),
        b!(Global, "A-0", "sidebar.width_auto", SidebarShown, "width", "Sidebar"),
        // ---- the leader (§5.2, less `space s` and `space y`: §0 step 4). Lists and views only.
        b!(Global, "space space", "palette.open", Always, "commands", "Leader"),
        b!(Global, "space f p", "finder.open", Always, "page or day", "Leader"),
        b!(Global, "space f d", "go.date", Always, "day", "Leader"),
        b!(Global, "space f t", "find.tag", Always, "tag", "Leader"),
        b!(Global, "space f s", "search.find", Always, "search everything", "Leader"),
        b!(Global, "space g t", "go.today", Always, "Today", "Leader"),
        b!(Global, "space g i", "go.inbox", Always, "Inbox", "Leader"),
        b!(Global, "space g k", "go.tasks", Always, "Tasks", "Leader"),
        b!(Global, "space g p", "go.pages", Always, "Pages", "Leader"),
        b!(Global, "space g v", "vault.picker", Always, "Vaults", "Leader"),
        b!(Global, "space g h", "nav.history", Always, "history", "Leader"),
        b!(Global, "space g j", "go.journal", Always, "Journal", "Leader"),
        b!(Global, "space g s", "go.search", Always, "Search", "Leader"),
        b!(Global, "space g l", "go.log", Always, "Log", "Leader"),
        b!(Global, "space g d", "go.date", Always, "a date", "Leader"),
        b!(Global, "space g g", "go.journal_today", Always, "today's journal", "Leader"),
        b!(Global, "space n p", "page.new", Always, "page", "Leader"),
        b!(Global, "space n c", "capture.here", Always, "capture here", "Leader"),
        b!(Global, "space n i", "capture.inbox", Always, "to inbox", "Leader"),
        b!(Global, "space t f", "focus.toggle", Always, "focus", "Leader"),
        b!(Global, "space t F", "focus.overlay", Always, "focus elements", "Leader"),
        b!(Global, "space t c", "context.toggle", Always, "context", "Leader"),
        b!(Global, "space t d", "pane.detail_toggle", Always, "detail pane", "Leader"),
        b!(Global, "space t i", "ids.toggle", Always, "ids", "Leader"),
        b!(Global, "space t s", "tasks.sort_cycle", ViewTasks, "sort", "Leader"),
        b!(Global, "space t w", "today.agenda_toggle", ViewToday, "agenda", "Leader"),
        b!(Global, "space t v", "today.by_vault", ViewToday, "by vault", "Leader"),
        b!(Global, "space v 1", "view.slot.1", Always, "view 1", "Leader"),
        b!(Global, "space v 2", "view.slot.2", Always, "view 2", "Leader"),
        b!(Global, "space v 3", "view.slot.3", Always, "view 3", "Leader"),
        b!(Global, "space v 4", "view.slot.4", Always, "view 4", "Leader"),
        b!(Global, "space v 5", "view.slot.5", Always, "view 5", "Leader"),
        b!(Global, "space v 6", "view.slot.6", Always, "view 6", "Leader"),
        b!(Global, "space v 7", "view.slot.7", Always, "view 7", "Leader"),
        b!(Global, "space v 8", "view.slot.8", Always, "view 8", "Leader"),
        b!(Global, "space v 9", "view.slot.9", Always, "view 9", "Leader"),
        b!(Global, "space v s", "view.save", Always, "save this filter", "Leader"),
        b!(Global, "space v e", "views.edit", Always, "edit views", "Leader"),
        b!(Global, "space c", "node.compare", AnyConflict, "compare", "Leader"),
        b!(Global, "space r", "review.lane_open", ToReview, "review lane", "Leader"),
        b!(Global, "space u", "update", UpdateAvailable, "update thc", "Leader"),
        b!(Global, "space e", "node.edit_external", Always, "$EDITOR", "Leader"),
        b!(Global, "space E", "doc.edit_external", Always, "$EDITOR page", "Leader"),
        b!(Global, "space q", "quit", Always, "quit", "Leader"),
        b!(Global, "space ?", "help.all", Always, "every key", "Leader"),
        b!(Global, "space a", "about", Always, "about · what's new", "Leader"),
        b!(Global, "space w w", "sidebar.focus", Always, "focus", "Leader"),
        b!(Global, "space w o", "sidebar.open_aside", Always, "aside", "Leader"),
        b!(Global, "space w x", "sidebar.close", Always, "close", "Leader"),
        b!(Global, "space w X", "sidebar.close_all", Always, "close all", "Leader"),
        b!(Global, "space w p", "sidebar.pin", Always, "pin", "Leader"),
        b!(Global, "space w c", "sidebar.fold", Always, "fold", "Leader"),
        b!(Global, "space w m", "sidebar.to_main", Always, "to main", "Leader"),
        b!(Global, "space w h", "sidebar.toggle", Always, "hide · show", "Leader"),
        b!(Global, "space w r", "sidebar.reopen", Always, "reopen", "Leader"),
        b!(Global, "space w =", "sidebar.wider", Always, "wider", "Leader"),
        b!(Global, "space w -", "sidebar.narrower", Always, "narrower", "Leader"),
        b!(Global, "space w 0", "sidebar.width_auto", Always, "auto width", "Leader"),
        // ---- list (§12.2)
        b!(List, "S-v", "vault.picker", Always, "vaults", "View"),
        b!(List, "j", "cursor.down", Always, "down", "Move"),
        b!(List, "down", "cursor.down", Always, "down", "Move"),
        b!(List, "k", "cursor.up", Always, "up", "Move"),
        b!(List, "up", "cursor.up", Always, "up", "Move"),
        b!(List, "g g", "cursor.top", Always, "top", "Move"),
        b!(List, "home", "cursor.top", Always, "top", "Move"),
        b!(List, "G", "cursor.bottom", Always, "bottom", "Move"),
        b!(List, "end", "cursor.bottom", Always, "bottom", "Move"),
        b!(List, "C-d", "cursor.half_down", Always, "half page down", "Move"),
        b!(List, "C-u", "cursor.half_up", Always, "half page up", "Move"),
        b!(List, "pagedown", "cursor.page_down", Always, "page down", "Move"),
        b!(List, "pageup", "cursor.page_up", Always, "page up", "Move"),
        b!(List, "enter", "open", Always, "open", "Move", 4),
        b!(List, "esc", "back", Always, "back", "Move"),
        b!(List, "h", "fold.close", Foldable, "fold", "Outline"),
        b!(List, "left", "fold.close", Foldable, "fold", "Outline"),
        b!(List, "h", "back", Always, "back", "Move"),
        b!(List, "left", "back", Always, "back", "Move"),
        b!(List, "l", "fold.open", Foldable, "unfold", "Outline"),
        b!(List, "right", "fold.open", Foldable, "unfold", "Outline"),
        b!(List, "right", "open", PagesIndex, "open", "Move"),
        b!(List, "x", "node.done", NodeIsTask, "done", "Change", 1),
        b!(List, "X", "node.reopen", NodeIsTask, "reopen", "Change"),
        b!(List, "t", "node.task_toggle", HasNode, "task", "Change", 3),
        b!(List, "S space", "node.status.todo", HasNode, "todo", "Change"),
        b!(List, "S /", "node.status.doing", HasNode, "doing", "Change"),
        b!(List, "S w", "node.status.waiting", HasNode, "waiting", "Change"),
        b!(List, "S x", "node.status.done", HasNode, "done", "Change"),
        b!(List, "S -", "node.status.cancelled", HasNode, "cancelled", "Change"),
        b!(List, "d", "node.due", HasNode, "due", "Time", 2),
        b!(List, "s", "node.scheduled", HasNode, "scheduled", "Time"),
        b!(List, "p h", "node.priority.high", HasNode, "high", "Change"),
        b!(List, "p m", "node.priority.med", HasNode, "med", "Change"),
        b!(List, "p l", "node.priority.low", HasNode, "low", "Change"),
        b!(List, "p -", "node.priority.none", HasNode, "none", "Change"),
        b!(List, "#", "node.tags", HasNode, "tags", "Change"),
        b!(List, "i", "node.text", HasNode, "edit text", "Change"),
        b!(List, "m", "node.move", HasNode, "move", "Change"),
        b!(List, "D", "node.delete", HasNode, "delete", "Change"),
        b!(List, "r", "node.skip", NodeRepeats, "skip", "Time"),
        b!(List, "z", "node.snooze", NodeHasAlert, "snooze", "Time"),
        b!(List, "Z", "node.ack", NodeHasAlert, "ack", "Time"),
        b!(List, "c", "node.compare", NodeHasConflict, "compare", "Change", 2),
        b!(List, "c", "node.compare", Always, "compare", "Change"),
        b!(List, "e", "node.edit_external", HasNode, "$EDITOR", "Change"),
        b!(List, "E", "doc.edit_external", Always, "$EDITOR page", "Change"),
        b!(List, "L", "node.history", Always, "history", "View"),
        b!(List, "y", "node.copy_id", HasNode, "copy id", "Change"),
        b!(List, "Y", "node.copy_full_id", HasNode, "copy full id", "Change"),
        b!(List, "space", "leader", Always, "leader", "View"),
        b!(List, "a", "capture.here", Always, "add", "Change", 2),
        b!(List, "A-o", "sidebar.open_aside", HasTarget, "aside", "Sidebar", 6),
        b!(List, "S-enter", "sidebar.open_aside", HasTarget, "aside", "Sidebar"),
        b!(List, "o", "sidebar.open_aside", RowAside, "aside", "Sidebar"),
        b!(List, "A", "capture.inbox", Always, "inbox", "Change"),
        // ---- views (§12.3)
        b!(Today, "x", "node.done", NodeIsTask, "done", "Change", 1),
        b!(Today, "a", "capture.here", Always, "add", "Change", 2),
        b!(Today, "w", "today.agenda_toggle", Always, "agenda", "View", 3),
        b!(Today, "space", "leader", Always, "leader", "View", 4),
        b!(Today, "v", "today.show_done", Always, "done today", "View"),
        b!(Today, "[", "day.prev", Always, "day", "Time"),
        b!(Today, "]", "day.next", Always, "day", "Time"),
        b!(Today, "{", "week.prev", Always, "week", "Time"),
        b!(Today, "}", "week.next", Always, "week", "Time"),
        b!(Journal, "[", "day.prev", Always, "day", "Time"),
        b!(Journal, "]", "day.next", Always, "day", "Time"),
        b!(Journal, "{", "week.prev", Always, "week", "Time"),
        b!(Journal, "}", "week.next", Always, "week", "Time"),
        b!(Journal, ".", "ids.toggle", Always, "ids", "View"),
        b!(Pages, ".", "ids.toggle", Always, "ids", "View"),
        b!(Pages, "enter", "open", PagesIndex, "open", "Move", 1),
        b!(Pages, "/", "pages.filter", PagesIndex, "filter", "Find", 2),
        b!(Search, "enter", "open", Always, "open", "Move", 1),
        b!(Search, "right", "open", Always, "open", "Move"),
        b!(Search, "/", "search.find", Always, "search", "Find", 2),
        b!(Inbox, "m", "node.move", HasNode, "move", "Change", 1),
        b!(Inbox, "t", "node.task_toggle", HasNode, "task", "Change", 2),
        b!(Inbox, "d", "node.due", HasNode, "date", "Time", 3),
        b!(Inbox, "x", "node.done", NodeIsTask, "done", "Change", 4),
        b!(Inbox, "D", "node.delete", HasNode, "delete", "Change", 5),
        b!(Tasks, "f", "tasks.filter", Always, "filter", "Find", 1),
        b!(Tasks, ",", "tasks.sort_cycle", Always, "sort", "View", 2),
        b!(Tasks, "x", "node.done", NodeIsTask, "done", "Change", 3),
        b!(Tasks, "space", "leader", Always, "leader", "View", 4),
        b!(Log, "a", "review.accept", LaneOn, "accept", "Change", 1),
        b!(Log, "u", "review.undo", LaneOn, "undo", "Change", 2),
        b!(Log, "A", "review.accept_all", LaneOn, "accept all", "Change", 3),
        b!(Log, "r", "log.lane_toggle", LaneOn, "all changes", "View", 4),
        b!(Log, "u", "log.undo_tx", LogAll, "undo tx", "Change", 1),
        b!(Log, "r", "log.lane_toggle", LogAll, "review", "View", 2),
        b!(Log, "@", "log.actor_cycle", LogBrowse, "actor", "View", 3),
        b!(Log, "@", "log.actor_cycle", LogAll, "actor", "View"),
        b!(Log, "R", "node.rewind", NodeLog, "rewind", "Change"),
        b!(Log, "enter", "open", LogBrowse, "open", "Move", 4),
        b!(ToastAlert, "x", "toast.done", Always, "done", "Change", 0),
        b!(ToastAlert, "z", "toast.snooze", Always, "snooze", "Time", 0),
        b!(ToastAlert, "Z", "toast.ack", Always, "ack", "Time", 0),
        b!(ToastAgent, "L", "review.last_agent_tx", Always, "review", "View", 0),
        b!(ToastAgent, "u", "undo", Always, "undo", "Change", 0),
        b!(ToastConfirm, "u", "undo", Always, "undo", "Change", 0),
        // ---- write (§12.5, writing.md §3): sealed; printable keys type.
        b!(Write, "C-t", "doc.task_cycle", Always, "task", "Write", 1),
        b!(Write, "C-enter", "doc.task_cycle", Always, "task", "Write"),
        b!(Write, "C-o", "doc.open", Always, "open", "Write", 2),
        b!(Write, "A-enter", "doc.open", Always, "open", "Write"),
        b!(Write, "C-p", "doc.day_prev", DocIsJournal, "day", "Time", 3),
        b!(Write, "C-n", "doc.day_next", DocIsJournal, "day", "Time", 3),
        b!(Write, "C-p", "doc.day_prev", Always, "day", "Time"),
        b!(Write, "C-n", "doc.day_next", Always, "day", "Time"),
        b!(Write, "A-o", "sidebar.open_aside", HasTarget, "aside", "Sidebar", 3),
        b!(Write, "A-s", "sidebar.focus", SidebarHasPanels, "sidebar", "Sidebar", 4),
        b!(Write, "A-T", "sidebar.reopen", SidebarClosedAny, "reopen", "Sidebar"),
        b!(Write, "A-\\", "sidebar.toggle", SidebarHasPanels, "hide · show", "Sidebar"),
        b!(Write, "A-=", "sidebar.wider", SidebarShown, "width", "Sidebar"),
        b!(Write, "A--", "sidebar.narrower", SidebarShown, "width", "Sidebar"),
        b!(Write, "A-0", "sidebar.width_auto", SidebarShown, "width", "Sidebar"),
        b!(Write, "esc", "doc.done", Always, "done", "Write", 4),
        // Undo is one of writing.md's ten keys: help lists it with them.
        b!(Write, "C-z", "doc.undo", Always, "undo", "Write"),
        b!(Write, "Cmd-z", "doc.undo", Always, "undo", "Write"),
        b!(Write, "C-y", "doc.redo", Always, "redo", "Write"),
        b!(Write, "C-r", "doc.redo", Always, "redo", "Write"),
        b!(Write, "Cmd-Z", "doc.redo", Always, "redo", "Write"),
        b!(Write, "f1", "help.context", Always, "keys", "View", 5),
        b!(Write, "A-?", "help.context", Always, "keys", "View"),
        b!(Write, "C-q", "quit", Always, "quit", "View"),
        b!(Write, "C-c", "clip.copy", Always, "copy", "Clipboard"),
        b!(Write, "C-x", "clip.cut", Always, "cut", "Clipboard"),
        b!(Write, "A-v", "paste.plain_next", Always, "plain paste", "Clipboard"),
        b!(Write, "Cmd-c", "clip.copy", Always, "copy", "Clipboard"),
        b!(Write, "Cmd-x", "clip.cut", Always, "cut", "Clipboard"),
        b!(Write, "A-a", "select.all", Always, "select all", "Clipboard"),
        b!(Write, "Cmd-a", "select.all", Always, "select all", "Clipboard"),
        b!(Write, "C-j", "line.soft_break", Always, "line break", "Write"),
        b!(Write, "S-enter", "line.soft_break", Always, "line break", "Write"),
        b!(Write, "enter", "line.newline", Always, "", "Write"),
        b!(Write, "tab", "line.indent", Always, "indent", "Write"),
        b!(Write, "S-tab", "line.outdent", Always, "outdent", "Write"),
        b!(Write, "A-up", "line.move_up", Always, "move line", "Write"),
        b!(Write, "A-down", "line.move_down", Always, "move line", "Write"),
        b!(Write, "A-z", "focus.toggle", Always, "focus", "View"),
        b!(Write, "A-[", "doc.day_prev", Always, "day", "Time"),
        b!(Write, "A-]", "doc.day_next", Always, "day", "Time"),
        b!(Write, "A-t", "go.journal_today", Always, "today", "Time"),
        b!(Write, "Cmd-[", "nav.back", Always, "back", "Go"),
        b!(Write, "Cmd-]", "nav.forward", Always, "forward", "Go"),
        b!(Write, "C-A-left", "nav.back", Always, "back", "Go"),
        b!(Write, "C-A-right", "nav.forward", Always, "forward", "Go"),
        b!(Write, "A-1", "go.today", Always, "Today", "View"),
        b!(Write, "A-2", "go.inbox", Always, "Inbox", "View"),
        b!(Write, "A-3", "go.tasks", Always, "Tasks", "View"),
        b!(Write, "A-4", "go.pages", Always, "Pages", "View"),
        b!(Write, "A-5", "go.journal", Always, "Journal", "View"),
        b!(Write, "A-6", "go.search", Always, "Search", "View"),
        b!(Write, "A-7", "go.log", Always, "Log", "View"),
        b!(Write, "A-:", "palette.open", Always, "commands", "View"),
        // Editing: caretline's keys are imported below (editing_keys.rs); these are thc's own.
        b!(Write, "C-d", "edit.delete_forward", Always, "", "Write"),
        // ⌘V as the kitty protocol reports it (thc's WezTerm keys send it while thc runs): thc
        // reads the clipboard itself, so a screenshot attaches and text pastes.
        b!(Write, "Cmd-v", "clip.paste_system", Always, "paste (screenshots too)", "Clipboard"),
        // ⌃V reads this machine's clipboard too, in any terminal (a local session's).
        b!(Write, "C-v", "clip.paste_system", Always, "paste (screenshots too)", "Clipboard"),
    ];
    let imported = crate::editing_keys::rows(&t);
    t.extend(imported);
    t.extend(vec![
        // ---- sidebar (sidebar.md §5.3): chords only, over the panel's own keys.
        b!(Sidebar, "A-s", "sidebar.focus", Always, "main", "Sidebar", 1),
        b!(Sidebar, "esc", "sidebar.back", SidebarOver, "back", "Sidebar", 6),
        b!(Sidebar, "esc", "sidebar.back", Always, "main", "Sidebar"),
        b!(Sidebar, "A-j", "sidebar.next", Always, "panel", "Sidebar", 2),
        b!(Sidebar, "A-k", "sidebar.prev", Always, "panel", "Sidebar", 2),
        b!(Sidebar, "A-c", "sidebar.fold", Always, "fold", "Sidebar", 3),
        b!(Sidebar, "A-w", "sidebar.close", Always, "close", "Sidebar", 4),
        b!(Sidebar, "A-m", "sidebar.to_main", PanelIsDoc, "to main", "Sidebar", 5),
        b!(Sidebar, "A-p", "sidebar.pin", Always, "pin · unpin", "Sidebar"),
        b!(Sidebar, "A-K", "sidebar.move_up", Always, "move", "Sidebar"),
        b!(Sidebar, "A-J", "sidebar.move_down", Always, "move", "Sidebar"),
        b!(Sidebar, "A-T", "sidebar.reopen", SidebarClosedAny, "reopen", "Sidebar"),
        b!(Sidebar, "A-\\", "sidebar.toggle", Always, "hide · show", "Sidebar"),
        b!(Sidebar, "A-=", "sidebar.wider", SidebarShown, "width", "Sidebar"),
        b!(Sidebar, "A--", "sidebar.narrower", SidebarShown, "width", "Sidebar"),
        b!(Sidebar, "A-0", "sidebar.width_auto", SidebarShown, "width", "Sidebar"),
        b!(Sidebar, "C-w", "pane.next", Always, "next pane", "Sidebar"),
        b!(Sidebar, "f1", "help.context", Always, "keys", "View", 9),
    ]);
    t.extend(vec![
        // ---- overlays and prompts (§12.6): sealed; these rows feed the footer and help.
        b!(Link, "up", "link.prev", Always, "choose", "Move", 1),
        b!(Link, "down", "link.next", Always, "choose", "Move", 1),
        b!(Link, "C-p", "link.prev", Always, "", "Move"),
        b!(Link, "C-n", "link.next", Always, "", "Move"),
        b!(Link, "enter", "link.insert", Always, "link", "Write", 2),
        b!(Link, "tab", "link.insert", Always, "", "Write"),
        b!(Link, "esc", "link.close", Always, "close", "Write", 3),
        b!(Prompt, "enter", "prompt.submit", Always, "save", "Change", 1),
        b!(Prompt, "esc", "prompt.cancel", Always, "cancel", "Change", 2),
        b!(Prompt, "C-c", "prompt.cancel", Always, "", "Change"),
        b!(Palette, "enter", "palette.run", Always, "run", "View", 1),
        b!(Palette, "tab", "palette.complete", Always, "complete", "View", 2),
        b!(Palette, "esc", "palette.close", Always, "", "View", 3),
        b!(Palette, "C-c", "palette.close", Always, "", "View"),
        b!(Palette, "down", "palette.next", Always, "", "Move"),
        b!(Palette, "up", "palette.prev", Always, "", "Move"),
        b!(Palette, "C-n", "palette.next", Always, "", "Move"),
        b!(Palette, "C-p", "palette.prev", Always, "", "Move"),
        b!(Finder, "up", "finder.prev", Always, "choose", "Move", 1),
        b!(Finder, "down", "finder.next", Always, "choose", "Move", 1),
        b!(Finder, "C-p", "finder.prev", Always, "", "Move"),
        b!(Finder, "C-n", "finder.next", Always, "", "Move"),
        b!(Finder, "enter", "finder.go", Always, "go", "Move", 2),
        b!(Finder, "esc", "finder.close", Always, "close", "Move", 3),
        b!(Vaults, "up", "vaults.prev", Always, "choose", "Move", 1),
        b!(Vaults, "down", "vaults.next", Always, "choose", "Move", 1),
        b!(Vaults, "enter", "vaults.switch", Always, "switch", "Move", 2),
        b!(Vaults, "n", "vaults.new", Always, "new vault", "Move", 3),
        b!(Vaults, "esc", "vaults.close", Always, "close", "Move", 4),
        b!(Capture, "enter", "capture.save", Always, "save", "Change", 1),
        b!(Capture, "tab", "capture.target_next", Always, "target", "Change", 2),
        b!(Capture, "S-tab", "capture.target_prev", Always, "", "Change"),
        b!(Capture, "esc", "capture.close", Always, "", "Change", 3),
        b!(Capture, "C-c", "capture.close", Always, "", "Change"),
        b!(Move, "enter", "move.go", Always, "move", "Change", 1),
        b!(Move, "1", "move.recent", Always, "recent", "Change", 2),
        b!(Move, "2", "move.recent", Always, "recent", "Change", 2),
        b!(Move, "3", "move.recent", Always, "recent", "Change", 2),
        b!(Move, "down", "move.next", Always, "", "Move"),
        b!(Move, "tab", "move.next", Always, "", "Move"),
        b!(Move, "up", "move.prev", Always, "", "Move"),
        b!(Move, "S-tab", "move.prev", Always, "", "Move"),
        b!(Move, "esc", "move.close", Always, "", "Change", 3),
        b!(Compare, "1", "compare.keep_current", CompareText, "keep current", "Change", 1),
        b!(Compare, "2", "compare.keep_other", CompareText, "keep other", "Change", 2),
        b!(Compare, "b", "compare.both", CompareText, "both", "Change", 3),
        b!(Compare, "e", "compare.edit", CompareText, "$EDITOR", "Change"),
        b!(Compare, "enter", "compare.ok", CompareMove, "ok", "Change", 1),
        b!(Compare, "1", "compare.keep_here", CompareRehomed, "keep here", "Change", 1),
        b!(Compare, "2", "compare.delete", CompareRehomed, "delete", "Change", 2),
        b!(Compare, "esc", "compare.later", Always, "later", "Change", 4),
        b!(Focus, "1", "focus.preset", Always, "preset", "View", 2),
        b!(Focus, "2", "focus.preset", Always, "preset", "View", 2),
        b!(Focus, "3", "focus.preset", Always, "preset", "View", 2),
        b!(Focus, "enter", "focus.save", Always, "save", "View", 3),
        b!(Focus, "esc", "focus.close", Always, "close", "View", 4),
        b!(Focus, "C-c", "focus.close", Always, "", "View"),
        b!(Help, "?", "help.all", Always, "every key", "View", 1),
        b!(Help, "esc", "help.close", Always, "close", "View", 2),
        b!(Notes, "esc", "notes.close", Always, "close", "View", 1),
    ]);
    t
}

/// One footer hint: its keys as shown (`⌃P ⌃N`, `↑↓`), its word, and each key's action.
#[derive(Clone, Debug, PartialEq)]
pub struct Hint {
    pub keys: String,
    pub label: String,
    pub actions: Vec<(String, &'static str)>,
}

/// The contexts whose ranked bindings make the footer (§6): the overlay, prompt or toast that
/// owns the bar; else the document (the link popup while it's open); else the view and `?`.
pub fn footer_ctxs(app: &App) -> Vec<Ctx> {
    use crate::app::{Overlay, ToastKind};
    if let Some(o) = &app.overlay {
        return vec![match o {
            Overlay::Help { .. } => Ctx::Help,
            Overlay::Focus => Ctx::Focus,
            Overlay::Finder { .. } => Ctx::Finder,
            Overlay::Vaults { .. } => Ctx::Vaults,
            Overlay::Palette { .. } => Ctx::Palette,
            Overlay::Capture { .. } => Ctx::Capture,
            Overlay::Compare { .. } => Ctx::Compare,
            Overlay::Move { .. } => Ctx::Move,
            // A list you pick from, like the finder: ↑↓ choose · Enter go · Esc.
            Overlay::History { .. } | Overlay::Scope { .. } => Ctx::Finder,
            Overlay::Recipe { .. } => Ctx::Notes,
            Overlay::About(_) => Ctx::Notes,
        }];
    }
    if app.prompt.as_ref().is_some_and(|(k, _)| !k.inline()) {
        return vec![Ctx::Prompt];
    }
    if let Some(t) = app.toast.as_ref().filter(|t| t.alive_at(app.ui.now_ms)) {
        match t.kind {
            ToastKind::Alert => return vec![Ctx::ToastAlert],
            ToastKind::Agent => return vec![Ctx::ToastAgent],
            ToastKind::Confirm => return vec![Ctx::ToastConfirm],
            _ => {}
        }
    }
    if app.ui.focus == crate::app::Focus::Sidebar && app.ui.sidebar.has_panels() {
        return vec![Ctx::Sidebar];
    }
    if app.doc.is_some() {
        return vec![if app.link_open { Ctx::Link } else { Ctx::Write }];
    }
    vec![Ctx::of_view(app.view), Ctx::Global]
}

/// The footer's hints, best first (§6): ranked bindings from `ctxs` whose `when` holds and that
/// a higher context doesn't shadow, sorted by rank; bindings sharing a rank and a word merge
/// (`⌃P ⌃N day`, `↑↓ choose`, `1 2 3 recent`).
pub fn footer(app: &App, ctxs: &[Ctx]) -> Vec<Hint> {
    let mut seen: Vec<Vec<Key>> = Vec::new();
    let mut ranked: Vec<(u8, usize, Hint)> = Vec::new();
    let mut order = 0;
    for &c in ctxs {
        for b in table().iter().filter(|b| b.ctx == c) {
            let Some(keys) = parse_seq(b.keys) else { continue };
            if !b.when.holds(app) || seen.contains(&keys) {
                continue;
            }
            seen.push(keys.clone());
            let Some(rank) = b.footer else { continue };
            let shown = display_seq(&keys);
            if let Some((_, _, h)) = ranked.iter_mut().find(|(r, _, h)| *r == rank && h.label == b.label) {
                let joiner = if matches!(shown.as_str(), "↑" | "↓") && matches!(h.keys.as_str(), "↑" | "↓") { "" } else { " " };
                h.keys = format!("{}{joiner}{shown}", h.keys);
                h.actions.push((shown, b.action));
            } else {
                ranked.push((rank, order, Hint { keys: shown.clone(), label: b.label.into(), actions: vec![(shown, b.action)] }));
                order += 1;
            }
        }
    }
    ranked.sort_by_key(|(r, o, _)| (*r, *o));
    // One key per action, ⌘ when it's known to arrive (keymap.md §7.0).
    if app.derived.mac && cmd_known(app) {
        for (_, _, h) in ranked.iter_mut() {
            if h.actions.len() == 1 {
                let action = h.actions[0].1;
                if let Some(b) = table().iter().find(|b| ctxs.contains(&b.ctx) && b.action == action && b.keys.starts_with("Cmd-")) {
                    if let Some(k) = parse_seq(b.keys) {
                        let shown = display_seq(&k);
                        h.keys = shown.clone();
                        h.actions = vec![(shown, action)];
                    }
                }
            }
        }
    }
    ranked.into_iter().map(|(_, _, h)| h).collect()
}

/// The table in effect: the defaults with `[keys.<context>]` from the settings applied (§8),
/// and what the remaps got wrong. Rebuilt when the TUI switches vaults (their `[keys.*]`
/// differ); each build is kept for the process (a few KB per switch), so `&'static` borrows
/// stay valid.
static TABLE: std::sync::RwLock<Option<&'static (Vec<Binding>, Vec<String>)>> = std::sync::RwLock::new(None);

fn effective() -> &'static (Vec<Binding>, Vec<String>) {
    if let Some(t) = *TABLE.read().unwrap() {
        return t;
    }
    // Your [keys.*] with the vault's layered on (vaults.md §9).
    let eff = thc_core::settings::current();
    let built: &'static (Vec<Binding>, Vec<String>) = Box::leak(Box::new(remap(defaults(), eff.table.get("keys").and_then(|k| k.as_table()))));
    *TABLE.write().unwrap() = Some(built);
    built
}

/// After a vault switch: the next lookup rebuilds the table from that vault's settings.
pub fn reset() {
    *TABLE.write().unwrap() = None;
}

pub fn table() -> &'static [Binding] {
    &effective().0
}

/// The config's remap problems (§8.3), for the bar at startup and `thc keys --conflicts`.
pub fn remap_problems() -> &'static [String] {
    &effective().1
}

/// The contexts a remap can name (§8.1). Pop-ups keep their own keys for now.
fn ctx_named(name: &str) -> Option<Ctx> {
    Ctx::ALL.into_iter().find(|c| c.name() == name).filter(|c| {
        matches!(c, Ctx::Global | Ctx::List | Ctx::Today | Ctx::Inbox | Ctx::Tasks | Ctx::Pages | Ctx::Journal | Ctx::Search | Ctx::Log | Ctx::Write | Ctx::Sidebar)
    })
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + (a[i - 1] != b[j - 1]) as usize);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Apply `[keys.<context>]` (§8.1): a key in a context replaces that key's defaults there, and
/// `no_op` unbinds it. Refused (§8.3), with the reason: an unknown context, key or action; a
/// printable key in `write`; a key that's also a prefix there; a chord no stock terminal sends
/// as an action's only key; removing `esc` in write or `C-c` in global.
pub fn remap(mut t: Vec<Binding>, keys: Option<&toml::Table>) -> (Vec<Binding>, Vec<String>) {
    let mut problems = Vec::new();
    let Some(keys) = keys else { return (t, problems) };
    let actions: Vec<&'static str> = {
        let mut v: Vec<&'static str> = t.iter().map(|b| b.action).collect();
        v.extend(["page.new", "daemon.start", "update", "changes", "about", "focus.overlay", "mouse.toggle", "keys.remap", "no_op"]);
        v.sort();
        v.dedup();
        v
    };
    for (cname, body) in keys {
        let Some(ctx) = ctx_named(cname) else {
            let why = if Ctx::ALL.iter().any(|c| c.name() == cname) { "pop-ups keep their own keys for now" } else { "isn't a context" };
            problems.push(format!("[keys.{cname}] {why}"));
            continue;
        };
        let Some(body) = body.as_table() else {
            problems.push(format!("[keys.{cname}] should be a table of \"key\" = \"action\""));
            continue;
        };
        let mut seen: Vec<(Vec<Key>, String)> = Vec::new();
        for (k, v) in body {
            let Some(action) = v.as_str() else {
                problems.push(format!("[keys.{cname}] \"{k}\" should name an action"));
                continue;
            };
            let Some(seq) = parse_seq(k) else {
                problems.push(format!("[keys.{cname}] \"{k}\" isn't a key · write x, C-t, A-up, S-tab, space f p"));
                continue;
            };
            if !actions.contains(&action) {
                let near = actions.iter().min_by_key(|a| edit_distance(a, action)).filter(|a| edit_distance(a, action) <= 3);
                problems.push(format!("[keys.{cname}] \"{action}\" isn't an action{}", near.map(|n| format!(" · did you mean {n}?")).unwrap_or_default()));
                continue;
            }
            if let Some((_, other)) = seen.iter().find(|(s2, _)| *s2 == seq) {
                problems.push(format!("[keys.{cname}] \"{k}\" is bound twice ({other} and {action}) · the later one wins"));
            }
            seen.push((seq.clone(), action.to_string()));
            let one = seq.len() == 1;
            if matches!(ctx, Ctx::Write | Ctx::Sidebar) && one && matches!(seq[0].code, KeyCode::Char(_)) && seq[0].mods.is_empty() {
                problems.push(format!("{} can't bind \"{k}\" (it would stop typing {k}) · use A-{k} or C-{k}", ctx.name()));
                continue;
            }
            let reserved = (ctx == Ctx::Write && seq == [Key { code: KeyCode::Esc, mods: KeyModifiers::NONE }] && action != "doc.done")
                || (ctx == Ctx::Global && seq == [Key { code: KeyCode::Char('c'), mods: KeyModifiers::CONTROL }] && action != "quit");
            if reserved {
                problems.push(format!("[keys.{cname}] \"{k}\" is reserved (someone must always be able to leave and quit) · add another key instead"));
                continue;
            }
            let prefix_clash = t.iter().filter(|b| b.ctx == ctx).filter_map(|b| parse_seq(b.keys).map(|s2| (b, s2))).find(|(_, s2)| (s2.len() > seq.len() && s2[..seq.len()] == seq[..]) || (seq.len() > s2.len() && seq[..s2.len()] == s2[..]));
            if let Some((b, _)) = prefix_clash {
                problems.push(format!("[keys.{cname}] \"{k}\" collides with \"{}\" (one starts the other)", b.keys));
                continue;
            }
            let unsendable = seq.iter().any(|q| {
                let c = q.mods.contains(KeyModifiers::CONTROL);
                (c && (q.mods.contains(KeyModifiers::SHIFT) || matches!(q.code, KeyCode::Char(ch) if ch.is_ascii_uppercase()))) || (c && matches!(q.code, KeyCode::Char('m' | 'i' | '['))) || (c && matches!(q.code, KeyCode::Char(ch) if ch.is_ascii_punctuation() && ch != ']')) || q.mods.contains(KeyModifiers::SUPER) || matches!(q.code, KeyCode::F(_))
            });
            let has_other = t.iter().any(|b| b.ctx == ctx && b.action == action && parse_seq(b.keys).is_some_and(|s2| s2 != seq));
            if unsendable && !has_other && action != "no_op" {
                problems.push(format!("[keys.{cname}] \"{k}\" can't be an action's only key (stock terminals don't send it) · bind {action} to another key too"));
                continue;
            }
            // Replace the key's defaults in this context, then bind it (unless no_op).
            let old: Vec<Binding> = t.iter().filter(|b| b.ctx == ctx && parse_seq(b.keys).as_deref() == Some(&seq[..])).cloned().collect();
            t.retain(|b| !(b.ctx == ctx && parse_seq(b.keys).as_deref() == Some(&seq[..])));
            // A list key's copies in the views (same key, same action: kept there for the footer's
            // rank) follow the remap, or they'd shadow it.
            if ctx == Ctx::List {
                let views = [Ctx::Today, Ctx::Inbox, Ctx::Tasks, Ctx::Pages, Ctx::Journal, Ctx::Search, Ctx::Log];
                t.retain(|b| !(views.contains(&b.ctx) && parse_seq(b.keys).as_deref() == Some(&seq[..]) && old.iter().any(|o| o.action == b.action)));
            }
            if action == "no_op" {
                continue;
            }
            // Words and rank: the action's own (from its defaults), else the key's old ones.
            let like = t.iter().find(|b| b.ctx == ctx && b.action == action).or_else(|| t.iter().find(|b| b.action == action)).cloned();
            let footer = like.as_ref().and_then(|b| (b.ctx == ctx).then_some(b.footer).flatten()).or_else(|| old.first().and_then(|b| b.footer));
            let at = t.iter().position(|b| b.ctx == ctx).unwrap_or(t.len());
            t.insert(
                at,
                Binding {
                    remapped: true,
                    ctx,
                    keys: leak(k),
                    action: leak(action),
                    when: When::Always,
                    label: like.as_ref().map_or("", |b| b.label),
                    group: like.as_ref().map_or("View", |b| b.group),
                    footer,
                },
            );
        }
    }
    (t, problems)
}

/// The active contexts outside documents and overlays, top first (§2.1).
pub fn stack(app: &App) -> Vec<Ctx> {
    stack_at(app, app.ui.now_ms)
}

fn stack_at(app: &App, now: u64) -> Vec<Ctx> {
    let mut s = Vec::with_capacity(5);
    if let Some(t) = app.toast.as_ref().filter(|t| t.alive_at(now)) {
        match t.kind {
            crate::app::ToastKind::Alert if app.alert_toast_node.is_some() => s.push(Ctx::ToastAlert),
            crate::app::ToastKind::Agent if app.last_agent_tx().is_some() => s.push(Ctx::ToastAgent),
            _ => {}
        }
    }
    s.push(Ctx::of_view(app.view));
    s.push(Ctx::List);
    s.push(Ctx::Global);
    s
}

/// What a key sequence does in these contexts: the first binding, top context first, whose keys
/// match exactly and whose `when` holds. `Some(None)`: the keys start a longer sequence.
pub fn lookup(app: &App, ctxs: &[Ctx], seq: &[Key]) -> Option<Option<&'static Binding>> {
    let mut prefix = false;
    for &c in ctxs {
        for bnd in table().iter().filter(|b| b.ctx == c) {
            let Some(keys) = parse_seq(bnd.keys) else { continue };
            if keys == seq && bnd.when.holds(app) {
                return Some(Some(bnd));
            }
            if keys.len() > seq.len() && keys[..seq.len()] == *seq && bnd.when.holds(app) {
                prefix = true;
            }
        }
    }
    prefix.then_some(None)
}

/// A key in a list view: extend the pending sequence, run what it completes, or cancel the
/// prefix and dispatch the key on its own (§4: a prefix never eats an arrow).
pub fn dispatch(app: &mut App, k: &KeyEvent) {
    let key = Key::of(k);
    let ctxs = stack(app);
    let mut seq = std::mem::take(&mut app.pending_keys);
    seq.push(key);
    match lookup(app, &ctxs, &seq) {
        Some(Some(b)) => {
            app.pending_since = None;
            run(app, b.action);
        }
        Some(None) => start_prefix(app, seq),
        // The leader steps back a level on Esc or ⌫ (closing at the top); a key it doesn't bind
        // closes it and says so (§5.1).
        None if seq.len() > 1 && is_leader(&seq) => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Backspace) && key.mods.is_empty() {
                seq.truncate(seq.len() - 2);
                app.pending_keys = seq;
                if app.pending_keys.is_empty() {
                    app.pending_since = None;
                }
            } else {
                app.pending_since = None;
                app.info(format!("{} isn't bound", display_seq(&seq)));
            }
        }
        None if seq.len() > 1 => {
            app.pending_since = None;
            // Cancel the prefix, then the key on its own (Esc only cancels; never over a
            // document: it's sealed).
            if app.toast.as_ref().is_some_and(|t| t.kind == crate::app::ToastKind::Info) {
                app.toast = None;
            }
            if app.doc.is_none() && key.code != KeyCode::Esc {
                if let Some(Some(b)) = lookup(app, &ctxs, &[key]) {
                    run(app, b.action);
                }
            }
        }
        None => {}
    }
}

/// A prefix is pending: the footer shows its breadcrumb and what can follow (§6).
pub fn start_prefix(app: &mut App, seq: Vec<Key>) {
    if app.pending_keys.is_empty() || app.pending_since.is_none() {
        app.ui.pending_since = Some(app.ui.now_ms);
    }
    app.pending_keys = seq;
}

fn is_leader(seq: &[Key]) -> bool {
    seq.first().is_some_and(|k| k.code == KeyCode::Char(' ') && k.mods.is_empty())
}

/// A group of the leader tree or a prefix, by its keys: what the breadcrumb, popup and help
/// call it (`space g` → go).
pub fn group_name(seq: &[Key]) -> Option<&'static str> {
    Some(match display_seq(seq).as_str() {
        "S" => "status",
        "p" => "priority",
        "g" => "go",
        "space" => "leader",
        "space f" => "find",
        "space g" => "go",
        "space n" => "new",
        "space t" => "toggle",
        "space v" => "views",
        "space w" => "sidebar",
        _ => return None,
    })
}

/// Whether the which-key popup shows now (§5.1): the leader's at once (unless the config says
/// otherwise), a prefix's after `which_key_ms`.
pub fn popup_due(app: &App) -> bool {
    popup_due_at(app, app.ui.now_ms)
}

pub(crate) fn popup_due_at(app: &App, now: u64) -> bool {
    use thc_core::tui_config::LeaderPopup;
    if app.pending_keys.is_empty() {
        return false;
    }
    let ms = app.tui_prefs.which_key_ms;
    let waited = |ms: i64| ms >= 0 && app.pending_since.is_some_and(|t| now.saturating_sub(t) as i64 >= ms);
    if is_leader(&app.pending_keys) {
        match app.tui_prefs.leader_popup {
            LeaderPopup::Immediate => true,
            LeaderPopup::Delay => waited(ms),
            LeaderPopup::Off => false,
        }
    } else {
        waited(ms)
    }
}

/// How long until a pending popup is due, for the event loop's poll.
pub fn popup_wait(app: &App) -> Option<std::time::Duration> {
    let since = app.pending_since?;
    if app.pending_keys.is_empty() || popup_due(app) || app.tui_prefs.which_key_ms < 0 {
        return None;
    }
    Some(std::time::Duration::from_millis(app.tui_prefs.which_key_ms as u64).saturating_sub(app.ui.age(since)))
}

/// The footer while a prefix is pending (§6, §5.1): `space g…` and what can follow, groups
/// first (`f +find`), the leader's `space commands` next, then the leaves.
pub fn prefix_footer(app: &App) -> Option<(String, Vec<Hint>)> {
    if app.pending_keys.is_empty() {
        return None;
    }
    Some((format!("{}…", display_seq(&app.pending_keys)), continuations(app, &app.pending_keys)))
}

/// What can follow a pending prefix: one hint per next key, groups (`+find`) before leaves.
pub fn continuations(app: &App, seq: &[Key]) -> Vec<Hint> {
    let ctxs = stack_at(app, app.ui.now_ms);
    let mut groups: Vec<Hint> = Vec::new();
    let mut leaves: Vec<Hint> = Vec::new();
    for &c in &ctxs {
        for b in table().iter().filter(|b| b.ctx == c) {
            let Some(keys) = parse_seq(b.keys) else { continue };
            if keys.len() <= seq.len() || keys[..seq.len()] != seq[..] || !b.when.holds(app) {
                continue;
            }
            let k = keys[seq.len()].display();
            if groups.iter().chain(leaves.iter()).any(|h| h.keys == k) {
                continue;
            }
            if keys.len() > seq.len() + 1 {
                let label = group_name(&keys[..=seq.len()]).unwrap_or("more");
                groups.push(Hint { keys: k.clone(), label: label.into(), actions: vec![(k, "")] });
            } else {
                let h = Hint { keys: k.clone(), label: b.label.into(), actions: vec![(k, b.action)] };
                if h.keys == "space" {
                    leaves.insert(0, h);
                } else {
                    leaves.push(h);
                }
            }
        }
    }
    groups.extend(leaves);
    groups
}


/// Help's words for an action where the footer's one word isn't enough (writing.md §3's copy
/// for `write`). Actions sharing words share a row: `Tab ⇧Tab  nest · un-nest an item`.
fn help_text(ctx: Ctx, action: &str) -> Option<&'static str> {
    Some(match (ctx, action) {
        (Ctx::Write, "line.newline") => "new line · twice: new note",
        (Ctx::Write, "line.indent" | "line.outdent") => "nest · un-nest an item",
        (Ctx::Write, "doc.task_cycle") => "text → [ ] → [x] → text",
        (Ctx::Write, "clip.copy" | "clip.cut") => "copy · cut",
        (Ctx::Write, "doc.undo" | "doc.redo") => "undo · redo",
        (Ctx::Write, "doc.open") => "open link · page or day",
        (Ctx::Write, "doc.day_prev" | "doc.day_next") => "previous · next day",
        (Ctx::Write, "doc.done") => "done: save, go back",
        (Ctx::Write, "help.context") => "these keys",
        (Ctx::Write, "focus.toggle") => "focus on · off",
        (Ctx::Write, "line.soft_break") => "line break in an item",
        (Ctx::Write, "paste.plain_next") => "next paste plain",
        (Ctx::Write, "line.move_up" | "line.move_down") => "move the line",
        (Ctx::Write, "edit.delete_word" | "edit.kill_to_end") => "delete word · to end",
        (Ctx::Write, a) if a.starts_with("go.") && a != "go.journal_today" => "views",
        (Ctx::Write, "go.journal_today") => "today's journal",
        (Ctx::Write, "quit") => "quit",
        (Ctx::Write, "clip.paste_hint") => "paste: your terminal's ⌘V",
        (_, "cursor.down" | "cursor.up") => "down · up",
        (_, "cursor.top" | "cursor.bottom") => "top · bottom",
        (_, "cursor.half_down" | "cursor.half_up") => "half page",
        (_, "fold.close" | "fold.open") => "fold · unfold",
        (_, a) if a.starts_with("go.") && !matches!(a, "go.date" | "go.journal_today" | "go.search") => "views",
        (_, "view.next" | "view.prev") => "next · previous view",
        (_, "capture.here" | "capture.inbox") => "capture · to inbox",
        (_, "node.done" | "node.reopen") => "done · reopen",
        (_, "node.due" | "node.scheduled") => "due · scheduled",
        (_, "node.copy_id" | "node.copy_full_id") => "copy id · full id",
        (_, "node.edit_external" | "doc.edit_external") => "$EDITOR line · page",
        (_, "node.snooze" | "node.ack") => "snooze · ack alert",
        (_, "day.prev" | "day.next") => "journal days",
        (_, "week.prev" | "week.next") => "journal weeks",
        (_, "cursor.page_down" | "cursor.page_up") => "page down · up",
        (_, "node.history") => "history of this node",
        (_, "pane.detail_toggle") => "detail pane",
        (_, "palette.open") => "commands",
        (_, "finder.open") => "go to a page or day",
        (_, "node.skip") => "skip occurrence",
        (_, "node.text") => "edit text",
        (_, "node.task_toggle") => "task on · off",
        _ => return None,
    })
}

/// The groups help shows, in order.
const GROUPS: [&str; 10] = ["Write", "Clipboard", "Move", "Change", "Time", "Find", "View", "Sidebar", "Outline", "Leader"];

/// This terminal, for the ⌘ memory: TERM_PROGRAM and its version.
fn terminal_id() -> String {
    format!("{} {}", std::env::var("TERM_PROGRAM").unwrap_or_default(), std::env::var("TERM_PROGRAM_VERSION").unwrap_or_default())
}

/// Whether a ⌘ key has arrived from this terminal before (keymap.md §7.0).
pub fn cmd_seen_cached(cache: &std::path::Path) -> bool {
    if crate::SNAPSHOT.with(|s| s.get()) {
        return std::env::var_os("THC_TUI_CMD_SEEN").is_some();
    }
    let id = terminal_id();
    std::fs::read_to_string(cache.join("cmd-seen")).is_ok_and(|s| s.lines().any(|l| l == id))
}

pub fn remember_cmd_seen(cache: &std::path::Path) {
    if crate::SNAPSHOT.with(|s| s.get()) {
        return;
    }
    let f = cache.join("cmd-seen");
    let mut s = std::fs::read_to_string(&f).unwrap_or_default();
    s.push_str(&terminal_id());
    s.push('\n');
    let _ = std::fs::write(f, s);
}

/// ⌘ keys are known to reach thc (keymap.md §7.0): one has arrived from this terminal; kitty or
/// Ghostty with the kitty protocol; or WezTerm with `thc setup wezterm` installed. Never a guess
/// from the platform alone.
pub fn cmd_known(app: &App) -> bool {
    app.cmd_seen || app.derived.command_keys
}

pub(crate) fn detect_cmd_known(app: &App) -> bool {
    // Snapshots decide by THC_TUI_CMD_SEEN alone, whatever terminal runs them.
    if app.cmd_seen || crate::SNAPSHOT.with(|s| s.get()) {
        return app.cmd_seen;
    }
    let tp = std::env::var("TERM_PROGRAM").unwrap_or_default();
    if app.kitty && (tp == "ghostty" || std::env::var("TERM").is_ok_and(|t| t == "xterm-kitty")) {
        return true;
    }
    if tp == "WezTerm" {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
        let files = [std::env::var_os("WEZTERM_CONFIG_FILE").map(std::path::PathBuf::from), Some(home.join(".config/wezterm/wezterm.lua")), Some(home.join(".wezterm.lua"))];
        return files.into_iter().flatten().any(|f| std::fs::read_to_string(f).is_ok_and(|t| t.contains("-- thc: begin")));
    }
    false
}

/// ⌘ keys are shown (macOS; never on Linux).
pub(crate) fn detect_mac() -> bool {
    cfg!(target_os = "macos") || std::env::var_os("THC_TUI_MAC").is_some()
}

/// `C-enter`, `S-enter`, `Cmd-…`: only with the kitty protocol (§12.5 ¹).
fn needs_kitty(keys: &str) -> bool {
    keys.starts_with("Cmd-") || keys == "C-enter" || keys == "S-enter"
}

/// `⌥1 ⌥2 … ⌥7` → `⌥1–⌥7` (a run of digits on the same modifiers).
fn compress(keys: &[String]) -> String {
    let digit = |k: &str| k.chars().last().filter(|c| c.is_ascii_digit()).map(|c| (k[..k.len() - 1].to_string(), c));
    if keys.len() >= 3 {
        if let (Some((pa, a)), Some((pb, b))) = (digit(&keys[0]), digit(&keys[keys.len() - 1])) {
            if pa == pb && keys.iter().all(|k| digit(k).is_some_and(|(p, _)| p == pa)) && (b as u8 - a as u8) as usize + 1 == keys.len() {
                return format!("{}–{}", keys[0], keys[keys.len() - 1]);
            }
        }
    }
    keys.join(" ")
}

/// Help for these contexts (§7): each group's rows `keys  words`, a key shown once (the top
/// context's meaning), one key per action, actions with the same words on one row.
pub fn help(app: &App, ctxs: &[Ctx]) -> Vec<(&'static str, Vec<(String, String)>)> {
    let mut seen: Vec<Vec<Key>> = Vec::new();
    // (group, words) → (actions in order, first key of each)
    let mut rows: Vec<(&'static str, String, Vec<(&'static str, String)>)> = Vec::new();
    // A key shows what it does now where its `when` holds (`/` searches outside Pages), else its
    // first meaning.
    for holding in [true, false] {
        for &c in ctxs {
            for b in table().iter().filter(|b| b.ctx == c) {
                let Some(keys) = parse_seq(b.keys) else { continue };
                let cmd = b.keys.starts_with("Cmd-");
                // On a Mac, ⌘ keys are shown (first, beside their twin: keymap.md §7.0).
                if seen.contains(&keys) || (needs_kitty(b.keys) && !app.kitty && !(cmd && app.derived.mac)) || (holding && !b.when.holds_for_help(app)) {
                    continue;
                }
                seen.push(keys.clone());
                // Sequences under one prefix share a row: `S  status: space / w x -`,
                // `space g  go: t i k p j s l d g`.
                if keys.len() >= 2 && !b.remapped {
                    let head_keys = &keys[..keys.len() - 1];
                    if let Some(word) = group_name(head_keys).filter(|w| *w != "leader" && !(*w == "go" && head_keys.len() == 1)) {
                        let head = display_seq(head_keys);
                        let tail = keys[keys.len() - 1].display();
                        match rows.iter_mut().find(|(g, w, _)| *g == b.group && w.starts_with(&format!("{word}:"))) {
                            Some((_, w, _)) => w.push_str(&format!(" {tail}")),
                            None => rows.push((b.group, format!("{word}: {tail}"), vec![(b.action, head)])),
                        }
                        continue;
                    }
                }
                // The digit views share a row (`1–7 views`), `6` included.
                let digit_view = c == Ctx::Global && matches!(keys[..], [Key { code: KeyCode::Char('1'..='7'), mods }] if mods.is_empty());
                let words = if digit_view { "views" } else { help_text(c, b.action).unwrap_or(b.label) };
                if words.is_empty() {
                    continue;
                }
                let shown = format!("{}{}", display_seq(&keys), if b.remapped { "•" } else { "" });
                match rows.iter_mut().find(|(g, w, _)| *g == b.group && w == words) {
                    Some((_, _, acts)) => {
                        // ⌘ and its twin on one action: `⌘C ⌃C`, ⌘ first.
                        if let Some((_, k)) = acts.iter_mut().find(|(a, k)| *a == b.action && cmd != k.starts_with('⌘') && !k.contains(' ')) {
                            *k = if cmd { format!("{shown} {k}") } else { format!("{k} {shown}") };
                        } else if b.remapped || !acts.iter().any(|(a, k)| *a == b.action && !(k.len() == 1 && shown.len() == 1 && k.chars().all(|c| c.is_ascii_digit()))) {
                            acts.push((b.action, shown));
                        }
                    }
                    None => rows.push((b.group, words.to_string(), vec![(b.action, shown)])),
                }
            }
        }
    }
    // ⌘ keys shown that aren't known to arrive: say how to get them.
    if app.derived.mac && !cmd_known(app) && rows.iter().any(|(_, _, acts)| acts.iter().any(|(_, k)| k.contains('⌘'))) {
        rows.push(("Clipboard", "⌘ keys: thc setup wezterm".to_string(), vec![("", String::new())]));
    }
    GROUPS
        .iter()
        .filter_map(|g| {
            let mut r: Vec<(String, String)> = rows.iter().filter(|(rg, _, _)| rg == g).map(|(_, w, acts)| (compress(&acts.iter().map(|(_, k)| k.clone()).collect::<Vec<_>>()), w.clone())).collect();
            r.sort_by_key(|(k, _)| k.chars().all(|c| c.is_ascii_digit() || c == '–' || c == ' ') as u8);
            (!r.is_empty()).then_some((*g, r))
        })
        .collect()
}


/// The contexts help covers now: the document's, else the view's, the list's and global; with
/// `all`, every view and toast too.
pub fn help_ctxs(app: &App, all: bool) -> Vec<Ctx> {
    let mut v = if app.ui.focus == crate::app::Focus::Sidebar && app.ui.sidebar.has_panels() {
        vec![Ctx::Sidebar, Ctx::Write]
    } else if app.doc.is_some() {
        vec![Ctx::Write]
    } else {
        vec![Ctx::of_view(app.view), Ctx::List, Ctx::Global]
    };
    if all {
        for c in [Ctx::Today, Ctx::Inbox, Ctx::Tasks, Ctx::Pages, Ctx::Journal, Ctx::Search, Ctx::Log, Ctx::List, Ctx::Global, Ctx::ToastAlert, Ctx::ToastAgent] {
            if !v.contains(&c) {
                v.push(c);
            }
        }
    }
    v
}

/// The key an action is on now, as hints show it (`x`, `⌃O`, `p` for a prefix): the first
/// binding in the list contexts, then anywhere. For the palette (§7).
pub fn key_for(app: &App, action: &str) -> Option<String> {
    if let Some(p) = action.strip_prefix("prefix:") {
        return Some(p.to_string());
    }
    let ctxs = [Ctx::of_view(app.view), Ctx::List, Ctx::Global];
    let first = |pred: &dyn Fn(&Binding) -> bool| table().iter().find(|b| b.action == action && pred(b)).and_then(|b| parse_seq(b.keys)).map(|k| display_seq(&k));
    first(&|b| ctxs.contains(&b.ctx) && b.ctx != Ctx::Global)
        .or_else(|| first(&|b| b.ctx == Ctx::Global))
        .or_else(|| first(&|b| !matches!(b.ctx, Ctx::Write)))
}

/// Every action the table names, with what it does. `false`: not an action here.
pub fn run(app: &mut App, action: &str) -> bool {
    // In a panel (sidebar_app.rs): anything beyond the document runs in the main view after
    // the key. ⌥O there opens what's at the panel's caret.
    if app.in_panel.is_some() {
        if action == "sidebar.open_aside" {
            if let Some(k) = app.aside_target() {
                app.panel_defer.push(crate::sidebar_app::Deferred::Aside(k));
            }
        } else {
            app.panel_defer.push(crate::sidebar_app::Deferred::Action(action.to_string()));
        }
        return true;
    }
    if action.starts_with("sidebar.") {
        return crate::sidebar_app::run(app, action);
    }
    if action == "pane.next" && app.ui.sidebar.has_panels() && app.ui.sidebar.shown {
        crate::sidebar_app::pane_next(app);
        return true;
    }
    // Presentation-only actions: a pure update on the state, IO as effects (update.rs).
    if crate::runtime_effects::action(app, action) {
        return true;
    }
    use crate::app::{Overlay, PromptKind, VIEWS};
    use crate::input::LineInput;
    match action {
        "go.pages" => app.show_pages(),
        "view.next" | "view.prev" => {
            let i = VIEWS.iter().position(|v| *v == app.view).unwrap_or(0);
            let n = VIEWS.len();
            match VIEWS[if action == "view.next" { (i + 1) % n } else { (i + n - 1) % n }] {
                View::Pages => app.show_pages(),
                v => app.set_view(v),
            }
        }
        "leader" => start_prefix(app, vec![Key { code: KeyCode::Char(' '), mods: KeyModifiers::NONE }]),
        "find.tag" => {
            app.set_view(View::Pages);
            app.page_open = None;
            app.pages_filter = "#".into();
            let _ = app.reload();
            app.prompt = Some((PromptKind::PagesFilter, LineInput::with("#")));
        }
        "review.lane_open" => {
            app.set_view(View::Log);
            app.log_node = None;
            if !app.review_lane {
                app.toggle_review_lane();
            }
        }
        "views.edit" => {
            app.set_view(View::Pages);
            app.page_open = None;
            app.pages_filter = "@".into();
            let _ = app.reload();
        }
        a if a.starts_with("view.slot.") => {
            let slot: u32 = a["view.slot.".len()..].parse().unwrap_or(0);
            match app.view_slots().into_iter().find(|(t, _)| *t == slot) {
                Some((_, v)) => app.use_view(&v.name),
                None => app.info(format!("no view on {slot} · space v s saves this filter as one")),
            }
        }
        "keys.remap" => crate::runtime_effects::dispatch(app, crate::update::Msg::RemapKeys),
        "daemon.start" => app.start_daemon(),
        "update" => app.start_update(),
        "changes" | "about" => crate::about::open(app),
        "focus.overlay" => app.focus_command(""),
        "palette.open" => app.open_palette(),
        "vault.picker" => app.open_vault_picker(),
        "view.scope" => {
            if app.view == View::Today && !app.others.is_empty() {
                let picked = app.scope_vaults();
                app.overlay = Some(crate::app::Overlay::Scope { sel: 0, picked });
            } else {
                app.info("one vault here · the scope picker is for views across vaults".to_string());
            }
        }
        "nav.back" => app.history_go(-1),
        "nav.forward" => app.history_go(1),
        "context.toggle" => app.toggle_context(),
        "go.journal_today" => {
            if app.doc.is_some() {
                app.save_doc(true);
            }
            app.journal_today();
        }
        "undo" | "log.undo_tx" => app.undo(),
        "back" => app.back(),
        "open" => app.open_selected(),
        "fold.close" => app.toggle_fold(Some(false)),
        "fold.open" => app.toggle_fold(Some(true)),
        "node.done" => app.toggle_done(false),
        "node.reopen" => app.toggle_done(true),
        "node.task_toggle" => app.toggle_task(),
        a if a.starts_with("node.status.") => {
            if let Some(id) = app.selected_id() {
                let c = match &a["node.status.".len()..] {
                    "todo" => ' ',
                    "doing" => '/',
                    "waiting" => 'w',
                    "done" => 'x',
                    _ => '-',
                };
                app.set_status(&id, c);
            }
        }
        a if a.starts_with("node.priority.") => {
            if let Some(id) = app.selected_id() {
                let c = match &a["node.priority.".len()..] {
                    "high" => 'h',
                    "med" => 'm',
                    "low" => 'l',
                    _ => '-',
                };
                app.set_priority(&id, c);
            }
        }
        "node.due" | "node.scheduled" => {
            if let Some(n) = app.selected_node().cloned() {
                let (kind, cur) = if action == "node.due" { (PromptKind::Due(n.id.clone()), n.due.clone()) } else { (PromptKind::Sched(n.id.clone()), n.scheduled.clone()) };
                let human = cur.as_deref().map(|c| crate::input::human_date(c, app.today)).unwrap_or_default();
                app.prompt = Some((kind, LineInput::with(&human)));
            }
        }
        "node.tags" => {
            if let Some(n) = app.selected_node() {
                let tags = app.vault.store.tags_of(&n.id).unwrap_or_default().join(" ");
                app.prompt = Some((PromptKind::Tags(n.id.clone()), LineInput::with(&tags)));
            }
        }
        "node.text" => {
            if let Some(n) = app.selected_node() {
                if n.title.is_some() && n.parent.is_none() {
                    app.info("rename pages with thc set <id> title=…");
                } else {
                    let text = app.vault.store.render_text(&n.text);
                    let first = text.lines().next().unwrap_or("").to_string();
                    app.prompt = Some((PromptKind::Text(n.id.clone()), LineInput::with(&first)));
                }
            }
        }
        "node.delete" => app.delete_selected(),
        "node.skip" => app.skip_selected(),
        "node.snooze" => {
            if let Some(a) = app.alert_of_selected() {
                app.prompt = Some((PromptKind::Snooze(a), LineInput::with("1h")));
            }
        }
        "node.ack" => app.ack_selected(),
        "node.compare" => app.open_compare(),
        "doc.edit_external" => {
            let root = match app.view {
                View::Journal => {
                    let key = app.journal_date.format("%Y-%m-%d").to_string();
                    app.vault.store.journal_node(&key).ok().flatten()
                }
                View::Pages => app.page_open.clone(),
                _ => app.selected_id(),
            };
            match root {
                Some(id) => app.editor_request = Some(id),
                None => app.info("nothing to edit yet: Enter opens it to write"),
            }
        }
        "node.copy_id" => app.copy_id(false),
        "node.copy_full_id" => app.copy_id(true),
        "capture.here" | "capture.inbox" => {
            let targets = app.capture_targets(action == "capture.inbox");
            app.overlay = Some(Overlay::Capture { input: LineInput::default(), targets, which: 0 });
        }
        "ids.toggle" => app.toggle_page_ids(),
        "tasks.sort_cycle" => app.cycle_sort(),
        "log.lane_toggle" => app.toggle_review_lane(),
        "log.actor_cycle" => app.cycle_log_actor(),
        "review.accept" => app.review_accept(),
        "review.undo" => app.review_revert(),
        "review.accept_all" => app.ask_accept_all(),
        "node.rewind" => app.ask_rewind(),
        "toast.done" | "toast.snooze" | "toast.ack" => {
            let Some(node) = app.alert_toast_node.clone() else { return false };
            app.selected = Some(node);
            let _ = app.reload();
            app.toast = None;
            match action {
                "toast.done" => app.toggle_done(false),
                "toast.ack" => app.ack_selected(),
                _ => {
                    if let Some(a) = app.alert_of_selected() {
                        app.prompt = Some((PromptKind::Snooze(a), LineInput::with("1h")));
                    }
                }
            }
        }
        "review.last_agent_tx" => {
            let Some(tx) = app.last_agent_tx() else { return false };
            app.toast = None;
            app.review_tx(&tx);
        }
        _ => return false,
    }
    true
}

// ---- `thc keys` (§7, §8.3) ---------------------------------------------------------------------

/// A binding's words in `thc keys`: its own label (one row per action), help's words where the
/// label is empty (the editing keys).
fn doc_words(c: Ctx, b: &Binding) -> &'static str {
    if !b.label.is_empty() {
        return b.label;
    }
    // The editing keys: help leaves them out (arrows need no telling); the tables name them,
    // in caretline's words (its command catalog).
    // Enter is thc's own (twice: a new note), so its words are thc's.
    if c == Ctx::Write && b.action != "line.newline" {
        if let Some(w) = crate::editing_keys::words(b.action) {
            return leak(&w);
        }
    }
    help_text(c, b.action).unwrap_or("")
}

/// The effective table as JSON: every binding with its context, keys, action, condition, label,
/// group and footer rank.
pub fn to_json() -> serde_json::Value {
    let rows: Vec<serde_json::Value> = table()
        .iter()
        .map(|b| {
            let mut o = serde_json::json!({"context": b.ctx.name(), "keys": b.keys, "action": b.action, "label": b.label, "group": b.group});
            if b.when != When::Always {
                o["when"] = b.when.name().into();
            }
            if let Some(r) = b.footer {
                o["footer"] = r.into();
            }
            if let Some(k) = parse_seq(b.keys) {
                o["shown"] = display_seq(&k).into();
            }
            o
        })
        .collect();
    serde_json::json!({"bindings": rows, "conflicts": conflicts()})
}

/// The table as the guide's tables: one per context, a row per action (its keys together).
pub fn to_markdown() -> String {
    let mut out = String::new();
    // The leader's tree gets its own table, after global's.
    let leader = |b: &&Binding| b.ctx == Ctx::Global && b.keys.starts_with("space ");
    let mut sections: Vec<(&'static str, Vec<&Binding>)> = Vec::new();
    for c in Ctx::ALL {
        sections.push((c.name(), table().iter().filter(|b| b.ctx == c && !leader(b)).collect()));
        if c == Ctx::Global {
            sections.push(("leader", table().iter().filter(leader).collect()));
        }
    }
    for (name, bindings) in sections {
        let c = bindings.first().map_or(Ctx::Global, |b| b.ctx);
        let mut rows: Vec<(Vec<String>, &'static str, &'static str, When, Option<u8>)> = Vec::new();
        for b in bindings {
            let Some(k) = parse_seq(b.keys) else { continue };
            let shown = format!("`{}`", display_seq(&k));
            let words = doc_words(c, b);
            // One row per action: its keys together; a condition only if every binding has it.
            match rows.iter_mut().find(|r| r.1 == b.action) {
                Some(r) => {
                    if !r.0.contains(&shown) {
                        r.0.push(shown);
                    }
                    if r.3 != b.when {
                        r.3 = When::Always;
                    }
                }
                None => rows.push((vec![shown], b.action, words, b.when, b.footer)),
            }
        }
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!("### `{name}`\n\n| Keys | Does | Action | When |\n|---|---|---|---|\n"));
        for (mut keys, action, words, when, _) in rows {
            // ⌘ first, then its twins (keymap.md §7.0).
            keys.sort_by_key(|k| !k.contains('⌘'));
            out.push_str(&format!("| {} | {} | `{action}` | {} |\n", keys.join(" "), if words.is_empty() { "" } else { words }, when.name()));
        }
        out.push('\n');
    }
    out.trim_end().to_string() + "\n"
}

/// What §8.3 would refuse or warn about in the effective table: unreachable duplicates, a key
/// that's also a prefix, a printable key bound in `write`, keys a stock terminal can't send.
pub fn conflicts() -> Vec<String> {
    let t = table();
    let mut out: Vec<String> = remap_problems().to_vec();
    for (i, a) in t.iter().enumerate() {
        let Some(ka) = parse_seq(a.keys) else {
            out.push(format!("[keys.{}] \"{}\" isn't a key", a.ctx.name(), a.keys));
            continue;
        };
        for b in t[..i].iter().filter(|b| b.ctx == a.ctx) {
            let Some(kb) = parse_seq(b.keys) else { continue };
            if kb == ka && b.when == a.when && b.action != a.action {
                out.push(format!("[keys.{}] \"{}\" is bound twice ({} and {}) · the later one never runs", a.ctx.name(), a.keys, b.action, a.action));
            }
            if (kb.len() > ka.len() && kb[..ka.len()] == ka[..]) || (ka.len() > kb.len() && ka[..kb.len()] == kb[..]) {
                out.push(format!("[keys.{}] \"{}\" and \"{}\": one is a prefix of the other", a.ctx.name(), b.keys, a.keys));
            }
        }
        if a.ctx == Ctx::Write && ka.len() == 1 && matches!(ka[0].code, KeyCode::Char(_)) && ka[0].mods.is_empty() {
            out.push(format!("write can't bind \"{}\" (it would stop typing it) · use A-{} or C-{}", a.keys, a.keys, a.keys));
        }
        // Chords a stock terminal never sends are only allowed as an action's second key.
        let stock_unsendable = ka.iter().any(|k| {
            let c = k.mods.contains(KeyModifiers::CONTROL);
            (c && (k.mods.contains(KeyModifiers::SHIFT) || matches!(k.code, KeyCode::Char(ch) if ch.is_ascii_uppercase())))
                || (c && matches!(k.code, KeyCode::Char('m' | 'i' | '[')))
                || (c && matches!(k.code, KeyCode::Char(ch) if ch.is_ascii_punctuation() && ch != ']'))
                || k.mods.contains(KeyModifiers::SUPER)
        });
        if stock_unsendable && !t.iter().any(|o| o.ctx == a.ctx && o.action == a.action && o.keys != a.keys && !needs_kitty(o.keys) && parse_seq(o.keys).is_some_and(|k| !k.iter().any(|k| k.mods.contains(KeyModifiers::SUPER) || (k.mods.contains(KeyModifiers::CONTROL) && k.mods.contains(KeyModifiers::SHIFT))))) {
            out.push(format!("[keys.{}] \"{}\" can't be sent by a stock terminal · give {} another key too", a.ctx.name(), a.keys, a.action));
        }
    }
    out
}

/// One line per binding, by context: `thc keys` without a format.
pub fn to_text() -> String {
    let mut out = String::new();
    for c in Ctx::ALL {
        let rows: Vec<&Binding> = table().iter().filter(|b| b.ctx == c).collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!("{}\n", c.name()));
        for b in rows {
            let shown = parse_seq(b.keys).map(|k| display_seq(&k)).unwrap_or_default();
            let words = doc_words(c, b);
            let when = if b.when == When::Always { String::new() } else { format!("  ({})", b.when.name()) };
            out.push_str(&format!("  {shown:<12} {:<26} {words}{when}\n", b.action));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_parses_and_names_an_action() {
        for b in defaults() {
            assert!(parse_seq(b.keys).is_some_and(|s| !s.is_empty()), "{} doesn't parse", b.keys);
        }
    }

    #[test]
    fn matching_is_exact() {
        let ev = |code, mods| KeyEvent::new(code, mods);
        let x = Key::parse("x").unwrap();
        assert_eq!(Key::of(&ev(KeyCode::Char('x'), KeyModifiers::NONE)), x);
        assert_ne!(Key::of(&ev(KeyCode::Char('x'), KeyModifiers::ALT)), x, "⌥X is not x");
        assert_ne!(Key::of(&ev(KeyCode::Char('x'), KeyModifiers::CONTROL)), x, "⌃X is not x");
        assert_eq!(Key::of(&ev(KeyCode::Char('X'), KeyModifiers::SHIFT)), Key::parse("X").unwrap());
        assert_eq!(Key::of(&ev(KeyCode::BackTab, KeyModifiers::SHIFT)), Key::parse("S-tab").unwrap());
        assert_eq!(Key::of(&ev(KeyCode::Char('z'), KeyModifiers::SUPER | KeyModifiers::SHIFT)), Key::parse("Cmd-Z").unwrap());
        assert_eq!(Key::of(&ev(KeyCode::Up, KeyModifiers::ALT)), Key::parse("A-up").unwrap());
        assert_eq!(Key::parse("ctrl-t"), Key::parse("C-t"));
    }

    #[test]
    fn keys_display_as_hints() {
        assert_eq!(Key::parse("C-t").unwrap().display(), "⌃T");
        assert_eq!(Key::parse("A-up").unwrap().display(), "⌥↑");
        assert_eq!(Key::parse("S-tab").unwrap().display(), "⇧Tab");
        assert_eq!(Key::parse("Cmd-c").unwrap().display(), "⌘C");
        assert_eq!(Key::parse("Cmd-Z").unwrap().display(), "⇧⌘Z");
        assert_eq!(Key::parse("space").unwrap().display(), "space");
        assert_eq!(display_seq(&parse_seq("g g").unwrap()), "g g");
    }

    fn cfg(s: &str) -> toml::Table {
        s.parse::<toml::Table>().unwrap()["keys"].as_table().unwrap().clone()
    }

    #[test]
    fn remaps_replace_unbind_and_refuse() {
        let (t, p) = remap(defaults(), Some(&cfg("[keys.list]\n\"x\" = \"node.delete\"\n[keys.global]\n\"q\" = \"no_op\"\n[keys.write]\n\"A-z\" = \"doc.task_cycle\"\n")));
        assert!(p.is_empty(), "{p:?}");
        let find = |c: Ctx, k: &str| t.iter().filter(|b| b.ctx == c && b.keys == k).map(|b| b.action).collect::<Vec<_>>();
        assert_eq!(find(Ctx::List, "x"), ["node.delete"]);
        assert!(find(Ctx::Global, "q").is_empty(), "no_op unbinds q");
        assert_eq!(find(Ctx::Write, "A-z"), ["doc.task_cycle"]);
        let refused = |toml: &str| remap(defaults(), Some(&cfg(toml))).1;
        assert!(refused("[keys.list]\n\"x\" = \"node.donee\"\n")[0].contains("did you mean node.done?"));
        assert!(refused("[keys.write]\n\"x\" = \"doc.task_cycle\"\n")[0].contains("write can't bind \"x\""));
        assert!(refused("[keys.list]\n\"g\" = \"node.done\"\n")[0].contains("collides with \"g g\""));
        assert!(refused("[keys.write]\n\"esc\" = \"no_op\"\n")[0].contains("reserved"));
        assert!(refused("[keys.list]\n\"C-S-x\" = \"find.tag\"\n")[0].contains("can't be an action's only key"));
        assert!(refused("[keys.list]\n\"C-S-x\" = \"node.skip\"\n").is_empty(), "a second key for skip (r) is fine");
        assert!(refused("[keys.palette]\n\"x\" = \"node.done\"\n")[0].contains("pop-ups keep their own keys"));
        assert!(refused("[keys.nope]\n\"x\" = \"node.done\"\n")[0].contains("isn't a context"));
        assert!(refused("[keys.list]\n\"C-x-y\" = \"node.done\"\n")[0].contains("isn't a key"));
    }

    #[test]
    fn the_defaults_have_no_conflicts() {
        assert_eq!(conflicts(), Vec::<String>::new());
    }

    #[test]
    fn a_key_is_never_both_a_binding_and_a_prefix_in_one_context() {
        // §8.3: `g` alone can't coexist with `g g` in the same context.
        let t = defaults();
        for a in &t {
            let ka = parse_seq(a.keys).unwrap();
            for b in t.iter().filter(|b| b.ctx == a.ctx) {
                let kb = parse_seq(b.keys).unwrap();
                assert!(!(kb.len() > ka.len() && kb[..ka.len()] == ka[..]), "{} {} is a prefix of {}", a.ctx.name(), a.keys, b.keys);
            }
        }
    }
}
