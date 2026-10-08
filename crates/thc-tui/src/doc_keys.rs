//! Keys inside a document (tui-editor.md §2, §4): Write by default, Esc to Navigate.

use crate::app::App;
use crate::editor::{BlockPos, Target};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use thc_core::outline::Kind;

/// Handle a key in the open document. Returns false to let the app's own keys run (Navigate
/// commands on the line's node, overlays, prompts).
pub fn handle(app: &mut App, k: KeyEvent) -> bool {
    if app.doc.is_none() || app.overlay.is_some() || app.prompt.is_some() || app.edit.is_some() {
        return false;
    }
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    // Parked (just arrived, navigation.md §6.1): Tab and ⇧Tab change views; every other key
    // starts writing and is never lost.
    if app.main.parked {
        let plain = !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        if plain && matches!(k.code, KeyCode::Tab | KeyCode::BackTab) && !app.main.link_open {
            app.save_doc(true);
            crate::keymap::run(app, if k.code == KeyCode::Tab { "view.next" } else { "view.prev" });
            return true;
        }
        app.main.parked = false;
    }
    // Write is sealed: a key it doesn't bind does nothing, never a list command (⌥X used to mark
    // the line done). An unbound ⌥ chord says once where commands are.
    // Documents are modeless (writing.md §3): always Write. Navigate is reached only by the
    // palette's replay of a command key on the caret's line (input.rs run_palette).
    if !write_key(app, k) && alt && !app.write_alt_hint {
        app.write_alt_hint = true;
        if let KeyCode::Char(c) = k.code {
            app.info(format!("⌥{} isn't a writing key · F1 shows the keys", c.to_uppercase()));
        }
    }
    let consumed = true;
    if consumed {
        app.doc_after_key();
    }
    consumed
}

fn write_key(app: &mut App, k: KeyEvent) -> bool {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    // ⌥ arriving as a composed character (⌥Z → Ω on a Mac without "Option as Meta"): typed as
    // is, and once, how to get the ⌥ keys (jank B4).
    if let KeyCode::Char(c) = k.code {
        if cfg!(target_os = "macos") && !ctrl && !alt && !app.kitty && !app.meta_hint && "åΩ∫ç∂ƒ©˙∆˚¬µøπœ®ß†√∑≈¥¡™£¢∞§¶•ªº–≠÷".contains(c) {
            app.meta_hint = true;
            app.info("turn on \"Option as Meta\" in your terminal for ⌥ keys · F1 keys");
        }
    }
    // The `[[` popup takes ↑ ↓ ⌃N ⌃P Enter Tab Esc while it's open.
    if app.main.link_open {
        if let Some((_, q)) = app.link_query() {
            let (m, create) = app.link_matches(&q);
            let total = m.len() + create.is_some() as usize;
            match k.code {
                KeyCode::Down | KeyCode::Char('n') if k.code == KeyCode::Down || ctrl => {
                    app.main.link_sel = Some(app.main.link_sel.map(|s| (s + 1).min(total.saturating_sub(1))).unwrap_or(0));
                    return true;
                }
                KeyCode::Up | KeyCode::Char('p') if k.code == KeyCode::Up || ctrl => {
                    app.main.link_sel = app.main.link_sel.and_then(|s| s.checked_sub(1)).or(if m.is_empty() { None } else { Some(0) });
                    return true;
                }
                KeyCode::Enter | KeyCode::Tab => {
                    match app.main.link_sel {
                        Some(i) if i < m.len() => app.link_insert(&m[i].1.clone()),
                        Some(_) if create.is_some() => app.link_insert(&create.clone().unwrap()),
                        _ => app.info(match &create {
                            Some(c) => format!("↓ then Enter creates ¶ {c}"),
                            None => "no page matches · keep typing".into(),
                        }),
                    }
                    return true;
                }
                KeyCode::Esc => {
                    app.main.link_open = false;
                    return true;
                }
                _ => {}
            }
        } else {
            app.main.link_open = false;
        }
    }
    // Typing on a ≠ line opens the compare first: text writes to a conflicted node are refused.
    if matches!(k.code, KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete | KeyCode::Enter) && !ctrl && !alt {
        let d = app.doc.as_ref().unwrap();
        if d.caret_block().conflict {
            app.selected = Some(d.caret_block().id.clone());
            app.open_compare();
            return true;
        }
    }
    // The table's `write` context (keymap.rs), exactly matched; then a printable key types;
    // anything else does nothing (sealed: writing.md §3, keymap.md §3.2).
    let key = crate::keymap::Key::of(&k);
    if let Some(Some(b)) = crate::keymap::lookup(app, &[crate::keymap::Ctx::Write], &[key]) {
        return write_action(app, b.action, shift);
    }
    if let KeyCode::Char(c) = k.code {
        if !ctrl && !alt && !k.modifiers.contains(KeyModifiers::SUPER) {
            crate::runtime_effects::dispatch(app, crate::update::Msg::Type { text: c.to_string() });
            // `[[` opens the link popup; the cursor starts on the first match.
            if c == '[' && app.link_query().is_some_and(|(_, q)| q.is_empty()) {
                app.main.link_open = true;
                app.main.link_sel = Some(0);
            } else if app.main.link_open {
                let (m, _) = app.link_query().map(|(_, q)| app.link_matches(&q)).unwrap_or_default();
                app.main.link_sel = if m.is_empty() { None } else { Some(app.main.link_sel.unwrap_or(0).min(m.len() - 1)) };
            }
            return true;
        }
    }
    false
}

