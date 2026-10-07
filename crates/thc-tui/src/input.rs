//! Key handling and a small single-line editor used by prompts, capture and the palette.

use crate::app::{App, Overlay, PromptKind, Row, View, fuzzy};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LineInput {
    pub buf: String,
    /// Cursor position in chars.
    pub cur: usize,
}

pub enum InputResult {
    Submit,
    Cancel,
    Changed,
    Ignored,
}

impl LineInput {
    pub fn with(s: &str) -> LineInput {
        LineInput { buf: s.to_string(), cur: s.chars().count() }
    }

    fn byte(&self, ci: usize) -> usize {
        self.buf.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(self.buf.len())
    }

    pub fn handle(&mut self, k: KeyEvent) -> InputResult {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let len = self.buf.chars().count();
        match k.code {
            KeyCode::Enter => return InputResult::Submit,
            KeyCode::Esc => return InputResult::Cancel,
            KeyCode::Char('c') if ctrl => return InputResult::Cancel,
            KeyCode::Char('a') if ctrl => self.cur = 0,
            KeyCode::Char('e') if ctrl => self.cur = len,
            KeyCode::Char('u') if ctrl => {
                let b = self.byte(self.cur);
                self.buf.replace_range(..b, "");
                self.cur = 0;
            }
            KeyCode::Char('k') if ctrl => {
                let b = self.byte(self.cur);
                self.buf.truncate(b);
            }
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Backspace if alt => self.delete_word(),
            KeyCode::Home => self.cur = 0,
            KeyCode::End => self.cur = len,
            KeyCode::Left if alt => self.cur = self.word_left(),
            KeyCode::Right if alt => self.cur = self.word_right(),
            KeyCode::Left => self.cur = self.cur.saturating_sub(1),
            KeyCode::Right => self.cur = (self.cur + 1).min(len),
            KeyCode::Backspace => {
                if self.cur > 0 {
                    let b = self.byte(self.cur - 1);
                    self.buf.remove(b);
                    self.cur -= 1;
                }
            }
            KeyCode::Delete => {
                if self.cur < len {
                    let b = self.byte(self.cur);
                    self.buf.remove(b);
                }
            }
            KeyCode::Char(c) if !ctrl => {
                let b = self.byte(self.cur);
                self.buf.insert(b, c);
                self.cur += 1;
            }
            _ => return InputResult::Ignored,
        }
        InputResult::Changed
    }

    fn word_left(&self) -> usize {
        let chars: Vec<char> = self.buf.chars().collect();
        let mut i = self.cur;
        while i > 0 && chars[i - 1] == ' ' {
            i -= 1;
        }
        while i > 0 && chars[i - 1] != ' ' {
            i -= 1;
        }
        i
    }

    fn word_right(&self) -> usize {
        let chars: Vec<char> = self.buf.chars().collect();
        let mut i = self.cur;
        while i < chars.len() && chars[i] != ' ' {
            i += 1;
        }
        while i < chars.len() && chars[i] == ' ' {
            i += 1;
        }
        i
    }

    fn delete_word(&mut self) {
        let start = self.word_left();
        let (a, b) = (self.byte(start), self.byte(self.cur));
        self.buf.replace_range(a..b, "");
        self.cur = start;
    }
}

pub fn palette_matches(app: &App, q: &str) -> Vec<crate::app::PaletteEntry> {
    let q = q.to_lowercase();
    let entries = app.palette_entries();
    // The label, or a `:name` command (`:changes`, `:about`) typed as its name.
    let score = |p: &crate::app::PaletteEntry| {
        let by_cmd = p.cmd.strip_prefix(':').and_then(|c| fuzzy(&q, c));
        fuzzy(&q, &p.label.to_lowercase()).max(by_cmd)
    };
    let mut m: Vec<(i64, usize)> = entries.iter().enumerate().filter_map(|(i, p)| score(p).map(|s| (s, i))).collect();
    if !q.is_empty() {
        m.sort_by_key(|(s, _)| -*s);
    }
    m.into_iter().map(|(_, i)| entries[i].clone()).collect()
}

/// A stored date as a person would type it, for an editor's prefill (tui-editor §5): `fri`
/// within a week, else `oct 9`; a time stays (`fri 14:00`).
pub(crate) fn human_date(stored: &str, today: chrono::NaiveDate) -> String {
    let Some(dv) = thc_core::dates::DateVal::from_stored(stored) else { return stored.to_string() };
    let d = dv.date();
    let n = (d - today).num_days();
    let day = match n {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        2..=6 => d.format("%a").to_string().to_lowercase(),
        _ => d.format("%b %-d").to_string().to_lowercase(),
    };
    match stored.split_once('T') {
        Some((_, t)) => format!("{day} {t}"),
        None => day,
    }
}

/// A mouse event (mouse.md): overlays first (a click outside closes them), then what was drawn
/// under the pointer (tabs, footer keys, chips, days, rows), then the document's text, then the
/// wheel in lists. A click runs the same action as its key.
/// A field's prompt for the selected node (a detail-pane value or a list chip): dates prefilled
/// as a person writes them; priority, tags and status as their keys ask.
fn field_prompt(app: &mut App, field: &str) {
    let key = |app: &mut App, c: char| handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    match field {
        "due" | "sched" => {
            if let Some(n) = app.selected_node().cloned() {
                let (kind, cur) = if field == "due" { (PromptKind::Due(n.id.clone()), n.due.clone()) } else { (PromptKind::Sched(n.id.clone()), n.scheduled.clone()) };
                let human = cur.as_deref().map(|c| human_date(c, app.today)).unwrap_or_default();
                app.prompt = Some((kind, LineInput::with(&human)));
            }
        }
        "priority" => key(app, 'p'),
        "tags" => key(app, '#'),
        _ => key(app, 'S'),
    }
}

