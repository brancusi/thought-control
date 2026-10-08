//! Documents as blocks (mac-editor-arch.md §3–§5): render, block ops in one transaction,
//! client-made ids, per-block stale, and `$EDITOR` landing on the same writes.

use thc_core::builder::TxBuilder;
use thc_core::edit;
use thc_core::event::{Actor, Op};
use thc_core::outline::{self, BlockOp, Kind};
use thc_core::vault::{self, Paths, Vault};

fn today() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 10, 3).unwrap()
}

fn setup() -> (Vault, String) {
    let root = thc_core::scratch::dir(&format!("thc-outline-{}", thc_core::id::new_id()));
    vault::init(&root.join("v"), None, None).unwrap();
    let paths = Paths { vault: root.join("v"), cache: root.join("cache") };
    let mut v = Vault::open(paths, Actor { kind: "human".into(), name: None }, "test").unwrap();
    let (_, page) = v
        .transact(|s| {
            let mut b = TxBuilder::new(s, today());
            let p = b.create_page("Notes", &[])?;
            Ok((b.finish(), p))
        })
        .unwrap();
    (v, page)
}

/// Plan and write one save; return the results.
fn save(v: &mut Vault, page: &str, ops: Vec<BlockOp>) -> Vec<outline::OpResult> {
    let p = page.to_string();
    let (_, results) = v
        .transact(move |s| {
            let (built, results) = outline::plan(s, &p, &ops, today())?;
            Ok((built, results))
        })
        .unwrap();
    results
}

fn create(id: &str, after: Option<&str>, kind: Kind, text: &str) -> BlockOp {
    BlockOp::Create { id: id.into(), parent: None, after: after.map(Into::into), kind, text: text.into() }
}

#[test]
fn a_save_of_new_blocks_is_one_transaction_in_order() {
    let (mut v, page) = setup();
    let (a, b, c) = (thc_core::id::new_id(), thc_core::id::new_id(), thc_core::id::new_id());
    // A typed paragraph, then Enter twice: each new block after the previous, in one save.
    let r = save(
        &mut v,
        &page,
        vec![
            create(&a, None, Kind::Para, "The quarter is about fewer bets."),
            create(&b, Some(&a), Kind::Task, "Draft the memo due:2026-10-09 !high"),
            create(&c, Some(&a), Kind::Bullet, "between them"),
        ],
    );
    assert!(r.iter().all(|x| x.state == "ok"), "{r:?}");
    let blocks = outline::render(&v.store, &page).unwrap();
    let ids: Vec<&str> = blocks.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(ids, [a.as_str(), c.as_str(), b.as_str()], "client ids kept, order by `after`");
    assert_eq!(blocks[0].kind, Kind::Para);
    assert_eq!(blocks[2].kind, Kind::Task);
    assert_eq!(blocks[2].text, "Draft the memo", "tokens became fields");
    assert_eq!(blocks[2].due.as_deref(), Some("2026-10-09"));
    assert_eq!(blocks[2].priority.as_deref(), Some("high"));
    let txs: std::collections::HashSet<String> = [&a, &b, &c].iter().flat_map(|id| v.store.history(id, 100).unwrap()).filter(|e| e.entity != page).map(|e| e.tx).collect();
    assert_eq!(txs.len(), 1, "one transaction");
}

#[test]
fn edits_keep_fields_they_dont_mention_and_nest() {
    let (mut v, page) = setup();
    let (a, b) = (thc_core::id::new_id(), thc_core::id::new_id());
    save(&mut v, &page, vec![create(&a, None, Kind::Task, "Book the venue due:fri"), create(&b, Some(&a), Kind::Bullet, "ask Sam")]);
    // The editor sends the clean text (the due date lives in the gutter): it must stay.
    save(&mut v, &page, vec![BlockOp::Edit { id: a.clone(), text: "Book the venue for the offsite !high".into(), base: None, rev: None, raw: false }]);
    // Tab: b nests under a.
    save(&mut v, &page, vec![BlockOp::Move { id: b.clone(), parent: Some(a.clone()), after: None, rev: None }]);
    let blocks = outline::render(&v.store, &page).unwrap();
    assert_eq!(blocks[0].text, "Book the venue for the offsite");
    assert!(blocks[0].due.is_some(), "the due date stays");
    assert_eq!(blocks[0].priority.as_deref(), Some("high"));
    assert_eq!((blocks[1].id.as_str(), blocks[1].depth), (b.as_str(), 1));
    // Kind and status: the cell and `- `.
    save(&mut v, &page, vec![BlockOp::Kind { id: b.clone(), kind: Kind::Task, rev: None }, BlockOp::Status { id: a.clone(), status: "doing".into(), rev: None }]);
    let blocks = outline::render(&v.store, &page).unwrap();
    assert_eq!(blocks[1].kind, Kind::Task);
    assert_eq!(blocks[0].status.as_deref(), Some("doing"));
    save(&mut v, &page, vec![BlockOp::Kind { id: b.clone(), kind: Kind::Para, rev: None }]);
    assert_eq!(outline::render(&v.store, &page).unwrap()[1].kind, Kind::Para, "task → paragraph clears the status");
}