/// A `write` action by ID (a footer click): false when it isn't one.
pub fn run_write(app: &mut App, action: &str) -> bool {
    if app.doc.is_none() || !crate::keymap::table().iter().any(|b| b.ctx == crate::keymap::Ctx::Write && b.action == action) {
        return false;
    }
    let r = write_action(app, action, false);
    app.doc_after_key();
    r
}

/// A `write` action from the table, on the open document. `shift`: a move extends the selection.
fn write_action(app: &mut App, action: &str, shift: bool) -> bool {
    // ⌘↑ ⌘↓ are big jumps: history keeps where the caret was (navigation.md §7.1).
    if !shift && matches!(action, "move.doc_start" | "move.doc_end") {
        let before = app.place();
        let r = write_action_inner(app, action, shift);
        app.history_jumped(before);
        return r;
    }
    write_action_inner(app, action, shift)
}

fn write_action_inner(app: &mut App, action: &str, shift: bool) -> bool {
    // ⌥V: an image on the clipboard is attached at the caret (attachments.md §2); otherwise the
    // next paste is plain text, as before.
    if action == "paste.plain_next" && app.attach_clipboard_image() {
        return true;
    }
    let d = app.doc.as_mut().unwrap();
    match action {
        // Esc: a selection clears; otherwise save and go back where the document was opened
        // from (navigation.md §2).
        "doc.done" => {
            if d.selection().is_some() {
                d.clear_selection();
                return true;
            }
            app.leave_doc();
        }
        "doc.open" => app.doc_open(),
        // In a day panel: that panel goes to the day before or after (sidebar.md §8.1).
        "doc.day_prev" | "doc.day_next" if app.in_panel.is_some() => {
            app.panel_defer.push(crate::sidebar_app::Deferred::Action(if action == "doc.day_prev" { "sidebar.day_prev" } else { "sidebar.day_next" }.into()));
        }
        "doc.day_prev" | "doc.day_next" => match d.target {
            Target::Journal { .. } => {
                app.save_doc(true);
                app.shift_journal(if action == "doc.day_prev" { -1 } else { 1 });
            }
            Target::Page { .. } => app.info("days are in the journal · ⌃O to go"),
        },
        // Lines an undo brings back may have changed elsewhere meanwhile: re-read them (fuzz:
        // a line deleted here, edited elsewhere, then undone, kept the old text and never saved).
        // The first ⌃Z after a drop: the attachment becomes the pasted path, as text (like undoing
        // an autocorrect); the next ⌃Z removes that.
        "doc.undo" if app.ui.main.last_drop.as_ref().is_some_and(|(id, _, depth)| d.undo_depth() == *depth && d.blocks().iter().any(|l| &l.id == id)) => {
            let (id, raw, _) = app.main.last_drop.take().unwrap();
            let d = app.doc.as_mut().unwrap();
            if let Some(i) = d.blocks().iter().position(|l| l.id == id) {
                let text = raw.trim();
                d.replace_content(&id, text);
                d.set_caret(BlockPos { line: i, byte: text.len() });
            }
            app.save_doc(true);
            app.info("kept the path as text · ⌃Z again removes it");
        }
        "clip.copy" => {
            // ⌃C copies the selection, nothing else (⌃Q quits).
            if d.selection().is_some() {
                copy(app);
            } else {
                app.info("nothing selected · ⌘A selects all");
            }
        }
        "clip.cut" => {
            if d.selection().is_some() {
                copy(app);
                app.doc.as_mut().unwrap().delete_selection();
            }
        }
        "clip.paste_hint" => app.info("paste with your terminal (⌘V)"),
        "paste.plain_next" => {
            app.paste_plain = !app.paste_plain;
            app.info(if app.paste_plain { "next paste is plain" } else { "next paste reads Markdown" });
        }
        // ⌃J on an attachment's line: a new line after it (a break inside it would break it).
        "line.soft_break" if crate::doc_ui::image_line(&d.caret_block().text).is_some() => {
            d.set_caret(BlockPos { line: d.caret().line, byte: d.caret_block().text.len() });
            d.newline();
        }
        // Enter on an attachment's line starts a new line after it; it never opens it (that's ⌃O
        // or a double-click: editing.md §7).
        "line.newline" if d.selection().is_none() && crate::doc_ui::image_line(&d.caret_block().text).is_some() => {
            d.set_caret(BlockPos { line: d.caret().line, byte: d.caret_block().text.len() });
            d.newline();
        }
        "focus.toggle" => {
            let on = !app.focus_mode;
            app.set_focus_mode(on);
        }
        "clip.paste_system" => app.paste_system(),
        // Editing and motion: caretline's commands (and thc's ⌃T), run on the document.
        other if crate::editing_keys::command_for(other, shift).is_some() => {
            let command = crate::editing_keys::command_for(other, shift).unwrap().to_string();
            crate::runtime_effects::dispatch(app, crate::update::Msg::Editor { command, at: app.ui.now_ms });
        }
        // The views, help, the palette, quit, today: global actions bound in write; save first.
        other => {
            app.save_doc(true);
            return crate::keymap::run(app, other);
        }
    }
    true
}

