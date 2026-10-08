//! The keymap (docs/design/keymap.md §0 step 1): one table, exact matching, prefixes that cancel
//! and re-dispatch, the palette running actions. Snapshots against a scratch vault.

mod common;

use serde_json::Value;
use std::path::PathBuf;

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = thc_core::scratch::dir(&format!("thc-keymap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        v.cli(&["init", "vault"]);
        v
    }

    fn cmd(&self, env: &[(&str, &str)], args: &[&str]) -> std::process::Output {
        let mut c = common::thc();
        c.args(args).current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env("THC_NOW", "2026-10-03T09:00").env("THC_TUI_RENDER", "1").env_remove("THC_ACTOR");
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    fn cli(&self, args: &[&str]) -> String {
        let o = self.cmd(&[], args);
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn tui(&self, keys: &str) -> String {
        let o = self.cmd(&[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", keys), ("THC_TUI_SNAPSHOT_WRITE", "1")], &["tui"]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn node(&self, q: &str) -> Value {
        let v: Value = serde_json::from_str(&self.cli(&["--json", "q", &format!("{q} (status:any or status:none)")])).unwrap();
        v["items"][0].clone()
    }
}

impl Drop for V {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn exact_matching_chords_never_run_letter_actions() {
    let v = V::new("exact");
    v.cli(&["todo", "Pay rent", "--due", "fri"]);
    // ⌥X and ⌃X on a task in Tasks: nothing. x: done.
    v.tui("3<m-x><c-x>");
    assert_eq!(v.node("text:rent")["status"], "todo");
    v.tui("3x");
    assert_eq!(v.node("text:rent")["status"], "done");
}

#[test]
fn prefixes_finish_cancel_and_redispatch() {
    let v = V::new("prefix");
    v.cli(&["todo", "Pay rent", "--due", "fri"]);
    // (THC_NOW is a Saturday: rent, due fri, sorts first; milk is due later.)
    v.cli(&["todo", "Buy milk", "--due", "+10d"]);
    // p h: high priority on the selected (first) task.
    v.tui("3ph");
    assert_eq!(v.node("text:rent")["priority"], "high");
    // S w: waiting.
    v.tui("3Sw");
    assert_eq!(v.node("text:rent")["status"], "waiting");
    // p then ↓: the prefix cancels and ↓ moves the cursor; then p l sets the second one low.
    v.tui("3p<down>pl");
    assert_eq!(v.node("text:milk")["priority"], "low");
    assert_eq!(v.node("text:rent")["priority"], "high", "the first one untouched");
    // Esc only cancels: no back, no priority.
    let f = v.tui("3p<esc>");
    assert!(f.contains("status:open"), "still Tasks: {f}");
    // The pending prefix says what can follow.
    let f = v.tui("3p");
    let bar = f.lines().last().unwrap();
    assert!(bar.trim_start().starts_with("p…") && bar.trim_end().ends_with("h high  m med  l low  - none  Esc cancel"), "{f}");
    // Its hints are buttons: clicking `l low` finishes the sequence.
    let x = bar.chars().count() - "l low  - none  Esc cancel".chars().count() + 1;
    v.tui(&format!("3p<click:{x},23>"));
    assert_eq!(v.node("text:rent")["priority"], "low");
}

#[test]
fn tasks_sorts_on_comma_and_s_is_always_scheduled() {
    let v = V::new("sort");
    v.cli(&["todo", "Pay rent", "--due", "fri"]);
    let f = v.tui("3,");
    assert!(!f.contains("sort:due") || f.contains("sort:"), "{f}");
    let before = v.tui("3");
    let after = v.tui("3,");
    assert_ne!(before.lines().nth(2), after.lines().nth(2), "the sort changed the query line");
    let f = v.tui("3s");
    assert!(f.lines().last().unwrap().contains("sched"), "s schedules in Tasks too: {f}");
}

#[test]
fn q_goes_back_before_it_quits() {
    let v = V::new("q");
    v.cli(&["todo", "Pay rent", "--due", "fri", "-t", "home"]);
    // A filtered Tasks: q puts the filter back (and doesn't quit).
    let f = v.tui("3f#home<cr>q");
    assert!(f.contains("status:open sort:due"), "{f}");
}

#[test]
fn the_palette_runs_actions_and_never_types_into_write() {
    let v = V::new("palette");
    // In today's journal, write a line, then the palette's Done: the line completes (as a
    // task it was), nothing typed.
    v.tui("5[ ] call mum<m-:>Done<cr>");
    let n = v.node("text:mum");
    assert_eq!(n["status"], "done", "{n}");
    assert_eq!(n["text"], "call mum");
    // Set priority from the palette over Write: the prefix waits for h.
    // (the day opens on a fresh line: ↑ to the task)
    v.tui("5<up><m-:>Set priority<cr>h");
    assert_eq!(v.node("text:mum")["priority"], "high");
    assert_eq!(v.node("text:mum")["text"], "call mum", "h didn't type");
}

#[test]
fn write_is_sealed_and_reads_the_table() {
    let v = V::new("write");
    // ⌃T cycles, ⌥X does nothing (says so once), Esc saves and goes to Today.
    let f = v.tui("5call<c-t><m-x>");
    assert!(f.contains("[ ] call") && f.contains("⌥X isn't a writing key"), "{f}");
    let f = v.tui("5call<esc>");
    assert!(f.contains("Nothing due") || f.contains("Next 7 days") || f.contains("Today"), "{f}");
    assert_eq!(v.node("text:call")["text"], "call");
}

/// A scratch daemon for the vault (the list footer shows with `● live`), stopped on drop.
struct Daemon(std::process::Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl V {
    fn daemon(&self) -> Daemon {
        let mut c = common::thc();
        c.args(["daemon", "run"]).current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env("THC_NOW", "2026-10-03T09:00").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let d = Daemon(c.spawn().unwrap());
        for _ in 0..50 {
            if self.cmd(&[], &["daemon", "status"]).status.success() {
                return d;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("the scratch daemon didn't come up");
    }

    fn bar(&self, size: &str, keys: &str) -> String {
        let o = self.cmd(&[("THC_TUI_SNAPSHOT", size), ("THC_TUI_KEYS", keys)], &["tui"]);
        let last = String::from_utf8_lossy(&o.stdout).lines().last().unwrap_or_default().trim_end().to_string();
        // (The version at the right edge, keymap.md §6, isn't a hint.)
        match last.rfind("   thc ") {
            Some(i) => last[..i].trim_end().to_string(),
            None => last,
        }
    }
}

#[test]
fn the_footer_is_generated_from_the_table() {
    let v = V::new("footer");
    v.cli(&["todo", "Pay rent", "--due", "fri"]);
    v.cli(&["page", "new", "Lisbon"]);
    // Write (writing.md §4): the same copy, now from the table's ranks; a page has no day keys;
    // at 80 `F1 keys` goes first.
    assert!(v.bar("120x24", "5").ends_with("⌃T task  ⌃O open  ⌃P ⌃N day  Esc done  F1 keys"));
    assert!(v.bar("120x24", "4<cr>").ends_with("⌃T task  ⌃O open  Esc done  F1 keys"));
    assert!(v.bar("80x24", "5").ends_with("⌃T task  ⌃O open  ⌃P ⌃N day  Esc done"));
    // Overlays and prompts.
    assert!(v.bar("120x24", "<c-o>").ends_with("↑↓ choose  Enter go  Esc close"));
    assert!(v.bar("120x24", "3d").ends_with("Enter save  Esc cancel"));
    assert!(v.bar("120x24", "3m").ends_with("Enter move  1 2 3 recent  Esc"));
    // Lists (keymap.md §6): the view's ranked keys and `? keys`; `x done` only on a task row.
    let _d = v.daemon();
    assert!(v.bar("120x24", "1").ends_with("● live   x done  a add  w agenda  space leader  ? keys"), "{}", v.bar("120x24", "1"));
    assert!(v.bar("120x24", "3").ends_with("f filter  , sort  x done  space leader  ? keys"));
    assert!(v.bar("120x24", "2").ends_with("● live   ? keys"), "an empty inbox: no node keys");
    assert!(v.bar("120x24", "7r").ends_with("a accept  u undo  A accept all  r all changes  ? keys") || v.bar("120x24", "7r").ends_with("r all changes  ? keys"), "{}", v.bar("120x24", "7r"));
    // A footer hint is a button for its action: `w agenda` toggles the agenda.
    let bar = v.bar("120x24", "1");
    let x = bar.chars().count() - "w agenda  space leader  ? keys".chars().count() + 1;
    let o = v.cmd(&[("THC_TUI_SNAPSHOT", "120x24"), ("THC_TUI_KEYS", &format!("1<click:{x},23>"))], &["tui"]);
    assert!(String::from_utf8_lossy(&o.stdout).lines().last().unwrap_or_default().trim_start().starts_with("agenda · 7 days"), "{}", String::from_utf8_lossy(&o.stdout));
}

#[test]
fn help_palette_and_thc_keys_read_the_table() {
    let v = V::new("help");
    v.cli(&["todo", "Pay rent", "--due", "fri"]);
    // Help in a document: writing.md's words on the table's keys, the Typing rows after.
    let f = v.tui("5<f1>");
    assert!(f.contains("⌃T         text → [ ] → [x] → text") && f.contains("Typing") && f.contains("⌥1–⌥7      views"), "{f}");
    // Help in a list: a prefix folds to one row; U stays undo (no redo yet).
    let f = v.tui("1??");
    assert!(f.contains("S          status: space / w x -") && f.contains("p          priority: h m l -"), "{f}");
    assert!(!f.contains("redo"), "no U redo until vault redo exists: {f}");
    // The palette shows each action's current key from the keymap.
    let f = v.tui("1:Done");
    assert!(f.lines().any(|l| l.contains("Done") && l.contains("thc done <id>") && l.trim_end().trim_end_matches('│').trim_end().ends_with('x')), "{f}");
    // thc keys: the table, no conflicts, and the guide's tables generated from it.
    let j: Value = serde_json::from_str(&v.cli(&["--json", "keys"])).unwrap();
    assert!(j["bindings"].as_array().unwrap().iter().any(|b| b["keys"] == "C-t" && b["action"] == "doc.task_cycle" && b["context"] == "write"));
    assert_eq!(j["conflicts"], serde_json::json!([]));
    assert_eq!(v.cli(&["keys", "--conflicts"]).trim(), "no conflicts");
    let md = v.cli(&["keys", "--markdown"]);
    let page = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/guide/keys.md")).unwrap();
    let a = "<!-- keys:begin (generated: scripts/keys-guide.sh) -->\n";
    let block = &page[page.find(a).unwrap() + a.len()..page.find("<!-- keys:end -->").unwrap()];
    assert_eq!(block.trim_end(), md.trim_end(), "docs/guide/keys.md is stale: run scripts/keys-guide.sh");
}

#[test]
fn help_scrolls_when_it_doesnt_fit_and_undo_is_a_writing_key() {
    let v = V::new("helpscroll");
    // At 100x30 the document help fits (with the ⌘ setup hint on a Mac), undo with the writing
    // keys.
    let o = v.cmd(&[("THC_TUI_SNAPSHOT", "100x30"), ("THC_TUI_KEYS", "5<f1>")], &["tui"]);
    let f = String::from_utf8_lossy(&o.stdout).to_string();
    // On a Mac, ⌘ first beside its twin (keymap.md §7.0).
    let undo = f.lines().position(|l| l.contains("⌘Z ⌃Z ⇧⌘Z ⌃Y undo · redo") || l.contains("⌃Z ⌃Y      undo · redo")).expect("undo in help");
    let writing = f.lines().position(|l| l.contains("Writing")).unwrap();
    let typing = f.lines().position(|l| l.contains("Typing")).unwrap();
    assert!(writing < undo && undo < typing, "undo sits in Writing: {f}");
    assert!(!f.contains("↓ more"), "it fits: {f}");
    // Every key overflows: the border says ↓ more; scrolled to the end, it doesn't.
    let o = v.cmd(&[("THC_TUI_SNAPSHOT", "100x26"), ("THC_TUI_KEYS", "1???")], &["tui"]);
    let f = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(f.contains("↓ more · Esc"), "{f}");
    let downs = "<down>".repeat(60);
    let o = v.cmd(&[("THC_TUI_SNAPSHOT", "100x26"), ("THC_TUI_KEYS", &format!("1???{downs}"))], &["tui"]);
    let g = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(!g.contains("↓ more") && g.contains("every key") && g.lines().nth(3) != f.lines().nth(3), "scrolled to the end: {g}");
}

#[test]
fn alt_a_selects_all_in_write() {
    let v = V::new("selall");
    // ⌥A then typing replaces the whole day's text written here.
    v.tui("5first<cr><cr>second<m-a>gone<esc>");
    let j: Value = serde_json::from_str(&v.cli(&["--json", "q", "journal=today"])).unwrap();
    let texts: Vec<&str> = j["items"].as_array().unwrap().iter().filter_map(|n| n["text"].as_str()).collect();
    assert_eq!(texts, ["gone"], "{j}");
}

#[test]
fn the_space_leader_and_which_key() {
    let v = V::new("leader");
    v.cli(&["todo", "Pay rent", "--due", "fri"]);
    // Space: the popup at once, groups with +, and the breadcrumb footer.
    let f = v.tui("1<space>");
    assert!(f.contains("─ space ─") && f.contains("f  +find") && f.contains("g  +go") && f.contains("space  commands"), "{f}");
    // The popup carries the keys; the footer only the breadcrumb and how to leave (no repeat).
    let bar = f.lines().last().unwrap();
    assert!(bar.trim_start().starts_with("space…") && bar.trim_end().ends_with("⌫ back  Esc cancel") && !bar.contains("f find"), "{f}");
    // Two levels, then a leaf: space g k goes to Tasks.
    let f = v.tui("1<space>g");
    assert!(f.contains("─ space g ─") && f.contains("k  Tasks") && f.lines().last().unwrap().contains("space g…"), "{f}");
    assert!(f.contains("E  $EDITOR page") || v.tui("1<space>").contains("E  $EDITOR page"));
    let f = v.tui("1<space>gk");
    assert!(f.contains("status:open") && !f.contains("─ space"), "{f}");
    // Esc steps back a level; another Esc closes; an unbound key closes and says so.
    let f = v.tui("1<space>g<esc>");
    assert!(f.contains("─ space ─") && !f.contains("─ space g ─"), "{f}");
    let f = v.tui("1<space>g<esc><esc>");
    assert!(!f.contains("─ space"), "{f}");
    let f = v.tui("1<space>x");
    assert!(f.contains("space x isn't bound") && !f.contains("─ space"), "{f}");
    // Other prefixes wait 200 ms for their popup (none in a snapshot); the footer shows at once.
    let f = v.tui("1g");
    assert!(!f.contains("─ g ─") && f.lines().last().unwrap().contains("g top  d go to date"), "{f}");
    // leader_popup = off: the breadcrumb only.
    let o = v.cmd(&[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", "1<space>"), ("THC_TUI_LEADER_POPUP", "off")], &["tui"]);
    let f = String::from_utf8_lossy(&o.stdout);
    assert!(!f.contains("─ space ─") && f.lines().last().unwrap().contains("space…") && f.lines().last().unwrap().contains("f find  g go  n new"), "the popup off: the footer lists them: {f}");
    // A popup entry is a button: clicking `+go` opens that level.
    let f = v.tui("1<space>");
    let (y, line) = f.lines().enumerate().find(|(_, l)| l.contains("g  +go")).unwrap();
    let x = line.chars().take_while(|_| true).collect::<String>().find("g  +go").map(|b| line[..b].chars().count()).unwrap();
    let f = v.tui(&format!("1<space><click:{x},{y}>"));
    assert!(f.contains("─ space g ─"), "{f}");
    // Every key lists the tree, a group per row; plain help has the leader on space.
    let f = v.cmd(&[("THC_TUI_SNAPSHOT", "100x60"), ("THC_TUI_KEYS", "1???")], &["tui"]);
    let f = String::from_utf8_lossy(&f.stdout);
    assert!(f.contains("space g    go: t i k p v h j s l d g"), "{f}");
    let f = v.tui("1??");
    assert!(f.contains("space      leader") && !f.contains("space g"), "{f}");
    // Nothing in documents: space types.
    v.tui("5ab cd<esc>");
    assert_eq!(v.node("text:cd")["text"], "ab cd");
}

#[test]
fn remaps_from_the_config() {
    let v = V::new("remap");
    v.cli(&["todo", "Pay rent", "--due", "fri"]);
    let cfg = v.root.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), "[keys.list]\n\"x\" = \"node.history\"\n\"C-x\" = \"node.done\"\n[keys.write]\n\"q\" = \"doc.task_cycle\"\n").unwrap();
    let c = cfg.to_str().unwrap();
    let run = |keys: &str| {
        let o = v.cmd(&[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", keys), ("THC_TUI_SNAPSHOT_WRITE", "1"), ("THC_CONFIG_DIR", c)], &["tui"]);
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    // The refused one (a printable key in write) shows in the bar at start, with how to see more.
    let f = run("");
    assert!(f.lines().last().unwrap().contains("keys: 1 problem · thc keys --conflicts"), "{f}");
    // It holds through keys (5 s, keymap.md §8.3), unlike an error toast.
    let f = run("1jk");
    assert!(f.lines().last().unwrap().contains("keys: 1 problem"), "{f}");
    // ⌃X now completes; x no longer does (it opens the history).
    run("3x");
    assert_eq!(v.node("text:rent")["status"], "todo");
    run("3<c-x>");
    assert_eq!(v.node("text:rent")["status"], "done");
    // thc keys shows the remap and lists the refusal (exit 6).
    let o = v.cmd(&[("THC_CONFIG_DIR", c)], &["keys", "--conflicts"]);
    assert_eq!(o.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&o.stdout).contains("write can't bind"));
    let o = v.cmd(&[("THC_CONFIG_DIR", c)], &["--json", "keys"]);
    let j: Value = serde_json::from_str(&String::from_utf8_lossy(&o.stdout)).unwrap();
    assert!(j["bindings"].as_array().unwrap().iter().any(|b| b["context"] == "list" && b["keys"] == "C-x" && b["action"] == "node.done"));
    // Help shows the remapped key.
    let f = run("3??");
    assert!(f.contains("⌃X•"), "{f}");
    assert!(f.contains("remap any key: :remap · thc keys --edit"), "{f}");
}

#[test]
fn help_marks_an_added_key_even_when_the_default_is_still_bound() {
    let v = V::new("remap-alias");
    let cfg = v.root.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), "[keys.list]\n\"C-x\" = \"node.done\"\n").unwrap();
    let o = v.cmd(&[("THC_CONFIG_DIR", cfg.to_str().unwrap()), ("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", "3??")], &["tui"]);
    assert!(o.status.success());
    let frame = String::from_utf8_lossy(&o.stdout);
    assert!(frame.contains("⌃X•"), "{frame}");
}

#[test]
fn editor_polish() {
    let v = V::new("m1polish");
    v.cli(&["todo", "Pay rent", "--due", "fri"]);
    let snap = |size: &str, keys: &str| {
        let o = v.cmd(&[("THC_TUI_SNAPSHOT", size), ("THC_TUI_KEYS", keys), ("THC_TUI_SNAPSHOT_CURSOR", "1")], &["tui"]);
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    // The caret doesn't show through help or the leader's panel; it's back when they close, and an
    // overlay that takes text keeps its own.
    assert_eq!(snap("100x26", "5call").matches('▮').count(), 1);
    assert_eq!(snap("100x26", "5call<f1>").matches('▮').count(), 0);
    assert_eq!(snap("100x26", "5call<f1><esc>").matches('▮').count(), 1);
    assert_eq!(snap("100x26", "1<space>").matches('▮').count(), 0);
    assert_eq!(snap("100x26", "5call<m-:>").matches('▮').count(), 1);
    // Help under 100 columns: one column, the words whole.
    let f = snap("80x24", "5<f1>");
    assert!(f.contains("new line · twice: new note") && !f.lines().any(|l| l.contains('│') && l.contains('…')), "{f}");
    // Search: the is:deleted tip only with no results.
    let f = snap("100x24", "6rent<cr>");
    assert!(f.contains("Pay rent") && !f.contains("deleted nodes are searchable"), "{f}");
    let f = snap("100x24", "6zzzz<cr>");
    assert!(f.contains("is:deleted"), "{f}");
}

/// keymap.md §7.0: on a Mac, help shows ⌘ first beside its twin (`⌘C ⌃C`); when
/// ⌘ isn't known to reach thc it says how; `thc keys --markdown` lists ⌘ first.
#[test]
fn mac_keys_come_first() {
    let root = thc_core::scratch::dir(&format!("thc-mackeys-help-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let run = |env: &[(&str, &str)], args: &[&str]| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env("THC_TUI_MAC", "1");
        for (k, v) in env {
            c.env(k, v);
        }
        String::from_utf8_lossy(&c.output().unwrap().stdout).to_string()
    };
    run(&[], &["init", "v"]);
    let help = run(&[("THC_TUI_SNAPSHOT", "130x50"), ("THC_TUI_KEYS", "<f1>")], &["j", "--no-focus"]);
    assert!(help.contains("⌘C ⌃C") && help.contains("⌘V ⌃V") && help.contains("⌘Z ⌃Z"), "{help}");
    assert!(help.contains("⌘ keys: thc setup wezterm"), "not known: the hint: {help}");
    let known = run(&[("THC_TUI_SNAPSHOT", "130x50"), ("THC_TUI_KEYS", "<f1>"), ("THC_TUI_CMD_SEEN", "1")], &["j", "--no-focus"]);
    assert!(known.contains("⌘C ⌃C") && !known.contains("thc setup wezterm"), "known: no hint: {known}");
    let md = run(&[], &["keys", "--markdown"]);
    let copy = md.lines().find(|l| l.contains("`clip.copy`")).unwrap();
    assert!(copy.starts_with("| `⌘C` `⌃C`"), "{copy}");
    let _ = std::fs::remove_dir_all(&root);
}
