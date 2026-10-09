//! Caret motion end to end (docs/design/motion.md): real documents, real wrapping, keys replayed
//! through the TUI, and the drawn caret checked. The rules themselves are table- and
//! property-tested in thc-tui's editor/motion.rs; these check that render and motion agree (I6).

mod common;

use std::path::PathBuf;

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = thc_core::scratch::dir(&format!("thc-motion-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        v.run(&[], &["init", "vault"]);
        v
    }

    fn run(&self, env: &[(&str, &str)], args: &[&str]) -> String {
        let mut c = common::thc();
        c.args(args).current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env("THC_NOW", "2026-10-03T09:00").env_remove("THC_ACTOR");
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    /// A page with these notes, in order.
    fn page(&self, title: &str, notes: &[&str]) {
        let j: serde_json::Value = serde_json::from_str(&self.run(&[], &["--json", "page", "new", title])).unwrap();
        let id = j["nodes"][0]["id"].as_str().or(j["id"].as_str()).expect("page id").to_string();
        for n in notes {
            self.run(&[], &["add", "--plain", "--under", &id, n]);
        }
    }

    /// The frame after `keys` in the page, the caret drawn as `▮`.
    fn keys(&self, title: &str, keys: &str) -> String {
        self.run(&[("THC_TUI_SNAPSHOT", "110x30"), ("THC_TUI_KEYS", keys), ("THC_TUI_SNAPSHOT_CURSOR", "1")], &["p", title, "--no-focus"])
    }
}

fn caret_row(f: &str) -> String {
    f.lines().find(|l| l.contains('▮')).unwrap_or_else(|| panic!("no caret:\n{f}")).trim().to_string()
}

const LONG: &str = "The quick brown fox jumps over the lazy dog and keeps running through the long grass until dusk, then rests under an old oak tree while the sun goes down behind the hills and the evening settles in quietly over everything.";

/// G1: at the start of a paragraph's last wrapped row, ↓ goes to the next note
/// (0.9.31 stayed on the same row, at its end).
#[test]
fn down_from_the_start_of_a_wrapped_row_goes_to_the_next_note() {
    let v = V::new("g1");
    v.page("Fox", &[LONG, "Short one.", "Last note."]);
    // Doc start, then ↓ row by row with the goal at 0: each lands at a row's start.
    let f = v.keys("Fox", "<c-home><down>");
    let second = caret_row(&f);
    assert!(second.starts_with('▮'), "row 2 starts with the caret: {f}");
    let f = v.keys("Fox", "<c-home><down><down>");
    let third = caret_row(&f);
    assert!(third.starts_with('▮') && third != second, "row 3: {f}");
    // From the start of the last wrapped row (the third, at 72 columns), ↓ is the next note.
    let f = v.keys("Fox", "<c-home><down><down><down><down>");
    assert!(caret_row(&f).contains("▮hort one."), "{f}");
    // And ↑ comes back to the start of the paragraph's last row.
    let f = v.keys("Fox", "<c-home><down><down><down><down><up>");
    let back = caret_row(&f);
    assert!(back.starts_with('▮') && LONG.ends_with(&back[3..].trim_end().to_string()) || back.contains("everything"), "{f}");
}

/// G4 and G5: End on a wrapped row stops before the break space, and → crosses the wrap in one
/// press; the caret is drawn where motion put it (I6).
#[test]
fn end_and_right_across_a_wrap() {
    let v = V::new("g4");
    v.page("Fox", &[LONG, "Short one."]);
    let f = v.keys("Fox", "<c-home><end>");
    let row = caret_row(&f);
    // The caret sits right after the row's last word, on that row (not at the next row's start).
    assert!(row.ends_with('▮') || row.contains("▮ "), "End stays on the row: {f}");
    let f = v.keys("Fox", "<c-home><end><right>");
    let next = caret_row(&f);
    assert!(next.starts_with('▮'), "→ crosses the wrap to the next row's start: {f}");
    assert_ne!(row, next);
}

/// editing.md §2 (E1-E3, E7): with a selection, ← and → collapse it to its start
/// or end and stop there; they never move on to the next character, row or note.
#[test]
fn left_and_right_collapse_a_selection() {
    let v = V::new("collapse");
    v.page("Sel", &["First note", "Second note"]);
    // ⌃Home ↓ End → "Second note▮"; ← ← → "Second no▮te"; ⇧← ×6 → "Sec⟦▮ond no⟧te".
    let sel = "<c-home><down><end><left><left><s-left><s-left><s-left><s-left><s-left><s-left>";
    let f = v.keys("Sel", &format!("{sel}<left>X"));
    // (The snapshot draws the cursor over the character after it.)
    assert!(f.contains("SecX▮nd note") && f.contains("First note"), "E7, ← to the start: {f}");
    let f = v.keys("Sel", &format!("{sel}<right>X"));
    assert!(f.contains("Second noX▮e"), "E2/E3, → to the end: {f}");
    // ⇧ keeps the anchor: one more ⇧← grows it; typing replaces it.
    let f = v.keys("Sel", &format!("{sel}<s-left>X"));
    assert!(f.contains("SeX▮e"), "⇧← grows it, typing replaces it: {f}");
}
