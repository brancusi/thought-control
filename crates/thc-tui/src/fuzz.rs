//! The editor under random use (hardening for milestone 1): typing, Enter, blank lines, ⌫ joins,
//! pastes, ⌃T, Tab / ⇧Tab, moving lines, undo / redo, remote edits, clicks, leaving and coming
//! back, against a scratch vault. After every few steps the saved notes must be exactly the
//! buffer: same notes in the same order, same kind, depth, status and text, no note lost or
//! doubled, untouched notes keeping their IDs, and nothing panics. A failure is shrunk to the
//! fewest ops that still fail and printed as a replayable list.
//!
//! CI runs a fixed seed. A longer soak: `THC_FUZZ_CASES=2000 THC_FUZZ_OPS=200 cargo test -p
//! thc-tui --release fuzz -- --nocapture` (`THC_FUZZ_SEED=n` starts elsewhere; `THC_FUZZ_EVERY=1`
//! checks after every op).

use crate::app::{App, View};
use crate::doc::Target;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::path::PathBuf;
use thc_core::event::Actor;
use thc_core::outline::{self, Kind};
use thc_core::vault::{Paths, Vault};

/// xorshift64*: small, seeded, the same on every machine.
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
    fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[self.below(v.len())]
    }
}

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// One thing a person (or another device) does.
#[derive(Clone, Copy, Debug)]
enum Op {
    Type(&'static str),
    Enter,
    Bs,
    Move(KeyCode),
    Select(KeyCode),
    TaskCycle,
    Tab,
    BackTab,
    MoveLine(bool),
    Undo,
    Redo,
    Paste(&'static str),
    Marker(&'static str),
    /// Another device rewrites the n-th saved note (mod how many there are).
    Remote(usize),
    Click(u16, u16),
    /// Esc (save, go to Today), then back to the day.
    LeaveReturn,
    Cut,
}

const WORDS: &[&str] = &["alpha", "bé", "漢字", "x", "two words", "🙂", "dash-y", "end."];
const MARKERS: &[&str] = &["- ", "[ ] ", "1. ", "* "];
const PASTES: &[&str] = &["- one\n- [ ] two\n\nthird para", "plain text pasted", "line a\nline b", "- [x] done item"];

fn generate(rng: &mut Rng, n: usize) -> Vec<Op> {
    (0..n)
        .map(|_| match rng.below(25) {
            0..=5 => Op::Type(rng.pick(WORDS)),
            6..=8 => Op::Enter,
            9 | 10 => Op::Bs,
            11 => Op::Move(rng.pick(&[KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right, KeyCode::Home, KeyCode::End, KeyCode::PageUp, KeyCode::PageDown])),
            12 => Op::Select(rng.pick(&[KeyCode::Left, KeyCode::Up, KeyCode::Right])),
            13 => Op::TaskCycle,
            14 => Op::Tab,
            15 => Op::BackTab,
            16 => Op::MoveLine(rng.below(2) == 0),
            17 => Op::Undo,
            18 => Op::Redo,
            19 => Op::Paste(rng.pick(PASTES)),
            20 => Op::Marker(rng.pick(MARKERS)),
            21 => Op::Remote(rng.below(8)),
            22 => Op::Click(rng.below(100) as u16, 4 + rng.below(24) as u16),
            23 => Op::LeaveReturn,
            _ => Op::Cut,
        })
        .collect()
}

pub(crate) struct Scratch {
    root: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub(crate) fn scratch(tag: &str) -> (Scratch, Vault) {
    let root = std::env::temp_dir().join(format!("thc-fuzz-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    thc_core::vault::init(&root.join("vault"), None, None).unwrap();
    let v = Vault::open(Paths { vault: root.join("vault"), cache: root.join("cache") }, Actor { kind: "human".into(), name: None }, "tui").unwrap();
    (Scratch { root }, v)
}

fn key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    crate::input::handle_key(app, KeyEvent::new(code, mods));
}

/// One note as compared: (kind, depth, status, text). Saving trims a note's trailing spaces
/// and leading ones (`1. ` typed is `1.` saved) and blank lines around it (the caret's line keeps them until you
/// leave it), so text compares without those.
type Note = (Kind, usize, Option<String>, String);

fn norm(t: &str) -> String {
    t.split('\n').map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_string()
}

/// The buffer's notes (lines with text; a line still empty is not a note), by ID.
fn buffer_notes(app: &App) -> Vec<(String, Note)> {
    let Some(d) = app.doc.as_ref() else { return vec![] };
    d.lines().iter().filter(|l| !l.text.trim().is_empty()).map(|l| (l.id.clone(), (l.kind, l.depth, if l.kind == Kind::Task { l.status.clone() } else { None }, norm(&l.text)))).collect()
}

/// Notes whose text is meant to differ from the vault for now (§9): a remote edit held while
/// you're on the line, or a ≠ conflict.
fn pending(app: &App) -> Vec<String> {
    app.doc.as_ref().map(|d| d.lines().iter().filter(|l| l.remote_text.is_some() || l.remote_shape || l.conflict).map(|l| l.id.clone()).collect()).unwrap_or_default()
}

/// What the vault holds for the open document, by ID, and every ID in order.
fn saved(app: &App) -> (Vec<(String, Note)>, Vec<String>) {
    let Some(d) = app.doc.as_ref() else { return (vec![], vec![]) };
    let root = match &d.target {
        Target::Journal { date } => app.vault.store.journal_node(&date.format("%Y-%m-%d").to_string()).ok().flatten(),
        Target::Page { id, .. } => Some(id.clone()),
    };
    let Some(root) = root.or_else(|| d.root.clone()) else { return (vec![], vec![]) };
    let blocks = outline::render(&app.vault.store, &root).unwrap();
    let notes = blocks.iter().filter(|b| !b.text.trim().is_empty()).map(|b| (b.id.clone(), (b.kind, b.depth, if b.kind == Kind::Task { b.status.clone() } else { None }, norm(&b.text)))).collect();
    let ids = blocks.iter().map(|b| b.id.clone()).collect();
    (notes, ids)
}

thread_local! {
    /// A writer that answers late (`Saver::manual`), when this thread's run uses one: the
    /// daemon's writer thread, slow or busy, under the test's control.
    static WRITER: std::cell::RefCell<Option<crate::doc_app::ManualWriter>> = const { std::cell::RefCell::new(None) };
}

/// Hand back every result the late writer holds; again while a save waited for them.
pub(crate) fn settle(app: &mut App) {
    for _ in 0..20 {
        let dones = WRITER.with(|w| w.borrow().as_ref().map(|w| w.run(&mut app.vault, app.today)).unwrap_or_default());
        let had = !dones.is_empty();
        WRITER.with(|w| {
            if let Some(w) = w.borrow().as_ref() {
                for d in dones {
                    w.deliver(d);
                }
            }
        });
        app.drain_saves(false);
        if !had && !app.doc_saver.as_ref().is_some_and(|s| s.busy()) {
            return;
        }
    }
    panic!("the saves never settled");
}

/// Save everything and wait for it (through the late writer when there is one).
fn flush(app: &mut App) {
    app.save_doc(true);
    app.drain_saves(true);
}

/// The late writer hands back the one result it holds, if any.
fn settle_one(app: &mut App) {
    let dones = WRITER.with(|w| w.borrow().as_ref().map(|w| w.run(&mut app.vault, app.today)).unwrap_or_default());
    WRITER.with(|w| {
        if let Some(w) = w.borrow().as_ref() {
            for d in dones {
                w.deliver(d);
            }
        }
    });
    app.drain_saves(false);
}

/// Save everything, then: the vault holds exactly the buffer, every ID once.
fn check(app: &mut App) -> Result<(), String> {
    check_with(app, true)
}

/// `check`, with sibling order optional (the two-device soak: see
/// sync_sibling_order_after_concurrent_moves).
fn check_with(app: &mut App, strict_order: bool) -> Result<(), String> {
    flush(app);
    let skip = pending(app);
    let buf: Vec<(String, Note)> = buffer_notes(app).into_iter().filter(|(id, _)| !skip.contains(id)).collect();
    let (held, ids) = saved(app);
    let held: Vec<(String, Note)> = held.into_iter().filter(|(id, _)| !skip.contains(id)).collect();
    let mut uniq = ids.clone();
    uniq.sort();
    uniq.dedup();
    if uniq.len() != ids.len() {
        return Err(format!("a note is in the document twice: {ids:?}"));
    }
    let (mut b, mut h) = (buf.clone(), held.clone());
    if !strict_order {
        b.sort_by(|x, y| x.0.cmp(&y.0));
        h.sort_by(|x, y| x.0.cmp(&y.0));
    }
    if b != h {
        return Err(format!("the saved notes aren't the buffer\nbuffer: {buf:#?}\nsaved: {held:#?}"));
    }
    Ok(())
}

fn apply(app: &mut App, op: Op) {
    match op {
        Op::Type(w) => {
            for c in w.chars() {
                key(app, KeyCode::Char(c), KeyModifiers::NONE);
            }
        }
        Op::Enter => key(app, KeyCode::Enter, KeyModifiers::NONE),
        Op::Bs => key(app, KeyCode::Backspace, KeyModifiers::NONE),
        Op::Move(c) => key(app, c, KeyModifiers::NONE),
        Op::Select(c) => key(app, c, KeyModifiers::SHIFT),
        Op::TaskCycle => key(app, KeyCode::Char('t'), KeyModifiers::CONTROL),
        Op::Tab => key(app, KeyCode::Tab, KeyModifiers::NONE),
        Op::BackTab => key(app, KeyCode::BackTab, KeyModifiers::SHIFT),
        Op::MoveLine(up) => key(app, if up { KeyCode::Up } else { KeyCode::Down }, KeyModifiers::ALT),
        Op::Undo => key(app, KeyCode::Char('z'), KeyModifiers::CONTROL),
        Op::Redo => key(app, KeyCode::Char('y'), KeyModifiers::CONTROL),
        Op::Paste(p) => {
            if app.doc.is_some() {
                crate::doc_keys::paste(app, p);
            }
        }
        Op::Marker(m) => {
            key(app, KeyCode::Enter, KeyModifiers::NONE);
            for c in m.chars() {
                key(app, KeyCode::Char(c), KeyModifiers::NONE);
            }
        }
        Op::Remote(n) => {
            let ids = saved(app).1;
            if !ids.is_empty() {
                let id = ids[n % ids.len()].clone();
                crate::remote_write(app, &id, "from elsewhere").unwrap();
                let _ = app.poll_external();
            }
        }
        Op::Click(x, y) => {
            for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
                crate::input::handle_mouse(app, MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE }, 1);
            }
        }
        Op::LeaveReturn => {
            key(app, KeyCode::Esc, KeyModifiers::NONE);
            key(app, KeyCode::Esc, KeyModifiers::NONE);
            app.overlay = None;
            app.prompt = None;
            app.journal_date = app.today;
            app.set_view(View::Journal);
        }
        Op::Cut => key(app, KeyCode::Char('x'), KeyModifiers::CONTROL),
    }
}

/// Run ops on a fresh vault; the first broken invariant (or panic) as Err.
fn run(ops: &[Op], every: usize, tag: &str) -> Result<(), String> {
    run_with(ops, every, tag, None)
}

/// `run`, with `late`: saves go to a writer that hands results back only at random moments
/// (seeded), as a busy daemon does, so later edits are made while a save is out.
fn run_with(ops: &[Op], every: usize, tag: &str, late: Option<u64>) -> Result<(), String> {
    let ops = ops.to_vec();
    let tag = tag.to_string();
    let r = std::panic::catch_unwind(move || -> Result<(), String> {
        let (_s, vault) = scratch(&tag);
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(vault).unwrap();
        app.daemon_live = false;
        let mut rng = late.map(Rng);
        WRITER.with(|w| *w.borrow_mut() = None);
        if rng.is_some() {
            let (saver, writer) = crate::doc_app::Saver::manual();
            app.doc_saver = Some(saver);
            WRITER.with(|w| *w.borrow_mut() = Some(writer));
        }
        app.journal_date = app.today;
        app.set_view(View::Journal);
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        for (step, op) in ops.iter().enumerate() {
            // Only the caret moves: the notes' IDs, saved before, must not change.
            let before = if matches!(op, Op::Move(_)) {
                flush(&mut app);
                Some(saved(&app).1)
            } else {
                None
            };
            // (A remote text held for a line lands as the caret leaves it: that's the remote
            // edit, not the motion, so a document holding one isn't compared.)
            let holding = app.doc.as_ref().is_some_and(|d| d.lines().iter().any(|l| l.remote_text.is_some() || l.remote_shape));
            let texts_before: Option<Vec<String>> = (matches!(op, Op::Move(_) | Op::Select(_)) && !holding).then(|| app.doc.as_ref().map(|d| d.lines().iter().map(|l| l.text.clone()).collect()).unwrap_or_default());
            apply(&mut app, *op);
            // The late writer answers now and then: one result at a time, at random.
            if let Some(rng) = rng.as_mut() {
                if rng.below(3) == 0 {
                    settle_one(&mut app);
                }
            }
            // Motion (motion.md §6): never edits (I7), and lands on a stop (I1).
            if let Some(tb) = texts_before {
                let ctx = crate::doc_ui::DocContext::from_app(&app);
                if let Some(d) = app.doc.as_mut() {
                    let ta: Vec<String> = d.lines().iter().map(|l| l.text.clone()).collect();
                    if ta != tb {
                        return Err(format!("step {step}: a motion changed the text: {tb:?} → {ta:?}"));
                    }
                    let (sw, detail) = (app.screen_width, app.show_detail);
                    let caret = d.view.caret;
                    let l = crate::motion::layout(d, &|l: &crate::doc::Line| crate::doc_ui::text_width(ctx, sw, detail, l.depth));
                    if !l.stops.is_empty() && l.valid(caret) != caret {
                        return Err(format!("step {step}: the caret {caret:?} isn't on a stop (valid: {:?})", l.valid(caret)));
                    }
                }
            }
            // A pop-up an op opened (a compare on a ≠ line) is closed; a document stays open.
            app.overlay = None;
            app.prompt = None;
            // A click on a tab or the day strip went elsewhere: back to today's day (what the
            // checks and the reopen compare).
            let on_today = app.view == View::Journal && app.journal_date == app.today && app.doc.as_ref().is_some_and(|d| matches!(d.target, Target::Journal { .. }));
            if !on_today {
                app.journal_date = app.today;
                app.set_view(View::Journal);
            }
            term.draw(|f| crate::ui::draw_app(f, &mut app)).unwrap();
            app.after_frame();
            if let Some(before) = before {
                flush(&mut app);
                let after = saved(&app).1;
                if before != after {
                    return Err(format!("step {step}: moving the caret changed the notes' IDs: {before:?} → {after:?}"));
                }
            }
            if step % every == every - 1 {
                check(&mut app).map_err(|e| format!("step {step}: {e}"))?;
            }
        }
        check(&mut app).map_err(|e| format!("end: {e}"))?;
        // Coming back reads the same document: the buffer round-trips through the vault.
        // (A note holding a remote text, or ≠, reads the vault's version when reopened.)
        let skip = pending(&app);
        let buf: Vec<(String, Note)> = buffer_notes(&app).into_iter().filter(|(id, _)| !skip.contains(id)).collect();
        app.doc = None;
        app.set_view(View::Today);
        app.journal_date = app.today;
        app.set_view(View::Journal);
        let again: Vec<(String, Note)> = buffer_notes(&app).into_iter().filter(|(id, _)| !skip.contains(id)).collect();
        if again != buf {
            return Err(format!("reopened, the document differs\nbefore: {buf:#?}\nafter: {again:#?}"));
        }
        Ok(())
    });
    match r {
        Ok(r) => r,
        Err(p) => Err(format!("panic: {}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())),
    }
}

/// The fewest ops that still fail: drop chunks, halving, while it fails.
fn shrink(ops: Vec<Op>, every: usize) -> Vec<Op> {
    shrink_with(ops, every, None)
}

fn shrink_with(mut ops: Vec<Op>, every: usize, late: Option<u64>) -> Vec<Op> {
    let mut chunk = ops.len() / 2;
    let mut n = 0;
    while chunk >= 1 {
        let mut i = 0;
        let mut progressed = false;
        while i < ops.len() {
            let mut t = ops.clone();
            t.drain(i..(i + chunk).min(t.len()));
            n += 1;
            if run_with(&t, every, &format!("shrink{n}"), late).is_err() {
                ops = t;
                progressed = true;
            } else {
                i += chunk;
            }
        }
        if !progressed {
            chunk /= 2;
        }
    }
    ops
}

#[test]
fn fuzz_the_editor_keeps_the_vault_equal_to_the_buffer() {
    // Quiet panics while shrinking; the report below says what failed.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let cases = env_num("THC_FUZZ_CASES", 40);
    let len = env_num("THC_FUZZ_OPS", 60) as usize;
    let first = env_num("THC_FUZZ_SEED", 1);
    let every = env_num("THC_FUZZ_EVERY", 4).max(1) as usize;
    let mut failure = None;
    // THC_SYNC_ONLY=<case>: that case alone, THC_SYNC_REPEAT times (a flaky one: ids and timing
    // vary run to run), unshrunk, with the first failure in full.
    if let Ok(only) = std::env::var("THC_SYNC_ONLY") {
        let case: u64 = only.parse().unwrap();
        let mut rng = Rng(0xD1CE_5EED ^ case.wrapping_mul(0x9E37_79B9));
        let ops = generate_sync(&mut rng, len);
        let repeat = env_num("THC_SYNC_REPEAT", 20);
        let mut fails = 0;
        for r in 0..repeat {
            if let Err(e) = run_sync(&ops, &format!("only{r}")) {
                if fails == 0 {
                    eprintln!("FAIL run {r}:\n{ops:?}\n{e}");
                }
                fails += 1;
            }
        }
        std::panic::set_hook(hook);
        assert!(fails == 0, "case {case}: {fails} of {repeat} runs failed");
        return;
    }
    for case in first..first + cases {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ case.wrapping_mul(0x1000_0001));
        let ops = generate(&mut rng, len);
        if let Err(e) = run(&ops, every, &format!("case{case}")) {
            let small = shrink(ops, every);
            let why = run(&small, every, "final").err().unwrap_or(e);
            failure = Some(format!("case {case}, shrunk to {} ops:\n{small:?}\n{why}", small.len()));
            break;
        }
    }
    std::panic::set_hook(hook);
    if let Some(f) = failure {
        panic!("{f}");
    }
}

/// Found by the fuzz test (shrunk): replays that once failed.
#[test]
fn fuzz_regressions() {
    for ops in [
        vec![Op::Type("漢字"), Op::Select(KeyCode::Up), Op::TaskCycle, Op::TaskCycle, Op::Enter],
        vec![Op::TaskCycle, Op::Type("two words"), Op::Marker("[ ] "), Op::Type("dash-y")],
        vec![Op::Type("dash-y"), Op::Enter, Op::Type("🙂"), Op::Click(19, 6), Op::Marker("- ")],
        vec![Op::Marker("* "), Op::Type("bé"), Op::Tab],
        vec![Op::Select(KeyCode::Right), Op::Enter, Op::Type("two words"), Op::Select(KeyCode::Up), Op::Type("🙂"), Op::Undo],
        vec![Op::Type("alpha"), Op::Marker("1. "), Op::Type("alpha"), Op::Tab, Op::Enter, Op::MoveLine(true), Op::BackTab, Op::Move(KeyCode::Left), Op::Paste("- [x] done item"), Op::Enter],
        vec![Op::TaskCycle, Op::Type("🙂"), Op::Enter, Op::Type("alpha"), Op::Tab, Op::Move(KeyCode::Home), Op::Type("two words"), Op::Marker("[ ] "), Op::Marker("- "), Op::Tab, Op::Move(KeyCode::Up), Op::Enter],
        vec![Op::Type("two words"), Op::Undo, Op::Bs, Op::Enter, Op::TaskCycle, Op::Type("dash-y"), Op::MoveLine(true), Op::Cut, Op::Remote(5), Op::Undo],
        vec![Op::Type("bé"), Op::Cut, Op::Type("🙂"), Op::Paste("line a\nline b"), Op::LeaveReturn, Op::LeaveReturn, Op::Move(KeyCode::Home), Op::Undo, Op::MoveLine(false), Op::Type("漢字"), Op::Marker("1. "), Op::TaskCycle, Op::Type("end."), Op::Move(KeyCode::Left), Op::Redo, Op::Enter, Op::Remote(3), Op::Move(KeyCode::Up), Op::Marker("[ ] "), Op::Redo, Op::Type("dash-y"), Op::Enter, Op::MoveLine(false), Op::Paste("line a\nline b"), Op::Remote(5)],
        vec![Op::Type("x"), Op::Marker("- "), Op::Type("🙂"), Op::BackTab, Op::BackTab, Op::Bs, Op::BackTab, Op::BackTab, Op::Undo, Op::Type("end.")],
        vec![Op::Paste("- [x] done item"), Op::Marker("1. "), Op::Move(KeyCode::Down), Op::Type("dash-y"), Op::Paste("plain text pasted"), Op::Type("bé"), Op::LeaveReturn, Op::Move(KeyCode::Home), Op::Cut, Op::Redo, Op::Bs, Op::Select(KeyCode::Up), Op::Type("x"), Op::Remote(3), Op::Undo],
        // A parent deleted while its child was the caret's line took the child with it.
        vec![Op::Type("two words"), Op::Click(58, 21), Op::Move(KeyCode::Left), Op::TaskCycle, Op::Click(95, 14), Op::Enter, Op::Move(KeyCode::End), Op::Enter, Op::Type("bé"), Op::MoveLine(false), Op::Cut, Op::Cut, Op::Tab, Op::Type("end."), Op::Move(KeyCode::Down), Op::Remote(7), Op::Select(KeyCode::Left), Op::Move(KeyCode::Home), Op::Move(KeyCode::Left), Op::Select(KeyCode::Up), Op::Type("dash-y"), Op::Select(KeyCode::Right)],
        // A save that only moved the caret's line marked its pasted text saved.
        vec![Op::Enter, Op::Bs, Op::Undo, Op::Type("dash-y"), Op::Select(KeyCode::Left), Op::BackTab, Op::LeaveReturn, Op::Paste("- [x] done item"), Op::TaskCycle, Op::Cut, Op::Enter, Op::MoveLine(false), Op::Undo, Op::TaskCycle, Op::Move(KeyCode::Down), Op::Type("🙂"), Op::Marker("1. "), Op::Type("alpha"), Op::TaskCycle, Op::Type("alpha"), Op::Bs, Op::Type("漢字"), Op::Type("漢字"), Op::Type("bé"), Op::Enter, Op::MoveLine(true), Op::Redo, Op::Type("two words"), Op::Type("two words"), Op::Select(KeyCode::Up), Op::Paste("plain text pasted"), Op::Cut, Op::Tab, Op::Redo, Op::Cut, Op::Move(KeyCode::Left), Op::Cut, Op::Type("🙂"), Op::Enter, Op::Tab, Op::Paste("line a\nline b")],
        // Markers at a note's start were read by the save (a done task "done item").
        vec![Op::Type("🙂"), Op::Enter, Op::Paste("- [x] done item"), Op::Click(0, 7), Op::Marker("- ")],
    ] {
        // Saves land at different moments with each check interval: both.
        for every in [1, 4] {
            if let Err(e) = run(&ops, every, "regression") {
                panic!("{ops:?} (checked every {every})\n{e}");
            }
        }
    }
}

// ---- saves answered late ---------------------------------------------------------------------

/// Ops on a fresh day, saves through a late writer that answers after op `i` only where
/// `answer[i]` (None: saved in place, the oracle). Then everything saved: the buffer's notes and
/// the vault's, without IDs.
fn late(ops: &[Op], answer: Option<&[bool]>) -> (Vec<Note>, Vec<Note>) {
    let (_s, vault) = scratch(&format!("late{}", answer.map_or(0, |a| a.iter().fold(1usize, |h, b| h * 2 + *b as usize))));
    crate::SNAPSHOT.with(|s| s.set(true));
    let mut app = App::new(vault).unwrap();
    app.daemon_live = false;
    WRITER.with(|w| *w.borrow_mut() = None);
    if answer.is_some() {
        let (saver, writer) = crate::doc_app::Saver::manual();
        app.doc_saver = Some(saver);
        WRITER.with(|w| *w.borrow_mut() = Some(writer));
    }
    app.journal_date = app.today;
    app.set_view(View::Journal);
    for (i, op) in ops.iter().enumerate() {
        apply(&mut app, *op);
        if answer.is_some_and(|a| a.get(i).copied().unwrap_or(false)) {
            settle_one(&mut app);
        }
    }
    flush(&mut app);
    let buf = buffer_notes(&app).into_iter().map(|x| x.1).collect();
    let held = saved(&app).0.into_iter().map(|x| x.1).collect();
    WRITER.with(|w| *w.borrow_mut() = None);
    (buf, held)
}

#[test]
fn late_saves_never_roll_back_what_came_after() {
    let t = Op::TaskCycle;
    for ops in [
        // ⌃T ⌃T: a task, then done; the first save's answer came back after the second ⌃T.
        vec![Op::Type("Book the flat"), t, t],
        vec![Op::Type("Book"), t, Op::Type(" the flat"), t],
        vec![Op::Type("x"), t, t, t],
        vec![Op::Type("x"), t, t, t, t],
        vec![Op::Type("one"), t, Op::Enter, Op::Type("two"), t, t, Op::Move(KeyCode::Up), t],
        vec![Op::Marker("[ ] "), Op::Type("a"), t, Op::Enter, t, Op::Type("b"), t],
        // Leaving while a save is out and another waits for it.
        vec![Op::Type("Book the flat"), t, t, Op::LeaveReturn],
        vec![Op::Type("due:fri x"), Op::Enter, Op::Type("y"), Op::Move(KeyCode::Up), Op::Type(" more"), t],
    ] {
        let (want, held) = late(&ops, None);
        assert_eq!(want, held, "{ops:?}: saved in place, the vault isn't the buffer");
        let n = ops.len();
        // Never answered until the end; after every op; after the first only; every other.
        let schedules: Vec<Vec<bool>> = vec![vec![false; n], vec![true; n], (0..n).map(|i| i == 0).collect(), (0..n).map(|i| i % 2 == 1).collect()];
        for answer in schedules {
            let (buf, held) = late(&ops, Some(&answer));
            assert_eq!(buf, want, "{ops:?} answered {answer:?}: the buffer isn't the last state");
            assert_eq!(held, want, "{ops:?} answered {answer:?}: the vault isn't the last state");
        }
    }
}

/// The editor fuzz with the daemon's writer slow: results come back at random moments, so
/// edits keep landing while a save is out.
#[test]
fn fuzz_with_late_saves() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let cases = env_num("THC_FUZZ_CASES", 25);
    let len = env_num("THC_FUZZ_OPS", 60) as usize;
    let first = env_num("THC_FUZZ_SEED", 1);
    let mut failure = None;
    // Found by the soak (case 131): undo brought back a saved note whose delete was pending with
    // its save state from before that save, so it was "new" and was made twice.
    let found = vec![Op::Marker("1. "), Op::Bs, Op::TaskCycle, Op::Type("bé"), Op::Type("🙂"), Op::Type("bé"), Op::Click(18, 10), Op::Enter, Op::Tab, Op::TaskCycle, Op::TaskCycle, Op::Redo, Op::Type("bé"), Op::Undo, Op::Paste("- [x] done item"), Op::Bs, Op::Type("漢字"), Op::Type("bé"), Op::MoveLine(true), Op::Type("dash-y"), Op::Undo, Op::Marker("[ ] "), Op::Cut, Op::Type("🙂"), Op::Paste("- [x] done item"), Op::Type("🙂"), Op::Type("x"), Op::Type("🙂"), Op::Move(KeyCode::Right), Op::Redo, Op::MoveLine(false), Op::Bs, Op::Move(KeyCode::Home), Op::Type("bé"), Op::Select(KeyCode::Up), Op::Enter, Op::Undo, Op::Bs, Op::Undo, Op::Marker("* ")];
    if let Err(e) = run_with(&found, 4, "late-found", Some(0xBADC_0FFE ^ 131)) {
        failure = Some(format!("soak case 131:\n{e}"));
    }
    for case in first..first + cases {
        if failure.is_some() {
            break;
        }
        let mut rng = Rng(0x5A7E_5A7E_0000_0001 ^ case.wrapping_mul(0x1000_0001));
        let ops = generate(&mut rng, len);
        let seed = 0xBADC_0FFE ^ case;
        if let Err(e) = run_with(&ops, 4, &format!("late{case}"), Some(seed)) {
            let small = shrink_with(ops, 4, Some(seed));
            let why = run_with(&small, 4, "final", Some(seed)).err().unwrap_or(e);
            failure = Some(format!("case {case} (late saves), shrunk to {} ops:\n{small:?}\n{why}", small.len()));
            break;
        }
    }
    std::panic::set_hook(hook);
    if let Some(f) = failure {
        panic!("{f}");
    }
}

// ---- two devices, one vault through a Dropbox-like shim ---------------------------------------

/// What happens in the two-device soak: an editor op on one device, a (partial) delivery of
/// one device's log files to the other, or a device noticing new files.
#[derive(Clone, Copy, Debug)]
enum SyncOp {
    Edit(usize, Op),
    /// Copy `pct`% of the not-yet-delivered bytes of every log file of device `from` (a prefix:
    /// the last line can arrive cut, as a sync client writes it).
    Deliver { from: usize, pct: u8 },
    Poll(usize),
}

fn deliver(roots: &[PathBuf; 2], from: usize, pct: u8) {
    let to = 1 - from;
    let src = roots[from].join("vault/log");
    let Ok(devs) = std::fs::read_dir(&src) else { return };
    for dev in devs.flatten() {
        let Ok(files) = std::fs::read_dir(dev.path()) else { continue };
        for f in files.flatten() {
            let target = roots[to].join("vault/log").join(dev.file_name()).join(f.file_name());
            let data = std::fs::read(f.path()).unwrap_or_default();
            let have = std::fs::metadata(&target).map(|m| m.len() as usize).unwrap_or(0);
            if data.len() <= have {
                continue;
            }
            let upto = have + (data.len() - have) * pct as usize / 100;
            if upto > have {
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::write(&target, &data[..upto]).unwrap();
            }
        }
    }
}

/// Both stores, as rows that must match: nodes (all fields, deleted ones too) and conflicts.
fn store_dump(app: &App) -> String {
    let c = &app.vault.store.conn;
    let mut out = String::new();
    for sql in [
        "SELECT id, coalesce(parent,''), ord, coalesce(title,''), text, coalesce(status,''), coalesce(scheduled,''), coalesce(due,''), coalesce(priority,''), coalesce(journal,''), is_tag, deleted FROM nodes ORDER BY id",
        "SELECT node, field, resolved FROM conflicts ORDER BY node, field",
    ] {
        let mut st = c.prepare(sql).unwrap();
        let n = st.column_count();
        let rows = st
            .query_map([], |r| {
                let mut s = String::new();
                for i in 0..n {
                    let v: rusqlite::types::Value = r.get(i)?;
                    s.push_str(&format!("{v:?}|"));
                }
                Ok(s)
            })
            .unwrap();
        for r in rows {
            out.push_str(&r.unwrap());
            out.push('\n');
        }
    }
    out
}

fn run_sync(ops: &[SyncOp], tag: &str) -> Result<(), String> {
    run_sync_with(ops, tag, false)
}

fn run_sync_with(ops: &[SyncOp], tag: &str, strict_order: bool) -> Result<(), String> {
    let ops = ops.to_vec();
    let tag = tag.to_string();
    let r = std::panic::catch_unwind(move || -> Result<(), String> {
        let base = std::env::temp_dir().join(format!("thc-sync-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let roots = [base.join("a"), base.join("b")];
        let mut apps: Vec<App> = Vec::new();
        crate::SNAPSHOT.with(|s| s.set(true));
        for root in &roots {
            thc_core::vault::init(&root.join("vault"), None, None).unwrap();
            let v = Vault::open(Paths { vault: root.join("vault"), cache: root.join("cache") }, Actor { kind: "human".into(), name: None }, "tui").unwrap();
            let mut app = App::new(v).unwrap();
            app.daemon_live = false;
            app.journal_date = app.today;
            app.set_view(View::Journal);
            apps.push(app);
        }
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let settle = |app: &mut App| {
            app.overlay = None;
            app.prompt = None;
            let on_today = app.view == View::Journal && app.journal_date == app.today && app.doc.as_ref().is_some_and(|d| matches!(d.target, Target::Journal { .. }));
            if !on_today {
                app.journal_date = app.today;
                app.set_view(View::Journal);
            }
        };
        for (step, op) in ops.iter().enumerate() {
            match *op {
                SyncOp::Edit(d, e) => {
                    apply(&mut apps[d], e);
                    settle(&mut apps[d]);
                    term.draw(|f| crate::ui::draw_app(f, &mut apps[d])).unwrap();
                    apps[d].after_frame();
                }
                SyncOp::Deliver { from, pct } => deliver(&roots, from, pct),
                SyncOp::Poll(d) => {
                    apps[d].poll_external().map_err(|e| format!("step {step}: poll: {e:#}"))?;
                    settle(&mut apps[d]);
                }
            }
            if std::env::var_os("THC_SYNC_TRACE").is_some() {
                eprintln!("== {step} {op:?}");
                for (i, app) in apps.iter().enumerate() {
                    let caret = app.doc.as_ref().map(|d| d.line().id.clone()).unwrap_or_default();
                    let buf: Vec<String> = app.doc.as_ref().map(|d| d.lines().iter().map(|l| format!("{}{} {:?} {:?} saved_kind={:?} edited={}", if l.id == caret { "^" } else { "" }, &l.id[..4], l.kind, l.text, l.saved_kind, l.edited())).collect()).unwrap_or_default();
                    eprintln!("  dev{i} buf {buf:?}\n  dev{i} vault {:?}", saved(app).0);
                }
            }
        }
        // Everything saved, everything delivered, until nothing moves.
        for _ in 0..6 {
            for app in apps.iter_mut() {
                app.save_doc(true);
                app.drain_saves(true);
            }
            deliver(&roots, 0, 100);
            deliver(&roots, 1, 100);
            for app in apps.iter_mut() {
                app.poll_external().map_err(|e| format!("final poll: {e:#}"))?;
                settle(app);
            }
        }
        let (da, db) = (store_dump(&apps[0]), store_dump(&apps[1]));
        if da != db {
            let diff: Vec<String> = da.lines().zip(db.lines()).filter(|(x, y)| x != y).map(|(x, y)| format!("a {x}\nb {y}")).take(6).collect();
            return Err(format!("the devices didn't converge ({} vs {} rows):\n{}", da.lines().count(), db.lines().count(), diff.join("\n")));
        }
        for (i, app) in apps.iter_mut().enumerate() {
            let today = app.today.format("%Y-%m-%d").to_string();
            let days: i64 = app.vault.store.conn.query_row("SELECT count(*) FROM nodes WHERE journal = ?1 AND deleted = 0", [&today], |r| r.get(0)).unwrap();
            // (None until the first save makes it.)
            if days > 1 {
                return Err(format!("device {i}: today's journal is {days} nodes, not one"));
            }
            check_with(app, strict_order).map_err(|e| format!("device {i}: {e}"))?;
            // Nothing live is outside every outline: a live note whose parent is deleted is
            // re-homed (and flagged), never just hidden.
            let hidden: i64 = app
                .vault
                .store
                .conn
                .query_row("SELECT count(*) FROM nodes c JOIN nodes p ON p.id = c.parent WHERE c.deleted = 0 AND p.deleted = 1 AND c.id NOT IN (SELECT node FROM rehomed)", [], |r| r.get(0))
                .unwrap();
            if hidden > 0 {
                return Err(format!("device {i}: {hidden} live note(s) under a deleted parent, not re-homed"));
            }
            let warnings = app.vault.warnings.clone();
            if !warnings.is_empty() {
                return Err(format!("device {i}: log warnings: {warnings:?}"));
            }
        }
        let _ = std::fs::remove_dir_all(&base);
        Ok(())
    });
    match r {
        Ok(r) => r,
        Err(p) => Err(format!("panic: {}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())),
    }
}

fn generate_sync(rng: &mut Rng, n: usize) -> Vec<SyncOp> {
    let edits = generate(rng, n);
    edits
        .into_iter()
        .flat_map(|e| {
            let d = rng.below(2);
            let mut v = vec![SyncOp::Edit(d, e)];
            match rng.below(10) {
                0..=2 => v.push(SyncOp::Deliver { from: rng.below(2), pct: [30u8, 60, 100, 100][rng.below(4)] }),
                3 | 4 => v.push(SyncOp::Poll(rng.below(2))),
                _ => {}
            }
            v
        })
        .collect()
}

fn shrink_sync(mut ops: Vec<SyncOp>) -> Vec<SyncOp> {
    let mut chunk = ops.len() / 2;
    let mut n = 0;
    while chunk >= 1 {
        let mut i = 0;
        let mut progressed = false;
        while i < ops.len() {
            let mut t = ops.clone();
            t.drain(i..(i + chunk).min(t.len()));
            n += 1;
            if run_sync(&t, &format!("shrink{n}")).is_err() {
                ops = t;
                progressed = true;
            } else {
                i += chunk;
            }
        }
        if !progressed {
            chunk /= 2;
        }
    }
    ops
}

/// Two devices editing the same day through the real editor, their logs crossing through a
/// shim that delivers late, partly (a line cut mid-way) and out of order: both converge to the
/// same store, today's day is one node, every buffer matches its vault, and no log line is bad.
#[test]
fn fuzz_two_devices_converge() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let cases = env_num("THC_SYNC_CASES", 12);
    let len = env_num("THC_SYNC_OPS", 40) as usize;
    let first = env_num("THC_SYNC_SEED", 1);
    let mut failure = None;
    for case in first..first + cases {
        let mut rng = Rng(0xD1CE_5EED ^ case.wrapping_mul(0x9E37_79B9));
        let ops = generate_sync(&mut rng, len);
        if let Err(e) = run_sync(&ops, &format!("case{case}")) {
            let small = shrink_sync(ops);
            let why = match run_sync(&small, "final") {
                Err(w) => w,
                Ok(()) => format!("(the shrunk replay passed: flaky; the original failure follows)\n{e}"),
            };
            failure = Some(format!("case {case}, shrunk to {} ops:\n{small:?}\n{why}", small.len()));
            break;
        }
    }
    std::panic::set_hook(hook);
    if let Some(f) = failure {
        panic!("{f}");
    }
}

/// Found by the two-device soak (shrunk): replays that once failed.
#[test]
fn sync_regressions() {
    for (n, ops) in [
        vec![SyncOp::Edit(1, Op::Type("end.")), SyncOp::Edit(1, Op::TaskCycle), SyncOp::Edit(1, Op::Enter), SyncOp::Edit(1, Op::Type("🙂")), SyncOp::Edit(0, Op::Type("alpha")), SyncOp::Edit(0, Op::LeaveReturn), SyncOp::Deliver { from: 0, pct: 100 }, SyncOp::Edit(1, Op::Enter), SyncOp::Edit(1, Op::Select(KeyCode::Up)), SyncOp::Edit(0, Op::Type("🙂")), SyncOp::Deliver { from: 1, pct: 100 }, SyncOp::Edit(0, Op::LeaveReturn), SyncOp::Edit(0, Op::Move(KeyCode::Up)), SyncOp::Edit(0, Op::Enter), SyncOp::Edit(0, Op::Click(40, 9))],
        // Flaky in the soak (ids and timing vary): replays that once failed, kept as guards.
        vec![SyncOp::Edit(1, Op::Paste("- one\n- [ ] two\n\nthird para")), SyncOp::Deliver { from: 1, pct: 100 }, SyncOp::Poll(0), SyncOp::Edit(1, Op::Enter), SyncOp::Edit(0, Op::Type("two words")), SyncOp::Edit(1, Op::Type("bé")), SyncOp::Edit(0, Op::Enter), SyncOp::Edit(0, Op::Enter), SyncOp::Edit(0, Op::MoveLine(true)), SyncOp::Edit(1, Op::LeaveReturn), SyncOp::Edit(1, Op::Move(KeyCode::Up)), SyncOp::Edit(0, Op::Undo), SyncOp::Edit(0, Op::Type("dash-y")), SyncOp::Edit(0, Op::LeaveReturn), SyncOp::Deliver { from: 0, pct: 100 }, SyncOp::Poll(1), SyncOp::Edit(1, Op::Enter)],
        vec![SyncOp::Edit(0, Op::Marker("1. ")), SyncOp::Edit(1, Op::Type("two words")), SyncOp::Edit(0, Op::Type("漢字")), SyncOp::Edit(1, Op::LeaveReturn), SyncOp::Edit(1, Op::Type("alpha")), SyncOp::Edit(0, Op::Enter), SyncOp::Edit(1, Op::LeaveReturn), SyncOp::Edit(0, Op::Bs), SyncOp::Edit(0, Op::Bs), SyncOp::Deliver { from: 1, pct: 100 }, SyncOp::Poll(0), SyncOp::Edit(1, Op::BackTab), SyncOp::Edit(0, Op::Enter)],
        // Deleted elsewhere while the caret was on it: it stayed for good.
        vec![SyncOp::Edit(0, Op::Type("🙂")), SyncOp::Edit(0, Op::Marker("- ")), SyncOp::Edit(0, Op::Select(KeyCode::Up)), SyncOp::Edit(0, Op::Marker("* ")), SyncOp::Deliver { from: 0, pct: 60 }, SyncOp::Poll(1), SyncOp::Edit(1, Op::Bs)],
        vec![SyncOp::Edit(0, Op::Marker("1. ")), SyncOp::Edit(0, Op::Move(KeyCode::Home)), SyncOp::Edit(0, Op::Paste("plain text pasted")), SyncOp::Edit(0, Op::Enter), SyncOp::Edit(0, Op::Select(KeyCode::Up)), SyncOp::Deliver { from: 0, pct: 30 }, SyncOp::Deliver { from: 0, pct: 60 }, SyncOp::Edit(0, Op::Marker("[ ] ")), SyncOp::Poll(1), SyncOp::Edit(1, Op::Select(KeyCode::Up))],
        // A kind changed elsewhere on the line you're on: held, never taken up.
        vec![SyncOp::Edit(0, Op::Type("end.")), SyncOp::Edit(0, Op::Marker("* ")), SyncOp::Deliver { from: 0, pct: 60 }, SyncOp::Poll(1), SyncOp::Edit(1, Op::Bs)],
        // A click mapped with the last frame's layout after the document reopened: a panic.
        vec![SyncOp::Edit(1, Op::Type("漢字")), SyncOp::Edit(1, Op::LeaveReturn), SyncOp::Edit(0, Op::Paste("- one\n- [ ] two\n\nthird para")), SyncOp::Edit(0, Op::Enter), SyncOp::Edit(0, Op::Paste("- one\n- [ ] two\n\nthird para")), SyncOp::Edit(1, Op::Type("dash-y")), SyncOp::Deliver { from: 1, pct: 100 }, SyncOp::Edit(1, Op::Marker("- ")), SyncOp::Poll(0), SyncOp::Edit(0, Op::Enter), SyncOp::Deliver { from: 1, pct: 100 }, SyncOp::Poll(0), SyncOp::Edit(1, Op::TaskCycle), SyncOp::Edit(0, Op::Click(95, 18))],
        // A day opened before it existed never took up the other device's notes.
        vec![SyncOp::Edit(0, Op::Type("漢字"))],
        vec![SyncOp::Edit(1, Op::Type("bé")), SyncOp::Edit(0, Op::Type("alpha"))],
        // Events taken in by a local save's catch-up never reached the open document.
        vec![SyncOp::Edit(1, Op::Type("dash-y")), SyncOp::Edit(1, Op::Marker("- ")), SyncOp::Edit(0, Op::Type("dash-y")), SyncOp::Deliver { from: 1, pct: 100 }],
        // A kind that arrived after its note (a cut delivery) never patched in.
        vec![SyncOp::Edit(0, Op::Paste("- one\n- [ ] two\n\nthird para")), SyncOp::Edit(0, Op::Enter), SyncOp::Deliver { from: 0, pct: 60 }, SyncOp::Edit(0, Op::Enter), SyncOp::Deliver { from: 0, pct: 60 }, SyncOp::Poll(1)],
        // Another device's text change, taken in by a save here, never reached the open line.
        vec![SyncOp::Edit(0, Op::Type("two words")), SyncOp::Edit(0, Op::Marker("[ ] ")), SyncOp::Deliver { from: 0, pct: 100 }, SyncOp::Edit(0, Op::Remote(4)), SyncOp::Edit(1, Op::Marker("[ ] ")), SyncOp::Edit(1, Op::Type("alpha")), SyncOp::Edit(1, Op::TaskCycle), SyncOp::Poll(1), SyncOp::Deliver { from: 0, pct: 100 }, SyncOp::Edit(1, Op::TaskCycle)],
        // Undo here took back a note that arrived from the other device since: deleted it.
        vec![SyncOp::Deliver { from: 1, pct: 100 }, SyncOp::Edit(0, Op::Type("🙂")), SyncOp::Edit(1, Op::Type("🙂")), SyncOp::Edit(0, Op::LeaveReturn), SyncOp::Edit(1, Op::LeaveReturn), SyncOp::Edit(1, Op::Bs), SyncOp::Edit(0, Op::Type("x")), SyncOp::Deliver { from: 1, pct: 100 }, SyncOp::Poll(0), SyncOp::Edit(0, Op::Undo)],
        vec![SyncOp::Edit(1, Op::Marker("- ")), SyncOp::Edit(0, Op::Type("dash-y")), SyncOp::Edit(1, Op::Type("漢字")), SyncOp::Edit(1, Op::Enter), SyncOp::Edit(1, Op::Select(KeyCode::Up)), SyncOp::Deliver { from: 1, pct: 100 }, SyncOp::Edit(0, Op::LeaveReturn), SyncOp::Edit(0, Op::Move(KeyCode::Up)), SyncOp::Edit(0, Op::Enter)],
        vec![SyncOp::Edit(0, Op::Marker("1. ")), SyncOp::Edit(0, Op::Move(KeyCode::Home)), SyncOp::Edit(0, Op::Paste("plain text pasted")), SyncOp::Edit(0, Op::Enter), SyncOp::Edit(0, Op::Select(KeyCode::Up)), SyncOp::Deliver { from: 0, pct: 30 }, SyncOp::Deliver { from: 0, pct: 60 }, SyncOp::Edit(0, Op::Marker("[ ] ")), SyncOp::Poll(1), SyncOp::Edit(1, Op::Select(KeyCode::Up))],
    ]
    .into_iter()
    .enumerate()
    {
        let tag = format!("regression{n}");
        let repeat: usize = std::env::var("THC_SYNC_REPEAT").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        for _ in 1..repeat {
            if let Err(e) = run_sync(&ops, &tag) {
                eprintln!("FAIL {ops:?}\n{e}\n");
                break;
            }
        }
        if let Err(e) = run_sync(&ops, &tag) {
            if std::env::var_os("THC_SYNC_ALL").is_some() {
                eprintln!("FAIL {ops:?}\n{e}\n");
                continue;
            }
            panic!("{ops:?}\n{e}");
        }
    }
}

/// A child added on one device under a note the other device deletes concurrently: the delete
/// tombstones only the descendants the deleting device knew, so the child survives. It's
/// re-homed under the nearest live ancestor and flagged ≠ (moved here) on both devices, never
/// in no outline (by design; daemon.md §4.0a).
#[test]
fn sync_orphan_under_concurrent_delete() {
    let ops = vec![SyncOp::Edit(1, Op::Paste("line a\nline b")), SyncOp::Edit(1, Op::Enter), SyncOp::Edit(1, Op::TaskCycle), SyncOp::Edit(0, Op::Type("two words")), SyncOp::Edit(0, Op::Marker("1. ")), SyncOp::Edit(0, Op::Move(KeyCode::Home)), SyncOp::Deliver { from: 0, pct: 100 }, SyncOp::Poll(1), SyncOp::Edit(1, Op::Tab), SyncOp::Edit(1, Op::Select(KeyCode::Up)), SyncOp::Edit(1, Op::Cut), SyncOp::Edit(0, Op::Type("end.")), SyncOp::Edit(1, Op::Undo), SyncOp::Edit(0, Op::Enter), SyncOp::Edit(0, Op::Select(KeyCode::Up)), SyncOp::Deliver { from: 0, pct: 60 }, SyncOp::Edit(1, Op::Type("🙂")), SyncOp::Edit(0, Op::Marker("[ ] ")), SyncOp::Edit(1, Op::Enter), SyncOp::Poll(1), SyncOp::Edit(1, Op::Select(KeyCode::Up))];
    if let Err(e) = run_sync(&ops, "orphan") {
        panic!("{e}");
    }
}

/// Open (two-device soak, flaky: depends on the random ids): after moves on both devices the
/// open document can show siblings in an older order than the vault's. The same notes, nothing
/// lost; reopening shows the vault's order. Ignored until patch_doc reconciles order.
#[test]
#[ignore]
fn sync_sibling_order_after_concurrent_moves() {
    for (n, ops) in [
        vec![SyncOp::Edit(1, Op::Paste("- one\n- [ ] two\n\nthird para")), SyncOp::Deliver { from: 1, pct: 100 }, SyncOp::Poll(0), SyncOp::Edit(1, Op::Enter), SyncOp::Edit(0, Op::Type("two words")), SyncOp::Edit(1, Op::Type("bé")), SyncOp::Edit(0, Op::Enter), SyncOp::Edit(0, Op::Enter), SyncOp::Edit(0, Op::MoveLine(true)), SyncOp::Edit(1, Op::LeaveReturn), SyncOp::Edit(1, Op::Move(KeyCode::Up)), SyncOp::Edit(0, Op::Undo), SyncOp::Edit(0, Op::Type("dash-y")), SyncOp::Edit(0, Op::LeaveReturn), SyncOp::Deliver { from: 0, pct: 100 }, SyncOp::Poll(1), SyncOp::Edit(1, Op::Enter)],
        vec![SyncOp::Edit(0, Op::Marker("1. ")), SyncOp::Edit(1, Op::Type("two words")), SyncOp::Edit(0, Op::Type("漢字")), SyncOp::Edit(1, Op::LeaveReturn), SyncOp::Edit(1, Op::Type("alpha")), SyncOp::Edit(0, Op::Enter), SyncOp::Edit(1, Op::LeaveReturn), SyncOp::Edit(0, Op::Bs), SyncOp::Edit(0, Op::Bs), SyncOp::Deliver { from: 1, pct: 100 }, SyncOp::Poll(0), SyncOp::Edit(1, Op::BackTab), SyncOp::Edit(0, Op::Enter)],
        vec![SyncOp::Edit(0, Op::Paste("- one\n- [ ] two\n\nthird para")), SyncOp::Edit(0, Op::Click(23, 6)), SyncOp::Deliver { from: 0, pct: 100 }, SyncOp::Edit(0, Op::Enter), SyncOp::Poll(1), SyncOp::Edit(1, Op::Bs), SyncOp::Edit(0, Op::Type("dash-y")), SyncOp::Edit(1, Op::MoveLine(true)), SyncOp::Edit(1, Op::MoveLine(true)), SyncOp::Edit(1, Op::MoveLine(false)), SyncOp::Edit(0, Op::Enter), SyncOp::Edit(1, Op::LeaveReturn), SyncOp::Edit(0, Op::Type("bé")), SyncOp::Deliver { from: 1, pct: 100 }, SyncOp::Poll(0)],
        vec![SyncOp::Edit(1, Op::Type("two words")), SyncOp::Edit(0, Op::Type("🙂")), SyncOp::Edit(0, Op::Marker("1. ")), SyncOp::Edit(0, Op::LeaveReturn), SyncOp::Deliver { from: 0, pct: 100 }, SyncOp::Edit(1, Op::LeaveReturn), SyncOp::Edit(1, Op::Type("alpha")), SyncOp::Edit(1, Op::LeaveReturn), SyncOp::Edit(0, Op::Paste("- one\n- [ ] two\n\nthird para")), SyncOp::Deliver { from: 1, pct: 100 }, SyncOp::Poll(0), SyncOp::Edit(0, Op::Bs), SyncOp::Edit(0, Op::Click(84, 11)), SyncOp::Edit(0, Op::Enter), SyncOp::Poll(1), SyncOp::Edit(0, Op::Type("漢字"))],
    ]
    .into_iter()
    .enumerate()
    {
        if let Err(e) = run_sync_with(&ops, &format!("order{n}"), true) {
            panic!("{e}");
        }
    }
}
