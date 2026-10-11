//! thc screen under fictty's fuzz driver (`fictty::fuzz`), on Headless. fictty generates the use
//! (keys, the mouse at real cells, pastes, resizes, waits, host acts), shrinks a failure and
//! checks what any screen must hold: no panic, each act inside a time budget, the same acts
//! replaying the same (state and frame), the terminal (escapes into an emulator) showing the
//! frame, and the editing invariants (`fuzz::editing`). What's thc's plugs into it:
//!
//! - the vault: generated pages with nested, numbered and lettered lists, paragraphs with
//!   children, tasks and links, built once per case and copied fresh for every screen `make`s
//!   (with ids derived under THC_FIXTURE_IDS, so a replay's notes get the same ids);
//! - thc's keys and combos: views, Pages and Enter, the list and sidebar keys, ⌥L, the Mac
//!   editing set and its ⌃/⌥ twins, capture syntax typed out, ⌘A ⌫ ⌘Z;
//! - an agent writing from outside: a host act (`host "agent:add:0:0"`) whose handler writes
//!   to the vault and polls the host, as the daemon's news would;
//! - thc's properties, as checks after every act: the vault round trip (every page an editor
//!   shows reads back from a fresh host as the editor holds it, list styles and all), ids kept
//!   (a note whose text is still there keeps its id), the focus on a shown node, and EI12 (⌘A ⌫
//!   empties a document, ⌘Z brings it all back);
//! - a failure's class (a round trip's kind), which shrinking keeps to.
//!
//! Replay runs on every 4th case (`THC_FUZZ_REPLAY_EVERY`). A case with agent writes runs
//! without fictty's editing checks: an outside change is not the person's to undo.
//!
//! `cargo test -p thc-screen --test fuzz` runs 64 cases from a fixed seed. A long survey:
//! `THC_FUZZ_KEEP_GOING=1 THC_FUZZ_CASES=5000 THC_FUZZ_SEED=7 cargo test -p thc-screen --release
//! --test fuzz thc_screen_holds -- --nocapture`. Failures already found and still open are in
//! tests/fuzz_regressions.json (each with the `pattern` of its message): a run counts them and
//! goes on, and only a new one is shrunk (fictty's `run`) and stops the run. Other knobs:
//! `THC_FUZZ_STEPS` (acts in a case, 40), `THC_FUZZ_BUDGET_MS` (2000), `THC_FUZZ_TERMINAL=0`,
//! `THC_FUZZ_FROM` (start at a case). `THC_FUZZ_CASE=@file` (or the JSON itself: `{"spec": …,
//! "acts": "key a; …", "agent": true}`) traces one case act by act (`trace_one_case`).
//!
//! The vaults, keys, agent writes and round-trip kinds are in tests/fuzz_parts.

use fictty::fuzz::{self, Act, Check, Step};
use fictty::state::Msg;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;
use thc_screen::{Route, ThcHost, View};

mod fuzz_parts;
use fuzz_parts::*;

/// A case: its vault (built once, the template) and the screen fuzzing works on now (each `make`
/// copies the template, so every screen starts from the same vault with the same ids).
struct Ctx {
    spec: VaultSpec,
    template: PathBuf,
    pages: Vec<String>,
    root: Option<PathBuf>,
    host: Option<ThcHost>,
}

type Shared = Rc<RefCell<Ctx>>;

impl Ctx {
    fn new(spec: VaultSpec) -> Shared {
        let (template, pages) = build(&spec);
        Rc::new(RefCell::new(Ctx { spec, template, pages, root: None, host: None }))
    }

    fn root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(|| self.template.clone())
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.template);
        if let Some(r) = &self.root {
            let _ = std::fs::remove_dir_all(r);
        }
    }
}

