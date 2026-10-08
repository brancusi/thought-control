//! Budgets per step (docs/testing-flows.md): typing under 4 ms, a page open under 100 ms,
//! the sidebar under 30 ms, the wheel under 4 ms, on 300- and 5,000-line pages. Each is a
//! step's handling and its frame, as the live loop does them, with the checks off. Release
//! builds only, and ignored by default: `cargo test --release -p thc-tui flows::perf -- --ignored
//! --nocapture --test-threads=1` prints p50 / p99 and the slowest steps. An ignore reason that
//! starts with a task id is over budget (that task).
use super::*;

const OFF: Checks = Checks { invariants: false, cold: false, replay: false, restore: false };

/// p50 and p99 of a set of timings (ms).
fn pct(v: &[f64]) -> (f64, f64) {
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p = |q: f64| v[((v.len() as f64 - 1.0) * q).round() as usize];
    (p(0.5), p(0.99))
}

/// Report a kind's timings (p50, p99, the slowest steps) and note it if p99 is over budget.
fn budget(f: &Flow, kind: &str, label: &str, ms: f64, over: &mut Vec<String>) {
    let t: Vec<(f64, &String)> = f.timings.iter().zip(&f.timed_steps).filter(|((k, _), _)| *k == kind).map(|((_, t), d)| (*t, d)).collect();
    assert!(!t.is_empty(), "{label}: no {kind} steps timed");
    let (p50, p99) = pct(&t.iter().map(|x| x.0).collect::<Vec<_>>());
    eprintln!("perf {label:<44} n={:<4} p50 {p50:>7.2} ms  p99 {p99:>7.2} ms  (budget {ms} ms)", t.len());
    if p99 > ms {
        let mut slow = t.clone();
        slow.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        for (ms, d) in slow.iter().take(3) {
            eprintln!("       slow: {ms:.2} ms  {d}");
        }
        over.push(format!("{label}: p99 {p99:.2} ms over {ms} ms (p50 {p50:.2})"));
    }
}

/// Fail with every budget a test went over.
fn held(over: Vec<String>) {
    assert!(over.is_empty(), "over budget:\n{}", over.join("\n"));
}

fn page(size: Size) -> (Flow, &'static str) {
    let title = if size == Size::Huge { "Huge Page" } else { "Long Page" };
    let mut f = Flow::new(&format!("perf {title}"), size, (140, 40), OFF);
    f.keys(&format!("<c-o>{title}<cr>"));
    (f, title)
}

fn typing(size: Size, label: &str) {
    let mut over = Vec::new();
    let (mut f, _) = page(size);
    f.keys("<c-home>").click_caret(doc_at("Line 12:", 8)).type_text(&super::typing::WORDS.repeat(3));
    f.keys("<cr>").type_text(&super::typing::WORDS.repeat(2));
    budget(&f, "type", label, 4.0, &mut over);
    f.done();
    held(over);
}

#[test]
#[ignore = "perf: cargo test --release -p thc-tui flows::perf -- --ignored --nocapture"]
fn perf_typing_300_lines() {
    typing(Size::Long, "typing, 300-line page");
}

#[test]
#[ignore = "perf: cargo test --release -p thc-tui flows::perf -- --ignored --nocapture"]
fn perf_typing_5000_lines() {
    typing(Size::Huge, "typing, 5,000-line page");
}

#[test]
#[ignore = "perf: cargo test --release -p thc-tui flows::perf -- --ignored --nocapture"]
fn perf_typing_5000_lines_deep() {
    let mut over = Vec::new();
    // Far down the page: the layout above the caret mustn't be redone per key.
    let (mut f, _) = page(Size::Huge);
    f.keys("<c-end>").type_text(&super::typing::WORDS.repeat(3));
    budget(&f, "type", "typing at the end of a 5,000-line page", 4.0, &mut over);
    f.done();
    held(over);
}

#[test]
#[ignore = "perf: cargo test --release -p thc-tui flows::perf -- --ignored --nocapture"]
fn perf_typing_beside_a_panel() {
    let mut over = Vec::new();
    for size in [Size::Long, Size::Huge] {
        let (mut f, _) = page(size);
        f.shift_click(text("Garden").in_doc()).expect_panels(1);
        f.keys("<c-home>").click_caret(doc_at("Line 12:", 8)).type_text(&super::typing::WORDS.repeat(3));
        budget(&f, "type", &format!("typing beside a panel, {size:?} page"), 4.0, &mut over);
        f.done();
    }
    held(over);
}

#[test]
#[ignore = "perf: cargo test --release -p thc-tui flows::perf -- --ignored --nocapture"]
fn perf_page_open() {
    let mut over = Vec::new();
    for size in [Size::Long, Size::Huge] {
        let (mut f, title) = page(size);
        for _ in 0..12 {
            f.keys("<c-o>Garden<cr>").keys("<c-o>").type_text(title);
            f.timed("page_open", |f| {
                f.keys("<cr>");
            });
        }
        budget(&f, "page_open", &format!("page open, {size:?} page"), 100.0, &mut over);
        f.done();
    }
    held(over);
}

