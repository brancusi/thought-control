//! Lists keep their order (navigation.md §8): looking never reorders; your own
//! row actions change a row where it is; the rail is frozen between arrivals; a row added from
//! elsewhere goes in without moving the selection.

mod common;

use std::path::PathBuf;

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = thc_core::scratch::dir(&format!("thc-order-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        v.cli(&["init", "vault"]);
        v
    }

    fn cmd(&self) -> std::process::Command {
        let mut c = common::thc();
        c.current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env_remove("THC_ACTOR").env("THC_NOW", "");
        c
    }

    fn cli(&self, args: &[&str]) -> String {
        let o = self.cmd().args(args).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn tui(&self, size: &str, keys: &str) -> String {
        let o = self.cmd().args(["tui"]).env("THC_TUI_SNAPSHOT", size).env("THC_TUI_KEYS", keys).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }
}

impl Drop for V {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The order of the given labels as they appear top to bottom.
fn order(f: &str, labels: &[&str]) -> Vec<String> {
    let mut seen: Vec<(usize, String)> = labels.iter().filter_map(|l| f.lines().position(|x| x.contains(l)).map(|y| (y, l.to_string()))).collect();
    seen.sort();
    seen.into_iter().map(|(_, l)| l).collect()
}

fn pos(f: &str, needle: &str) -> (usize, usize) {
    for (y, l) in f.lines().enumerate() {
        if let Some(b) = l.find(needle) {
            return (l[..b].chars().count(), y);
        }
    }
    panic!("{needle:?} not on screen: {f}");
}

const PAGES: [&str; 5] = ["Alpha", "Bravo", "Charlie", "Delta", "Echo"];

#[test]
fn n21_looking_never_reorders_pages() {
    let v = V::new("n21");
    for p in PAGES {
        v.cli(&["page", "new", p]);
    }
    let f = v.tui("100x30", "4");
    let base = order(&f, &PAGES);
    assert_eq!(base, PAGES.to_vec(), "by name: {f}");
    let (x, y) = pos(&f, "Charlie");
    let (hx, hy) = pos(&f, "Echo");
    for keys in [format!("4<click:{},{y}>", x + 1), format!("4<hover:{},{hy}>", hx + 1), format!("4<dclick:{},{y}><esc>", x + 1), format!("4<click:{},{y}><cr><esc>", x + 1)] {
        let f = v.tui("100x30", &keys);
        assert_eq!(order(&f, &PAGES), base, "{keys}: {f}");
    }
}

#[test]
fn n23_the_rail_is_frozen_while_you_go_through_it() {
    let v = V::new("n23");
    for p in PAGES {
        v.cli(&["page", "new", p]);
    }
    // Wide enough for the rail. Open Alpha from Pages, then Delta and Echo through the rail.
    let rail = |f: &str| -> Vec<String> {
        let col: Vec<String> = f.lines().map(|l| l.chars().take(24).collect::<String>()).collect();
        order(&col.join("\n"), &PAGES)
    };
    let f = v.tui("140x30", "4<c-o>Alpha<cr>");
    let base = rail(&f);
    assert_eq!(base.len(), 5, "the rail: {f}");
    let (dx, dy) = pos(&f.lines().map(|l| l.chars().take(24).collect::<String>()).collect::<Vec<_>>().join("\n"), "Delta");
    let keys = format!("4<c-o>Alpha<cr><click:{},{dy}>", dx + 1);
    let f = v.tui("140x30", &keys);
    assert!(f.contains("▌Delta"), "{f}");
    assert_eq!(rail(&f), base, "the rail keeps its order: {f}");
    let (ex, ey) = pos(&f.lines().map(|l| l.chars().take(24).collect::<String>()).collect::<Vec<_>>().join("\n"), "Echo");
    let f = v.tui("140x30", &format!("{keys}<click:{},{ey}>", ex + 1));
    assert!(f.contains("▌Echo"), "{f}");
    assert_eq!(rail(&f), base, "still: {f}");
}

#[test]
fn n24_done_in_today_stays_dimmed_in_place_until_refresh() {
    let v = V::new("n24");
    v.cli(&["todo", "first task", "--due", "today"]);
    v.cli(&["todo", "second task", "--due", "today"]);
    v.cli(&["todo", "third task", "--due", "today"]);
    let labels = ["first task", "second task", "third task"];
    let f = v.tui("100x30", "1");
    let base = order(&f, &labels);
    // x on the first: it stays where it is, done.
    let f = v.tui("100x30", "1x");
    assert_eq!(order(&f, &labels), base, "{f}");
    let row = f.lines().find(|l| l.contains(&base[0])).unwrap();
    assert!(row.contains("[x]"), "done where it is: {f}");
    assert!(!f.contains("Done today") || f.lines().position(|l| l.contains("Done today")) > f.lines().position(|l| l.contains(&base[2])), "{f}");
    // ⌃L: it moves to Done today.
    let f = v.tui("100x30", "1x<c-l>");
    let done_at = f.lines().position(|l| l.contains("Done today")).expect("a Done today section");
    let row_at = f.lines().position(|l| l.contains(&base[0])).unwrap();
    assert!(row_at > done_at, "moved on ⌃L: {f}");
}

#[test]
fn n25_a_row_added_elsewhere_doesnt_move_the_selection() {
    let v = V::new("n25");
    for t in ["bravo task", "delta task"] {
        v.cli(&["todo", t, "--due", "today"]);
    }
    // Select delta, then an agent adds a task that sorts before it.
    // (The two tie on the sort, so either may be first: pick the key that lands on delta.)
    let sel = |f: &str| f.lines().find(|l| l.starts_with('▌')).map(|l| l.to_string()).unwrap_or_default();
    let keys = if sel(&v.tui("100x30", "1")).contains("delta task") { "1" } else { "1j" };
    let f = v.tui("100x30", keys);
    assert!(sel(&f).contains("delta task"), "{f}");
    let f = v.tui("100x30", &format!("{keys}<agent:[ ] alpha task due:today>"));
    assert!(f.contains("alpha task"), "the new row shows: {f}");
    assert!(sel(&f).contains("delta task"), "the selection stays on its row: {f}");
}