/// fictty's `make`: a fresh copy of the case's vault, a host on it, and the screen, whose host
/// acts are an agent's writes to that vault.
fn maker(ctx: &Shared) -> impl Fn() -> fictty::Headless + use<> {
    let ctx = ctx.clone();
    move || {
        let mut c = ctx.borrow_mut();
        if let Some(old) = c.root.take() {
            let _ = std::fs::remove_dir_all(old);
        }
        let root = scratch();
        copy_dir(&c.template, &root);
        let host = ThcHost::new(open(&root, human())).at(now());
        let route = match c.spec.start_page {
            Some(p) => Route { view: View::Page, id: Some(c.pages[p as usize % c.pages.len()].clone()), ..Route::default() },
            None => Route::to(View::Today),
        };
        let (w, h) = c.spec.size();
        let (vault, pages, h2) = (root.clone(), c.pages.clone(), host.clone());
        let s = host.app(&route, "ember-dark").headless(w, h).unwrap().on_host_act(move |s, arg| {
            agent_write_vault(&vault, &pages, arg);
            h2.poll(false);
            // The screen takes the host's news now, as a running one would before the next key.
            let now_ms = s.state().now_ms;
            s.msg(Msg::Tick { now_ms });
        });
        c.root = Some(root);
        c.host = Some(host);
        s
    }
}

// ---- thc's properties -----------------------------------------------------------------------------

/// Every page shown in an editor now, with its text.
fn shown_pages(s: &mut fictty::Headless) -> BTreeMap<String, String> {
    let st = s.state();
    let mut out = BTreeMap::new();
    for id in fictty::state::focus_order(st) {
        if let Some((key, _)) = fictty::state::field_view(st, &id)
            && key.starts_with("page/")
            && let Some(t) = st.store.get(&key).and_then(|e| e.read()["text"].as_str().map(String::from))
        {
            out.insert(key, t);
        }
    }
    out
}

fn key_of(a: &Act) -> &str {
    match a {
        Act::Key(k) => k,
        _ => "",
    }
}

/// thc's checks.
fn thc_checks(ctx: &Shared) -> Vec<Check> {
    let focus = fuzz::check("the focus is on a shown node", |s: &mut Step| {
        let st = s.screen.state();
        match st.focus.clone() {
            Some(f) if !fictty::state::focus_order(st).contains(&f) => Err(format!("focus: on {f:?}, which isn't shown")),
            _ => Ok(()),
        }
    });

    // 2 and 4: every page an editor shows reads back from a fresh host as it holds it; and
    // what the person typed is what was saved: a save that sent the editor back another text
    // (the host does, so the editor shows what the vault holds) is a round trip that failed,
    // unless it merged in an outside change.
    let c = ctx.clone();
    let round_trip = fuzz::check("the vault round trip", move |s: &mut Step| {
        let rewrites = c.borrow().host.as_ref().map(|h| h.take_rewrites()).unwrap_or_default();
        for (key, held, given) in rewrites {
            let (held, given) = (norm(&held), norm(&given));
            let kind = round_trip_kind(&held, &given);
            if held != given && !matches!(kind, "agent-stale" | "box-stale") {
                return Err(format!("round trip [{kind}]: {key}: the editor held\n{held:?}\nbut the save rewrote it to\n{given:?}"));
            }
        }
        let root = c.borrow().root();
        for (key, text) in shown_pages(s.screen) {
            let fresh = ThcHost::new(open(&root, human())).at(now());
            let saved = fresh.load_doc(&key).map_err(|e| format!("round trip: {key} won't load: {e:#}"))?;
            if norm(&saved) != norm(&text) {
                let kind = round_trip_kind(&norm(&text), &norm(&saved));
                return Err(format!("round trip [{kind}]: {key}: the editor holds\n{text:?}\nbut a fresh load gives\n{saved:?}"));
            }
        }
        Ok(())
    });

    // 3: the notes an act doesn't delete keep their ids: a note whose text is still there (once)
    // must still be under its id. An agent's write may delete and add as it likes.
    let c = ctx.clone();
    let prev: Rc<RefCell<Option<Vec<(String, String)>>>> = Rc::default();
    let reset = prev.clone();
    let ids = fuzz::check("ids kept", move |s: &mut Step| {
        let (root, template, pages) = {
            let c = c.borrow();
            (c.root(), c.template.clone(), c.pages.clone())
        };
        let after = notes_in_order(&root, &pages);
        let before = prev.borrow_mut().replace(after.clone()).unwrap_or_else(|| notes_in_order(&template, &pages));
        if matches!(s.act, Act::Host(_)) {
            return Ok(());
        }
        let by_text = |l: &[(String, String)]| {
            let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (id, t) in l {
                m.entry(t.clone()).or_default().push(id.clone());
            }
            m
        };
        let (b, a) = (by_text(&before), by_text(&after));
        for (t, was) in &b {
            if was.len() != 1 || t.trim().is_empty() {
                continue;
            }
            if let Some(now_ids) = a.get(t)
                && now_ids.len() == 1
                && now_ids[0] != was[0]
                && !after.iter().any(|(id, _)| id == &was[0])
            {
                return Err(format!("ids: {t:?} was {} and is now {} (the old one is gone)", was[0], now_ids[0]));
            }
        }
        Ok(())
    })
    .on_reset(move || *reset.borrow_mut() = None);

    // EI12: ⌘A ⌫ leaves an empty document, and ⌘Z brings it all back (in a writable editor that
    // kept the focus).
    let looks: Rc<RefCell<Vec<fuzz::Look>>> = Rc::default();
    let reset = looks.clone();
    let ei12 = fuzz::check("EI12", move |s: &mut Step| {
        let mut l = looks.borrow_mut();
        if l.is_empty() {
            l.push(s.before.clone());
        }
        l.push(s.after.clone());
        if l.len() > 4 {
            l.remove(0);
        }
        let n = s.acts.len();
        if n < 3 || l.len() < 4 || s.acts[n - 3..].iter().map(key_of).collect::<Vec<_>>() != ["d-a", "bs", "d-z"] {
            return Ok(());
        }
        let (was, gone, after) = (&l[0], &l[2], &l[3]);
        let Some((id, text, _, _)) = was.focused() else { return Ok(()) };
        let writable = !s.screen.state().tree().find(id).and_then(|n| n.field()).is_some_and(|f| f.readonly);
        if !writable || text.trim().is_empty() || gone.focus() != Some(id) || after.focus() != Some(id) || !was.same_doc(gone, id) || !was.same_doc(after, id) {
            return Ok(());
        }
        let gone_text = gone.editors[id].0.clone();
        if !gone_text.trim().is_empty() {
            return Err(format!("EI12: ⌘A ⌫ left {gone_text:?}"));
        }
        let now = &after.editors[id].0;
        if now != text {
            return Err(format!("EI12: ⌘Z after ⌘A ⌫: want {text:?}, got {now:?}"));
        }
        Ok(())
    })
    .on_reset(move || reset.borrow_mut().clear());

    vec![focus, round_trip, ids, ei12]
}

