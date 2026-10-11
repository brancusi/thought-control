//! What thc's fuzzing is made of (crates/thc-screen/tests/fuzz.rs): the generated vaults, thc's
//! keys and combos, an agent's writes, and how a round trip's difference is told. A module of
//! its own so another fuzz layer (a pty one, on `fictty::sim`) can include it.

#![allow(dead_code)]

use chrono::{NaiveDate, NaiveDateTime};
use fictty::fuzz::Config;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thc_core::builder::TxBuilder;
use thc_core::event::Actor;
use thc_core::vault::{self, Paths, Vault};

pub fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 11).unwrap()
}

pub fn now() -> NaiveDateTime {
    today().and_hms_opt(9, 0, 0).unwrap()
}

pub fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// splitmix64, for the vaults: a case's vault comes from its seed.
pub struct Rng(u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

// ---- the vault ------------------------------------------------------------------------------------

/// A note as generated: its kind, its words, a link to another page, its children.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoteSpec {
    pub kind: Kind,
    pub words: Vec<u8>,
    pub link: Option<u8>,
    pub children: Vec<NoteSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    Bullet,
    Para,
    Todo,
    Done,
    /// A numbered item (a run of them starts `1.`; their children are lettered).
    Numbered,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultSpec {
    pub pages: Vec<Vec<NoteSpec>>,
    /// Where the screen starts: a page (its index) or Today.
    pub start_page: Option<u8>,
    /// The screen's size: 140x40, 120x32 or 100x30.
    pub size: u8,
}

pub const WORDS: &[&str] = &["alpha", "beta", "gamma", "delta", "milk", "eggs", "plan", "call", "the", "bank", "garden", "tent", "fuel", "notes", "x"];
pub const TITLES: &[&str] = &["Garden", "Health", "Q4 Plan", "Trip", "Reading"];

pub fn gen_note(r: &mut Rng, depth: usize) -> NoteSpec {
    let kind = match r.below(11) {
        0..=3 => Kind::Bullet,
        4..=5 => Kind::Para,
        6..=7 => Kind::Todo,
        8 if depth == 0 => Kind::Done,
        _ => Kind::Numbered,
    };
    let words = (0..1 + r.below(4)).map(|_| r.below(WORDS.len() as u64) as u8).collect();
    let link = (r.below(5) == 0).then(|| r.below(4) as u8);
    let children = if depth < 3 { (0..r.below(3 - depth as u64)).map(|_| gen_note(r, depth + 1)).collect() } else { vec![] };
    NoteSpec { kind, words, link, children }
}

pub fn gen_vault(seed: u64) -> VaultSpec {
    let mut r = Rng(seed ^ 0x5eed);
    let pages = (0..1 + r.below(3)).map(|_| (0..r.below(5)).map(|_| gen_note(&mut r, 0)).collect()).collect();
    let start_page = (r.below(10) < 7).then(|| r.below(3) as u8);
    VaultSpec { pages, start_page, size: r.below(3) as u8 }
}

impl VaultSpec {
    pub fn size(&self) -> (u16, u16) {
        [(140, 40), (120, 32), (100, 30)][self.size as usize % 3]
    }
}

pub fn text_of(n: &NoteSpec, titles: &[&str]) -> String {
    let mut t: Vec<&str> = n.words.iter().map(|w| WORDS[*w as usize % WORDS.len()]).collect();
    let link = n.link.map(|l| format!("see [[{}]]", titles[l as usize % titles.len()]));
    if let Some(l) = &link {
        t.push(l);
    }
    t.join(" ")
}

/// A scratch vault holding the spec's pages (the case's template). Returns the root and the
/// pages' ids.
pub fn build(spec: &VaultSpec) -> (PathBuf, Vec<String>) {
    // One device name, a pinned clock, and log and node ids derived from it (THC_FIXTURE_IDS),
    // for every vault here and the screens on them: fictty's replay compares the state and the
    // frame, which hold them (the Log's rows). Set before any vault opens, in this test binary
    // only (a `thc screen` it spawns inherits them).
    static DEVICE: std::sync::Once = std::sync::Once::new();
    // SAFETY: the first vault isn't opened until this has run; nothing reads the env meanwhile.
    DEVICE.call_once(|| unsafe {
        std::env::set_var("THC_DEVICE", "fuzz");
        std::env::set_var("THC_NOW", "2026-10-11T09:00");
        std::env::set_var("THC_FIXTURE_IDS", "1");
    });
    let root = scratch();
    vault::init(&root.join("v"), None, None).unwrap();
    let mut v = open(&root, human());
    let titles: Vec<&str> = TITLES.iter().take(spec.pages.len()).copied().collect();
    let ids = v
        .transact(|s| {
            let mut b = TxBuilder::new(s, today());
            let mut ids = Vec::new();
            for t in &titles {
                ids.push(b.create_page(t, &[])?);
            }
            fn add(b: &mut TxBuilder, parent: &str, notes: &[NoteSpec], titles: &[&str], in_numbered: bool) -> anyhow::Result<()> {
                let mut prev_numbered = false;
                for n in notes {
                    let mut props = serde_json::Map::new();
                    match n.kind {
                        Kind::Para => {
                            props.insert("style".into(), "para".into());
                        }
                        Kind::Todo => {
                            props.insert("status".into(), "todo".into());
                        }
                        Kind::Done => {
                            props.insert("status".into(), "done".into());
                        }
                        // The first of a run carries the run's style; under a numbered item,
                        // letters are the default.
                        Kind::Numbered if !prev_numbered && !in_numbered => {
                            props.insert("list".into(), "1.".into());
                        }
                        _ => {}
                    }
                    let id = b.create(Some(parent.to_string()), &text_of(n, titles), None, props, &[], None)?;
                    add(b, &id, &n.children, titles, n.kind == Kind::Numbered)?;
                    prev_numbered = n.kind == Kind::Numbered;
                }
                Ok(())
            }
            for (p, notes) in ids.iter().zip(&spec.pages) {
                add(&mut b, p, notes, &titles, false)?;
            }
            Ok((b.finish()?, ids))
        })
        .unwrap()
        .1;
    (root, ids)
}

pub fn scratch() -> PathBuf {
    thc_core::scratch::dir(&format!("thc-fuzz-{}", thc_core::id::new_id()))
}

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (a, b) = (e.path(), to.join(e.file_name()));
        if e.file_type().unwrap().is_dir() { copy_dir(&a, &b) } else { std::fs::copy(&a, &b).map(|_| ()).unwrap() }
    }
}