#[test]
fn a_stale_block_is_left_out_and_its_neighbours_commit() {
    let (mut v, page) = setup();
    let (a, b, c) = (thc_core::id::new_id(), thc_core::id::new_id(), thc_core::id::new_id());
    save(&mut v, &page, vec![create(&a, None, Kind::Bullet, "one"), create(&b, Some(&a), Kind::Bullet, "two"), create(&c, Some(&b), Kind::Bullet, "three")]);
    let revs: Vec<Option<String>> = [&a, &b, &c].iter().map(|id| v.store.rev(id)).collect();
    // Another device (or an agent) changes b after the editor read it.
    save(&mut v, &page, vec![BlockOp::Edit { id: b.clone(), text: "two, edited elsewhere".into(), base: None, rev: None, raw: false }]);
    // The editor saves all three lines with the revisions it read.
    let r = save(
        &mut v,
        &page,
        vec![
            BlockOp::Edit { id: a.clone(), text: "one!".into(), base: None, rev: revs[0].clone(), raw: false },
            BlockOp::Edit { id: b.clone(), text: "two, mine".into(), base: None, rev: revs[1].clone(), raw: false },
            BlockOp::Edit { id: c.clone(), text: "three!".into(), base: None, rev: revs[2].clone(), raw: false },
        ],
    );
    assert_eq!(r.iter().map(|x| x.state).collect::<Vec<_>>(), ["ok", "stale", "ok"]);
    assert_eq!(r[1].block.as_ref().map(|x| x.text.as_str()), Some("two, edited elsewhere"), "stale carries the server's state");
    let texts: Vec<String> = outline::render(&v.store, &page).unwrap().into_iter().map(|x| x.text).collect();
    assert_eq!(texts, ["one!", "two, edited elsewhere", "three!"], "neither side overwritten; neighbours saved");
    // The tx holds only the passing ops: b has no event from it.
    let last_tx = v.store.history(&a, 1).unwrap()[0].tx.clone(); // newest first
    assert!(v.store.history(&b, 100).unwrap().iter().all(|e| e.tx != last_tx));
}

#[test]
fn a_client_id_that_exists_is_never_a_second_node() {
    let (mut v, page) = setup();
    let a = thc_core::id::new_id();
    save(&mut v, &page, vec![create(&a, None, Kind::Bullet, "first")]);
    let r = save(&mut v, &page, vec![create(&a, None, Kind::Bullet, "a retry or a clash")]);
    assert_eq!(r[0].state, "exists");
    assert_eq!(outline::render(&v.store, &page).unwrap().len(), 1);
    let bad = save(&mut v, &page, vec![create("not-an-id", None, Kind::Bullet, "x")]);
    assert_eq!(bad[0].state, "error");
}

#[test]
fn editor_and_dollar_editor_write_the_same_events_for_the_same_edit() {
    // The same logical edit (one line's text) through the Mac editor's block op and through the
    // `$EDITOR` round trip produces the same event ops.
    let (mut v, page) = setup();
    let a = thc_core::id::new_id();
    save(&mut v, &page, vec![create(&a, None, Kind::Bullet, "Call the venue")]);
    let block_ops = outline::plan(&v.store, &page, &[BlockOp::Edit { id: a.clone(), text: "Call the venue back".into(), base: None, rev: None, raw: false }], today()).unwrap().0;
    let rendered = edit::render(&v.store, &page).unwrap();
    let edited = rendered.text.replace("Call the venue", "Call the venue back");
    let mut b = TxBuilder::new(&v.store, today());
    edit::apply(&mut b, &rendered, &edited).unwrap();
    let editor_ops = b.finish();
    let strip = |ops: &[Op]| ops.iter().map(|o| serde_json::to_value(o).unwrap()).collect::<Vec<_>>();
    assert_eq!(strip(&block_ops), strip(&editor_ops), "one write path for both editors");
}