/// The checks a case runs: thc's, and fictty's editing invariants when no agent writes.
fn checks(ctx: &Shared, agent: bool) -> Vec<Check> {
    let mut out = thc_checks(ctx);
    if !agent {
        out.extend(fuzz::editing());
    }
    out
}

// ---- running cases --------------------------------------------------------------------------------

/// A failure's one line for matching and counting: the property and its message.
fn summary(f: &fuzz::Failure) -> String {
    format!("{}: {}", f.property, f.message)
}

/// A failure's class, for counting: the property and, for a round trip, its kind.
fn failure_class(f: &fuzz::Failure) -> String {
    class(&f.property, &f.message)
}

/// The failures already found and still open (fuzz_regressions.json entries with a `pattern`).
fn open_patterns() -> Vec<(String, String)> {
    regressions()
        .iter()
        .filter(|r| r["open"].is_string())
        .filter_map(|r| Some((r["pattern"].as_str()?.to_string(), r["name"].as_str().unwrap_or("?").to_string())))
        .collect()
}

fn regressions() -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fuzz_regressions.json");
    std::fs::read_to_string(path).ok().and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default()
}

/// A case as fuzz_regressions.json keeps it.
fn case_json(spec: &VaultSpec, agent: bool, acts: &[Act]) -> Value {
    json!({"spec": spec, "agent": agent, "acts": fuzz::line(acts)})
}

/// Play a case's acts (`fuzz::replay`: no shrinking): the failure, if any.
fn play(spec: &VaultSpec, agent: bool, replay: bool, acts: &str) -> Result<(), fuzz::Failure> {
    let ctx = Ctx::new(spec.clone());
    let mut checks = checks(&ctx, agent);
    fuzz::replay(maker(&ctx), acts, &mut checks, &config(0, agent, replay))
}

