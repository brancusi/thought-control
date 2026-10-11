//! The e2e scenarios (scripts/e2e/scenarios/*.json), headless: every editing golden that targets
//! thc's TUI and needs no terminal runs here as a flow, from the same file the real-terminal
//! harness (scripts/e2e/run.sh) drives WezTerm with. A scenario seeds a page with the golden's
//! notes, opens it, makes the golden's selection with keys, runs its steps and compares the
//! document (text, caret and selection), the clipboard and the screen with what it expects.
//!
//! A scenario that fails here on purpose names the board task or finding under
//! `known["thc-tui/headless"]` (or `known["thc-tui"]`); the test fails when a known failure
//! starts passing, so the list stays true.

use super::*;
use thc_core::outline::Kind;

const TARGET: &str = "thc-tui";

/// A note as the golden notation writes it.
#[derive(Clone, Debug, PartialEq)]
struct Note {
    depth: usize,
    kind: Kind,
    status: Option<String>,
    text: String,
}

/// A place: (note, char offset in its text).
type Pos = (usize, usize);

/// A golden (`Hello ⟦wor▮⟧ld`, ` ‖ ` between notes, `⏎` a soft break, `[ ] ` / `- ` markers,
/// two spaces a level) as notes, the selection's anchor and the caret.
fn parse(golden: &str) -> (Vec<Note>, Pos, Pos) {
    let g = golden.replace(" ¦ ", " ‖ ");
    let mut notes = Vec::new();
    let (mut head, mut anchor, mut open): (Option<Pos>, Option<Pos>, Option<Pos>) = (None, None, None);
    for (i, part) in g.split(" ‖ ").enumerate() {
        let bare: String = part.chars().filter(|c| !"⟦⟧▮".contains(*c)).collect();
        let ind = bare.len() - bare.trim_start_matches(' ').len();
        let depth = ind / 2;
        let rest = &bare[depth * 2..];
        let (mark, kind, status) = if rest.starts_with("[ ] ") {
            ("[ ] ", Kind::Task, Some("todo".to_string()))
        } else if rest.starts_with("[x] ") {
            ("[x] ", Kind::Task, Some("done".to_string()))
        } else if rest.starts_with("- ") {
            ("- ", Kind::Bullet, None)
        } else {
            ("", Kind::Para, None)
        };
        let skip = depth * 2 + mark.chars().count();
        let (mut text, mut n) = (String::new(), 0);
        for c in part.chars() {
            if "⟦⟧▮".contains(c) {
                let at = (i, text.chars().count());
                match c {
                    '⟦' => open = Some(at),
                    '⟧' => anchor = Some(if head == open { at } else { open.unwrap_or(at) }),
                    _ => head = Some(at),
                }
                continue;
            }
            if n < skip {
                n += 1;
                continue;
            }
            text.push(if c == '⏎' { '\n' } else { c });
        }
        notes.push(Note { depth, kind, status, text });
    }
    let head = head.unwrap_or_else(|| panic!("no caret in {golden:?}"));
    (notes, anchor.unwrap_or(head), head)
}

/// Notes and a selection back as a golden.
fn show(notes: &[Note], anchor: Pos, head: Pos) -> String {
    let (lo, hi) = if anchor != head { (Some(anchor.min(head)), Some(anchor.max(head))) } else { (None, None) };
    let mut parts = Vec::new();
    for (i, n) in notes.iter().enumerate() {
        let mark = match n.kind {
            Kind::Task if n.status.as_deref() == Some("done") => "[x] ",
            Kind::Task => "[ ] ",
            // An empty line's kind isn't text yet: a page's fresh line reads `▮`.
            Kind::Bullet if n.text.is_empty() => "",
            Kind::Bullet => "- ",
            _ => "",
        };
        let mut s = "  ".repeat(n.depth);
        let dec_open = lo == Some((i, 0));
        if dec_open {
            s.push('⟦');
        }
        s.push_str(mark);
        let chars: Vec<char> = n.text.chars().collect();
        for j in 0..=chars.len() {
            if lo == Some((i, j)) && !dec_open {
                s.push('⟦');
            }
            if head == (i, j) {
                s.push('▮');
            }
            if hi == Some((i, j)) {
                s.push('⟧');
            }
            if let Some(c) = chars.get(j) {
                s.push(if *c == '\n' { '⏎' } else { *c });
            }
        }
        parts.push(s);
    }
    parts.join(" ‖ ")
}

fn norm(golden: &str) -> String {
    let (n, a, h) = parse(golden);
    show(&n, a, h)
}

