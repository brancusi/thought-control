//! The keymap and the key-script notation.

use caretline::keymap::{parse_keys, ScriptItem};
use caretline::{keymap, script_to_msgs, By, Dir, Key, KeyCode, Mods, Msg};

fn key(code: KeyCode, f: impl FnOnce(&mut Mods)) -> Key {
    let mut mods = Mods::default();
    f(&mut mods);
    Key { code, mods }
}

#[test]
fn script_tokens_parse() {
    let items = parse_keys("a<cr><s-left><a-left><c-z><bs><up><down><lt><c-s-z><d-a><wait:250>x<").unwrap();
    let keys: Vec<_> = items
        .iter()
        .map(|i| match i {
            ScriptItem::Key(k) => format!("{:?}", k.code),
            ScriptItem::Wait(ms) => format!("wait {ms}"),
        })
        .collect();
    assert_eq!(
        keys,
        ["Char('a')", "Enter", "Left", "Left", "Char('z')", "Backspace", "Up", "Down", "Char('<')", "Char('z')", "Char('a')", "wait 250", "Char('x')", "Char('<')"]
    );
    assert!(parse_keys("<nope>").is_err());
    assert!(parse_keys("<q-x>").is_err());
}

#[test]
fn key_bindings() {
    use By::*;
    use Dir::*;
    let mv = |dir, by, extend| Some(Msg::Move { dir, by, extend });
    let cases: Vec<(Key, Option<Msg>)> = vec![
        (key(KeyCode::Left, |_| {}), mv(Backward, Grapheme, false)),
        (key(KeyCode::Right, |m| m.shift = true), mv(Forward, Grapheme, true)),
        (key(KeyCode::Left, |m| m.alt = true), mv(Backward, Word, false)),
        (key(KeyCode::Right, |m| { m.alt = true; m.shift = true }), mv(Forward, Word, true)),
        (key(KeyCode::Left, |m| m.cmd = true), mv(Backward, LineStart, false)),
        (key(KeyCode::Right, |m| m.cmd = true), mv(Forward, LineEnd, false)),
        (key(KeyCode::Up, |_| {}), mv(Backward, VisualLine, false)),
        (key(KeyCode::Down, |m| m.shift = true), mv(Forward, VisualLine, true)),
        (key(KeyCode::Up, |m| m.cmd = true), mv(Backward, DocStart, false)),
        (key(KeyCode::Home, |_| {}), mv(Backward, LineStart, false)),
        (key(KeyCode::End, |m| m.shift = true), mv(Forward, LineEnd, true)),
        (key(KeyCode::Home, |m| m.ctrl = true), mv(Backward, DocStart, false)),
        (key(KeyCode::PageDown, |_| {}), mv(Forward, Page, false)),
        (key(KeyCode::Char('a'), |m| m.ctrl = true), mv(Backward, LineStart, false)),
        (key(KeyCode::Char('e'), |m| m.ctrl = true), mv(Forward, LineEnd, false)),
        (key(KeyCode::Char('a'), |m| m.cmd = true), Some(Msg::SelectAll)),
        (key(KeyCode::Char('z'), |m| m.ctrl = true), Some(Msg::Undo)),
        (key(KeyCode::Char('z'), |m| m.cmd = true), Some(Msg::Undo)),
        (key(KeyCode::Char('Z'), |m| { m.cmd = true; m.shift = true }), Some(Msg::Redo)),
        (key(KeyCode::Char('z'), |m| { m.ctrl = true; m.shift = true }), Some(Msg::Redo)),
        (key(KeyCode::Char('y'), |m| m.ctrl = true), Some(Msg::Redo)),
        (key(KeyCode::Char('c'), |m| m.cmd = true), Some(Msg::Copy)),
        (key(KeyCode::Char('x'), |m| m.ctrl = true), Some(Msg::Cut)),
        (key(KeyCode::Char('v'), |m| m.ctrl = true), Some(Msg::Paste { text: None })),
        (key(KeyCode::Char('s'), |m| m.cmd = true), Some(Msg::Save)),
        (key(KeyCode::Char('q'), |m| m.ctrl = true), Some(Msg::Quit)),
        (key(KeyCode::Char('é'), |_| {}), Some(Msg::InsertText { text: "é".into() })),
        (key(KeyCode::Char('a'), |m| m.shift = true), Some(Msg::InsertText { text: "A".into() })),
        (key(KeyCode::Enter, |_| {}), Some(Msg::InsertNewline)),
        (key(KeyCode::Backspace, |m| m.alt = true), Some(Msg::DeleteWordBackward)),
        (key(KeyCode::Backspace, |m| m.cmd = true), Some(Msg::DeleteToLineStart)),
        (key(KeyCode::Delete, |m| m.alt = true), Some(Msg::DeleteWordForward)),
        (key(KeyCode::Delete, |m| m.cmd = true), Some(Msg::DeleteToLineEnd)),
        (key(KeyCode::Esc, |_| {}), Some(Msg::Collapse)),
        (key(KeyCode::Char('j'), |m| m.ctrl = true), None),
        (key(KeyCode::BackTab, |_| {}), None),
    ];
    for (k, want) in cases {
        assert_eq!(keymap(&k), want, "{k:?}");
    }
}

#[test]
fn waits_become_ticks() {
    let msgs = script_to_msgs("a<wait:100>b<wait:50>", 1000).unwrap();
    assert_eq!(msgs[1], Msg::Tick { now_ms: 1100 });
    assert_eq!(msgs[3], Msg::Tick { now_ms: 1150 });
}