fn drive(cases: u64) {
    let known = open_patterns();
    let keep_going = env_num("THC_FUZZ_KEEP_GOING", 0) == 1;
    let base = env_num("THC_FUZZ_SEED", 1);
    let replay_every = env_num("THC_FUZZ_REPLAY_EVERY", 4);
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut fresh: BTreeMap<String, (usize, String, Value)> = BTreeMap::new();
    let mut first_new: Option<String> = None;
    let started = Instant::now();
    let mut n = 0;
    for i in env_num("THC_FUZZ_FROM", 0)..cases {
        n += 1;
        // A case: its vault from its seed, and a third of them with an agent writing.
        let seed = base.wrapping_mul(1_000_000).wrapping_add(i);
        let spec = gen_vault(seed);
        let agent = i % 3 == 2;
        let replay = replay_every > 0 && i % replay_every == 0;
        let cfg = config(seed, agent, replay);
        let acts = fuzz::generate(&cfg, seed.wrapping_mul(1_000_003), spec.size());
        let Err(f) = play(&spec, agent, replay, &fuzz::line(&acts)) else { continue };
        let line = summary(&f);
        if let Some((_, name)) = known.iter().find(|(p, _)| line.contains(p.as_str())) {
            *seen.entry(name.clone()).or_default() += 1;
            continue;
        }
        if keep_going {
            let e = fresh.entry(failure_class(&f)).or_insert_with(|| (0, format!("(case {i}) {line}"), case_json(&spec, agent, &acts)));
            e.0 += 1;
            continue;
        }
        // A new one: fictty's run generates the same acts (its first run's seed is
        // `seed × 1 000 003`) and shrinks them, keeping to the failure's class.
        let ctx = Ctx::new(spec.clone());
        let mut checks = checks(&ctx, agent);
        let shrunk = match fuzz::run(maker(&ctx), &mut checks, &cfg) {
            Err(s) => s,
            Ok(()) => f,
        };
        first_new = Some(format!(
            "{shrunk}\n\nThe case, for crates/thc-screen/tests/fuzz_regressions.json (with a name, owner, `open` and the `pattern` of its message):\n{}",
            serde_json::to_string(&case_json(&spec, agent, &shrunk.acts)).unwrap()
        ));
        break;
    }
    let mins = started.elapsed().as_secs_f64() / 60.0;
    eprintln!("thc screen fuzz: {n} cases run in {mins:.1} min ({:.0} a minute)", n as f64 / mins.max(1e-9));
    for (name, count) in &seen {
        eprintln!("   known, still open: {name} hit {count} times");
    }
    for (class, (count, first, case)) in &fresh {
        eprintln!("   NEW: {class} hit {count} times; the first:\n      {}\n      case: {}", first.lines().take(6).collect::<Vec<_>>().join("\n      "), serde_json::to_string(case).unwrap());
    }
    assert!(fresh.is_empty(), "the survey found new failures (above)");
    if let Some(f) = first_new {
        panic!("thc screen fuzz: a new failure, shrunk:\n{f}");
    }
}

#[test]
fn thc_screen_holds_its_properties_under_random_use() {
    drive(env_num("THC_FUZZ_CASES", 64));
}

