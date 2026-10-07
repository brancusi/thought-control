//! Flows: a person who does a bit of everything. A seeded random walk over a page: typing
//! (wide characters too), Enter, Tab, Backspace, caret keys, selection, undo and redo, clicks
//! anywhere in the document, the wheel, resizes. The invariants hold after every step; the
//! same seed walks the same way every run.
use super::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn walk(seed: u64, steps: usize, size: (u16, u16)) {
    let mut f = flow_with(&format!("monkey {seed:x}"), Size::Small, size);
    // An empty line under the caret isn't saved, so a fresh session lays it out without it.
    f.known("02pjq", Known::Restore);
    f.keys("<c-o>Q4 Plan<cr>");
    let mut rng = Rng(seed);
    let chars = ["a", "e", "t", " ", "s", "日", "🙂", "é", "W", ".", "o", "n"];
    let caret = ["<left>", "<right>", "<up>", "<down>", "<home>", "<end>", "<m-left>", "<m-right>", "<s-right>", "<s-left>", "<s-down>", "<c-home>", "<c-end>"];
    for _ in 0..steps {
        // Somewhere to write, whatever the last step did.
        if f.s.app.doc.is_none() || f.s.app.ui.overlay.is_some() || f.s.app.prompt.is_some() || f.s.app.ui.focus != Focus::List {
            f.keys("<esc>");
            if f.s.app.doc.is_none() {
                f.keys("<c-o>Q4 Plan<cr>");
            }
            continue;
        }
        match rng.below(20) {
            0..=7 => {
                let n = 1 + rng.below(6);
                let s: String = (0..n).map(|_| chars[rng.below(chars.len())]).collect();
                f.type_text(&s);
            }
            8 => {
                f.keys("<cr>");
            }
            9 => {
                f.keys(["<tab>", "<s-tab>"][rng.below(2)]);
            }
            10 => {
                f.keys_as(Motion::Typing, "<bs>");
            }
            11..=13 => {
                f.moves(caret[rng.below(caret.len())]);
            }
            14 => {
                f.keys(["<c-z>", "<c-y>"][rng.below(2)]);
            }
            15 | 16 => {
                // A click on a random cell of a row of the page's text (⌥: never follows a link;
                // the rows under it, linked from and also today, open what they name).
                let rows: Vec<u16> = f.s.app.render.doc_hits.iter().map(|h| h.y).collect();
                if let (Some(r), false) = (f.shot.doc_view, rows.is_empty()) {
                    let (x, y) = (r.x + rng.below(r.width as usize) as u16, rows[rng.below(rows.len())]);
                    f.alt_click(At::Cell(x, y));
                }
            }
            17 => {
                f.wheel(rng.below(2) == 0, 1 + rng.below(3) as u32, None);
            }
            18 => {
                f.idle();
            }
            _ => {
                let sizes = [(80, 24), (100, 30), (140, 36), (120, 40)];
                let (w, h) = sizes[rng.below(sizes.len())];
                f.resize(w, h);
            }
        }
    }
    f.done();
}

#[test]
fn monkey_on_a_page() {
    walk(0x5eed_f10e, 200, (140, 36));
}

#[test]
fn monkey_on_a_narrow_screen() {
    walk(0xc0ffee, 200, (80, 24));
}

#[test]
#[ignore = "j9xm7"]
fn monkey_deadbeef_backspace_at_the_end() {
    // Step 499: a Backspace near the end of the scrolled page pulls the view down.
    walk(0xdead_beef, 250, (80, 24));
}

#[test]
fn monkey_more_seeds() {
    // THC_FLOW_SEEDS=n walks n more seeds (a longer hunt, off CI).
    let n: u64 = std::env::var("THC_FLOW_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    for i in 0..n {
        walk(0x1000 + i * 7919, 150, (120, 32));
    }
}