/// The open document as a golden. The fresh empty line a page opens with is left out unless the
/// caret or the selection reaches it.
fn shown(f: &Flow) -> Result<String, String> {
    let d = f.pane_doc().ok_or("no document open")?;
    let pos = |p: crate::editor::BlockPos| (p.line, d.blocks()[p.line].text[..p.byte].chars().count());
    let head = pos(d.caret());
    let anchor = d.anchor().map(pos).unwrap_or(head);
    let mut notes: Vec<Note> = d.blocks().iter().map(|l| Note { depth: l.depth, kind: l.kind(), status: l.status.clone(), text: l.text.clone() }).collect();
    while notes.len() > 1 && notes.last().is_some_and(|n| n.text.is_empty()) && head.0 < notes.len() - 1 && anchor.0 < notes.len() - 1 {
        notes.pop();
    }
    Ok(show(&notes, anchor, head))
}

/// A key by the scenarios' names (`d-s-z`: c ⌃, a ⌥, d ⌘, s ⇧) as a thc key-script token.
fn token(name: &str) -> String {
    let mut mods = String::new();
    let mut rest = name;
    while rest.len() > 2 && rest.as_bytes()[1] == b'-' && "cads".contains(&rest[..1]) {
        mods.push_str(&rest[..1]);
        rest = &rest[2..];
    }
    let base = match rest {
        "enter" => "cr",
        r => r,
    };
    if mods.is_empty() && base.chars().count() == 1 {
        return if base == "<" { "<lt>".into() } else { base.into() };
    }
    let pre: String = [('c', "c-"), ('a', "m-"), ('s', "s-"), ('d', "d-")].iter().filter(|(m, _)| mods.contains(*m)).map(|(_, p)| *p).collect();
    format!("<{pre}{base}>")
}

/// `Hello wo|rld`: the document place of the bar, on the line reading `Hello world`.
fn place(spec: &str) -> At {
    let byte = spec.find('|').unwrap_or(0);
    doc_at(&spec.replace('|', ""), byte)
}

/// Every key of `want` is in `got` with the same value.
fn subset(want: &Value, got: &Value, path: &str) -> Vec<String> {
    match want {
        Value::Object(m) => m.iter().flat_map(|(k, v)| subset(v, got.get(k).unwrap_or(&Value::Null), &format!("{path}.{k}"))).collect(),
        _ if want == got => vec![],
        _ => vec![format!("{path}: want {want}, got {got}")],
    }
}

/// An expectation for this target: its own block (keyed by target name) over the shared keys.
fn for_target(e: &Value) -> serde_json::Map<String, Value> {
    let mut out: serde_json::Map<String, Value> = e.as_object().cloned().unwrap_or_default().into_iter().filter(|(k, _)| !k.starts_with("thc-") && k != "fictty-notes").collect();
    if let Some(Value::Object(m)) = e.get(TARGET) {
        out.extend(m.clone());
    }
    out
}

fn check(f: &mut Flow, e: &serde_json::Map<String, Value>) -> Result<(), String> {
    if let Some(Value::String(doc)) = e.get("doc") {
        let got = shown(f)?;
        if got != norm(doc) {
            return Err(format!("doc: want {}\n     got  {got}", norm(doc)));
        }
    }
    if let Some(Value::String(clip)) = e.get("clip") {
        let got = crate::runtime_effects::SNAPSHOT_CLIPBOARD.with(|c| c.borrow().clone());
        if got.as_deref() != Some(clip.as_str()) {
            return Err(format!("clipboard: want {clip:?}, got {got:?}"));
        }
    }
    if e.contains_key("caret_line_min") || e.contains_key("caret_line_max") {
        let line = f.pane_doc().ok_or("no document open")?.caret().line as u64;
        if let Some(min) = e.get("caret_line_min").and_then(Value::as_u64).filter(|m| line < *m) {
            return Err(format!("caret: want it on note {min} or later, it's on {line}"));
        }
        if let Some(max) = e.get("caret_line_max").and_then(Value::as_u64).filter(|m| line > *m) {
            return Err(format!("caret: want it on note {max} or earlier, it's on {line}"));
        }
    }
    let frame = f.shot.frame();
    let list = |v: Option<&Value>| -> Vec<String> {
        match v {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a.iter().filter_map(|s| s.as_str().map(String::from)).collect(),
            _ => vec![],
        }
    };
    for t in list(e.get("screen")) {
        if !frame.contains(&t) {
            return Err(format!("screen: want {t:?} on screen:\n{frame}"));
        }
    }
    for t in list(e.get("no_screen")) {
        if frame.contains(&t) {
            return Err(format!("screen: want no {t:?}:\n{frame}"));
        }
    }
    for (key, last) in [("caret_at_row_start", true), ("caret_at_row_end", false)] {
        let Some(Value::String(needle)) = e.get(key) else { continue };
        let rows: Vec<&str> = frame.lines().filter(|r| r.contains(needle.as_str())).collect();
        let row = (if last { rows.last() } else { rows.first() }).ok_or(format!("{needle:?} isn't on screen:\n{frame}"))?;
        let i = if last { row.rfind(needle.as_str()) } else { row.find(needle.as_str()) }.unwrap_or(0);
        let start = row[..i].rfind(['│', '┃', '║']).map_or(0, |b| b + '│'.len_utf8());
        let end = row[i..].find(['│', '┃', '║']).map_or(row.len(), |b| i + b);
        let seg = row[start..end].trim();
        let d = f.pane_doc().ok_or("no document open")?;
        let c = d.caret();
        let text = &d.blocks()[c.line].text;
        let ok = if last { text[c.byte..].starts_with(seg) } else { text[..c.byte].trim_end_matches(' ').ends_with(seg) };
        if !ok {
            return Err(format!("caret: want it at the {} of the row {seg:?}; the note reads {:?} around byte {}", if last { "start" } else { "end" }, text, c.byte));
        }
    }
    let rows: Vec<&str> = frame.lines().map(|r| r.trim_matches(|c: char| " │▌┃║".contains(c))).collect();
    if let Some(Value::String(t)) = e.get("row_starts") {
        if !rows.iter().any(|r| r.starts_with(t.as_str())) {
            return Err(format!("screen: want a row starting {t:?}:\n{frame}"));
        }
    }
    if let Some(Value::String(t)) = e.get("row_ends") {
        if !rows.iter().any(|r| r.ends_with(t.as_str())) {
            return Err(format!("screen: want a row ending {t:?}:\n{frame}"));
        }
    }
    if let Some(want) = e.get("thc_state") {
        let got = f.s.state().to_json();
        let probs = subset(want, &got, "");
        if !probs.is_empty() {
            return Err(format!("state: {}", probs.join("; ")));
        }
    }
    Ok(())
}