pub fn copy(app: &mut App) {
    let d = app.doc.as_mut().unwrap();
    let notes = d.selected_parts().len();
    let md = d.copy_text();
    // editing.md §5: `copied 14 chars` inside a note, `copied 3 notes` across notes.
    let what = if notes > 1 { format!("{notes} notes") } else { format!("{} char{}", md.chars().count(), if md.chars().count() == 1 { "" } else { "s" }) };
    crate::runtime_effects::dispatch(app, crate::update::Msg::Copy { text: md, notice: format!("copied {what}") });
}

pub(crate) fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for chunk in b.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// A bracketed paste: Markdown as outline lines in one undo step, saved at once so tokens are
/// read like any saved line. Over 500 lines asks first (not yet: pastes that big say so).
pub fn paste(app: &mut App, text: &str) {
    app.clock_tick();
    app.main.parked = false;
    // A dropped file arrives as its path (attachments.md §2): attached at once, and the
    // first ⌃Z keeps the path instead. ⌥V first makes it plain text.
    if !app.paste_plain {
        if let Some(p) = dropped_file(text) {
            app.attach_dropped(&p, text);
            return;
        }
    }
    // ⌘V with only an image on the clipboard: terminals send an empty paste. Look for the image.
    if text.is_empty() {
        if !app.attach_clipboard_image() {
            app.info("nothing to paste");
        }
        return;
    }
    let Some(d) = app.doc.as_mut() else { return };
    if !app.ui.main.write {
        app.ui.main.write = true;
    }
    // One line: typed in as is, less what can't show in a line (a tab, a stray CR).
    if !text.contains('\n') && !text.contains('\r') {
        d.paste_line(&text.replace('\t', "    "));
        app.doc_after_key();
        return;
    }
    let (n, images) = d.paste(text, app.ui.paste_plain);
    app.paste_plain = false;
    app.save_doc(false);
    let mut msg = format!("pasted {n} line{}", if n == 1 { "" } else { "s" });
    if images > 0 {
        msg.push_str(&format!(" · {images} image{} left out", if images == 1 { "" } else { "s" }));
    }
    app.info(msg);
    app.doc_after_key();
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_encodes() {
        assert_eq!(super::base64(b"Man"), "TWFu");
        assert_eq!(super::base64(b"Ma"), "TWE=");
        assert_eq!(super::base64(b"- [ ] x"), "LSBbIF0geA==");
    }
}