#[test]
fn render_ids_patches_with_depth_and_reports_gone() {
    let (mut v, page) = setup();
    let (a, b) = (thc_core::id::new_id(), thc_core::id::new_id());
    save(&mut v, &page, vec![create(&a, None, Kind::Bullet, "parent"), BlockOp::Create { id: b.clone(), parent: Some(a.clone()), after: None, kind: Kind::Bullet, text: "child".into() }]);
    save(&mut v, &page, vec![BlockOp::Delete { id: a.clone(), rev: None }]);
    let (blocks, gone) = outline::render_ids(&v.store, &page, &[a.clone(), b.clone()]).unwrap();
    assert!(blocks.is_empty(), "deleting a block deletes its children");
    assert_eq!(gone.len(), 2);
}

#[test]
fn a_line_changed_elsewhere_becomes_a_conflict_and_mine_shows() {
    // editor.md §8.1: the editor's save is written at once; both versions are kept as an ordinary
    // text conflict, the editor's text wins locally, and its neighbours save normally.
    let (mut v, page) = setup();
    let (a, b) = (thc_core::id::new_id(), thc_core::id::new_id());
    save(&mut v, &page, vec![create(&a, None, Kind::Bullet, "Book the venue"), create(&b, Some(&a), Kind::Bullet, "neighbour")]);
    let read = outline::render(&v.store, &page).unwrap();
    let base = read[0].text_rev.clone();
    assert!(base.is_some() && !read[0].conflict);
    // Claude changes the line after the editor read it.
    save(&mut v, &page, vec![BlockOp::Edit { id: a.clone(), text: "Book the venue for 40".into(), base: None, rev: None, raw: false }]);
    // The editor leaves the line: its save carries the base it started from.
    let r = save(
        &mut v,
        &page,
        vec![
            BlockOp::Edit { id: a.clone(), text: "Book the venue in Lisbon".into(), base: base.clone(), rev: None, raw: false },
            BlockOp::Edit { id: b.clone(), text: "neighbour, edited".into(), base: read[1].text_rev.clone(), rev: None, raw: false },
        ],
    );
    assert!(r.iter().all(|x| x.state == "ok"), "{r:?}");
    let now = outline::render(&v.store, &page).unwrap();
    assert_eq!(now[0].text, "Book the venue in Lisbon", "mine shows");
    assert!(now[0].conflict, "and the line is ≠: both versions kept");
    let loser: String = v.store.conn.query_row("SELECT loser_value FROM conflicts WHERE node=?1 AND resolved=0", [&a], |r| r.get(0)).unwrap();
    assert_eq!(loser, "Book the venue for 40", "claude's version is kept");
    assert!(!now[1].conflict && now[1].text == "neighbour, edited");
}

#[test]
fn a_line_deleted_elsewhere_while_edited_is_kept() {
    let (mut v, page) = setup();
    let a = thc_core::id::new_id();
    save(&mut v, &page, vec![create(&a, None, Kind::Bullet, "Book the venue")]);
    let base = outline::render(&v.store, &page).unwrap()[0].text_rev.clone();
    save(&mut v, &page, vec![BlockOp::Delete { id: a.clone(), rev: None }]);
    let r = save(&mut v, &page, vec![BlockOp::Edit { id: a.clone(), text: "Book the venue, keep this".into(), base, rev: None, raw: false }]);
    assert_eq!(r[0].state, "ok", "{r:?}");
    let now = outline::render(&v.store, &page).unwrap();
    assert_eq!((now.len(), now[0].id.as_str(), now[0].text.as_str()), (1, a.as_str(), "Book the venue, keep this"), "restored: same id, my text");
}

