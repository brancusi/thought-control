//! Writing in documents (tui-editor.md: Write by default, Esc to Navigate), end to end: keys
//! replayed through a snapshot with THC_TUI_SNAPSHOT_WRITE=1 against a temp vault, then the
//! result read back with the CLI. A page opens with the caret at the start of its first line.

mod common;

use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn thc(root: &Path, env: &[(&str, &str)], args: &[&str]) -> String {
    let mut c = common::thc();
    c.args(args).current_dir(root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env("THC_NOW", "2026-10-03T09:00").env_remove("THC_ACTOR");
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into()
}

fn keys(root: &Path, k: &str) -> String {
    thc(root, &[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", k), ("THC_TUI_SNAPSHOT_WRITE", "1")], &["tui"])
}

fn children(root: &Path, page: &str) -> Vec<Value> {
    let v: Value = serde_json::from_str(&thc(root, &[], &["--json", "q", &format!("parent:{page}"), "sort:title"])).unwrap();
    v["items"].as_array().unwrap().clone()
}

pub fn setup(name: &str) -> (std::path::PathBuf, String) {
    let root = std::env::temp_dir().join(format!("thc-edit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    let page: Value = serde_json::from_str(&thc(&root, &[], &["--json", "page", "new", "Plans"])).unwrap();
    let id = page["nodes"][0]["id"].as_str().unwrap().to_string();
    thc(&root, &[], &["add", "First line", "--under", &id]);
    (root, id)
}

#[test]
fn lines_save_on_leave_with_tokens_parsed() {
    let (root, page) = setup("save");
    let r = &root;
    // Open the page (Pages → filter → Enter), then a new line below the first, with a token.
    // Esc saves and goes back to the Pages list it came from (navigation.md §2).
    let frame = keys(r, "4Plans<cr><c-e><cr>Call printer due:fri<cr>Second<esc>");
    assert!(frame.contains("Pages  1"), "Esc lands on the Pages list: {frame}");
    let kids = children(r, &page);
    let texts: Vec<&str> = kids.iter().map(|n| n["text"].as_str().unwrap()).collect();
    assert!(texts.contains(&"Call printer"), "{texts:?}");
    assert!(texts.contains(&"Second"), "{texts:?}");
    let printer = kids.iter().find(|n| n["text"] == "Call printer").unwrap();
    assert_eq!(printer["due"], "2026-10-09", "due:fri parsed on save and taken out of the text");
    // Editing an existing row: unchanged writes nothing; a change is one transaction.
    let before = thc(r, &[], &["--json", "log"]).len();
    keys(r, "4Plans<cr><esc>");
    assert_eq!(thc(r, &[], &["--json", "log"]).len(), before, "an unchanged line writes nothing");
    keys(r, "4Plans<cr><c-e>, edited !high<esc>");
    let first = children(r, &page).into_iter().find(|n| n["text"].as_str().unwrap().starts_with("First line")).unwrap();
    assert_eq!(first["text"], "First line, edited");
    assert_eq!(first["priority"], "high");
    // An empty new line is never saved.
    let n = children(r, &page).len();
    keys(r, "4Plans<cr><c-e><cr><esc>");
    assert_eq!(children(r, &page).len(), n);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn enter_tab_and_backspace_shape_the_outline() {
    let (root, page) = setup("keys");
    let r = &root;
    // Continuous writing: A, then B nested under A, then C at B's depth.
    keys(r, "4Plans<cr><c-e><cr>A<cr>B<tab><cr>C<esc>");
    let kids = children(r, &page);
    let a = kids.iter().find(|n| n["text"] == "A").expect("A at the top level").clone();
    let a_id = a["id"].as_str().unwrap();
    let under_a = children(r, a_id);
    let t: Vec<&str> = under_a.iter().map(|n| n["text"].as_str().unwrap()).collect();
    assert_eq!(t, ["B", "C"], "B nested with Tab, C continued at its depth");
    // Mid-line Enter splits into two lines.
    keys(r, "4Plans<cr><c-e><cr>Hello world<left><left><left><left><left><cr><esc>");
    let t: Vec<String> = children(r, &page).iter().map(|n| n["text"].as_str().unwrap().to_string()).collect();
    assert!(t.contains(&"Hello".to_string()) && t.contains(&"world".to_string()), "{t:?}");
    // An empty new line is never a node.
    let before = children(r, &page).len();
    keys(r, "4Plans<cr><c-e><cr><esc>");
    assert_eq!(children(r, &page).len(), before);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn page_ids_hidden_by_default_and_toggled() {
    let (root, page) = setup("ids");
    let r = &root;
    let first = children(r, &page)[0]["short"].as_str().unwrap().to_string();
    // IDs are hidden in documents (tui-editor.md §3); `y` in Navigate copies the line's.
    let frame = keys(r, "4Plans<cr>");
    assert!(frame.contains("First line") && !frame.contains(&first), "IDs hidden on pages: {frame}");
    let frame = keys(r, "4Plans<cr><c-e> more");
    assert!(frame.contains("First line more") && !frame.contains(&first), "and while writing: {frame}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_stray_key_in_the_finder_never_creates_a_page() {
    let (root, _) = setup("guard");
    let r = &root;
    let pages = || serde_json::from_str::<Value>(&thc(r, &[], &["--json", "pages"])).unwrap()["count"].as_u64().unwrap();
    let before = pages();
    // One character, Enter: a hint, nothing created.
    let frame = keys(r, "4x<cr>");
    assert!(frame.contains("type a longer name to create a page"), "{frame}");
    // No match, Enter: the hint, still nothing.
    let frame = keys(r, "4zzz<cr>");
    assert!(frame.contains("+ new page \"zzz\"") && frame.contains("↓ then Enter creates"), "{frame}");
    assert_eq!(pages(), before);
    // ↓ onto `+ new page`, then Enter: created, opened, first line in edit mode.
    let frame = keys(r, "4zzz<down><cr>");
    // (THC_NOW pins the clock, and its warning owns the bar here.)
    assert!(frame.contains("zzz"), "opened: {frame}");
    assert_eq!(pages(), before + 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn page_notes_read_as_paragraphs() {
    let (root, page) = setup("prose");
    let r = &root;
    let out = thc(r, &[], &["--json", "add", "The quarter is about fewer, deeper bets, with time to finish what we start and room to think about what comes after.", "--under", &page]);
    let id = serde_json::from_str::<Value>(&out).unwrap()["nodes"][0]["id"].as_str().unwrap().to_string();
    // Written as a paragraph ($EDITOR sets this); a plain note stays a bullet.
    thc(r, &[], &["set", &id, "style=para"]);
    let frame = keys(r, "4Plans<cr>");
    // Wrapped, not cut; no `·` note glyph; a blank row between paragraphs.
    assert!(frame.contains("fewer, deeper bets") && frame.contains("what comes after."), "{frame}");
    // (Past the header, whose vault name may be shortened.)
    assert!(!frame.lines().skip(1).any(|l| l.contains('…')), "a paragraph wraps instead of truncating: {frame}");
    // "First line" was added from the CLI: a bullet, with its `·`, not a paragraph.
    let lines: Vec<&str> = frame.lines().collect();
    let i = lines.iter().position(|l| l.contains("First line")).unwrap();
    assert!(lines[i].contains("· First line"), "bullets stay bullets: {frame}");
    assert!(lines[i + 1].trim().is_empty(), "blank row between a bullet and a paragraph: {frame}");
    assert!(!lines.iter().any(|l| l.contains("· The quarter")), "the paragraph has no `·`: {frame}");
    let _ = std::fs::remove_dir_all(&root);
}

/// With the daemon live, saves go through a writer thread (blocks.apply over the socket):
/// typing never waits, and everything lands.
#[test]
fn saves_go_through_the_daemon_when_it_is_live() {
    let (root, page) = setup("daemon");
    let r = &root;
    thc(r, &[("THC_TEST_DAEMON", "1")], &["daemon", "start"]);
    let frame = thc(r, &[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", "4Plans<cr><c-e><cr>Through the daemon due:fri<cr>And another<esc>"), ("THC_TUI_SNAPSHOT_WRITE", "1"), ("THC_TUI_SNAPSHOT_DAEMON", "1")], &["tui"]);
    let kids = children(r, &page);
    thc(r, &[], &["daemon", "stop"]);
    let texts: Vec<&str> = kids.iter().map(|n| n["text"].as_str().unwrap()).collect();
    assert!(texts.contains(&"Through the daemon") && texts.contains(&"And another"), "{texts:?}\n{frame}");
    let first = kids.iter().find(|n| n["text"] == "Through the daemon").unwrap();
    assert_eq!(first["due"], "2026-10-09", "tokens parsed by the daemon's save");
    let _ = std::fs::remove_dir_all(&root);
}

/// `thc j [date]` and `thc p <page>` open the TUI on that document, in Write.
#[test]
fn thc_j_and_thc_p_open_documents_in_write() {
    let (root, _) = setup("jp");
    let r = &root;
    let snap = |args: &[&str]| thc(r, &[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", "")], args);
    let frame = snap(&["j", "--no-focus"]);
    assert!(frame.contains("SAT 03 OCT 2026 · TODAY"), "today's journal: {frame}");
    let frame = snap(&["j", "2026-10-01", "--no-focus"]);
    assert!(frame.contains("THU 01 OCT 2026"), "a given day: {frame}");
    let frame = snap(&["j", "yesterday", "--no-focus"]);
    assert!(frame.contains("FRI 02 OCT 2026 · YESTERDAY"), "a word: {frame}");
    let frame = snap(&["p", "Plans"]);
    assert!(frame.contains("Plans") && frame.contains("First line"), "the page by title: {frame}");
    let frame = thc(r, &[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", "Hello from j"), ("THC_TUI_SNAPSHOT_WRITE", "1")], &["j"]);
    assert!(frame.contains("Hello from j"), "typing lands in the day: {frame}");
    // Esc saves it (and goes to Today).
    thc(r, &[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", "Hello from j<esc>"), ("THC_TUI_SNAPSHOT_WRITE", "1")], &["j"]);
    let day: serde_json::Value = serde_json::from_str(&thc(r, &[], &["--json", "q", "journal=today"])).unwrap();
    assert!(day["items"].as_array().unwrap().iter().any(|n| n["text"] == "Hello from j"), "{day}");
    let _ = std::fs::remove_dir_all(&root);
}

/// ⌃T acts on the line under the caret, not the first one it touched.
#[test]
fn x_follows_the_cursor_in_a_document() {
    let (root, _) = setup("xfollow");
    let r = &root;
    for t in ["[ ] First task", "[ ] Second task", "[ ] Third task"] {
        thc(r, &[], &["add", t]);
    }
    thc(r, &[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", "5<up><up><up><c-t><down><c-t><esc>"), ("THC_TUI_SNAPSHOT_WRITE", "1")], &["tui"]);
    let day: serde_json::Value = serde_json::from_str(&thc(r, &[], &["--json", "q", "journal=today", "sort:created"])).unwrap();
    let status = |t: &str| day["items"].as_array().unwrap().iter().find(|n| n["text"] == t).map(|n| n["status"].as_str().unwrap_or("").to_string()).unwrap_or_default();
    assert_eq!(status("First task"), "done");
    assert_eq!(status("Second task"), "done", "x after j completes the second, not the first again");
    assert_eq!(status("Third task"), "todo");
    let _ = std::fs::remove_dir_all(&root);
}

/// `thc j` opens in focus by default (only the text), `--no-focus` with the full screen, and
/// `[tui] journal = "normal"` in config.toml changes the default.
#[test]
fn thc_j_focus_is_config_driven() {
    let (root, _) = setup("focus");
    let r = &root;
    thc(r, &[], &["add", "Slept well"]);
    let snap = |env: &[(&str, &str)], args: &[&str]| {
        let mut e = vec![("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", "")];
        e.extend_from_slice(env);
        thc(r, &e, args)
    };
    let frame = snap(&[], &["j"]);
    assert!(frame.contains("Slept well"), "{frame}");
    assert!(!frame.contains("Journal"), "focus: no tabs: {frame}");
    let frame = snap(&[("THC_TUI_FOCUS", "bare")], &["j"]);
    assert!(!frame.contains("TODAY") && !frame.contains("Journal") && !frame.contains("writing"), "bare focus: only the text: {frame}");
    let frame = snap(&[], &["j", "--no-focus"]);
    assert!(frame.contains("SAT 03 OCT 2026 · TODAY") && frame.contains("Journal"), "normal: the full screen: {frame}");
    // config.toml decides the default.
    let cfg = root.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), "[tui]\njournal = \"normal\"\n").unwrap();
    let cfg_s = cfg.to_string_lossy().to_string();
    let frame = snap(&[("THC_CONFIG_DIR", &cfg_s)], &["j"]);
    assert!(frame.contains("TODAY"), "journal = normal: {frame}");
    let frame = snap(&[("THC_CONFIG_DIR", &cfg_s)], &["j", "--focus"]);
    assert!(!frame.contains("Journal"), "the flag beats the file: {frame}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_document_footer_and_help_agree() {
    let (root, _) = setup("footer");
    let r = &root;
    thc(r, &[], &["add", "Slept well"]);
    // THC_NOW="" unpins the clock: a pinned clock's warning owns the bar.
    let snap = |size: &str, keys: &str| thc(r, &[("THC_TUI_SNAPSHOT", size), ("THC_TUI_KEYS", keys), ("THC_NOW", "")], &["j", "--no-focus"]);
    let last = |f: String| f.lines().last().unwrap_or_default().to_string();
    // writing.md §4: the five keys, wide and at 80 (F1 keys, then the name, go first).
    let bar = last(snap("120x24", ""));
    assert!(bar.contains("§ ") && bar.contains(" words · autosaved") && bar.contains("⌃T task  ⌃O open  ⌃P ⌃N day  Esc done  F1 keys"), "{bar}");
    let bar = last(snap("80x24", ""));
    assert!(bar.contains("⌃T task  ⌃O open  ⌃P ⌃N day  Esc done") && !bar.contains("F1"), "{bar}");
    // A page has no day keys.
    let bar = last(thc(r, &[("THC_TUI_SNAPSHOT", "120x24"), ("THC_NOW", "")], &["p", "Plans"]));
    assert!(bar.contains("⌃T task  ⌃O open  Esc done  F1 keys") && !bar.contains("day"), "{bar}");
    // F1 (or ⌥?) shows the ten keys.
    for keys in ["<f1>", "<m-?>"] {
        let f = snap("100x30", keys);
        assert!(f.contains("Writing") && f.contains("text → [ ] → [x] → text") && f.contains("done: save, go back"), "{keys}: {f}");
    }
    // Help shows in Focus too.
    let f = thc(r, &[("THC_TUI_SNAPSHOT", "100x30"), ("THC_TUI_KEYS", "<f1>"), ("THC_NOW", "")], &["j", "--focus"]);
    assert!(f.contains("Writing"), "{f}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn focus_is_composable_and_config_driven() {
    let (root, _) = setup("compose");
    let r = &root;
    thc(r, &[], &["add", "Slept well, wrote a little"]);
    let cfg = root.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let cfg_s = cfg.to_string_lossy().to_string();
    // THC_NOW="" unpins the clock (its warning would own the footer).
    let snap = |env: &[(&str, &str)], size: &str, keys: &str| {
        let mut e = vec![("THC_TUI_SNAPSHOT", size), ("THC_TUI_KEYS", keys), ("THC_NOW", ""), ("THC_CONFIG_DIR", cfg_s.as_str())];
        e.extend_from_slice(env);
        thc(r, &e, &["j"])
    };
    // writer (the default): the date, the strip, the footer with a word count.
    let f = snap(&[], "120x24", "");
    assert!(f.contains("TODAY") && f.contains("autosaved") && f.contains("words") && !f.contains("Journal"), "{f}");
    // bare: the text, and a one-time note that the keys moved.
    let f = snap(&[("THC_TUI_FOCUS", "bare")], "120x24", "");
    assert!(!f.contains("TODAY") && !f.contains("autosaved") && f.contains("footer off · :focus to change"), "{f}");
    // planner at 140: the month replaces the strip; tabs come back as an element.
    let f = snap(&[("THC_TUI_FOCUS", "planner,+tabs")], "140x24", "");
    assert!(f.contains("Mo Tu We Th Fr Sa Su") && f.contains("Journal"), "{f}");
    // [tui.focus] in the file, with a typo warned about once.
    std::fs::write(cfg.join("config.toml"), "[tui.focus]\npreset = \"bare\"\nfooter = true\nfooterr = true\n").unwrap();
    let f = snap(&[], "120x24", "");
    assert!(f.contains("unknown key \"footerr\" · did you mean footer?") && !f.contains("TODAY"), "{f}");
    // :focus toggles live and Enter saves only what differs from the preset.
    let f = thc(r, &[("THC_TUI_SNAPSHOT", "120x30"), ("THC_TUI_KEYS", "<m-:>focus<cr>h<cr>"), ("THC_NOW", ""), ("THC_CONFIG_DIR", &cfg_s), ("THC_TUI_SNAPSHOT_WRITE", "1")], &["j"]);
    assert!(f.contains("TODAY"), "h turned the header on: {f}");
    let saved = std::fs::read_to_string(cfg.join("config.toml")).unwrap();
    assert!(saved.contains("preset = \"bare\"\nfooter = true\nheader = true\n"), "{saved}");
    // :focus commands.
    let f = thc(r, &[("THC_TUI_SNAPSHOT", "120x24"), ("THC_TUI_KEYS", "<m-:>focus planner -clock<cr>"), ("THC_NOW", ""), ("THC_CONFIG_DIR", &cfg_s)], &["j"]);
    assert!(f.contains("focus · custom"), "{f}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn keys_never_fall_through_to_list_commands() {
    let (root, page) = setup("fall");
    let r = &root;
    thc(r, &[], &["todo", "Call the bank", "--under", &page]);
    let status = |r: &Path| children(r, &page).into_iter().find(|n| n["text"].as_str().unwrap().starts_with("Call the bank")).unwrap()["status"].clone();
    // Write is sealed: ⌥X / ⌃X on the line type nothing and change nothing (D1).
    // THC_NOW="" unpins the clock: its warning would own the bar.
    let f = thc(r, &[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", "4Plans<cr><down><m-x><c-x><c-s>"), ("THC_TUI_SNAPSHOT_WRITE", "1"), ("THC_NOW", "")], &["tui"]);
    assert_eq!(status(r), "todo", "{f}");
    assert!(f.contains("⌥X isn't a writing key · F1 shows the keys"), "{f}");
    // In lists, ⌃X isn't x (D2).
    keys(r, "3<c-x>");
    assert_eq!(status(r), "todo");
    // The palette from Write runs the command on the caret's line; it types nothing (D3).
    keys(r, "4Plans<cr><down><m-:>Done<cr>");
    assert_eq!(status(r), "done");
    let texts: Vec<String> = children(r, &page).iter().map(|n| n["text"].as_str().unwrap().to_string()).collect();
    assert!(texts.iter().all(|t| !t.ends_with('x')), "nothing typed: {texts:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_caret_follows_a_soft_break_at_the_end_of_a_line() {
    // ⇧Enter / ⌃J at the end of a line: the caret is on the new empty row (it stayed above).
    let (root, _) = setup("softbreak");
    let f = thc(&root, &[("THC_TUI_SNAPSHOT", "80x24"), ("THC_TUI_KEYS", "abc<c-j>"), ("THC_TUI_SNAPSHOT_CURSOR", "1")], &["j", "--no-focus"]);
    let lines: Vec<&str> = f.lines().collect();
    let at = lines.iter().position(|l| l.contains("abc")).unwrap();
    assert!(!lines[at].contains('▮') && lines[at + 1].trim() == "▮", "{f}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_caret_sits_after_emoji_and_backspace_takes_whole_characters() {
    let (root, _) = setup("graphemes");
    let snap = |keys: &str| thc(&root, &[("THC_TUI_SNAPSHOT", "80x24"), ("THC_TUI_KEYS", keys), ("THC_TUI_SNAPSHOT_CURSOR", "1")], &["j", "--no-focus"]);
    let row = |f: &str, needle: &str| f.lines().find(|l| l.contains(needle)).unwrap_or_default().trim().to_string();
    // The caret right after the last character, whatever came before (it drifted 4 columns
    // after a family emoji and sat on the next letter after ❤️).
    assert_eq!(row(&snap("a👨‍👩‍👧b"), "a👨"), "a👨‍👩‍👧b▮");
    assert_eq!(row(&snap("a❤️b"), "a❤"), "a❤️b▮");
    // ⌫ takes the whole character: the family, a flag, an accented letter.
    assert_eq!(row(&snap("a👨‍👩‍👧<bs>b"), "▮"), "ab▮");
    assert_eq!(row(&snap("a🇯🇵<bs>b"), "▮"), "ab▮");
    assert_eq!(row(&snap("ae\u{301}<bs>b"), "▮"), "ab▮");
    // Typing a token never moves the lines below (the meta row follows the saved meta).
    // The caret stays on the token line, with a line below it.
    let a = snap("Call Sam<cr>Next line<up><c-e> due:");
    let b = snap("Call Sam<cr>Next line<up><c-e> due:monday !high #home every:week");
    assert!(b.contains("✓"), "the chip is up: {b}");
    let at = |f: &str| f.lines().position(|l| l.contains("Next line"));
    assert_eq!(at(&a), at(&b));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn typed_numbers_make_a_list() {
    // `1. ` typed makes a numbered item (the number in the hang), Enter numbers the
    // next, Enter on an empty item ends the list.
    let (root, _) = setup("numbered");
    let f = thc(&root, &[("THC_TUI_SNAPSHOT", "80x24"), ("THC_TUI_KEYS", "1. First<cr>Second<cr><cr>After")], &["j", "--no-focus"]);
    let rows: Vec<&str> = f.lines().map(str::trim_end).filter(|l| l.contains("First") || l.contains("Second") || l.contains("After")).collect();
    assert_eq!(rows, ["    1. First", "    2. Second", "       After"], "{f}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn editing_a_line_again_is_no_conflict() {
    // Write a line, leave it, come back and change it: the second save's base is the first
    // save's committed revision, so it's no conflict (0.8.x took the plan's provisional one).
    let (root, _) = setup("again");
    let r = &root;
    thc(r, &[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", "aaa<cr><cr>bbb<up><up><c-e>X<down><down>"), ("THC_TUI_SNAPSHOT_WRITE", "1")], &["j", "--no-focus"]);
    let c = thc(r, &[], &["--json", "conflict", "ls"]);
    let v: Value = serde_json::from_str(&c).unwrap();
    assert!(v.as_array().map_or_else(|| v["conflicts"].as_array().unwrap().is_empty(), |a| a.is_empty()), "{c}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn double_task_cycle_completes_a_newly_tasked_plain_line() {
    let (root, page) = setup("task-double");
    // Every press advances the model, even before the first kind change has been saved.
    let frame = thc(&root, &[("THC_TUI_SNAPSHOT", "80x24"), ("THC_TUI_KEYS", "<c-t><c-t>"), ("THC_TUI_SNAPSHOT_WRITE", "1")], &["p", "Plans"]);
    let kids = children(&root, &page);
    assert_eq!(kids.iter().find(|n| n["text"] == "First line").unwrap()["status"], "done", "{frame}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn double_task_cycle_completes_each_plain_line_while_moving_down() {
    let (root, page) = setup("task-double-down");
    for text in ["Second line", "Third line", "Fourth line", "Fifth line"] {
        thc(&root, &[], &["add", text, "--under", &page]);
    }
    let frame = thc(&root, &[("THC_TUI_SNAPSHOT", "80x24"), ("THC_TUI_KEYS", "<c-t><c-t><down><c-t><c-t>"), ("THC_TUI_SNAPSHOT_WRITE", "1")], &["p", "Plans"]);
    let kids = children(&root, &page);
    for text in ["First line", "Second line"] {
        assert_eq!(kids.iter().find(|n| n["text"] == text).unwrap()["status"], "done", "{text}: {frame}");
    }
    assert!(kids.iter().find(|n| n["text"] == "Third line").unwrap().get("status").is_none());
    let _ = std::fs::remove_dir_all(root);
}