pub fn open(root: &Path, actor: Actor) -> Vault {
    Vault::open(Paths { vault: root.join("v"), cache: root.join("c") }, actor, "fuzz").unwrap()
}

pub fn human() -> Actor {
    Actor { kind: "human".into(), name: None }
}

// ---- what a person (or an agent) does -------------------------------------------------------------

/// thc's keys, on top of fictty's alphabet: motions and their ⇧ forms, the Mac set and its ⌃/⌥
/// twins, edits, the clipboard, undo, views, the list and sidebar keys.
pub const KEYS: &[&str] = &[
    "a-left", "a-right", "d-left", "d-right", "d-up", "d-down", "c-a", "c-e", "a-b", "a-f", "a-s-left", "a-s-right", "d-s-left", "d-s-right", "d-s-up", "d-s-down", "s-home",
    "s-end", "a-del", "d-del", "c-u", "c-t", "a-l", "a-a", "c-y", "2", "3", "4", "5", "6", "7", "j", "k", "X", "w", "\\", "~", "T", "o", "a-s", "a-j", "a-k", "a-c", "a-w", "a-p",
    "a-s-t", "a-\\", "f", "/", "u", "U",
];

/// Keys that go in together: capture syntax typed out, opening a page, ⌘A ⌫ ⌘Z (EI12).
pub fn combos() -> Vec<Vec<String>> {
    let typed = |t: &str| t.chars().map(|c| if c == ' ' { "space".to_string() } else { c.to_string() }).collect::<Vec<_>>();
    let mut out: Vec<Vec<String>> = ["1. ", "- ", "[ ] ", "[[Garden]] ", "#tag ", "due:fri ", "word ", "x y"].iter().map(|t| typed(t)).collect();
    for c in [&["d-a", "bs", "d-z"][..], &["4", "enter"], &["4", "down", "enter"], &["4", "down", "down", "enter"], &["esc", "4", "enter"], &["d-x", "d-v"], &["d-c", "d-v"], &["d-z", "d-s-z"]] {
        out.push(c.iter().map(|s| s.to_string()).collect());
    }
    out
}

pub const PASTES: &[&str] = &["pasted", "- a\n- b", "1. one\n2. two", "line one\nline two", "[[Health]]"];

/// An agent's write from outside, as a host act (`host "agent:add:<page>:<word>"`,
/// `agent:edit:<pick>:<word>`, `agent:delete:<pick>`, `agent:done:<pick>`): the screen's
/// `on_host_act` handler writes it to the vault (`agent_write_vault`) and tells the host. A pick is
/// a note in outline order (modulo how many there are), so a replay picks the same note.
pub fn agent_acts() -> Vec<String> {
    let mut out = Vec::new();
    for p in 0..3 {
        out.push(format!("agent:add:{p}:{p}"));
    }
    for k in [0, 3, 7, 11] {
        out.push(format!("agent:edit:{k}:{}", k % 5));
        out.push(format!("agent:delete:{k}"));
        out.push(format!("agent:done:{k}"));
    }
    out
}

