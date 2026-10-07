//! The sidebar's model and runtime under test (docs/design/sidebar.md §7.2, §11, §15): several
//! views on one document under random use (S25), the stack kept across launches (S20), a
//! panel's typing saved once into the right document.

use crate::app::{App, Focus};
use crate::editor::{BlockPos, Doc, MAIN_VIEW, Target};
use crate::sidebar::PanelKey;
use thc_core::builder::TxBuilder;

/// xorshift64*, seeded: the same on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn doc(texts: &[&str]) -> Doc {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
    let blocks: Vec<thc_core::outline::Block> = texts
        .iter()
        .enumerate()
        .map(|(i, t)| serde_json::from_value(serde_json::json!({"id": format!("n{i}"), "parent": null, "depth": 0, "kind": "bullet", "text": t, "text_rev": "r"})).unwrap())
        .collect();
    Doc::new(Target::Journal { date: today }, Some("root".into()), &blocks, today)
}

fn texts(d: &Doc) -> Vec<String> {
    d.blocks().iter().map(|l| l.text.clone()).collect()
}

/// Every view's caret is on a real place: a note that exists, a byte on a character boundary
/// inside its text (motion.md I1).
fn carets_valid(d: &mut Doc) {
    for v in d.views() {
        d.with_view(v, |d| {
            let c = d.caret();
            let l = &d.blocks()[c.line];
            assert!(c.byte <= l.text.len() && l.text.is_char_boundary(c.byte), "view {v}: caret {c:?} in {:?}", l.text);
        });
    }
}