/// `THC_FUZZ_CASE=@file` or `THC_FUZZ_CASE='{"spec": …, "acts": "…", "agent": …}'`: one case,
/// act by act, with the focused editor and the frame after each, then its verdict.
#[test]
fn trace_one_case() {
    let Ok(raw) = std::env::var("THC_FUZZ_CASE") else { return };
    let raw = match raw.strip_prefix('@') {
        Some(f) => std::fs::read_to_string(f).expect("THC_FUZZ_CASE=@file"),
        None => raw,
    };
    let case: Value = serde_json::from_str(&raw).expect("THC_FUZZ_CASE is a case's JSON");
    let spec: VaultSpec = serde_json::from_value(case["spec"].clone()).unwrap();
    let line = case["acts"].as_str().unwrap_or("");
    let ctx = Ctx::new(spec.clone());
    let mut s = maker(&ctx)();
    eprintln!("--- start: {:?}\n{}", fuzz::focused_golden(&mut s), s.screen());
    for act in fuzz::parse_line(line).unwrap() {
        act.apply(&mut s);
        eprintln!("--- {act}: {:?}\n{}", fuzz::focused_golden(&mut s), s.screen());
    }
    // Where a second screen's state differs (what fictty's replay compares).
    let state = |s: &mut fictty::Headless| {
        let mut v = serde_json::to_value(s.state()).unwrap();
        v.as_object_mut().map(|o| o.remove("stats"));
        v
    };
    let first = state(&mut s);
    let mut again = maker(&ctx)();
    for act in fuzz::parse_line(line).unwrap() {
        act.apply(&mut again);
    }
    let second = state(&mut again);
    fn diff(a: &Value, b: &Value, at: String, out: &mut Vec<String>) {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                for k in x.keys().chain(y.keys()).collect::<std::collections::BTreeSet<_>>() {
                    diff(x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null), format!("{at}.{k}"), out);
                }
            }
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
                for (i, (p, q)) in x.iter().zip(y).enumerate() {
                    diff(p, q, format!("{at}[{i}]"), out);
                }
            }
            _ if a != b => out.push(format!("{at}: {} vs {}", a.to_string().chars().take(200).collect::<String>(), b.to_string().chars().take(200).collect::<String>())),
            _ => {}
        }
    }
    let mut out = Vec::new();
    diff(&first, &second, String::new(), &mut out);
    eprintln!("--- a second screen's state differs at {} places:\n{}", out.len(), out.iter().take(20).cloned().collect::<Vec<_>>().join("\n"));
    drop(s);
    drop(again);
    drop(ctx);
    match play(&spec, case["agent"].as_bool().unwrap_or(false), true, line) {
        Ok(()) => eprintln!("--- passes"),
        Err(f) => eprintln!("--- fails:\n{f}"),
    }
}

/// Every case the fuzzer found (crates/thc-screen/tests/fuzz_regressions.json), replayed. A
/// regression still failing is listed with its owner and why: the test fails only when one fails
/// differently or when a listed one starts passing (then take it off the list).
#[test]
fn the_fuzzers_regressions_replay() {
    let mut bad = Vec::new();
    for r in regressions() {
        let spec: VaultSpec = serde_json::from_value(r["case"]["spec"].clone()).unwrap();
        let got = play(&spec, r["case"]["agent"].as_bool().unwrap_or(false), true, r["case"]["acts"].as_str().unwrap_or(""));
        match (got, r["open"].as_str(), r["pattern"].as_str()) {
            // A flaky one (`"flaky": true`) passes some runs.
            (Ok(()), Some(issue), _) if r["flaky"] != true => bad.push(format!("{}: passes now; mark it fixed ({issue})", r["name"])),
            (Err(f), Some(_), Some(p)) if !summary(&f).contains(p) => bad.push(format!("{}: fails differently:\n{f}", r["name"])),
            (Err(f), None, _) => bad.push(format!("{}: fails again:\n{f}", r["name"])),
            _ => {}
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// An agent's write that lands while the person's typing isn't saved yet (the pty fuzz's
/// reload-while-unsaved): the save merges the typing into the vault, and the editor shows the
/// merge. Headless saves at once, so the agent writes to the vault before the host hears of it
/// (no poll), and the person's next key is the save that meets it.
#[test]
fn a_save_that_merges_an_outside_write_shows_the_merge() {
    let spec = VaultSpec { pages: vec![vec![NoteSpec { kind: Kind::Bullet, words: vec![0], link: None, children: vec![] }]], start_page: Some(0), size: 0 };
    let ctx = Ctx::new(spec);
    let mut s = maker(&ctx)();
    let (root, pages) = (ctx.borrow().root(), ctx.borrow().pages.clone());
    s.keys(&["d-down"]);
    // The agent's note goes to the vault; the screen hasn't heard.
    agent_write_vault(&root, &pages, "agent:add:0:1");
    s.keys(&["y", "o"]);
    let text = shown_pages(&mut s).into_values().next().unwrap_or_default();
    assert!(text.contains("agent beta") && text.contains("yo"), "the editor holds {text:?}");
    let fresh = ThcHost::new(open(&root, human())).at(now()).load_doc(&format!("page/{}", pages[0])).unwrap();
    assert_eq!(norm(&fresh), norm(&text));
}

