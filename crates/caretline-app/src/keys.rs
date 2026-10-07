//! The editor's keys, from caretline's command catalog and default keymap: `caretline keys`
//! prints them, and F1 (or Alt-?) shows them over the editor. Generated, so they never drift
//! from what the keys do.

use caretline::commands::{commands, default_keymap, Category, Platform};
use caretline::view::{Cell, Frame, Role};

/// `<c-s-z>` as `Ctrl-Shift-Z`, `<d-left>` as `Cmd-Left`.
pub fn label(keys: &str) -> String {
    let inner = keys.trim_start_matches('<').trim_end_matches('>');
    let mut parts: Vec<String> = Vec::new();
    let mut rest = inner;
    while rest.len() > 2 && rest.as_bytes()[1] == b'-' {
        parts.push(
            match &rest[..1] {
                "c" => "Ctrl",
                "a" => "Alt",
                "d" => "Cmd",
                _ => "Shift",
            }
            .to_string(),
        );
        rest = &rest[2..];
    }
    let name = match rest {
        "cr" => "Enter".to_string(),
        "bs" => "Backspace".to_string(),
        "del" => "Delete".to_string(),
        "esc" => "Esc".to_string(),
        "tab" => "Tab".to_string(),
        "pgup" => "PageUp".to_string(),
        "pgdn" => "PageDown".to_string(),
        "left" | "right" | "up" | "down" | "home" | "end" => {
            let mut c = rest.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
        r => r.to_uppercase(),
    };
    parts.push(name);
    parts.join("-")
}

/// The keys as lines: each category, then each command with its keys (⌘ chords marked).
pub fn lines(outline: bool) -> Vec<String> {
    let map = default_keymap(outline);
    let mut out = Vec::new();
    let mut last: Option<Category> = None;
    for c in commands() {
        let keys: Vec<String> = map
            .iter()
            .filter(|b| b.command == c.id)
            .map(|b| if b.platform == Platform::Mac { format!("{} (mac)", label(b.keys)) } else { label(b.keys) })
            .collect();
        if keys.is_empty() {
            continue;
        }
        if last != Some(c.category) {
            if last.is_some() {
                out.push(String::new());
            }
            out.push(c.category.name().to_string());
            last = Some(c.category);
        }
        out.push(format!("  {:<24} {}", c.name, keys.join(", ")));
    }
    out.push(String::new());
    out.push("Printable keys type themselves. (mac): Cmd, in a terminal that sends it.".into());
    out
}

/// `caretline keys [--outline] [--json]`.
pub fn main(args: &[String]) -> Result<(), String> {
    let outline = args.iter().any(|a| a == "--outline");
    if args.iter().any(|a| a == "--json") {
        let v = serde_json::json!({ "commands": commands(), "keymap": default_keymap(outline) });
        println!("{}", serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?);
        return Ok(());
    }
    if let Some(bad) = args.iter().find(|a| !matches!(a.as_str(), "--outline" | "--json")) {
        return Err(format!("keys: unknown argument {bad} (usage: caretline keys [--outline] [--json])"));
    }
    println!("{}", if outline { "caretline keys (outline documents)" } else { "caretline keys" });
    println!();
    for l in lines(outline) {
        println!("{l}");
    }
    Ok(())
}

/// The keys drawn over `frame` in a box, from row 1 (scrolled by `offset` lines when they don't
/// fit), with a footer that says how to close it.
pub fn overlay(frame: &mut Frame, outline: bool, offset: usize) {
    let mut body = lines(outline);
    body.insert(0, "Keys".into());
    body.insert(1, String::new());
    let (w, h) = (frame.width as usize, frame.height as usize);
    let inner = body.iter().map(|l| l.chars().count()).max().unwrap_or(0).min(w.saturating_sub(4));
    let rows = (h.saturating_sub(4)).min(body.len());
    let x0 = (w.saturating_sub(inner + 4)) / 2;
    let y0 = (h.saturating_sub(rows + 3)) / 2;
    let offset = offset.min(body.len().saturating_sub(rows));
    let put = |frame: &mut Frame, x: usize, y: usize, s: &str, role: Role| {
        let mut cx = x;
        for ch in s.chars() {
            if cx >= w || y >= h {
                break;
            }
            frame.cells[y * w + cx] = Cell { symbol: ch.to_string().as_str().into(), role, char_idx: None };
            cx += 1;
        }
    };
    let blank = " ".repeat(inner + 4);
    for y in y0..(y0 + rows + 3).min(h) {
        put(frame, x0, y, &blank, Role::Status);
    }
    for (k, l) in body.iter().skip(offset).take(rows).enumerate() {
        let l: String = l.chars().take(inner).collect();
        put(frame, x0 + 2, y0 + 1 + k, &l, Role::Status);
    }
    let footer = if body.len() > rows { "↑↓ scroll · any other key closes" } else { "any key closes" };
    put(frame, x0 + 2, y0 + rows + 1, footer, Role::StatusAccent);
    frame.cursor = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_read_as_keys() {
        assert_eq!(label("<c-s-z>"), "Ctrl-Shift-Z");
        assert_eq!(label("<d-left>"), "Cmd-Left");
        assert_eq!(label("<pgdn>"), "PageDown");
        assert_eq!(label("<a-b>"), "Alt-B");
    }

    #[test]
    fn every_bound_command_is_listed() {
        let text = lines(true).join("\n");
        for c in commands() {
            if default_keymap(true).iter().any(|b| b.command == c.id) {
                assert!(text.contains(c.name), "{} missing", c.id);
            }
        }
        assert!(text.contains("Indent") && text.contains("Tab"), "{text}");
        assert!(!lines(false).join("\n").contains("Indent"), "a plain document has no outline keys");
    }

    #[test]
    fn the_overlay_draws_over_the_editor() {
        let s = caretline::State::new("hello", None, caretline::Viewport { width: 80, height: 24 });
        let mut f = caretline::view(&s);
        overlay(&mut f, false, 0);
        let text = f.to_text();
        assert!(text.contains("Keys") && text.contains("Word left") && text.contains("closes"), "{text}");
        assert_eq!(f.cursor, None);
        let mut small = caretline::view(&caretline::State::new("", None, caretline::Viewport { width: 30, height: 6 }));
        overlay(&mut small, true, 999);
        assert!(small.to_text().contains("scroll"), "{}", small.to_text());
    }
}
