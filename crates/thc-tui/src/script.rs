//! Key scripts: the text form of keyboard, mouse and paste input (`2j<cr>`, `<c-o>`,
//! `<click:10,3>`, `<paste:hello>`). `THC_TUI_KEYS`, the protocol's `keys` op and
//! `thc ui send keys` all read it, and every terminal key has a token, so a trace can record
//! any key as one.
//!
//! A token is `<mods-name>`: mods any of `c-` (⌃), `m-` (⌥), `s-` (⇧), `d-` (⌘), in that
//! order; name a single character or one of `cr esc tab bs del space up down left right home
//! end pgup pgdn f1..f12 lt`. Any other character is itself.

use crate::session::{Fixture, Mouse, MouseKind, Msg};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

/// A key event as its token: `j`, `J`, `<cr>`, `<c-o>`, `<s-tab>`, `<c-m-left>`, `<lt>`.
pub fn key_token(k: &KeyEvent) -> String {
    let mut mods = k.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::SUPER);
    let name = match k.code {
        KeyCode::Char('<') => "lt".to_string(),
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => {
            // ⇧ lives in a letter's case.
            if c.is_alphabetic() || !c.is_ascii() || c.is_ascii_punctuation() || c.is_ascii_digit() {
                mods.remove(KeyModifiers::SHIFT);
            }
            c.to_string()
        }
        KeyCode::Enter => "cr".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => {
            mods |= KeyModifiers::SHIFT;
            "tab".into()
        }
        KeyCode::Backspace => "bs".into(),
        KeyCode::Delete => "del".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pgup".into(),
        KeyCode::PageDown => "pgdn".into(),
        KeyCode::F(n) => format!("f{n}"),
        _ => return "<null>".into(),
    };
    let prefix: String = [(KeyModifiers::CONTROL, "c-"), (KeyModifiers::ALT, "m-"), (KeyModifiers::SHIFT, "s-"), (KeyModifiers::SUPER, "d-")]
        .iter()
        .filter(|(m, _)| mods.contains(*m))
        .map(|(_, p)| *p)
        .collect();
    if prefix.is_empty() && name.chars().count() == 1 {
        return name;
    }
    format!("<{prefix}{name}>")
}