/// One scenario: Ok, Err(why), or a skip (Err starting `skip:`).
fn run(s: &Value) -> Result<(), String> {
    let id = s["id"].as_str().unwrap_or("?");
    let setup = s["setup"].as_str().ok_or("skip: not an editing golden")?;
    let (notes, anchor, head) = parse(setup);
    let title = format!("g{}z", id.to_lowercase().chars().filter(char::is_ascii_alphanumeric).collect::<String>());
    let seed_notes = notes.clone();
    let seed_title = title.clone();
    crate::runtime_effects::SNAPSHOT_CLIPBOARD.with(|c| *c.borrow_mut() = None);
    let mut f = Flow::new_seeded(&format!("e2e {id}"), Size::Small, (120, 36), Checks::default(), move |v| {
        use thc_core::builder::TxBuilder;
        let today = thc_core::dates::today();
        v.transact(|st| {
            let mut b = TxBuilder::new(st, today);
            let page = b.create_page(&seed_title, &[])?;
            let mut parents: Vec<String> = Vec::new();
            for n in seed_notes.iter().filter(|n| !(seed_notes.len() == 1 && n.text.is_empty())) {
                parents.truncate(n.depth);
                let parent = parents.last().cloned().unwrap_or_else(|| page.clone());
                let mut props = serde_json::Map::new();
                match n.kind {
                    Kind::Para => {
                        props.insert(thc_core::edit::STYLE.into(), thc_core::edit::PARA.into());
                    }
                    Kind::Task => {
                        props.insert("status".into(), n.status.clone().unwrap_or_else(|| "todo".into()).into());
                    }
                    _ => {}
                }
                let nid = b.create(Some(parent), &n.text, None, props, &[], None)?;
                parents.push(nid);
            }
            Ok((b.finish()?, ()))
        })
        .expect("seeding the golden's page");
    });
    f.named(&format!("e2e {id}"));
    let r = drive(&mut f, s, &title, &notes, anchor, head);
    match r {
        Ok(()) => {
            f.done();
            Ok(())
        }
        Err(e) => {
            // A failed scenario ends here; the flow's own end-of-story checks are for a pass.
            f.done = true;
            Err(e)
        }
    }
}

