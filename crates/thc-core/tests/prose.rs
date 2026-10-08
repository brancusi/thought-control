//! Prose on pages (tui-handoff §10.8, SPEC §6.2): top-level plain notes round-trip through
//! `$EDITOR` as paragraphs. A paragraph wrapped over several lines is one note; a blank line
//! ends it; a trailing `\` keeps a line break; bullets stay bullets.

use thc_core::builder::TxBuilder;
use thc_core::edit;
use thc_core::event::Actor;
use thc_core::vault::{self, Paths, Vault};

fn setup() -> (Vault, String, std::path::PathBuf) {
    let root = thc_core::scratch::dir(&format!("thc-prose-{}", thc_core::id::new_id()));
    vault::init(&root.join("v"), None, None).unwrap();
    let paths = Paths { vault: root.join("v"), cache: root.join("cache") };
    let mut v = Vault::open(paths, Actor { kind: "human".into(), name: None }, "test").unwrap();
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
    let (_, page) = v
        .transact(|s| {
            let mut b = TxBuilder::new(s, today);
            let p = b.create_page("Notes", &[])?;
            Ok((b.finish(), p))
        })
        .unwrap();
    (v, page, root)
}

fn save(v: &mut Vault, page: &str, text: &str) {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
    let rendered = edit::render(&v.store, page).unwrap();
    v.transact(|s| {
        let mut b = TxBuilder::new(s, today);
        edit::apply(&mut b, &rendered, text)?;
        Ok((b.finish(), ()))
    })
    .unwrap();
}

#[test]
fn a_wrapped_paragraph_stays_one_note() {
    let (mut v, page, root) = setup();
    // Written in the editor: two paragraphs (the first wrapped over two lines), a bullet list.
    save(
        &mut v,
        &page,
        "# Notes\n\nThe quarter is about fewer, deeper bets,\nwith time to finish what we start.\n\nSecond thought.\n\n- [ ] Draft the memo due:2026-10-09\n- A bullet note\n",
    );
    let kids = v.store.children(&page).unwrap();
    let texts: Vec<String> = kids.iter().map(|k| k.text.clone()).collect();
    assert_eq!(texts[0], "The quarter is about fewer, deeper bets, with time to finish what we start.", "one note, lines joined by a space");
    assert_eq!(texts[1], "Second thought.");
    assert_eq!(kids[2].status.as_deref(), Some("todo"));
    assert_eq!(kids.len(), 4);

    // Rendered back: paragraphs without "- ", the id on the last line, blank lines between;
    // the task stays a bullet. (The plain bullet note is prose now: no status, no children.)
    let r = edit::render(&v.store, &page).unwrap();
    let short = v.store.short(&kids[0].id);
    assert!(r.text.contains(&format!("The quarter is about fewer, deeper bets, with time to finish what we start. ^{short}\n\nSecond thought.")), "{}", r.text);
    assert!(r.text.contains("- [ ] Draft the memo"), "{}", r.text);

    // Edit → save → edit: unchanged text writes nothing and keeps identity.
    let before = v.store.children(&page).unwrap();
    save(&mut v, &page, &r.text);
    let after = v.store.children(&page).unwrap();
    assert_eq!(before.iter().map(|n| (&n.id, &n.text)).collect::<Vec<_>>(), after.iter().map(|n| (&n.id, &n.text)).collect::<Vec<_>>());

    // Re-wrapping a paragraph in the editor keeps it one note with the same id.
    let rewrapped = r.text.replace("fewer, deeper bets, with time", "fewer, deeper bets,\nwith time");
    save(&mut v, &page, &rewrapped);
    let n = v.store.node(&kids[0].id).unwrap().unwrap();
    assert_eq!(n.text, "The quarter is about fewer, deeper bets, with time to finish what we start.");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_trailing_backslash_keeps_a_line_break() {
    let (mut v, page, root) = setup();
    save(&mut v, &page, "# Notes\n\nRoses are red, \\\nviolets are blue.\n");
    let kids = v.store.children(&page).unwrap();
    assert_eq!(kids.len(), 1);
    assert_eq!(kids[0].text, "Roses are red,\nviolets are blue.");
    let r = edit::render(&v.store, &page).unwrap();
    assert!(r.text.contains("Roses are red, \\\nviolets are blue. ^"), "{}", r.text);
    let _ = std::fs::remove_dir_all(root);
}

/// The data-safety property: an unchanged open → save of a page mixing bullets, tasks, nested
/// items and paragraphs writes nothing, and adjacent bullets stay separate notes.
#[test]
fn an_unchanged_edit_writes_nothing() {
    let (mut v, page, root) = setup();
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
    // Bullets made outside the editor (CLI/TUI capture), then a paragraph and a task via the editor.
    v.transact(|s| {
        let mut b = TxBuilder::new(s, today);
        let cap = thc_core::capture::parse("A bullet item", today)?;
        b.create_from_capture(Some(page.clone()), &cap, None)?;
        let cap = thc_core::capture::parse("Another bullet", today)?;
        b.create_from_capture(Some(page.clone()), &cap, None)?;
        Ok((b.finish(), ()))
    })
    .unwrap();
    let r0 = edit::render(&v.store, &page).unwrap();
    let with_more = r0.text.replacen("<!--", "A paragraph written in the editor,\nwrapped over two lines.\n\n- [ ] A task due:2026-10-09\n    - A nested note\n\n<!--", 1);
    save(&mut v, &page, &with_more);
    let kids = v.store.children(&page).unwrap();
    let texts: Vec<&str> = kids.iter().map(|k| k.text.as_str()).collect();
    assert!(texts.contains(&"A bullet item") && texts.contains(&"Another bullet"), "adjacent bullets stay two notes: {texts:?}");
    assert_eq!(kids.len(), 4, "{texts:?}");

    // Bullets render as bullets, the paragraph as a paragraph.
    let r = edit::render(&v.store, &page).unwrap();
    assert!(r.text.contains("- A bullet item ^") && r.text.contains("- Another bullet ^"), "{}", r.text);
    assert!(r.text.contains("A paragraph written in the editor, wrapped over two lines. ^"), "{}", r.text);
    assert!(!r.text.contains("- A paragraph"), "{}", r.text);

    // Unchanged open → save: zero events.
    let before = v.store.max_okey().unwrap();
    save(&mut v, &page, &r.text);
    assert_eq!(v.store.max_okey().unwrap(), before, "an unchanged edit must write nothing:\n{}", r.text);
    let _ = std::fs::remove_dir_all(root);
}
