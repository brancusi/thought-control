//! Keys: a terminal-independent key type, the keymap (a pure function from a key to a
//! message), and the key-script notation used by tests and the headless CLI.

use serde::{Deserialize, Serialize};

use crate::msg::{By, Dir, Msg};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCode {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Esc,
}

/// Modifier keys. `cmd` is the macOS Command key (the kitty protocol's "super").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Mods {
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub cmd: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Key {
    pub code: KeyCode,
    #[serde(default)]
    pub mods: Mods,
}

impl Key {
    pub fn plain(code: KeyCode) -> Key {
        Key {
            code,
            mods: Mods::default(),
        }
    }
}

fn mv(dir: Dir, by: By, extend: bool) -> Option<Msg> {
    Some(Msg::Move { dir, by, extend })
}

/// Maps a key to a message. Pure; unknown keys map to `None`.
///
/// macOS text-field keys come first, with Ctrl twins for terminals that don't forward Cmd
/// (see the README for the table).
pub fn keymap(key: &Key) -> Option<Msg> {
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
/// | `Ctrl-T` | `task_cycle` |
/// | `Shift-Enter`, `Ctrl-J` | `soft_break` |
/// | `Alt-↑` / `Alt-↓` | `move_block` |
/// | `Ctrl-↑` / `Ctrl-↓` | `move` by `block` (Shift extends) |
/// | `Alt-V` | `paste_plain` |
pub fn outline_keymap(key: &Key) -> Option<Msg> {
    let m = key.mods;
    use Dir::{Backward as B, Forward as F};
    match key.code {
        KeyCode::Tab if !(m.ctrl || m.alt || m.cmd) => {
            return Some(if m.shift { Msg::Outdent } else { Msg::Indent });
        }
        KeyCode::BackTab => return Some(Msg::Outdent),
        KeyCode::Enter if m.shift && !(m.ctrl || m.alt || m.cmd) => return Some(Msg::SoftBreak),
        KeyCode::Char(c) if m.ctrl && !m.cmd && !m.alt => match c.to_ascii_lowercase() {
            't' => return Some(Msg::TaskCycle),
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
    keymap(key)
}

/// [`outline_keymap`] for an outline document, else [`keymap`].
pub fn keymap_for(outline: bool, key: &Key) -> Option<Msg> {
    if outline {
        outline_keymap(key)
    } else {
        keymap(key)
    }
}

/// One step of a key script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptItem {
    Key(Key),
    /// Advance the clock by this many milliseconds (becomes a `Tick`).
    Wait(u64),
}

/// Parses a key script: literal characters, plus `<...>` tokens.
///
/// Tokens: `<cr>` `<enter>` `<bs>` `<del>` `<tab>` `<s-tab>` `<esc>` `<space>` `<left>`
/// `<right>` `<up>` `<down>` `<home>` `<end>` `<pgup>` `<pgdn>` `<lt>` (a literal `<`), and
/// `<wait:MS>`. Modifier prefixes, combinable: `s-` shift, `c-` ctrl, `a-` alt (`m-` too),
/// `d-` Cmd (super). For example `<c-s-z>`, `<a-left>`, `<d-a>`. A `<` that doesn't start
/// a token is a literal character.
pub fn parse_keys(script: &str) -> Result<Vec<ScriptItem>, String> {
    let chars: Vec<char> = script.chars().collect();
    let mut items = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            if let Some(len) = chars[i + 1..].iter().position(|&c| c == '>') {
                let token: String = chars[i + 1..i + 1 + len].iter().collect();
                if !token.is_empty() {
                    items.push(parse_token(&token)?);
                    i += len + 2;
                    continue;
                }
            }
        }
        let code = match c {
            '\n' | '\r' => KeyCode::Enter,
            '\t' => KeyCode::Tab,
            c => KeyCode::Char(c),
        };
        items.push(ScriptItem::Key(Key::plain(code)));
        i += 1;
    }
    Ok(items)
}

fn parse_token(token: &str) -> Result<ScriptItem, String> {
    if let Some(ms) = token.strip_prefix("wait:") {
        return ms
            .parse()
            .map(ScriptItem::Wait)
            .map_err(|_| format!("bad wait in <{token}>"));
    }
    let mut mods = Mods::default();
    let mut rest = token;
    loop {
        let lower = rest.to_ascii_lowercase();
        // A modifier prefix is one letter and a dash, followed by more.
        if rest.len() > 2 && rest.as_bytes()[1] == b'-' {
            match &lower[..1] {
                "s" => mods.shift = true,
                "c" => mods.ctrl = true,
                "a" | "m" => mods.alt = true,
                "d" => mods.cmd = true,
                _ => return Err(format!("unknown modifier in <{token}>")),
            }
            rest = &rest[2..];
            continue;
        }
        break;
    }
    let name = rest.to_ascii_lowercase();
    let code = match name.as_str() {
        "cr" | "enter" | "ret" | "return" => KeyCode::Enter,
        "bs" | "backspace" => KeyCode::Backspace,
        "del" | "delete" => KeyCode::Delete,
        "tab" if mods.shift => {
            mods.shift = false;
            KeyCode::BackTab
        }
        "tab" => KeyCode::Tab,
        "esc" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" | "pageup" => KeyCode::PageUp,
        "pgdn" | "pagedown" => KeyCode::PageDown,
        "lt" => KeyCode::Char('<'),
        "gt" => KeyCode::Char('>'),
        _ => {
            let mut cs = rest.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => return Err(format!("unknown key <{token}>")),
            }
        }
    };
    Ok(ScriptItem::Key(Key { code, mods }))
}

/// Turns a key script into messages through the keymap. Waits become `Tick`s, counted
/// from `now_ms`. Keys the keymap ignores are dropped.
pub fn script_to_msgs(script: &str, now_ms: u64) -> Result<Vec<Msg>, String> {
    script_to_msgs_for(script, now_ms, false)
}

/// [`script_to_msgs`] through the outline keymap when `outline` is set.
pub fn script_to_msgs_for(script: &str, now_ms: u64, outline: bool) -> Result<Vec<Msg>, String> {
    let mut now = now_ms;
    let mut msgs = Vec::new();
    for item in parse_keys(script)? {
        match item {
            ScriptItem::Key(key) => msgs.extend(keymap_for(outline, &key)),
            ScriptItem::Wait(ms) => {
                now += ms;
                msgs.push(Msg::Tick { now_ms: now });
            }
        }
    }
    Ok(msgs)
}