/// The char index a click `col` cells into `buf` lands on (the right half of a wide character
/// goes after it).
fn caret_at(buf: &str, col: u16) -> usize {
    let mut x = 0u16;
    for (i, c) in buf.chars().enumerate() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) as u16;
        if col < x + cw.div_ceil(2).max(1) {
            return i;
        }
        x += cw;
    }
    buf.chars().count()
}

pub fn handle_mouse(app: &mut App, m: ratatui::crossterm::event::MouseEvent, clicks: u8) {
    app.clock_tick();
    let moved = m.kind == ratatui::crossterm::event::MouseEventKind::Moved;
    handle_mouse_inner(app, m, clicks);
    if !moved {
        app.history_tick(false);
    }
}

fn handle_mouse_inner(app: &mut App, m: ratatui::crossterm::event::MouseEvent, clicks: u8) {
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState, MouseButton, MouseEventKind as K};
    let (x, y) = (m.column, m.row);
    if !app.render.size.contains((x, y).into()) {
        return;
    }
    let key = |app: &mut App, code: KeyCode, modifiers: KeyModifiers| handle_key(app, KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    let at = app.render.click_targets.iter().rev().find(|t| t.y == y && x >= t.x0 && x < t.x1).map(|t| t.what.clone());
    let left_down = m.kind == K::Down(MouseButton::Left);
    if m.kind == K::Moved {
        app.hover = Some((x, y));
        // Hover selects a menu row (mouse.md "Overlays are menus").
        match at {
            Some(crate::ui::Click::Menu(i)) => {
                if let Some(sel) = app.overlay.as_mut().and_then(|o| o.sel_mut()) {
                    *sel = i;
                }
            }
            Some(crate::ui::Click::LinkRow(i)) if app.link_open => app.link_sel = Some(i),
            _ => {}
        }
        return;
    }
    // Dragging the scrollbar's thumb: the view follows the pointer.
    if app.scroll_drag {
        match m.kind {
            K::Drag(MouseButton::Left) => {
                if let (Some((top, h, total, _, _)), Some(d)) = (app.render.doc_scrollbar, app.doc.as_mut()) {
                    d.set_scroll((y.saturating_sub(top) as usize * total / h.max(1) as usize).min(total.saturating_sub(1)), true);
                } else if let Some((top, h, total, _, _)) = app.render.list_scrollbar {
                    // A list has no caret to protect: the cursor goes to the row under the thumb.
                    let row = (y.saturating_sub(top) as usize * total / h.max(1) as usize).min(total.saturating_sub(1));
                    app.select_row(row);
                }
                return;
            }
            K::Up(_) => {
                app.scroll_drag = false;
                return;
            }
            _ => {}
        }
    }
    // Overlays own the mouse: outside closes (the click goes no further), rows run.
    if let (K::ScrollUp | K::ScrollDown, Some(Overlay::About(_))) = (m.kind, app.overlay.as_ref()) {
        if let Some(Overlay::About(mut a)) = app.overlay.take() {
            crate::about::wheel(app, &mut a, m.kind == K::ScrollDown);
            app.overlay = Some(Overlay::About(a));
        }
        return;
    }
    if let (K::ScrollUp | K::ScrollDown, Some(Overlay::Help { scroll, .. })) = (m.kind, app.ui.overlay.as_mut()) {
        let d: i32 = if m.kind == K::ScrollDown { 3 } else { -3 };
        *scroll = (*scroll as i32 + d).clamp(0, app.render.help_max_scroll as i32) as u16;
        return;
    }
    // Every overlay is a menu (mouse.md "Overlays are menus"): one click acts, the wheel moves
    // the selection, a click outside closes and goes no further.
    if app.overlay.is_some() {
        if matches!(m.kind, K::ScrollUp | K::ScrollDown) {
            if app.overlay.as_mut().and_then(|o| o.sel_mut()).is_some() {
                key(app, if m.kind == K::ScrollUp { KeyCode::Up } else { KeyCode::Down }, KeyModifiers::NONE);
            }
            return;
        }
        if left_down {
            if app.render.overlay_rect.is_some_and(|r| !(x >= r.x && x < r.right() && y >= r.y && y < r.bottom())) {
                // The scope picker applies on a click outside (view-explain.md §2); others close.
                if matches!(app.overlay, Some(Overlay::Scope { .. })) {
                    key(app, KeyCode::Enter, KeyModifiers::NONE);
                } else {
                    app.overlay = None;
                }
                return;
            }
            match at {
                Some(crate::ui::Click::Menu(i)) => {
                    // The scope picker's rows are checkboxes: a click toggles (Space).
                    let toggles = matches!(app.overlay, Some(Overlay::Scope { .. }));
                    if let Some(sel) = app.overlay.as_mut().and_then(|o| o.sel_mut()) {
                        *sel = i;
                        key(app, if toggles { KeyCode::Char(' ') } else { KeyCode::Enter }, KeyModifiers::NONE);
                    }
                }
                Some(crate::ui::Click::Caret { x0 }) => {
                    if let Some(input) = app.overlay.as_mut().and_then(|o| o.input_mut()) {
                        input.cur = caret_at(&input.buf, x.saturating_sub(x0));
                    }
                }
                Some(crate::ui::Click::Key(code, mods)) => {
                    // Help is a menu: its row closes it and runs the key. Other overlays take the
                    // key themselves (focus letters, compare's 1 / 2 / b, Enter, Esc).
                    if matches!(app.overlay, Some(Overlay::Help { .. })) && code != KeyCode::Esc && code != KeyCode::Char('?') {
                        app.overlay = None;
                    }
                    key(app, code, mods);
                }
                Some(crate::ui::Click::Action(action)) if matches!(app.overlay, Some(Overlay::Help { .. })) => {
                    app.overlay = None;
                    crate::keymap::run(app, action);
                }
                _ => {}
            }
        }
        return;
    }
    // The list's own find-as-you-type prompts (Pages, Search) keep their text but step aside for
    // a click: rows, tabs and the rest work as everywhere (a double-click opens the page). Every
    // click was swallowed while they were up, so a page clicked open never opened and typing
    // went into the hidden filter.
    if left_down && matches!(app.prompt.as_ref().map(|(k, _)| k), Some(PromptKind::PagesFilter | PromptKind::Search)) {
        app.prompt = None;
    }
    if app.prompt.is_some() {
        return;
    }
    if let Some(crate::ui::Click::Panel(i, part)) = at.clone().filter(|_| matches!(m.kind, K::Down(_))) {
        crate::sidebar_app::header_click(app, i, part, clicks, m.kind == K::Down(MouseButton::Middle));
        return;
    }
    if crate::sidebar_app::mouse(app, m, clicks) {
        return;
    }
    if left_down {
        use crate::ui::Click;
        match at {
            Some(Click::Meta { line, field }) => {
                let Some(d) = app.doc.as_mut() else { return };
                let Some(l) = d.blocks().get(line) else { return };
                let id = l.id.clone();
                match field {
                    "open" => app.doc_open(),
                    "conflict" => {
                        app.selected = Some(id);
                        app.open_compare();
                    }
                    _ => {
                        // A date chip opens its editor, prefilled (mouse.md §3).
                        app.save_doc(true);
                        if let Some(n) = app.vault.store.node(&id).ok().flatten() {
                            let (kind, cur) = if field == "due" { (PromptKind::Due(id.clone()), n.due.clone()) } else { (PromptKind::Sched(id.clone()), n.scheduled.clone()) };
                            let human = cur.as_deref().map(|c| human_date(c, app.today)).unwrap_or_default();
                            app.prompt = Some((kind, LineInput::with(&human)));
                        }
                    }
                }
                return;
            }
            Some(Click::View(v)) => {
                app.save_doc(true);
                app.doc_origin = None;
                if v == View::Pages {
                    app.show_pages();
                } else {
                    app.set_view(v);
                }
                return;
            }
            Some(Click::Key(code, mods)) => {
                key(app, code, mods);
                return;
            }
            // ⇧-click a day or a page row (the strip, the rail, a crumb): beside (sidebar.md §2).
            Some(Click::Day(d)) if m.modifiers.contains(KeyModifiers::SHIFT) => {
                let key = crate::sidebar::PanelKey::day(&app.ui.vault_name, &d.format("%Y-%m-%d").to_string());
                app.open_aside(key, false);
                return;
            }
            Some(Click::Node(id)) if m.modifiers.contains(KeyModifiers::SHIFT) && app.vault.store.node(&id).ok().flatten().is_some_and(|n| n.parent.is_none() && n.title.is_some()) => {
                let key = crate::sidebar::PanelKey::page(&app.ui.vault_name, &id);
                app.open_aside(key, false);
                return;
            }
            Some(Click::Day(d)) => {
                app.save_doc(true);
                app.doc_origin = None;
                app.journal_date = d;
                app.selected = None;
                app.set_view(View::Journal);
                return;
            }
            Some(Click::Row(i)) => {
                app.select_row(i);
                // The row's own box toggles it (as x / X); a double-click opens it.
                let cells = app.render.cells_at(x, y, 2, 2);
                let on_box = ["[ ]", "[x]", "[/]", "[w]", "[-]"].iter().any(|b| cells.contains(b)) && {
                    let here = app.render.cells_at(x, y, 0, 0);
                    ["[", "]", " ", "x", "/", "w", "-"].contains(&here.as_str()) && cells.contains('[')
                };
                if on_box && clicks == 1 {
                    let done = app.selected_node().is_some_and(|n| n.status.as_deref() == Some("done"));
                    app.toggle_done(done);
                } else if clicks == 2 {
                    key(app, KeyCode::Enter, KeyModifiers::NONE);
                }
                return;
            }
            Some(Click::Scroll(i)) => {
                // On the thumb: drag it. On the track: a page toward the click.
                if let (Some((_, h, _, ty, th)), Some(d)) = (app.render.doc_scrollbar, app.doc.as_mut()) {
                    if i >= ty && i < ty + th {
                        app.scroll_drag = true;
                    } else if i < ty {
                        d.set_scroll(d.scroll().saturating_sub(h as usize), true);
                    } else {
                        d.set_scroll(d.scroll() + h as usize, true);
                    }
                }
                return;
            }
            Some(Click::ListScroll(i)) => {
                if let Some((_, h, _, ty, th)) = app.render.list_scrollbar {
                    if i >= ty && i < ty + th {
                        app.scroll_drag = true;
                    } else if i < ty {
                        app.move_cursor(-(h as isize));
                    } else {
                        app.move_cursor(h as isize);
                    }
                }
                return;
            }
            Some(Click::Node(id)) => {
                app.save_doc(true);
                app.focus_node(&id);
                return;
            }
            Some(Click::RowField(i, field)) => {
                app.select_row(i);
                field_prompt(app, field);
                return;
            }
            Some(Click::Field(field)) => {
                field_prompt(app, field);
                return;
            }
            Some(Click::Action(a)) => {
                // A footer hint: its action, in the document when one is open (keymap.rs).
                if app.doc.is_some() && !crate::doc_keys::run_write(app, a) {
                    crate::keymap::run(app, a);
                } else if app.doc.is_none() {
                    crate::keymap::run(app, a);
                }
                return;
            }
            Some(Click::FooterBox(id)) => {
                // Completed where it lives: the doc saves, the row's node is the one acted on.
                app.save_doc(true);
                let done = app.vault.store.node(&id).ok().flatten().is_some_and(|n| n.status.as_deref() == Some("done"));
                app.selected = Some(id.clone());
                app.row_vault.clear();
                app.rows = app.vault.store.node(&id).ok().flatten().map(|n| vec![crate::app::App::node_row_public(n)]).unwrap_or_default();
                app.cursor = 0;
                app.toggle_done(done);
                return;
            }
            Some(Click::HistoryTx(tx)) => {
                key(app, KeyCode::Char('L'), KeyModifiers::NONE);
                if let Some(i) = app.rows.iter().position(|r| matches!(r, Row::Tx { tx: t, .. } if *t == tx)) {
                    app.select_row(i);
                }
                return;
            }
            Some(Click::LinkRow(i)) => {
                if app.link_open {
                    app.link_sel = Some(i);
                    key(app, KeyCode::Enter, KeyModifiers::NONE);
                }
                return;
            }
            Some(Click::Text) => return,
            Some(Click::Panel(..)) => return,
            Some(Click::Menu(_) | Click::Caret { .. } | Click::Box | Click::Link) | None => {}
        }
    }
    if crate::doc_keys::mouse(app, m, clicks) {
        return;
    }
    // The wheel in a list moves through it.
    match m.kind {
        K::ScrollUp => app.move_cursor(-(app.tui_prefs.wheel_rows.max(1) as isize)),
        K::ScrollDown => app.move_cursor(app.tui_prefs.wheel_rows.max(1) as isize),
        _ => {}
    }
}