/// A key token's event (`j`, `<c-o>`, …). None: not a key.
pub fn key_event(token: &str) -> Option<KeyEvent> {
    let ev = |code, modifiers| Some(KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    let Some(inner) = token.strip_prefix('<').and_then(|t| t.strip_suffix('>')) else {
        let mut cs = token.chars();
        let c = cs.next()?;
        return if cs.next().is_none() { ev(KeyCode::Char(c), KeyModifiers::NONE) } else { None };
    };
    let mut mods = KeyModifiers::NONE;
    let mut rest = inner;
    loop {
        let m = match rest.get(..2) {
            Some("c-") => KeyModifiers::CONTROL,
            Some("m-") => KeyModifiers::ALT,
            Some("s-") => KeyModifiers::SHIFT,
            Some("d-") => KeyModifiers::SUPER,
            _ => break,
        };
        // `<c-->`: the name is `-` itself.
        if rest.len() == 2 {
            break;
        }
        mods |= m;
        rest = &rest[2..];
    }
    let code = match rest {
        "cr" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" if mods.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
        "tab" => KeyCode::Tab,
        "bs" => KeyCode::Backspace,
        "del" => KeyCode::Delete,
        "space" => KeyCode::Char(' '),
        "lt" => KeyCode::Char('<'),
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" => KeyCode::PageUp,
        "pgdn" => KeyCode::PageDown,
        f if f.len() >= 2 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => KeyCode::F(f[1..].parse().ok()?),
        s if s.chars().count() == 1 => KeyCode::Char(s.chars().next()?),
        _ => return None,
    };
    ev(code, mods)
}

/// What a script asks for, in order. `fixtures`: allow the test-only tokens (`<agent:…>`,
/// `<remote:ID:TEXT>`, `<alert>`), which write to the vault as another actor would, and pass
/// over unknown tokens (THC_TUI_KEYS); otherwise an unknown token is an error.
pub fn parse(script: &str, fixtures: bool) -> Result<Vec<Msg>, String> {
    Ok(parse_grouped(script, fixtures)?.into_iter().flatten().collect())
}

/// Like `parse`, with each token's messages together (a click is a press and a release).
pub fn parse_grouped(script: &str, fixtures: bool) -> Result<Vec<Vec<Msg>>, String> {
    let mut out: Vec<Vec<Msg>> = Vec::new();
    let mut rest = script;
    while !rest.is_empty() {
        if !rest.starts_with('<') {
            let c = rest.chars().next().unwrap();
            out.push(vec![Msg::Key { key: c.to_string() }]);
            rest = &rest[c.len_utf8()..];
            continue;
        }
        // A paste token's text may hold `>`: it ends at a `>` followed by `<` or the end.
        let end = if rest.starts_with("<paste:") {
            let b = rest.as_bytes();
            (1..b.len()).find(|&i| b[i] == b'>' && (i + 1 == b.len() || b[i + 1] == b'<'))
        } else {
            rest.find('>')
        };
        let Some(end) = end else { return Err(format!("unclosed token: {rest}")) };
        let token = &rest[..=end];
        let inner = &rest[1..end];
        rest = &rest[end + 1..];
        if let Some(text) = inner.strip_prefix("paste:") {
            out.push(vec![Msg::Paste { text: text.replace("\\n", "\n") }]);
        } else if let Some((doc, json)) = inner.strip_prefix("layer:").map(|j| (false, j)).or_else(|| inner.strip_prefix("doc:").map(|j| (true, j))) {
            // Test fixtures: a layer op (`<layer:{"op":"hint.show",…}>`) or a document-view op
            // (`<doc:{"op":"doc.msgs",…}>`) as an agent sends it.
            if !fixtures {
                return Err(format!("{token} is a test fixture, not input"));
            }
            let req: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("{token}: {e}"))?;
            let actor = req.get("actor").and_then(serde_json::Value::as_str).map(str::to_string);
            out.push(vec![if doc { Msg::DocView { req, actor } } else { Msg::Layer { req, actor } }]);
        } else if let Some(m) = mouse(inner)? {
            out.push(m);
        } else if let Some(f) = fixture(inner) {
            if !fixtures {
                return Err(format!("{token} is a test fixture, not input"));
            }
            out.push(vec![Msg::Fixture { fixture: f }]);
        } else if key_event(token).is_some() {
            out.push(vec![Msg::Key { key: token.to_string() }]);
        } else if inner == "null" || inner == "nop" || fixtures {
            // Nothing: a key the terminal sent that thc has no name for, `<nop>` (a beat in a
            // fixture: a frame drawn), and in THC_TUI_KEYS any token it doesn't know, as always.
            out.push(vec![]);
        } else {
            return Err(format!("unknown key {token}"));
        }
    }
    Ok(out)
}

fn fixture(inner: &str) -> Option<Fixture> {
    if inner == "alert" {
        return Some(Fixture::Alert);
    }
    if let Some(text) = inner.strip_prefix("agent:") {
        return Some(Fixture::Agent { text: text.to_string() });
    }
    let (id, text) = inner.strip_prefix("remote:")?.split_once(':')?;
    Some(Fixture::Remote { id: id.to_string(), text: text.to_string() })
}