/// fictty's fuzz config with thc's keys, combos and pastes; with `agent`, about one act in
/// twelve is an agent's write.
pub fn config(seed: u64, agent: bool, replay: bool) -> Config {
    let d = Config::default();
    let mut keys = d.keys.clone();
    keys.extend(KEYS.iter().map(|k| k.to_string()));
    let mut pastes = d.pastes.clone();
    pastes.extend(PASTES.iter().map(|p| p.to_string()));
    Config {
        seed,
        runs: 1,
        steps: env_num("THC_FUZZ_STEPS", 40) as usize,
        budget: Duration::from_millis(env_num("THC_FUZZ_BUDGET_MS", 2000)),
        keys,
        combos: combos(),
        pastes,
        host_acts: if agent { agent_acts() } else { vec![] },
        host: if agent { 8 } else { 0 },
        // The same acts on a fresh screen: the same state and frame. Node ids are derived under
        // THC_FIXTURE_IDS (set in `build`), so a copy of the vault makes the same ones.
        replay,
        terminal: env_num("THC_FUZZ_TERMINAL", 1) == 1,
        // Shrinking keeps to the failure's class (a round trip's kind), not just its property.
        class: Some(class),
        ..d
    }
}

/// The notes under the pages in outline order (pages in order, depth first), with their text
/// (a task's status before it).
pub fn notes_in_order(root: &Path, pages: &[String]) -> Vec<(String, String)> {
    let v = open(root, human());
    let mut out = Vec::new();
    pub fn walk(v: &Vault, id: &str, out: &mut Vec<(String, String)>) {
        for n in v.store.children(id).unwrap_or_default() {
            // A task and a note with the same words are different lines.
            out.push((n.id.clone(), format!("{}{}", n.status.as_deref().map(|s| format!("[{s}] ")).unwrap_or_default(), n.text)));
            walk(v, &n.id, out);
        }
    }
    for p in pages {
        walk(&v, p, &mut out);
    }
    out
}

/// Do an agent act (`agent:add:<page>:<word>`, `agent:edit:<pick>:<word>`,
/// `agent:delete:<pick>`, `agent:done:<pick>`) to the vault at `root`, as an agent.
pub fn agent_write_vault(root: &Path, pages: &[String], act: &str) {
    let parts: Vec<&str> = act.split(':').collect();
    let num = |i: usize| parts.get(i).and_then(|p| p.parse::<usize>().ok()).unwrap_or(0);
    let notes: Vec<String> = notes_in_order(root, pages).into_iter().map(|(id, _)| id).collect();
    let pick = (!notes.is_empty()).then(|| notes[num(2) % notes.len()].clone());
    let mut a = open(root, Actor { kind: "agent".into(), name: Some("claude".into()) });
    let _ = a.transact(|s| {
        let mut b = TxBuilder::new(s, today());
        match (parts.get(1).copied(), pick) {
            (Some("add"), _) => {
                let cap = thc_core::capture::parse(&format!("agent {}", WORDS[num(3) % WORDS.len()]), today()).unwrap();
                b.create_from_capture(Some(pages[num(2) % pages.len()].clone()), &cap, None)?;
            }
            (Some("edit"), Some(id)) => b.set_text(&id, &format!("edited {}", WORDS[num(3) % WORDS.len()]))?,
            (Some("delete"), Some(id)) => {
                b.delete(&id)?;
            }
            (Some("done"), Some(id)) => {
                let _ = b.complete(&id);
            }
            _ => {}
        }
        Ok((b.finish()?, ()))
    });
}