/// Translate a key into an action.
/// Keys while a row is edited in place (tui-handoff §10.4): every printable key types.
fn edit_key(app: &mut App, k: KeyEvent) {
    match k.code {
        // Esc saves (outliners save as you go); `u` straight after drops the change.
        KeyCode::Esc => {
            if let Ok(saved) = app.edit_commit() {
                app.edit_leave(saved);
            }
        }
        KeyCode::Enter => app.edit_enter(),
        KeyCode::Up => app.edit_move(-1),
        KeyCode::Down => app.edit_move(1),
        KeyCode::Tab => app.edit_nest(true),
        KeyCode::BackTab => app.edit_nest(false),
        KeyCode::Backspace if app.edit.as_ref().is_some_and(|e| e.input.buf.is_empty()) => app.edit_backspace_empty(),
        // ⌃J: a line break inside the node's text (a multi-line note).
        KeyCode::Char('j') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(e) = app.edit.as_mut() {
                let mut chars: Vec<char> = e.input.buf.chars().collect();
                chars.insert(e.input.cur.min(chars.len()), '\n');
                e.input.buf = chars.into_iter().collect();
                e.input.cur += 1;
            }
        }
        // ⌃E with the caret already at the end: continue in $EDITOR.
        KeyCode::Char('e') if k.modifiers.contains(KeyModifiers::CONTROL) && app.edit.as_ref().is_some_and(|e| e.input.cur >= e.input.buf.chars().count()) => {
            if let Ok(saved) = app.edit_commit() {
                let id = saved.clone();
                app.edit_leave(saved);
                app.editor_request = id;
            }
        }
        _ => {
            let Some(e) = app.edit.as_mut() else { return };
            if matches!(e.input.handle(k), InputResult::Changed) {
                e.error = None;
                // `- ` typed at the start of an empty paragraph line makes it a bullet (§10.8).
                if e.para && e.node.is_none() && e.input.buf == "- " {
                    e.input.buf.clear();
                    e.input.cur = 0;
                    e.para = false;
                }
            }
        }
    }
}

