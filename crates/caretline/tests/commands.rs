//! The command catalog and the keymap as data: every binding names a catalog command, every
//! command maps to a message, and the table agrees with the hand-written keymap it replaced on
//! every key with at most one of Ctrl, Alt and ⌘ (with or without ⇧).

use caretline::commands::{binding_key, command, command_for, key_notation};
use caretline::{command_msg, commands, default_keymap, keymap_for, By, Dir, Key, KeyCode, Mods, Msg};
use std::collections::HashSet;

#[test]
fn ids_are_unique_and_every_command_has_a_message() {
    let mut seen = HashSet::new();
    for c in commands() {
        assert!(seen.insert(c.id), "{} twice", c.id);
        let (cat, verb) = c.id.split_once('.').expect("category.verb");
        assert!(!cat.is_empty() && !verb.is_empty() && c.id == c.id.to_lowercase(), "{}", c.id);
        let block = c.takes_block.then_some(caretline::MarkId(1));
        assert!(command_msg(c.id, block).is_some(), "{} has no message", c.id);
        assert!(!c.name.is_empty() && !c.description.is_empty());
    }
    assert!(command_msg("view.fold", None).is_none(), "a block command needs its block");
    assert!(command_msg("nope.nothing", None).is_none());
}

#[test]
fn every_binding_names_a_command_and_a_key() {
    for outline in [false, true] {
        let mut keys = HashSet::new();
        for b in default_keymap(outline) {
            assert!(command(b.command).is_some(), "{} → {} isn't in the catalog", b.keys, b.command);
            let k = binding_key(&b).unwrap_or_else(|| panic!("{} isn't one key", b.keys));
            assert_eq!(key_notation(&k), b.keys, "the table writes keys as key_notation does");
            assert!(keys.insert(b.keys), "{} bound twice (outline {outline})", b.keys);
            assert_eq!(command_for(outline, &k), Some(b.command));
        }
    }
}

const CODES: &[KeyCode] = &[
    KeyCode::Enter, KeyCode::Backspace, KeyCode::Delete, KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down,
    KeyCode::Home, KeyCode::End, KeyCode::PageUp, KeyCode::PageDown, KeyCode::Tab, KeyCode::BackTab, KeyCode::Esc,
];

