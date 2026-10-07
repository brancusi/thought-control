//! The editing keys come from caretline: its command catalog and default keymap
//! (`caretline::commands`). thc translates each caretline command it runs to its own `write`
//! action and imports the bindings into its one key table (`keymap.rs`), so the editing keys,
//! `thc keys`, help and the palette are caretline's vocabulary and can't drift from it. thc's
//! own differences (⌃D, the system paste, Esc as done…) are explicit rows in `keymap.rs`, which
//! win over an imported row on the same key; user remaps (`thc keys --edit`) apply on top.

use crate::keymap::{Binding, Ctx, When};
use caretline::commands::{command, default_keymap, Category, CommandInfo};

/// caretline's command → thc's `write` action. A `select.*` command is its `move.*` action
/// with ⇧ (thc's moves select with ⇧ held).
pub const ACTIONS: &[(&str, &str)] = &[
    ("move.left", "move.left"),
    ("move.right", "move.right"),
    ("move.up", "move.up"),
    ("move.down", "move.down"),
    ("move.word_left", "move.word_left"),
    ("move.word_right", "move.word_right"),
    ("move.line_start", "move.home"),
    ("move.line_end", "move.end"),
    ("move.doc_start", "move.doc_start"),
    ("move.doc_end", "move.doc_end"),
    ("move.page_up", "move.page_up"),
    ("move.page_down", "move.page_down"),
    ("move.block_up", "move.para_up"),
    ("move.block_down", "move.para_down"),
    ("select.left", "move.left"),
    ("select.right", "move.right"),
    ("select.up", "move.up"),
    ("select.down", "move.down"),
    ("select.word_left", "move.word_left"),
    ("select.word_right", "move.word_right"),
    ("select.line_start", "move.home"),
    ("select.line_end", "move.end"),
    ("select.doc_start", "move.doc_start"),
    ("select.doc_end", "move.doc_end"),
    ("select.page_up", "move.page_up"),
    ("select.page_down", "move.page_down"),
    ("select.block_up", "move.para_up"),
    ("select.block_down", "move.para_down"),
    ("select.all", "select.all"),
    ("edit.newline", "line.newline"),
    ("edit.soft_break", "line.soft_break"),
    ("edit.backspace", "edit.backspace"),
    ("edit.delete_forward", "edit.delete_forward"),
    ("edit.delete_word", "edit.delete_word"),
    ("edit.delete_to_line_start", "edit.kill_to_start"),
    ("edit.kill_line", "edit.kill_to_end"),
    ("structure.indent", "line.indent"),
    ("structure.outdent", "line.outdent"),
    ("structure.move_up", "line.move_up"),
    ("structure.move_down", "line.move_down"),
    ("clip.copy", "clip.copy"),
    ("clip.cut", "clip.cut"),
    ("history.undo", "doc.undo"),
    ("history.redo", "doc.redo"),
];

/// thc's own editor commands (host commands registered on every document) → thc's `write`
/// action.
pub const HOST_ACTIONS: &[(&str, &str)] = &[(crate::editor::TASK_CYCLE, "doc.task_cycle")];

/// The command a `write` action runs on the document: caretline's (a move's `select.*` form
/// with ⇧) or one of thc's host commands. None: the action isn't an editor command.
pub fn command_for(action: &str, shift: bool) -> Option<&'static str> {
    if let Some((c, _)) = HOST_ACTIONS.iter().find(|(_, a)| *a == action) {
        return Some(c);
    }
    let cmds: Vec<&'static str> = ACTIONS.iter().filter(|(_, a)| *a == action).map(|(c, _)| *c).collect();
    cmds.iter().find(|c| c.starts_with("select.") == shift).or(cmds.first()).copied()
}

/// The thc action for a caretline command.
pub fn action(command: &str) -> Option<&'static str> {
    ACTIONS.iter().find(|(c, _)| *c == command).map(|(_, a)| *a)
}

/// The caretline command a thc action is (its `move.*` form for a move).
pub fn command_of(action: &str) -> Option<&'static CommandInfo> {
    ACTIONS.iter().find(|(_, a)| *a == action).and_then(|(c, _)| command(c))
}

/// An action's words from the catalog, for `thc keys` and the palette: its name, and for a
/// move that ⇧ extends, says so.
pub fn words(action: &str) -> Option<String> {
    let c = command_of(action)?;
    let selects = c.category == Category::Move && ACTIONS.iter().any(|(s, a)| *a == action && s.starts_with("select."));
    let name = c.name.to_lowercase();
    Some(if selects { format!("{name} (⇧ selects)") } else { name })
}

