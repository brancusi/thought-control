//! How long rendering a long document takes, query by query (run with --ignored --nocapture).
use std::time::Instant;
use thc_core::builder::TxBuilder;
use thc_core::event::Actor;
use thc_core::outline::{self, BlockOp, Kind};
use thc_core::vault::{self, Paths, Vault};

#[test]
#[ignore]
fn render_a_5000_block_page() {
    let root = std::env::temp_dir().join(format!("thc-operf-{}", thc_core::id::new_id()));
    vault::init(&root.join("v"), None, None).unwrap();
    let mut v = Vault::open(Paths { vault: root.join("v"), cache: root.join("cache") }, Actor { kind: "human".into(), name: None }, "test").unwrap();
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
    let (_, page) = v.transact(|s| { let mut b = TxBuilder::new(s, today); let p = b.create_page("Long", &[])?; Ok((b.finish(), p)) }).unwrap();
    let mut after: Option<String> = None;
    for chunk in 0..10 {
        let ops: Vec<BlockOp> = (0..500).map(|i| {
            let id = thc_core::id::new_id();
            let op = BlockOp::Create { id: id.clone(), parent: None, after: after.clone(), kind: if i % 3 == 0 { Kind::Task } else { Kind::Bullet }, text: format!("line {chunk}-{i} with some words in it") };
            after = Some(id);
            op
        }).collect();
        let p = page.clone();
        v.transact(move |s| outline::plan(s, &p, &ops, today)).unwrap();
    }
    for _ in 0..3 {
        let t = Instant::now();
        let b = outline::render(&v.store, &page).unwrap();
        eprintln!("render {} blocks: {:?}", b.len(), t.elapsed());
    }
}

/// Render a page from an existing vault, query by query:
/// THC_PERF_VAULT=<vault> THC_PERF_CACHE=<cache> THC_PERF_PAGE=<title>.
#[test]
#[ignore]
fn render_a_page_from_a_vault() {
    let (Ok(vault), Ok(cache), Ok(title)) = (std::env::var("THC_PERF_VAULT"), std::env::var("THC_PERF_CACHE"), std::env::var("THC_PERF_PAGE")) else { return };
    let v = Vault::open(Paths { vault: vault.into(), cache: cache.into() }, Actor { kind: "human".into(), name: None }, "test").unwrap();
    let page = v.store.find_root_by_title(&title, false).unwrap().unwrap();
    let t = Instant::now();
    let b = outline::render(&v.store, &page).unwrap();
    eprintln!("render {} blocks: {:?}", b.len(), t.elapsed());
    let sub = "WITH RECURSIVE sub(id) AS (SELECT id FROM nodes WHERE parent=?1 AND deleted=0 UNION ALL SELECT n2.id FROM sub CROSS JOIN nodes n2 INDEXED BY by_parent ON n2.parent=sub.id WHERE n2.deleted=0) SELECT id FROM sub";
    let t = Instant::now();
    let n = v.store.nodes_where(&format!("n.id IN ({sub}) ORDER BY n.ord, n.id"), &[&page]).unwrap();
    eprintln!("  subtree nodes {}: {:?}", n.len(), t.elapsed());
    for (name, q) in [
        ("props", format!("SELECT node, value FROM props WHERE key='style' AND node IN ({sub})")),
        ("tags", format!("SELECT e.src, t.title FROM edges e JOIN nodes t ON t.id=e.dst WHERE e.rel='tag' AND t.title IS NOT NULL AND e.src IN ({sub}) ORDER BY t.title")),
        ("text_rev", format!("SELECT id, text_eid FROM nodes WHERE id IN ({sub})")),
        ("events rev", format!("SELECT entity, eid FROM (SELECT entity, eid, max(okey) FROM events WHERE entity IN ({sub}) GROUP BY entity)")),
        ("conflicts", format!("SELECT node, '' FROM conflicts WHERE field='text' AND resolved=0 AND node IN ({sub})")),
    ] {
        let t = Instant::now();
        let mut st = v.store.conn.prepare(&q).unwrap();
        let c = st.query_map([&page], |_| Ok(())).unwrap().count();
        eprintln!("  {name} {c}: {:?}", t.elapsed());
    }
}