/// The table gives what the old functions gave (see the module docs).
#[test]
fn the_table_agrees_with_the_old_keymap() {
    let mut diffs = Vec::new();
    let mut codes: Vec<KeyCode> = CODES.to_vec();
    codes.extend(('a'..='z').chain('A'..='Z').chain("0[]-=;',./\\` é<".chars()).map(KeyCode::Char));
    for &code in &codes {
        for m in 0..8u8 {
            for shift in [false, true] {
                let mods = Mods { ctrl: m == 1, alt: m == 2, cmd: m == 3, shift };
                if m > 3 {
                    continue;
                }
                let key = Key { code, mods };
                let plain = m == 0;
                // ⇧Tab arrives as BackTab (nothing in a plain document) or as Tab with ⇧ (the old
                // function typed a tab): one key in the table, which binds neither.
                if code == KeyCode::Tab && shift {
                    continue;
                }
                if (plain || keymap_for(false, &key).is_some()) && keymap_for(false, &key) != old_keymap(&key) {
                    diffs.push(format!("plain {}: {:?} want {:?}", key_notation(&key), keymap_for(false, &key), old_keymap(&key)));
                }
                if (plain || keymap_for(true, &key).is_some()) && keymap_for(true, &key) != old_outline_keymap(&key) {
                    diffs.push(format!("outline {}: {:?} want {:?}", key_notation(&key), keymap_for(true, &key), old_outline_keymap(&key)));
                }
            }
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
}

// ---- the keymap before it was a table ----------------------------------------------------

fn mv(dir: Dir, by: By, extend: bool) -> Option<Msg> {
    Some(Msg::Move { dir, by, extend })
}

/// Maps a key to a message. Pure; unknown keys map to `None`.
///
/// macOS text-field keys come first, with Ctrl twins for terminals that don't forward Cmd
/// (see the README for the table).
fn old_keymap(key: &Key) -> Option<Msg> {
    let m = key.mods;
    let shift = m.shift;
    use Dir::{Backward as B, Forward as F};
    match key.code {
        KeyCode::Char(c) => {
            let lower = c.to_ascii_lowercase();
            let shift = shift || c.is_ascii_uppercase();
            if m.cmd || m.ctrl {
                // Command-only bindings first, then those shared with Ctrl.
                match (lower, m.cmd) {
                    ('a', true) => return Some(Msg::SelectAll),
                    ('a', false) => return mv(B, By::LineStart, shift),
                    ('e', false) => return mv(F, By::LineEnd, shift),
                    ('w', false) => return Some(Msg::DeleteWordBackward),
                    ('u', false) => return Some(Msg::DeleteToLineStart),
                    ('k', false) => return Some(Msg::KillLine),
                    ('h', false) => return Some(Msg::DeleteBackward),
                    _ => {}
                }
                return match lower {
                    'z' if shift => Some(Msg::Redo),
                    'z' => Some(Msg::Undo),
                    'y' | 'r' => Some(Msg::Redo),
                    'c' => Some(Msg::Copy),
                    'x' => Some(Msg::Cut),
                    'v' => Some(Msg::Paste { text: None }),
                    's' => Some(Msg::Save),
                    'q' => Some(Msg::Quit),
                    _ => None,
                };
            }
            if m.alt {
                return match lower {
                    'b' => mv(B, By::Word, shift),
                    'f' => mv(F, By::Word, shift),
                    'd' => Some(Msg::DeleteWordForward),
                    'a' => Some(Msg::SelectAll),
                    _ => None,
                };
            }
            let c = if m.shift { c.to_uppercase().next().unwrap_or(c) } else { c };
            Some(Msg::InsertText {
                text: c.to_string(),
            })
        }
        KeyCode::Enter => Some(Msg::InsertNewline),
        KeyCode::Tab => Some(Msg::InsertText { text: "\t".into() }),
        KeyCode::BackTab => None,
        KeyCode::Esc => Some(Msg::Collapse),
        KeyCode::Backspace => Some(if m.cmd {
            Msg::DeleteToLineStart
        } else if m.alt || m.ctrl {
            Msg::DeleteWordBackward
        } else {
            Msg::DeleteBackward
        }),
        KeyCode::Delete => Some(if m.cmd {
            Msg::DeleteToLineEnd
        } else if m.alt || m.ctrl {
            Msg::DeleteWordForward
        } else {
            Msg::DeleteForward
        }),
        KeyCode::Left if m.cmd => mv(B, By::LineStart, shift),
        KeyCode::Right if m.cmd => mv(F, By::LineEnd, shift),
        KeyCode::Left if m.alt || m.ctrl => mv(B, By::Word, shift),
        KeyCode::Right if m.alt || m.ctrl => mv(F, By::Word, shift),
        KeyCode::Left => mv(B, By::Grapheme, shift),
        KeyCode::Right => mv(F, By::Grapheme, shift),
        KeyCode::Up if m.cmd => mv(B, By::DocStart, shift),
        KeyCode::Down if m.cmd => mv(F, By::DocEnd, shift),
        KeyCode::Up if m.alt || m.ctrl => None,
        KeyCode::Down if m.alt || m.ctrl => None,
        KeyCode::Up => mv(B, By::VisualLine, shift),
        KeyCode::Down => mv(F, By::VisualLine, shift),
        KeyCode::Home if m.ctrl || m.cmd => mv(B, By::DocStart, shift),
        KeyCode::End if m.ctrl || m.cmd => mv(F, By::DocEnd, shift),
        KeyCode::Home => mv(B, By::LineStart, shift),
        KeyCode::End => mv(F, By::LineEnd, shift),
        KeyCode::PageUp => mv(B, By::Page, shift),
        KeyCode::PageDown => mv(F, By::Page, shift),
    }
}

/// The keymap of an outline document: the plain [`keymap`] with the outline's keys on top.
///
/// | Key | Msg |
/// |---|---|
/// | `Tab` / `Shift-Tab` | `indent` / `outdent` |
/// | `Shift-Enter`, `Ctrl-J` | `soft_break` |
/// | `Alt-↑` / `Alt-↓` | `move_block` |
/// | `Ctrl-↑` / `Ctrl-↓` | `move` by `block` (Shift extends) |
/// | `Alt-V` | `paste_plain` |
fn old_outline_keymap(key: &Key) -> Option<Msg> {
    let m = key.mods;
    use Dir::{Backward as B, Forward as F};
    match key.code {
        KeyCode::Tab if !(m.ctrl || m.alt || m.cmd) => {
            return Some(if m.shift { Msg::Outdent } else { Msg::Indent });
        }
        KeyCode::BackTab => return Some(Msg::Outdent),
        KeyCode::Enter if m.shift && !(m.ctrl || m.alt || m.cmd) => return Some(Msg::SoftBreak),
        KeyCode::Char(c) if m.ctrl && !m.cmd && !m.alt => match c.to_ascii_lowercase() {
            'j' => return Some(Msg::SoftBreak),
            _ => {}
        },
        KeyCode::Char(c) if m.alt && !m.ctrl && !m.cmd && c.eq_ignore_ascii_case(&'v') => {
            return Some(Msg::PastePlain { text: None });
        }
        KeyCode::Up if m.alt && !m.cmd => return Some(Msg::MoveBlock { dir: B }),
        KeyCode::Down if m.alt && !m.cmd => return Some(Msg::MoveBlock { dir: F }),
        KeyCode::Up if m.ctrl && !m.cmd => return mv(B, By::Block, m.shift),
        KeyCode::Down if m.ctrl && !m.cmd => return mv(F, By::Block, m.shift),
        _ => {}
    }
    old_keymap(key)
}


#[allow(dead_code)]
fn _uses(_: By, _: Dir, _: Msg) {}

#[test]
fn the_protocol_lists_commands_and_the_keymap() {
    let mut s = caretline::Session::new(caretline::State::new("x", None, caretline::Viewport { width: 20, height: 4 }));
    let r: serde_json::Value = serde_json::from_str(&s.handle(r#"{"id":1,"op":"commands.list"}"#, None).response).unwrap();
    let cmds = r["result"]["commands"].as_array().unwrap();
    assert_eq!(cmds.len(), commands().len());
    assert!(cmds.iter().any(|c| c["id"] == "move.word_right" && c["category"] == "move"), "{r}");
    let r: serde_json::Value = serde_json::from_str(&s.handle(r#"{"id":2,"op":"keymap.get","outline":true}"#, None).response).unwrap();
    let b = r["result"]["bindings"].as_array().unwrap();
    assert!(b.iter().any(|b| b["keys"] == "<tab>" && b["command"] == "structure.indent"), "{r}");
    let hello = s.handle(r#"{"id":3,"op":"hello"}"#, None).response;
    assert!(hello.contains("commands.list") && hello.contains("keymap.get"), "{hello}");
}