/// The mouse tokens (mouse.md §9): `click dclick tclick sclick cclick aclick mclick :x,y`,
/// `drag:x1,y1,x2,y2`, `wheel:up|down[:n][@x,y]`, `hover:x,y`. A press and its release each
/// become a message; a drag a press, a move per cell and a release.
fn mouse(inner: &str) -> Result<Option<Vec<Msg>>, String> {
    let Some((name, arg)) = inner.split_once(':') else { return Ok(None) };
    let xy = |s: &str| -> Result<(u16, u16), String> {
        let (x, y) = s.split_once(',').ok_or_else(|| format!("<{inner}>: want x,y"))?;
        Ok((x.trim().parse().map_err(|_| format!("<{inner}>: bad x"))?, y.trim().parse().map_err(|_| format!("<{inner}>: bad y"))?))
    };
    let m = |kind, x, y, mods: &str| Msg::Mouse { mouse: Mouse { kind, x, y, mods: mods.to_string(), clicks: None } };
    let click = |kind, x, y, mods: &str, n: u8| Msg::Mouse { mouse: Mouse { kind, x, y, mods: mods.to_string(), clicks: Some(n) } };
    Ok(Some(match name {
        "click" | "dclick" | "tclick" | "sclick" | "cclick" | "aclick" | "mclick" => {
            let (x, y) = xy(arg)?;
            let (down, up, mods, n) = match name {
                "dclick" => (MouseKind::Down, MouseKind::Up, "", 2),
                "tclick" => (MouseKind::Down, MouseKind::Up, "", 3),
                "sclick" => (MouseKind::Down, MouseKind::Up, "s", 1),
                "cclick" => (MouseKind::Down, MouseKind::Up, "c", 1),
                "aclick" => (MouseKind::Down, MouseKind::Up, "m", 1),
                "mclick" => (MouseKind::MiddleDown, MouseKind::MiddleUp, "", 1),
                _ => (MouseKind::Down, MouseKind::Up, "", 1),
            };
            let mut v = Vec::new();
            for k in 1..=n {
                v.push(click(down, x, y, mods, k));
                v.push(click(up, x, y, mods, k));
            }
            v
        }
        "drag" => {
            let p: Vec<u16> = arg.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            let [x1, y1, x2, y2] = p[..] else { return Err(format!("<{inner}>: want x1,y1,x2,y2")) };
            let mut v = vec![click(MouseKind::Down, x1, y1, "", 1)];
            let steps = x1.abs_diff(x2).max(y1.abs_diff(y2)).max(1);
            for i in 1..=steps {
                let f = |a: u16, b: u16| (a as i32 + (b as i32 - a as i32) * i as i32 / steps as i32) as u16;
                v.push(click(MouseKind::Drag, f(x1, x2), f(y1, y2), "", 1));
            }
            v.push(click(MouseKind::Up, x2, y2, "", 1));
            v
        }
        "wheel" => {
            let (spec, at) = arg.split_once('@').map_or((arg, None), |(a, b)| (a, Some(b)));
            let (dir, n) = spec.split_once(':').map_or((spec, 1), |(d, n)| (d, n.parse().unwrap_or(1)));
            let kind = match dir {
                "up" => MouseKind::ScrollUp,
                "down" => MouseKind::ScrollDown,
                _ => return Err(format!("<{inner}>: wheel:up or wheel:down")),
            };
            // No position: the middle of the screen, as the snapshot always did (x is filled in
            // by the session from the screen's width).
            let (x, y) = match at {
                Some(a) => xy(a)?,
                None => (u16::MAX, 10),
            };
            (0..n).map(|_| m(kind, x, y, "")).collect()
        }
        "hover" => {
            let (x, y) = xy(arg)?;
            vec![m(MouseKind::Moved, x, y, "")]
        }
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_token_round_trips() {
        let codes = [
            KeyCode::Char('a'),
            KeyCode::Char('Z'),
            KeyCode::Char('<'),
            KeyCode::Char(' '),
            KeyCode::Char('é'),
            KeyCode::Char('-'),
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Tab,
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Up,
            KeyCode::PageDown,
            KeyCode::F(5),
        ];
        let mods = [KeyModifiers::NONE, KeyModifiers::CONTROL, KeyModifiers::ALT, KeyModifiers::SUPER, KeyModifiers::CONTROL | KeyModifiers::ALT];
        for code in codes {
            for m in mods {
                let k = KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE };
                let t = key_token(&k);
                let back = key_event(&t).unwrap_or_else(|| panic!("{t}"));
                assert_eq!((back.code, back.modifiers), (code, m), "{t}");
            }
        }
        let bt = KeyEvent { code: KeyCode::BackTab, modifiers: KeyModifiers::SHIFT, kind: KeyEventKind::Press, state: KeyEventState::NONE };
        assert_eq!(key_token(&bt), "<s-tab>");
        assert_eq!(key_event("<s-tab>").unwrap().code, KeyCode::BackTab);
    }

    #[test]
    fn scripts_read_keys_mouse_and_paste() {
        let msgs = parse("2j<cr><c-o><click:3,4><paste:a>b<cr>>", false).unwrap();
        let keys: Vec<String> = msgs.iter().map(|m| serde_json::to_string(m).unwrap()).collect();
        assert_eq!(keys.len(), 7, "{keys:?}");
        assert!(keys[6].contains("a>b<cr>"), "{keys:?}");
        assert!(parse("<oops>", false).is_err());
        assert!(parse("<agent:hi>", false).is_err());
        assert!(parse("<agent:hi>", true).is_ok());
        assert_eq!(parse_grouped("<nop>", false).unwrap(), vec![Vec::<Msg>::new()]);
    }
}