/// A key, then the history recorder (history.rs): where it left you is a step if it's new.
pub fn handle_key(app: &mut App, k: KeyEvent) {
    app.clock_tick();
    let tab = matches!(k.code, KeyCode::Tab | KeyCode::BackTab) && app.overlay.is_none();
    let view = app.view;
    handle_key_inner(app, k);
    app.history_tick(tab && app.view != view && app.doc.is_none());
}

fn handle_key_inner(app: &mut App, k: KeyEvent) {
    // Tests only: F12 panics, to prove a crash restores the terminal and keeps what's typed.
    if k.code == KeyCode::F(12) && std::env::var("THC_TUI_TEST_PANIC").is_ok_and(|v| v == "1") {
        panic!("test panic (THC_TUI_TEST_PANIC)");
    }
    if app.edit.is_some() && app.overlay.is_none() {
        return edit_key(app, k);
    }
    // A prefix in progress (`p`, `S`, `g`) takes the next key first.
    if !app.pending_keys.is_empty() && app.overlay.is_none() && app.prompt.is_none() {
        crate::keymap::dispatch(app, &k);
        return;
    }
    // The sidebar has the keyboard (sidebar.md §5.2).
    if crate::sidebar_app::key(app, k) {
        return;
    }
    // A document (a journal day, an open page): always Write (writing.md §3).
    if crate::doc_keys::handle(app, k) {
        return;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    // A ⌘ key reached us: this terminal sends them (remembered for the next session).
    if k.modifiers.contains(KeyModifiers::SUPER) && !app.cmd_seen {
        app.cmd_seen = true;
        crate::keymap::remember_cmd_seen(&app.vault.paths.cache);
    }
    if ctrl && k.code == KeyCode::Char('c') && app.prompt.is_none() && app.overlay.is_none() {
        app.quit = true;
        return;
    }
    // Errors stay until the next key.
    if app.toast.as_ref().is_some_and(|t| t.kind == crate::app::ToastKind::Error) {
        app.toast = None;
        if k.code == KeyCode::Esc {
            return;
        }
    }

    // Confirmations (`A y` accept all, `R y` rewind): any other key says nothing changed.
    let plain_char = matches!(k.code, KeyCode::Char(_)) && !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    if let Some((prefix, id)) = app.awaiting.take().filter(|_| plain_char) {
        match (prefix, k.code) {
            ('A', KeyCode::Char('y')) => app.review_accept_all(),
            ('W', KeyCode::Char('y')) => app.rewind_to(&id),
            _ => app.info("nothing changed"),
        }
        return;
    }

    if let Some(mut ov) = app.overlay.take() {
        match &mut ov {
            Overlay::Help { all, scroll } => {
                // ↑↓ PgUp PgDn (and j k) scroll keys that don't fit; ? shows every key; any other
                // key closes.
                let step = match k.code {
                    KeyCode::Down | KeyCode::Char('j') => Some(1i32),
                    KeyCode::Up | KeyCode::Char('k') => Some(-1),
                    KeyCode::PageDown | KeyCode::Char(' ') => Some(10),
                    KeyCode::PageUp => Some(-10),
                    _ => None,
                };
                if let Some(d) = step {
                    *scroll = (*scroll as i32 + d).clamp(0, app.render.help_max_scroll as i32) as u16;
                    app.overlay = Some(ov);
                } else if k.code == KeyCode::Char('?') && !*all {
                    *all = true;
                    *scroll = 0;
                    app.overlay = Some(ov);
                }
                return;
            }
            Overlay::About(a) => {
                if crate::about::key(app, a, k) {
                    app.overlay = Some(ov);
                }
                return;
            }
            Overlay::Focus => {
                use thc_core::tui_config::{El, PRESETS};
                if k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                    // ⌃C closes; other chords do nothing (D4).
                    if !(ctrl && k.code == KeyCode::Char('c')) {
                        app.overlay = Some(ov);
                    }
                    return;
                }
                match k.code {
                    KeyCode::Esc => return,
                    KeyCode::Enter => {
                        app.save_focus();
                        return;
                    }
                    KeyCode::Char(c @ '1'..='3') => app.focus_cfg.use_preset(PRESETS[(c as u8 - b'1') as usize]),
                    KeyCode::Char(c) => {
                        if let Some(e) = El::from_letter(c) {
                            app.focus_cfg.toggle(e);
                        }
                    }
                    _ => {}
                }
                app.overlay = Some(ov);
                return;
            }
            Overlay::Vaults { rows, sel, naming } => {
                // `n`: a name for a new vault, then it's made and opened.
                if let Some(input) = naming {
                    match input.handle(k) {
                        InputResult::Cancel => *naming = None,
                        InputResult::Submit => {
                            let name = input.buf.trim().to_string();
                            let mut reg = thc_core::registry::Registry::load();
                            match thc_core::registry::create_vault(&mut reg, &name, None) {
                                Ok((e, _)) => {
                                    app.switch_to = Some(e.path);
                                    return;
                                }
                                Err(err) => app.error(format!("{err:#}")),
                            }
                        }
                        _ => {}
                    }
                    app.overlay = Some(ov);
                    return;
                }
                match k.code {
                    KeyCode::Down | KeyCode::Char('j') => *sel = (*sel + 1).min(rows.len().saturating_sub(1)),
                    KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                    KeyCode::Char('n') => *naming = Some(LineInput::default()),
                    KeyCode::Esc => return,
                    KeyCode::Enter => {
                        if let Some(r) = rows.get(*sel) {
                            if !r.current {
                                app.switch_to = Some(r.path.clone());
                            }
                        }
                        return;
                    }
                    _ => {}
                }
                app.overlay = Some(ov);
                return;
            }
            Overlay::Recipe { name } => {
                let name = name.clone();
                let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                match k.code {
                    // `e`: edit it in $EDITOR, a section per line (view-explain.md §3).
                    KeyCode::Char('e') => app.editor_request = Some(format!("@view:{name}")),
                    KeyCode::Char('c') if ctrl => {
                        let text = app.recipe(&name).map(|r| r.view.sections.iter().map(|s| format!("{} · {}", s.title, s.query)).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                        crate::runtime_effects::dispatch(app, crate::update::Msg::Copy { text, notice: format!("copied @{name}'s queries") });
                    }
                    KeyCode::Char('c') => app.prompt = Some((PromptKind::ViewCopy(name.clone()), LineInput::default())),
                    KeyCode::Char('r') => app.reset_view(&name),
                    KeyCode::Char('*') => {
                        let picked = app.scope_vaults();
                        app.overlay = Some(Overlay::Scope { sel: 0, picked });
                    }
                    KeyCode::Char('?') => app.overlay = Some(Overlay::Help { all: false, scroll: 0 }),
                    _ => {}
                }
                return;
            }
            Overlay::Scope { sel, picked } => {
                // Rows: 0 all, 1 this vault, then each vault (view-explain.md §2).
                let choices = app.scope_choices();
                let n = choices.len() + 2;
                let here = app.vault_name.clone();
                let toggle = |sel: usize, picked: &mut Vec<String>| match sel {
                    0 => *picked = choices.clone(),
                    1 => *picked = vec![here.clone()],
                    i => {
                        if let Some(name) = choices.get(i - 2) {
                            if let Some(p) = picked.iter().position(|x| x == name) {
                                if picked.len() > 1 {
                                    picked.remove(p);
                                }
                            } else {
                                picked.push(name.clone());
                            }
                        }
                    }
                };
                match k.code {
                    KeyCode::Down | KeyCode::Char('j') => *sel = (*sel + 1).min(n - 1),
                    KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                    KeyCode::Char('a') => toggle(0, picked),
                    KeyCode::Char('.') => toggle(1, picked),
                    KeyCode::Char(' ') => toggle(*sel, picked),
                    KeyCode::Enter => {
                        let s = app.scope_of(picked);
                        app.set_scope(s);
                        return;
                    }
                    KeyCode::Char('s') => {
                        let s = app.scope_of(picked);
                        app.save_scope_default(s);
                        return;
                    }
                    KeyCode::Esc => return,
                    _ => {}
                }
                app.overlay = Some(ov);
                return;
            }
            Overlay::History { sel } => {
                let n = app.history.entries.len();
                match k.code {
                    KeyCode::Down | KeyCode::Char('j') => *sel = (*sel + 1).min(n.saturating_sub(1)),
                    KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                    KeyCode::Enter => {
                        // Newest first on screen.
                        if let Some(i) = n.checked_sub(1 + *sel) {
                            app.history_jump_to(i);
                        }
                        return;
                    }
                    _ if k.code == KeyCode::Esc => return,
                    _ => {}
                }
                app.overlay = Some(ov);
                return;
            }
            Overlay::Finder { input, sel } => match k.code {
                KeyCode::Down => *sel += 1,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Char('n') if ctrl => *sel += 1,
                KeyCode::Char('p') if ctrl => *sel = sel.saturating_sub(1),
                KeyCode::Char('o') if ctrl => return,
                _ => match input.handle(k) {
                    InputResult::Cancel => return,
                    InputResult::Submit => {
                        let m = app.finder_matches(&input.buf);
                        if let Some((_, g)) = m.get((*sel).min(m.len().saturating_sub(1))).cloned() {
                            app.finder_go(g);
                        }
                        return;
                    }
                    InputResult::Changed => *sel = 0,
                    InputResult::Ignored => {}
                },
            },
            Overlay::Palette { input, sel } => match k.code {
                KeyCode::Down => *sel += 1,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Char('n') if ctrl => *sel += 1,
                KeyCode::Char('p') if ctrl => *sel = sel.saturating_sub(1),
                KeyCode::Tab => {
                    let m = palette_matches(app, &input.buf);
                    if let Some(e) = m.get((*sel).min(m.len().saturating_sub(1))) {
                        *input = LineInput::with(&e.label);
                    }
                }
                _ => match input.handle(k) {
                    InputResult::Cancel => return,
                    InputResult::Submit => {
                        // `view add <name>` / `view save <name>` save the current Tasks filter.
                        let buf = input.buf.trim().to_string();
                        if crate::sidebar_app::palette(app, &buf) {
                            return;
                        }
                        if buf == "focus" || buf.starts_with("focus ") {
                            app.focus_command(&buf["focus".len()..]);
                            return;
                        }
                        if buf == "mouse" || buf.starts_with("mouse ") {
                            app.mouse_request = Some(match buf["mouse".len()..].trim() {
                                "on" => true,
                                "off" => false,
                                _ => !app.tui_prefs.mouse,
                            });
                            return;
                        }
                        if let Some(name) = buf.strip_prefix("context ") {
                            let n = name.trim().trim_start_matches('@');
                            app.set_context(if n == "none" || n == "off" { None } else { Some(n) });
                            return;
                        }
                        for (prefix, add) in [("view add ", true), ("view save ", false)] {
                            if let Some(name) = buf.strip_prefix(prefix) {
                                app.save_view(name.trim(), add);
                                return;
                            }
                        }
                        let matches = palette_matches(app, &input.buf);
                        if let Some(e) = matches.get((*sel).min(matches.len().saturating_sub(1))) {
                            run_palette(app, &e.keys.clone());
                        }
                        return;
                    }
                    InputResult::Changed => *sel = 0,
                    InputResult::Ignored => {}
                },
            },
            Overlay::Capture { input, targets, which } => match k.code {
                KeyCode::Tab => *which = (*which + 1) % targets.len().max(1),
                KeyCode::BackTab => *which = (*which + targets.len() - 1) % targets.len().max(1),
                _ => match input.handle(k) {
                    InputResult::Cancel => return,
                    InputResult::Submit => {
                        let text = input.buf.trim().to_string();
                        if text.is_empty() {
                            return;
                        }
                        let target = targets[*which].clone();
                        if let Err(e) = app.capture(&text, &target) {
                            app.error(crate::app::friendly_error(&e));
                            app.overlay = Some(ov);
                        }
                        return;
                    }
                    InputResult::Changed => {
                        // Editing after a refusal clears it; the preview takes the bar back.
                        if app.toast.as_ref().is_some_and(|t| t.kind == crate::app::ToastKind::Error) {
                            app.toast = None;
                        }
                    }
                    _ => {}
                },
            },
            // Plain keys only: ⌃B resolved "both" before (D4).
            Overlay::Compare { .. } if k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) && k.code != KeyCode::Esc => {
                if ctrl && k.code == KeyCode::Char('c') {
                    return;
                }
                app.overlay = Some(ov);
                return;
            }
            Overlay::Compare { detail } => {
                let node = detail.node.clone();
                if detail.kind == "rehomed" {
                    match k.code {
                        KeyCode::Char('1') => app.keep_rehomed(&detail),
                        KeyCode::Char('2') => app.delete_rehomed(&detail),
                        KeyCode::Esc => {}
                        _ => app.overlay = Some(ov),
                    }
                    return;
                }
                if detail.kind == "move" {
                    match k.code {
                        KeyCode::Enter => app.dismiss_move(&node),
                        KeyCode::Esc => {}
                        _ => app.overlay = Some(ov),
                    }
                    return;
                }
                match k.code {
                    KeyCode::Char('1') => app.resolve_conflict(&node, if app.doc.is_some() { "yours" } else { "current" }),
                    KeyCode::Char('2') => app.resolve_conflict(&node, if app.doc.is_some() { "theirs" } else { "other" }),
                    KeyCode::Char('b') => app.resolve_conflict(&node, "both"),
                    KeyCode::Char('e') => app.editor_request = Some(node),
                    KeyCode::Esc => {}
                    _ => app.overlay = Some(ov),
                }
                return;
            }
            Overlay::Move { node, input, sel } => {
                // Digits pick a recent destination when the query is empty.
                if let KeyCode::Char(c @ '1'..='3') = k.code {
                    if input.buf.is_empty() {
                        if let Some(t) = app.recent_moves.get((c as u8 - b'1') as usize).cloned() {
                            let node = node.clone();
                            app.do_move(&node, t);
                            return;
                        }
                    }
                }
                match k.code {
                    KeyCode::Down | KeyCode::Tab => *sel += 1,
                    KeyCode::Up | KeyCode::BackTab => *sel = sel.saturating_sub(1),
                    _ => match input.handle(k) {
                        InputResult::Cancel => return,
                        InputResult::Submit => {
                            let items: Vec<_> = app.move_items(&input.buf).into_iter().flatten().collect();
                            if let Some(it) = items.get((*sel).min(items.len().saturating_sub(1))) {
                                let node = node.clone();
                                app.do_move(&node, it.target.clone());
                            }
                            return;
                        }
                        InputResult::Changed => *sel = 0,
                        InputResult::Ignored => {}
                    },
                }
            }
        }
        app.overlay = Some(ov);
        return;
    }

    if let Some((kind, mut input)) = app.prompt.take() {
        // The finders (Pages, Search: navigation.md §6): Tab and ⇧Tab always cycle views, the
        // query kept; with the query empty, digits, ?, :, space and q are commands again, and
        // Esc goes back.
        if matches!(kind, PromptKind::PagesFilter | PromptKind::Search) {
            let empty = input.buf.is_empty();
            // (A ⌘ chord is never text, and ⌃⌥← / ⌃⌥→ are history: they go to the keymap.)
            let falls_through = matches!(k.code, KeyCode::Tab | KeyCode::BackTab)
                || k.modifiers.contains(KeyModifiers::SUPER)
                || (ctrl && alt && matches!(k.code, KeyCode::Left | KeyCode::Right))
                || (empty && !ctrl && !alt && matches!(k.code, KeyCode::Char('1'..='9' | '?' | ':' | ' ' | 'q')))
                || (empty && k.code == KeyCode::Esc);
            if falls_through {
                crate::keymap::dispatch(app, &k);
                return;
            }
            if k.code == KeyCode::Esc {
                // Esc with a query clears it and puts the caret away; the selection stays.
                if kind == PromptKind::PagesFilter {
                    app.pages_filter.clear();
                } else {
                    app.search_terms.clear();
                }
                let _ = app.reload();
                return;
            }
        }
        // Inline inputs (Pages finder, Search, Tasks filter) still move the list with arrows.
        if kind.inline() {
            let mv = match k.code {
                KeyCode::Down => Some(1),
                KeyCode::Up => Some(-1),
                KeyCode::Char('n') if ctrl => Some(1),
                KeyCode::Char('p') if ctrl => Some(-1),
                _ => None,
            };
            if let Some(d) = mv {
                app.move_cursor(d);
                app.prompt = Some((kind, input));
                return;
            }
        }
        if kind == PromptKind::Filter {
            // Tab applies the suggested fix for a bad token.
            if k.code == KeyCode::Tab {
                if let Some((_, bad, Some(fix))) = app.tasks_error.clone() {
                    let fixed = input.buf.replacen(&bad, &fix, 1);
                    app.prompt = Some((kind, LineInput::with(&fixed)));
                    return;
                }
            }
            // Saved views by slot: digits only while the input is empty (§6.4 "Text inputs").
            if let KeyCode::Char(c @ '1'..='9') = k.code {
                if input.buf.is_empty() || app.input_untouched {
                    let slot = c as u32 - '0' as u32;
                    if let Some((_, v)) = app.view_slots().into_iter().find(|(t, _)| *t == slot) {
                        app.prompt = None;
                        app.use_view(&v.name);
                        return;
                    }
                }
            }
            app.input_untouched = false;
        }
        // The Tasks filter: Esc closes it, the query as it was (navigation.md §6, N15).
        if kind == PromptKind::Filter && k.code == KeyCode::Esc {
            return;
        }
        // Esc in an inline input clears the text first, then leaves the input.
        if kind.inline() && k.code == KeyCode::Esc && !input.buf.is_empty() {
            match kind {
                PromptKind::Filter => {
                    app.submit_prompt(PromptKind::Filter, String::new());
                }
                PromptKind::PagesFilter => {
                    app.pages_filter.clear();
                    let _ = app.reload();
                }
                PromptKind::Search => {
                    app.search_terms.clear();
                    let _ = app.reload();
                }
                _ => {}
            }
            app.prompt = Some((kind, LineInput::default()));
            return;
        }
        match input.handle(k) {
            InputResult::Cancel => {}
            InputResult::Submit => app.submit_prompt(kind, input.buf.clone()),
            _ => {
                if kind == PromptKind::PagesFilter {
                    app.pages_filter = input.buf.clone();
                    app.selected = None;
                    let _ = app.reload();
                }
                if kind == PromptKind::Search && input.buf.chars().count() >= 2 {
                    app.search_terms = input.buf.clone();
                    app.selected = None;
                    let _ = app.reload();
                }
                if k.code == KeyCode::Char('q') && ctrl && kind == PromptKind::Search {
                    app.tasks_filter = format!("text:{}", input.buf);
                    app.set_view(View::Tasks);
                    return;
                }
                app.prompt = Some((kind, input));
            }
        }
        return;
    }

    // Pages and Search are finders (navigation.md §6): arriving never takes the cursor, and a
    // letter starts the find with that letter (`/` an empty one). With the kept query empty,
    // digits, ?, :, space and q stay commands; once it has text every printable key types.
    let finder = app.doc.is_none() && app.overlay.is_none() && ((app.view == View::Pages && app.page_open.is_none()) || app.view == View::Search);
    // (A ⌘ chord is never a letter typed: ⌘[ is history, not a find for "[".)
    if finder && !ctrl && !alt && !k.modifiers.contains(KeyModifiers::SUPER) {
        if let KeyCode::Char(c) = k.code {
            let (kind, kept) = if app.view == View::Pages { (PromptKind::PagesFilter, app.pages_filter.clone()) } else { (PromptKind::Search, app.search_terms.clone()) };
            let command = kept.is_empty() && matches!(c, '1'..='9' | '?' | ':' | ' ' | 'q');
            if !command {
                let buf = if c == '/' && kept.is_empty() { String::new() } else { format!("{kept}{c}") };
                if kind == PromptKind::PagesFilter {
                    app.pages_filter = buf.clone();
                } else if buf.chars().count() >= 2 {
                    app.search_terms = buf.clone();
                }
                app.selected = None;
                let _ = app.reload();
                app.prompt = Some((kind, LineInput::with(&buf)));
                return;
            }
        }
    }
    // Everything else is the keymap (keymap.rs): the toast, the view, the list, then global.
    crate::keymap::dispatch(app, &k);
}