/// The mouse in a document (mouse.md §3, §4). `clicks` counts presses in a row (2: a word, 3:
/// the note). Returns false when the event isn't the document's (an overlay, outside it).
pub fn mouse(app: &mut App, m: ratatui::crossterm::event::MouseEvent, clicks: u8) -> bool {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    if app.doc.is_none() || app.overlay.is_some() || app.prompt.is_some() {
        return false;
    }
    let shift = m.modifiers.contains(KeyModifiers::SHIFT);
    let ctrl = m.modifiers.contains(KeyModifiers::CONTROL);
    let alt = m.modifiers.contains(KeyModifiers::ALT);
    // ⌘ (super): no terminal puts it in a mouse report; it comes from WezTerm's ⌘-release
    // (thc_keys.lua, read in lib.rs) or a client's `mods: "d"`.
    let cmd = m.modifiers.contains(KeyModifiers::SUPER);
    match m.kind {
        // The wheel scrolls the view and never moves the caret; a key brings the view back.
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let rows = app.tui_prefs.wheel_rows.max(1);
            let d = app.doc.as_mut().unwrap();
            d.scroll_view(if m.kind == MouseEventKind::ScrollUp { -(rows as isize) } else { rows as isize });
            true
        }
        MouseEventKind::Down(button @ (MouseButton::Left | MouseButton::Middle)) => {
            // The ≠ mark opens the compare (mouse.md §3).
            if let Some(line) = crate::doc_ui::conflict_mark_at(app, m.column, m.row) {
                let id = app.doc.as_ref().unwrap().blocks()[line].id.clone();
                app.selected = Some(id);
                app.open_compare();
                return true;
            }
            let Some((line, byte, hang)) = crate::doc_ui::hit(app, m.column, m.row) else { return false };
            // A link opens beside, in the sidebar (sidebar.md §2, §12; mouse.md): ⇧-click on its
            // title, or ⌘-, ⌃- or middle-click anywhere on it. The caret stays where it was.
            if clicks == 1 {
                let text = &app.doc.as_ref().unwrap().blocks()[line].text;
                let beside = match crate::doc_app::link_at(text, byte) {
                    Some(title) if button == MouseButton::Middle || ctrl || cmd => Some(title),
                    Some(title) if shift && crate::doc_app::on_link_title(text, byte) => Some(title),
                    _ => None,
                };
                if let Some(title) = beside {
                    app.open_aside_link(&title);
                    return true;
                }
            }
            app.main.parked = false;
            // The view doesn't move for a click: `hit` placed it in the view as drawn, which may
            // be scrolled away from the caret by the wheel. (Following the old caret first
            // scrolled back to it, and the click landed rows away from the pointer.)
            app.main.link_open = false;
            app.main.click_link = None;
            let d = app.doc.as_mut().unwrap();
            // The task box is a button: open ⇄ done.
            if hang && button == MouseButton::Left && clicks == 1 && !shift && d.blocks()[line].kind() == Kind::Task {
                d.select_range(None, BlockPos { line, byte: 0 });
                if d.task_box(line) == "done" {
                    app.save_doc(true);
                }
                app.doc_after_click();
                return true;
            }
            let p = BlockPos { line, byte };
            // A press on a link's title moves nothing yet (E63): released in place it follows the
            // link (with ⌘: beside, and the caret stays), dragged it selects from here. On the
            // `[[` / `]]`, or with ⌥, it places the caret (E64, E65). One frame, the page that
            // opens: the caret never flashes inside the link first.
            if clicks == 1 && !shift && !alt && button == MouseButton::Left && crate::doc_app::on_link_title(&d.blocks()[line].text, byte) {
                app.main.click_link = Some(p);
                app.main.drag_from = Some(p);
                return true;
            }
            match clicks {
                2 => d.select_word_at(p),
                3 => d.select_block(line),
                _ => d.click(p, shift),
            }
            app.main.drag_from = (clicks == 1 && !shift).then_some(p);
            // A double-click on an attachment opens it; a single click only puts the caret there
            // (editing.md §6-7: a click to move around used to open Preview).
            if clicks == 2 && !shift && button == MouseButton::Left {
                if let Some((_, path)) = crate::doc_ui::image_line(&app.doc.as_ref().unwrap().blocks()[line].text) {
                    app.main.drag_from = None;
                    app.open_attachment(&path);
                    app.doc_after_key();
                    return true;
                }
            }
            app.doc_after_click();
            true
        }
        // A drag selects by grapheme across rows, lines and notes.
        MouseEventKind::Drag(MouseButton::Left) => {
            let Some(from) = app.main.drag_from else { return false };
            let Some((vx, vy, vw, vh)) = app.render.doc_view else { return true };
            // A press on a link that moves off it is a drag, not a click (the link isn't
            // followed): the selection starts where it was pressed.
            if let Some(p) = app.main.click_link {
                if crate::doc_ui::hit(app, m.column, m.row).is_some_and(|(line, byte, _)| BlockPos { line, byte } == p) {
                    return true;
                }
                app.main.click_link = None;
                app.doc.as_mut().unwrap().click(from, false);
            }
            // The pointer as a cell of the view: above it is its first row, below it one past
            // its last. The engine extends the selection there, and on the first or last text
            // row scrolls a row to bring in more (the runtime repeats a held drag at an edge:
            // cmd_click::Pointer).
            let col = m.column.saturating_sub(vx).min(vw.saturating_sub(1));
            let row = if m.row < vy { 0 } else { (m.row - vy).min(vh) };
            app.doc.as_mut().unwrap().drag_to(col, row);
            app.ui.main.drag_at = Some((m.column, m.row));
            app.ui.main.drag_ms = app.ui.now_ms;
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let dragged = app.main.drag_from.take().is_some();
            app.ui.main.drag_at = None;
            if let Some(p) = app.main.click_link.take() {
                let title = crate::doc_app::link_at(&app.doc.as_ref().unwrap().blocks()[p.line].text, p.byte);
                match title {
                    // Released with ⌘: beside, and the caret stays where it was.
                    Some(title) if cmd => app.open_aside_link(&title),
                    _ => {
                        app.doc.as_mut().unwrap().click(p, false);
                        app.doc_after_click();
                        app.doc_open();
                    }
                }
                return true;
            }
            let mut selected = false;
            if let Some(d) = app.doc.as_mut() {
                if d.anchor() == Some(d.caret()) {
                    d.clear_selection();
                }
                selected = d.anchor().is_some();
            }
            if dragged && selected {
                native_selection_hint(app);
            }
            app.doc_after_click();
            true
        }
        _ => false,
    }
}