#[test]
fn an_idle_save_stores_text_without_reading_tokens() {
    let (mut v, page) = setup();
    let a = thc_core::id::new_id();
    save(&mut v, &page, vec![create(&a, None, Kind::Task, "Book the venue")]);
    save(&mut v, &page, vec![BlockOp::Edit { id: a.clone(), text: "Book the venue due:fr".into(), base: None, rev: None, raw: true }]);
    let b = &outline::render(&v.store, &page).unwrap()[0];
    assert_eq!((b.text.as_str(), b.due.as_deref()), ("Book the venue due:fr", None), "half-typed token kept as text");
}

#[test]
fn markdown_copy_matches_edit() {
    let (mut v, page) = setup();
    let rendered = edit::render(&v.store, &page).unwrap();
    let edited = format!("{}\nA paragraph.\n\n- Plan\n  - [ ] Book venue due:2026-10-09 !high\n  - notes\n- [x] Done\n", rendered.text);
    v.transact(|s| {
        let mut b = TxBuilder::new(s, today());
        edit::apply(&mut b, &rendered, &edited)?;
        Ok((b.finish(), ()))
    })
    .unwrap();
    let md = outline::markdown(&outline::render(&v.store, &page).unwrap());
    assert!(md.contains("A paragraph.\n\n- Plan\n  - [ ] Book venue due:2026-10-09 !high\n  - notes\n- [x] Done\n"), "{md}");
    // `thc edit` writes the same lines, each ending in its id.
    let again = edit::render(&v.store, &page).unwrap().text;
    let stripped: String = again.lines().map(|l| l.rsplit_once(" ^").map(|(a, _)| a).unwrap_or(l)).collect::<Vec<_>>().join("\n");
    assert!(stripped.contains("- Plan\n  - [ ] Book venue due:2026-10-09 !high\n  - notes\n- [x] Done"), "{again}");
}

#[test]
fn no_after_means_first_among_siblings() {
    let (mut v, page) = setup();
    let (a, b, c) = (thc_core::id::new_id(), thc_core::id::new_id(), thc_core::id::new_id());
    save(&mut v, &page, vec![create(&a, None, Kind::Bullet, "A"), create(&b, Some(&a), Kind::Bullet, "B")]);
    // A line typed above everything is created first, not appended.
    save(&mut v, &page, vec![create(&c, None, Kind::Bullet, "C")]);
    let order = |v: &Vault| outline::render(&v.store, &page).unwrap().into_iter().map(|b| b.text).collect::<Vec<_>>();
    assert_eq!(order(&v), ["C", "A", "B"]);
    // ⌥⌘↑ on B among C, A, B: B moves after C, A after B.
    save(&mut v, &page, vec![
        BlockOp::Move { id: b.clone(), parent: None, after: Some(c.clone()), rev: None },
        BlockOp::Move { id: a.clone(), parent: None, after: Some(b.clone()), rev: None },
    ]);
    assert_eq!(order(&v), ["C", "B", "A"]);
    save(&mut v, &page, vec![BlockOp::Move { id: a.clone(), parent: None, after: None, rev: None }]);
    assert_eq!(order(&v), ["A", "C", "B"]);
}

#[test]
fn a_paragraphs_children_save_under_it_and_export_indented() {
    let (mut v, page) = setup();
    let (p, c, t) = (thc_core::id::new_id(), thc_core::id::new_id(), thc_core::id::new_id());
    // Tab under a paragraph (any note nests under the one above): its children are its nodes.
    let child = |id: &str, after: Option<&str>, kind: Kind, text: &str| BlockOp::Create { id: id.into(), parent: Some(p.clone()), after: after.map(Into::into), kind, text: text.into() };
    let r = save(&mut v, &page, vec![create(&p, None, Kind::Para, "Para line"), child(&c, None, Kind::Para, "first subtask\nmore of it"), child(&t, Some(&c), Kind::Task, "a task under it")]);
    assert!(r.iter().all(|r| r.state == "ok"), "{r:?}");
    let blocks = outline::render(&v.store, &page).unwrap();
    let depth = |id: &str| blocks.iter().find(|b| b.id == id).unwrap().depth;
    assert_eq!((depth(&p), depth(&c), depth(&t)), (0, 1, 1));
    let md = outline::markdown(&blocks);
    assert_eq!(md, "Para line\n\n  first subtask\n  more of it\n\n  - [ ] a task under it\n", "{md}");
}