/// caretline's chord notation (`<c-s-z>`, `<a-left>`, `<d-bs>`) in thc's (`C-Z`, `A-left`,
/// `Cmd-backspace`): ⇧ on a letter is its capital.
pub fn thc_keys(caretline: &str) -> String {
    let inner = caretline.trim_start_matches('<').trim_end_matches('>');
    let (mut ctrl, mut alt, mut cmd, mut shift) = (false, false, false, false);
    let mut rest = inner;
    while rest.len() > 2 && rest.as_bytes()[1] == b'-' {
        match &rest[..1] {
            "c" => ctrl = true,
            "a" => alt = true,
            "d" => cmd = true,
            _ => shift = true,
        }
        rest = &rest[2..];
    }
    let name = match rest {
        "cr" => "enter".to_string(),
        "bs" => "backspace".to_string(),
        "del" => "delete".to_string(),
        "pgup" => "pageup".to_string(),
        "pgdn" => "pagedown".to_string(),
        "lt" => "<".to_string(),
        "gt" => ">".to_string(),
        r if r.chars().count() == 1 && shift => {
            shift = false;
            r.to_uppercase()
        }
        r => r.to_string(),
    };
    let mut s = String::new();
    for (on, p) in [(ctrl, "C-"), (alt, "A-"), (shift, "S-"), (cmd, "Cmd-")] {
        if on {
            s.push_str(p);
        }
    }
    s.push_str(&name);
    s
}

fn group(c: &CommandInfo) -> &'static str {
    match c.category {
        Category::Move | Category::Select => "Move",
        Category::Clipboard => "Clipboard",
        _ => "Write",
    }
}

/// caretline's outline keymap as `write` rows: every binding whose command thc runs, unless
/// `taken` (thc's own rows) already binds its key.
pub fn rows(taken: &[Binding]) -> Vec<Binding> {
    let parse = |k: &str| crate::keymap::parse_seq(k);
    let mut out: Vec<Binding> = Vec::new();
    for b in default_keymap(true) {
        let (Some(action), Some(info)) = (action(b.command), command(b.command)) else { continue };
        let keys = thc_keys(b.keys);
        let Some(seq) = parse(&keys) else { continue };
        let bound = |r: &Binding| r.ctx == Ctx::Write && parse(r.keys).as_ref() == Some(&seq);
        if taken.iter().any(bound) || out.iter().any(bound) {
            continue;
        }
        out.push(Binding { remapped: false, ctx: Ctx::Write, keys: Box::leak(keys.into_boxed_str()), action, when: When::Always, label: "", group: group(info), footer: None });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notation_converts() {
        assert_eq!(thc_keys("<c-s-z>"), "C-Z");
        assert_eq!(thc_keys("<a-s-left>"), "A-S-left");
        assert_eq!(thc_keys("<d-bs>"), "Cmd-backspace");
        assert_eq!(thc_keys("<s-tab>"), "S-tab");
        assert_eq!(thc_keys("<pgdn>"), "pagedown");
        for b in default_keymap(true) {
            assert!(crate::keymap::parse_seq(&thc_keys(b.keys)).is_some(), "{} → {}", b.keys, thc_keys(b.keys));
        }
    }

    #[test]
    fn every_action_is_a_write_action_thc_runs() {
        for (c, a) in ACTIONS {
            assert!(command(c).is_some(), "{c} isn't in caretline's catalog");
            let runs = crate::keymap::table().iter().any(|b| b.ctx == Ctx::Write && b.action == *a);
            assert!(runs, "{a} isn't bound in write");
        }
    }

    /// Every caretline binding thc runs is in the table, by caretline's keys or by a thc row on
    /// the same key: the editing keys come from the catalog.
    #[test]
    fn the_write_table_holds_caretlines_editing_keys() {
        let t = crate::keymap::table();
        for b in default_keymap(true) {
            let Some(_) = action(b.command) else { continue };
            let seq = crate::keymap::parse_seq(&thc_keys(b.keys));
            assert!(t.iter().any(|r| r.ctx == Ctx::Write && crate::keymap::parse_seq(r.keys) == seq), "{} ({}) isn't bound in write", b.keys, b.command);
        }
    }
}