fn run_palette(app: &mut App, id: &str) {
    app.recent_cmds.retain(|k| k != id);
    app.recent_cmds.insert(0, id.to_string());
    app.recent_cmds.truncate(3);
    if let Some(name) = id.strip_prefix('@') {
        app.use_view(name);
        return;
    }
    if let Some(name) = id.strip_prefix("vault:") {
        match thc_core::registry::Registry::load().find(name) {
            Some(e) => app.switch_to = Some(e.path.clone()),
            None => app.error(format!("no vault \"{name}\" · thc vault ls")),
        }
        return;
    }
    if let Some(name) = id.strip_prefix("ctx:") {
        app.set_context(if name == "none" { None } else { Some(name) });
        return;
    }
    // An editing command (caretline's catalog): on the document, as its key would.
    if let Some(action) = id.strip_prefix("edit:") {
        crate::doc_keys::run_write(app, action);
        return;
    }
    // An entry runs its action (keymap.md §4), never replays a key: from Write it acts on the
    // caret's line (saved and selected) and leaves the caret where it was.
    if app.doc.is_some() {
        app.save_doc(true);
        app.doc_select_line();
    }
    match id.strip_prefix("prefix:") {
        // A prefix (`p`): its continuations wait for the next key, even over a document.
        Some(k) => {
            if let Some(key) = crate::keymap::Key::parse(k) {
                crate::keymap::start_prefix(app, vec![key]);
            }
        }
        None => {
            crate::keymap::run(app, id);
        }
    }
}
