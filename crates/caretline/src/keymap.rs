//! Keys: a terminal-independent key type, the keymap (a pure function from a key to a
//! message), and the key-script notation used by tests and the headless CLI.

use serde::{Deserialize, Serialize};

use crate::msg::Msg;

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

/// Maps a key to a message: a lookup in the default keymap ([`crate::commands::default_keymap`]),
/// and a printable key without Ctrl, Alt or ⌘ types itself. Pure; unknown keys map to `None`.
///
/// macOS text-field keys come first, with Ctrl twins for terminals that don't forward Cmd
/// (see docs/caretline/keys.md for the table).
pub fn keymap(key: &Key) -> Option<Msg> {
    crate::commands::lookup(false, key)
}

/// The keymap of an outline document: the plain [`keymap`] with the outline's keys on top
/// (`Tab`/`Shift-Tab` indent and outdent, `Shift-Enter`/`Ctrl-J` a soft break, `Alt-↑/↓` move
/// a block, `Ctrl-↑/↓` step by block, `Alt-V` paste plain).
pub fn outline_keymap(key: &Key) -> Option<Msg> {
    crate::commands::lookup(true, key)
}

/// [`outline_keymap`] for an outline document, else [`keymap`].
pub fn keymap_for(outline: bool, key: &Key) -> Option<Msg> {
    crate::commands::lookup(outline, key)
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