/// Once per device, the first time a drag selects in thc: how to get the terminal's own
/// selection while thc has the mouse (mouse.md §7).
fn native_selection_hint(app: &mut App) {
    let Some(key) = app.drag_hint.take() else { return };
    let _ = std::fs::create_dir_all(&app.vault.paths.cache);
    let _ = std::fs::write(app.vault.paths.cache.join("mouse-hint-shown"), "");
    app.info(format!("selected in thc · ⌃C copies · {key}-drag for your terminal's own selection"));
}

/// The first drag's hint, when this device hasn't shown it yet: the key the terminal's own
/// selection takes (`App::drag_hint`).
pub(crate) fn drag_hint_pending(cache: &std::path::Path) -> Option<String> {
    if cache.join("mouse-hint-shown").exists() {
        return None;
    }
    Some(match std::env::var("TERM_PROGRAM").as_deref() {
        Ok("iTerm.app") | Ok("Apple_Terminal") => "⌥",
        _ => "⇧",
    }.to_string())
}

/// A pasted single line that's the path of an existing file (how terminals deliver a dropped
/// file): quotes and backslash-escaped spaces undone.
fn dropped_file(text: &str) -> Option<std::path::PathBuf> {
    let t = text.trim();
    if t.contains('\n') || t.is_empty() {
        return None;
    }
    let t = t.trim_matches(|c| c == '\'' || c == '"');
    let unescaped = t.replace("\\ ", " ");
    let p = std::path::PathBuf::from(unescaped.strip_prefix("file://").unwrap_or(&unescaped));
    (p.is_absolute() && p.is_file()).then_some(p)
}

/// How far the held pointer is on or past the edge of the view it drags in (0: on its first or
/// last row; 1: a row past it…), or None: inside it, or no drag held.
pub fn drag_edge(app: &App) -> Option<u16> {
    app.main.drag_from?;
    let (_, y) = app.ui.main.drag_at?;
    let (vy, vh) = match app.panel_pointer.as_ref() {
        Some(k) => app.render.panel_views.iter().find(|(p, _)| p == k).map(|(_, r)| (r.y, r.height))?,
        None => app.render.doc_view.map(|(_, vy, _, vh)| (vy, vh))?,
    };
    if vh == 0 {
        return None;
    }
    let last = vy + vh - 1;
    if y <= vy {
        Some(vy - y)
    } else if y >= last {
        Some(y - last)
    } else {
        None
    }
}

/// The wait between repeats of a drag held on an edge: 50 ms on it, shorter the further past.
pub fn drag_repeat_ms(edge: u16) -> u64 {
    (50 / (1 + edge as u64)).max(10)
}