/// What kind of difference a round trip made (so a known one can be matched by its kind).
pub fn round_trip_kind(held: &str, saved: &str) -> &'static str {
    let h: Vec<&str> = held.lines().collect();
    let v: Vec<&str> = saved.lines().collect();
    // An agent's note (text `agent …`) the editor shows and the vault no longer has, or the
    // other way round: an outside change (or its undo) the editor didn't take in.
    let agent = |a: &str, b: &str| {
        a.lines().any(|l| {
            l.find("agent ").is_some_and(|i| {
                let note: String = l[i..].split_whitespace().take(2).collect::<Vec<_>>().join(" ");
                !b.contains(&note)
            })
        })
    };
    if agent(held, saved) || agent(saved, held) {
        return "agent-stale";
    }
    // Only a box differs (`[ ]` ↔ `[x]`): a task done or reopened elsewhere, not taken in.
    if h.len() == v.len() && h.iter().zip(&v).all(|(a, b)| a == b || a.replace("[x]", "[ ]") == b.replace("[x]", "[ ]")) {
        return "box-stale";
    }
    // A capture token is read on save (`due:fri` → `due:2026-10-16`, a task), even half typed
    // (`due:f`).
    if held.split_whitespace().any(|w| w.starts_with("due:") && w.len() > 4 && !saved.split_whitespace().any(|s| s == w)) {
        return "token";
    }
    // Fewer lines, the same words: a line break (and the next line's indent) became a space.
    let words = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
    if v.len() < h.len() && words(held) == words(saved) && saved.lines().all(|l| held.contains(l.trim_end()) || l.contains(' ')) {
        return "lines-joined";
    }
    let squeeze = |t: &str| {
        let mut out: Vec<&str> = Vec::new();
        for l in t.lines() {
            if !(l.trim().is_empty() && out.last().is_some_and(|p: &&str| p.trim().is_empty())) {
                out.push(l);
            }
        }
        out.join("\n").trim_start_matches('\n').to_string()
    };
    if squeeze(held) == squeeze(saved) {
        return "blank-lines";
    }
    // A line left indented under nothing (its parent line deleted, or blank): it saves out a level.
    let flat = |t: &str| squeeze(t).lines().map(str::trim_start).collect::<Vec<_>>().join("\n");
    if flat(held) == flat(saved) && squeeze(held).lines().next().is_some_and(|l| l.starts_with(' ')) {
        return "orphan-indent";
    }
    let strip = |l: &str| -> String {
        let t = l.trim_start();
        // One marker off the front: a box, a dash, or a number or letter and a dot.
        for m in ["- [ ] ", "- [x] ", "- ", "[ ] ", "[x] "] {
            if let Some(rest) = t.strip_prefix(m) {
                return rest.to_string();
            }
        }
        let t = match t.split_once(". ") {
            Some((m, rest)) if !m.is_empty() && m.len() <= 4 && m.chars().all(|c| c.is_ascii_alphanumeric()) => rest,
            _ => t,
        };
        t.to_string()
    };
    if h.len() == v.len() {
        if h.iter().zip(&v).all(|(a, b)| a.trim_start() == b.trim_start()) {
            return "indent";
        }
        // A marker typed in front of another (`1. - alpha`, `1. - [ ] alpha`): the inner one is
        // dropped.
        let all = |l: &str| {
            let mut t = l.to_string();
            loop {
                let n = strip(&t);
                if n == t {
                    return t;
                }
                t = n;
            }
        };
        let markers = |l: &str| {
            let (mut t, mut n) = (l.to_string(), 0);
            loop {
                let m = strip(&t);
                if m == t || n > 4 {
                    return n;
                }
                t = m;
                n += 1;
            }
        };
        if h.iter().zip(&v).all(|(a, b)| a == b || (markers(a) >= 2 && all(a) == all(b))) && h.iter().any(|a| markers(a) >= 2) {
            return "marker:nested";
        }
        if h.iter().zip(&v).all(|(a, b)| strip(a) == strip(b)) {
            // Which marker changed: a task's `- ` added (`[ ] x` → `- [ ] x`), a numbered or
            // lettered style (`1.` ↔ `a.` ↔ `-`), or something else.
            let diffs: Vec<(&&str, &&str)> = h.iter().zip(&v).filter(|(a, b)| a != b).collect();
            if diffs.iter().all(|(a, b)| b.trim_start() == format!("- {}", a.trim_start())) {
                return "marker:dash-added";
            }
            let ordered = |l: &str| l.trim_start().split_once(". ").is_some_and(|(m, _)| !m.is_empty() && m.len() <= 4 && m.chars().all(|c| c.is_ascii_alphanumeric()));
            if diffs.iter().any(|(a, b)| ordered(a) || ordered(b)) {
                return "marker:list-style";
            }
            return "marker";
        }
        let mut a: Vec<String> = h.iter().map(|l| strip(l)).collect();
        let mut b: Vec<String> = v.iter().map(|l| strip(l)).collect();
        a.sort();
        b.sort();
        if a == b {
            return "reordered";
        }
    }
    if v.len() < h.len() {
        return "line-dropped";
    }
    if v.len() > h.len() {
        return "line-added";
    }
    "other"
}

/// A document's text as compared: lines without trailing spaces, without blank lines or bare
/// list markers at the end (a line being written isn't a note yet).
pub fn norm(t: &str) -> String {
    let mut lines: Vec<&str> = t.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|l| l.is_empty() || ["-", "- [ ]", "1.", "a.", "i."].contains(l)) {
        lines.pop();
    }
    lines.join("\n")
}

pub fn class(property: &str, message: &str) -> String {
    match message.find(']') {
        Some(i) if message.starts_with("round trip [") => message[..=i].to_string(),
        _ => format!("{property}: {}", message.split([':', '\n']).next().unwrap_or("")),
    }
}

