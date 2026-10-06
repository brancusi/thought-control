//! Keys inside a document (tui-editor.md §2, §4): Write by default, Esc to Navigate.

use crate::app::App;
use crate::doc::{Pos, Target};
use crate::motion::Motion;
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
    if app.doc_parked {
        let plain = !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        if plain && matches!(k.code, KeyCode::Tab | KeyCode::BackTab) && !app.link_open {
            app.save_doc(true);
            crate::keymap::run(app, if k.code == KeyCode::Tab { "view.next" } else { "view.prev" });
            return true;
        }
        app.doc_parked = false;
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
    if app.link_open {
        if let Some((_, q)) = app.link_query() {
            let (m, create) = app.link_matches(&q);
            let total = m.len() + create.is_some() as usize;
            match k.code {
                KeyCode::Down | KeyCode::Char('n') if k.code == KeyCode::Down || ctrl => {
                    app.link_sel = Some(app.link_sel.map(|s| (s + 1).min(total.saturating_sub(1))).unwrap_or(0));
                    return true;
                }
                KeyCode::Up | KeyCode::Char('p') if k.code == KeyCode::Up || ctrl => {
                    app.link_sel = app.link_sel.and_then(|s| s.checked_sub(1)).or(if m.is_empty() { None } else { Some(0) });
                    return true;
                }
                KeyCode::Enter | KeyCode::Tab => {
                    match app.link_sel {
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
                    app.link_open = false;
                    return true;
                }
                _ => {}
            }
        } else {
            app.link_open = false;
        }
    }
    // Typing on a ≠ line opens the compare first: text writes to a conflicted node are refused.
    if matches!(k.code, KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete | KeyCode::Enter) && !ctrl && !alt {
        let d = app.doc.as_ref().unwrap();
        if d.line().conflict {
            app.selected = Some(d.line().id.clone());
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
            let mut buf = [0u8; 4];
            crate::runtime_effects::type_text(app, c.encode_utf8(&mut buf));
            // `[[` opens the link popup; the cursor starts on the first match.
            if c == '[' && app.link_query().is_some_and(|(_, q)| q.is_empty()) {
                app.link_open = true;
                app.link_sel = Some(0);
            } else if app.link_open {
                let (m, _) = app.link_query().map(|(_, q)| app.link_matches(&q)).unwrap_or_default();
                app.link_sel = if m.is_empty() { None } else { Some(app.link_sel.unwrap_or(0).min(m.len() - 1)) };
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
    // The view's height (last frame's), less two rows of context: a page (motion.md §4).
    let page = app.render.doc_view_rows.saturating_sub(2).max(1);
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
                d.view.anchor = None;
                return true;
            }
            app.leave_doc();
        }
        "doc.open" => app.doc_open(),
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
        "doc.undo" if app.last_drop.as_ref().is_some_and(|(id, _, depth)| d.undo_depth() == *depth && d.lines().iter().any(|l| &l.id == id)) => {
            let (id, raw, _) = app.last_drop.take().unwrap();
            let d = app.doc.as_mut().unwrap();
            if let Some(i) = d.lines().iter().position(|l| l.id == id) {
                d.lines_mut()[i].text = raw.trim().to_string();
                d.view.caret = Pos { line: i, byte: d.lines()[i].text.len() };
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
        "line.soft_break" if crate::doc_ui::image_line(&d.line().text).is_some() => {
            d.view.caret.byte = d.line().text.len();
            d.newline();
        }
        // Enter on an attachment's line starts a new line after it; it never opens it (that's ⌃O
        // or a double-click: editing.md §7).
        "line.newline" if d.selection().is_none() && crate::doc_ui::image_line(&d.line().text).is_some() => {
            d.view.caret.byte = d.line().text.len();
            d.newline();
        }
        "focus.toggle" => app.set_focus_mode(!app.focus_mode),
        "clip.paste_system" => app.paste_system(),
        // Editing and motion: caretline commands, applied to the document.
        other if command_for(other, shift, page as isize).is_some() => {
            crate::runtime_effects::edit(app, command_for(other, shift, page as isize).unwrap());
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
    app.doc_parked = false;
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
    if !app.doc_write {
        app.doc_write = true;
    }
    // One line: typed in as is, less what can't show in a line (a tab, a stray CR).
    if !text.contains('\n') && !text.contains('\r') {
        crate::runtime_effects::type_text(app, &text.replace('\t', "    "));
        app.doc_after_key();
        return;
    }
    let (lines, images) = crate::doc::parse_paste(text, app.paste_plain);
    app.paste_plain = false;
    let n = lines.len();
    d.paste_lines(lines);
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
    match m.kind {
        // The wheel scrolls the view and never moves the caret; a key brings the view back.
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let rows = app.tui_prefs.wheel_rows.max(1);
            let d = app.doc.as_mut().unwrap();
            d.scroll = if m.kind == MouseEventKind::ScrollUp { d.scroll.saturating_sub(rows) } else { d.scroll + rows };
            app.doc_scroll_free = true;
            true
        }
        MouseEventKind::Down(button @ (MouseButton::Left | MouseButton::Middle)) => {
            // The ≠ mark opens the compare (mouse.md §3).
            if let Some(line) = crate::doc_ui::conflict_mark_at(app, m.column, m.row) {
                let id = app.doc.as_ref().unwrap().lines()[line].id.clone();
                app.selected = Some(id);
                app.open_compare();
                return true;
            }
            let Some((line, byte, hang)) = crate::doc_ui::hit(app, m.column, m.row) else { return false };
            app.doc_parked = false;
            app.doc_scroll_free = false;
            app.link_open = false;
            app.click_link = None;
            let d = app.doc.as_mut().unwrap();
            // The task box is a button: open ⇄ done.
            if hang && button == MouseButton::Left && clicks == 1 && !shift && d.lines()[line].kind == Kind::Task {
                d.view.anchor = None;
                d.view.caret = Pos { line, byte: 0 };
                if d.task_box(line) == "done" {
                    app.save_doc(true);
                }
                app.doc_after_key();
                return true;
            }
            let p = Pos { line, byte };
            match clicks {
                2 => d.select_word(p),
                3 => d.select_note(line),
                _ => {
                    if shift {
                        if d.view.anchor.is_none() {
                            d.view.anchor = Some(d.view.caret);
                        }
                    } else {
                        d.view.anchor = None;
                    }
                    d.view.caret = p;
                    d.view.goal = None;
                }
            }
            app.drag_from = (clicks == 1 && !shift).then_some(p);
            // A double-click on an attachment opens it; a single click only puts the caret there
            // (editing.md §6-7: a click to move around used to open Preview).
            if clicks == 2 && !shift && button == MouseButton::Left {
                if let Some((_, path)) = crate::doc_ui::image_line(&app.doc.as_ref().unwrap().lines()[line].text) {
                    app.drag_from = None;
                    app.open_attachment(&path);
                    app.doc_after_key();
                    return true;
                }
            }
            // ⌃-click or middle-click on a link opens it (saving first).
            let on_link = crate::doc_app::link_at(&app.doc.as_ref().unwrap().lines()[line].text, byte).is_some();
            if on_link && (ctrl || button == MouseButton::Middle) {
                app.doc_after_key();
                app.doc_open();
                return true;
            }
            // A plain click on a link's title follows it when released (E63); on the `[[` / `]]`,
            // or with ⌥, it only places the caret (E64, E65).
            if clicks == 1 && !shift && !alt && button == MouseButton::Left && crate::doc_app::on_link_title(&app.doc.as_ref().unwrap().lines()[line].text, byte) {
                app.click_link = Some(p);
            }
            app.doc_after_key();
            true
        }
        // A drag selects by grapheme across rows, lines and notes.
        MouseEventKind::Drag(MouseButton::Left) => {
            let (Some(from), Some((line, byte, _))) = (app.drag_from, crate::doc_ui::hit(app, m.column, m.row)) else { return app.drag_from.is_some() };
            let d = app.doc.as_mut().unwrap();
            d.view.anchor = Some(from);
            d.view.caret = Pos { line, byte };
            d.view.goal = None;
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let dragged = app.drag_from.take().is_some();
            if let Some(p) = app.click_link.take() {
                let d = app.doc.as_ref().unwrap();
                if d.view.caret == p && d.view.anchor.is_none_or(|a| a == p) {
                    app.doc.as_mut().unwrap().view.anchor = None;
                    app.doc_after_key();
                    app.doc_open();
                    return true;
                }
            }
            let mut selected = false;
            if let Some(d) = app.doc.as_mut() {
                if d.view.anchor == Some(d.view.caret) {
                    d.view.anchor = None;
                }
                selected = d.view.anchor.is_some();
            }
            if dragged && selected {
                native_selection_hint(app);
            }
            app.doc_after_key();
            true
        }
        _ => false,
    }
}

/// Once per device, the first time a drag selects in thc: how to get the terminal's own
/// selection while thc has the mouse (mouse.md §7).
fn native_selection_hint(app: &mut App) {
    let flag = app.vault.paths.cache.join("mouse-hint-shown");
    if flag.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&app.vault.paths.cache);
    let _ = std::fs::write(&flag, "");
    let key = match std::env::var("TERM_PROGRAM").as_deref() {
        Ok("iTerm.app") | Ok("Apple_Terminal") => "⌥",
        _ => "⇧",
    };
    app.info(format!("selected in thc · ⌃C copies · {key}-drag for your terminal's own selection"));
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

/// The keymap's write actions that are caretline commands (editing and motion).
fn command_for(action: &str, shift: bool, page: isize) -> Option<caretline::Command> {
    use caretline::Command as C;
    let mv = |motion| Some(C::Move { motion, select: shift });
    match action {
        "line.newline" => Some(C::Newline),
        "line.soft_break" => Some(C::SoftBreak),
        "line.indent" => Some(C::Indent),
        "line.outdent" => Some(C::Outdent),
        "line.move_up" => Some(C::MoveLine(-1)),
        "line.move_down" => Some(C::MoveLine(1)),
        "doc.task_cycle" => Some(C::TaskCycle),
        "doc.undo" => Some(C::Undo),
        "doc.redo" => Some(C::Redo),
        "select.all" => Some(C::SelectAll),
        "edit.backspace" => Some(C::Backspace),
        "edit.delete_word" => Some(C::DeleteWordBack),
        "edit.delete_forward" => Some(C::Delete),
        "edit.kill_to_end" => Some(C::KillToEnd),
        "edit.kill_to_start" => Some(C::KillToStart),
        "move.left" => mv(Motion::Left),
        "move.right" => mv(Motion::Right),
        "move.word_left" => mv(Motion::WordLeft),
        "move.word_right" => mv(Motion::WordRight),
        "move.up" => mv(Motion::Up),
        "move.down" => mv(Motion::Down),
        "move.para_up" => mv(Motion::NoteUp),
        "move.para_down" => mv(Motion::NoteDown),
        "move.page_up" => mv(Motion::Page(-page)),
        "move.page_down" => mv(Motion::Page(page)),
        "move.home" => mv(Motion::Home),
        "move.end" => mv(Motion::End),
        "move.doc_start" => mv(Motion::DocStart),
        "move.doc_end" => mv(Motion::DocEnd),
        _ => None,
    }
}