#[test]
#[ignore = "perf: cargo test --release -p thc-tui flows::perf -- --ignored --nocapture"]
fn perf_sidebar_open() {
    let mut over = Vec::new();
    for size in [Size::Long, Size::Huge] {
        let (mut f, title) = page(size);
        // A small page beside the big one, and the big one beside a small one.
        for _ in 0..12 {
            f.timed("sidebar_open", |f| {
                f.shift_click(text("Garden").in_doc());
            });
            f.keys("<m-s><m-w>");
        }
        budget(&f, "sidebar_open", &format!("sidebar open beside a {size:?} page"), 30.0, &mut over);
        let id = f.s.app.vault.store.nodes_where("title = ?1", &[&title]).unwrap()[0].id.clone();
        f.keys("<c-o>Garden<cr>");
        f.timings.clear();
        f.timed_steps.clear();
        for _ in 0..12 {
            f.timed("sidebar_open", |f| {
                f.msg("aside", Motion::Any, Msg::Aside { target: id.clone(), pin: false, fold: false, close: false, actor: None });
            });
            f.keys("<m-s><m-w>");
        }
        budget(&f, "sidebar_open", &format!("the {size:?} page opened beside"), 30.0, &mut over);
        f.done();
    }
    held(over);
}

#[test]
#[ignore = "perf: cargo test --release -p thc-tui flows::perf -- --ignored --nocapture"]
fn perf_wheel_and_page_keys() {
    let mut over = Vec::new();
    for size in [Size::Long, Size::Huge] {
        let (mut f, _) = page(size);
        f.keys("<c-home>").wheel(true, 120, None).wheel(false, 60, None);
        budget(&f, "scroll", &format!("the wheel, {size:?} page"), 4.0, &mut over);
        f.timings.clear();
        f.timed_steps.clear();
        f.timed("scroll", |f| {
            for _ in 0..30 {
                f.keys("<pgdn>");
            }
            for _ in 0..30 {
                f.keys("<down>");
            }
        });
        budget(&f, "scroll", &format!("PgDn and ↓, {size:?} page"), 4.0, &mut over);
        f.done();
    }
    held(over);
}

/// Where a keystroke's time goes on a 5,000-line page, at its top and its end: the frame's
/// preparation alone, the key's handling, caretline's insert (with thc's line sync), the whole
/// `apply` (handling + layout following the caret) and the draw.
#[test]
#[ignore = "diagnostic: cargo test --release -p thc-tui flows::perf::diag_split -- --ignored --nocapture"]
fn diag_split() {
    for at in ["<c-home>", "<c-end>"] {
        let (mut f, _) = page(Size::Huge);
        f.keys(at);
        let mut prep = Vec::new();
        for _ in 0..50 {
            let t = Instant::now();
            crate::ui::follow_frame(&mut f.s.app, ratatui::layout::Rect::new(0, 0, 140, 40));
            prep.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let (mut hk, mut ins) = (Vec::new(), Vec::new());
        for c in "abcdefghij".repeat(5).chars() {
            let t = Instant::now();
            crate::input::handle_key(&mut f.s.app, crate::script::key_event(&c.to_string()).unwrap());
            hk.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            f.s.app.doc.as_mut().unwrap().insert("x");
            ins.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let (mut a, mut d) = (Vec::new(), Vec::new());
        for c in "the quick brown fox jumps over the lazy dog ".repeat(4).chars() {
            let t0 = Instant::now();
            f.s.apply(Msg::Key { key: c.to_string() }).unwrap();
            let t1 = Instant::now();
            let _ = draw(&mut f.term, &mut f.s);
            a.push((t1 - t0).as_secs_f64() * 1e3);
            d.push(t1.elapsed().as_secs_f64() * 1e3);
            f.s.runtime(Msg::Frame);
        }
        eprintln!(
            "{at}: prepare {:.2} · handle_key {:.2} · Doc::insert {:.2} · apply {:.2} (p99 {:.2}) · draw {:.2} (p99 {:.2})  [p50 ms]",
            pct(&prep).0,
            pct(&hk).0,
            pct(&ins).0,
            pct(&a).0,
            pct(&a).1,
            pct(&d).0,
            pct(&d).1
        );
        f.done();
    }
}

#[test]
#[ignore = "perf: cargo test --release -p thc-tui flows::perf -- --ignored --nocapture"]
fn perf_typing_500_keys_steady() {
    // A long run of typing, saves and undo steps and all, on a 5,000-line page and beside a
    // panel on it: no key stutters (a periodic spike shows at p99).
    let mut over = Vec::new();
    let words: String = super::typing::WORDS.chars().cycle().take(500).collect();
    let (mut f, _) = page(Size::Huge);
    f.keys("<c-home>").click_caret(doc_at("Line 12:", 8)).type_text(&words);
    budget(&f, "type", "500 keys, 5,000-line page", 4.0, &mut over);
    f.done();
    let (mut f, _) = page(Size::Huge);
    f.keys("<c-home>").shift_click(text("Garden").in_doc()).expect_panels(1);
    f.click_caret(doc_at("Line 12:", 8)).type_text(&words);
    budget(&f, "type", "500 keys beside a panel, 5,000-line page", 4.0, &mut over);
    f.done();
    // The page beside itself: one document, two views at two widths.
    let (mut f, title) = page(Size::Huge);
    let id = f.s.app.vault.store.nodes_where("title = ?1", &[&title]).unwrap()[0].id.clone();
    f.msg("aside", Motion::Any, Msg::Aside { target: id, pin: false, fold: false, close: false, actor: None }).expect_panels(1);
    f.keys("<c-home>").click_caret(doc_at("Line 12:", 8)).type_text(&words);
    budget(&f, "type", "500 keys, 5,000-line page beside itself", 4.0, &mut over);
    f.done();
    held(over);
}