fn drive(f: &mut Flow, s: &Value, title: &str, notes: &[Note], anchor: Pos, head: Pos) -> Result<(), String> {
    let setup = s["setup"].as_str().unwrap_or_default();
    f.keys(&format!("<c-o>{title}<cr>"));
    let off = |p: Pos| notes[..p.0].iter().map(|n| n.text.chars().count() + 1).sum::<usize>() + p.1;
    let (n, m) = (off(anchor), off(head));
    let mut script = String::from("<c-home>");
    script.push_str(&"<right>".repeat(n));
    script.push_str(&(if m >= n { "<s-right>" } else { "<s-left>" }).repeat(m.abs_diff(n)));
    f.keys(&script);
    let got = shown(f)?;
    if got != norm(setup) {
        return Err(format!("setup: want {}\n       got  {got}", norm(setup)));
    }
    let clip_before = s["clip_before"].as_str();
    for st in s["steps"].as_array().cloned().unwrap_or_default() {
        if let Some(k) = st["key"].as_str() {
            match (k, clip_before) {
                // ⌘V reads the system clipboard, which a flow doesn't have: the paste stands in.
                ("d-v" | "c-v", Some(clip)) => f.paste(clip),
                ("d-v" | "c-v", None) => match crate::runtime_effects::SNAPSHOT_CLIPBOARD.with(|c| c.borrow().clone()) {
                    Some(clip) => f.paste(&clip),
                    None => f.keys(&token(k)),
                },
                _ => f.keys(&token(k)),
            };
        } else if let Some(t) = st["type"].as_str() {
            f.type_text(t);
        } else if let Some(t) = st["paste"].as_str() {
            f.paste(t);
        } else if let Some(p) = st["click"].as_str() {
            f.click(place(p));
        } else if let Some(p) = st["sclick"].as_str() {
            f.shift_click(place(p));
        } else if let Some(p) = st["dclick"].as_str() {
            f.double_click(place(p));
        } else if let Some(p) = st["tclick"].as_str() {
            f.triple_click(place(p));
        } else if let Some(d) = st["drag"].as_array() {
            if st["double"].as_bool() == Some(true) {
                return Err("skip: a flow has no double-click-then-drag gesture".into());
            }
            f.drag(place(d[0].as_str().unwrap_or_default()), place(d[1].as_str().unwrap_or_default()));
        } else if st.get("assert").is_some() {
            check(f, &for_target(&st["assert"]))?;
        } else if st.get("bytes").is_some() {
            return Err("skip: raw bytes need a terminal".into());
        } else if st.get("wait").is_some() {
            f.idle();
        }
    }
    let mut e = for_target(&s["expect"]);
    if let Some(Value::Object(m)) = s["expect_by_target"].get(TARGET) {
        e.extend(m.clone());
    }
    check(f, &e)
}

fn first(e: &str) -> &str {
    e.lines().find(|l| !l.trim().is_empty()).unwrap_or_default()
}

#[test]
fn the_notation_round_trips() {
    for g in ["Hello ⟦wor▮⟧ld", "Hello ⟦▮wor⟧ld", "- o⟦ne ‖   - tw▮⟧o", "[ ] ▮A ‖ [x] B", "⟦ab ‖ cd▮⟧", "one ▮two⏎three", "▮"] {
        assert_eq!(norm(g), g.replace(" ¦ ", " ‖ "));
    }
    let (n, a, h) = parse("- o⟦ne ‖   - tw▮⟧o");
    assert_eq!((n[1].depth, n[1].kind, a, h), (1, Kind::Bullet, (0, 1), (1, 2)));
}

#[test]
fn the_e2e_scenarios_hold_headless() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/e2e/scenarios");
    let mut files: Vec<_> = std::fs::read_dir(&dir).expect("scripts/e2e/scenarios").filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    let only = std::env::var("E2E_ONLY").ok();
    let (mut bad, mut report) = (Vec::new(), Vec::new());
    for path in files {
        let s: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let id = s["id"].as_str().unwrap_or("?").to_string();
        let targets: Vec<&str> = s["targets"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_else(|| vec![TARGET]);
        if !targets.contains(&TARGET) || s.get("setup").is_none() || only.as_deref().is_some_and(|o| o != id) {
            continue;
        }
        let known = s["known"].get(format!("{TARGET}/headless")).or_else(|| s["known"].get(TARGET)).and_then(Value::as_str).map(String::from);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&s))).unwrap_or_else(|p| {
            Err(p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "panicked".into()))
        });
        let line = match (&r, &known) {
            (Err(e), _) if e.starts_with("skip:") => format!("skip  {id:6} {e}"),
            (Ok(()), None) => format!("PASS  {id}"),
            (Ok(()), Some(k)) => {
                bad.push(format!("{id}: passes now; drop its known failure ({k})"));
                format!("XPASS {id}  ({k})")
            }
            (Err(e), Some(k)) => format!("XFAIL {id}  [{k}] {}", first(e)),
            (Err(e), None) => {
                bad.push(format!("{id} ({}): {e}", path.file_name().unwrap().to_string_lossy()));
                format!("FAIL  {id}  {}", first(e))
            }
        };
        report.push(line);
    }
    eprintln!("{}", report.join("\n"));
    assert!(bad.is_empty(), "e2e scenarios, headless:\n{}", bad.join("\n\n"));
}