/// S25: two to four views on one document, random messages through random views. After each
/// step every view reads the document's text and has a valid caret; undo through any view
/// takes the document back to where it started.
#[test]
fn s25_several_views_on_one_document_under_random_use() {
    let cases = std::env::var("THC_FUZZ_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(60u64);
    for case in 0..cases {
        let mut rng = Rng(0x5eed_0000 + case * 7919);
        let start = ["alpha beta", "gamma", "delta épsilon 🙂 zeta", "eta"];
        let mut d = doc(&start);
        d.fill_ids((0..400).map(|i| format!("id{case:04}{i:06}")).collect());
        let n_views = 2 + rng.below(3) as u32;
        for v in 1..n_views {
            d.add_view(v);
        }
        let mut now = 1_000u64;
        for _ in 0..60 {
            let v = rng.below(n_views as usize) as u32;
            now += 2_000; // each step its own undo step
            d.tick(now);
            d.with_view(v, |d| match rng.below(9) {
                0 => {
                    d.insert(["x", "yz", " ", "é", "🙂"][rng.below(5)]);
                }
                1 => {
                    d.run_command("edit.newline");
                }
                2 => {
                    d.run_command("edit.backspace");
                }
                3 => {
                    d.run_command(["move.left", "move.right", "move.up", "move.down", "move.line_end", "move.line_start"][rng.below(6)]);
                }
                4 => {
                    let line = rng.below(d.blocks().len());
                    let len = d.blocks()[line].text.len();
                    let mut byte = rng.below(len + 1);
                    while !d.blocks()[line].text.is_char_boundary(byte) {
                        byte -= 1;
                    }
                    d.set_caret(BlockPos { line, byte });
                }
                5 => {
                    d.run_command("structure.indent");
                }
                6 => {
                    d.run_command("history.undo");
                }
                7 => {
                    d.run_command("history.redo");
                }
                _ => {
                    d.run_command("select.word_right");
                }
            })
            .expect("a view");
            carets_valid(&mut d);
            // One document: whichever view is current, the text is the same.
            let main = d.with_view(MAIN_VIEW, |d| texts(d)).unwrap();
            for v in d.views() {
                assert_eq!(d.with_view(v, |d| texts(d)).unwrap(), main, "case {case}");
            }
            let rows: usize = d.blocks().iter().map(|l| l.text.split('\n').count()).sum();
            assert_eq!(d.engine_text().split('\n').count(), rows, "case {case}: the engine and the lines agree");
        }
        // Undo all the way back, through random views.
        for _ in 0..400 {
            let v = rng.below(n_views as usize) as u32;
            if d.with_view(v, |d| d.run_command("history.undo")).unwrap() == crate::editor::Outcome::Nothing("nothing to undo".into()) {
                break;
            }
        }
        assert_eq!(texts(&d), start, "case {case}: undo through any view restores the document");
        carets_valid(&mut d);
    }
}

/// An edit through one view keeps another view's caret on its text (caretline's rebase).
#[test]
fn an_edit_in_one_view_keeps_the_other_views_caret_on_its_text() {
    let mut d = doc(&["alpha", "beta gamma"]);
    d.add_view(1);
    d.with_view(1, |d| d.set_caret(BlockPos { line: 1, byte: 5 }));
    d.set_caret(BlockPos { line: 0, byte: 0 });
    d.insert("new ");
    d.run_command("edit.newline");
    let c = d.with_view(1, |d| (d.caret(), d.blocks()[d.caret().line].text.clone())).unwrap();
    assert_eq!(&c.1[c.0.byte..], "gamma", "{c:?}");
    assert_eq!(d.current_view(), MAIN_VIEW);
}

/// The views of a document read again from the vault come back at their places.
#[test]
fn views_survive_a_reread() {
    let mut d = doc(&["one", "two", "three"]);
    d.add_view(4);
    d.with_view(4, |d| d.set_caret(BlockPos { line: 2, byte: 3 }));
    d.use_view(4);
    let (cur, places) = d.view_places();
    let mut nd = doc(&["one", "two", "three"]);
    nd.adopt_views(cur, &places);
    assert_eq!(nd.current_view(), 4);
    assert_eq!(nd.caret(), BlockPos { line: 2, byte: 3 });
    assert!(nd.has_view(MAIN_VIEW));
    // A document only a panel shows: its one view keeps its id.
    let mut p = doc(&["x"]);
    p.rename_view(9);
    let (cur, places) = p.view_places();
    let mut np = doc(&["x"]);
    np.adopt_views(cur, &places);
    assert_eq!(np.views(), vec![9]);
}

fn page_vault(tag: &str) -> (crate::fuzz::Scratch, thc_core::vault::Vault, String, String) {
    let (s, mut v) = crate::fuzz::scratch(tag);
    let today = thc_core::dates::today();
    let mut ids = (String::new(), String::new());
    v.transact(|st| {
        let mut b = TxBuilder::new(st, today);
        let health = b.create_page("Health", &[])?;
        let lisbon = b.create_page("Lisbon flat", &[])?;
        b.create_from_capture(Some(health.clone()), &thc_core::capture::parse("Dr. Patel, 555-0100", today).unwrap(), None)?;
        b.create_from_capture(Some(lisbon.clone()), &thc_core::capture::parse("the deposit", today).unwrap(), None)?;
        ids = (health, lisbon);
        Ok((b.finish(), ()))
    })
    .unwrap();
    (s, v, ids.0, ids.1)
}

fn reopen(v: &thc_core::vault::Vault) -> thc_core::vault::Vault {
    thc_core::vault::Vault::open(v.paths.clone(), thc_core::event::Actor { kind: "human".into(), name: None }, "tui").unwrap()
}

/// S20: quit and relaunch, and the stack, pins, folds and carets come back; a deleted page's
/// panel is dropped, with one line in the bar.
#[test]
fn s20_the_stack_comes_back_after_a_relaunch() {
    crate::SNAPSHOT.with(|s| s.set(false));
    let (_s, v, health, lisbon) = page_vault("s20");
    let mut app = App::new(reopen(&v)).unwrap();
    app.daemon_live = false;
    let vault = app.ui.vault_name.clone();
    app.open_aside(PanelKey::page(&vault, &health), false);
    app.open_aside(PanelKey::page(&vault, &lisbon), false);
    crate::runtime_effects::sidebar_op(&mut app, crate::update::SidebarOp::Pin { key: PanelKey::page(&vault, &health) });
    // Type in Lisbon (the active panel), at the end of its first line.
    app.focus_panel(PanelKey::page(&vault, &lisbon));
    for c in "!".chars() {
        crate::input::handle_key(&mut app, ratatui::crossterm::event::KeyEvent::new(ratatui::crossterm::event::KeyCode::End, ratatui::crossterm::event::KeyModifiers::NONE));
        crate::input::handle_key(&mut app, ratatui::crossterm::event::KeyEvent::new(ratatui::crossterm::event::KeyCode::Char(c), ratatui::crossterm::event::KeyModifiers::NONE));
    }
    app.focus_main();
    app.save_everything();
    let stack = app.ui.sidebar.clone();
    drop(app);
    let saved = v.store.query("text:deposit", thc_core::dates::today(), 5).unwrap();
    assert_eq!(reopen(&v).store.query("text:deposit", thc_core::dates::today(), 5).unwrap()[0].text, "the deposit!", "{saved:?}");
    let mut again = App::new(reopen(&v)).unwrap();
    again.daemon_live = false;
    assert_eq!(again.ui.sidebar.open, stack.open, "the stack, pins and carets came back");
    assert!(again.ui.sidebar.open[0].pinned);
    let c = again.ui.sidebar.get(&PanelKey::page(&vault, &lisbon)).unwrap().doc_view().unwrap().caret.clone().unwrap();
    assert_eq!(c.byte, "the deposit!".len());
    drop(again);
    // Lisbon is deleted elsewhere: the next launch drops it and says so once.
    let mut w = reopen(&v);
    let today = thc_core::dates::today();
    w.transact(|st| {
        let mut b = TxBuilder::new(st, today);
        b.delete(&lisbon)?;
        Ok((b.finish(), ()))
    })
    .unwrap();
    let third = App::new(reopen(&v)).unwrap();
    assert_eq!(third.ui.sidebar.open.len(), 1, "{:?}", third.ui.sidebar.open);
    let toast: String = third.ui.toast.as_ref().map(|t| t.parts.iter().map(|(s, _)| s.as_str()).collect()).unwrap_or_default();
    assert!(toast.contains("Lisbon flat was deleted · removed from the sidebar"), "{toast}");
    assert_eq!(third.ui.focus, Focus::List);
}

/// A panel's own document saves into its page, not the main view's (one transaction).
#[test]
fn a_panels_typing_saves_into_its_own_page() {
    crate::SNAPSHOT.with(|s| s.set(true));
    let (_s, v, health, _) = page_vault("own");
    let mut app = App::new(reopen(&v)).unwrap();
    app.daemon_live = false;
    let vault = app.ui.vault_name.clone();
    // Main: the journal (typing there too).
    app.ui.view = crate::app::View::Journal;
    let _ = app.reload();
    app.open_aside(PanelKey::page(&vault, &health), false);
    crate::input::handle_key(&mut app, ratatui::crossterm::event::KeyEvent::new(ratatui::crossterm::event::KeyCode::Char('s'), ratatui::crossterm::event::KeyModifiers::ALT));
    assert_eq!(app.ui.focus, Focus::Sidebar);
    for c in "Note ".chars() {
        crate::input::handle_key(&mut app, ratatui::crossterm::event::KeyEvent::new(ratatui::crossterm::event::KeyCode::Char(c), ratatui::crossterm::event::KeyModifiers::NONE));
    }
    crate::input::handle_key(&mut app, ratatui::crossterm::event::KeyEvent::new(ratatui::crossterm::event::KeyCode::Esc, ratatui::crossterm::event::KeyModifiers::NONE));
    assert_eq!(app.ui.focus, Focus::List);
    let today = thc_core::dates::today();
    let notes = app.vault.store.children(&health).unwrap();
    assert!(notes.iter().any(|n| n.text == "Note Dr. Patel, 555-0100"), "{notes:?}");
    // The journal didn't get it.
    let journal = app.vault.store.journal_node(&today.format("%Y-%m-%d").to_string()).unwrap();
    if let Some(j) = journal {
        assert!(!app.vault.store.children(&j).unwrap().iter().any(|n| n.text.contains("Note")));
    }
}
